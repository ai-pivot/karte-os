//! kernel/src/sched/ready_queue.rs — 32-level priority-bitmap ready queue (P1.1)
//!
//! O(1) pick-next via a u32 bitmap: one bit per non-empty priority level,
//! `trailing_zeros` finds the highest-priority runnable level directly.
//! AiBatch starvation protection: if a ready AiBatch task has been skipped for
//! AI_BATCH_STARVATION_TICKS consecutive ticks it is re-queued at
//! AI_BATCH_BOOST_PRIO (the Normal band) until it gets picked once.

use alloc::collections::VecDeque;

use super::class::{AI_BATCH_BOOST_PRIO, AI_BATCH_PRIO, AI_BATCH_STARVATION_TICKS};

const LEVELS: usize = 32;

/// Opaque per-task handle stored in the queue (scheduler slot id).
pub type QueueToken = usize;

pub struct ReadyQueue {
    bitmap: u32,
    queues: [VecDeque<QueueToken>; LEVELS],
    /// Consecutive ticks during which a ready AiBatch task went unpicked.
    batch_starved: u32,
}

impl ReadyQueue {
    pub const fn new() -> Self {
        Self {
            bitmap: 0,
            queues: [const { VecDeque::new() }; LEVELS],
            batch_starved: 0,
        }
    }

    /// Insert `token` for `prio`. Duplicate tokens are the caller's contract
    /// to avoid (a task is enqueued at most once between wakeups).
    pub fn push(&mut self, token: QueueToken, prio: u8) {
        let lvl = (prio as usize).min(LEVELS - 1);
        if !self.queues[lvl].contains(&token) {
            self.queues[lvl].push_back(token);
        }
        self.bitmap |= 1 << lvl;
    }

    /// Highest-priority token, or None. Starvation protection: after N
    /// consecutive picks that left a READY AiBatch task waiting, promote it to
    /// the Normal band (queue tail at boosted level) so it competes fairly.
    pub fn pop_next(&mut self) -> Option<QueueToken> {
        let batch_lvl = AI_BATCH_PRIO as usize;
        let boosted_lvl = AI_BATCH_BOOST_PRIO as usize;
        // Promote (move) one starved batch task to the Normal band's tail —
        // never ahead of strictly higher-priority waiters, since the pick
        // below still scans the bitmap from the top.
        if self.batch_starved >= AI_BATCH_STARVATION_TICKS && !self.queues[batch_lvl].is_empty() {
            if let Some(tok) = self.queues[batch_lvl].pop_front() {
                if self.queues[batch_lvl].is_empty() {
                    self.bitmap &= !(1 << batch_lvl);
                }
                self.queues[boosted_lvl].push_back(tok);
                self.bitmap |= 1 << boosted_lvl;
                self.batch_starved = 0;
            }
        }
        if self.bitmap == 0 {
            return None;
        }
        let lvl = self.bitmap.trailing_zeros() as usize;
        let pick = self.pick_from(lvl);
        if pick.is_some() {
            if lvl == batch_lvl || lvl == boosted_lvl {
                // A batch task was served; starvation clock resets.
                self.batch_starved = 0;
            } else if !self.queues[batch_lvl].is_empty() {
                self.batch_starved += 1;
            } else {
                self.batch_starved = 0;
            }
        }
        pick
    }

    fn pick_from(&mut self, lvl: usize) -> Option<QueueToken> {
        let token = self.queues[lvl].pop_front()?;
        if self.queues[lvl].is_empty() {
            self.bitmap &= !(1 << lvl);
        }
        Some(token)
    }

    /// Remove a specific token wherever it sits (used when a queued task is
    /// killed or re-prioritized). Returns true if it was found.
    pub fn remove(&mut self, token: QueueToken) -> bool {
        for lvl in 0..LEVELS {
            if let Some(pos) = self.queues[lvl].iter().position(|&t| t == token) {
                self.queues[lvl].remove(pos);
                if self.queues[lvl].is_empty() {
                    self.bitmap &= !(1 << lvl);
                }
                return true;
            }
        }
        false
    }

