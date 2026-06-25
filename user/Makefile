ARCH ?= x86_64

ifeq ($(ARCH),riscv64)
    CROSS = riscv64-linux-gnu-
    RUSTC_TARGET = riscv64gc-unknown-none-elf
    LD_SCRIPT = user.ld
else ifeq ($(ARCH),x86_64)
    CROSS =
    RUSTC_TARGET = x86_64-unknown-none
    LD_SCRIPT = user-x86_64.ld
endif

AS = $(CROSS)as
LD = $(CROSS)ld
OBJCOPY = $(CROSS)objcopy

# The kernel embeds shell.elf via include_bytes!, so it must be built here too
# (otherwise `make build` / CI fails after `make clean`).
RUSTC = rustc
RUSTFLAGS_USER = --edition 2024 --target $(RUSTC_TARGET) \
	-C panic=abort -C opt-level=2 -C relocation-model=static -C link-arg=-T$(LD_SCRIPT) --crate-type bin

# Assembly programs (RISC-V only)
ifeq ($(ARCH),riscv64)
ASM_TARGETS = hello.elf heap_test.elf file_test.elf spawn_test.elf
else
ASM_TARGETS =
endif

RUST_TARGETS = minimal.elf shell.elf ls.elf cat.elf echo.elf mkdir.elf rm.elf env.elf pwd.elf \
 grep.elf sed.elf wc.elf head.elf tail.elf tui-demo.elf dmesg.elf ext4_write_test.elf ext4_sparse_test.elf

TARGETS = $(ASM_TARGETS) $(RUST_TARGETS)

all: $(TARGETS)

# Rust programs — single pattern rule builds any .elf from corresponding .rs
%.elf: %.rs syscall.rs $(LD_SCRIPT)
	$(RUSTC) $(RUSTFLAGS_USER) -o $@ $<

# Assembly programs (RISC-V only)
ifeq ($(ARCH),riscv64)
hello.o: hello.S
	$(AS) -march=rv64gc -o $@ $<

hello.elf: hello.o
	$(LD) -T $(LD_SCRIPT) -o $@ $<

heap_test.o: heap_test.S
	$(AS) -march=rv64gc -o $@ $<

heap_test.elf: heap_test.o
	$(LD) -T $(LD_SCRIPT) -o $@ $<

file_test.o: file_test.S
	$(AS) -march=rv64gc -o $@ $<

file_test.elf: file_test.o
	$(LD) -T $(LD_SCRIPT) -o $@ $<

spawn_test.o: spawn_test.S
	$(AS) -march=rv64gc -o $@ $<

spawn_test.elf: spawn_test.o
	$(LD) -T $(LD_SCRIPT) -o $@ $<
endif

clean:
	rm -f *.o *.elf
