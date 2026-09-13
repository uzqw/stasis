/// Platform-agnostic raw key events.
///
/// Backends translate native keycodes into this.  Shared logic (keypad)
/// turns it into characters using only `Shift` state, never CapsLock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawKey {
    CapsLock,
    ShiftLeft,
    ShiftRight,
    Backspace,
    Enter,
    Digit(u8),    // 0-9
    Letter(char), // a-z (shared logic upper-cases on Shift)
    Other,
}

// Linux evdev keycode -> RawKey (constants come from the evdev crate)
#[cfg(target_os = "linux")]
pub mod evdev {
    use super::RawKey;
    use ::evdev::KeyCode as K;

    pub fn to_raw(key: K) -> RawKey {
        match key {
            K::KEY_CAPSLOCK => RawKey::CapsLock,
            K::KEY_LEFTSHIFT => RawKey::ShiftLeft,
            K::KEY_RIGHTSHIFT => RawKey::ShiftRight,
            K::KEY_BACKSPACE => RawKey::Backspace,
            K::KEY_ENTER | K::KEY_KPENTER => RawKey::Enter,
            K::KEY_1 => RawKey::Digit(1),
            K::KEY_2 => RawKey::Digit(2),
            K::KEY_3 => RawKey::Digit(3),
            K::KEY_4 => RawKey::Digit(4),
            K::KEY_5 => RawKey::Digit(5),
            K::KEY_6 => RawKey::Digit(6),
            K::KEY_7 => RawKey::Digit(7),
            K::KEY_8 => RawKey::Digit(8),
            K::KEY_9 => RawKey::Digit(9),
            K::KEY_0 => RawKey::Digit(0),
            K::KEY_A => RawKey::Letter('a'),
            K::KEY_B => RawKey::Letter('b'),
            K::KEY_C => RawKey::Letter('c'),
            K::KEY_D => RawKey::Letter('d'),
            K::KEY_E => RawKey::Letter('e'),
            K::KEY_F => RawKey::Letter('f'),
            K::KEY_G => RawKey::Letter('g'),
            K::KEY_H => RawKey::Letter('h'),
            K::KEY_I => RawKey::Letter('i'),
            K::KEY_J => RawKey::Letter('j'),
            K::KEY_K => RawKey::Letter('k'),
            K::KEY_L => RawKey::Letter('l'),
            K::KEY_M => RawKey::Letter('m'),
            K::KEY_N => RawKey::Letter('n'),
            K::KEY_O => RawKey::Letter('o'),
            K::KEY_P => RawKey::Letter('p'),
            K::KEY_Q => RawKey::Letter('q'),
            K::KEY_R => RawKey::Letter('r'),
            K::KEY_S => RawKey::Letter('s'),
            K::KEY_T => RawKey::Letter('t'),
            K::KEY_U => RawKey::Letter('u'),
            K::KEY_V => RawKey::Letter('v'),
            K::KEY_W => RawKey::Letter('w'),
            K::KEY_X => RawKey::Letter('x'),
            K::KEY_Y => RawKey::Letter('y'),
            K::KEY_Z => RawKey::Letter('z'),
            _ => RawKey::Other,
        }
    }
}

// Windows virtual keycode -> RawKey
#[cfg(target_os = "windows")]
pub mod win {
    use super::RawKey;

    pub const VK_CAPITAL: u32 = 0x14;
    pub const VK_BACK: u32 = 0x08;
    pub const VK_RETURN: u32 = 0x0D;
    pub const VK_LSHIFT: u32 = 0xA0;
    pub const VK_RSHIFT: u32 = 0xA1;
    pub const VK_SHIFT: u32 = 0x10;

    pub fn to_raw(vk: u32) -> RawKey {
        match vk {
            VK_CAPITAL => RawKey::CapsLock,
            VK_LSHIFT | VK_RSHIFT | VK_SHIFT => RawKey::ShiftLeft, // unified shift
            VK_BACK => RawKey::Backspace,
            VK_RETURN => RawKey::Enter,
            0x30..=0x39 => RawKey::Digit((vk - 0x30) as u8),
            0x41..=0x5A => RawKey::Letter((vk as u8).to_ascii_lowercase() as char),
            _ => RawKey::Other,
        }
    }
}

// macOS virtual keycode -> RawKey
#[cfg(target_os = "macos")]
pub mod mac {
    use super::RawKey;

    pub const VK_CAPS_LOCK: u16 = 57;
    pub const VK_SHIFT: u16 = 56;
    pub const VK_RIGHT_SHIFT: u16 = 60;
    pub const VK_DELETE: u16 = 51;
    pub const VK_RETURN: u16 = 36;
    pub const VK_KEYPAD_ENTER: u16 = 76;

    pub const VK_ANSI_0: u16 = 29;
    pub const VK_ANSI_1: u16 = 18;
    pub const VK_ANSI_2: u16 = 19;
    pub const VK_ANSI_3: u16 = 20;
    pub const VK_ANSI_4: u16 = 21;
    pub const VK_ANSI_5: u16 = 23;
    pub const VK_ANSI_6: u16 = 22;
    pub const VK_ANSI_7: u16 = 26;
    pub const VK_ANSI_8: u16 = 28;
    pub const VK_ANSI_9: u16 = 25;

