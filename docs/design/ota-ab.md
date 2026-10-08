# OTA A/B — 分区约定 + bootloader 交接协议 + 回滚

> ROADMAP §P3.3 尾项验收。v1：QEMU 内升级+断电回滚演示（选槽逻辑真跑）。

## 1. 磁盘分区约定（ext4 首槽之后的保留区）

| 区块 | 偏移（字节） | 大小 | 内容 |
|------|-------------|------|------|
| FS 区 | 0x0 | 64MB（mkdisk 现状） | ext4（rootfs：/bin /apps /manifest） |
| Slot A | 0x4000000 | 4MB | 内核镜像 A + 32B KotaHeader 前缀 |
| Slot B | 0x8000000 | 4MB | 内核镜像 B + 32B KotaHeader 前缀 |
| OTA 状态 | 0xC000000 | 64B | 当前槽位/试运行标记（三副本循环写） |

## 2. KotaHeader（32 字节，每 slot 镜像前置）

```c
struct KotaHeader {        // 32B
    u32 magic;             // 'KOTA' 0x41544F4B
    u32 version;           // 单调递增（比较选新）
    u32 image_len;         // 镜像字节数（含本头）
    u32 image_crc;         // CRC32(image_len 覆盖范围)
    u64 build_ts_ms;       // 构建时间戳
    u32 flags;             // bit0=trial（试运行，回滚窗口未确认）
    u32 reserved;
}
```

## 3. Bootloader 交接协议（选槽规则）

1. 读 A/B 头；magic 或 CRC 失败者出局。
2. 两候选都有效 → 取 `version` 大者；相等取非 trial。
3. trial 槽启动后由内核在（首次成功进入用户态 + 心跳确认）写状态区
   `confirm` 清 trial；未确认且重启 → bootloader 强制另一槽（**断电回滚**）。
4. 状态区三副本循环写（写入中掉电由副本一致性兜底）。

## 4. v1 验收（QEMU 演示）

- `kernel/src/ota.rs`：KotaHeader 解析 + CRC32 + `select_slot()` 全规则单测。
- QEMU 演示：disk.img 预置 A(valid v2)/B(trial v3，镜像体损坏 CRC 失败) →
  内核启动选择 A 并打印 `[ota] slot=A v2 (B crc-bad, rolled back)`。
