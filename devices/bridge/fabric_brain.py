#!/usr/bin/env python3
"""KarteOS CapDesc 织物 —— 脑端桥（确定性脚本版：统一发现 + 统一调用）

异构 IoT 设备（各跑 KarteOS 不同档位）经 unix socket 暴露串口，本桥是
确定性「脑」侧总线：汇聚 KRT1 announce → 设备注册表；对每台设备统一
invoke 一个能力 → 收集 tool result。

（真 LLM 作为脑端的版本见 llm_brain.py）

用法: fabric_brain.py <sockdir> [expect_devices] [id:tool,...] [deadline_s]
"""
import sys
import time

from fabric import Fabric


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

    fb = Fabric(sockdir, tag="brain")
    last_tool = {}
    print(f"[brain] CapDesc fabric bridge up — expect={expect} devices, dir={sockdir}")
    t0 = time.time()
    invoked = False

    while True:
        elapsed = time.time() - t0
        fb.attach()
        fb.pump(0.2)

        if len(fb.reg) >= expect and not invoked:
            print(f"[brain] registered {len(fb.reg)}/{expect} devices — unified invoke plane")
            for dev, info in fb.reg.items():
                tool = forced.get(dev) or (info["tools"][0] if info["tools"] else "echo")
                last_tool[dev] = tool
                fb.invoke(dev, tool)
            invoked = True

        if invoked and len(fb.reg) >= expect and len(fb.results) >= len(fb.reg):
            print(f"[brain] all {len(fb.results)}/{len(fb.reg)} tool calls answered")
            print(
                f"[brain] FABRIC OK: {len(fb.reg)} devices registered, "
                f"{len(fb.results)} unified invocations answered"
            )
            for dev in sorted(fb.reg):
                print(
                    f"[brain]   {dev}: {last_tool.get(dev, '?')} -> "
                    f"{fb.results.get(dev, '<no-answer>')}"
                )
            return 0

        if elapsed > deadline:
            print(
                f"[brain] FABRIC TIMEOUT after {deadline:.0f}s — "
                f"registered={len(fb.reg)}/{expect} answered={len(fb.results)}"
            )
            for dev in sorted(fb.reg):
                print(
                    f"[brain]   {dev}: tools={','.join(fb.reg[dev]['tools'])} "
                    f"-> {fb.results.get(dev, '<no-answer>')}"
                )
            return 1


if __name__ == "__main__":
    sys.exit(main())
