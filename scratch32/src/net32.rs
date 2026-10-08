//! 虚拟 ESP32（riscv32 virt）网络层 — virtio-mmio legacy 驱动 + 最小 UDP
//!
//! 移植自主内核 kernel/src/driver/net.rs 的 legacy 修复经验：
//!   * QEMU virtio-mmio version=1 → QueueAlign(0x03C)+QueuePFN(0x040)，
//!     vring 布局 desc@0 / avail@128 / used@4096（QUEUE_ALIGN=4096）
//!   * rx 队列预填充 WRITE desc，recv 用单调 used cursor + desc 归还
//!   * 设备每帧通知（VIRTIO_F_EVENT_IDX 不协商）
//! 最小网络栈：ARP + IPv4 + UDP 构造/解析（静态 IP 10.0.2.15/24）。

use core::ptr::{read_volatile, write_volatile};

// ---- virtio-mmio legacy 寄存器（version=1） ----
const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
const REG_STATUS: usize = 0x070;
const REG_QUEUE_SEL: usize = 0x030;
const REG_QUEUE_NUM_MAX: usize = 0x034;
const REG_QUEUE_NUM: usize = 0x038;
const REG_QUEUE_ALIGN: usize = 0x03C;
const REG_QUEUE_PFN: usize = 0x040;
const REG_QUEUE_NOTIFY: usize = 0x050;
const REG_INT_STATUS: usize = 0x060;
const REG_INT_ACK: usize = 0x064;
const REG_DEV_FEATURES: usize = 0x010;
const REG_DRV_FEATURES: usize = 0x020;

const S_ACK: u32 = 1;
const S_DRIVER: u32 = 2;
const S_DRIVER_OK: u32 = 4;
const S_FEATURES_OK: u32 = 8;

const QUEUE_SIZE: u32 = 8;
const NET_MAX_PACKET: usize = 1524;
const VRING_DESC_F_WRITE: u16 = 2;

// QEMU riscv32 virt: virtio slots at 0x10001000.., 8 slots; net 常在 slot 6
const VIRTIO_BASE: usize = 0x1000_1000;
const VIRTIO_SLOTS: usize = 8;

// ---- vring 布局（与主内核一致） ----
#[repr(C)]
#[derive(Clone, Copy)]
struct VringDesc {
    addr: u32,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(4096))]
struct QueueMem {
    desc: [VringDesc; QUEUE_SIZE as usize],
    avail_buf: [u8; 4096 - (QUEUE_SIZE as usize) * 16],
    used_buf: [u8; 4096],
    data: [u8; (QUEUE_SIZE as usize) * NET_MAX_PACKET],
}

struct TxRing {
    mem: &'static mut QueueMem,
    avail_idx: u16,
    last_used: u16,
    free: [bool; 8],
    next_free: usize,
}

struct RxRing {
    mem: &'static mut QueueMem,
    avail_idx: u16,
    last_used: u16,
}

// vring 内存直接落 .bss（RAM），绝不经过栈——64KB×2 的栈临时值会溢出
// 16KB 的 MCU 栈（这正是 init 挂死的根因）。
static mut TX_MEM: QueueMem = QueueMem {
    desc: [VringDesc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; QUEUE_SIZE as usize],
    avail_buf: [0; 4096 - (QUEUE_SIZE as usize) * 16],
    used_buf: [0; 4096],
    data: [0; (QUEUE_SIZE as usize) * NET_MAX_PACKET],
};
static mut RX_MEM: QueueMem = QueueMem {
    desc: [VringDesc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; QUEUE_SIZE as usize],
    avail_buf: [0; 4096 - (QUEUE_SIZE as usize) * 16],
    used_buf: [0; 4096],
    data: [0; (QUEUE_SIZE as usize) * NET_MAX_PACKET],
};

