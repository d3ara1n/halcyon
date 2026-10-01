//! 内嵌可复用的 fence 运输（替代全局 Remote Call 槽表的 TLB 用途）。
//!
//! 设计见 plans/todo-2026-10-01-memory-redesign.md 设计轮次 3：
//! - 每个拥有体（ChangeWork、整空间退休 Work）内嵌 [`Transport`]：每目标 hart
//!   一个槽，随拥有体出生准入，发布不分配、不会 Busy。
//! - 每 hart 一条侵入式收件链（叶锁）。目标在安全点摘槽 → 执行 fence →
//!   归还槽（出链，回到本 owner 保留态）→ 最后一次访问批次状态时 release
//!   ack → 最后者锁外 wake。故“最终 ack = fence 完成 ∧ 槽已归还”。
//! - 批次身份含新 epoch；最后 ack 后目标不再读写旧槽或批次字段。
//! - 零目标不伪造 ack，发布 armed 后直接满足同一完成谓词。

use core::{
    cell::UnsafeCell,
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{hart, maintenance, sync::TicketSpinlock};

const HARTS: usize = hart::HART_NUM_LIMIT;

/// 一次批次的 fence 内容。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fence {
    pub(crate) space_identity: usize,
    pub(crate) translation_epoch: u64,
    pub(crate) instruction_epoch: u64,
}

/// 单目标槽：只在其收件链锁下读写 `next`/`fence`。
struct Slot {
    next: UnsafeCell<*const Slot>,
    fence: UnsafeCell<Fence>,
    owner: UnsafeCell<*const Transport>,
}

/// 内嵌运输：拥有体保证在任一槽在链期间存活（Blocked 期间由维护队列强根持有，
/// 且最后 ack 晚于全部出链）。
pub(crate) struct Transport {
    slots: [Slot; HARTS],
    remaining: AtomicU64,
    /// 最后 ack 方取走并锁外 wake；只在 `remaining` 归零的唯一方读取。
    wake: UnsafeCell<Option<maintenance::WakeToken>>,
}

// SAFETY: 槽字段只在所属 hart 收件链锁下访问；`wake` 只由 arm 方（remaining
// 非零前）与唯一最后 ack 方（remaining 归零的 AcqRel RMW 之后）访问。
unsafe impl Send for Transport {}
unsafe impl Sync for Transport {}

struct Inbox {
    head: *const Slot,
}

// SAFETY: 裸指针只在收件链锁下解引用。
unsafe impl Send for Inbox {}

static INBOXES: [TicketSpinlock<Inbox>; HARTS] = [const {
    TicketSpinlock::new(crate::sync::ranks::REMOTE_CALL, Inbox { head: ptr::null() })
}; HARTS];

impl Transport {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [const {
                Slot {
                    next: UnsafeCell::new(ptr::null()),
                    fence: UnsafeCell::new(Fence {
                        space_identity: 0,
                        translation_epoch: 0,
                        instruction_epoch: 0,
                    }),
                    owner: UnsafeCell::new(ptr::null()),
                }
            }; HARTS],
            remaining: AtomicU64::new(0),
            wake: UnsafeCell::new(None),
        }
    }

    /// 上一批是否全部确认（槽均已归还，可复用）。
    pub(crate) fn idle(&self) -> bool {
        self.remaining.load(Ordering::Acquire) == 0
    }

    /// 发布一批：`token` 为完成后唤醒的稳定根（由 step 内 `maintenance::arm`
    /// 取得）。`mask==0` 立即完成并返回 token 交调用者 wake。PTE store 与 epoch
    /// 发布必须先于本调用。
    pub(crate) fn publish(
        &self,
        mask: u64,
        fence: Fence,
        token: maintenance::WakeToken,
    ) -> Option<maintenance::WakeToken> {
        assert!(self.idle(), "transport reused before final acknowledgement");
        if mask == 0 {
            return Some(token);
        }
        // SAFETY: remaining==0，无目标访问 wake。
        unsafe { *self.wake.get() = Some(token) };
        self.remaining.store(mask, Ordering::Release);
        for target in 0..HARTS {
            if mask & (1 << target) == 0 {
                continue;
            }
            let slot = &self.slots[target];
            let mut inbox = INBOXES[target].lock();
            // SAFETY: 本批前槽已出链（idle），持收件链锁。
            unsafe {
                *slot.fence.get() = fence;
                *slot.owner.get() = self;
                *slot.next.get() = inbox.head;
            }
            inbox.head = slot;
        }
        // SAFETY: 数据 fence 使 PTE、epoch 与槽写入先于 IPI 可见。
        unsafe { core::arch::asm!("fence rw, rw", options(nostack, preserves_flags)) };
        let _ = crate::registry::try_ipi_slots(mask);
        None
    }
}

/// 本 hart 安全点：每次至多处理 `limit` 个槽（链长受 owner 数而非容量约束，
/// 故按批限）。返回处理数。
pub(crate) fn drain_current(limit: usize) -> usize {
    let target = hart::current().slot();
    let mut done = 0;
    while done < limit {
        let slot = {
            let mut inbox = INBOXES[target].lock();
            let head = inbox.head;
            if head.is_null() {
                break;
            }
            // SAFETY: 链上槽的拥有体存活（见 Transport 文档）；持锁。
            inbox.head = unsafe { *(*head).next.get() };
            head
        };
        // SAFETY: 槽已出链（归还到 owner 保留态），fence/owner 只本方在 ack 前读。
        let (fence, owner) = unsafe { (*(*slot).fence.get(), &*(*(*slot).owner.get())) };
        crate::remote_call::synchronize_remote(fence);
        let bit = 1u64 << target;
        // 最后一次访问批次状态：release ack。
        let previous = owner.remaining.fetch_and(!bit, Ordering::AcqRel);
        assert!(previous & bit != 0, "fence target acknowledged twice");
        if previous == bit {
            // SAFETY: 唯一最后 ack 方。
            let token = unsafe { (*owner.wake.get()).take() };
            if let Some(token) = token {
                maintenance::wake(token);
                maintenance::kick();
            }
        }
        done += 1;
    }
    done
}
