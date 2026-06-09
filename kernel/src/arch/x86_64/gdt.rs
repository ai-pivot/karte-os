//! GDT (Global Descriptor Table) setup using the `x86_64` crate.
//!
//! On x86_64, the GDT is mostly legacy — segmentation is effectively disabled
//! in long mode. We still need it for:
//! - Kernel code/data segments
//! - User code/data segments (Ring 3)
//! - TSS (Task State Segment) for IST (Interrupt Stack Table) entries
//!   used by double fault handling
//!
//! SMP: Each CPU has its own GDT + TSS (because TSS.RSP0 is per-CPU).
//! MAX_CPUS = 4 is enough for QEMU -smp 4.

use core::sync::atomic::Ordering;

use spin::Once;
use x86_64::VirtAddr;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

/// Maximum number of CPUs supported.
pub const MAX_CPUS: usize = 4;

/// IST index for double fault handler (separate stack to avoid corruption).
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// IST index for syscall (int 0x80) handler — separate stack so that
/// nested Timer ISR (which uses TSS.RSP0) does not clobber the syscall
/// handler's CPU-pushed iretq frame (RIP, CS, RFLAGS, RSP, SS).
pub const SYSCALL_IST_INDEX: u16 = 1;

/// IST index for Timer ISR — separate stack so that IOAPIC-routed external
/// interrupts (keyboard, UART) can nest on TSS.RSP0 without clobbering the
/// Timer ISR's stack frame.
pub const TIMER_IST_INDEX: u16 = 2;

/// IST index for external IRQ ISR (keyboard).
pub const KEYBOARD_IST_INDEX: u16 = 3;

/// IST index for external IRQ ISR (COM1 UART).
/// Separate from keyboard to prevent IST[3] stack corruption when both
/// IRQ1 and IRQ4 fire during the same Timer ISR 'sti' window.
pub const COM1_IST_INDEX: u16 = 4;

/// IST index for Page Fault handler — uses dedicated stack and CR3 switch
/// so that kernel data structures are accessible during page fault handling.
pub const PAGE_FAULT_IST_INDEX: u16 = 5;

/// Number of IST entries we actually use.
const NUM_IST: usize = 6;

const IST_STACK_SIZE: usize = 4096 * 8; // 32KB per IST stack

/// Per-CPU GDT structure.
struct PerCpuGdt {
    gdt: GlobalDescriptorTable,
    code: SegmentSelector,
    data: SegmentSelector,
    user_code: SegmentSelector,
    user_data: SegmentSelector,
    tss: SegmentSelector,
}

/// IST stacks — one set per CPU, one per IST index.
static mut IST_STACKS: [[u8; IST_STACK_SIZE]; MAX_CPUS * NUM_IST] =
    [[0u8; IST_STACK_SIZE]; MAX_CPUS * NUM_IST];

/// TSS — one per CPU.
static mut PER_CPU_TSS: [TaskStateSegment; MAX_CPUS] = [
    TaskStateSegment::new(),
    TaskStateSegment::new(),
    TaskStateSegment::new(),
    TaskStateSegment::new(),
];

/// GDT + cached selectors — one per CPU.
static PER_CPU_GDT: [Once<PerCpuGdt>; MAX_CPUS] =
    [Once::new(), Once::new(), Once::new(), Once::new()];

/// User code segment selector value (for iretq).
/// After GDT init, this is set to the actual selector.
pub static USER_CODE_SEL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
/// User data segment selector value (for iretq).
pub static USER_DATA_SEL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
/// Kernel code segment selector value.
pub static KCODE_SEL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// Initialize the GDT for a specific CPU and load it into the GDTR.
/// Also loads the TSS and all segment registers.
///
/// # Safety
/// Must be called once per CPU during early boot, before any interrupt
/// or user-mode code is used on that CPU.
pub fn init() {
    init_for_cpu(0);
}