struct TxRing2 {
    avail_idx: u16,
    last_used: u16,
    free: [bool; 8],
}
static mut TX: Option<TxRing2> = None;
static mut RX: Option<(u16, u16)> = None; // (avail_idx, last_used)
static mut DEV_BASE: usize = 0;
// RV32IMC 无 A 扩展 → 无 64 位原子。MAC 仅在 init 写一次、其后只读
// （单核 M-mode，顺序保证），用 UnsafeCell + 手动 Sync 即可。
use core::cell::UnsafeCell;
pub struct MacCell(UnsafeCell<[u8; 6]>);
unsafe impl Sync for MacCell {}
static NET_MAC: MacCell = MacCell(UnsafeCell::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]));

fn mac_bytes() -> [u8; 6] {
    unsafe { *NET_MAC.0.get() }
}

fn mac_store(b: &[u8; 6]) {
    unsafe { *NET_MAC.0.get() = *b; }
}

pub const SELF_IP: [u8; 4] = [10, 0, 2, 15];
pub const GW_IP: [u8; 4] = [10, 0, 2, 2];

fn rd(base: usize, off: usize) -> u32 {
    unsafe { read_volatile((base + off) as *const u32) }
}
fn wr(base: usize, off: usize, v: u32) {
    unsafe { write_volatile((base + off) as *mut u32, v) }
}

fn setup_queue(base: usize, queue_index: u32, is_rx: bool) {
    wr(base, REG_QUEUE_SEL, queue_index);
    let num_max = rd(base, REG_QUEUE_NUM_MAX);
    if num_max == 0 {
        uart_puts("[net32] queue absent\n");
        return;
    }
    let n = if QUEUE_SIZE <= num_max { QUEUE_SIZE } else { num_max };
    wr(base, REG_QUEUE_NUM, n);
    unsafe {
        let (mem_addr, rx) = if is_rx {
            (core::ptr::addr_of!(RX_MEM) as u32, true)
        } else {
            (core::ptr::addr_of!(TX_MEM) as u32, false)
        };
        if rx {
            let rx = core::ptr::addr_of_mut!(RX).as_mut().unwrap();
            *rx = Some((0, 0));
            // rx 预填充：全部 desc 挂 avail（WRITE），设备可直接投递
            let av = core::ptr::addr_of_mut!(RX_MEM.avail_buf) as *mut u16;
            for i in 0..n as usize {
                RX_MEM.desc[i] = VringDesc {
                    addr: (core::ptr::addr_of!(RX_MEM.data) as u32) + (i * NET_MAX_PACKET) as u32,
                    len: NET_MAX_PACKET as u32,
                    flags: VRING_DESC_F_WRITE,
                    next: 0,
                };
                write_volatile(av.add(2 + i), i as u16);
            }
            if let Some((avail_idx, _)) = rx.as_mut() {
                *avail_idx = n as u16;
            }
            write_volatile(av.add(1), n as u16); // avail idx
            // kick：legacy 设备必须显式 notify 才会开始消费 avail
            wr(base, REG_QUEUE_NOTIFY, 0);
        } else {
            let tx = core::ptr::addr_of_mut!(TX).as_mut().unwrap();
            *tx = Some(TxRing2 {
                avail_idx: 0,
                last_used: 0,
                free: [true; 8],
            });
        }
        // legacy：页对齐 vring，PFN = 物理地址 >> 12
        wr(base, REG_QUEUE_ALIGN, 4096);
        wr(base, REG_QUEUE_PFN, mem_addr >> 12);
        // 与主内核序列一致：QueueReady（0x044，legacy 设备忽略但保持 diff-free）
        wr(base, 0x044, 1);
    }
}

