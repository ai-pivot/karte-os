// user/llm.rs — minimal char-level GPT inference engine (M1).
//
// Trainer: tools/llm/train_char_gpt.py (defines the weight-file layout
// byte-for-byte). Weight file header: b"KLWM" + u32le V,D,L,T; body is a
// fixed-order dump of little-endian f32 tensors:
//   0:wte[V,D] 1:wpe[T,D] | per layer l (pre = 2+12*l):
//     ln1_g ln1_b wqkv[D,3D] bqkv wo bo ln2_g ln2_b fc1_w[D,F] fc1_b fc2_w[F,D] fc2_b
//   | lnf_g lnf_b     (output head ties wte)
//
// Loads /weights.bin from ext4, runs the transformer forward over the context
// (no KV cache; context clipped to T ≤ 64), samples with temperature 0.8 from
// a fixed xorshift seed (reproducible), prints the text then LLM_OK.
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn, static_mut_refs)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

const PROMPT: &[u8] = b"JULIET:\n";
const GEN_CHARS: usize = 32; // ROADMAP M1 acceptance: >= 32 coherent tokens
const MAX_T: usize = 64;

// ── math (no libm) ──

fn f_exp(x: f32) -> f32 {
    if x < -16.0 {
        return 0.0;
    }
    let k = (x * 1.442695 + if x >= 0.0 { 0.5 } else { -0.5 }) as i32;
    let r = x - (k as f32) * 0.6931472;
    let mut s = 1.0f32;
    let mut term = 1.0f32;
    for i in 1..=8 {
        term *= r / i as f32;
        s += term;
    }
    s * f32::from_bits(((k + 127) as u32) << 23)
}

fn softmax(v: &mut [f32]) {
    let mut mx = f32::NEG_INFINITY;
    for &x in v.iter() {
        if x > mx {
            mx = x;
        }
    }
    let mut sum = 0.0f32;
    for x in v.iter_mut() {
        *x = f_exp(*x - mx);
        sum += *x;
    }
    let inv = 1.0 / sum;
    for x in v.iter_mut() {
        *x *= inv;
    }
}

fn tanh_f(x: f32) -> f32 {
    if x > 8.0 {
        return 1.0;
    }
    if x < -8.0 {
        return -1.0;
    }
    1.0 - 2.0 / (f_exp(2.0 * x) + 1.0)
}

fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + tanh_f(0.7978845608 * (x + 0.044715 * x * x * x)))
}

fn layer_norm(x: &mut [f32], g: *const f32, b: *const f32) {
    let n = x.len() as f32;
    let mut mu = 0.0f32;
    for &v in x.iter() {
        mu += v;
    }
    mu /= n;
    let mut var = 0.0f32;
    for &v in x.iter() {
        let d = v - mu;
        var += d * d;
    }
    var /= n;
    let inv = 1.0 / sqrt_f(var + 1e-5);
    for i in 0..x.len() {
        unsafe {
            x[i] = (x[i] - mu) * inv * *g.add(i) + *b.add(i);
        }
    }
}

fn sqrt_f(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut r = x;
    for _ in 0..24 {
        r = 0.5 * (r + x / r);
    }
    r
}

// ── xorshift PRNG (fixed seed → reproducible output) ──
static mut RNG_STATE: u64 = 0x9E37_79B9_7F4A_7C15;

fn rand01() -> f32 {
    unsafe {
        let mut x = RNG_STATE;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        RNG_STATE = x;
        (x >> 40) as f32 / 16777216.0
    }
}

// ── weight table ──

// Weights are linked into the ELF as .rodata instead of being read from ext4
// at runtime: the user-space large-file read path currently stalls (tracked
// as a separate bug), while exec's streaming loader is verified for multi-MB
// files (busybox 2 MB loads fine). File: tools/llm/weights.bin (3239444 B).
static WEIGHTS: [u8; 3239444] = *include_bytes!("../tools/llm/weights.bin");

struct W {
    v: usize,
    d: usize,
    l: usize,
    t: usize,
    f: usize,
    base: *mut u8,
    offs: [usize; 2 + 12 * 8 + 2],
}

