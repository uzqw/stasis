use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const APP_NAME: &str = "stasis";
const CONFIG_NAME: &str = "config.json";

/// Minimum portable configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Unlock password.  Plaintext on disk is intentional parity with old app.
    /// File permissions are tightened on save.
    pub password: String,
    /// Directory where aide writes plans and events.
    /// Defaults to ~/Downloads for compatibility.
    #[serde(default = "default_events_dir")]
    pub events_dir: PathBuf,
    /// Override the default data directory (logs, config).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            password: String::new(),
            events_dir: default_events_dir(),
            data_dir: None,
        }
    }
}

impl Config {
    /// Load from the standard config path.  If missing, return default.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)?;
        let cfg: Self = serde_json::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.validate()?;
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&tmp)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&tmp, perms)?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn config_path() -> anyhow::Result<PathBuf> {
        if let Ok(v) = std::env::var("STASIS_CONFIG") {
            return Ok(PathBuf::from(v));
        }
        let dir = data_dir()?;
        Ok(dir.join(CONFIG_NAME))
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.password.is_empty() {
            anyhow::bail!("password must not be empty");
        }
        if self.password.len() > 64 {
            anyhow::bail!("password too long (max 64)");
        }
        if !self.password.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("password must be ASCII letters or digits");
        }
        Ok(())
    }

    /// Effective events directory.
    pub fn effective_events_dir(&self) -> PathBuf {
        std::env::var("STASIS_EVENTS_DIR")
            .or_else(|_| std::env::var("INPUT_LOCKER_EVENTS_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| self.events_dir.clone())
    }
}

fn data_dir() -> io::Result<PathBuf> {
    if let Ok(v) = std::env::var("STASIS_DATA_DIR") {
        return Ok(PathBuf::from(v));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return Ok(PathBuf::from(appdata).join(APP_NAME));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home).join("Library/Application Support").join(APP_NAME));
        }
    }
    // Linux / generic
    if let Some(home) = std::env::var_os("HOME") {
        let base = PathBuf::from(home);
        if let Some(cfg) = std::env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(cfg).join(APP_NAME));
        }
        return Ok(base.join(".config").join(APP_NAME));
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "no home directory"))
}

fn default_events_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Downloads").join("input-locker-events");
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(profile).join("Downloads").join("input-locker-events");
    }
    PathBuf::from("input-locker-events")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn validate_password_rules() {
        let mut cfg = Config::default();
        cfg.password = "abc123".into();
        assert!(cfg.validate().is_ok());

        cfg.password = "".into();
        assert!(cfg.validate().is_err());

        cfg.password = "abc!".into();
        assert!(cfg.validate().is_err());

        cfg.password = "a".repeat(65);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn roundtrip_json() {
        let mut cfg = Config::default();
        cfg.password = "hello99".into();
        let json = serde_json::to_string(&cfg).unwrap();
        let loaded: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.password, "hello99");
    }

    #[test]
    fn load_missing_returns_default() {
        // Does not panic
        let _ = Config::default();
    }
}
