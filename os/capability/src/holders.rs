//! 对象侧 capability 持有计数。
//!
//! 计数覆盖表内、在途与事务暂持的 authority；普通存储引用不计入。
//! 状态为单个原子字：低位计数，最高位为终结封闭标记。归零判定与封闭在同一
//! 原子操作中完成，因此 Terminal 对象不存在“归零后被重新铸造”的窗口。

use core::sync::atomic::{AtomicU64, Ordering};

const SEALED: u64 = 1 << 63;
const COUNT_MASK: u64 = SEALED - 1;

/// 最后持有消失时对象的语义分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroPolicy {
    /// 归零无业务动作，可由合法派生重新铸造，不产生事件（如 ProcessControl）。
    Remintable,
    /// 归零即终结：同一原子操作封闭铸造入口并产生一次事件，永不复活。
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldersError {
    /// Terminal 对象已终结，不再接受铸造。
    Sealed,
    /// 计数达到准入上限；释放始终可完成，只有新增被拒绝。
    ReachLimit,
}

#[derive(Debug)]
pub struct Holders {
    state: AtomicU64,
    policy: ZeroPolicy,
    limit: u64,
}

impl Holders {
    /// 默认准入上限：远低于计数位宽，使上限检查与计数不可能回绕。
    pub const DEFAULT_LIMIT: u64 = u32::MAX as u64;

    pub const fn new(policy: ZeroPolicy) -> Self {
        Self::with_limit(policy, Self::DEFAULT_LIMIT)
    }

    pub const fn with_limit(policy: ZeroPolicy, limit: u64) -> Self {
        let limit = if limit > COUNT_MASK { COUNT_MASK } else { limit };
        Self {
            state: AtomicU64::new(0),
            policy,
            limit,
        }
    }

    pub const fn policy(&self) -> ZeroPolicy {
        self.policy
    }

    pub fn count(&self) -> u64 {
        self.state.load(Ordering::Acquire) & COUNT_MASK
    }

    pub fn is_sealed(&self) -> bool {
        self.state.load(Ordering::Acquire) & SEALED != 0
    }

    /// 新增一份 authority。初始未发布的零与终结后的零由封闭位区分。
    pub(crate) fn acquire(&self) -> Result<(), HoldersError> {
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            if current & SEALED != 0 {
                return Err(HoldersError::Sealed);
            }
            if current & COUNT_MASK >= self.limit {
                return Err(HoldersError::ReachLimit);
            }
            match self.state.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    /// 释放一份 authority；返回 true 表示这是 Terminal 对象的最后持有，
    /// 且封闭已在同一原子操作中完成。Remintable 归零恒返回 false。
    pub(crate) fn release(&self) -> bool {
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            let count = current & COUNT_MASK;
            assert!(count > 0, "capability released more times than acquired");
            let last = count == 1 && self.policy == ZeroPolicy::Terminal;
            let next = if last { SEALED } else { current - 1 };
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return last,
                Err(actual) => current = actual,
            }
        }
    }
}