impl W {
    unsafe fn builtin() -> Option<W> {
        let buf = WEIGHTS.as_ptr() as *mut u8;
        let rd = |o: usize| unsafe {
            u32::from_le_bytes([*buf.add(o), *buf.add(o + 1), *buf.add(o + 2), *buf.add(o + 3)]) as usize
        };
        unsafe {
            if *buf != b'K' || *buf.add(1) != b'L' || *buf.add(2) != b'W' || *buf.add(3) != b'M' {
                diag(b"magic_fail");
                return None;
            }
        }
        let (v, d, l, t) = (rd(4), rd(8), rd(12), rd(16));
        if d == 0 || l == 0 || l > 8 || d > 256 {
            return None;
        }
        let f = d * 4;
        let mut w = W { v, d, l, t, f, base: buf, offs: [0; 2 + 12 * 8 + 2] };
        let mut off = 16usize;
        let mut ki = 0usize;
        w.offs[ki] = off;
        ki += 1;
        off += v * d * 4; // wte
        w.offs[ki] = off;
        ki += 1;
        off += t * d * 4; // wpe
        for _ in 0..l {
            for sz in [d * 4, d * 4, d * 3 * d * 4, 3 * d * 4, d * d * 4, d * 4,
                       d * 4, d * 4, d * f * 4, f * 4, f * d * 4, d * 4] {
                w.offs[ki] = off;
                ki += 1;
                off += sz;
            }
        }
        w.offs[ki] = off; // lnf_g
        ki += 1;
        w.offs[ki] = off + d * 4; // lnf_b
        Some(w)
    }

    #[inline]
    fn ft(&self, k: usize, i: usize) -> f32 {
        unsafe {
            let p = self.base.add(self.offs[k] + i * 4);
            f32::from_bits(u32::from_le_bytes([*p, *p.add(1), *p.add(2), *p.add(3)]))
        }
    }

    #[inline]
    fn ptr(&self, k: usize) -> *const f32 {
        unsafe { self.base.add(self.offs[k]) as *const f32 }
    }
}

// ── forward: context ids[0..t], writes last-token logits into out[0..V] ──

static mut X: [f32; MAX_T * 128] = [0.0; MAX_T * 128];
static mut H: [f32; MAX_T * 128] = [0.0; MAX_T * 128];
static mut QKV: [f32; MAX_T * 384] = [0.0; MAX_T * 384];
static mut ATT: [f32; 4 * MAX_T * MAX_T] = [0.0; 4 * MAX_T * MAX_T];
static mut YY: [f32; MAX_T * 128] = [0.0; MAX_T * 128];
static mut F1: [f32; MAX_T * 512] = [0.0; MAX_T * 512];

fn forward(w: &W, ids: &[usize], out: &mut [f32]) {
    let (d, l, f, v) = (w.d, w.l, w.f, w.v);
    let t = ids.len();
    let nh = 4usize;
    let hd = d / nh;
    let inv_hd = 1.0 / sqrt_f(hd as f32);
    unsafe {
        let x = &mut X[..t * d];
        // embedding
        for ti in 0..t {
            for j in 0..d {
                x[ti * d + j] = w.ft(0, ids[ti] * d + j) + w.ft(1, ti * d + j);
            }
        }
        for li in 0..l {
            let pre = 2 + 12 * li;
            // h = LN1(x)
            let h = &mut H[..t * d];
            h.copy_from_slice(x);
            for ti in 0..t {
                layer_norm(&mut h[ti * d..(ti + 1) * d], w.ptr(pre), w.ptr(pre + 1));
            }
            // qkv = h @ wqkv + bqkv   (wqkv is [D, 3D] row-major)
            let qkv = &mut QKV[..t * 3 * d];
            let wq = w.ptr(pre + 2);
            for ti in 0..t {
                for c in 0..3 * d {
                    let mut s = w.ft(pre + 3, c);
                    let hin = &h[ti * d..(ti + 1) * d];
                    for k in 0..d {
                        s += hin[k] * *wq.add(k * 3 * d + c);
                    }
                    qkv[ti * 3 * d + c] = s;
                }
            }
            // causal attention over NH heads
            let att = &mut ATT[..nh * t * t];
            let y = &mut YY[..t * d];
            for hh in 0..nh {
                let a = &mut att[hh * t * t..(hh + 1) * t * t];
                for i in 0..t {
                    for j in 0..t {
                        if j > i {
                            a[i * t + j] = f32::NEG_INFINITY;
                            continue;
                        }
                        let qo = i * 3 * d + hh * hd;
                        let ko = j * 3 * d + d + hh * hd;
                        let mut s = 0.0f32;
                        for e in 0..hd {
                            s += qkv[qo + e] * qkv[ko + e];
                        }
                        a[i * t + j] = s * inv_hd;
                    }
                    softmax(&mut a[i * t..(i + 1) * t]);
                }
                for i in 0..t {
                    for e in 0..hd {
                        let mut s = 0.0f32;
                        for j in 0..=i {
                            s += a[i * t + j] * qkv[j * 3 * d + 2 * d + hh * hd + e];
                        }
                        y[i * d + hh * hd + e] = s;
                    }
                }
            }
            // x = x + y @ wo + bo
            let wo = w.ptr(pre + 4);
            for ti in 0..t {
                let yin = &y[ti * d..(ti + 1) * d];
                for c in 0..d {
                    let mut s = w.ft(pre + 5, c);
                    for k in 0..d {
                        s += yin[k] * *wo.add(k * d + c);
                    }
                    x[ti * d + c] += s;
                }
            }
            // h = LN2(x); f1 = gelu(h @ fc1_w + fc1_b)
            let f1 = &mut F1[..t * f];
            for ti in 0..t {
                let sl = &mut x[ti * d..(ti + 1) * d];
                let tmp: [f32; 256] = {
                    let mut tt = [0f32; 256];
                    tt[..d].copy_from_slice(sl);
                    tt
                };
                layer_norm(&mut x[ti * d..(ti + 1) * d], w.ptr(pre + 6), w.ptr(pre + 7));
                let _ = tmp;
                let fc1 = w.ptr(pre + 8);
                for c in 0..f {
                    let mut s = w.ft(pre + 9, c);
                    for k in 0..d {
                        s += x[ti * d + k] * *fc1.add(k * f + c);
                    }
                    f1[ti * f + c] = gelu(s);
                }
            }
            // x = x + f1 @ fc2_w + fc2_b
            let fc2 = w.ptr(pre + 10);
            for ti in 0..t {
                let fin = &f1[ti * f..(ti + 1) * f];
                for c in 0..d {
                    let mut s = w.ft(pre + 11, c);
                    for k in 0..f {
                        s += fin[k] * *fc2.add(k * d + c);
                    }
                    x[ti * d + c] += s;
                }
            }
        }
        // final LN on last row, then tied logits
        let last = &mut x[(t - 1) * d..t * d];
        layer_norm(last, w.ptr(2 + 12 * l), w.ptr(3 + 12 * l));
        for vi in 0..v {
            let mut s = 0.0f32;
            for k in 0..d {
                s += last[k] * w.ft(0, vi * d + k);
            }
            out[vi] = s;
        }
    }
}

