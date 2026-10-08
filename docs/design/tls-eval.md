# TLS 评估 — KarteOS 用户态 MQTT 通道加密（rustls no_std 谱系）

> ROADMAP §P3.3 尾项验收：TLS 评估文档。结论：**rustls + no_std 谱系为
> 首选路径**，分三阶段落地；明文 MQTT（当前已通链路）作为 v0 保持不变。

## 1. 候选对比

| 方案 | no_std | 内存足迹 | 维护 | 结论 |
|------|--------|---------|------|------|
| **rustls**（ring/aws-lc 系） | `rustls` crate 本体 no_std；ring 在 RV64 需要 atomic 指令（QEMU rv64gc ✅；rv32imc 不可用） | 堆 ~50-80KB/连接 | 活跃、CVE 响应快 | ✅ 主内核（riscv64gc/x86_64/aarch64）首选 |
| **rustls + postcard-no_std-crypto**（纯 Rust AES/GCM+RSA，无 ring） | ✅ 全 no_std | ~20-40KB | 中 | ✅ S 档 MCU（rv32imc/ESP32-C3 无 atomic）备选 |
| mbedtls-sys | 需要 libc/alloc shim | 大 | C 绑定重 | ❌ 与无 libc 目标冲突 |
| 自研 TLS | — | — | 协议复杂度极高 | ❌ 永不 |

## 2. 三阶段落地

1. **TLS 1.3 客户端（主内核）**：rustls（no_std + alloc）+ 用户态 MQTT 客
   户端升级 `mqtts://`；证书验证用 webpki-roots 内嵌根（IoT 局域场景再换
   私有 CA）。验收：QEMU 内 mqtts:1883 明文/8883 TLS 双端口对照，抓包
   确认密文。
2. **S 档 MCU 通道加密（ESP32-C3）**：纯 Rust crypto 后端（AES-128-GCM +
   ECDSA P-256），TLS 1.3 仅 client 模式；内存预算 <48KB（scratch32 的
   512KB 内存的 ~10%）。
3. **证书生命周期**：CapDesc 的 `capabilities` 增加 `tls.identity` 字段；
   设备身份 = 设备 ID + 厂商签名证书（制造时烧录），与 capauth 的 HMAC
   token 链并存（TLS 管传输，capauth 管授权）。

## 3. 与现有栈的接口

- 内核已有 TCP socket（sys 70-77）；rustls 运行在**用户态**（openssl-free），
  仅依赖 `read/write` 两个 fd 语义 —— 不新增内核 syscall。
- QEMU 抓包验收：`tcpdump -w` 端到端密文对照（v1）。
- 风险：ring 在 QEMU TCG 下握手耗时（ECDSA 签验 ~200ms 级）——MQTT keepalive
  放宽到 60s 以上。
