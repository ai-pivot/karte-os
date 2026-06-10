.PHONY: build run debug clean doc check build-test test boot-test smp-test \
        shell disk user-build user-clean \
        run-riscv run-x86 build-riscv build-x86 shell-riscv shell-x86 \
        setup-riscv setup-x86 usb-image usb-write run-9p

# ═══════════════════════════════════════════════════════════════
#  KarteOS — Dual-Architecture Makefile
#
#  Quick Start:
#    make run          → Build & run on RISC-V (default)
#    make run-riscv    → Same as above
#    make run-x86      → Build & run on x86_64
#    make shell        → Build user programs + kernel + run (RISC-V)
#    make shell-x86    → Build user programs + kernel + run (x86_64)
#    make test         → Run RISC-V integration tests
#    make disk         → Create disk.img (FAT32 + ext4)
#
#  Requirements:
#    RISC-V:  rust stable (1.93+), qemu-system-riscv64, gcc-riscv64-linux-gnu
#    x86_64:  rust nightly, qemu-system-x86_64, grub-mkrescue (grub-common+xorriso)
# ═══════════════════════════════════════════════════════════════

# ── RISC-V config ──
QEMU_RV    := qemu-system-riscv64
TARGET_RV  := riscv64gc-unknown-none-elf
KERNEL_RV  := target/$(TARGET_RV)/release/karte-os-kernel
QEMU_RV_FLAGS := \
  -machine virt -cpu rv64 -bios default -nographic \
  -m 128M -smp 1 \
  -drive id=blk0,file=disk.img,format=raw,if=none \
  -device virtio-blk-device,drive=blk0 \
  -netdev user,id=net0,hostfwd=tcp::2323-:23,hostfwd=udp::2323-:23 \
  -device virtio-net-device,netdev=net0

# ── x86_64 config ──
QEMU_X86     := qemu-system-x86_64
TARGET_X86   := x86_64-unknown-none
KERNEL_X86   := target/$(TARGET_X86)/release/karte-os-kernel
ISO_DIR      := target/x86_64-iso
ISO_FILE     := target/karte-os-x86_64.iso
QEMU_X86_FLAGS := \
  -machine pc -cpu qemu64 -m 128M -smp 1 \
  -cdrom $(ISO_FILE) -serial stdio -display none -no-reboot \
  -drive file=disk.img,format=raw,if=none,id=hd0 \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=hd0,bus=ahci.0

# ═══════════════════════════════════════════════════════════════
#  Disk image (auto-created if missing)
# ═══════════════════════════════════════════════════════════════
disk.img:
	@echo "[disk] Creating 64MB disk image..."
	@tools/mkdisk.sh init
	@echo "[disk] Done. Use 'tools/mkdisk.sh put <file>' to add files."

disk: disk.img

# ═══════════════════════════════════════════════════════════════
#  RISC-V 64 (primary, stable Rust)
# ═══════════════════════════════════════════════════════════════

## Build RISC-V kernel
build-riscv:
	@cd user && $(MAKE) ARCH=riscv64 clean && $(MAKE) ARCH=riscv64
	@# Force kernel rebuild to pick up correct user binaries
	@rm -f $(KERNEL_RV)
	@rm -rf target/$(TARGET_RV)/release/.fingerprint/karte-os-kernel-*
	cargo build --release -p karte-os-kernel

## Run on RISC-V QEMU
run-riscv: build-riscv disk.img
	$(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV)

## Debug on RISC-V (GDB stub)
debug-riscv: build-riscv disk.img
	$(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV) -S -s

## Build user programs + kernel + run shell (RISC-V)
shell-riscv: disk.img
	@cd user && $(MAKE) ARCH=riscv64 clean && $(MAKE) ARCH=riscv64
	@rm -f $(KERNEL_RV)
	@rm -rf target/$(TARGET_RV)/release/.fingerprint/karte-os-kernel-*
	cargo build --release -p karte-os-kernel
	$(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV)

## Run RISC-V tests
test:
	@cd user && $(MAKE) ARCH=riscv64 clean && $(MAKE) ARCH=riscv64
	@rm -f $(KERNEL_RV)
	@rm -rf target/$(TARGET_RV)/release/.fingerprint/karte-os-kernel-*
	cargo build --release -p karte-os-kernel --features test_mode
	@bash scripts/run-tests.sh
	@echo ""
	@echo "Restoring normal kernel build..."
	@rm -f $(KERNEL_RV)
	@rm -rf target/$(TARGET_RV)/release/.fingerprint/karte-os-kernel-*
	@cargo build --release -p karte-os-kernel

