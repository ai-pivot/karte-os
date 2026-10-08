#!/usr/bin/env python3
"""tools/llm/train_char_gpt.py — numpy-only char-level GPT trainer for M1.

Trains a small transformer on tinyshakespeare (deps: numpy only), then
exports all weights as a single little-endian f32 binary in a fixed layout
consumed byte-for-byte by user/llm.rs.

Layout (after 16-byte header):
    header: b"KLWM" | u32 vocab | u32 d_model | u32 n_layer | u32 block
    wte [V,D]  wpe [T,D]
    per layer l:
      ln1_g [D] ln1_b [D]  wqkv [D,3D] bqkv [3D]
      wo [D,D] bo [D]      ln2_g [D] ln2_b [D]
      fc1_w [D,F] fc1_b [F]  fc2_w [F,D] fc2_b [D]
    lnf_g [D] lnf_b [D]     (output head ties wte)

Self-check: `--check N` perturbs N sampled parameters and compares analytic
gradients against finite differences before training starts.

Usage: python3 train_char_gpt.py <data.txt> <out.bin> [steps] [--check N]
"""
import sys
import numpy as np

RNG = np.random.default_rng(42)
NH = 4


def ln_f(x, g, b, eps=1e-5):
    mu = x.mean(-1, keepdims=True)
    xc = x - mu
    var = (xc * xc).mean(-1, keepdims=True)
    inv = 1.0 / np.sqrt(var + eps)
    xh = xc * inv
    return xh * g + b, (xh, inv)


def ln_b(dy, xh, inv, g):
    d_g = (dy * xh).sum(axis=(0, 1))
    d_b = dy.sum(axis=(0, 1))
    dxn = dy * g
    dx = inv * (dxn - dxn.mean(-1, keepdims=True)
                - xh * (dxn * xh).mean(-1, keepdims=True))
    return dx, d_g, d_b


def gelu_f(x):
    t = np.tanh(np.sqrt(2.0 / np.pi) * (x + 0.044715 * x ** 3))
    return 0.5 * x * (1.0 + t), t


def gelu_b(dy, x, t):
    dt = (1.0 - t * t) * np.sqrt(2.0 / np.pi) * (1.0 + 3 * 0.044715 * x ** 2)
    return dy * (0.5 * (1.0 + t) + 0.5 * x * dt)


def softmax(x):
    x = x - x.max(-1, keepdims=True)
    e = np.exp(x)
    return e / e.sum(-1, keepdims=True)