/// Initialize the GDT for a specific CPU.
pub fn init_for_cpu(cpu_id: usize) {
    let cpu_id = cpu_id.min(MAX_CPUS - 1);

    PER_CPU_GDT[cpu_id].call_once(|| {
        // Set up IST stacks for this CPU's TSS
        unsafe {
            // IST[0]: Double Fault
            let df_stack_top = VirtAddr::new(
                IST_STACKS[cpu_id * NUM_IST].as_ptr() as u64
                    + IST_STACKS[cpu_id * NUM_IST].len() as u64,
            );
            PER_CPU_TSS[cpu_id].interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
                df_stack_top;

            // IST[1]: Syscall (int 0x80) — prevents Timer ISR nesting from
            // clobbering the syscall handler's CPU-pushed iretq frame.
            let sys_stack_top = VirtAddr::new(
                IST_STACKS[cpu_id * NUM_IST + 1].as_ptr() as u64
                    + IST_STACKS[cpu_id * NUM_IST + 1].len() as u64,
            );
            PER_CPU_TSS[cpu_id].interrupt_stack_table[SYSCALL_IST_INDEX as usize] = sys_stack_top;

            // IST[2]: Timer ISR — prevents IOAPIC-routed IRQs (keyboard, UART)
            // from clobbering the Timer ISR's stack frame when they nest during
            // the Timer handler's 'sti'.
            let timer_stack_top = VirtAddr::new(
                IST_STACKS[cpu_id * NUM_IST + 2].as_ptr() as u64
                    + IST_STACKS[cpu_id * NUM_IST + 2].len() as u64,
            );
            PER_CPU_TSS[cpu_id].interrupt_stack_table[TIMER_IST_INDEX as usize] = timer_stack_top;

            // IST[3]: Keyboard handler
            let kbd_stack_top = VirtAddr::new(
                IST_STACKS[cpu_id * NUM_IST + 3].as_ptr() as u64
                    + IST_STACKS[cpu_id * NUM_IST + 3].len() as u64,
            );
            PER_CPU_TSS[cpu_id].interrupt_stack_table[KEYBOARD_IST_INDEX as usize] = kbd_stack_top;

            // IST[4]: COM1 UART handler
            let com1_stack_top = VirtAddr::new(
                IST_STACKS[cpu_id * NUM_IST + 4].as_ptr() as u64
                    + IST_STACKS[cpu_id * NUM_IST + 4].len() as u64,
            );
            PER_CPU_TSS[cpu_id].interrupt_stack_table[COM1_IST_INDEX as usize] = com1_stack_top;

            // IST[5]: Page Fault handler
            let pf_stack_top = VirtAddr::new(
                IST_STACKS[cpu_id * NUM_IST + 5].as_ptr() as u64
                    + IST_STACKS[cpu_id * NUM_IST + 5].len() as u64,
            );
            PER_CPU_TSS[cpu_id].interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] = pf_stack_top;
        }

        let mut gdt = GlobalDescriptorTable::new();

        // IMPORTANT: GDT entry order is critical for SYSCALL/SYSRET compatibility.
        // Intel SYSRET: CS = STAR[48:63]+16, SS = STAR[48:63]+8 (RPL=3)
        // AMD SYSRET:   CS = STAR[32:47]+16, SS = STAR[32:47]+8 (RPL=3)
        // SYSCALL:      CS = STAR[48:63],    SS = STAR[48:63]+8
        //
        // With STAR = (0x08<<48)|(0x08<<32):
        //   SYSCALL: CS=0x08 (kernel code), SS=0x10 (kernel data)
        //   SYSRET:  CS=0x08+16=0x18 (user code), SS=0x08+8=0x10 (kernel data, OK in 64-bit)
        //   (SS=0x10|3=0x13 is fine — 64-bit mode skips DPL check for SS)
        //
        // GDT layout:
        //   Entry 1 (0x08): kernel code  ← SYSCALL CS
        //   Entry 2 (0x10): kernel data  ← SYSCALL SS
        //   Entry 3 (0x18): user code    ← SYSRET CS = 0x18|3 = 0x1B
        //   Entry 4 (0x20): user data    ← iretq SS = 0x20|3 = 0x23
        let code = gdt.append(Descriptor::kernel_code_segment()); // 0x08
        let data = gdt.append(Descriptor::kernel_data_segment()); // 0x10
        let user_code = gdt.append(Descriptor::user_code_segment()); // 0x18 → 0x1B with RPL
        let user_data = gdt.append(Descriptor::user_data_segment()); // 0x20 → 0x23 with RPL
        let tss = unsafe {
            gdt.append(Descriptor::tss_segment(&*core::ptr::addr_of!(
                PER_CPU_TSS[cpu_id]
            )))
        };

        // Cache selector values with RPL=3 for Ring 3 user mode
        USER_CODE_SEL.store(user_code.0 | 3, Ordering::Relaxed); // 0x18|3 = 0x1B
        USER_DATA_SEL.store(user_data.0 | 3, Ordering::Relaxed); // 0x20|3 = 0x23
        KCODE_SEL.store(code.0, Ordering::Relaxed);
        crate::console_println!(
            "[GDT] code={:#x} data={:#x} user_code={:#x} user_data={:#x} tss={:#x}",
            code.0,
            data.0,
            user_code.0,
            user_data.0,
            tss.0
        );

        PerCpuGdt {
            gdt,
            code,
            data,
            user_code,
            user_data,
            tss,
        }
    });

    let gdt = PER_CPU_GDT[cpu_id].get().unwrap();

    // Load GDTR
    gdt.gdt.load();

    // Reload all segment registers + TSS
    unsafe {
        use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
        use x86_64::instructions::tables::load_tss;

        CS::set_reg(gdt.code);
        DS::set_reg(gdt.data);
        ES::set_reg(gdt.data);
        FS::set_reg(gdt.data);
        GS::set_reg(gdt.data);
        SS::set_reg(gdt.data);
        load_tss(gdt.tss);
    }
}

