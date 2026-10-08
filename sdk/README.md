# karte-sdk — KarteOS 开发者工具包

> ROADMAP §P4 karte-sdk。验收：外部开发者 30 分钟内发布一个新工具。

## 快速开始（30 分钟发布一个工具）

### 1. CapDesc 工具（Rust 用户态，~10 分钟）

```bash
cp -r sdk/templates/tool mytool && cd mytool
# 编辑 src/main.rs：填写 CapDesc（device_id/tools）+ 工具处理循环
# 见模板内 TODO 标注的 3 处
```

构建与部署（RISC-V 交叉编译 + mkdisk 放盘 + QEMU 运行）：

```bash
cd user && make ARCH=riscv64 mytool.elf
../tools/mkdisk.sh put user/mytool.elf
../tools/mkdisk.sh deploy   # 或 tools/qemu-dev.sh 一键
```

### 2. WASM 应用（内核微型解释器，~10 分钟）

```bash
cp -r sdk/templates/wasm-app myapp && cd myapp
# 编辑 module.wat：i32.const/i32.add 表达式（解释器 v0 指令集）
wat2wasm module.wat -o module.wasm   # 或用仓库 tools/wat2wasm 路径
# 编辑 manifest.json：capabilities 最小化申请 + 签名
```

部署后内核按 `docs/design/wasm-apps.md` 的 manifest 校验器加载。

### 3. 一键 QEMU（~2 分钟）

```bash
tools/qemu-dev.sh          # 构建内核 + 部署所有程序 + 启动 QEMU（串口直连）
tools/qemu-dev.sh --arch x86_64
```

### 4. 工具自述与发现（~8 分钟）

工具启动后向 DRT 广播 CapDesc（协议：`docs/design/mcp-fabric-protocol.md`）；
脑端 DRT ≤3s 聚合进工具表并注入模型上下文——新工具即刻可被脑调用。

## 目录结构

- `templates/tool/` — CapDesc 工具脚手架（sys 70-77 之外的全部样板已写好）
- `templates/wasm-app/` — WASM 应用脚手架（module.wat + manifest.json）
- `../tools/qemu-dev.sh` — 一键构建+部署+运行

## 能力与安全

- 所有工具调用经 `capauth` 令牌四道关（存在/有效期/用途/签名）——
  见 `docs/design/wasm-apps.md` §1 capability 定稿。
- manifest 能力申请遵循最小化原则；签名验证走 capauth HMAC 路径（v1）。
