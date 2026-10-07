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
    arch::asm,
    cell::UnsafeCell,
    ptr,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use crate::{hart, maintenance, sync::Spinlock};

const HARTS: usize = hart::HART_NUM_LIMIT;
/// 单安全点远端槽消费预算；整链验收时按 qT 与真实负载定标。
pub(crate) const SAFE_POINT_BATCH: usize = 16;

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
    tail: *const Slot,
}

// SAFETY: 裸指针只在收件链锁下解引用。
unsafe impl Send for Inbox {}

static INBOXES: [Spinlock<Inbox>; HARTS] = [const {
    Spinlock::new(
        crate::sync::ranks::REMOTE_CALL,
        Inbox {
            head: ptr::null(),
            tail: ptr::null(),
        },
    )
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

    /// 仍待确认的目标；发布者解开业务锁后据此敲门，提前轮询 ack 可省去 IPI。
    pub(crate) fn pending(&self) -> u64 {
        self.remaining.load(Ordering::Acquire)
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
        for (target, inbox) in INBOXES.iter().enumerate() {
            if mask & (1 << target) == 0 {
                continue;
            }
            let slot = &self.slots[target];
            let mut inbox = inbox.lock();
            // SAFETY: 本批前槽已出链（idle），持收件链锁。
            unsafe {
                *slot.fence.get() = fence;
                *slot.owner.get() = self;
                *slot.next.get() = ptr::null();
                if inbox.tail.is_null() {
                    inbox.head = slot;
                } else {
                    *(*inbox.tail).next.get() = slot;
                }
            }
            inbox.tail = slot;
        }
        // SAFETY: 数据 fence 使 PTE、epoch 与槽写入先于 IPI 可见。
        unsafe { core::arch::asm!("fence rw, rw", options(nostack, preserves_flags)) };
        None
    }
}

/// 发布者解开全部业务锁后发送门铃；槽链 Pending 电平是失败时的后备真值。
pub(crate) fn kick(mask: u64) {
    if mask != 0 {
        let _ = crate::runtime_stop::notify_slots(mask);
    }
}

/// idle 的双重检查：有待收槽不能停驻，即使门铃失败。
pub(crate) fn has_current() -> bool {
    !INBOXES[hart::current().slot()].lock().head.is_null()
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
            if inbox.head.is_null() {
                inbox.tail = ptr::null();
            }
            head
        };
        // SAFETY: 槽已出链（归还到 owner 保留态），fence/owner 只本方在 ack 前读。
        let (fence, owner) = unsafe { (*(*slot).fence.get(), &*(*(*slot).owner.get())) };
        synchronize_remote(fence);
        let bit = 1u64 << target;
        // 最后一次访问批次状态：release ack。
        let previous = owner.remaining.fetch_and(!bit, Ordering::AcqRel);
        assert!(previous & bit != 0, "fence target acknowledged twice");
        if previous == bit {
            // SAFETY: 唯一最后 ack 方。
            let token = unsafe { (*owner.wake.get()).take() };
            if let Some(token) = token {
                drop(maintenance::wake(token));
                maintenance::kick();
            }
        }
        done += 1;
    }
    done
}

struct LocalEpoch {
    identity: AtomicUsize,
    translation: AtomicU64,
    instruction: AtomicU64,
}

impl LocalEpoch {
    const fn new() -> Self {
        Self {
            identity: AtomicUsize::new(0),
            translation: AtomicU64::new(0),
            instruction: AtomicU64::new(0),
        }
    }
}

/// 每 hart 最近完成的地址翻译与指令同步代次。它不是 active 集合；只由所属
/// hart 更新，原子字段用于静态数组的 Sync 与 execution gate 的 acquire 复检。
static LOCAL_EPOCHS: [LocalEpoch; HARTS] = [const { LocalEpoch::new() }; HARTS];

/// dispatch 前把本 hart 同步到稳定 AddressSpace 当前代次。若 identity 改变，
/// 当前 ASID=0 下同时执行全量翻译与指令同步。
pub(crate) fn synchronize_local(identity: usize, translation: u64, instruction: u64) {
    if local_observes(identity, translation, instruction) {
        return;
    }
    let local = &LOCAL_EPOCHS[hart::current().slot()];
    let previous_identity = local.identity.load(Ordering::Acquire);
    let previous_instruction = local.instruction.load(Ordering::Acquire);
    fence_local(previous_identity != identity || previous_instruction < instruction);
    publish_local(local, identity, translation, instruction);
}

/// execution gate 内复检本 hart 是否已经达到指定代次。
pub(crate) fn local_observes(identity: usize, translation: u64, instruction: u64) -> bool {
    let local = &LOCAL_EPOCHS[hart::current().slot()];
    local.identity.load(Ordering::Acquire) == identity
        && local.translation.load(Ordering::Acquire) >= translation
        && local.instruction.load(Ordering::Acquire) >= instruction
}

fn fence_local(instruction: bool) {
    // SAFETY: 当前 ASID 恒 0，全量本地失效对任意稳定 address-space identity
    // 都保守正确；不访问内存，也不改变控制流状态。
    unsafe { asm!("sfence.vma", options(nostack, preserves_flags)) };
    if instruction {
        // SAFETY: 本地指令流同步；只在 identity 改变或 instruction epoch 前进时执行。
        unsafe { asm!("fence.i", options(nostack, preserves_flags)) };
    }
}

fn publish_local(local: &LocalEpoch, identity: usize, translation: u64, instruction: u64) {
    let previous_identity = local.identity.load(Ordering::Acquire);
    if previous_identity == identity {
        local.translation.fetch_max(translation, Ordering::Release);
        local.instruction.fetch_max(instruction, Ordering::Release);
    } else {
        local.translation.store(translation, Ordering::Relaxed);
        local.instruction.store(instruction, Ordering::Relaxed);
        local.identity.store(identity, Ordering::Release);
    }
}

/// 内嵌运输目标侧：执行 fence 并记录本 hart 已同步代次（vm::remote 使用）。
pub(crate) fn synchronize_remote(fence: crate::vm::remote::Fence) {
    fence_local(fence.instruction_epoch != 0);
    let local = &LOCAL_EPOCHS[hart::current().slot()];
    publish_local(
        local,
        fence.space_identity,
        fence.translation_epoch,
        fence.instruction_epoch,
    );
}
