//! PS/2 keyboard driver for x86_64.
//!
//! Reads scancodes from I/O port 0x60 (IRQ 1), translates Set 1 scancodes
//! to ASCII characters, and feeds them into the TTY subsystem.
//! Supports US keyboard layout with Shift modifier.

#[cfg(target_arch = "x86_64")]
use core::sync::atomic::{AtomicU8, Ordering};

/// Modifier key state bits.
const MOD_SHIFT: u8 = 0x01;
const MOD_CTRL: u8 = 0x02;
const MOD_ALT: u8 = 0x04;
const MOD_CAPS: u8 = 0x08;

/// Extended scancode prefix.
const EXTENDED_PREFIX: u8 = 0xE0;

/// Current modifier state.
static MODIFIERS: AtomicU8 = AtomicU8::new(0);

/// Whether we're in an extended scancode sequence.
static mut EXTENDED: bool = false;

/// US keyboard layout — unshifted (scancode set 1, scancodes 0x02..0x35).
const SCANCODE_MAP: &[u8; 58] = b"\
    \x00\x1B1234567890-=\x08\
    \x09qwertyuiop[]\x0D\
    \x00asdfghjkl;'`\x00\
    \\zxcvbnm,./\x00\
    *\x00 ";

/// US keyboard layout — shifted.
const SCANCODE_MAP_SHIFT: &[u8; 58] = b"\
    \x00\x1B!@#$%^&*()_+\x08\
    \x09QWERTYUIOP{}\x0D\
    \x00ASDFGHJKL:\"~\x00\
    |ZXCVBNM<>?\x00\
    *\x00 ";

/// Extended key mappings (E0-prefixed scancodes).
/// Returns Some(ascii) or None.
fn handle_extended_scancode(scancode: u8) -> Option<u8> {
    match scancode {
        // Numpad Enter → same as regular Enter
        0x1C => Some(b'\n'),
        // Numpad / → same as regular /
        0x35 => Some(b'/'),
        // Right Ctrl, Right Alt, etc. — ignore
        _ => None,
    }
}

/// Handle a make (key press) scancode.
fn handle_make(scancode: u8) -> Option<u8> {
    match scancode {
        // Left Shift
        0x2A | 0x36 => {
            let mods = MODIFIERS.load(Ordering::Relaxed);
            MODIFIERS.store(mods | MOD_SHIFT, Ordering::Relaxed);
            None
        }
        // Left Ctrl
        0x1D => {
            let mods = MODIFIERS.load(Ordering::Relaxed);
            MODIFIERS.store(mods | MOD_CTRL, Ordering::Relaxed);
            None
        }
        // Left Alt
        0x38 => {
            let mods = MODIFIERS.load(Ordering::Relaxed);
            MODIFIERS.store(mods | MOD_ALT, Ordering::Relaxed);
            None
        }
        // Caps Lock (toggle)
        0x3A => {
            let mods = MODIFIERS.load(Ordering::Relaxed);
            MODIFIERS.store(mods ^ MOD_CAPS, Ordering::Relaxed);
            None
        }
        // Printable keys
        _ => {
            if (scancode as usize) < SCANCODE_MAP.len() {
                let mods = MODIFIERS.load(Ordering::Relaxed);
                let shifted = (mods & MOD_SHIFT) != 0;
                let caps = (mods & MOD_CAPS) != 0;
                let map = if shifted {
                    SCANCODE_MAP_SHIFT
                } else {
                    SCANCODE_MAP
                };
                let c = map[scancode as usize];

                // For letters, Caps Lock inverts shift
                if c == 0 {
                    return None;
                }
                if caps && c.is_ascii_alphabetic() {
                    if shifted {
                        Some(c.to_ascii_lowercase())
                    } else {
                        Some(c.to_ascii_uppercase())
                    }
                } else {
                    Some(c)
                }
            } else {
                None
            }
        }
    }
}

/// Handle a break (key release) scancode.
fn handle_break(scancode: u8) {
    match scancode {
        // Left/Right Shift release
        0x2A | 0x36 => {
            let mods = MODIFIERS.load(Ordering::Relaxed);
            MODIFIERS.store(mods & !MOD_SHIFT, Ordering::Relaxed);
        }
        // Ctrl release
        0x1D => {
            let mods = MODIFIERS.load(Ordering::Relaxed);
            MODIFIERS.store(mods & !MOD_CTRL, Ordering::Relaxed);
        }
        // Alt release
        0x38 => {
            let mods = MODIFIERS.load(Ordering::Relaxed);
            MODIFIERS.store(mods & !MOD_ALT, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Process a scancode byte from the PS/2 keyboard.
///
/// Called from the keyboard interrupt handler (IRQ 1, vector 33).
/// Translates the scancode to an ASCII character and feeds it into
/// the TTY subsystem if it produces a character.
pub fn handle_scancode(scancode: u8) {
    // Extended key sequence
    if scancode == EXTENDED_PREFIX {
        unsafe { EXTENDED = true };
        return;
    }

    let extended = unsafe { EXTENDED };
    if extended {
        unsafe { EXTENDED = false };
        // Extended break code: E0 0x80+scancode
        if scancode >= 0x80 {
            // Extended key release — ignore for now
            return;
        }
        // Extended key press
        if let Some(c) = handle_extended_scancode(scancode) {
            crate::driver::tty::feed_byte(c);
        }
        return;
    }

    // Key release (break code): high bit set
    if scancode >= 0x80 {
        handle_break(scancode & 0x7F);
        return;
    }

    // Key press (make code)
    if let Some(c) = handle_make(scancode) {
        crate::driver::tty::feed_byte(c);
    }
}

/// Initialize the PS/2 keyboard driver.
///
/// The PS/2 keyboard is already configured by the BIOS/QEMU.
/// We just need to ensure the keyboard IRQ (IRQ 1) is enabled.
/// IRQ routing is handled by the IDT setup (keyboard_handler at vector 33).
pub fn init() {
    // Reset modifier state
    MODIFIERS.store(0, Ordering::Relaxed);
    unsafe { EXTENDED = false };

    // Enable keyboard IRQ via PIC (IRQ 1 = bit 1 of OCW1 mask).
    // On QEMU with LAPIC, legacy PIC emulation handles this.
    // Read current mask, clear bit 1 (IRQ 1 = keyboard), write back.
    unsafe {
        let mut pic1_mask: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0xA1);
        let mask = pic1_mask.read();
        pic1_mask.write(mask & !0x02); // Clear bit 1 to unmask IRQ 1
    }

    crate::console_println!("[keyboard] PS/2 driver initialized");
}

/// Poll for keyboard input (non-interrupt).
/// Reads PS/2 status port 0x64, if bit 0 is set, reads data port 0x60.
/// This works with USB Legacy emulation even when IOAPIC routing is wrong.
pub fn poll() {
    unsafe {
        let status: u8;
        core::arch::asm!("in al, dx", in("dx") 0x64u16, out("al") status);
        if status & 1 != 0 {
            let data: u8;
            core::arch::asm!("in al, dx", in("dx") 0x60u16, out("al") data);
            handle_scancode(data);
        }
    }
}