/// 探测 8 个 slot 找 DeviceID=1（net），完成 legacy 初始化。
pub fn init() -> bool {
    for slot in 0..VIRTIO_SLOTS {
        let base = VIRTIO_BASE + slot * 0x1000;
        if rd(base, REG_MAGIC) != 0x74726976 {
            continue; // 'virt'
        }
        if rd(base, REG_DEVICE_ID) != 1 {
            continue; // 1 = net
        }
        unsafe { DEV_BASE = base };
        wr(base, REG_STATUS, S_ACK);
        wr(base, REG_STATUS, S_ACK | S_DRIVER);
        // 不协商任何 feature（0）：最小驱动，帧不带 offload
        let _ = rd(base, REG_DEV_FEATURES);
        wr(base, REG_DRV_FEATURES, 0);
        // legacy 必需：GuestPageSize（0x028）= 4096（vring 页单位）
        wr(base, 0x028, 4096);
        wr(base, REG_STATUS, S_ACK | S_DRIVER | S_FEATURES_OK);
        if rd(base, REG_STATUS) & S_FEATURES_OK == 0 {
            uart_puts("[net32] FEATURES_OK failed\n");
            return false;
        }
        unsafe {
            TX = Some(TxRing2 {
                avail_idx: 0,
                last_used: 0,
                free: [true; 8],
            });
            RX = Some((0, 0));
        }
        setup_queue(base, 0, true); // rx
        setup_queue(base, 1, false); // tx
        // 读设备 MAC（legacy config 基址 0x100）
        unsafe {
            let mut m = [0u8; 6];
            for (i, byte) in m.iter_mut().enumerate() {
                *byte = read_volatile((base + 0x100 + i) as *const u8);
            }
            mac_store(&m);
        }
        wr(base, REG_STATUS, S_ACK | S_DRIVER | S_FEATURES_OK | S_DRIVER_OK);
        uart_puts("[net32] virtio-net legacy up (slot ");
        uart_dec(slot as u32);
        uart_puts(")\n");
        return true;
    }
    uart_puts("[net32] no virtio-net found\n");
    false
}

/// 发送一个以太网帧（单 desc，TX_MEM 静态 vring）。
pub fn send_frame(frame: &[u8]) -> bool {
    unsafe {
        let base = DEV_BASE;
        if base == 0 || frame.len() > NET_MAX_PACKET - 10 {
            return false;
        }
        let tx = match core::ptr::addr_of_mut!(TX).as_mut().unwrap().as_mut() {
            Some(t) => t,
            None => return false,
        };
        // 回收已被设备消费的 desc（TX used cursor）
        let used_p = core::ptr::addr_of!(TX_MEM.used_buf) as *const u16;
        let tuidx = read_volatile(used_p.add(1));
        while (tx.last_used as u32).wrapping_sub(0).wrapping_add(1) != 0
            && tx.last_used != tuidx
        {
            let slot = (tx.last_used as usize) % QUEUE_SIZE as usize;
            let ent = used_p.add(2 + slot * 4);
            let id = read_volatile(ent) as usize;
            if id < QUEUE_SIZE as usize {
                tx.free[id] = true;
            }
            tx.last_used = tx.last_used.wrapping_add(1);
        }
        let mut did: Option<usize> = None;
        for i in 0..QUEUE_SIZE as usize {
            if tx.free[i] {
                did = Some(i);
                break;
            }
        }
        let did = match did {
            Some(d) => d,
            None => return false,
        };
        tx.free[did] = false;
        let base_p = core::ptr::addr_of_mut!(TX_MEM.data) as *mut u8;
        let buf = core::slice::from_raw_parts_mut(base_p.add(did * NET_MAX_PACKET), NET_MAX_PACKET);
        // 10B 设备头（virtio-net hdr，legacy 10B）
        buf[..10].fill(0);
        buf[10..10 + frame.len()].copy_from_slice(frame);
        TX_MEM.desc[did] = VringDesc {
            addr: (core::ptr::addr_of!(TX_MEM.data) as u32) + (did * NET_MAX_PACKET) as u32,
            len: (10 + frame.len()) as u32,
            flags: 0,
            next: 0,
        };
        let av = core::ptr::addr_of_mut!(TX_MEM.avail_buf) as *mut u16;
        write_volatile(
            av.add(2 + (tx.avail_idx % QUEUE_SIZE as u16) as usize),
            did as u16,
        );
        tx.avail_idx = tx.avail_idx.wrapping_add(1);
        write_volatile(av.add(1), tx.avail_idx);
        wr(base, REG_QUEUE_NOTIFY, 1); // tx queue = 1
        // 设备侧 used ring 原始 dump（flags/idx + 首个 entry）
        let up = core::ptr::addr_of!(TX_MEM.used_buf) as *const u16;
        uart_puts("[net32] tx used raw f=");
        uart_dec(read_volatile(up) as u32);
        uart_puts(" i=");
        uart_dec(read_volatile(up.add(1)) as u32);
        uart_puts(" e0=");
        uart_dec(read_volatile(up.add(2)) as u32);
        uart_puts("/");
        uart_dec(read_volatile(up.add(3)) as u32);
        uart_puts("\n");
        true
    }
}

