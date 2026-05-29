CROSS = riscv64-linux-gnu-
AS = $(CROSS)as
LD = $(CROSS)ld
OBJCOPY = $(CROSS)objcopy

# The kernel embeds shell.elf via include_bytes!, so it must be built here too
# (otherwise `make build` / CI fails after `make clean`).
RUSTC = rustc
RUSTFLAGS_USER = --edition 2024 --target riscv64gc-unknown-none-elf \
	-C panic=abort -C opt-level=2 -C link-arg=-Tuser.ld --crate-type bin

TARGETS = hello.elf heap_test.elf file_test.elf spawn_test.elf shell.elf

all: $(TARGETS)

shell.elf: shell.rs user.ld
	$(RUSTC) $(RUSTFLAGS_USER) -o $@ shell.rs

hello.o: hello.S
	$(AS) -march=rv64gc -o $@ $<

hello.elf: hello.o
	$(LD) -T user.ld -o $@ $<

heap_test.o: heap_test.S
	$(AS) -march=rv64gc -o $@ $<

heap_test.elf: heap_test.o
	$(LD) -T user.ld -o $@ $<

file_test.o: file_test.S
	$(AS) -march=rv64gc -o $@ $<

file_test.elf: file_test.o
	$(LD) -T user.ld -o $@ $<

spawn_test.o: spawn_test.S
	$(AS) -march=rv64gc -o $@ $<

spawn_test.elf: spawn_test.o
	$(LD) -T user.ld -o $@ $<

clean:
	rm -f *.o *.elf
