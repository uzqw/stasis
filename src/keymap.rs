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

// Linux evdev keycode -> RawKey
#[cfg(target_os = "linux")]
pub mod evdev {
    use super::RawKey;

    pub const KEY_A: u16 = 30;
    pub const KEY_B: u16 = 48;
    pub const KEY_C: u16 = 46;
    pub const KEY_D: u16 = 32;
    pub const KEY_E: u16 = 18;
    pub const KEY_F: u16 = 33;
    pub const KEY_G: u16 = 34;
    pub const KEY_H: u16 = 35;
    pub const KEY_I: u16 = 23;
    pub const KEY_J: u16 = 36;
    pub const KEY_K: u16 = 37;
    pub const KEY_L: u16 = 38;
    pub const KEY_M: u16 = 50;
    pub const KEY_N: u16 = 49;
    pub const KEY_O: u16 = 24;
    pub const KEY_P: u16 = 25;
    pub const KEY_Q: u16 = 16;
    pub const KEY_R: u16 = 19;
    pub const KEY_S: u16 = 31;
    pub const KEY_T: u16 = 20;
    pub const KEY_U: u16 = 22;
    pub const KEY_V: u16 = 47;
    pub const KEY_W: u16 = 17;
    pub const KEY_X: u16 = 45;
    pub const KEY_Y: u16 = 21;
    pub const KEY_Z: u16 = 44;

    pub const KEY_1: u16 = 2;
    pub const KEY_2: u16 = 3;
    pub const KEY_3: u16 = 4;
    pub const KEY_4: u16 = 5;
    pub const KEY_5: u16 = 6;
    pub const KEY_6: u16 = 7;
    pub const KEY_7: u16 = 8;
    pub const KEY_8: u16 = 9;
    pub const KEY_9: u16 = 10;
    pub const KEY_0: u16 = 11;

    pub const KEY_BACKSPACE: u16 = 14;
    pub const KEY_ENTER: u16 = 28;
    pub const KEY_KPENTER: u16 = 96;
    pub const KEY_CAPSLOCK: u16 = 58;
    pub const KEY_LEFTSHIFT: u16 = 42;
    pub const KEY_RIGHTSHIFT: u16 = 54;

    pub fn to_raw(code: u16) -> RawKey {
        match code {
            KEY_CAPSLOCK => RawKey::CapsLock,
            KEY_LEFTSHIFT => RawKey::ShiftLeft,
            KEY_RIGHTSHIFT => RawKey::ShiftRight,
            KEY_BACKSPACE => RawKey::Backspace,
            KEY_ENTER | KEY_KPENTER => RawKey::Enter,
            KEY_1 => RawKey::Digit(1),
            KEY_2 => RawKey::Digit(2),
            KEY_3 => RawKey::Digit(3),
            KEY_4 => RawKey::Digit(4),
            KEY_5 => RawKey::Digit(5),
            KEY_6 => RawKey::Digit(6),
            KEY_7 => RawKey::Digit(7),
            KEY_8 => RawKey::Digit(8),
            KEY_9 => RawKey::Digit(9),
            KEY_0 => RawKey::Digit(0),
            KEY_A => RawKey::Letter('a'),
            KEY_B => RawKey::Letter('b'),
            KEY_C => RawKey::Letter('c'),
            KEY_D => RawKey::Letter('d'),
            KEY_E => RawKey::Letter('e'),
            KEY_F => RawKey::Letter('f'),
            KEY_G => RawKey::Letter('g'),
            KEY_H => RawKey::Letter('h'),
            KEY_I => RawKey::Letter('i'),
            KEY_J => RawKey::Letter('j'),
            KEY_K => RawKey::Letter('k'),
            KEY_L => RawKey::Letter('l'),
            KEY_M => RawKey::Letter('m'),
            KEY_N => RawKey::Letter('n'),
            KEY_O => RawKey::Letter('o'),
            KEY_P => RawKey::Letter('p'),
            KEY_Q => RawKey::Letter('q'),
            KEY_R => RawKey::Letter('r'),
            KEY_S => RawKey::Letter('s'),
            KEY_T => RawKey::Letter('t'),
            KEY_U => RawKey::Letter('u'),
            KEY_V => RawKey::Letter('v'),
            KEY_W => RawKey::Letter('w'),
            KEY_X => RawKey::Letter('x'),
            KEY_Y => RawKey::Letter('y'),
            KEY_Z => RawKey::Letter('z'),
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
        assert_eq!(evdev::to_raw(evdev::KEY_A), RawKey::Letter('a'));
        assert_eq!(evdev::to_raw(evdev::KEY_Z), RawKey::Letter('z'));
        assert_eq!(evdev::to_raw(evdev::KEY_1), RawKey::Digit(1));
        assert_eq!(evdev::to_raw(evdev::KEY_0), RawKey::Digit(0));
        assert_eq!(evdev::to_raw(evdev::KEY_BACKSPACE), RawKey::Backspace);
        assert_eq!(evdev::to_raw(evdev::KEY_ENTER), RawKey::Enter);
        assert_eq!(evdev::to_raw(evdev::KEY_CAPSLOCK), RawKey::CapsLock);
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
