use std::time::{Duration, Instant};

use crate::keymap::RawKey;

const TRIGGER_COUNT: usize = 3;
const TRIGGER_WINDOW: Duration = Duration::from_secs(2);
const PASSWORD_MAX_LEN: usize = 64;

/// Unlock gesture key: press it [`TRIGGER_COUNT`] times within [`TRIGGER_WINDOW`].
///
/// `j` has exactly one physical key on every platform: no keypad twin and no
/// modifier-flag ambiguity (macOS reports CapsLock through `flagsChanged`,
/// where press and release are hard to tell apart).
const TRIGGER_KEY: RawKey = RawKey::Letter('j');

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
/// - Tracks [`TRIGGER_KEY`] presses within a 2-second window.
/// - Once armed (`unlock_mode = true`), builds a password buffer; the trigger
///   key is then an ordinary password character again.
/// - Shift state is tracked; CapsLock is ignored entirely.
/// - Only letters and digits are accepted; everything else is swallowed.
#[derive(Debug, Clone)]
pub struct Keypad {
    trigger_times: Vec<Instant>,
    shift_down: bool,
    unlock_mode: bool,
    password: String,
}

impl Keypad {
    pub fn new() -> Self {
        Self {
            trigger_times: Vec::with_capacity(TRIGGER_COUNT),
            shift_down: false,
            unlock_mode: false,
            password: String::with_capacity(16),
        }
    }

    pub fn reset(&mut self) {
        self.trigger_times.clear();
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
        // Gesture counting only while disarmed: once armed the trigger key is a
        // normal password character, and a repeated gesture must never clear
        // what has already been typed.
        if key == TRIGGER_KEY && !self.unlock_mode {
            self.trigger_times.push(now);
            self.trigger_times
                .retain(|&t| now.duration_since(t) < TRIGGER_WINDOW);
            if self.trigger_times.len() >= TRIGGER_COUNT {
                self.trigger_times.clear();
                self.unlock_mode = true;
                self.password.clear();
                return Action::UnlockMode;
            }
            return Action::None;
        }
        match key {
            // No longer the gesture; ignored and never buffered.
            RawKey::CapsLock => Action::None,
            RawKey::ShiftLeft | RawKey::ShiftRight => {
                self.shift_down = true;
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
        self.trigger_times.clear();
    }

    /// Call when the input device is lost or backend exits unexpectedly.
    /// Clears transient state (shift, gesture tracking) but preserves password
    /// and unlock mode so the user can continue after reconnect.
    pub fn device_lost(&mut self) {
        self.shift_down = false;
        self.trigger_times.clear();
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

    /// Arm the keypad with the gesture, three presses 300 ms apart.
    fn arm(k: &mut Keypad, now: Instant) {
        for i in 0..TRIGGER_COUNT {
            k.feed(now + Duration::from_millis(i as u64 * 300), TRIGGER_KEY);
        }
    }

    #[test]
    fn trigger_three_times_arms_unlock_mode() {
        let mut k = Keypad::new();
        let now = Instant::now();
        assert_eq!(k.feed(now, TRIGGER_KEY), Action::None);
        assert_eq!(
            k.feed(now + Duration::from_millis(300), TRIGGER_KEY),
            Action::None
        );
        assert_eq!(
            k.feed(now + Duration::from_millis(600), TRIGGER_KEY),
            Action::UnlockMode
        );
        assert!(k.is_unlock_mode());
        // Arming presses are swallowed, they are not password characters.
        assert!(k.password().is_empty());
    }

    #[test]
    fn trigger_expires_after_window() {
        let mut k = Keypad::new();
        let now = Instant::now();
        k.feed(now, TRIGGER_KEY);
        k.feed(now + Duration::from_millis(300), TRIGGER_KEY);
        // third press outside window → no unlock
        assert_eq!(
            k.feed(
                now + TRIGGER_WINDOW + Duration::from_millis(10),
                TRIGGER_KEY
            ),
            Action::None
        );
        assert!(!k.is_unlock_mode());
    }

    #[test]
    fn caps_lock_no_longer_arms_or_types() {
        let mut k = Keypad::new();
        let now = Instant::now();
        for i in 0..5 {
            assert_eq!(
                k.feed(now + Duration::from_millis(i * 100), RawKey::CapsLock),
                Action::None
            );
        }
        assert!(!k.is_unlock_mode());
        arm(&mut k, now);
        assert!(k.is_unlock_mode());
        assert_eq!(k.feed(now, RawKey::CapsLock), Action::None);
        assert!(k.password().is_empty());
    }

    #[test]
    fn trigger_key_is_a_password_character_once_armed() {
        let mut k = Keypad::new();
        let now = Instant::now();
        arm(&mut k, now);
        assert!(k.is_unlock_mode());
        assert_eq!(k.feed(now, TRIGGER_KEY), Action::Password { len: 1 });
        assert_eq!(k.password(), "j");
        // Still armed after typing the trigger key as a character.
        assert!(k.is_unlock_mode());
    }

    #[test]
    fn password_build_and_submit() {
        let mut k = Keypad::new();
        let now = Instant::now();
        arm(&mut k, now);
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
        arm(&mut k, now);
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
        arm(&mut k, now);
        k.feed(now, RawKey::Letter('x'));
        assert!(k.is_unlock_mode());
        k.cancel_unlock_mode();
        assert!(!k.is_unlock_mode());
        assert!(k.password().is_empty());
    }

    #[test]
    fn repeated_gesture_after_arming_types_characters() {
        let mut k = Keypad::new();
        let now = Instant::now();
        arm(&mut k, now);
        // After arming the trigger key types, it never disarms or clears.
        assert_eq!(k.feed(now, TRIGGER_KEY), Action::Password { len: 1 });
        assert_eq!(k.feed(now, TRIGGER_KEY), Action::Password { len: 2 });
        assert!(k.is_unlock_mode());
        assert_eq!(k.password(), "jj");
    }

    #[test]
    fn max_password_len() {
        let mut k = Keypad::new();
        let now = Instant::now();
        arm(&mut k, now);
        for _ in 0..PASSWORD_MAX_LEN + 10 {
            k.feed(now, RawKey::Letter('a'));
        }
        assert_eq!(k.password().len(), PASSWORD_MAX_LEN);
    }

    #[test]
    fn device_lost_clears_shift_and_gesture() {
        let mut k = Keypad::new();
        let now = Instant::now();
        k.feed(now, RawKey::ShiftLeft);
        assert!(k.shift_down);
        k.feed(now + Duration::from_millis(100), TRIGGER_KEY);
        assert!(!k.trigger_times.is_empty());

        k.device_lost();
        assert!(!k.shift_down);
        assert!(k.trigger_times.is_empty());
        // unlock_mode and password should survive
        assert!(!k.is_unlock_mode()); // not armed yet
        assert!(k.password().is_empty());
    }

    #[test]
    fn device_lost_preserves_unlock_mode_and_password() {
        let mut k = Keypad::new();
        let now = Instant::now();
        arm(&mut k, now);
        k.feed(now, RawKey::Letter('x'));
        assert!(k.is_unlock_mode());
        assert_eq!(k.password(), "x");

        k.device_lost();
        assert!(k.is_unlock_mode());
        assert_eq!(k.password(), "x");
    }
}
