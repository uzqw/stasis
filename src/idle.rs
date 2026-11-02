//! Hold the desktop's own idle chain off while a rest is locked.
//!
//! Locking grabs evdev, so the compositor stops seeing input and its idle
//! timers fire exactly when the rest starts: the screen blanks and the lock
//! screen hides the countdown.  `keepawake` only speaks the freedesktop D-Bus
//! ScreenSaver interface and logind; Hyprland/Quickshell ignore both, so on
//! Omarchy the screen still went dark.
//!
//! Omarchy's shell keeps the same decision in a state file and watches its
//! directory, so a rest holds the screen on by creating that file and drops
//! it on unlock.  The file belongs to the user: Stasis only removes what it
//! created.  A sentinel in the Stasis state directory records that ownership
//! across daemon restarts, so a crash mid-rest is cleaned up on the next
//! start instead of pinning the screen on forever.

use std::fs;
use std::path::PathBuf;

/// Relative to `$HOME`; overridable with `STASIS_STAY_AWAKE_FILE`.
pub const DEFAULT_FILE: &str = ".local/state/omarchy/indicators/stay-awake";
pub const SENTINEL: &str = "stay-awake-owned";

pub struct StayAwake {
    file: Option<PathBuf>,
    sentinel: Option<PathBuf>,
    owned: bool,
}

impl StayAwake {
    pub fn new() -> Self {
        #[cfg(target_os = "linux")]
        {
            let file = std::env::var_os("STASIS_STAY_AWAKE_FILE")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(DEFAULT_FILE))
                });
            let sentinel = Some(crate::config::Config::state_dir().join(SENTINEL));
            let owned = file.as_ref().is_some_and(|f| f.exists())
                && sentinel.as_ref().is_some_and(|s| s.exists());
            Self {
                file,
                sentinel,
                owned,
            }
        }
        #[cfg(not(target_os = "linux"))]
        Self {
            file: None,
            sentinel: None,
            owned: false,
        }
    }

    /// Create the marker while a rest is locked, remove it afterwards.
    pub fn sync(&mut self, hold: bool) {
        let (Some(file), Some(sentinel)) = (&self.file, &self.sentinel) else {
            return;
        };
        if hold {
            if file.exists() {
                // The user may have enabled stay-awake themselves; only a
                // sentinel from an earlier Stasis run proves we own it.
                self.owned = self.owned || sentinel.exists();
                return;
            }
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if fs::write(file, b"stasis rest\n").is_err() {
                return;
            }
            if let Some(dir) = sentinel.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(sentinel, b"");
            self.owned = true;
        } else if self.owned {
            let _ = fs::remove_file(file);
            let _ = fs::remove_file(sentinel);
            self.owned = false;
        } else {
            // Stale sentinel from a run that no longer owns the file.
            let _ = fs::remove_file(sentinel);
        }
    }
}

impl Default for StayAwake {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(dir: &std::path::Path) -> StayAwake {
        StayAwake {
            file: Some(dir.join("stay-awake")),
            sentinel: Some(dir.join("stay-awake-owned")),
            owned: false,
        }
    }

    #[test]
    fn creates_and_releases_its_own_marker() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = guard(dir.path());
        g.sync(true);
        assert!(dir.path().join("stay-awake").exists());
        assert!(dir.path().join("stay-awake-owned").exists());
        g.sync(false);
        assert!(!dir.path().join("stay-awake").exists());
        assert!(!dir.path().join("stay-awake-owned").exists());
    }

    #[test]
    fn leaves_a_user_enabled_marker_alone() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("stay-awake"), b"").unwrap();
        let mut g = guard(dir.path());
        g.sync(true);
        g.sync(false);
        assert!(dir.path().join("stay-awake").exists());
        assert!(!dir.path().join("stay-awake-owned").exists());
    }

    #[test]
    fn a_restart_adopts_a_marker_it_created_before() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("stay-awake"), b"").unwrap();
        fs::write(dir.path().join("stay-awake-owned"), b"").unwrap();
        let mut g = StayAwake {
            owned: true,
            ..guard(dir.path())
        };
        g.sync(true);
        assert!(dir.path().join("stay-awake").exists());
        g.sync(false);
        assert!(!dir.path().join("stay-awake").exists());
        assert!(!dir.path().join("stay-awake-owned").exists());
    }

    #[test]
    fn releases_a_stale_marker_on_startup() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("stay-awake"), b"").unwrap();
        fs::write(dir.path().join("stay-awake-owned"), b"").unwrap();
        let mut g = StayAwake {
            owned: true,
            ..guard(dir.path())
        };
        // The rest window is over: the first status publish releases it.
        g.sync(false);
        assert!(!dir.path().join("stay-awake").exists());
    }

    #[test]
    fn without_a_home_there_is_nothing_to_do() {
        let mut g = StayAwake {
            file: None,
            sentinel: None,
            owned: false,
        };
        g.sync(true);
        g.sync(false);
    }
}
