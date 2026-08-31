//! Hart 间短动作运输：固定请求槽、IPI 门铃与完成确认。
//!
//! Pending 槽是工作真值，IPI 只负责提示目标检查。业务事务负责决定目标集合、
//! 持有旧资源并实现 [`Completion`]；本模块只保证请求发布后不可取消、目标动作
//! 完成后才确认，以及槽在确认后才可复用。

use alloc::sync::Arc;
use core::{
    arch::asm,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use crate::{hart, registry, sync::Spinlock};

const HARTS: usize = hart::HART_NUM_LIMIT;
const SLOTS_PER_HART: usize = 4;
const VALID_HART_MASK: u64 = (1u64 << HARTS) - 1;
const MAX_DRAIN_PER_SAFE_POINT: usize = SLOTS_PER_HART;

type Table = remote_call::RemoteCalls<Call, HARTS, SLOTS_PER_HART>;

static CALLS: Spinlock<Table> = Spinlock::new(crate::sync::ranks::REMOTE_CALL, Table::new());

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

/// 全部目标确认后由最后一个目标调用。实现必须保持有界，并自行把较长工作转入
/// 已有异步状态机；调用发生时不持 Remote Call 锁。
pub(crate) trait Completion: Send + Sync {
    fn complete(self: Arc<Self>);
}

/// 地址翻译失效请求。当前 ASID 恒 0，第一版执行全量本地 fence；范围与稳定
/// identity 随请求保留，供 AddressSpace 完成核验和未来范围优化使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FenceRequest {
    pub space_identity: usize,
    pub translation_epoch: u64,
    pub instruction_epoch: u64,
    pub start_vpn: usize,
    pub page_count: usize,
    records_epoch: bool,
}

impl FenceRequest {
    pub fn new(
        space_identity: usize,
        translation_epoch: u64,
        instruction_epoch: u64,
        start_vpn: usize,
        page_count: usize,
    ) -> Self {
        assert!(
            space_identity != 0,
            "remote-call address-space identity must be nonzero"
        );
        assert!(
            translation_epoch != 0,
            "remote-call translation epoch must be nonzero"
        );
        assert!(page_count != 0, "remote-call fence range must be nonempty");
        Self {
            space_identity,
            translation_epoch,
            instruction_epoch,
            start_vpn,
            page_count,
            records_epoch: true,
        }
    }

    fn selftest() -> Self {
        Self {
            space_identity: usize::MAX,
            translation_epoch: 1,
            instruction_epoch: 1,
            start_vpn: 0,
            page_count: 1,
            records_epoch: false,
        }
    }
}

struct BatchCompletion {
    remaining: AtomicU64,
    sink: Arc<dyn Completion>,
}

impl BatchCompletion {
    fn acknowledge(&self, target: usize) -> bool {
        let bit = 1u64 << target;
        let previous = self.remaining.fetch_and(!bit, Ordering::AcqRel);
        assert!(previous & bit != 0, "remote-call target acknowledged twice");
        // 最后一个 AcqRel RMW acquire 前序目标组成的 release sequence。
        previous == bit
    }
}

struct Call {
    request: FenceRequest,
    completion: Arc<BatchCompletion>,
}

