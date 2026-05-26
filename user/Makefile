CROSS = riscv64-linux-gnu-
AS = $(CROSS)as
LD = $(CROSS)ld
OBJCOPY = $(CROSS)objcopy

TARGETS = hello.elf heap_test.elf file_test.elf

all: $(TARGETS)

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

clean:
	rm -f *.o *.elf
