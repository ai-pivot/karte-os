# MCP Device Fabric — KarteOS 设备织物协议 v1

> 定稿于 P2.5（ROADMAP Phase 2 DoD）。本文件即原 ROADMAP 附录 A/B 的
> 抽离落点，与实现一一对应：`kernel/src/capability.rs`（L1）、
> `kernel/src/drt.rs`（L2）、`user/toolserver.rs` + `kernel/src/mcp_cb.rs`（L3）、
> `kernel/src/capauth.rs`(L4)、`kernel/src/brain.rs`（脑端/自治）。

## 0. 分层总览

| 层 | 名称 | 职责 | 内核/用户 | 模块 |
|----|------|------|-----------|------|
| L1 | CapDesc 能力描述 | 驱动声明工具（name/schema/perm）→ 自动生成 MCP tool 定义 | 内核 | capability.rs |
| L2 | DRT 发现注册 | announce/心跳/超时/离线状态机 + UDP 广播 + 脑端工具表聚合 | 内核 | drt.rs |
| L3 | 调用传输 | JSON-RPC 2.0（tool-server）+ MCP-CB 紧凑二进制 + 幂等/超时 | 用户+内核 | toolserver.rs / mcp_cb.rs |
| L4 | 安全 | 能力令牌四道关校验 + signed manifest 注册 | 内核 | capauth.rs |
| 脑端 | brain agent | 规则引擎 v0 → 调用 → 回填；断脑自治 LocalQueue | 内核(v0)/用户(v1) | brain.rs |

## 1. 附录 A：CapDesc ABI（v1）

```rust
CapDesc { device_id: &str, device_type: &str, version: u32, tools: &[ToolDesc] }
ToolDesc { name, desc, inputs: &[FieldDesc], outputs: &[FieldDesc], perm: u8 }
FieldDesc { name, ty: Ty, desc, required: bool }
Ty    = Bool | I32 | I64 | F64 | Str | Bytes | Json   // JSON kind: boolean/integer/number/string/string(base64)/object
perm  = EXEC(1) | READ(2) | WRITE(4) | CONFIG(8)      // 位集，P2.4 令牌校验消费
```

注册语义：
- `register_device(desc)`：幂等拒绝重复 `device_id`（返回 Err）
- `set_online(id, bool)`：上线/离线状态位（L2 DRT 消费）
- `lookup_desc(id)`：wire 分发用查找
- 内置示范设备（boot 注册）：`vfs0`（read/write/ls）、`timer0`（sleep_until/sleep_ms）、`gpio0`（write/read）

### A.1 CapDesc → MCP tool JSON 生成

`tool_json(device_type, tool)` 输出：

```json
{"name":"<device_type>_<tool.name>","description":"<tool.desc>",
 "inputSchema":{"type":"object","properties":{...},"required":[...]},
 "outputSchema":{"type":"object","properties":{...}},
 "perm":<bits>}
```

快照测试锁定（`capdesc_tool_json_snapshot_*`）。

### A.2 MCP-CB 紧凑二进制（v1）

```
调用帧： "MCB1" | seq(u8) | name_len(u8) | name | args_len(u16 LE) | args_json
结果帧： "MCB1" | seq(u8) | status(u8: 0=OK 1=TOOL_ERR 2=TIMEOUT) | payload_len(u16 LE) | payload
```

网关转译器 `json_to_cb` / `cb_parse` / `result_to_cb`；双路径等价性由
`mcpcb_dualpath_equivalence` 单测锁定（同一调用 JSON 与 CB 解析出的
name/args 逐字节一致）。

### A.3 能力令牌与签名清单（v1）

令牌四道关（`TokenManager::verify`，任一不满足即拒）：
1. 存在（且 (device, tool) 匹配颁发时的绑定）
2. 未过期（`now < expires_ms`）
3. 未吊销（revoke 幂等，防时钟回拨复活；sweep 保留吊销 id）
4. perm 匹配（`token.perm_bits & need == need`）

signed manifest（设备注册）：
```
manifest_hash = FNV-1a64(device_id || 0x00 || registration_key)
```
信任锚为内核 `registration_key()` 密钥表（v0 内置 vfs0/timer0/gpio0）；
`verify_manifest` 不匹配即伪冒注册被拒。真 ed25519 归 Phase3 安全升级。

## 2. 附录 B：DRT 协议（v0/v1）

### B.1 状态机

```
Announce(新设备) → Online
Online  --无心跳 ≥1500ms--> Stale --再 ≥3000ms--> Offline
心跳（任何状态）→ Online；Announce（任何状态）→ Online
Bye（任何状态）→ Offline
```

参数：`HEARTBEAT_TIMEOUT_MS=1500`、`STALE_TIMEOUT_MS=3000`（合计保证
工具表变更 ≤3s 反映——P2.2 验收）。

### B.2 v0 UDP wire（端口 43110）

```
消息： "KRT1|<verb>|<device_id>|<seq>"
verb： A=announce  H=heartbeat  B=bye
```

`handle_wire(msg, now)` 分发到状态机；新设备/状态变化递增
`TABLE_SEQ`（脑端 `tool_table()` 以 seq 感知变更）。非法格式/未知
设备 announce 丢弃。mDNS/DNS-SD（`_karte._tcp`，v1，avahi 互通）在
smoltcp 上实现，归双机演示一并真测。

### B.3 脑端工具表

`tool_table() -> { seq, tools: ["<device_type>_<tool>", ...], devices: [...] }`
（仅含非 Offline 设备；稳定排序）。

## 3. 调用语义（L3）

- **幂等**：`CallTable` 以 seq 为键；InFlight/Done 的重复调用拒绝，
  Done 可 `replay` 缓存结果
- **超时**：`sweep(now)` 将过期 InFlight 判 TimedOut；超时后同 seq
  允许重试（重开）
- **传输**：v0 stdio（tool-server 每行一个 JSON-RPC 请求）；Streamable
  HTTP（smoltcp TCP）与双机 UDP 演示归双机演示项

## 4. 脑端与断脑自治

- **规则引擎 v0**（`brain.rs::Brain`）：预置脚本步骤推进，成功回填并
  前进、失败标记不前进；LLM v1 挂 M1 推理栈
- **断脑自治**（`brain.rs::LocalQueue`）：脑心跳超时 → 肢体继续执行
  缓存任务；脑恢复 → Done 任务标记 Reported，`report()` 回填结果清单
  （reconcile）

## 5. 已知约束（诚实记录）

- QEMU 6.2 TCG 多任务并发时 stdio 观察层字节交错（内核 trace 证明
  syscall 全部到达）——双机演示用 spawn+管道交换规避 tty 交错
- v0 manifest 签名原语为 FNV-1a（语义完整，非密码学安全）
- gpio 为用户态影子表（v0），内核虚拟 GPIO 端口随双机演示接入
