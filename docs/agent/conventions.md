# Coding Conventions

## Rust 2024 Edition Requirements

These are **mandatory** — violating them causes compile errors or silent bugs.

### unsafe Declarations

```rust
// Function attributes
#[unsafe(no_mangle)]    // NOT #[no_mangle]
#[unsafe(link_section = ".text.entry")]

// External blocks
unsafe extern "C" {     // NOT extern "C" {}
    fn trap_entry();
}

// Inside unsafe fn, unsafe operations need explicit unsafe {} blocks
// (unsafe_op_in_unsafe_fn is enabled by default in edition 2024)
```

### Function Pointer Casting

```rust
// Correct: two-step cast
fn_ptr as *const () as usize

// Wrong: direct fn → usize is not allowed
fn_ptr as usize  // error
```

### Static Variables

```rust
// Use atomics instead of static mut
static COUNTER: AtomicUsize = AtomicUsize::new(0);
static LOCK: SpinLock<Data> = SpinLock::new(Data { ... });

// Avoid:
// static mut VAR: usize = 0;  // triggers static_mut_refs lint
```

## Console Output

```rust
// Macro is #[macro_export] in sbi.rs → available at crate root
crate::console_println!("text {}", var);  // CORRECT
sbi::console_println!("text {}", var);    // WRONG (won't compile)
```

## Error Handling

- No `std::error::Error` in no_std — use `Result<T, ()>` or custom error types
- Panic handler in `lang_items.rs` — prints to SBI console and shuts down
- Device probe failures: log and continue (don't panic)

## Naming Conventions

- Module names: snake_case (e.g., `virtio.rs`, `spinlock.rs`)
- Types: PascalCase (e.g., `TrapContext`, `TaskControlBlock`)
- Constants: SCREAMING_SNAKE (e.g., `PAGE_SIZE`, `UART_BASE`)
- Public API functions: snake_case (e.g., `alloc_frame`, `set_next_timer`)
- Assembly labels: `.label_name` (local), `global_name` (global)

## Memory Safety Patterns

- MMIO: always use `core::ptr::read_volatile` / `write_volatile`
- Page table allocation: `PageTable::zeroed()` returns `&'static mut`
- Lock guard pattern: `SpinLockGuard` implements `Deref`/`DerefMut`/`Drop`
