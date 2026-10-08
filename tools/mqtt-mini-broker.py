#!/usr/bin/env python3
"""P3.3 MQTT 3.1.1 mini broker (host side) — zero-dependency subset:
CONNECT/CONNACK, PUBLISH/PUBACK (QoS1), SUBSCRIBE/SUBACK, PINGREQ/PINGRESP.
Used to validate KarteOS's MQTT client end-to-end via QEMU user-net
(10.0.2.2:1883), and doubles as an independent third-party verifier.
Usage: python3 tools/mqtt-mini-broker.py [port]  (default 1883)"""
import socket
import struct
import sys
import threading


def remaining_len(n: int) -> bytes:
    out = b""
    while True:
        b = n % 128
        n //= 128
        if n > 0:
            b |= 0x80
        out += bytes([b])
        if n == 0:
            return out


def read_exact(sock, n):
    data = b""
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise ConnectionError("peer closed")
        data += chunk
    return data


def read_varint(sock):
    mult, val = 1, 0
    while True:
        b = read_exact(sock, 1)[0]
        val += (b & 0x7F) * mult
        if not (b & 0x80):
            return val
        mult *= 128


class ClientCtx:
    def __init__(self, sock, addr):
        self.sock = sock
        self.addr = addr
        self.subs = {}  # topic_filter -> max_qos
        self.lock = threading.Lock()


TOPICS = {}  # topic -> set(ClientCtx)
GLOB = threading.Lock()


def send_packet(ctx, fixed_first: int, body: bytes):
    ctx.sock.sendall(bytes([fixed_first]) + remaining_len(len(body)) + body)


def handle_connect(ctx, body: bytes) -> None:
    # protocol name + level(4) + flags + keepalive, then client id
    plen = struct.unpack(">H", body[:2])[0]
    proto = body[2:2 + plen]
    level = body[2 + plen]
    if proto != b"MQTT" or level != 4:
        send_packet(ctx, 0x20, b"\x00\x01")  # unacceptable protocol
        raise ConnectionError("bad protocol")
    send_packet(ctx, 0x20, b"\x00\x00")  # CONNACK accepted
    print(f"[broker] CONNECT ok from {ctx.addr}")


def topic_match(filter_: str, topic: str) -> bool:
    if filter_ == topic:
        return True
    f = filter_.split("/")
    t = topic.split("/")
    i = 0
    while i < len(f):
        if f[i] == "#":
            return True
        if i >= len(t):
            return False
        if f[i] == "+":
            i += 1
            continue
        if f[i] != t[i]:
            return False
        i += 1
    return i == len(t)


def broadcast(topic: str, payload: bytes, qos: int) -> int:
    body = struct.pack(">H", len(topic)) + topic.encode()
    if qos == 1:
        body += b"\x00\x01"  # packet id 1 (broker-side reuse ok for demo)
    body += payload
    delivered = 0
    with GLOB:
        targets = [c for cset in TOPICS.values() for c in cset if any(topic_match(f, topic) for f in c.subs)]
    for c in targets:
        try:
            send_packet(c, 0x30 | (qos << 1), body)
            delivered += 1
        except OSError:
            pass
    return delivered


def handle_subscribe(ctx, body: bytes) -> None:
    pid = body[:2]
    i = 2
    acks = bytearray(pid)
    while i < len(body):
        tlen = struct.unpack(">H", body[i:i + 2])[0]
        topic = body[i + 2:i + 2 + tlen].decode()
        qos = body[i + 2 + tlen]
        i += 2 + tlen + 1
        ctx.subs[topic] = qos
        acks.append(qos)
        with GLOB:
            TOPICS.setdefault(topic, set()).add(ctx)
        print(f"[broker] SUBSCRIBE {topic} qos={qos} from {ctx.addr}")
    send_packet(ctx, 0x90, bytes(acks))


def handle_publish(ctx, first: int, body: bytes) -> None:
    qos = (first >> 1) & 0x3
    tlen = struct.unpack(">H", body[:2])[0]
    topic = body[2:2 + tlen].decode()
    off = 2 + tlen
    pid = None
    if qos > 0:
        pid = body[off:off + 2]
        off += 2
    payload = body[off:]
    n = broadcast(topic, payload, min(qos, 1))
    if pid is not None:
        send_packet(ctx, 0x40, pid)  # PUBACK
    print(f"[broker] PUBLISH topic={topic} {len(payload)}B qos={qos} -> {n} clients")


def serve_client(sock, addr):
    ctx = ClientCtx(sock, addr)
    try:
        while True:
            first = read_exact(sock, 1)[0]
            ln = read_varint(sock)
            body = read_exact(sock, ln)
            t = first >> 4
            if t == 1:
                handle_connect(ctx, body)
            elif t == 3:
                handle_publish(ctx, first, body)
            elif t == 8:
                handle_subscribe(ctx, body)
            elif t == 12:
                send_packet(ctx, 0xD0, b"")  # PINGRESP
            elif t == 14:
                break  # DISCONNECT
    except (ConnectionError, OSError):
        pass
    finally:
        with GLOB:
            for cset in TOPICS.values():
                cset.discard(ctx)
        try:
            sock.close()
        except OSError:
            pass
        print(f"[broker] closed {addr}")


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 1883
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("0.0.0.0", port))
    srv.listen(8)
    print(f"[broker] MQTT mini-broker on :{port}")
    while True:
        sock, addr = srv.accept()
        threading.Thread(target=serve_client, args=(sock, addr), daemon=True).start()


if __name__ == "__main__":
    main()
