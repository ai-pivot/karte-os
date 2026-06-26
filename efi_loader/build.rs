// efi_loader/build.rs
//
// Resolves KERNEL_BIN_PATH and computes START64_OFFSET from the kernel
// ELF symbol table.  Passes both via cargo:rustc-env.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let kernel_bin_str = env::var("KERNEL_BIN_PATH")
        .expect("KERNEL_BIN_PATH must be set to the kernel flat binary path.\n\
                 Build the kernel first:\n  \
                 cd /home/user/src/karte-os && \n  \
                 cargo +nightly build --release --target x86_64-unknown-none -p karte-os-kernel -Z build-std=core,alloc && \n  \
                 objcopy -O binary target/x86_64-unknown-none/release/karte-os-kernel target/x86_64-unknown-none/release/kernel.bin");

    let kernel_bin = PathBuf::from(&kernel_bin_str);
    let abs_bin = if kernel_bin.is_absolute() {
        kernel_bin
    } else {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        PathBuf::from(manifest_dir)
            .parent()
            .unwrap()
            .join(&kernel_bin)
    };

    if !abs_bin.exists() {
        panic!(
            "KERNEL_BIN_PATH '{}' does not exist. Build the kernel first.",
            abs_bin.display()
        );
    }

    // Derive the ELF path from the flat binary path.
    // Typically: target/.../release/kernel.bin → karte-os-kernel
    let elf_path = abs_bin.parent().unwrap().join("karte-os-kernel");

    // Compute _start64 offset in the flat binary.
    // The kernel ELF is linked at high-half VMA with DIRECT_MAP_BASE.
    // _start64's VMA minus the kernel base (0x10_0000 physical) gives
    // the offset within kernel.bin.
    let start64_offset = if elf_path.exists() {
        compute_start64_offset(&elf_path)
    } else {
        // Fallback if we can't find the ELF (allow START64_OFFSET env override)
        env::var("START64_OFFSET").ok()
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                eprintln!("cargo:warning=Could not find kernel ELF at {} to compute _start64 offset, using default 0x1D8", elf_path.display());
                Some(0x1D8usize)
            })
            .unwrap_or(0x1D8)
    };

    println!("cargo:rustc-env=KERNEL_BIN_PATH={}", abs_bin.display());
    println!("cargo:rustc-env=START64_OFFSET={}", start64_offset);
    println!("cargo:rerun-if-env-changed=KERNEL_BIN_PATH");
    println!("cargo:rerun-if-env-changed=START64_OFFSET");
    println!("cargo:rerun-if-changed={}", abs_bin.display());
}

fn compute_start64_offset(elf_path: &std::path::Path) -> usize {
    // Use 'nm' to find _start64 symbol
    let output = Command::new("nm")
        .arg(elf_path)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());

    let start64_addr = output.as_ref().and_then(|txt| {
        txt.lines()
            .find(|line| line.ends_with(" T _start64") || line.ends_with(" t _start64"))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|addr| usize::from_str_radix(addr, 16).ok())
    });

    match start64_addr {
        Some(addr) => {
            // nm may return either a high-half VMA
            //   (0xFFFF_FFFF_8010_01D8 = DIRECT_MAP_BASE + KERNEL_PHYS_BASE + off)
            // or a low physical address (0x1001D8 = KERNEL_PHYS_BASE + off)
            // depending on how the kernel ELF is linked.  Handle both.
            const DIRECT_MAP_BASE: usize = 0xFFFF_FFFF_8000_0000;
            const KERNEL_PHYS_BASE: usize = 0x10_0000;
            let offset = if addr >= DIRECT_MAP_BASE {
                addr - (DIRECT_MAP_BASE + KERNEL_PHYS_BASE)
            } else if addr >= KERNEL_PHYS_BASE {
                addr - KERNEL_PHYS_BASE
            } else {
                addr
            };
            eprintln!(
                "cargo:info=_start64 offset={:#x} (addr={:#x})",
                offset, addr
            );
            offset
        }
        None => {
            eprintln!("cargo:warning=Could not find _start64 symbol in ELF, using default 0x1D8");
            0x1D8
        }
    }
}
