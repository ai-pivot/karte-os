.PHONY: build run debug clean doc check build-test test boot-test smp-test test-run disk.img shell

# 默认架构
ARCH ?= riscv64

# 架构特定配置
ifeq ($(ARCH),riscv64)
    QEMU := qemu-system-riscv64
    QEMU_MACHINE := virt
    QEMU_CPU := rv64
    QEMU_BIOS := -bios default
    TARGET := riscv64gc-unknown-none-elf
    KERNEL_ELF := target/$(TARGET)/release/karte-os-kernel
    QEMU_BLOCKDEV := -drive id=blk0,file=disk.img,format=raw,if=none -device virtio-blk-device,drive=blk0
else ifeq ($(ARCH),x86_64)
    QEMU := qemu-system-x86_64
    QEMU_MACHINE := q35
    QEMU_CPU := qemu64
    QEMU_BIOS :=
    TARGET := x86_64-unknown-none
    KERNEL_ELF := target/$(TARGET)/release/karte-os-kernel
    QEMU_BLOCKDEV := -drive file=disk.img,format=raw,if=virtio
endif

QEMU_FLAGS := \
 -machine $(QEMU_MACHINE) \
 -cpu $(QEMU_CPU) \
 -nographic \
 $(QEMU_BIOS) \
 -m 128M \
 -smp 1 \
 $(QEMU_BLOCKDEV)

# Ensure disk image exists (64MB FAT32 formatted)
disk.img:
	@if [ ! -f disk.img ]; then \
		echo "Creating 64MB FAT32 disk image..."; \
		dd if=/dev/zero of=disk.img bs=1M count=64 2>/dev/null; \
		mkfs.vfat -F 32 disk.img 2>/dev/null || true; \
	fi

# Build the kernel (force rebuild to avoid stale test_mode binary)
# We delete the binary + fingerprint because cargo's incremental cache doesn't
# distinguish between different --features flags — 'make test' builds with
# test_mode, which would otherwise be reused by 'make build'.
ifeq ($(ARCH),riscv64)
build:
	@rm -f target/riscv64gc-unknown-none-elf/release/karte-os-kernel
	@rm -rf target/riscv64gc-unknown-none-elf/release/.fingerprint/karte-os-kernel-*
	cargo build --release -p karte-os-kernel
else ifeq ($(ARCH),x86_64)
build:
	cd user && $(MAKE) ARCH=x86_64 || true
	cargo build --release --target kernel/x86_64-karte-os.json -p karte-os-kernel
endif

# Run in QEMU
run: build disk.img
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL_ELF)

# Run with GDB support
debug: build disk.img
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL_ELF) -S -s

# Run for 10 seconds then kill (for CI testing)
test-run: build disk.img
	timeout 10 $(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL_ELF) || true

# Clean build artifacts
clean:
	cargo clean

# Generate documentation
doc:
	cargo doc -p karte-os-kernel --no-deps

# Check without building
check:
	cargo check --release -p karte-os-kernel

# Build test kernel (outputs to same path, use 'make build' after to restore)
build-test:
	cargo build --release -p karte-os-kernel --features test_mode

# Run tests in QEMU (test_mode kernel), then restore normal build
test: build-test
	bash scripts/run-tests.sh
	@echo ""
	@echo "Restoring normal kernel build..."
	@rm -f target/riscv64gc-unknown-none-elf/release/karte-os-kernel
	@rm -rf target/riscv64gc-unknown-none-elf/release/.fingerprint/karte-os-kernel-*
	@cargo build --release -p karte-os-kernel

# Run boot test (normal mode)
boot-test: build disk.img
	@echo "Running boot test..."
	@timeout 10 $(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL_ELF) 2>&1 | grep -qa "KarteOS Shell" \
		&& echo "Boot test passed" || echo "Boot test failed"

# Build shell + kernel and run
shell: disk.img
	cd user && $(MAKE)
	@rm -f target/riscv64gc-unknown-none-elf/release/karte-os-kernel
	@rm -rf target/riscv64gc-unknown-none-elf/release/.fingerprint/karte-os-kernel-*
	cargo build --release -p karte-os-kernel
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL_ELF)
