#!/usr/bin/env python3
"""KarteOS CapDesc 织物 —— 脑端桥（多设备统一发现 + 统一调用）

异构 IoT 设备（各跑 KarteOS 不同档位：rv32imc MCU / rv64 主内核 / aarch64 骨架）
都通过 unix socket 暴露串口，本桥是「脑」侧的总线：

  1. 汇聚所有设备的 KRT1 wire 流 → 设备注册表（id → 能力表，来自设备 announce）
  2. 对每台设备调用一个能力（统一调用平面，跨 ISA 完全一致）→ 收集 KRT1|T 应答
  3. 打印「脑-肢体」猜想验证证据：N 台异构设备统一注册 + 统一调用全应答

wire 协议（与 kernel/src/drt.rs 同构）：
  KRT1|A|<id>|<seq>|<tool1,tool2,...>   announce（设备 → 脑）
  KRT1|I|<id>|<seq>|<tool>              invoke（脑 → 设备）
  KRT1|T|<id>|<seq>|<result>            tool result（设备 → 脑）
  KRT1|H|<id>|<seq>                     heartbeat

用法: fabric_brain.py <sockdir> [expect_devices] [id:tool,...] [deadline_s]
"""
import glob
import os
import selectors
import socket
import sys
import time


def parse_krt1(line: str):
    parts = line.split("|")
    if len(parts) < 4 or parts[0] != "KRT1":
        return None
    verb, dev, seq = parts[1], parts[2], parts[3]
    rest = "|".join(parts[4:]) if len(parts) > 4 else ""
    return verb, dev, seq, rest


def send_flow_controlled(sock, data: bytes, chunk: int = 8, gap: float = 0.06):
    """16550 RX FIFO 仅 16 字节且无流控 —— 分块限速写入避免溢出丢帧。

    真实串口织物的标准做法：sender 限速 + receiver 帧聚合（设备侧已实现）。
    """
    for i in range(0, len(data), chunk):
        sock.sendall(data[i : i + chunk])
        if i + chunk < len(data):
            time.sleep(gap)


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: fabric_brain.py <sockdir> [expect] [id:tool,...] [deadline_s]")
        return 2
    sockdir = sys.argv[1]
    expect = int(sys.argv[2]) if len(sys.argv) > 2 else 2
    forced = {}
    if len(sys.argv) > 3 and sys.argv[3]:
        for kv in sys.argv[3].split(","):
            if ":" in kv:
                k, v = kv.split(":", 1)
                forced[k] = v
    deadline = float(sys.argv[4]) if len(sys.argv) > 4 else 30.0

    reg = {}          # id -> {"tools": [...], "sock": s, "announces": n}
    resp = {}         # id -> result
    last_tool = {}    # id -> tool
    buf = {}          # sock -> bytes（行缓冲）
    sock_path = {}    # sock -> path
    invoked = set()
    orphan = set()    # 已断开的 path
    sel = selectors.DefaultSelector()
    t_start = time.time()

    print(f"[brain] CapDesc fabric bridge up — expect={expect} devices, dir={sockdir}")

    while True:
        elapsed = time.time() - t_start

        # 1) 接入新出现的设备 socket
        if elapsed < deadline:
            for path in sorted(glob.glob(os.path.join(sockdir, "*.sock"))):
                if path in orphan:
                    continue
                if any(v["path"] == path for v in reg.values()):
                    continue
                if any(sock_path.get(k) == path for k in sock_path):
                    continue
                try:
                    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    s.connect(path)
                    s.setblocking(False)
                    sel.register(s, selectors.EVENT_READ)
                    buf[s] = b""
                    sock_path[s] = path
                    print(f"[brain] attached device channel {os.path.basename(path)}")
                except OSError:
                    continue

        # 2) 读事件（行缓冲解析 KRT1）
        for key, _ in sel.select(0.2):
            s = key.fileobj
            try:
                data = s.recv(4096)
            except OSError:
                data = b""
            if not data:
                orphan.add(sock_path.get(s, ""))
                try:
                    sel.unregister(s)
                except Exception:
                    pass
                continue
            buf[s] = buf.get(s, b"") + data
            while b"\n" in buf[s]:
                line, buf[s] = buf[s].split(b"\n", 1)
                text = line.decode("utf-8", "replace").strip()
                parsed = parse_krt1(text)
                if not parsed:
                    continue
                verb, dev, seq, rest = parsed
                if verb == "A":
                    tools = [t for t in rest.split(",") if t]
                    if dev not in reg:
                        reg[dev] = {
                            "tools": tools,
                            "sock": s,
                            "path": sock_path.get(s, ""),
                            "announces": 0,
                        }
                        print(f"[brain] device registered: {dev} tools={','.join(tools)}")
                    reg[dev]["announces"] += 1
                elif verb == "T":
                    resp[dev] = rest
                    print(f"[brain] result <- {dev} tool={last_tool.get(dev)} result={rest}")

        # 3) 注册齐 → 统一调用（每个设备一个能力，跨 ISA 同一调用路径）
        if len(reg) >= expect and not invoked:
            print(f"[brain] registered {len(reg)}/{expect} devices — unified invoke plane")
            for dev, info in reg.items():
                tool = forced.get(dev) or (info["tools"][0] if info["tools"] else "echo")
                last_tool[dev] = tool
                frame = f"KRT1|I|{dev}|0|{tool}\n".encode()
                try:
                    send_flow_controlled(info["sock"], frame)
                    print(f"[brain] invoke -> {dev} tool={tool}")
                except OSError as e:
                    print(f"[brain] invoke failed {dev}: {e}")
            invoked.add(1)

        # 4) 收敛判定
        if invoked and len(reg) >= expect and len(resp) >= len(reg):
            print(f"[brain] all {len(resp)}/{len(reg)} tool calls answered")
            print(
                f"[brain] FABRIC OK: {len(reg)} devices registered, "
                f"{len(resp)} unified invocations answered"
            )
            for dev in sorted(reg):
                print(
                    f"[brain]   {dev}: {last_tool.get(dev, '?')} -> "
                    f"{resp.get(dev, '<no-answer>')}"
                )
            return 0

        if elapsed > deadline:
            print(
                f"[brain] FABRIC TIMEOUT after {deadline:.0f}s — "
                f"registered={len(reg)}/{expect} answered={len(resp)}"
            )
            for dev in sorted(reg):
                print(
                    f"[brain]   {dev}: tools={','.join(reg[dev]['tools'])} "
                    f"-> {resp.get(dev, '<no-answer>')}"
                )
            return 1


if __name__ == "__main__":
    sys.exit(main())
