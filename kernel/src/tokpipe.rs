//! Phase 4 — token IPC 零拷贝管道（ROADMAP §P4-3）
//!
//! 脑↔应用的 token 流管道：环形缓冲 + 生产/消费游标 + **零拷贝读**（借用
//! 窗口 view() 直接返回内部 slice，无需 copy 到用户缓冲）。语义上是 pipe
//! 的 AI 特化：等宽 token 流、高吞吐、可背压。
//! v0 单生产者/单消费者（内核内部与脑端演示已足够），多生产者 v1。

pub const TOKEN_SLOTS: usize = 256;

/// 一个 token 的载荷上限（模拟 embedding/logit 窗口片段）。
pub const TOKEN_MAX: usize = 64;

#[derive(Clone, Copy)]
pub struct TokenSlot {
    pub len: u16,
    pub data: [u8; TOKEN_MAX],
}

/// 零拷贝 token 环：SPSC。
pub struct TokenPipe {
    ring: [TokenSlot; TOKEN_SLOTS],
    head: usize, // 生产者写位置
    tail: usize, // 消费者读位置
    dropped: u64,
}

impl TokenPipe {
    pub fn new() -> Self {
        Self {
            ring: core::array::from_fn(|_| TokenSlot {
                len: 0,
                data: [0u8; TOKEN_MAX],
            }),
            head: 0,
            tail: 0,
            dropped: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.head.wrapping_sub(self.tail) % (TOKEN_SLOTS * 2)
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// 生产（拷贝入环——入队一次拷贝，读侧零拷贝）。
    /// 环满时丢最旧（AI 流语义：保最新），dropped 计数。
    pub fn push(&mut self, data: &[u8]) {
        let n = data.len().min(TOKEN_MAX);
        // 满：tail 前移丢最旧
        if self.len() == TOKEN_SLOTS {
            self.tail = (self.tail + 1) % (TOKEN_SLOTS * 2);
            self.dropped += 1;
        }
        let slot = &mut self.ring[self.head % TOKEN_SLOTS];
        slot.len = n as u16;
        slot.data[..n].copy_from_slice(&data[..n]);
        self.head = (self.head + 1) % (TOKEN_SLOTS * 2);
    }

    /// 零拷贝消费：返回 (迭代器化的借用窗口, token 数)。
    /// 借用窗口从 tail 起 cnt 个连续逻辑 token（绕环折叠为两段切片）。
    pub fn view(&self, max: usize) -> ([&[TokenSlot]; 2], usize) {
        let avail = self.len().min(max);
        if avail == 0 {
            return ([&[], &[]], 0);
        }
        let t = self.tail % TOKEN_SLOTS;
        let first = (TOKEN_SLOTS - t).min(avail);
        let (s0, s1) = if first == avail {
            (&self.ring[t..t + first], &self.ring[0..0])
        } else {
            (&self.ring[t..], &self.ring[..avail - first])
        };
        ([s0, s1], avail)
    }

    /// 消费 n 个 token（推进 tail；配合 view 使用）。
    pub fn consume(&mut self, n: usize) {
        self.tail = (self.tail + n) % (TOKEN_SLOTS * 2);
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Token IPC Tests ──");

    crate::test::run_test("tokpipe_fifo_roundtrip", || {
        // 16KB TokenPipe 放堆上，避免内核栈溢出
        let p = alloc::boxed::Box::leak(alloc::boxed::Box::new(TokenPipe::new()));
        for i in 0..8u32 {
            p.push(&i.to_le_bytes());
        }
        let (win, n) = p.view(16);
        if n != 8 {
            return false;
        }
        // FIFO：按序读回
        let mut ok = true;
        for (i, slot) in win.iter().flat_map(|s| s.iter()).enumerate() {
            ok &= slot.data[..4] == (i as u32).to_le_bytes();
        }
        p.consume(8);
        let empty = p.is_empty();

        ok && empty
    });

    crate::test::run_test("tokpipe_wraparound_zero_copy", || {
        let p = alloc::boxed::Box::leak(alloc::boxed::Box::new(TokenPipe::new()));
        // 填满再消费，把 tail 推到接近环尾制造 wrap
        for i in 0..250u32 {
            p.push(&i.to_le_bytes());
        }
        p.consume(245);
        for i in 0..10u32 {
            p.push(&(1000 + i).to_le_bytes());
        }
        let (win, n) = p.view(64);
        // 剩余 = 5(旧) + 10(新) = 15，跨 wrap 应拆两段且内容正确
        if n != 15 {
            return false;
        }
        let seq: alloc::vec::Vec<u32> = win
            .iter()
            .flat_map(|s| s.iter())
            .map(|slot| u32::from_le_bytes(slot.data[..4].try_into().unwrap()))
            .collect();
        let ok = seq[0] == 245
            && seq[4] == 249
            && seq[5] == 1000
            && seq[14] == 1009
            && win[0].len() + win[1].len() == 15;

        ok
    });

    crate::test::run_test("tokpipe_backpressure_drops_oldest", || {
        let p = alloc::boxed::Box::leak(alloc::boxed::Box::new(TokenPipe::new()));
        for i in 0..(TOKEN_SLOTS as u32 + 7) {
            p.push(&i.to_le_bytes());
        }
        // 超容量 7 个 → 丢最旧 7 个，dropped 计数正确
        if p.dropped() != 7 {
            return false;
        }
        let (win, n) = p.view(TOKEN_SLOTS);
        let ok = n == TOKEN_SLOTS && win[0][0].data[..4] == 7u32.to_le_bytes();

        ok
    });
}
