//! PLIC 短锁唯一仲裁寄存器、in-service 和观察链；提示在锁外交付。
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::{
        memory_pool::MemoryPool,
        resources::MetadataCharge,
        wait_source::{Hint, ObservationSource, Snapshot, Step, WaitNode, WaitQueue},
    },
};
use alloc::sync::Arc;
use core::{cell::UnsafeCell, pin::Pin};
use erhino_shared::{call::SystemCallError as Error, irq::IrqToken, object::ObjectSignals};

// 所选 virt PLIC 的 source 容量；0 是 claim 的空结果。
pub(crate) const SOURCES: usize = crate::board::irq::SOURCES;
const NONE: u32 = u32::MAX;
struct Controller {
    base: usize,
    contexts: [u32; crate::hart::HART_NUM_LIMIT],
    lines: [Option<Arc<Line>>; SOURCES + 1],
}
static CONTROLLER: Spinlock<Option<Controller>> = Spinlock::new(ranks::PLIC, None);
static NEXT_USE: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

impl Controller {
    fn read(&self, offset: usize) -> u32 {
        // SAFETY: board 核定固定 PLIC 寄存器域，正式直映射覆盖；
        // 运行期由全局短锁串行，启动期由尚未启动其他 hart 的唯一 owner 独占。
        unsafe {
            core::arch::asm!("fence iorw, iorw", options(nostack));
            let value = ((self.base + offset) as *const u32).read_volatile();
            core::arch::asm!("fence iorw, iorw", options(nostack));
            value
        }
    }
    fn write(&self, offset: usize, value: u32) {
        // SAFETY: 同 read，固定偏移由已准入 source/context 形成。
        unsafe {
            core::arch::asm!("fence iorw, iorw", options(nostack));
            ((self.base + offset) as *mut u32).write_volatile(value);
            core::arch::asm!("fence iorw, iorw", options(nostack));
        }
    }
    fn enable(&self, source: u32, target: usize, enabled: bool) {
        let offset = 0x2000 + self.contexts[target] as usize * 0x80 + source as usize / 32 * 4;
        let bit = 1 << (source % 32);
        let value = self.read(offset);
        self.write(offset, if enabled { value | bit } else { value & !bit });
    }
    fn complete(&self, source: u32, target: usize) {
        self.write(0x20_0004 + self.contexts[target] as usize * 0x1000, source);
    }
}

/// 单核启动期，正式映射与 registry 已建立，其他 hart 尚未启动。
pub(crate) fn init(platform: &crate::board::irq::Platform) {
    let mut contexts = [NONE; crate::hart::HART_NUM_LIMIT];
    crate::registry::with_registry(|registry| {
        for (slot, record) in registry.records() {
            contexts[slot.0] = platform
                .context(record.hartid)
                .expect("admitted hart has no PLIC context");
        }
    });
    let controller = Controller {
        base: crate::mm::phys_to_virt(platform.base),
        contexts,
        lines: [const { None }; SOURCES + 1],
    };
    let mut guard = CONTROLLER.lock();
    assert!(guard.is_none(), "PLIC initialized twice");
    *guard = Some(controller);
}

pub(crate) fn targets() -> u64 {
    CONTROLLER.lock().as_ref().map_or(0, |controller| {
        controller
            .contexts
            .iter()
            .enumerate()
            .fold(0, |mask, (slot, context)| {
                mask | if *context != NONE { 1 << slot } else { 0 }
            })
    })
}
/// 固件已完成本 hart 的 cold/warm 初始化，stvec 已建立，尚未发布 Online。
/// boot 等全员 Online 才发布 Ready，因而此处没有已经激活的用户 IRQ source。
pub(crate) fn enter_hart() -> Result<(), &'static str> {
    {
        let guard = CONTROLLER.lock();
        let Some(controller) = guard.as_ref() else {
            return Ok(());
        };
        let context = controller.contexts[crate::hart::current().slot()];
        if context == NONE {
            return Err("hart has no admitted supervisor PLIC context");
        }
        // 只接管本 hart 的 S context；HSM warm 初始化可能覆盖启动 hart 的早期跨核写入。
        for word in 0..=SOURCES / 32 {
            let offset = 0x2000 + context as usize * 0x80 + word * 4;
            controller.write(offset, 0);
            if controller.read(offset) != 0 {
                return Err("PLIC supervisor enables did not clear");
            }
        }
        let threshold = 0x20_0000 + context as usize * 0x1000;
        controller.write(threshold, 0);
        if controller.read(threshold) != 0 {
            return Err("PLIC supervisor threshold did not clear");
        }
    }
    crate::csr::write_sie(crate::csr::SIE_SSIE | crate::csr::SIE_STIE | crate::csr::SIE_SEIE);
    Ok(())
}