    pub const VK_ANSI_A: u16 = 0;
    pub const VK_ANSI_B: u16 = 11;
    pub const VK_ANSI_C: u16 = 8;
    pub const VK_ANSI_D: u16 = 2;
    pub const VK_ANSI_E: u16 = 14;
    pub const VK_ANSI_F: u16 = 3;
    pub const VK_ANSI_G: u16 = 5;
    pub const VK_ANSI_H: u16 = 4;
    pub const VK_ANSI_I: u16 = 34;
    pub const VK_ANSI_J: u16 = 38;
    pub const VK_ANSI_K: u16 = 40;
    pub const VK_ANSI_L: u16 = 37;
    pub const VK_ANSI_M: u16 = 46;
    pub const VK_ANSI_N: u16 = 45;
    pub const VK_ANSI_O: u16 = 31;
    pub const VK_ANSI_P: u16 = 35;
    pub const VK_ANSI_Q: u16 = 12;
    pub const VK_ANSI_R: u16 = 15;
    pub const VK_ANSI_S: u16 = 1;
    pub const VK_ANSI_T: u16 = 17;
    pub const VK_ANSI_U: u16 = 32;
    pub const VK_ANSI_V: u16 = 9;
    pub const VK_ANSI_W: u16 = 13;
    pub const VK_ANSI_X: u16 = 7;
    pub const VK_ANSI_Y: u16 = 16;
    pub const VK_ANSI_Z: u16 = 6;

    pub fn to_raw(vk: u16) -> RawKey {
        match vk {
            VK_CAPS_LOCK => RawKey::CapsLock,
            VK_SHIFT | VK_RIGHT_SHIFT => RawKey::ShiftLeft,
            VK_DELETE => RawKey::Backspace,
            VK_RETURN | VK_KEYPAD_ENTER => RawKey::Enter,
            VK_ANSI_0 => RawKey::Digit(0),
            VK_ANSI_1 => RawKey::Digit(1),
            VK_ANSI_2 => RawKey::Digit(2),
            VK_ANSI_3 => RawKey::Digit(3),
            VK_ANSI_4 => RawKey::Digit(4),
            VK_ANSI_5 => RawKey::Digit(5),
            VK_ANSI_6 => RawKey::Digit(6),
            VK_ANSI_7 => RawKey::Digit(7),
            VK_ANSI_8 => RawKey::Digit(8),
            VK_ANSI_9 => RawKey::Digit(9),
            VK_ANSI_A => RawKey::Letter('a'),
            VK_ANSI_B => RawKey::Letter('b'),
            VK_ANSI_C => RawKey::Letter('c'),
            VK_ANSI_D => RawKey::Letter('d'),
            VK_ANSI_E => RawKey::Letter('e'),
            VK_ANSI_F => RawKey::Letter('f'),
            VK_ANSI_G => RawKey::Letter('g'),
            VK_ANSI_H => RawKey::Letter('h'),
            VK_ANSI_I => RawKey::Letter('i'),
            VK_ANSI_J => RawKey::Letter('j'),
            VK_ANSI_K => RawKey::Letter('k'),
            VK_ANSI_L => RawKey::Letter('l'),
            VK_ANSI_M => RawKey::Letter('m'),
            VK_ANSI_N => RawKey::Letter('n'),
            VK_ANSI_O => RawKey::Letter('o'),
            VK_ANSI_P => RawKey::Letter('p'),
            VK_ANSI_Q => RawKey::Letter('q'),
            VK_ANSI_R => RawKey::Letter('r'),
            VK_ANSI_S => RawKey::Letter('s'),
            VK_ANSI_T => RawKey::Letter('t'),
            VK_ANSI_U => RawKey::Letter('u'),
            VK_ANSI_V => RawKey::Letter('v'),
            VK_ANSI_W => RawKey::Letter('w'),
            VK_ANSI_X => RawKey::Letter('x'),
            VK_ANSI_Y => RawKey::Letter('y'),
            VK_ANSI_Z => RawKey::Letter('z'),
            _ => RawKey::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RawKey;

    #[test]
    #[cfg(target_os = "linux")]
    fn evdev_maps_qwerty() {
        use super::evdev;
        use ::evdev::KeyCode as K;
        assert_eq!(evdev::to_raw(K::KEY_A), RawKey::Letter('a'));
        assert_eq!(evdev::to_raw(K::KEY_Z), RawKey::Letter('z'));
        assert_eq!(evdev::to_raw(K::KEY_1), RawKey::Digit(1));
        assert_eq!(evdev::to_raw(K::KEY_0), RawKey::Digit(0));
        assert_eq!(evdev::to_raw(K::KEY_BACKSPACE), RawKey::Backspace);
        assert_eq!(evdev::to_raw(K::KEY_ENTER), RawKey::Enter);
        assert_eq!(evdev::to_raw(K::KEY_KPENTER), RawKey::Enter);
        assert_eq!(evdev::to_raw(K::KEY_CAPSLOCK), RawKey::CapsLock);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn win_maps_qwerty() {
        use super::win;
        assert_eq!(win::to_raw(0x41), RawKey::Letter('a'));
        assert_eq!(win::to_raw(0x5A), RawKey::Letter('z'));
        assert_eq!(win::to_raw(0x30), RawKey::Digit(0));
        assert_eq!(win::to_raw(0x39), RawKey::Digit(9));
        assert_eq!(win::to_raw(win::VK_BACK), RawKey::Backspace);
        assert_eq!(win::to_raw(win::VK_CAPITAL), RawKey::CapsLock);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn mac_maps_qwerty() {
        use super::mac;
        assert_eq!(mac::to_raw(mac::VK_ANSI_A), RawKey::Letter('a'));
        assert_eq!(mac::to_raw(mac::VK_ANSI_Z), RawKey::Letter('z'));
        assert_eq!(mac::to_raw(mac::VK_ANSI_0), RawKey::Digit(0));
        assert_eq!(mac::to_raw(mac::VK_ANSI_9), RawKey::Digit(9));
        assert_eq!(mac::to_raw(mac::VK_DELETE), RawKey::Backspace);
        assert_eq!(mac::to_raw(mac::VK_CAPS_LOCK), RawKey::CapsLock);
    }
}
