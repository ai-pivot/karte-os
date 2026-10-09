#!/usr/bin/env python3
"""云端 LLM 作为脑端 —— function calling 驱动 CapDesc 织物上的 IoT 设备。

流程：
  1. 从织物收集设备能力（KRT1 announce，来自多台异构设备）
  2. 生成 OpenAI 兼容 tools schema（每设备每能力一个 function）
  3. 云端 LLM（端点在 xbot.db 的 user_llm_subscriptions）多轮 function calling：
     模型决策 → 本桥经 KRT1 invoke 路由到对应设备 → 收 tool result → 回填 → 下一轮
  4. 模型输出最终回答；打印完整决策链（模型决策 + 设备执行）

用法: llm_brain.py <sockdir> [expect_devices] [task] [subscription_name]
"""
import json
import os
import sqlite3
import sys
import time
import urllib.request

from fabric import Fabric

DEFAULT_TASK = (
    "现场巡逻任务：1) 读取环境温度与湿度；"
    "2) 如果温度高于 20 度且湿度高于 40，打开继电器；"
    "3) 拍一张现场快照；"
    "4) 用一句中文汇报你做了什么以及依据。"
)

SYSTEM_PROMPT = (
    "你是 KarteOS 织物的脑端（运行在云端）。现场有若干异构 IoT 设备，"
    "它们通过 function 暴露能力（传感器/执行器/摄像头/网关）。"
    "请严格依据工具返回值做判断，必要时多步调用；最后用中文给出简短汇报。"
)


def load_subscription(name="dpsk"):
    """从 xbot 配置库取云端端点（base_url, api_key, model）。"""
    db = os.path.expanduser("~/.xbot/xbot.db")
    c = sqlite3.connect("file:%s?mode=ro" % db, uri=True)
    row = c.execute(
        "select base_url, api_key, model from user_llm_subscriptions "
        "where name=? and enabled=1",
        (name,),
    ).fetchone()
    if not row or not row[0] or not row[1]:
        raise SystemExit(f"subscription {name!r} unusable (missing base_url/api_key)")
    return row


def norm(name: str) -> str:
    """OpenAI function 名只允许 [a-zA-Z0-9_-]。"""
    return name.replace(".", "_").replace("-", "_")


def build_tools(devices):
    """设备能力表 → OpenAI function calling tools schema（+ 反查索引）。"""
    tools, index = [], {}
    for dev, caps in sorted(devices.items()):
        for cap in caps:
            fn = norm(cap)
            if fn in index:  # 同名能力（跨设备）加设备前缀去歧义
                fn = f"{norm(dev)}__{fn}"
            index[fn] = (dev, cap)
            tools.append(
                {
                    "type": "function",
                    "function": {
                        "name": fn,
                        "description": f"[{dev}] KarteOS CapDesc tool: {cap}",
                        "parameters": {"type": "object", "properties": {}},
                    },
                }
            )
    return tools, index


def llm_chat(base, key, model, messages, tools):
    body = json.dumps(
        {
            "model": model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "temperature": 0,
        }
    ).encode()
    req = urllib.request.Request(
        base.rstrip("/") + "/chat/completions",
        data=body,
        headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=180) as r:
        return json.load(r)


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: llm_brain.py <sockdir> [expect] [task] [subscription]")
        return 2
    sockdir = sys.argv[1]
    expect = int(sys.argv[2]) if len(sys.argv) > 2 else 4
    task = sys.argv[3] if len(sys.argv) > 3 else DEFAULT_TASK
    sub = sys.argv[4] if len(sys.argv) > 4 else "dpsk"
    base, key, model = load_subscription(sub)

    fb = Fabric(sockdir, tag="llm-fabric")
    print(f"[llm] cloud model: {model} @ {base}")
    print(f"[llm] task: {task}")

    # 1) 等设备注册齐（announce 汇聚 → 设备注册表）
    t0 = time.time()
    while len(fb.reg) < expect and time.time() - t0 < 60:
        fb.attach()
        fb.pump(0.2)
    devices = fb.devices()
    if len(devices) < expect:
        print(f"[llm] FAIL: only {len(devices)}/{expect} devices registered")
        return 1
    n_tools = sum(len(v) for v in devices.values())
    print(f"[llm] fabric ready: {len(devices)} devices, {n_tools} tools injected into model context")

    tools, index = build_tools(devices)
    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": task},
    ]

    calls = []
    for turn in range(8):
        resp = llm_chat(base, key, model, messages, tools)
        msg = resp["choices"][0]["message"]
        tool_calls = msg.get("tool_calls") or []
        if not tool_calls:
            final = (msg.get("content") or "").strip()
            print(f"[llm] final answer: {final}")
            print(
                f"[llm] LLM FABRIC OK: {len(devices)} devices, "
                f"{len(calls)} tool calls executed by the model"
            )
            for dev, cap, res in calls:
                print(f"[llm]   {dev}.{cap} -> {res}")
            return 0
        messages.append(msg)  # assistant 的 tool_calls 回合
        for tc in tool_calls:
            fn = tc["function"]["name"]
            dev, cap = index.get(fn, (None, None))
            print(f"[llm] turn {turn}: model decided -> {fn} (device={dev} tool={cap})")
            if dev is None:
                result = "err:unknown-tool"
            else:
                fb.invoke(dev, cap)
                result = fb.wait_result(dev, timeout=15)
                if result is None:
                    result = "err:timeout"
            calls.append((dev, cap, result))
            messages.append(
                {"role": "tool", "tool_call_id": tc.get("id", fn), "content": str(result)}
            )
    print("[llm] FAIL: exceeded turn budget")
    return 1


if __name__ == "__main__":
    sys.exit(main())
