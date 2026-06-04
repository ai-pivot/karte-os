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

/// Number of IST entries we actually use.
const NUM_IST: usize = 2;

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
        }

        let mut gdt = GlobalDescriptorTable::new();

        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let tss = unsafe {
            gdt.append(Descriptor::tss_segment(&*core::ptr::addr_of!(
                PER_CPU_TSS[cpu_id]
            )))
        };

        // Cache selector values (same for all CPUs since GDT layout is identical)
        USER_CODE_SEL.store(user_code.0, Ordering::Relaxed);
        USER_DATA_SEL.store(user_data.0, Ordering::Relaxed);
        KCODE_SEL.store(code.0, Ordering::Relaxed);

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
pub unsafe fn set_kernel_rsp0(kernel_stack_top: u64) {
    // Determine which CPU we're on by checking which TSS has been loaded.
    // Simple approach: use LAPIC ID to index into per-CPU TSS array.
    let cpu_id = crate::arch::smp::current_hart().min(MAX_CPUS - 1);
    unsafe {
        PER_CPU_TSS[cpu_id].privilege_stack_table[0] = x86_64::VirtAddr::new(kernel_stack_top);
    }
}
