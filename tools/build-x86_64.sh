#!/bin/bash
# Build x86_64 ISO for KarteOS
# Must run AFTER `make test` (which overwrites user programs with RISC-V versions)

set -e

echo "=== Building x86_64 KarteOS ISO ==="

# Step 1: Create minimal x86-64 ELF stubs for cfg-gated assembly programs
python3 << 'PYEOF'
import struct
def make_stub(name):
    h = bytearray(64)
    h[0:4] = b'\x7fELF'
    h[4] = 2  # 64-bit
    h[5] = 1  # little endian
    h[6] = 1  # ELF version
    struct.pack_into('<H', h, 16, 2)   # ET_EXEC
    struct.pack_into('<H', h, 18, 62)  # EM_X86_64
    struct.pack_into('<I', h, 20, 1)   # EV_CURRENT
    struct.pack_into('<Q', h, 24, 0x1000)  # entry
    struct.pack_into('<Q', h, 32, 64)  # phoff
    struct.pack_into('<H', h, 52, 64)  # ehsize
    struct.pack_into('<H', h, 54, 56)  # phentsize
    struct.pack_into('<H', h, 56, 1)   # phnum
    p = bytearray(56)
    struct.pack_into('<I', p, 0, 1)    # PT_LOAD
    struct.pack_into('<Q', p, 8, 120)  # p_offset
    struct.pack_into('<Q', p, 16, 0x1000)  # p_vaddr
    struct.pack_into('<Q', p, 32, 1)   # p_filesz
    struct.pack_into('<Q', p, 40, 1)   # p_memsz
    struct.pack_into('<Q', p, 48, 0x1000)  # p_flags (RX)
    with open(name, 'wb') as f:
        f.write(bytes(h) + bytes(p) + b'\xc3')  # ret instruction
for n in ['user/hello.elf', 'user/heap_test.elf', 'user/file_test.elf', 'user/spawn_test.elf']:
    make_stub(n)
    print(f'  Created stub: {n}')
PYEOF

# Step 2: Build x86_64 user programs (clean first to force rebuild)
echo "=== Building x86_64 user programs ==="
cd user && make ARCH=x86_64 clean && cd ..

# Step 2b: Recreate stubs (clean removed them)
python3 << 'PYEOF'
import struct
def make_stub(name):
    h = bytearray(64)
    h[0:4] = b'\x7fELF'; h[4] = 2; h[5] = 1; h[6] = 1
    struct.pack_into('<H', h, 16, 2); struct.pack_into('<H', h, 18, 62)
    struct.pack_into('<I', h, 20, 1); struct.pack_into('<Q', h, 24, 0x1000)
    struct.pack_into('<Q', h, 32, 64); struct.pack_into('<H', h, 52, 64)
    struct.pack_into('<H', h, 54, 56); struct.pack_into('<H', h, 56, 1)
    p = bytearray(56)
    struct.pack_into('<I', p, 0, 1); struct.pack_into('<Q', p, 8, 120)
    struct.pack_into('<Q', p, 16, 0x1000); struct.pack_into('<Q', p, 32, 1)
    struct.pack_into('<Q', p, 40, 1); struct.pack_into('<Q', p, 48, 0x1000)
    with open(name, 'wb') as f:
        f.write(bytes(h) + bytes(p) + b'\xc3')
for n in ['user/hello.elf', 'user/heap_test.elf', 'user/file_test.elf', 'user/spawn_test.elf']:
    make_stub(n)
PYEOF

cd user && make ARCH=x86_64 && cd ..

# Step 3: Verify all ELFs
echo "=== Verifying ELF machine types ==="
python3 << 'PYEOF'
import struct
all_ok = True
for n in ['user/hello.elf', 'user/heap_test.elf', 'user/file_test.elf',
          'user/spawn_test.elf', 'user/shell.elf', 'user/echo.elf',
          'user/ls.elf', 'user/cat.elf']:
    try:
        with open(n, 'rb') as f:
            d = f.read(20)
        m = struct.unpack_from('<H', d, 18)[0]
        status = 'OK' if m == 62 else 'WRONG!'
        if m != 62:
            all_ok = False
        print(f'  {n}: machine={m} {status}')
    except FileNotFoundError:
        print(f'  {n}: NOT FOUND (optional)')
assert all_ok, 'Some ELFs have wrong machine type!'
PYEOF

# Step 4: Force rebuild kernel (include_bytes! must pick up new files)
echo "=== Building x86_64 kernel ==="
rm -rf target/x86_64-unknown-none
# IMPORTANT: Also clean the release fingerprint cache to ensure
# cfg(target_arch) constants are correctly compiled
cargo +nightly build --release --target x86_64-unknown-none \
    -p karte-os-kernel -Z build-std=core,alloc 2>&1 | tail -5

# Step 5: Create ISO
echo "=== Creating ISO ==="
mkdir -p target/x86_64-iso/boot/grub
cp target/x86_64-unknown-none/release/karte-os-kernel target/x86_64-iso/boot/karte-os-kernel
cat > target/x86_64-iso/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0
menuentry "KarteOS" {
    multiboot2 /boot/karte-os-kernel
    boot
}
EOF
grub-mkrescue -o target/karte-os-x86_64.iso target/x86_64-iso 2>&1 | tail -1

echo "=== Done! ISO: target/karte-os-x86_64.iso ==="
