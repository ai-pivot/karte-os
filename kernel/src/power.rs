//! P3.4 电源管理 — DVFS / 休眠框架接口（真机字段留位）
//!
//! S 档 MCU（ESP32-C3）与 L 档 SoC 的统一电源抽象：
//!   - `PState`：频率/电压档位（真机 DVFS 驱动实现 `set_pstate`）
//!   - `SleepMode`：浅睡（WFI）/ 深睡（SoC suspend，唤醒源字段留位）
//! v0 只做接口与默认无操作实现（QEMU 无 DVFS 模型），真机驱动到位后
//! 按 SoC cfg 提供具体后端；接口形状不变（ROADMAP §P3.4 验收）。

use core::sync::atomic::{AtomicU8, Ordering};

/// 频率/电压档位（档位数量与含义由真机驱动定义；0 = 保留最高性能）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PState {
    /// 最高性能（默认）。
    Max = 0,
    /// 真机字段留位：具体频率由驱动翻译。
    Mid = 1,
    /// 真机字段留位。
    Low = 2,
}

/// 休眠模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum SleepMode {
    /// 浅睡：wfi，任何中断唤醒（tickless idle 已在 timer 层合并 tick）。
    Wfi = 0,
    /// 深睡：SoC suspend；唤醒源（RTC/GPIO/定时器）真机字段留位。
    Deep = 1,
}

static CURRENT_PSTATE: AtomicU8 = AtomicU8::new(0);

/// 当前 DVFS 档位。
pub fn current_pstate() -> PState {
    match CURRENT_PSTATE.load(Ordering::Relaxed) {
        1 => PState::Mid,
        2 => PState::Low,
        _ => PState::Max,
    }
}

/// 请求 DVFS 档位切换（v0：QEMU 无模型，仅记录状态；真机驱动后端接此）。
pub fn set_pstate(s: PState) -> Result<(), ()> {
    CURRENT_PSTATE.store(s as u8, Ordering::Relaxed);
    Ok(())
}

/// 进入休眠（v0：浅睡 = wfi；深睡留位，真机 SoC suspend 驱动实现）。
pub fn enter_sleep(mode: SleepMode) {
    match mode {
        SleepMode::Wfi => unsafe { core::arch::asm!("wfi") },
        SleepMode::Deep => {
            // 真机字段留位：SoC suspend + 唤醒源配置（esp32c3: RTC timer / GPIO wake）。
            // QEMU/无驱动环境退化为浅睡，语义不中断。
            unsafe { core::arch::asm!("wfi") }
        }
    }
}
