//! P3.3 OTA A/B — 分区头解析 + 选槽（bootloader 交接协议内核侧）
//!
//! ROADMAP §P3.3。协议见 docs/design/ota-ab.md：每 slot 32B KotaHeader
//! 前缀，magic/CRC 校验失败出局；双有效取 version 大者（相等取非 trial）。
//! v1 验收 = 单测全规则 + QEMU 演示（B 槽 CRC 损坏 → 回退 A）。

pub const KOTA_MAGIC: u32 = 0x4154_4F4B; // 'KOTA' little-endian read
pub const KOTA_HEADER_LEN: usize = 32;

/// 试运行标记（flags bit0）：未 confirm 的 trial 槽重启时让位于对槽。
pub const KOTA_FLAG_TRIAL: u32 = 1;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum KotaErr {
    BadMagic,
    BadCrc,
}

/// CRC32（IEEE 多项式，无表实现——镜像校验频次低，简洁优先）。
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// 从 32B 头解析字段并校验 magic + 镜像 CRC。
/// `image`：该 slot 的完整镜像（含头本身），头内 image_crc 覆盖整镜像。
pub fn parse_slot(image: &[u8]) -> Result<(u32, u32, u32), KotaErr> {
    if image.len() < KOTA_HEADER_LEN {
        return Err(KotaErr::BadMagic);
    }
    let rd32 = |off: usize| {
        u32::from_le_bytes([image[off], image[off + 1], image[off + 2], image[off + 3]])
    };
    if rd32(0) != KOTA_MAGIC {
        return Err(KotaErr::BadMagic);
    }
    let version = rd32(4);
    let image_len = rd32(8);
    let crc = rd32(12);
    let flags = rd32(28);
    let len = (image_len as usize).min(image.len());
    // CRC computed with the crc field itself zeroed (matches make_slot,
    // which computes over the header with crc=0 before writing it).
    let mut probe = alloc::vec::Vec::with_capacity(len);
    probe.extend_from_slice(&image[..len]);
    probe[12..16].copy_from_slice(&[0, 0, 0, 0]);
    if crc32(&probe) != crc {
        return Err(KotaErr::BadCrc);
    }
    Ok((version, crc, flags))
}

/// 选槽规则：A/B 镜像二选一。返回 0=A / 1=B。
/// 规则：magic/CRC 失败出局；双有效取 version 大；相等取非 trial；
/// 全失败默认 A（无镜像 → 上层走 RamFS 兜底）。
pub fn select_slot(a: &[u8], b: &[u8]) -> usize {
    match (parse_slot(a), parse_slot(b)) {
        (Ok((va, _, fa)), Ok((vb, _, fb))) => {
            if va != vb {
                usize::from(vb > va)
            } else {
                usize::from(fa & KOTA_FLAG_TRIAL != 0 && fb & KOTA_FLAG_TRIAL == 0)
            }
        }
        (Ok(_), Err(_)) => 0,
        (Err(_), Ok(_)) => 1,
        (Err(_), Err(_)) => 0,
    }
}

/// 构造一个带头的 slot 镜像（演示/测试用）。
pub fn make_slot(version: u32, payload: &[u8], trial: bool) -> alloc::vec::Vec<u8> {
    let mut img = alloc::vec![0u8; KOTA_HEADER_LEN + payload.len()];
    img[0..4].copy_from_slice(&KOTA_MAGIC.to_le_bytes());
    img[4..8].copy_from_slice(&version.to_le_bytes());
    let total = img.len() as u32;
    img[8..12].copy_from_slice(&total.to_le_bytes());
    img[28..32].copy_from_slice(&(if trial { KOTA_FLAG_TRIAL } else { 0 }).to_le_bytes());
    img[KOTA_HEADER_LEN..].copy_from_slice(payload);
    let crc = crc32(&img);
    img[12..16].copy_from_slice(&crc.to_le_bytes());
    img
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── OTA A/B Tests ──");

    crate::test::run_test("ota_header_roundtrip", || {
        let img = make_slot(3, b"kernel-v3", false);
        // crc 字段存的是"清零自身后"的 CRC——用同口径重新计算对比
        let mut probe = img.clone();
        probe[12..16].copy_from_slice(&[0, 0, 0, 0]);
        match parse_slot(&img) {
            Ok((version, crc, flags)) => version == 3 && flags == 0 && crc == crc32(&probe),
            Err(_) => false,
        }
    });

    crate::test::run_test("ota_crc_rejects_corruption", || {
        let mut img = make_slot(3, b"kernel-v3", false);
        // 破坏镜像体一个字节（模拟升级写入中断电）
        img[KOTA_HEADER_LEN] ^= 0xFF;
        parse_slot(&img) == Err(KotaErr::BadCrc)
            // magic 错误同样拒绝
            && {
                let mut bad = make_slot(1, b"x", false);
                bad[0] = b'X';
                parse_slot(&bad) == Err(KotaErr::BadMagic)
            }
    });

    crate::test::run_test("ota_select_rules", || {
        // B 槽 CRC 损坏（断电回滚场景）→ 选 A
        let a = make_slot(2, b"k-a", false);
        let mut b = make_slot(3, b"k-b", true);
        b[KOTA_HEADER_LEN] ^= 0xFF;
        select_slot(&a, &b) == 0
            // 双有效取版本大者
            && select_slot(&a, &make_slot(3, b"k-b", false)) == 1
            // 版本相等取非 trial
            && select_slot(&make_slot(3, b"k-a", true), &make_slot(3, b"k-b", false)) == 1
            // 双坏默认 A（RamFS 兜底路径）
            && select_slot(&[0u8; 40], &[0u8; 40]) == 0
    });
}