## Run x86_64 integration tests in QEMU
test-x86:
	@bash scripts/run-tests-x86_64.sh

## Run ALL tests (RISC-V + x86_64)
test-all: test test-x86

## Build test kernel only
build-test:
	@$(MAKE) user-build ARCH=riscv64
	cargo build --release -p karte-os-kernel --features test_mode

## Boot test (verify shell starts)
boot-test: build-riscv disk.img
	@echo "Running boot test..."
	@timeout 10 $(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV) 2>&1 | grep -qa "KarteOS Shell" \
		&& echo "Boot test passed" || echo "Boot test failed"

## SMP test
smp-test: disk.img
	@$(MAKE) user-build ARCH=riscv64
	@rm -f $(KERNEL_RV)
	cargo build --release -p karte-os-kernel
	@timeout 15 $(QEMU_RV) -machine virt -cpu rv64 -bios default -nographic -m 128M -smp 4 \
		-drive id=blk0,file=disk.img,format=raw,if=none -device virtio-blk-device,drive=blk0 \
		-netdev user,id=net0 -device virtio-net-device,netdev=net0 \
		-kernel $(KERNEL_RV) 2>&1 | grep -qa "KarteOS Shell" \
		&& echo "SMP test passed" || echo "SMP test failed"

# ═══════════════════════════════════════════════════════════════
#  x86_64 (secondary, nightly Rust + GRUB ISO)
# ═══════════════════════════════════════════════════════════════

## Build x86_64 kernel (needs nightly + grub-mkrescue)
build-x86:
	@cd user && $(MAKE) ARCH=x86_64 clean && $(MAKE) ARCH=x86_64
	@# Create stub files for RISC-V-only assembly programs (cfg-gated out)
	@touch user/hello.elf user/heap_test.elf user/file_test.elf user/spawn_test.elf
	@# Force kernel rebuild to pick up new user binaries
	@rm -f $(KERNEL_X86)
	@rm -rf target/$(TARGET_X86)/release/.fingerprint/karte-os-kernel-*
	cargo +nightly build --release --target $(TARGET_X86) -p karte-os-kernel -Z build-std=core,alloc
	@mkdir -p $(ISO_DIR)/boot/grub
	@cp $(KERNEL_X86) $(ISO_DIR)/boot/karte-os-kernel
	@printf 'set timeout=0\nset default=0\nmenuentry "KarteOS" {\n    multiboot2 /boot/karte-os-kernel\n    boot\n}\n' > $(ISO_DIR)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO_FILE) $(ISO_DIR) 2>/dev/null
	@echo "[x86_64] Build complete: $(ISO_FILE)"

## Run on x86_64 QEMU
run-x86: build-x86 disk.img
	$(QEMU_X86) $(QEMU_X86_FLAGS)

## Debug on x86_64 (GDB stub)
debug-x86: build-x86 disk.img
	$(QEMU_X86) $(QEMU_X86_FLAGS) -S -s

## Build user programs + kernel + run shell (x86_64)
shell-x86: disk.img
	@cd user && $(MAKE) ARCH=x86_64 clean && $(MAKE) ARCH=x86_64
	@touch user/hello.elf user/heap_test.elf user/file_test.elf user/spawn_test.elf
	@rm -f $(KERNEL_X86)
	@rm -rf target/$(TARGET_X86)/release/.fingerprint/karte-os-kernel-*
	cargo +nightly build --release --target $(TARGET_X86) -p karte-os-kernel -Z build-std=core,alloc
	@mkdir -p $(ISO_DIR)/boot/grub
	@cp $(KERNEL_X86) $(ISO_DIR)/boot/karte-os-kernel
	@printf 'set timeout=0\nset default=0\nmenuentry "KarteOS" {\n    multiboot2 /boot/karte-os-kernel\n    boot\n}\n' > $(ISO_DIR)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO_FILE) $(ISO_DIR) 2>/dev/null
	$(QEMU_X86) $(QEMU_X86_FLAGS)

# ═══════════════════════════════════════════════════════════════
#  Default targets (RISC-V)
# ═══════════════════════════════════════════════════════════════
build: build-riscv
run:   run-riscv
debug: debug-riscv
shell: shell-riscv

