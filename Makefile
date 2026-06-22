.PHONY: build run debug clean doc check build-test test boot-test smp-test \
        deploy deploy-riscv deploy-x86 user-build user-clean \
        run-riscv run-x86 build-riscv build-x86 shell-riscv shell-x86 \
        iso-riscv iso-x86 setup-riscv setup-x86 \
        share-riscv share-x86 usb-image usb-write \
        release release-riscv release-x86 release-all

# ═══════════════════════════════════════════════════════════════
#  KarteOS — Dual-Architecture Makefile
#
#  The simplest way to build and run:
#
#    make              → Build & run RISC-V (default)
#    make iso-x86      → Build x86_64 kernel + ISO + deploy programs → run
#    make deploy       → Create disk.img with all RISC-V programs
#    make deploy-x86   → Create disk.img with all x86_64 programs
#    make test-all     → Run tests on both architectures
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
  -m 256M -smp 1 \
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
  -machine pc -cpu qemu64 -m 512M -smp 2 \
  -cdrom $(ISO_FILE) -serial stdio -display none -no-reboot \
  -drive file=disk.img,format=raw,if=none,id=hd0 \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=hd0,bus=ahci.0 \
  -netdev user,id=net0 \
  -device virtio-net-pci,netdev=net0

# Shared folder default
HOST_DIR ?= /tmp/karteos-share

# ═══════════════════════════════════════════════════════════════
#  Disk image — auto-created with user programs
# ═══════════════════════════════════════════════════════════════

## Create disk.img and deploy all RISC-V user programs (one command!)
deploy: disk.img
	@echo "[deploy] Installing RISC-V programs..."
	@cd user && $(MAKE) ARCH=riscv64 clean > /dev/null 2>&1 && $(MAKE) ARCH=riscv64 > /dev/null 2>&1
	@bash tools/mkdisk.sh deploy-riscv

## Create disk.img and deploy all x86_64 user programs + xbot + /etc + CA certs
deploy-x86:
	@echo "[deploy] Rebuilding disk image from scratch..."
	@rm -f disk.img
	@bash tools/mkdisk.sh init > /dev/null 2>&1
	@cd user && $(MAKE) ARCH=x86_64 clean > /dev/null 2>&1 && $(MAKE) ARCH=x86_64 > /dev/null 2>&1
	@bash tools/mkdisk.sh deploy-x86
	@if [ -f xbot-cli-static-x86_64 ]; then \
		DISK=disk.img bash tools/mkdisk.sh put xbot-cli-static-x86_64 xbot-cli-static > /dev/null 2>&1; \
		echo "[deploy] xbot-cli-static deployed"; \
	fi
	@mkdir -p /tmp/karte_etc
	@printf '127.0.0.1 localhost\n' > /tmp/karte_etc/hosts
	@printf 'nameserver 10.0.2.3\n' > /tmp/karte_etc/resolv.conf
	@printf 'mkdir etc\nmkdir etc/ssl\nmkdir etc/ssl/certs\nmkdir etc/pki\nmkdir etc/pki/tls\nmkdir etc/pki/tls/certs\nwrite /tmp/karte_etc/hosts etc/hosts\nwrite /tmp/karte_etc/resolv.conf etc/resolv.conf\n' > /tmp/_dbg_deploy
	@if [ -f /etc/ssl/certs/ca-certificates.crt ]; then \
		printf 'write /etc/ssl/certs/ca-certificates.crt etc/ssl/certs/ca-certificates.crt\n' >> /tmp/_dbg_deploy; \
		printf 'write /etc/ssl/certs/ca-certificates.crt etc/pki/tls/certs/ca-bundle.crt\n' >> /tmp/_dbg_deploy; \
	fi
	@cat /tmp/_dbg_deploy | debugfs -w disk.img 2>/dev/null
	@echo "[deploy] /etc/hosts, resolv.conf, ssl/certs deployed"