/// Commit 前对完整目标集合的 affine reservation。Drop 只可能发生在 Publish
/// 前，并精确归还已经取得的槽。
pub(crate) struct ReservedBatch {
    target_mask: u64,
    slots: [Option<remote_call::Reservation>; HARTS],
    completion: Arc<BatchCompletion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReserveError {
    EmptyTargets,
    InvalidTargets,
    Busy,
    AllocationFailed,
}

struct SelfTestCompletion {
    targets: u32,
}

impl Completion for SelfTestCompletion {
    fn complete(self: Arc<Self>) {
        log!(
            Remote,
            "self-test passed: {} hart fence request(s) acknowledged",
            self.targets
        );
    }
}

/// 启动期异步真机探针：不等待、不占调度流；全部 admitted hart 的安全点确认
/// 后由最后一方打印完成锚点。
pub(crate) fn selftest() {
    let target_mask = registry::admitted_mask();
    let completion: Arc<dyn Completion> = Arc::try_new(SelfTestCompletion {
        targets: target_mask.count_ones(),
    })
    .expect("remote-call self-test completion allocation failed");
    let batch = reserve(target_mask, completion).expect("remote-call self-test reservation failed");
    let request = FenceRequest::selftest();
    batch.publish(request).ring();
}

/// 在业务 Commit 前预留全部目标槽和完成记录。失败保持请求表零发布副作用。
pub(crate) fn reserve(
    target_mask: u64,
    sink: Arc<dyn Completion>,
) -> Result<ReservedBatch, ReserveError> {
    if target_mask == 0 {
        return Err(ReserveError::EmptyTargets);
    }
    let admitted = registry::admitted_mask();
    if target_mask & !VALID_HART_MASK != 0 || target_mask & !admitted != 0 {
        return Err(ReserveError::InvalidTargets);
    }

    let completion = Arc::try_new(BatchCompletion {
        remaining: AtomicU64::new(target_mask),
        sink,
    })
    .map_err(|_| ReserveError::AllocationFailed)?;
    let mut slots: [Option<remote_call::Reservation>; HARTS] = [const { None }; HARTS];
    let mut calls = CALLS.lock();
    for (target, slot) in slots.iter_mut().enumerate() {
        if target_mask & (1u64 << target) == 0 {
            continue;
        }
        match calls.reserve(target) {
            Ok(reservation) => *slot = Some(reservation),
            Err(_) => {
                for reservation in slots.iter_mut().filter_map(Option::take) {
                    assert!(
                        calls.cancel(reservation),
                        "reserved remote-call slot must cancel"
                    );
                }
                return Err(ReserveError::Busy);
            }
        }
    }
    drop(calls);

    Ok(ReservedBatch {
        target_mask,
        slots,
        completion,
    })
}

impl ReservedBatch {
    /// Commit 内发布全部请求。此路径不分配、不可失败，只返回锁外门铃权；
    /// 请求从此归远端与完成对象共同所有，Doorbell 消散也不会撤销 Pending。
    pub fn publish(mut self, request: FenceRequest) -> Doorbell {
        let mut calls = CALLS.lock();
        for reservation in self.slots.iter_mut().filter_map(Option::take) {
            let call = Call {
                request,
                completion: Arc::clone(&self.completion),
            };
            calls
                .publish(reservation, call)
                .unwrap_or_else(|_| panic!("reserved remote-call slot must publish"));
        }
        drop(calls);
        Doorbell {
            target_mask: self.target_mask,
        }
    }
}

/// Pending 发布后的锁外门铃权。业务锁释放后调用 [`Self::ring`]；即使 token
/// 被意外消散，请求仍由后续 trap/scheduler 安全点按 Pending 电平补消费。
#[must_use = "published Remote Calls should ring their targets after releasing business locks"]
pub(crate) struct Doorbell {
    target_mask: u64,
}

impl Doorbell {
    pub fn ring(self) {
        // SAFETY: data fence 只约束本 hart 的既有内存访问；Publish 前的 PTE、
        // epoch 与槽写入必须先于随后 IPI 对目标可见。
        unsafe { asm!("fence rw, rw", options(nostack, preserves_flags)) };
        let failed = registry::try_ipi_slots(self.target_mask);
        if failed != 0 {
            warn!(
                Remote,
                "IPI doorbell failed for hart slot mask {failed:#x}; requests remain pending"
            );
        }
    }
}

impl Drop for ReservedBatch {
    fn drop(&mut self) {
        if self.slots.iter().all(Option::is_none) {
            return;
        }
        let mut calls = CALLS.lock();
        for reservation in self.slots.iter_mut().filter_map(Option::take) {
            assert!(
                calls.cancel(reservation),
                "reserved remote-call slot must roll back"
            );
        }
    }
}

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

/// 在本 hart 的 trap/scheduler 安全点执行固定数量请求。即使生产者持续发布，
/// 单次调用也不会超过每 hart 槽数。
pub(crate) fn drain_current() -> usize {
    let target = hart::current().slot();
    let mut completed = 0;
    while completed < MAX_DRAIN_PER_SAFE_POINT {
        let Some(taken) = CALLS.lock().take(target) else {
            break;
        };
        let (token, call) = taken.into_parts();
        execute_fence(call.request);
        let completes_batch = call.completion.acknowledge(target);
        assert!(
            CALLS.lock().finish(token),
            "taken remote-call slot must finish"
        );
        if completes_batch {
            call.completion.sink.clone().complete();
        }
        completed += 1;
    }
    completed
}

fn execute_fence(request: FenceRequest) {
    let _ = (request.start_vpn, request.page_count);
    fence_local(request.instruction_epoch != 0);
    if request.records_epoch {
        let local = &LOCAL_EPOCHS[hart::current().slot()];
        publish_local(
            local,
            request.space_identity,
            request.translation_epoch,
            request.instruction_epoch,
        );
    }
}