/// Update the TSS.RSP0 for the current CPU.
///
/// # Safety
/// The caller must ensure `kernel_stack_top` points to valid, accessible memory.
/// Physical address of PER_CPU_TSS[0].privilege_stack_table[0].
/// Used by trap_return_user asm to directly update TSS.RSP0 without calling Rust.
#[cfg(target_arch = "x86_64")]
/// Atomic version of TSS_RSP0_ADDR for lock-free RSP0 updates from schedule().
pub static TSS_RSP0_ADDR_ATOMIC: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

#[unsafe(no_mangle)]
pub static mut TSS_RSP0_ADDR: u64 = 0;

/// Set TSS.RSP0 for a specific CPU without calling current_hart().
/// Used in schedule() where calling current_hart() in ISR context may be unsafe.
#[cfg(target_arch = "x86_64")]
pub unsafe fn set_kernel_rsp0_for_cpu(cpu_id: usize, kernel_stack_top: u64) {
    unsafe {
        let tss = &mut PER_CPU_TSS[cpu_id.min(MAX_CPUS - 1)];
        tss.privilege_stack_table[0] = x86_64::VirtAddr::new_truncate(kernel_stack_top);
    }
}

pub unsafe fn set_kernel_rsp0(kernel_stack_top: u64) {
    let cpu_id = crate::arch::smp::current_hart().min(MAX_CPUS - 1);
    unsafe {
        PER_CPU_TSS[cpu_id].privilege_stack_table[0] = x86_64::VirtAddr::new(kernel_stack_top);
        // Store address for trap_return_user asm (direct TSS write)
        let rsp0_ptr = core::ptr::addr_of_mut!(PER_CPU_TSS[0].privilege_stack_table[0]);
        TSS_RSP0_ADDR = rsp0_ptr as u64;
        TSS_RSP0_ADDR_ATOMIC.store(rsp0_ptr as u64, core::sync::atomic::Ordering::Relaxed);
    }
}
