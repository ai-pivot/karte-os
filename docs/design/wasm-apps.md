# WASM 应用模型与 Capability 定稿（P3.2 设计文档）

> ROADMAP Phase 3 · P3.2。实现现状：`kernel/src/wasm.rs` 微型解释器 v0
> （parse + i32.const/add/drop/end，3 单测，RV 167/167——**跨芯片**：同
> 一 wasm 字节码在 riscv64/x86_64 内核解释执行结果一致，解释器与 ISA 解耦）。

## 1. Capability 体系定稿（令牌/命名空间/继承/审计）

四要素与 P2.4 `capauth.rs` 的落地对应：

| 要素 | 语义 | 落地（现状） |
|------|------|-------------|
| 令牌 | 每工具一颗 token，脑持有、设备校验、可吊销 | `capauth::IssueToken/ValidateToken/Revoke`（四道关：存在/有效期/用途/签名）✅ |
| 命名空间 | token 绑定 `device_id + tool_name` 二元组，跨设备不可复用 | token 隐含 namespace（同一 token 对其他 device/tool 校验失败）✅；v1：显式 namespace 字段 + 通配（`home/#`） |
| 继承 | 脑签发的 token 可授权肢体再签发受限子令牌（scope 缩小） | v1：`parent_token_id` + scope 收缩校验（子 scope ⊆ 父 scope） |
| 审计 | 每次校验落 ring 日志（谁/何时/对什么工具） | v1：复用 kernel_log 环（`[capauth]` 前缀）+ `dmesg` 可查 |

## 2. WASM 应用 ↔ CapDesc 工具互暴露

- **WASM 应用注册工具**：应用启动时向 `capability::register(CapDesc)` 注
  册 `device_id = "wasm:<app_name>"`，`tools = &[...]`——脑端 DRT 正常发
  现与调用（无需内核改动）。
- **调用链**：脑 → device `invoke` → wasm 运行时 → `call_export()`（v0
  为纯函数工具；v1 支持 WASI 风格 host 函数，host 函数直接桥接内核
  syscall 子集，含 capauth 校验）。

## 3. 应用清单格式（manifest）

```jsonc
{
  "app": "sensor-aggregator",
  "version": "1.2.0",
  "entry": "module.wasm",
  "capabilities": [           // 能力申请（最小化原则）
    { "tool": "gpio.read",  "device": "local" },
    { "tool": "mqtt.publish", "topic": "karteo/telemetry/#" }
  ],
  "signature": "<ed25519(hmac) over canonical-json above>",
  "limits": { "mem_pages": 16, "fuel": 100000 }
}
```

- 校验器（v1）：签名 → 能力申请 ⊆ 授权集 → 版本单调。v0 的签名验
  证直接复用 `capauth` 的 HMAC 路径。
- 签名/manifest 的文件格式与 ext4 部署一致（`/apps/<app>/manifest.json`）。

## 4. 跨芯片演示（验收对应）

`kernel/src/wasm.rs` 的 3 个单测在 `make test`（riscv64）与
`make test-x86`（x86_64）下对**同一份 wasm 字节码常量**解释执行并断
言一致结果（42 / 栈语义）——这就是"同一 wasm 应用跨芯片无改动运行"
的内核级证据；用户态 wasm 应用的双架构部署走 ext4 同一文件。

## 5. v1 路线（wasmi 谱系替换）

v0 子集解释器用于锁定模型与 ABI；v1 引入完整指令集（wasmi no_std 移植
或自研扩展），接口不变（`parse/call_export` → `call(name, args)`），
WASM↔CapDesc 互暴露与 manifest 校验器同步落地。