## Create empty disk.img if missing
disk.img:
	@echo "[disk] Creating 64MB disk image..."
	@bash tools/mkdisk.sh init

disk: disk.img

# ═══════════════════════════════════════════════════════════════
#  Internal helpers — build user programs + kernel
# ═══════════════════════════════════════════════════════════════

# Build RISC-V user programs + kernel (force rebuild for include_bytes)
_build-riscv-kernel:
	@cd user && $(MAKE) ARCH=riscv64 clean > /dev/null 2>&1 && $(MAKE) ARCH=riscv64 > /dev/null 2>&1
	@rm -f $(KERNEL_RV)
	@rm -rf target/$(TARGET_RV)/release/.fingerprint/karte-os-kernel-*
	cargo build --release -p karte-os-kernel --target $(TARGET_RV)

# Build x86_64 user programs + kernel + ISO
_build-x86-iso:
	@cd user && $(MAKE) ARCH=x86_64 clean > /dev/null 2>&1 && $(MAKE) ARCH=x86_64 > /dev/null 2>&1
	@touch user/hello.elf user/heap_test.elf user/file_test.elf user/spawn_test.elf
	@rm -f $(KERNEL_X86)
	@rm -rf target/$(TARGET_X86)/release/.fingerprint/karte-os-kernel-*
	cargo +nightly build --release --target $(TARGET_X86) -p karte-os-kernel -Z build-std=core,alloc
	@mkdir -p $(ISO_DIR)/boot/grub
	@cp $(KERNEL_X86) $(ISO_DIR)/boot/karte-os-kernel
	@printf 'set timeout=0\nset default=0\nterminal_input console\nterminal_output console\nmenuentry "KarteOS" {\n    multiboot2 /boot/karte-os-kernel\n    boot\n}\n' > $(ISO_DIR)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO_FILE) $(ISO_DIR) 2>/dev/null
	@echo "[x86_64] ISO ready: $(ISO_FILE)"

# ═══════════════════════════════════════════════════════════════
#  RISC-V 64 (primary, stable Rust)
# ═══════════════════════════════════════════════════════════════

## Build RISC-V kernel only
build-riscv:
	@$(MAKE) _build-riscv-kernel

## Run on RISC-V QEMU (build + run)
run-riscv: _build-riscv-kernel disk.img
	$(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV)

## Debug on RISC-V (GDB stub)
debug-riscv: _build-riscv-kernel disk.img
	$(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV) -S -s

## Build + deploy + run shell (RISC-V) — the one-command experience
shell-riscv: deploy
	@$(MAKE) _build-riscv-kernel
	$(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV)

## Build ISO-like image for RISC-V (kernel + disk with programs)
iso-riscv: deploy
	@$(MAKE) _build-riscv-kernel
	@echo "[riscv] Ready. Run: $(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV)"

# ═══════════════════════════════════════════════════════════════
#  x86_64 (secondary, nightly Rust + GRUB ISO)
# ═══════════════════════════════════════════════════════════════

## Build x86_64 kernel + ISO only (no deploy)
build-x86:
	@$(MAKE) _build-x86-iso

## Run on x86_64 QEMU (build ISO + run)
run-x86: iso-x86
	$(QEMU_X86) $(QEMU_X86_FLAGS)

## Debug on x86_64 (GDB stub)
debug-x86: _build-x86-iso disk.img
	$(QEMU_X86) $(QEMU_X86_FLAGS) -S -s

## Build + deploy + run shell (x86_64) — the one-command experience
shell-x86: deploy-x86
	@$(MAKE) _build-x86-iso
	$(QEMU_X86) $(QEMU_X86_FLAGS)

## Build ISO with all user programs deployed (x86_64) — the one-command build
iso-x86: deploy-x86 _build-x86-iso
	@echo "[x86_64] Complete! ISO + disk with all programs ready."
	@echo "[x86_64] Run: make run-x86  (or make shell-x86 to rebuild+run)"