# ═══════════════════════════════════════════════════════════════
#  Common targets
# ═══════════════════════════════════════════════════════════════

## Build user programs (ARCH=riscv64 or ARCH=x86_64)
user-build:
	@cd user && $(MAKE) ARCH=$(ARCH)

## Clean user programs
user-clean:
	@cd user && $(MAKE) clean

## Clean all build artifacts
clean:
	cargo clean
	rm -rf $(ISO_DIR) $(ISO_FILE)

## Generate docs
doc:
	cargo doc -p karte-os-kernel --no-deps

## Check without building
check:
	cargo check --release -p karte-os-kernel

## Format code
fmt:
	cargo fmt
	@cd user && for f in *.rs; do rustfmt --edition 2024 $$f 2>/dev/null; done; true

## Install RISC-V dependencies (Ubuntu/Debian)
setup-riscv:
	@echo "Installing RISC-V toolchain..."
	rustup target add riscv64gc-unknown-none-elf
	sudo apt-get install -y qemu-system-riscv64 gcc-riscv64-linux-gnu

## Install x86_64 dependencies (Ubuntu/Debian)
setup-x86:
	@echo "Installing x86_64 toolchain..."
	rustup toolchain install nightly
	rustup component add rust-src --toolchain nightly
	sudo apt-get install -y qemu-system-x86_64 grub-common xorriso

## Show help
help:
	@echo ""
	@echo "KarteOS — Dual-Architecture OS (RISC-V 64 + x86_64)"
	@echo ""
	@echo "  Quick Start:"
	@echo "    make run          Build & run on RISC-V (default)"
	@echo "    make run-x86      Build & run on x86_64"
	@echo "    make shell        Build all + run interactive shell (RISC-V)"
	@echo "    make shell-x86    Build all + run interactive shell (x86_64)"
	@echo "    make disk         Create disk.img if missing"
	@echo ""
	@echo "  Testing (RISC-V):"
	@echo "    make test         Run 70 integration tests"
	@echo "    make boot-test    Verify boot reaches shell"
	@echo "    make smp-test     Test with 4 CPU cores"
	@echo ""
	@echo "  Development:"
	@echo "    make build-riscv  Build RISC-V kernel only"
	@echo "    make build-x86    Build x86_64 kernel + ISO"
	@echo "    make debug        Run with GDB stub (-S -s)"
	@echo "    make fmt          Format all code"
	@echo "    make clean        Remove all build artifacts"
	@echo ""
	@echo "  Setup (Ubuntu/Debian):"
	@echo "    make setup-riscv  Install RISC-V toolchain"
	@echo "    make setup-x86    Install x86_64 toolchain"
	@echo ""
	@echo "  Disk image tools:"
	@echo "    tools/mkdisk.sh init          Create 64MB disk"
	@echo "    tools/mkdisk.sh put <file>    Copy file to disk"
	@echo "    tools/mkdisk.sh list          List files on disk"
	@echo ""
	@echo "  USB / Installation:"
	@echo "    make usb-image    Create bootable USB disk image"
	@echo "    make usb-write    Write to USB drive (set USB_DEV=/dev/sdX)"
	@echo ""
	@echo "  QEMU exit: Ctrl+A then X"
	@echo ""

# ═══════════════════════════════════════════════════════════════
#  USB / Installation (x86_64)
# ═══════════════════════════════════════════════════════════════

## Create bootable USB image (512MB by default, set USB_SIZE_MB=...)
usb-image: build-x86 disk.img
	@echo "[usb] Creating bootable USB image..."
	sudo tools/mkusb.sh image
	@echo "[usb] Image: target/karte-os-usb.img"
	@echo "[usb] Write to USB: dd if=target/karte-os-usb.img of=/dev/sdX bs=4M status=progress"

## Write directly to USB drive (set USB_DEV=/dev/sdX)
usb-write: build-x86 disk.img
	@test -n "$(USB_DEV)" || (echo "Usage: make usb-write USB_DEV=/dev/sdX" && exit 1)
	sudo tools/mkusb.sh $(USB_DEV)

## Run x86_64 QEMU with virtio-9p host directory sharing
## Usage: make run-9p HOST_DIR=/path/to/share
run-9p: build-x86 disk.img
	$(QEMU_X86) $(QEMU_X86_FLAGS) \
	  -fsdev local,id=share1,path=$(HOST_DIR),security_model=none \
	  -device virtio-9p-pci,fsdev=share1,mount_tag=hostshare