class TinyGPT:
    def __init__(self, V, D, L, T, F):
        self.V, self.D, self.L, self.T, self.F = V, D, L, T, F
        s = 0.02
        self.p = {"wte": RNG.normal(0, s, (V, D)), "wpe": RNG.normal(0, s, (T, D))}
        for l in range(L):
            pre = f"l{l}_"
            self.p.update({
                pre + "ln1_g": np.ones(D),  pre + "ln1_b": np.zeros(D),
                pre + "wqkv": RNG.normal(0, s, (D, 3 * D)), pre + "bqkv": np.zeros(3 * D),
                pre + "wo": RNG.normal(0, s / np.sqrt(2 * L), (D, D)), pre + "bo": np.zeros(D),
                pre + "ln2_g": np.ones(D),  pre + "ln2_b": np.zeros(D),
                pre + "fc1_w": RNG.normal(0, s, (D, F)), pre + "fc1_b": np.zeros(F),
                pre + "fc2_w": RNG.normal(0, s, (F, D)), pre + "fc2_b": np.zeros(D),
            })
        self.p["lnf_g"] = np.ones(D)
        self.p["lnf_b"] = np.zeros(D)
        self.keys = list(self.p)

    def forward(self, idx, save=False):
        Bn, T = idx.shape
        D = self.D
        x = self.p["wte"][idx] + self.p["wpe"][:T]
        caches = []
        for l in range(self.L):
            pre = f"l{l}_"
            h, c1 = ln_f(x, self.p[pre + "ln1_g"], self.p[pre + "ln1_b"])
            qkv = h @ self.p[pre + "wqkv"] + self.p[pre + "bqkv"]
            q, k, v = np.split(qkv, 3, axis=-1)
            hd = D // NH
            qs = q.reshape(Bn, T, NH, hd).transpose(0, 2, 1, 3)
            ks = k.reshape(Bn, T, NH, hd).transpose(0, 2, 1, 3)
            vs = v.reshape(Bn, T, NH, hd).transpose(0, 2, 1, 3)
            S = qs @ ks.transpose(0, 1, 3, 2) / np.sqrt(hd)
            mask = np.triu(np.full((T, T), -1e10), k=1)
            A = softmax(S + mask)
            y = (A @ vs).transpose(0, 2, 1, 3).reshape(Bn, T, D)
            a1 = y @ self.p[pre + "wo"] + self.p[pre + "bo"]
            x2 = x + a1
            h2, c2 = ln_f(x2, self.p[pre + "ln2_g"], self.p[pre + "ln2_b"])
            z = h2 @ self.p[pre + "fc1_w"] + self.p[pre + "fc1_b"]
            f1, tg = gelu_f(z)
            a2 = f1 @ self.p[pre + "fc2_w"] + self.p[pre + "fc2_b"]
            x3 = x2 + a2
            if save:
                caches.append((pre, h, c1, qkv, qs, ks, vs, A, y, x, x2, h2, c2, z, tg, f1))
            x = x3
        xf, cf = ln_f(x, self.p["lnf_g"], self.p["lnf_b"])
        logits = xf @ self.p["wte"].T
        if save:
            caches.append(("lnf", None, cf, None, None, None, None, None, None, x, None, None, None, None, None, xf))
        return logits, caches

    def loss_and_grads(self, idx, tgt):
        Bn, T = idx.shape
        logits, caches = self.forward(idx, save=True)
        V = self.V
        probs = softmax(logits.reshape(-1, V))
        loss = -np.log(probs[np.arange(Bn * T), tgt.reshape(-1)] + 1e-9).mean()
        dlogits = probs.copy()
        dlogits[np.arange(Bn * T), tgt.reshape(-1)] -= 1.0
        dlogits /= Bn * T
        dlogits = dlogits.reshape(Bn, T, V)
        grads = {k: np.zeros_like(p) for k, p in self.p.items()}
        xf = caches[-1][15]
        grads["wte"] += np.einsum("btv,btd->vd", dlogits, xf)
        dxf = dlogits @ self.p["wte"]
        dxb, d_g, d_b = ln_b(dxf, *caches[-1][2], self.p["lnf_g"])
        grads["lnf_g"] += d_g
        grads["lnf_b"] += d_b
        dx = dxb
        for l in reversed(range(self.L)):
            pre, h, c1, qkv, qs, ks, vs, A, y, x_in, x2, h2, c2, z, tg, f1 = caches[l]
            da2 = dx
            grads[pre + "fc2_w"] += f1.reshape(-1, self.F).T @ da2.reshape(-1, self.D)
            grads[pre + "fc2_b"] += da2.sum((0, 1))
            df1 = da2 @ self.p[pre + "fc2_w"].T
            dz = gelu_b(df1, z, tg)
            grads[pre + "fc1_w"] += h2.reshape(-1, self.D).T @ dz.reshape(-1, self.F)
            grads[pre + "fc1_b"] += dz.sum((0, 1))
            dh2 = dz @ self.p[pre + "fc1_w"].T
            dx2b, d_g2, d_b2 = ln_b(dh2, *c2, self.p[pre + "ln2_g"])
            grads[pre + "ln2_g"] += d_g2
            grads[pre + "ln2_b"] += d_b2
            # x3 = x2 + a2  ⇒  d(x2) = dx (identity) + dx2b (a2 branch)
            dx2 = dx + dx2b
            da1 = dx2
            grads[pre + "wo"] += y.reshape(-1, self.D).T @ da1.reshape(-1, self.D)
            grads[pre + "bo"] += da1.sum((0, 1))
            dy = da1 @ self.p[pre + "wo"].T
            hd = self.D // NH
            dy4 = dy.reshape(Bn, T, NH, hd).transpose(0, 2, 1, 3)
            dA = dy4 @ vs.transpose(0, 1, 3, 2)
            dvs = A.transpose(0, 1, 3, 2) @ dy4
            dS = A * (dA - (dA * A).sum(-1, keepdims=True))
            dqs = dS @ ks
            dks = dS.transpose(0, 1, 3, 2) @ qs
            dqs /= np.sqrt(hd)
            dks /= np.sqrt(hd)
            dq = dqs.transpose(0, 2, 1, 3).reshape(Bn, T, self.D)
            dk = dks.transpose(0, 2, 1, 3).reshape(Bn, T, self.D)
            dv = dvs.transpose(0, 2, 1, 3).reshape(Bn, T, self.D)
            dqkv = np.concatenate([dq, dk, dv], axis=-1)
            grads[pre + "wqkv"] += h.reshape(-1, self.D).T @ dqkv.reshape(-1, 3 * self.D)
            grads[pre + "bqkv"] += dqkv.sum((0, 1))
            dh = dqkv @ self.p[pre + "wqkv"].T
            dhx, d_g1, d_b1 = ln_b(dh, *c1, self.p[pre + "ln1_g"])
            grads[pre + "ln1_g"] += d_g1
            grads[pre + "ln1_b"] += d_b1
            dx = dhx + da1  # residual through x2 = x_in + a1
        np.add.at(grads["wte"], idx.reshape(-1), dx.reshape(-1, self.D))
        grads["wpe"] += dx.sum(0)
        return loss, grads

    def sample(self, stoi, itos, prompt, n, temp=0.8):
        ids = [stoi[c] for c in prompt]
        out = list(ids)
        for _ in range(n):
            ctx = np.array(out[-self.T:])[None, :]
            logits, _ = self.forward(ctx)
            lg = logits[0, -1] / temp
            lg = lg - lg.max()
            pr = softmax(lg)
            nxt = RNG.choice(self.V, p=pr)
            out.append(int(nxt))
        return "".join(itos[i] for i in out)

    def export(self, path):
        keys = ["wte", "wpe"]
        for l in range(self.L):
            pre = f"l{l}_"
            keys += [pre + k for k in ("ln1_g", "ln1_b", "wqkv", "bqkv", "wo", "bo",
                                       "ln2_g", "ln2_b", "fc1_w", "fc1_b", "fc2_w", "fc2_b")]
        keys += ["lnf_g", "lnf_b"]
        with open(path, "wb") as f:
            f.write(b"KLWM")
            f.write(np.array([self.V, self.D, self.L, self.T], dtype="<u4").tobytes())
            for k in keys:
                f.write(self.p[k].astype("<f4").tobytes())
        n = sum(self.p[k].size for k in keys)
        print(f"exported {n} params ({n * 4 / 1e6:.2f} MB f32) -> {path}")


