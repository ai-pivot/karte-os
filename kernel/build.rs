use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    match target_arch.as_str() {
        "riscv64" => {
            let memory_x = include_bytes!("memory.x");
            fs::write(out_dir.join("memory.x"), memory_x).unwrap();
            println!("cargo:rustc-link-search={}", out_dir.display());
            println!("cargo:rerun-if-changed=memory.x");
        }
        "x86_64" => {
            let memory_ld = include_bytes!("memory-x86_64.ld");
            let dest = out_dir.join("memory-x86_64.ld");
            fs::write(&dest, memory_ld).unwrap();
            println!("cargo:rustc-link-search={}", out_dir.display());
            println!("cargo:rustc-link-arg=-T{}", dest.display());
            // Don't page-align sections, keep them in order
            println!("cargo:rustc-link-arg=--nmagic");
            // Link as static executable (not PIE/DYN)
            println!("cargo:rustc-link-arg=-static");
            // No default libs
            println!("cargo:rustc-link-arg=-nostdlib");
            // No PIE
            println!("cargo:rustc-link-arg=-no-pie");
            println!("cargo:rerun-if-changed=memory-x86_64.ld");

            // Assemble AP trampoline using system assembler
            let asm_src = fs::canonicalize("src/arch/x86_64/ap_trampoline.S").unwrap();
            let asm_obj = out_dir.join("ap_trampoline.o");
            let status = std::process::Command::new("as")
                .arg("--64")
                .arg("-o")
                .arg(&asm_obj)
                .arg(&asm_src)
                .status()
                .expect("Failed to run `as` assembler. Install: sudo apt install binutils");
            if !status.success() {
                panic!("Failed to assemble ap_trampoline.S");
            }
            println!("cargo:rustc-link-arg={}", asm_obj.display());
            println!("cargo:rerun-if-changed=src/arch/x86_64/ap_trampoline.S");
        }
        _ => {}
    }
}