/// 收一个以太网帧（used cursor 消费，返回长度）。
pub fn recv_frame(out: &mut [u8]) -> Option<usize> {
    unsafe {
        let base = DEV_BASE;
        if base == 0 {
            return None;
        }
        let ist = rd(base, REG_INT_STATUS);
        if ist != 0 {
            wr(base, REG_INT_ACK, ist);
        }
        let rx = core::ptr::addr_of_mut!(RX).as_mut().unwrap().as_mut()?;
        let (avail_idx, last_used) = *rx;
        let used = core::ptr::addr_of!(RX_MEM.used_buf) as *const u16;
        let uidx = read_volatile(used.add(1));
        if uidx == last_used {
            return None;
        }
        uart_puts("[net32] rx used idx ");
        uart_dec(uidx as u32);
        uart_puts(" last ");
        uart_dec(last_used as u32);
        uart_puts("\n");
        let slot = (last_used as usize) % QUEUE_SIZE as usize;
        let ent = used.add(2 + slot * 4);
        let id = read_volatile(ent) as usize;
        let len = read_volatile(ent.add(1)) as usize;
        *rx = (avail_idx, last_used.wrapping_add(1));
        if len < 10 || id >= QUEUE_SIZE as usize {
            return None;
        }
        let copy_len = (len - 10).min(out.len());
        let base_p = core::ptr::addr_of!(RX_MEM.data) as *const u8;
        let src = core::slice::from_raw_parts(
            base_p.add(id * NET_MAX_PACKET + 10),
            copy_len,
        );
        out[..copy_len].copy_from_slice(src);
        // desc 归还 avail
        let av = core::ptr::addr_of_mut!(RX_MEM.avail_buf) as *mut u16;
        write_volatile(
            av.add(2 + (avail_idx % QUEUE_SIZE as u16) as usize),
            id as u16,
        );
        let new_avail = avail_idx.wrapping_add(1);
        write_volatile(av.add(1), new_avail);
        *rx = (new_avail, last_used.wrapping_add(1));
        wr(base, REG_QUEUE_NOTIFY, 0);
        Some(copy_len)
    }
}

// ---------------- 最小 ARP/IPv4/UDP ----------------

fn eth_header(out: &mut [u8], dst_mac: &[u8; 6], etype: u16) {
    out[..6].copy_from_slice(dst_mac);
    out[6..12].copy_from_slice(&mac_bytes());
    out[12] = (etype >> 8) as u8;
    out[13] = (etype & 0xFF) as u8;
}

/// ARP request（解析网关 MAC）。reply 存入 ARP_CACHE。
pub fn arp_request() {
    let mut f = [0u8; 42];
    eth_header(&mut f, &[0xFF; 6], 0x0806);
    f[14] = 0; f[15] = 1; // ht=1
    f[16] = 8; f[17] = 0; // pt=IPv4
    f[18] = 6; f[19] = 4;
    f[20] = 0; f[21] = 1; // request
    f[22..28].copy_from_slice(&mac_bytes());
    f[28..32].copy_from_slice(&SELF_IP);
    f[38..42].copy_from_slice(&GW_IP);
    let ok = send_frame(&f);
    uart_puts("[net32] arp sent ok=");
    uart_dec(ok as u32);
    uart_puts("\n");
}

static mut ARP_CACHE: ([u8; 4], [u8; 6], bool) = (GW_IP, [0; 6], false);

pub fn gw_mac_known() -> bool {
    unsafe { ARP_CACHE.2 }
}

