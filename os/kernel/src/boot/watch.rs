//! 初始进程的预付终态观察；只有观察来源，不持 capability、Body 或退出租约。
use crate::{
    maintenance::{self as mt, WakeToken, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::{
        memory_pool::MemoryPool,
        resources::MetadataCharge,
        wait_source::{Hint, HintTarget, ObservationSource, WaitNode},
    },
};
use alloc::sync::Arc;
use core::{
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
};
use erhino_shared::{call::SystemCallError, object::ObjectSignals};
pub(super) struct InitialWatch {
    record: WorkRecord,
    source: Arc<dyn ObservationSource>,
    node: WaitNode,
    attached: AtomicBool,
    wake: Spinlock<Option<WakeToken>>,
    _metadata: MetadataCharge,
}
impl InitialWatch {
    pub(super) fn prepare(
        source: Arc<dyn ObservationSource>,
        pool: &Arc<MemoryPool>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        Arc::try_new(Self {
            record: WorkRecord::new(pool.maintenance_account().clone()),
            source,
            node: WaitNode::new(),
            attached: AtomicBool::new(false),
            wake: Spinlock::new(ranks::LEAF, None),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
    /// Bootstrap 唯一 owner 在 ProcessStart 之前调用，不重新分配观察承载。
    pub(super) fn start(self: &Arc<Self>) {
        assert!(
            !self.attached.swap(true, Ordering::Relaxed),
            "initial process observer started twice"
        );
        let target: Arc<dyn HintTarget> = self.clone();
        // SAFETY: Arc 稳定存储，source 强根持有到 detach，只有本观察 owner 操作节点。
        unsafe {
            self.source.attach(
                Pin::new_unchecked(&self.node),
                ObjectSignals::DONE,
                Hint::new(Arc::downgrade(&target), 1),
            );
        }
        let work: Arc<dyn Work> = self.clone();
        mt::publish(work);
        mt::kick();
    }
    fn detach(&self) {
        if self.attached.swap(false, Ordering::Relaxed) {
            // SAFETY: 节点由本对象唯一挂入该来源，source/节点在返回前均存活。
            drop(unsafe { self.source.detach(Pin::new_unchecked(&self.node)) });
        }
    }
}
impl Work for InitialWatch {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        let work: Arc<dyn Work> = self.clone();
        *self.wake.lock() = Some(mt::arm(&work));
        if !self.source.snapshot().signals.contains(ObjectSignals::DONE) {
            return maintenance::record::Outcome::Blocked;
        }
        let wake = self.wake.lock().take();
        drop(wake);
        self.detach();
        log!(Boot, "initial process reached DONE; runtime stop requested");
        crate::runtime_stop::request();
        crate::runtime_stop::check();
        unreachable!("runtime stop returned after initial process completion");
    }
}
impl HintTarget for InitialWatch {
    fn hint(&self, identity: u64) {
        if identity != 1 {
            return;
        }
        let token = self.wake.lock().take();
        if let Some(token) = token {
            drop(mt::wake(token));
            mt::kick();
        }
    }
}
impl Drop for InitialWatch {
    fn drop(&mut self) {
        self.detach();
    }
}