# ═══════════════════════════════════════════════════════════════
#  Default targets
# ═══════════════════════════════════════════════════════════════
build: build-riscv
run:   run-riscv
debug: debug-riscv
shell: shell-riscv
iso:   iso-riscv

# ═══════════════════════════════════════════════════════════════
#  Testing
# ═══════════════════════════════════════════════════════════════

## Run RISC-V integration tests (96 tests)
test:
	@cd user && $(MAKE) ARCH=riscv64 clean > /dev/null 2>&1 && $(MAKE) ARCH=riscv64 > /dev/null 2>&1
	@rm -f $(KERNEL_RV)
	@rm -rf target/$(TARGET_RV)/release/.fingerprint/karte-os-kernel-*
	cargo build --release -p karte-os-kernel --target $(TARGET_RV) --features test_mode
	@bash scripts/run-tests.sh
	@echo "Restoring normal kernel build..."
	@rm -f $(KERNEL_RV)
	@rm -rf target/$(TARGET_RV)/release/.fingerprint/karte-os-kernel-*
	@cargo build --release -p karte-os-kernel --target $(TARGET_RV) > /dev/null 2>&1

## Run x86_64 integration tests (103 tests)
test-x86:
	@bash scripts/run-tests-x86_64.sh

## Run ALL tests (RISC-V + x86_64)
test-all: test test-x86

## Build test kernel only
build-test:
	@cd user && $(MAKE) ARCH=riscv64 > /dev/null 2>&1
	cargo build --release -p karte-os-kernel --target $(TARGET_RV) --features test_mode

## Boot test (verify shell starts)
boot-test: _build-riscv-kernel disk.img
	@echo "Running boot test..."
	@timeout 10 $(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV) 2>&1 | grep -qa "KarteOS Shell" \
		&& echo "Boot test passed" || echo "Boot test failed"

## SMP test (4 cores)
smp-test: _build-riscv-kernel disk.img
	@timeout 15 $(QEMU_RV) -machine virt -cpu rv64 -bios default -nographic -m 128M -smp 4 \
		-drive id=blk0,file=disk.img,format=raw,if=none -device virtio-blk-device,drive=blk0 \
		-netdev user,id=net0 -device virtio-net-device,netdev=net0 \
		-kernel $(KERNEL_RV) 2>&1 | grep -qa "KarteOS Shell" \
		&& echo "SMP test passed" || echo "SMP test failed"

# ═══════════════════════════════════════════════════════════════
#  Host-shared folder (virtio-9p)
# ═══════════════════════════════════════════════════════════════

## Run RISC-V with host shared folder (HOST_DIR=/path/to/share)
share-riscv: _build-riscv-kernel disk.img
	@mkdir -p $(HOST_DIR)
	$(QEMU_RV) $(QEMU_RV_FLAGS) -kernel $(KERNEL_RV) \
	  -fsdev local,id=share1,path=$(HOST_DIR),security_model=none \
	  -device virtio-9p-device,fsdev=share1,mount_tag=hostshare

## Run x86_64 with host shared folder (HOST_DIR=/path/to/share)
share-x86: _build-x86-iso disk.img
	@mkdir -p $(HOST_DIR)
	$(QEMU_X86) $(QEMU_X86_FLAGS) \
	  -fsdev local,id=share1,path=$(HOST_DIR),security_model=none \
	  -device virtio-9p-pci,fsdev=share1,mount_tag=hostshare

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

## Format all code
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

# ═══════════════════════════════════════════════════════════════
#  USB / Installation (x86_64)
# ═══════════════════════════════════════════════════════════════

## Create bootable USB image (set USB_SIZE_MB=... for size)
usb-image: _build-x86-iso disk.img
	@echo "[usb] Creating bootable USB image..."
	sudo tools/mkusb.sh image
	@echo "[usb] Image: target/karte-os-usb.img"
	@echo "[usb] Write: dd if=target/karte-os-usb.img of=/dev/sdX bs=4M status=progress"

