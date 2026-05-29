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
- **Stride**: 0x1000 (page-sized) between devices
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

## Block Device Abstraction

- **File**: `kernel/src/driver/block.rs`
- **Trait**: `BlockDevice` — abstract interface between filesystems and block devices
- **API**:
  - `read_block(block_id, buf)` / `write_block(block_id, buf)` — fixed-size block I/O (default 512 bytes)
  - `block_size()` — returns block size (default 512)
  - `capacity_blocks()` — total blocks on device
- **VirtIOBlock adapter**: wraps `virtio::read_block/write_block` as `BlockDevice` impl
- **Global registry**: `set_block_device(dev)` / `get_block_device()` — stores `&'static dyn BlockDevice` in a SpinLock

## Virtual File System (VFS)

- **File**: `kernel/src/driver/vfs.rs` (482 lines)
- **Core trait**: `FileSystem` — all concrete filesystems (FAT32, RamFS, ext4) implement it
- **Mount table**: up to 8 mounts, prefix-based path resolution (longest prefix match)
- **Open file table**: per-system `OpenFileTable`, max 64 open files, fd reuse
- **Note**: This VFS layer exists but is NOT yet wired into syscalls. Syscalls currently go through the legacy `fs.rs` path.

### FileSystem trait methods
| Method | Purpose |
|--------|---------|
| `name() -> &str` | FS name identifier |
| `root_inode() -> u64` | Root directory inode number |
| `lookup(dir, name) -> u64` | Look up entry by name in directory |
| `metadata(inode) -> VfsMetadata` | Get file/dir metadata |
| `readdir(dir, idx) -> Option<VfsDirEntry>` | Enumerate directory entries |
| `read_file(inode, offset, buf) -> usize` | Read from file at offset |
| `write_file(inode, offset, data) -> usize` | Write to file at offset |
| `create_file(dir, name) -> u64` | Create new file |
| `create_dir(dir, name) -> u64` | Create subdirectory |
| `unlink(dir, name)` | Remove entry from directory |
| `set_file_size(inode, size)` | Truncate/extend file |

### VFS error types: `VfsError` enum (NotFound, AlreadyExists, NotADirectory, NotAFile, PermissionDenied, IoError, InvalidParam, OutOfMemory, DirectoryNotEmpty)

## RamFS (VFS-compatible)

- **File**: `kernel/src/driver/ramfs.rs` (315 lines)
- **Type**: In-memory filesystem implementing `vfs::FileSystem` trait
- **Structure**: `BTreeMap<u64, RamFsNode>` indexed by inode number, root inode = 1
- **Node types**: File or directory, with `children: Vec<u64>` for directories
- **Data storage**: `RamFileData` — either `Static(&'static [u8])` (for embedded binaries) or `Owned(Vec<u8>)`
- **Initialization**: `new_initialized()` pre-populates with embedded user programs

## FAT32 (starry-fatfs based)

- **File**: `kernel/src/driver/fat32.rs` (272 lines)
- **Library**: `starry-fatfs 0.4.1-preview.2` (alloc + lfn features)
- **Storage adapter**: `Fat32Storage` — implements `fatfs::IoBase + Read + Write + Seek` over VirtIO block device
- **Sector I/O**: Byte-granularity read/write with internal sector-aligned buffering (read-modify-write for unaligned writes)
- **Mount strategy**: Try mounting existing FS first; if fails, format as FAT32 then mount
- **Global state**: `SpinLock<Option<FileSystem<Fat32Storage>>>`
- **Limitation**: Only operates on root directory (flat file namespace)
- **NOTE**: FAT32 does NOT implement `vfs::FileSystem` trait — it has its own standalone API

## Legacy Filesystem (fs.rs)

- **File**: `kernel/src/driver/fs.rs` (515 lines)
- **Type**: Simple flat in-memory filesystem with `Vec<File>` storage
- **Role**: Orchestration layer — initializes RamFS + FAT32, provides unified `read_file_owned/write_file_owned` API
- **Init flow**: Populates embedded binaries → tries FAT32 init → injects binaries into FAT32
- **Priority**: FAT32 first for reads/writes (persistent), RamFS as fallback (embedded)
- **FD table**: `FdTable` with 32 slots, per-process (stored in `Process::fd_table`)
- **Used by**: All syscalls (`sys_open`, `sys_read`, `sys_write`, `sys_close`, `sys_spawn`, `sys_exec`)
- **Tests**: 15 filesystem tests in `#[cfg(feature = "test_mode")]`

## Key MMIO Addresses Summary

| Device | Address | Size |
|--------|---------|------|
| UART0 | 0x10000000 | 0x1000 |
| VirtIO MMIO[0] | 0x10001000 | 0x1000 |
| VirtIO MMIO[1] | 0x10002000 | 0x1000 |
| ... | +0x1000 each | ... |
| VirtIO MMIO[7] | 0x10008000 | 0x1000 |
| PLIC | 0x0C000000 | 0x400000 |
