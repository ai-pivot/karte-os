.PHONY: build run debug clean doc check build-test test boot-test

# Project settings
KERNEL_BIN := target/riscv64gc-unknown-none-elf/release/karte-os-kernel
KERNEL_ELF := target/riscv64gc-unknown-none-elf/release/karte-os-kernel

# QEMU settings
QEMU := qemu-system-riscv64
QEMU_FLAGS := \
	-machine virt \
	-cpu rv64 \
	-nographic \
	-bios default \
	-m 128M \
	-smp 1

# Build the kernel
build:
	cargo build --release -p karte-os-kernel

# Run in QEMU
run: build
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL_ELF)

# Run with GDB support
debug: build
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL_ELF) -S -s

# Run for 10 seconds then kill (for CI testing)
test-run: build
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

# Build test kernel
build-test:
	cargo build --release -p karte-os-kernel --features test_mode

# Run tests in QEMU
test: build-test
	bash scripts/run-tests.sh

# Run boot test (normal mode)
boot-test: build
	@echo "Running boot test..."
	@timeout 10 qemu-system-riscv64 -machine virt -cpu rv64 -nographic \
		-bios default -m 128M -smp 1 \
		-kernel $(KERNEL_ELF) 2>&1 | grep -qa "initialized successfully" \
		&& echo "✅ Boot test passed" || echo "❌ Boot test failed"
