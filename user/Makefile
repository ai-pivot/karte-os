CROSS = riscv64-linux-gnu-
AS = $(CROSS)as
LD = $(CROSS)ld
OBJCOPY = $(CROSS)objcopy

all: hello.elf

hello.o: hello.S
	$(AS) -march=rv64gc -o $@ $<

hello.elf: hello.o
	$(LD) -T user.ld -o $@ $<

clean:
	rm -f *.o *.elf
