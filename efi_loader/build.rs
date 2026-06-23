// efi_loader/build.rs
//
// Resolves KERNEL_BIN_PATH to an absolute path and passes it to the compiler
// via cargo:rustc-env so include_bytes!(env!("KERNEL_BIN_PATH")) works.

use std::env;
use std::path::PathBuf;

fn main() {
    let kernel_path_str = env::var("KERNEL_BIN_PATH")
        .expect("KERNEL_BIN_PATH must be set to the kernel flat binary path.\n\
                 Build the kernel first:\n  \
                 cd /home/user/src/karte-os && \n  \
                 cargo +nightly build --release --target x86_64-unknown-none -p karte-os-kernel -Z build-std=core,alloc && \n  \
                 objcopy -O binary target/x86_64-unknown-none/release/karte-os-kernel target/x86_64-unknown-none/release/kernel.bin");

    let kernel_path = PathBuf::from(&kernel_path_str);
    let abs_path = if kernel_path.is_absolute() {
        kernel_path
    } else {
        // Resolve relative to workspace root (parent of efi_loader/)
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        PathBuf::from(manifest_dir)
            .parent()
            .unwrap()
            .join(&kernel_path)
    };

    if !abs_path.exists() {
        panic!(
            "KERNEL_BIN_PATH '{}' does not exist. Build the kernel first.",
            abs_path.display()
        );
    }

    println!("cargo:rustc-env=KERNEL_BIN_PATH={}", abs_path.display());
    println!("cargo:rerun-if-env-changed=KERNEL_BIN_PATH");
    println!("cargo:rerun-if-changed={}", abs_path.display());
}