    pub fn is_empty(&self) -> bool {
        self.bitmap == 0
    }

    pub fn len(&self) -> usize {
        self.queues.iter().map(|q| q.len()).sum()
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    use super::class::*;
    use crate::test::run_test;

    // 1000 random enqueues/dequeues must match a linear-scan reference model
    // exactly (ROADMAP P1.1 acceptance: O(1) bitmap selection stays
    // order-correct). Deterministic LCG so failures are reproducible.
    run_test("readyqueue_bitmap_priority_order_1k", || {
        let mut seed: u32 = 0x1234_5678;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 16) as usize
        };
        let mut q = ReadyQueue::new();
        // Reference model: (token, prio), linear-scan min-prio wins, FIFO tie-break.
        let mut model: alloc::vec::Vec<(usize, u8)> = alloc::vec::Vec::new();
        let mut token_ctr = 0usize;
        for _ in 0..1000 {
            match next() % 3 {
                0 | 1 => {
                    // Levels 0..=30 only: level 31 (AiBatch) is excluded so
                    // the starvation-boost logic never perturbs this test —
                    // boost semantics are covered by their own test below.
                    let prio = (next() % 31) as u8;
                    let t = token_ctr;
                    token_ctr += 1;
                    q.push(t, prio);
                    model.push((t, prio));
                }
                _ => {
                    if model.is_empty() {
                        if q.pop_next().is_some() {
                            return false; // popped from an empty queue
                        }
                    } else {
                        // Highest priority (lowest level), earliest arrival.
                        let best = model
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, (_, p))| *p)
                            .map(|(i, _)| i)
                            .unwrap();
                        let (want, _) = model.remove(best);
                        if q.pop_next() != Some(want) {
                            return false;
                        }
                    }
                }
            }
        }
        // Drain: both structures must empty out in identical order.
        while !model.is_empty() {
            let best = model
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, p))| *p)
                .map(|(i, _)| i)
                .unwrap();
            let (want, _) = model.remove(best);
            if q.pop_next() != Some(want) {
                return false;
            }
        }
        q.pop_next().is_none() && q.is_empty()
    });

    // RT strictly beats Normal and AiBatch regardless of arrival order.
    run_test("readyqueue_rt_beats_normal_and_batch", || {
        let mut q = ReadyQueue::new();
        q.push(1, AI_BATCH_PRIO);
        q.push(2, NORMAL_PRIO);
        q.push(3, SchedClass::RtFifo(1).priority()); // prio 0
        let first = q.pop_next();
        let second = q.pop_next();
        let third = q.pop_next();
        first == Some(3) && second == Some(2) && third == Some(1)
    });

    // Same level keeps FIFO arrival order; remove() works mid-queue.
    run_test("readyqueue_fifo_order_and_remove", || {
        let mut q = ReadyQueue::new();
        q.push(10, 5);
        q.push(11, 5);
        q.push(12, 5);
        q.remove(11);
        let a = q.pop_next();
        let b = q.pop_next();
        let c = q.pop_next();
        a == Some(10) && b == Some(12) && c.is_none()
    });

    // AiBatch starvation protection: after N consecutive picks that left a
    // READY batch task waiting, the batch task is promoted into the Normal
    // band (never ahead of strictly higher-priority waiters) and is served
    // as soon as it reaches the front of that band.
    run_test("readyqueue_aibatch_starvation_boost", || {
        let mut q = ReadyQueue::new();
        q.push(1, NORMAL_PRIO); // Normal waiter in the boosted band
        q.push(2, AI_BATCH_PRIO); // starved batch task
        for _ in 0..AI_BATCH_STARVATION_TICKS {
            if q.pop_next() != Some(1) {
                return false;
            }
            q.push(1, NORMAL_PRIO);
        }
        // A high-priority (RT-band) arrival must still win over the promoted
        // batch task — boost may not jump the priority order.
        q.push(3, SchedClass::RtFifo(1).priority());
        let a = q.pop_next();
        let b = q.pop_next();
        let c = q.pop_next();
        a == Some(3) && b == Some(1) && c == Some(2)
    });
}