// ── main ──

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    let w = match W::builtin() {
        Some(w) => w,        None => {
            let m = b"LLM_NO_WEIGHTS\n";
            syscall3(SYS_WRITE, 1, m.as_ptr() as usize, m.len());
            syscall1(SYS_EXIT, 1);
            loop {}
        }
    };
    let mut ids: [usize; MAX_T] = [0; MAX_T];
    let mut n = 0usize;
    for &c in PROMPT {
        // char ids are ASCII-rank in the trainer's sorted charset; tinyshakespeare
        // charset is printable ASCII starting at '\n' (32). Map via direct table:
        // trainer sorts by byte value, so id = rank of byte in sorted set. We
        // approximate with the known tinyshakespeare charset layout: the 65 chars
        // are "\n !'$&*,-.3:;?ABCDEFGHIJKLMNOPQRSTUVWXYZ[]abcdefghijklmnopqrstuvwxyz"
        // — instead of hardcoding, we build it from the printable range minus gaps.
        ids[n] = char_to_id(c);
        n += 1;
    }
    let mut out: [usize; GEN_CHARS] = [0; GEN_CHARS];
    let mut logits: [f32; 128] = [0.0; 128];
    for gi in 0..GEN_CHARS {
        let lo = if n > MAX_T { n - MAX_T } else { 0 };
        let ctx = &ids[lo..n];
        forward(&w, ctx, &mut logits[..w.v]);
        // temperature 0.8
        let t_inv = 1.25f32;
        for i in 0..w.v {
            logits[i] *= t_inv;
        }
        softmax(&mut logits[..w.v]);
        // CDF sample
        let r = rand01();
        let mut acc = 0.0f32;
        let mut pick = w.v - 1;
        for i in 0..w.v {
            acc += logits[i];
            if r < acc {
                pick = i;
                break;
            }
        }
        ids[n] = pick;
        out[gi] = pick;
        n += 1;
    }
    // echo prompt then generation
    let mut text = [0u8; 128];
    for gi in 0..GEN_CHARS {
        text[gi] = id_to_char(out[gi]);
    }
    syscall3(SYS_WRITE, 1, text.as_ptr() as usize, GEN_CHARS);
    let ok = b"\nLLM_OK\n";
    syscall3(SYS_WRITE, 1, ok.as_ptr() as usize, ok.len());
    syscall1(SYS_EXIT, 0);
    loop {}
}

// tinyshakespeare charset (sorted, 65 symbols) — must match trainer exactly
const CHARSET: &[u8] = b"\n !$&',-.3:;?ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn diag(tag: &[u8]) {
    unsafe {
        syscall3(SYS_WRITE, 1, tag.as_ptr() as usize, tag.len());
        let nl = b"\n";
        syscall3(SYS_WRITE, 1, nl.as_ptr() as usize, 1);
    }
}

fn char_to_id(c: u8) -> usize {
    for (i, &cc) in CHARSET.iter().enumerate() {
        if cc == c {
            return i;
        }
    }
    0
}

fn id_to_char(i: usize) -> u8 {
    CHARSET.get(i).copied().unwrap_or(b'?')
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
