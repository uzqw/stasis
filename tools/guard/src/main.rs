//! Repository guards for Stasis: Markdown rules, line width, credential policy.
//!
//! Usage:
//!
//! ```text
//! guard --staged        # check staged files (called by .githooks/pre-commit)
//! guard <path>...       # check explicit paths
//! guard                 # check docs/ and root *.md
//! ```
//!
//! Rules live in AGENTS.md ("文档怎么写", "Guardrails"); this binary is their
//! executable copy. No external crates: the guard must run before any
//! dependency exists.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Visual width limit for prose and source lines (CJK counts as two columns).
const MAX_WIDTH: usize = 100;
/// Files larger than this are not scanned for credential markers.
const MAX_SCAN_BYTES: u64 = 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let staged = args.iter().any(|a| a == "--staged");
    let explicit: Vec<String> = args.into_iter().filter(|a| !a.starts_with("--")).collect();

    let files: Vec<PathBuf> = if staged {
        match staged_files() {
            Ok(files) => files,
            Err(err) => {
                eprintln!("✗ 仓库守卫无法读取暂存区：{err}");
                std::process::exit(1);
            }
        }
    } else if explicit.is_empty() {
        default_files()
    } else {
        explicit.into_iter().map(PathBuf::from).collect()
    };

    let mut violations: Vec<String> = Vec::new();
    for path in &files {
        violations.extend(check(path, staged));
    }

    if violations.is_empty() {
        return;
    }
    for violation in &violations {
        eprintln!("{violation}");
    }
    eprintln!(
        "✗ 仓库守卫发现 {} 处问题，请修复后重新 git add 再提交（规则见 AGENTS.md）。",
        violations.len()
    );
    std::process::exit(1);
}

/// `git diff --cached --name-only` for added/copied/modified/renamed files.
fn staged_files() -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args([
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=ACMR",
        ])
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Default scope when no path is given: docs/ plus root Markdown files.
fn default_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_markdown(Path::new("docs"), &mut files);
    if let Ok(entries) = fs::read_dir(".") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_markdown(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, out);
        } else if is_markdown(&path) {
            out.push(path);
        }
    }
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("md")
}

/// Run every guard that applies to one file; returns `path:line: message` items.
fn check(path: &Path, staged: bool) -> Vec<String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if let Some(reason) = policy_name(name) {
        return vec![format!("{}: {reason}", path.display())];
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut violations = Vec::new();
    match ext.as_str() {
        "md" => violations.extend(check_markdown(path)),
        "rs" | "py" | "sh" | "toml" | "yaml" | "yml" => {
            violations.extend(check_lines(path, staged));
        }
        _ => {}
    }
    violations.extend(check_credentials(path));
    violations
}

/// Guardrail: never commit credential files (AGENTS.md "Guardrails").
fn policy_name(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    let hit = name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".pfx")
        || name.ends_with(".p12")
        || name == "id_rsa"
        || name == "id_ed25519"
        || name.starts_with("credentials");
    hit.then_some("疑似凭据文件，禁止提交（Guardrails 见 AGENTS.md）")
}

/// Guardrail: no private key material inside a committed text file.
fn check_credentials(path: &Path) -> Vec<String> {
    // This file defines the markers, so it necessarily contains them; a scanner
    // cannot be its own subject. Everything else is scanned.
    if path.to_string_lossy().ends_with(file!()) {
        return Vec::new();
    }
    if fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_SCAN_BYTES {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    // Full PEM armor: matching only the phrase would flag documentation
    // and this scanner itself.
    for marker in [
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----",
        "-----BEGIN PRIVATE KEY-----",
    ] {
        if text.contains(marker) {
            return vec![format!(
                "{}: 含私钥内容（{marker}），禁止提交（Guardrails 见 AGENTS.md）",
                path.display()
            )];
        }
    }
    Vec::new()
}

/// Markdown guard: link targets exist, fences are labelled, prose width <= 100.
fn check_markdown(path: &Path) -> Vec<String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => return vec![format!("{}: 读取失败: {err}", path.display())],
    };
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut violations = Vec::new();
    let mut in_fence = false;

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if in_fence {
            if is_fence_close(trimmed) {
                in_fence = false;
            }
            continue;
        }
        if is_fence_open(trimmed) {
            if fence_info(trimmed).is_empty() {
                violations.push(format!(
                    "{}:{number}: 代码块未标注语言（如 ```rust、```sh）",
                    path.display()
                ));
            }
            in_fence = true;
            continue;
        }

        let width = display_width(line);
        if width > MAX_WIDTH && !trimmed.starts_with('|') && !is_single_token(line) {
            violations.push(format!(
                "{}:{number}: 正文行视觉宽度 {width} 超过 {MAX_WIDTH}（表格/长 URL 豁免）",
                path.display()
            ));
        }
        for target in links(line) {
            if let Some(message) = check_link(dir, &target) {
                violations.push(format!("{}:{number}: {message}", path.display()));
            }
        }
    }
    violations
}