## Write directly to USB drive (USB_DEV=/dev/sdX)
usb-write: _build-x86-iso disk.img
	@test -n "$(USB_DEV)" || (echo "Usage: make usb-write USB_DEV=/dev/sdX" && exit 1)
	sudo tools/mkusb.sh $(USB_DEV)

# ═══════════════════════════════════════════════════════════════
#  Release — Build distributable image packages
# ═══════════════════════════════════════════════════════════════

## Build RISC-V distributable release (kernel + disk + run script)
release-riscv:
	@bash tools/mkrelease.sh riscv64 $(DISK_SIZE)

## Build x86_64 distributable release (ISO + disk + run script)
release-x86:
	@bash tools/mkrelease.sh x86_64 $(DISK_SIZE)

## Build both architecture releases
release-all:
	@bash tools/mkrelease.sh both $(DISK_SIZE)

## Build default release (RISC-V)
release: release-riscv

# ═══════════════════════════════════════════════════════════════
#  Help
# ═══════════════════════════════════════════════════════════════

## Show this help
help:
	@echo ""
	@echo "╔═══════════════════════════════════════════════════════════╗"
	@echo "║  KarteOS — Dual-Architecture OS (RISC-V 64 + x86_64)    ║"
	@echo "╚═══════════════════════════════════════════════════════════╝"
	@echo ""
	@echo "  One-Command Build & Run:"
	@echo "    make                Build & run on RISC-V (default)"
	@echo "    make shell          Build all + deploy programs + run (RISC-V)"
	@echo "    make shell-x86      Build all + deploy programs + run (x86_64)"
	@echo "    make iso-x86        Build x86_64 ISO + deploy all programs"
	@echo ""
	@echo "  Deploy Programs to Disk:"
	@echo "    make deploy         Create disk.img + deploy all RISC-V programs"
	@echo "    make deploy-x86     Create disk.img + deploy all x86_64 programs"
	@echo "    make disk           Create empty disk.img only"
	@echo ""
	@echo "  Host Shared Folder:"
	@echo "    make share-riscv    Run with host dir mounted (HOST_DIR=/path)"
	@echo "    make share-x86      Same, x86_64"
	@echo ""
	@echo "  Testing (199 tests total):"
	@echo "    make test           Run RISC-V integration tests (96)"
	@echo "    make test-x86       Run x86_64 integration tests (103)"
	@echo "    make test-all       Run both architectures"
	@echo "    make boot-test      Verify boot reaches shell"
	@echo "    make smp-test       Test 4-core SMP"
	@echo ""
	@echo "  Development:"
	@echo "    make build-riscv    Build RISC-V kernel only"
	@echo "    make build-x86      Build x86_64 kernel + ISO only"
	@echo "    make debug          Run with GDB stub (-S -s)"
	@echo "    make fmt            Format all code"
	@echo "    make clean          Remove all build artifacts"
	@echo ""
	@echo "  Disk Image Tools:"
	@echo "    tools/mkdisk.sh deploy          One-command: disk + all programs"
	@echo "    tools/mkdisk.sh put <file>      Copy file to disk"
	@echo "    tools/mkdisk.sh get <file>      Copy file from disk"
	@echo "    tools/mkdisk.sh list            List files on disk"
	@echo ""
	@echo "  Setup (Ubuntu/Debian):"
	@echo "    make setup-riscv    Install RISC-V toolchain"
	@echo "    make setup-x86      Install x86_64 toolchain"
	@echo ""
	@echo ""
	@echo "  Release (distributable packages):"
	@echo "    make release       Build RISC-V release tarball (kernel+disk+run.sh)"
	@echo "    make release-riscv Same as above, explicit"
	@echo "    make release-x86   Build x86_64 release tarball (ISO+disk+run.sh)"
	@echo "    make release-all   Build both architectures"
	@echo ""
	@echo "  QEMU exit: Ctrl+A then X"
	@echo ""
