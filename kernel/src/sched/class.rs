//! kernel/src/sched/class.rs — Scheduling classes for Scheduler 2.0 (P1.1)
//!
//! Four classes map onto a shared 32-level priority space (0 = highest):
//!   RtFifo(n) / RtRoundRobin(n)  n ∈ 1..=16  → prio = n-1      (levels 0..15, real-time)
//!   Normal                        (no arg)     → prio = 24      (interactive/default)
//!   AiBatch                       (no arg)     → prio = 31      (background LLM work)
//! Levels 16..23 and 25..30 are reserved for future dynamic-nice support.

use crate::test::run_test;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedClass {
    /// Real-time FIFO: runs until it blocks, exits, or is preempted by a
    /// strictly higher-priority RT task.
    RtFifo(u8),
    /// Real-time round-robin: same as RtFifo but yields after its quantum.
    RtRoundRobin(u8),
    /// Default interactive class.
    Normal,
    /// Background batch work (e.g. on-device LLM inference). Lowest priority;
    /// protected from starvation by temporary boosting (see ReadyQueue).
    AiBatch,
}

pub const RT_LEVELS: u8 = 16;
pub const NORMAL_PRIO: u8 = 24;
pub const AI_BATCH_PRIO: u8 = 31;
/// Priority AiBatch tasks are boosted to when starved (same band as Normal).
pub const AI_BATCH_BOOST_PRIO: u8 = NORMAL_PRIO;
/// Ticks a ready AiBatch task may be skipped before being boosted.
pub const AI_BATCH_STARVATION_TICKS: u32 = 64;

impl SchedClass {
    /// Base priority in the 32-level space. RT tasks sort by their level.
    pub fn priority(&self) -> u8 {
        match self {
            SchedClass::RtFifo(n) | SchedClass::RtRoundRobin(n) => {
                let n = (*n).clamp(1, RT_LEVELS);
                n - 1
            }
            SchedClass::Normal => NORMAL_PRIO,
            SchedClass::AiBatch => AI_BATCH_PRIO,
        }
    }

    /// True for RtFifo/RtRoundRobin — RT always runs before Normal/AiBatch.
    pub fn is_realtime(&self) -> bool {
        matches!(self, SchedClass::RtFifo(_) | SchedClass::RtRoundRobin(_))
    }

    /// Scheduler ticks a running task of this class gets before requeue.
    /// RtFifo never expires (usize::MAX); everything else is round-robin.
    pub fn quantum(&self) -> usize {
        match self {
            SchedClass::RtFifo(_) => usize::MAX,
            SchedClass::RtRoundRobin(_) => 4,
            SchedClass::Normal => 2,
            SchedClass::AiBatch => 2,
        }
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    run_test("sched_class_priority_mapping", || {
        let cases = [
            (SchedClass::RtFifo(1), 0u8),
            (SchedClass::RtFifo(16), 15),
            (SchedClass::RtRoundRobin(8), 7),
            (SchedClass::Normal, NORMAL_PRIO),
            (SchedClass::AiBatch, AI_BATCH_PRIO),
        ];
        cases.iter().all(|(c, want)| c.priority() == *want)
    });

    run_test("sched_class_rt_clamp_and_flags", || {
        // Out-of-range RT levels clamp into the RT band, never into Normal/AiBatch.
        let hi = SchedClass::RtFifo(200).priority();
        let lo = SchedClass::RtRoundRobin(0).priority();
        hi < RT_LEVELS
            && lo == 0
            && SchedClass::RtFifo(1).is_realtime()
            && !SchedClass::Normal.is_realtime()
            && !SchedClass::AiBatch.is_realtime()
            && SchedClass::RtFifo(3).quantum() == usize::MAX
            && SchedClass::RtRoundRobin(3).quantum() == 4
    });
}
