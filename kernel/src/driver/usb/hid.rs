//! HID boot keyboard report parsing and TTY integration.

#![allow(dead_code)]

use crate::driver::tty;

// HID Usage IDs for USB boot keyboard (HID Usage Table 1.12).
const HID_USAGE_A: u8 = 0x04;
const HID_USAGE_Z: u8 = 0x1D;
const HID_USAGE_1: u8 = 0x1E;
const HID_USAGE_9: u8 = 0x26;
const HID_USAGE_0: u8 = 0x27;
const HID_USAGE_ENTER: u8 = 0x28;
const HID_USAGE_ESCAPE: u8 = 0x29;
const HID_USAGE_BACKSPACE: u8 = 0x2A;
const HID_USAGE_TAB: u8 = 0x2B;
const HID_USAGE_SPACE: u8 = 0x2C;
const HID_USAGE_MINUS: u8 = 0x2D;
const HID_USAGE_EQUAL: u8 = 0x2E;
const HID_USAGE_LEFT_BRACKET: u8 = 0x2F;
const HID_USAGE_RIGHT_BRACKET: u8 = 0x30;
const HID_USAGE_BACKSLASH: u8 = 0x31;
const HID_USAGE_SEMICOLON: u8 = 0x33;
const HID_USAGE_APOSTROPHE: u8 = 0x34;
const HID_USAGE_GRAVE: u8 = 0x35;
const HID_USAGE_COMMA: u8 = 0x36;
const HID_USAGE_DOT: u8 = 0x37;
const HID_USAGE_SLASH: u8 = 0x38;
const HID_USAGE_CAPSLOCK: u8 = 0x39;
const HID_USAGE_F1: u8 = 0x3A;
const HID_USAGE_F12: u8 = 0x45;
const HID_USAGE_PRINTSCREEN: u8 = 0x46;
const HID_USAGE_SCROLLLOCK: u8 = 0x47;
const HID_USAGE_INSERT: u8 = 0x49;
const HID_USAGE_HOME: u8 = 0x4A;
const HID_USAGE_PAGEUP: u8 = 0x4B;
const HID_USAGE_DELETE: u8 = 0x4C;
const HID_USAGE_END: u8 = 0x4D;
const HID_USAGE_PAGEDOWN: u8 = 0x4E;
const HID_USAGE_RIGHT_ARROW: u8 = 0x4F;
const HID_USAGE_LEFT_ARROW: u8 = 0x50;
const HID_USAGE_DOWN_ARROW: u8 = 0x51;
const HID_USAGE_UP_ARROW: u8 = 0x52;
const HID_USAGE_NUMLOCK: u8 = 0x53;

// Keypad
const HID_USAGE_KEYPAD_1: u8 = 0x59;
const HID_USAGE_KEYPAD_9: u8 = 0x61;
const HID_USAGE_KEYPAD_0: u8 = 0x62;
const HID_USAGE_KEYPAD_DOT: u8 = 0x63;
const HID_USAGE_KEYPAD_ENTER: u8 = 0x58;
const HID_USAGE_KEYPAD_PLUS: u8 = 0x57;
const HID_USAGE_KEYPAD_MINUS: u8 = 0x56;
const HID_USAGE_KEYPAD_STAR: u8 = 0x55;
const HID_USAGE_KEYPAD_SLASH: u8 = 0x54;

// Modifier bits in byte 0 of the boot report
const MOD_LCTRL: u8 = 0x01;
const MOD_LSHIFT: u8 = 0x02;
const MOD_LALT: u8 = 0x04;
const MOD_LMETA: u8 = 0x08;
const MOD_RCTRL: u8 = 0x10;
const MOD_RSHIFT: u8 = 0x20;
const MOD_RALT: u8 = 0x40;
const MOD_RMETA: u8 = 0x80;

#[derive(Clone, Copy)]
struct KeyState {
    shift: bool,
    caps: bool,
}

static KEY_STATE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
const KEY_CAPS: u8 = 1 << 0;

