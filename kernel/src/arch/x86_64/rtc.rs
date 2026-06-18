//! CMOS RTC reader for real wall clock time.
//!
//! Reads date/time from the CMOS RTC (I/O ports 0x70/0x71) and converts
//! to Unix epoch seconds.

use core::sync::atomic::AtomicU64;

/// Unix timestamp captured at boot from CMOS RTC.
static BOOT_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Read a CMOS register.
#[inline]
unsafe fn cmos_read(reg: u8) -> u8 {
    use x86_64::instructions::port::Port;
    let mut idx: Port<u8> = Port::new(0x70);
    let mut val: Port<u8> = Port::new(0x71);
    // Disable NMI (bit 7 = 1) while selecting register
    idx.write(reg | 0x80);
    val.read()
}

/// Convert BCD to binary.
#[inline]
fn bcd_to_bin(bcd: u8) -> u8 {
    (bcd & 0x0F) + ((bcd >> 4) * 10)
}

/// Convert civil date (year/month/day) to days since 1970-01-01.
/// Uses Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Read the CMOS RTC and return Unix epoch seconds.
/// Waits for non-updating state to avoid reading corrupted values.
pub fn read_rtc_epoch() -> u64 {
    unsafe {
        // Wait for RTC not updating (Register A bit 7)
        loop {
            if cmos_read(0x0A) & 0x80 == 0 {
                break;
            }
        }

        // Read registers twice to avoid roll-over
        let (sec1, min1, hour1, day1, mon1, year1, cent1) = loop {
            // Check not updating again
            if cmos_read(0x0A) & 0x80 != 0 {
                continue;
            }
            let s = cmos_read(0x00);
            let mi = cmos_read(0x02);
            let h = cmos_read(0x04);
            let d = cmos_read(0x07);
            let mo = cmos_read(0x08);
            let y = cmos_read(0x09);
            let c = cmos_read(0x32);

            // Read again to verify stability
            if cmos_read(0x0A) & 0x80 != 0 {
                continue;
            }
            let s2 = cmos_read(0x00);
            let mi2 = cmos_read(0x02);
            let h2 = cmos_read(0x04);
            let d2 = cmos_read(0x07);
            let mo2 = cmos_read(0x08);

            if s == s2 && mi == mi2 && h == h2 && d == d2 && mo == mo2 {
                break (s, mi, h, d, mo, y, c);
            }
        };

        // Check if RTC is in BCD mode (Register B bit 2)
        let reg_b = cmos_read(0x0B);
        let is_binary = reg_b & 0x04 != 0;

        let (sec, min, hour, day, mon, year_raw, cent_raw) = if is_binary {
            (sec1, min1, hour1, day1, mon1, year1, cent1)
        } else {
            (
                bcd_to_bin(sec1),
                bcd_to_bin(min1),
                bcd_to_bin(hour1),
                bcd_to_bin(day1),
                bcd_to_bin(mon1),
                bcd_to_bin(year1),
                bcd_to_bin(cent1),
            )
        };

        // Handle 12-hour format (bit 1 of Register B)
        let hour24 = if reg_b & 0x02 == 0 {
            // 12-hour mode
            let mut h = hour & 0x7F;
            if h == 0 {
                h = 12;
            }
            if hour & 0x80 != 0 && h != 12 {
                h += 12;
            }
            h
        } else {
            // 24-hour mode
            hour
        };

        // Compute full year
        let century = if cent_raw == 0 { 20 } else { cent_raw as i64 };
        let full_year = century * 100 + year_raw as i64;

        // Convert to Unix timestamp
        let days = days_from_civil(full_year, mon as i64, day as i64);
        let epoch = days * 86400 + hour24 as i64 * 3600 + min as i64 * 60 + sec as i64;

        epoch as u64
    }
}

/// Initialize the boot epoch from CMOS RTC.
/// Called once during boot.
pub fn init_rtc() {
    let epoch = read_rtc_epoch();
    BOOT_EPOCH.store(epoch, core::sync::atomic::Ordering::Relaxed);
    crate::console_println!("[rtc] Boot time: epoch={} (UTC)", epoch);
}

/// Get the current wall clock time as Unix epoch seconds + sub-second nanoseconds.
/// Returns (seconds, nanoseconds).
pub fn wall_clock() -> (i64, i64) {
    let boot = BOOT_EPOCH.load(core::sync::atomic::Ordering::Relaxed);
    let up_ms = crate::arch::platform::uptime_ms();
    let total_secs = boot + up_ms / 1000;
    let nsecs = ((up_ms % 1000) * 1_000_000) as i64;
    (total_secs as i64, nsecs)
}
