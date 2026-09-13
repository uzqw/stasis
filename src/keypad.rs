use std::time::{Duration, Instant};

use crate::keymap::RawKey;

const CAPS_TRIGGER_COUNT: usize = 3;
const CAPS_TRIGGER_WINDOW: Duration = Duration::from_secs(2);
const PASSWORD_MAX_LEN: usize = 64;

/// Actions produced by feeding raw keys into the keypad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// CapsLock trigger entered unlock mode (sticky, cannot be turned off here).
    UnlockMode,
    /// Password buffer changed; current length for UI dots.
    Password { len: usize },
    /// Enter pressed while armed → submit current buffer.
    Submit,
    /// Nothing changed (swallowed key, duplicate, out-of-window, etc.).
    None,
}

/// Shared keypad logic.  One instance per lock session.
///
/// - Tracks CapsLock presses within a 2-second window.
/// - Once armed (`unlock_mode = true`), builds a password buffer.
/// - Shift state is tracked but CapsLock LED is ignored for password input.
/// - Only letters and digits are accepted; everything else is swallowed.
#[derive(Debug, Clone)]
pub struct Keypad {
    caps_times: Vec<Instant>,
    shift_down: bool,
    unlock_mode: bool,
    password: String,
}

impl Keypad {
    pub fn new() -> Self {
        Self {
            caps_times: Vec::with_capacity(CAPS_TRIGGER_COUNT),
            shift_down: false,
            unlock_mode: false,
            password: String::with_capacity(16),
        }
    }

    pub fn reset(&mut self) {
        self.caps_times.clear();
        self.shift_down = false;
        self.unlock_mode = false;
        self.password.clear();
    }

    pub fn is_unlock_mode(&self) -> bool {
        self.unlock_mode
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// Feed a single key press (already de-duplicated by backend).
    /// Returns the action for the caller to act on.
    pub fn feed(&mut self, now: Instant, key: RawKey) -> Action {
        match key {
            RawKey::ShiftLeft | RawKey::ShiftRight => {
                self.shift_down = true;
                Action::None
            }
            RawKey::CapsLock => {
                self.caps_times.push(now);
                self.caps_times
                    .retain(|&t| now.duration_since(t) < CAPS_TRIGGER_WINDOW);
                if self.caps_times.len() >= CAPS_TRIGGER_COUNT {
                    self.caps_times.clear();
                    if !self.unlock_mode {
                        self.unlock_mode = true;
                        self.password.clear();
                        return Action::UnlockMode;
                    }
                }
                Action::None
            }
            RawKey::Backspace => {
                if self.unlock_mode {
                    self.password.pop();
                    return Action::Password {
                        len: self.password.len(),
                    };
                }
                Action::None
            }
            RawKey::Enter => {
                if self.unlock_mode {
                    return Action::Submit;
                }
                Action::None
            }
            RawKey::Letter(c) => {
                if self.unlock_mode {
                    let ch = if self.shift_down {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    };
                    self.push(ch);
                    return Action::Password {
                        len: self.password.len(),
                    };
                }
                Action::None
            }
            RawKey::Digit(d) => {
                if self.unlock_mode {
                    self.push(char::from(b'0' + d));
                    return Action::Password {
                        len: self.password.len(),
                    };
                }
                Action::None
            }
            RawKey::Other => Action::None,
        }
    }

    /// Call when a shift key is released.
    pub fn shift_release(&mut self) {
        self.shift_down = false;
    }

    /// Call on wrong password to close unlock mode and clear buffer.
    pub fn cancel_unlock_mode(&mut self) {
        self.unlock_mode = false;
        self.password.clear();
        self.caps_times.clear();
    }

    /// Call when the input device is lost or backend exits unexpectedly.
    /// Clears transient state (shift, caps tracking) but preserves password
    /// and unlock mode so the user can continue after reconnect.
    pub fn device_lost(&mut self) {
        self.shift_down = false;
        self.caps_times.clear();
    }

    fn push(&mut self, ch: char) {
        if self.password.len() < PASSWORD_MAX_LEN {
            self.password.push(ch);
        }
    }
}

impl Default for Keypad {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_three_times_unlock_mode() {
        let mut k = Keypad::new();
        let now = Instant::now();
        assert_eq!(k.feed(now, RawKey::CapsLock), Action::None);
        assert_eq!(
            k.feed(now + Duration::from_millis(300), RawKey::CapsLock),
            Action::None
        );
        assert_eq!(
            k.feed(now + Duration::from_millis(600), RawKey::CapsLock),
            Action::UnlockMode
        );
        assert!(k.is_unlock_mode());
    }