/// 每次至多一次 claim。返回 true 表示消费了一个源，idle 必须重回工作仲裁。
pub(crate) fn service() -> bool {
    let sip: usize;
    // SAFETY: 本 hart 只读中断 pending。
    unsafe {
        core::arch::asm!("csrr {}, sip", out(reg) sip, options(nomem, nostack));
    }
    if sip & crate::csr::SIE_SEIE == 0 {
        return false;
    }
    let line = {
        let guard = CONTROLLER.lock();
        let Some(controller) = guard.as_ref() else {
            return false;
        };
        let target = crate::hart::current().slot();
        let context = controller.contexts[target];
        if context == NONE {
            return false;
        }
        let source = controller.read(0x20_0004 + context as usize * 0x1000);
        if source == 0 {
            return false;
        }
        assert!(
            source as usize <= SOURCES,
            "PLIC returned an invalid source"
        );
        let line = controller.lines[source as usize]
            .as_ref()
            .expect("PLIC claimed an unowned source");
        assert_eq!(
            line.target, target,
            "PLIC source arrived on another context"
        );
        // SAFETY: 所有 Service 与 WaitQueue 访问均在 CONTROLLER 锁内。
        let state = unsafe { &mut *line.state.get() };
        if state.claim().is_err() {
            controller.complete(source, target);
            state.close();
            controller.enable(source, target, false);
            controller.write(source as usize * 4, 0);
            unsafe {
                // 序号耗尽封使用，不提前表示树责任已经回执。
                line.queue.update(ObjectSignals::CLOSED, false);
            }
        } else {
            unsafe {
                line.queue.update(ObjectSignals::READABLE, false);
            }
        }
        line.clone()
    };
    line.notify();
    true
}