/// 构造并发送 UDP（网关 MAC 已知时）。
pub fn udp_send(dst_ip: [u8; 4], dst_port: u16, src_port: u16, payload: &[u8]) -> bool {
    let (gw_mac, ok) = unsafe { (ARP_CACHE.1, ARP_CACHE.2) };
    if !ok {
        return false;
    }
    let mut f = [0u8; 14 + 20 + 8 + 256];
    eth_header(&mut f, &gw_mac, 0x0800);
    let ip_total = (20 + 8 + payload.len()) as u16;
    // IPv4
    f[14] = 0x45;
    f[15] = 0;
    f[16..18].copy_from_slice(&ip_total.to_be_bytes());
    f[19..21].copy_from_slice(&42u16.to_be_bytes()); // id
    f[22..24].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
    f[24] = 64;
    f[25] = 17; // UDP
    // 校验和置 0（局域简化，主机栈普遍接受）
    f[26..30].copy_from_slice(&SELF_IP);
    f[30..34].copy_from_slice(&dst_ip);
    // UDP
    let uh = 34;
    f[uh..uh + 2].copy_from_slice(&src_port.to_be_bytes());
    f[uh + 2..uh + 4].copy_from_slice(&dst_port.to_be_bytes());
    f[uh + 4..uh + 6].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    f[uh + 6..uh + 8].copy_from_slice(&0u16.to_be_bytes());
    f[uh + 8..uh + 8 + payload.len()].copy_from_slice(payload);
    send_frame(&f[..14 + ip_total as usize])
}

/// 收帧 + 分发：ARP reply 存缓存；UDP 上抛给回调。
pub fn poll(mut on_udp: impl FnMut([u8; 4], u16, &[u8])) {
    static mut RX_SEEN: u32 = 0; // 单核主循环，无并发
    let mut f = [0u8; NET_MAX_PACKET];
    while let Some(n) = recv_frame(&mut f) {
        unsafe {
            RX_SEEN += 1;
            uart_puts("[net32] rx#");
            uart_dec(RX_SEEN);
        }
        uart_puts(" len=");
        uart_dec(n as u32);
        uart_puts("\n");
        if n < 34 {
            continue;
        }
        match ((f[12] as u16) << 8) | f[13] as u16 {
            0x0806 => {
                // ARP reply: sender ip @28, sender mac @22
                if n >= 42 && f[21] == 2 {
                    let mut ip = [0u8; 4];
                    ip.copy_from_slice(&f[28..32]);
                    let mut mac = [0u8; 6];
                    mac.copy_from_slice(&f[22..28]);
                    unsafe {
                        ARP_CACHE = (ip, mac, true);
                    }
                    uart_puts("[net32] ARP reply stored\n");
                }
            }
            0x0800 => {
                if f[23] == 17 && n >= 42 {
                    // UDP: dst ip @30, src port @34, dst port @36, payload @42
                    let mut src = [0u8; 4];
                    src.copy_from_slice(&f[26..30]);
                    let sport = ((f[34] as u16) << 8) | f[35] as u16;
                    let dport = ((f[36] as u16) << 8) | f[37] as u16;
                    let ulen = (((f[38] as u16) << 8) | f[39] as u16) as usize;
                    if dport == 43110 && ulen >= 8 {
                        let plen = (ulen - 8).min(n - 42);
                        on_udp(src, sport, &f[42..42 + plen]);
                    }
                }
            }
            _ => {}
        }
    }
}

// ---- UART 辅助（main.rs 的 putc 复用：fn 指针解耦——v0 直接重复实现） ----
fn uart_puts(s: &str) {
    for b in s.bytes() {
        unsafe {
            while read_volatile((0x1000_0000 + 5) as *const u8) & 0x20 == 0 {}
            write_volatile(0x1000_0000 as *mut u8, b);
        }
    }
}

fn uart_dec(mut v: u32) {
    let mut b = [0u8; 12];
    let mut i = b.len();
    if v == 0 {
        i -= 1;
        b[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    uart_puts(core::str::from_utf8(&b[i..]).unwrap_or("?"));
}