/// Line-width guard: staged added lines only, so legacy debt does not block.
fn check_lines(path: &Path, staged: bool) -> Vec<String> {
    let lines = if staged {
        added_lines(path)
    } else {
        read_lines(path)
    };
    let mut violations = Vec::new();
    for (number, line) in lines {
        let width = display_width(&line);
        if width > MAX_WIDTH {
            violations.push(format!(
                "{}:{number}: 代码行视觉宽度 {width} 超过 {MAX_WIDTH}，请换行",
                path.display()
            ));
        }
    }
    violations
}

/// Added lines of a staged file with their line numbers in the new version.
fn added_lines(path: &Path) -> Vec<(usize, String)> {
    let output = Command::new("git")
        .args(["diff", "--cached", "-U0", "--"])
        .arg(path)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut current: usize = 0;
    for raw in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(rest) = raw.strip_prefix("@@ ") {
            current = hunk_start(rest);
            continue;
        }
        if raw.starts_with("+++") || raw.starts_with("---") {
            continue;
        }
        if let Some(text) = raw.strip_prefix('+') {
            lines.push((current, text.to_string()));
            current += 1;
        } else if raw.starts_with(' ') {
            current += 1;
        }
    }
    lines
}

/// Parse `-1,2 +10,3 @@` and return the new-file start line (10).
fn hunk_start(header: &str) -> usize {
    let Some(plus) = header.find('+') else {
        return 0;
    };
    let rest = &header[plus + 1..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().unwrap_or(0)
}

fn read_lines(path: &Path) -> Vec<(usize, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.to_string()))
        .collect()
}

/// Relative link targets on one line, `[text](target)` form.
fn links(line: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            break;
        };
        targets.push(after[..end].to_string());
        rest = &after[end..];
    }
    targets
}

/// Empty string means the target is fine.
fn check_link(dir: &Path, target: &str) -> Option<String> {
    let target = target.trim();
    if target.is_empty()
        || target.starts_with('#')
        || target.contains(char::is_whitespace)
        || target.starts_with("mailto:")
    {
        return None;
    }
    if let Some(index) = target.find("://") {
        if is_scheme(&target[..index]) {
            return None;
        }
    }
    let path = target.split('#').next().unwrap_or(target);
    if path.is_empty() {
        return None;
    }
    if dir.join(path).exists() {
        None
    } else {
        Some(format!("相对链接目标不存在: {target}"))
    }
}

fn is_scheme(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn is_fence_open(line: &str) -> bool {
    line.starts_with("```") || line.starts_with("~~~")
}

fn is_fence_close(line: &str) -> bool {
    if line.len() < 3 {
        return false;
    }
    let marker = line.chars().next().unwrap_or(' ');
    if marker != '`' && marker != '~' {
        return false;
    }
    line.chars().all(|c| c == marker)
}

fn fence_info(line: &str) -> &str {
    line.trim_start_matches(['`', '~']).trim()
}

/// Single unbreakable token (long URL or command); CJK prose is always breakable.
fn is_single_token(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.contains(' ') {
        return false;
    }
    !trimmed.chars().any(|c| c >= '\u{3000}')
}

/// Visual width: CJK/full-width characters count as two columns.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| {
            let code = c as u32;
            let wide = matches!(code,
                0x1100..=0x115F
                | 0x2E80..=0x303E
                | 0x3041..=0x33FF
                | 0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xA000..=0xA4CF
                | 0xAC00..=0xD7A3
                | 0xF900..=0xFAFF
                | 0xFE30..=0xFE4F
                | 0xFF00..=0xFF60
                | 0xFFE0..=0xFFE6);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_cjk_as_two() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("中文"), 4);
        assert_eq!(display_width("a中"), 3);
    }

    #[test]
    fn fence_parsing() {
        assert!(is_fence_open("```rust"));
        assert!(!is_fence_close("```rust"));
        assert!(is_fence_close("```"));
        assert_eq!(fence_info("```rust"), "rust");
        assert_eq!(fence_info("```"), "");
    }

    #[test]
    fn hunk_start_reads_new_file_line() {
        assert_eq!(hunk_start("-1,2 +10,3 @@"), 10);
        assert_eq!(hunk_start("-1 +1 @@"), 1);
    }

    #[test]
    fn links_extract_targets() {
        let found = links("see [a](docs/a.md) and [b](#x)");
        assert_eq!(found, vec!["docs/a.md", "#x"]);
        assert!(links("no link here").is_empty());
    }

    #[test]
    fn external_and_anchor_links_pass() {
        let dir = Path::new(".");
        assert!(check_link(dir, "https://example.com/a").is_none());
        assert!(check_link(dir, "#section").is_none());
        assert!(check_link(dir, "missing-file.md").is_some());
        assert!(check_link(dir, "Cargo.toml").is_none());
    }

    #[test]
    fn credential_content_is_blocked() {
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nxx\n";
        let path = std::env::temp_dir().join("guard-test-key.txt");
        fs::write(&path, pem).unwrap();
        assert!(!check_credentials(&path).is_empty());
        fs::write(&path, "the phrase BEGIN PRIVATE KEY in prose is fine\n").unwrap();
        assert!(check_credentials(&path).is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn credential_names_are_blocked() {
        assert!(policy_name(".env").is_some());
        assert!(policy_name("id_rsa").is_some());
        assert!(policy_name("server.key").is_some());
        assert!(policy_name("main.rs").is_none());
        assert!(policy_name("docs/design.md").is_none());
    }
}