    #[test]
    fn caps_expires_after_window() {
        let mut k = Keypad::new();
        let now = Instant::now();
        k.feed(now, RawKey::CapsLock);
        k.feed(now + Duration::from_millis(300), RawKey::CapsLock);
        // third press outside window → no unlock
        assert_eq!(
            k.feed(
                now + CAPS_TRIGGER_WINDOW + Duration::from_millis(10),
                RawKey::CapsLock
            ),
            Action::None
        );
        assert!(!k.is_unlock_mode());
    }

    #[test]
    fn password_build_and_submit() {
        let mut k = Keypad::new();
        let now = Instant::now();
        k.feed(now, RawKey::CapsLock);
        k.feed(now + Duration::from_millis(300), RawKey::CapsLock);
        k.feed(now + Duration::from_millis(600), RawKey::CapsLock);
        assert!(k.is_unlock_mode());

        assert_eq!(
            k.feed(now, RawKey::Letter('a')),
            Action::Password { len: 1 }
        );
        k.shift_release(); // none pressed yet
        assert_eq!(k.feed(now, RawKey::Digit(1)), Action::Password { len: 2 });
        assert_eq!(k.password(), "a1");
        assert_eq!(k.feed(now, RawKey::Backspace), Action::Password { len: 1 });
        assert_eq!(k.feed(now, RawKey::Enter), Action::Submit);
    }

    #[test]
    fn shift_uppercases_letter_not_digit() {
        let mut k = Keypad::new();
        let now = Instant::now();
        // arm
        for i in 0..3 {
            k.feed(
                now + Duration::from_millis(i as u64 * 300),
                RawKey::CapsLock,
            );
        }
        k.feed(now, RawKey::ShiftLeft);
        k.feed(now, RawKey::Letter('a'));
        assert_eq!(k.password(), "A");
        k.feed(now, RawKey::Digit(1));
        assert_eq!(k.password(), "A1");
    }

    #[test]
    fn wrong_password_clears_mode() {
        let mut k = Keypad::new();
        let now = Instant::now();
        for i in 0..3 {
            k.feed(
                now + Duration::from_millis(i as u64 * 300),
                RawKey::CapsLock,
            );
        }
        k.feed(now, RawKey::Letter('x'));
        assert!(k.is_unlock_mode());
        k.cancel_unlock_mode();
        assert!(!k.is_unlock_mode());
        assert!(k.password().is_empty());
    }

    #[test]
    fn caps_does_not_turn_off_unlock_mode() {
        let mut k = Keypad::new();
        let now = Instant::now();
        for i in 0..3 {
            k.feed(
                now + Duration::from_millis(i as u64 * 300),
                RawKey::CapsLock,
            );
        }
        // extra caps after armed must not disarm
        assert_eq!(k.feed(now, RawKey::CapsLock), Action::None);
        assert!(k.is_unlock_mode());
    }

    #[test]
    fn max_password_len() {
        let mut k = Keypad::new();
        let now = Instant::now();
        for i in 0..3 {
            k.feed(
                now + Duration::from_millis(i as u64 * 300),
                RawKey::CapsLock,
            );
        }
        for _ in 0..PASSWORD_MAX_LEN + 10 {
            k.feed(now, RawKey::Letter('a'));
        }
        assert_eq!(k.password().len(), PASSWORD_MAX_LEN);
    }

    #[test]
    fn device_lost_clears_shift_and_caps() {
        let mut k = Keypad::new();
        let now = Instant::now();
        k.feed(now, RawKey::ShiftLeft);
        assert!(k.shift_down);
        k.feed(now + Duration::from_millis(100), RawKey::CapsLock);
        assert!(!k.caps_times.is_empty());

        k.device_lost();
        assert!(!k.shift_down);
        assert!(k.caps_times.is_empty());
        // unlock_mode and password should survive
        assert!(!k.is_unlock_mode()); // not armed yet
        assert!(k.password().is_empty());
    }

    #[test]
    fn device_lost_preserves_unlock_mode_and_password() {
        let mut k = Keypad::new();
        let now = Instant::now();
        for i in 0..3 {
            k.feed(
                now + Duration::from_millis(i as u64 * 300),
                RawKey::CapsLock,
            );
        }
        k.feed(now, RawKey::Letter('x'));
        assert!(k.is_unlock_mode());
        assert_eq!(k.password(), "x");

        k.device_lost();
        assert!(k.is_unlock_mode());
        assert_eq!(k.password(), "x");
    }
}
