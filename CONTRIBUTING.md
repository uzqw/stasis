# 提交与协作规范（Contributing）

核心原则：**提交可追溯、守卫可执行、规范只有一份事实源**。

本项目提交信息**统一使用英文**（type、动词、摘要、正文子项）；文档与注释可以用中文。

## 一、提交信息规范（Commit Message）

采用 **Conventional Commits**。格式：

```text
<type>(<scope>): <summary>

- <verb> ...
- <verb> ...

Generated-by: pi (<provider>/<model>)
```

- `type` 只能取：`feat`、`fix`、`refactor`、`docs`、`test`、`chore`、`perf`、`build`、`revert`。
- `scope` 用模块名：`core`、`keypad`、`protocol`、`engine`、`platform`、`linux`、`windows`、
  `macos`、`ui`、`config`、`docs`、`tools`、`ci`；跨模块用 `*` 或省略。
- 摘要与每个正文子项的**首词必须是动词**，且**只能从动词表选**（闭合清单）。
- 摘要是祈使句、一句话讲清做了什么，**≤50 字符**（不含 `type(scope):` 前缀）。
- 正文空一行后用 `-` 列出具体变更，每个子项以动词开头，**每行 ≤72 字符**。
- 关联 Issue 在标题尾部引用：`fix(protocol): correct stale ack handling (#12)`。
- AI 生成的提交：纯文本，不要 Markdown 代码块，不要多余解释。

示例：

```text
fix(keypad): reject duplicated caps presses

- filter key repeat before counting the unlock gesture
- reset the password buffer when unlock mode is cancelled

Generated-by: pi (anthropic/claude-sonnet-4-5)
```

### 动词表与机器强制

- **唯一事实源**：`.githooks/commit-lint.txt`（`type` 与 `verb` 两张表，逐行一条）。
- `.githooks/commit-msg` 读取该表校验标题与正文子项，首词不在表内、type 非法、非英文、
  摘要超 50 字符、子项超 72 字符都会让 `git commit` 直接失败。
- 改动词只改 `commit-lint.txt`；不要把表复制进钩子，也不要在别处维护第二份。
- `git commit --no-verify` 是唯一逃生口，只用于守卫本身出问题，不用于绕过违规。

### commit 类型对照（标签/Issue 风格）

| type | 用途 |
| --- | --- |
| feat | 新功能、新能力 |
| fix | 缺陷、安全、可靠性修复 |
| refactor | 行为不变的重构 |
| docs | 文档、规范、注释 |
| test | 测试与测试基建 |
| chore | 构建脚本、依赖、提交卫生 |
| perf | 性能优化 |
| build | 打包、CI、发布产物 |
| revert | 回滚提交 |

## 二、分支与工作副本命名

```text
分支：   <type>/<slug>                      例：docs/commit-guards
工作副本：stasis-<type>-<slug>               例：stasis-docs-commit-guards
```

- `<type>` 与提交 type 一致；`<slug>` 简短英文小写加连字符。
- 有 Issue 编号时带上：`fix/12-stale-ack`。
- 历史命名不强制迁移；新分支一律按此规则。

## 三、文档规范

文档规则写在 [AGENTS.md](AGENTS.md)「文档怎么写」，由 `.githooks/pre-commit` 机器强制（相对链接
存在、代码块标注语言、正文视觉宽度 ≤100）。文档结构索引见 [docs/README.md](docs/README.md)。

## 四、AI 生成内容声明（Attribution）

- **责任人原则**：每笔改动的评审与合并决定由人做出。
- 关键内容由生成式工具实质性生成时（语法补全、格式化、翻译、拼写纠正不算），提交末尾加 Git
  trailer：

```text
Generated-by: <tool> (<provider>/<model>)
```

  例：`Generated-by: pi (anthropic/claude-sonnet-4-5)`。pi 里模型信息读环境变量 `$PI_PROVIDER`、
  `$PI_MODEL`，取不到就只写工具名。amend 或 squash 后 trailer 必须保留。

## 五、本地自查（提交/推送前）

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --quiet --manifest-path tools/guard/Cargo.toml        # 全量文档守卫
```

- 尚未有可编译 crate 时，至少跑守卫，并如实说明未验证项。
- 本地跑不了的检查要写明原因与风险，**不得声称验证通过**。
- 涉及真实输入抓取的测试只在显式授权的桌面执行，且必须先准备恢复通道（见 AGENTS.md
  「平台安全」）。

## 六、本文件的维护

本文件是提交/分支/PR 规范的说明性事实源；机器可执行的 type 与动词表在
`.githooks/commit-lint.txt`。修改规则时两处一起改，并把变更写进提交正文子项。