def check_grads(g, idx, tgt, n=40):
    _, grads = g.loss_and_grads(idx, tgt)
    flat = [(k, i) for k in g.keys for i in range(0, g.p[k].size, max(1, g.p[k].size // 3))]
    picks = [flat[RNG.integers(len(flat))] for _ in range(n)]
    eps = 1e-4
    worst = 0.0
    for k, i in picks:
        old = g.p[k].flat[i]
        g.p[k].flat[i] = old + eps
        lp, _ = g.loss_and_grads(idx, tgt)
        g.p[k].flat[i] = old - eps
        lm, _ = g.loss_and_grads(idx, tgt)
        g.p[k].flat[i] = old
        num = (lp - lm) / (2 * eps)
        ana = grads[k].flat[i]
        rel = abs(num - ana) / max(1e-6, abs(num) + abs(ana))
        worst = max(worst, rel)
        if rel > 1e-2 and worst == rel:
            print(f"  grad MISMATCH {k}[{i}]: num={num:.6f} ana={ana:.6f} rel={rel:.4f}")
    print(f"grad check worst rel err = {worst:.5f} ({'PASS' if worst < 1e-2 else 'FAIL'})")
    return worst < 1e-2


def main():
    data_path, out_path = sys.argv[1], sys.argv[2]
    args = sys.argv[3:]
    steps = int(args[0]) if args and not args[0].startswith("--") else 3000
    do_check = "--check" in args
    text = open(data_path, "r", encoding="utf-8").read()
    chars = sorted(set(text))
    V = len(chars)
    stoi = {c: i for i, c in enumerate(chars)}
    itos = {i: c for c, i in stoi.items()}
    ids = np.array([stoi[c] for c in text], dtype=np.int64)
    D, T, L, F = 128, 64, 4, 512
    g = TinyGPT(V, D, L, T, F)
    n_par = sum(p.size for p in g.p.values())
    print(f"model: V={V} D={D} L={L} T={T} F={F} -> {n_par / 1e6:.2f}M params")
    if do_check:
        ix = RNG.integers(0, len(ids) - T - 1, size=4)
        xb = np.stack([ids[i:i + T] for i in ix])
        yb = np.stack([ids[i + 1:i + T + 1] for i in ix])
        if not check_grads(g, xb, yb):
            sys.exit(1)
    params = [g.p[k] for k in g.keys]
    m = [np.zeros_like(p) for p in params]
    v_ = [np.zeros_like(p) for p in params]
    lr, b1, b2, eps = 3e-3, 0.9, 0.95, 1e-8
    loss = 0.0
    for step in range(1, steps + 1):
        ix = RNG.integers(0, len(ids) - T - 1, size=16)
        xb = np.stack([ids[i:i + T] for i in ix])
        yb = np.stack([ids[i + 1:i + T + 1] for i in ix])
        loss, grads = g.loss_and_grads(xb, yb)
        for i, k in enumerate(g.keys):
            gr = grads[k]
            m[i] = b1 * m[i] + (1 - b1) * gr
            v_[i] = b2 * v_[i] + (1 - b2) * (gr * gr)
            mh = m[i] / (1 - b1 ** step)
            vh = v_[i] / (1 - b2 ** step)
            g.p[k] -= lr * mh / (np.sqrt(vh) + eps)
        if step % 100 == 0 or step == 1:
            print(f"step {step:5d} loss {loss:.4f}", flush=True)
        if step % 500 == 0:
            lr *= 0.85
    print("sample:", g.sample(stoi, itos, "JULIET:\n", 200))
    g.export(out_path)
    with open(out_path + ".meta", "w") as f:
        f.write(f"V={V} D={D} L={L} T={T} F={F} steps={steps} final_loss={loss:.4f}\n")


if __name__ == "__main__":
    main()