pub(super) struct Line {
    source: u32,
    target: usize,
    state: UnsafeCell<plic::Service>,
    queue: WaitQueue,
    _metadata: MetadataCharge,
}
// SAFETY: state/queue 只由唯一 CONTROLLER 短锁访问；Arc 保持稳定地址。
unsafe impl Send for Line {}
unsafe impl Sync for Line {}
impl Line {
    pub(super) fn generation(&self) -> u64 {
        let _guard = CONTROLLER.lock();
        // SAFETY: 同一控制器锁保护状态。
        unsafe { &*self.state.get() }.generation()
    }
    pub(super) fn new(
        pool: &Arc<MemoryPool>,
        source: u32,
        target: usize,
    ) -> Result<Arc<Self>, Error> {
        if source == 0
            || source as usize > SOURCES
            || target >= crate::hart::HART_NUM_LIMIT
            || targets() & (1 << target) == 0
        {
            return Err(Error::IllegalArgument);
        }
        let generation = NEXT_USE.allocate().ok_or(Error::ReachLimit)?;
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        Arc::try_new(Self {
            source,
            target,
            state: UnsafeCell::new(plic::Service::new(generation)),
            queue: WaitQueue::new(pool.maintenance_account().clone()),
            _metadata: metadata,
        })
        .map_err(|_| Error::OutOfMemory)
    }
    pub(super) fn activate(self: &Arc<Self>) -> Result<(), Error> {
        let mut guard = CONTROLLER.lock();
        let controller = guard.as_mut().ok_or(Error::NotSupported)?;
        // SAFETY: 即使尚未安装到源槽，候选状态也由同一控制器锁保护。
        if unsafe { &*self.state.get() }.closed() {
            return Err(Error::ObjectClosed);
        }
        if controller.lines[self.source as usize].is_some() {
            return Err(Error::ObjectBusy);
        }
        controller.lines[self.source as usize] = Some(self.clone());
        controller.enable(self.source, self.target, true);
        // priority 最后开放；固定 QEMU 11.1.2 的 enable 写本身不重算输出电平。
        controller.write(self.source as usize * 4, 1);
        Ok(())
    }
    pub(super) fn close(self: &Arc<Self>) {
        let removed = {
            let mut guard = CONTROLLER.lock();
            let controller = guard.as_mut().expect("IRQ use without a PLIC");
            // SAFETY: 同一控制器锁封用户操作和硬件入口，包含尚未 activate 的候选。
            let outstanding = unsafe { &mut *self.state.get() }.close();
            let owned = controller.lines[self.source as usize]
                .as_ref()
                .is_some_and(|line| Arc::ptr_eq(line, self));
            if owned {
                if outstanding {
                    controller.complete(self.source, self.target);
                }
                controller.enable(self.source, self.target, false);
                controller.write(self.source as usize * 4, 0);
            }
            unsafe {
                self.queue.update(ObjectSignals::CLOSED, false);
            }
            if owned {
                controller.lines[self.source as usize].take()
            } else {
                None
            }
        };
        drop(removed);
        self.notify();
    }
    /// 树已经摘除责任和父 source 占用后，才关闭观察并发布 DONE。
    pub(super) fn complete(self: &Arc<Self>) {
        {
            let _guard = CONTROLLER.lock();
            // SAFETY: 稳定来源的同一锁；硬件关闭已经完成，不会再发布 READABLE。
            unsafe {
                assert!(
                    (&*self.state.get()).closed(),
                    "IRQ completion before hardware close"
                );
                self.queue
                    .update(ObjectSignals::DONE | ObjectSignals::CLOSED, true);
            }
        }
        self.notify();
    }
    fn notify(self: &Arc<Self>) {
        let work: Arc<dyn Work> = self.clone();
        mt::publish(work);
        mt::kick();
    }
    pub(super) fn take(self: &Arc<Self>) -> Result<IrqToken, Error> {
        let token = {
            let _guard = CONTROLLER.lock();
            // SAFETY: 状态与来源链受同一锁保护，Take 只消费提示，不完成硬件领取。
            let token = unsafe { &mut *self.state.get() }.take()?;
            unsafe {
                self.queue.update(ObjectSignals::NONE, false);
            }
            token
        };
        self.notify();
        Ok(token)
    }
    pub(super) fn ack(&self, token: IrqToken) -> Result<(), Error> {
        let guard = CONTROLLER.lock();
        let controller = guard.as_ref().ok_or(Error::ObjectClosed)?;
        // SAFETY: 核对 token 与 complete 同门，旧代次/重复确认不能触碰新 source。
        unsafe { &mut *self.state.get() }.ack(token)?;
        controller.complete(self.source, self.target);
        Ok(())
    }
}
impl ObservationSource for Line {
    fn snapshot(&self) -> Snapshot {
        let _guard = CONTROLLER.lock();
        // SAFETY: 唯一来源锁。
        unsafe { self.queue.snapshot() }
    }
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _guard = CONTROLLER.lock();
        // SAFETY: caller 保持 pin/寿命直到 detach，来源同门串行。
        unsafe {
            self.queue.attach(node, interest, hint);
        }
    }
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _guard = CONTROLLER.lock();
        // SAFETY: caller 保证节点来自本来源，返回后才释放。
        unsafe { self.queue.detach(node) }
    }
}
impl Work for Line {
    fn record(&self) -> &WorkRecord {
        self.queue.record()
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        let next = {
            let _guard = CONTROLLER.lock();
            // SAFETY: 同来源锁，每步至多一个订阅凭据。
            unsafe { self.queue.step() }
        };
        match next {
            Step::Idle => maintenance::record::Outcome::Idle,
            Step::Progress => maintenance::record::Outcome::Ready,
            Step::Hint(delivery) => {
                delivery.send();
                maintenance::record::Outcome::Ready
            }
        }
    }
}
