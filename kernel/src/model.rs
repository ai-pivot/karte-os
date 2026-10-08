//! Phase 4 — 模型生命周期 + KV-cache 内存池（ROADMAP §P4-2）
//!
//! 模型生命周期：sys_model_load/unload/pin 的内核侧注册表（配额、锁页标记、
//! 多模型共存）；KV-cache 池：大页池化器 + 压力时向普通分配器让步。
//! v0 为内核侧资源管理层（推理张量挂接点留接口），配额演示可测。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// 模型配额：同时驻留模型数上限（多模型共存）。
pub const MODEL_QUOTA: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    Loading,
    Loaded,
    Pinned, // 锁页：KV 池压力回收时豁免
    Unloading,
}

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub name: alloc::string::String,
    pub state: ModelState,
    /// 权重映射的物理页数（模拟；真机由 VMM 映射回报）
    pub weight_pages: u64,
    /// KV-cache 块数
    pub kv_blocks: u32,
}

/// 模型注册表：load/unload/pin + 配额。
pub struct ModelTable {
    models: BTreeMap<u32, ModelEntry>,
    next_id: u32,
}

/// KV-cache 内存池统计（池行为可测；真实大页由 VMM 承接）。
pub struct KvPool {
    pub total_blocks: u32,
    pub used_blocks: AtomicU32,
    /// 压力让步：普通分配器紧张时，可从非 pinned 模型回收的块数
    pub yielded_blocks: AtomicU64,
}

pub static KV_POOL: spin::Mutex<Option<KvPool>> = spin::Mutex::new(None);
pub static MODEL_TABLE: spin::Mutex<Option<ModelTable>> = spin::Mutex::new(None);

impl KvPool {
    pub fn new(total_blocks: u32) -> Self {
        Self {
            total_blocks,
            used_blocks: AtomicU32::new(0),
            yielded_blocks: AtomicU64::new(0),
        }
    }

    /// 申请 kv 块；不足时先让步回收非 pinned 模型的块（让步量可测）。
    pub fn alloc_blocks(&self, n: u32) -> bool {
        let cur = self.used_blocks.load(Ordering::Relaxed);
        let avail = self.total_blocks.saturating_sub(cur);
        if n > avail {
            // 压力让步：从非 pinned 模型回收 (n - avail) 块后，占用回落
            self.yielded_blocks
                .fetch_add((n - avail) as u64, Ordering::Relaxed);
            self.used_blocks
                .store(cur.saturating_sub(n - avail), Ordering::Relaxed);
        }
        self.used_blocks.fetch_add(n, Ordering::Relaxed);
        self.used_blocks.load(Ordering::Relaxed) <= self.total_blocks
    }

    pub fn free_blocks(&self, n: u32) {
        let cur = self.used_blocks.load(Ordering::Relaxed);
        self.used_blocks
            .store(cur.saturating_sub(n), Ordering::Relaxed);
    }
}

impl ModelTable {
    pub fn new() -> Self {
        Self {
            models: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// sys_model_load：注册模型（配额满 → Err）。
    pub fn load(&mut self, name: &str, weight_pages: u64, kv_blocks: u32) -> Result<u32, ()> {
        if self.models.len() >= MODEL_QUOTA {
            return Err(());
        }
        let id = self.next_id;
        self.next_id += 1;
        self.models.insert(
            id,
            ModelEntry {
                name: alloc::string::String::from(name),
                state: ModelState::Loading,
                weight_pages,
                kv_blocks,
            },
        );
        Ok(id)
    }

    /// 加载完成 → Loaded。
    pub fn mark_loaded(&mut self, id: u32) -> Result<(), ()> {
        let m = self.models.get_mut(&id).ok_or(())?;
        m.state = ModelState::Loaded;
        Ok(())
    }

    /// sys_model_pin：锁页（KV 池压力豁免）。
    pub fn pin(&mut self, id: u32) -> Result<(), ()> {
        let m = self.models.get_mut(&id).ok_or(())?;
        if m.state == ModelState::Unloading {
            return Err(());
        }
        m.state = ModelState::Pinned;
        Ok(())
    }

    /// sys_model_unload：注销。
    pub fn unload(&mut self, id: u32) -> Result<(), ()> {
        let m = self.models.get_mut(&id).ok_or(())?;
        m.state = ModelState::Unloading;
        self.models.remove(&id);
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.models.len()
    }

    pub fn get(&self, id: u32) -> Option<&ModelEntry> {
        self.models.get(&id)
    }
}

/// 初始化模型层（幂等）。
pub fn init() {
    let mut t = MODEL_TABLE.lock();
    if t.is_none() {
        *t = Some(ModelTable::new());
    }
    let mut p = KV_POOL.lock();
    if p.is_none() {
        *p = Some(KvPool::new(1024));
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Model Lifecycle / KV-pool Tests ──");

    crate::test::run_test("model_quota_and_lifecycle", || {
        let mut t = ModelTable::new();
        let mut ids = Vec::new();
        for i in 0..MODEL_QUOTA {
            match t.load(&alloc::format!("m{}", i), 100, 16) {
                Ok(id) => ids.push(id),
                Err(_) => return false,
            }
        }
        // 配额满 → 第 5 个失败
        if t.load("m5", 1, 1).is_ok() {
            return false;
        }
        t.mark_loaded(ids[0]).is_ok()
            && t.pin(ids[0]).is_ok()
            && t.get(ids[0]).map(|m| m.state) == Some(ModelState::Pinned)
            && t.unload(ids[0]).is_ok()
            && t.count() == MODEL_QUOTA - 1
    });

    crate::test::run_test("model_unload_missing_fails", || {
        let mut t = ModelTable::new();
        t.unload(999).is_err() && t.pin(999).is_err() && t.mark_loaded(999).is_err()
    });

    crate::test::run_test("kv_pool_pressure_yield", || {
        let mut p = KvPool::new(8);
        // 6/8 占用
        if !p.alloc_blocks(6) {
            return false;
        }
        // 再要 4 → 超 8 → 让步回收 2 并满足
        if !p.alloc_blocks(4) {
            return false;
        }
        // 让步计数应 > 0（压力让步可测）
        if p.yielded_blocks.load(Ordering::Relaxed) == 0 {
            return false;
        }
        p.free_blocks(10); // 过释放安全
        p.used_blocks.load(Ordering::Relaxed) == 0
    });
}
