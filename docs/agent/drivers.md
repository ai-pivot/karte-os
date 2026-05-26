# Drivers

## UART (ns16550a)

- **File**: `kernel/src/driver/uart.rs`
- **Base address**: `0x1000_0000` (QEMU virt UART0)
- **Type**: MMIO, ns16550a-compatible
- **Baud**: QEMU ignores baudrate settings, defaults work

### Register Map

| Offset | Name | Purpose |
|--------|------|---------|
| 0 | THR (write) / RBR (read) | TX/RX data |
| 1 | IER | Interrupt enable |
| 2 | FCR (write) / ISR (read) | FIFO control / status |
| 3 | LCR | Line control (8N1 = 0x03) |
| 4 | MCR | Modem control |
| 5 | LSR | Line status (bit 5 = TX empty, bit 0 = data ready) |

### API
- `Uart::new(base)` — constructor
- `init()` — configure 8N1, enable FIFO, enable RX interrupt
- `putc(c)` / `getc() -> Option<u8>` — character I/O
- `puts(s)` — string output (adds \r before \n)
- Implements `core::fmt::Write` for use with `write!`/`writeln!`

## VirtIO Block Device

- **File**: `kernel/src/driver/virtio.rs`
- **Base address**: `0x1000_1000` (first VirtIO MMIO slot)
- **Stride**: 0x200 between devices
- **Max devices**: 8

### MMIO Register Layout

| Offset | Name | Purpose |
|--------|------|---------|
| 0x000 | MagicValue | Must be 0x74726976 ("virt") |
| 0x004 | Version | Should be 2 |
| 0x008 | DeviceID | 2 = block, 1 = net |
| 0x00c | VendorID | Vendor identifier |
| 0x100+ | Config | Device-specific config space |

### VirtIOHal
- `alloc_pages()` — uses `pmm::alloc_frame()` for DMA buffers
- `phys_to_virt()` / `virt_to_phys()` — identity mapping (returns same address)
- Uses `virtio-drivers` crate with `alloc` feature

### Probe Sequence
1. Read MagicValue at each slot
2. If magic != 0x74726976, skip slot
3. If DeviceID == 2 (block), initialize VirtIOBlk
4. Log capacity and block size

## VirtIO Network

- **File**: `kernel/src/driver/net.rs`
- **Device type**: VirtIO Net (DeviceID = 1)
- **VirtQueues**: Queue 0 = receive, Queue 1 = transmit

### Implementation
- Direct MMIO register manipulation
- VirtQueue with descriptor table, available ring, used ring
- `QUEUE_SIZE = 8` entries per queue
- MAC address read from config space at offset 0x100

### API
- `VirtIONet::probe()` — scan for VirtIO Net device
- `init()` — reset, negotiate features, setup VirtQueues
- `mac_addr()` — read MAC from device config
- `send_packet(data)` / `recv_packet(buf)` — packet I/O

## Filesystem

- **File**: `kernel/src/driver/fs.rs`
- **Type**: In-memory (no persistence)
- **Uses**: `alloc::vec::Vec`, `alloc::string::String` (requires kernel heap)

### API
- `FileSystem::new()` — create empty FS
- `create(name)` — create empty file
- `write(name, data)` — write data to file
- `read(name) -> Option<&[u8]>` — read file content
- `delete(name)` — remove file
- `list() -> &[File]` — list all files

## Key MMIO Addresses Summary

| Device | Address | Size |
|--------|---------|------|
| UART0 | 0x10000000 | 0x1000 |
| VirtIO MMIO[0] | 0x10001000 | 0x200 |
| VirtIO MMIO[1] | 0x10001200 | 0x200 |
| ... | +0x200 each | ... |
| VirtIO MMIO[7] | 0x10001E00 | 0x200 |
| PLIC | 0x0C000000 | 0x400000 |
