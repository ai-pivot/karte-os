#!/usr/bin/env python3
"""KarteOS 织物通道 —— 设备串口汇聚 + KRT1 收发（fabric_brain / llm_brain 共用）

把 N 台异构设备（各跑 KarteOS 不同档位）的 unix-socket 串口汇聚成一条
「织物总线」，向上提供：
  - 设备注册表（id → 能力表，来自 KRT1 announce）
  - 统一调用（KRT1|I invoke，流控分块发送）
  - 统一应答（KRT1|T tool result）
  - 非 KRT1 行缓冲（设备日志，诊断用）

wire 协议与 kernel/src/drt.rs 同构。
"""
import glob
import os
import selectors
import socket
import time

VERBS = {"A": "announce", "T": "tool-result", "H": "heartbeat", "B": "bye"}


def parse_krt1(line: str):
    """'KRT1|<verb>|<id>|<seq>|[rest]' → (verb, id, seq, rest) 或 None"""
    parts = line.split("|")
    if len(parts) < 4 or parts[0] != "KRT1":
        return None
    rest = "|".join(parts[4:]) if len(parts) > 4 else ""
    return parts[1], parts[2], parts[3], rest


class Fabric:
    """多设备织物通道（脑端侧）。"""

    def __init__(self, sockdir: str, verbose: bool = True, tag: str = "fabric"):
        self.sockdir = sockdir
        self.verbose = verbose
        self.tag = tag
        self.sel = selectors.DefaultSelector()
        self.buf = {}  # sock -> bytes（行缓冲）
        self.sock_of = {}  # dev_id -> sock
        self.sock_path = {}  # sock -> path
        self.reg = {}  # dev_id -> {"tools": [...], "announces": n}
        self.results = {}  # dev_id -> result
        self.invoked_tool = {}  # dev_id -> 最近一次 invoke 的工具名（证据打印用）
        self.logs = []  # 非 KRT1 行（设备日志）
        self._attached_paths = set()
        self._dead_paths = set()

    def _say(self, msg: str):
        if self.verbose:
            print(f"[{self.tag}] {msg}", flush=True)

    # ── 通道管理 ────────────────────────────────────────────────
    def attach(self):
        """连接新出现的设备 socket（非阻塞，可反复调用）。"""
        for path in sorted(glob.glob(os.path.join(self.sockdir, "*.sock"))):
            if path in self._attached_paths or path in self._dead_paths:
                continue
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(path)
                s.setblocking(False)
                self.sel.register(s, selectors.EVENT_READ)
                self.buf[s] = b""
                self.sock_path[s] = path
                self._attached_paths.add(path)
                self._say(f"attached device channel {os.path.basename(path)}")
            except OSError:
                continue

    def pump(self, seconds: float):
        """处理事件最多 seconds 秒。"""
        end = time.time() + seconds
        while True:
            remain = end - time.time()
            if remain <= 0:
                return
            for key, _ in self.sel.select(min(0.1, remain)):
                self._read(key.fileobj)

    def _read(self, s):
        try:
            data = s.recv(4096)
        except OSError:
            data = b""
        if not data:
            self._dead_paths.add(self.sock_path.get(s, ""))
            try:
                self.sel.unregister(s)
            except Exception:
                pass
            return
        self.buf[s] = self.buf.get(s, b"") + data
        while b"\n" in self.buf[s]:
            line, self.buf[s] = self.buf[s].split(b"\n", 1)
            text = line.decode("utf-8", "replace").strip()
            parsed = parse_krt1(text)
            if not parsed:
                if text:
                    self.logs.append(text)
                    self.logs = self.logs[-200:]
                continue
            verb, dev, _seq, rest = parsed
            if verb == "A":
                tools = [t for t in rest.split(",") if t]
                if dev not in self.reg:
                    self.reg[dev] = {"tools": tools, "announces": 0}
                    self.sock_of[dev] = s
                    self._say(f"device registered: {dev} tools={','.join(tools)}")
                self.reg[dev]["announces"] += 1
            elif verb == "T":
                self.results[dev] = rest
                self._say(f"result <- {dev} tool={self.invoked_tool.get(dev)} result={rest}")
            elif verb == "B":
                self._say(f"device bye: {dev}")

    # ── 调用与查询 ──────────────────────────────────────────────
    def devices(self):
        return {d: v["tools"] for d, v in self.reg.items()}

    def send_flow_controlled(self, sock, data: bytes, chunk: int = 8, gap: float = 0.06):
        """16550 RX FIFO 仅 16B 且无流控 —— 分块限速写入避免溢出丢帧。"""
        for i in range(0, len(data), chunk):
            sock.sendall(data[i : i + chunk])
            if i + chunk < len(data):
                time.sleep(gap)

    def invoke(self, dev: str, tool: str, seq: int = 0) -> bool:
        s = self.sock_of.get(dev)
        if s is None:
            return False
        self.invoked_tool[dev] = tool
        self.send_flow_controlled(s, f"KRT1|I|{dev}|{seq}|{tool}\n".encode())
        self._say(f"invoke -> {dev} tool={tool}")
        return True

    def wait_result(self, dev: str, timeout: float = 12.0) -> str | None:
        """等指定设备的一个 tool result（消费者模式）。"""
        end = time.time() + timeout
        while time.time() < end:
            if dev in self.results:
                return self.results.pop(dev)
            self.attach()
            self.pump(0.15)
        return None