/// Convert a HID usage code + modifier byte into an ASCII character (or 0
/// if it does not map to a printable/control character we forward to TTY).
pub fn hid_to_ascii(usage: u8, modifier: u8) -> u8 {
    let shift = modifier & (MOD_LSHIFT | MOD_RSHIFT) != 0;
    let caps = KEY_STATE.load(core::sync::atomic::Ordering::Relaxed) & KEY_CAPS != 0;

    // CapsLock toggles on press; track via KEY_STATE.
    if usage == HID_USAGE_CAPSLOCK {
        let cur = KEY_STATE.load(core::sync::atomic::Ordering::Relaxed);
        KEY_STATE.store(cur ^ KEY_CAPS, core::sync::atomic::Ordering::Relaxed);
        return 0;
    }

    let letter_shift = shift ^ caps;
    match usage {
        HID_USAGE_A..=HID_USAGE_Z => {
            let base = if letter_shift { b'A' } else { b'a' };
            base + (usage - HID_USAGE_A)
        }
        HID_USAGE_1..=HID_USAGE_9 => {
            if shift {
                shift_digit(usage - HID_USAGE_1 + 1)
            } else {
                b'0' + (usage - HID_USAGE_1 + 1)
            }
        }
        HID_USAGE_0 => {
            if shift {
                b')'
            } else {
                b'0'
            }
        }
        HID_USAGE_ENTER => b'\n',
        HID_USAGE_KEYPAD_ENTER => b'\n',
        HID_USAGE_TAB => b'\t',
        HID_USAGE_BACKSPACE => 0x7F, // DEL; TTY maps to backspace
        HID_USAGE_SPACE => b' ',
        HID_USAGE_ESCAPE => 0x1B,
        HID_USAGE_MINUS => {
            if shift {
                b'_'
            } else {
                b'-'
            }
        }
        HID_USAGE_EQUAL => {
            if shift {
                b'+'
            } else {
                b'='
            }
        }
        HID_USAGE_LEFT_BRACKET => {
            if shift {
                b'{'
            } else {
                b'['
            }
        }
        HID_USAGE_RIGHT_BRACKET => {
            if shift {
                b'}'
            } else {
                b']'
            }
        }
        HID_USAGE_BACKSLASH => {
            if shift {
                b'|'
            } else {
                b'\\'
            }
        }
        HID_USAGE_SEMICOLON => {
            if shift {
                b':'
            } else {
                b';'
            }
        }
        HID_USAGE_APOSTROPHE => {
            if shift {
                b'"'
            } else {
                b'\''
            }
        }
        HID_USAGE_GRAVE => {
            if shift {
                b'~'
            } else {
                b'`'
            }
        }
        HID_USAGE_COMMA => {
            if shift {
                b'<'
            } else {
                b','
            }
        }
        HID_USAGE_DOT => {
            if shift {
                b'>'
            } else {
                b'.'
            }
        }
        HID_USAGE_SLASH => {
            if shift {
                b'?'
            } else {
                b'/'
            }
        }
        HID_USAGE_KEYPAD_DOT => b'.',
        HID_USAGE_KEYPAD_0 => b'0',
        HID_USAGE_KEYPAD_1..=HID_USAGE_KEYPAD_9 => b'1' + (usage - HID_USAGE_KEYPAD_1),
        HID_USAGE_KEYPAD_PLUS => b'+',
        HID_USAGE_KEYPAD_MINUS => b'-',
        HID_USAGE_KEYPAD_STAR => b'*',
        HID_USAGE_KEYPAD_SLASH => b'/',
        _ => 0,
    }
}

fn shift_digit(d: u8) -> u8 {
    match d {
        1 => b'!',
        2 => b'@',
        3 => b'#',
        4 => b'$',
        5 => b'%',
        6 => b'^',
        7 => b'&',
        8 => b'*',
        9 => b'(',
        _ => b' ',
    }
}

/// Feed a decoded ASCII byte into the TTY subsystem.
pub fn feed_to_tty(ascii: u8) {
    tty::feed_byte(ascii);
}

/// Test helper: directly set the CapsLock tracking bit.
#[cfg(feature = "test_mode")]
pub fn set_caps_for_test(on: bool) {
    KEY_STATE.store(
        if on { KEY_CAPS } else { 0 },
        core::sync::atomic::Ordering::Relaxed,
    );
}
