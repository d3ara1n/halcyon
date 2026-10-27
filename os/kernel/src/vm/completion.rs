//! 私有执行宿主的最后退款回执；末字段保证先销毁宿主成员，再交付完成。

use alloc::sync::Arc;
use erhino_shared::object::ObjectSignals;

use super::change::VmAccessLease;
use crate::task::{resources::MetadataCharge, wait_source::SignalSource};

pub(super) struct Completion {
    pub(super) publication: Option<crate::resource::mmio::MmioPublication>,
    metadata: Option<MetadataCharge>,
    access: Option<VmAccessLease>,
    source: Arc<SignalSource>,
    internal: bool,
}

impl Completion {
    pub(super) fn new(
        metadata: MetadataCharge,
        access: Option<VmAccessLease>,
        source: Arc<SignalSource>,
        internal: bool,
    ) -> Self {
        Self {
            publication: None,
            metadata: Some(metadata),
            access,
            source,
            internal,
        }
    }

    pub(super) fn payer(&self) -> &Arc<crate::task::memory_pool::MemoryPool> {
        self.metadata
            .as_ref()
            .expect("completion already released")
            .payer()
    }

    pub(super) fn source(&self) -> Arc<SignalSource> {
        self.source.clone()
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        // 私有 Work 的全部先行字段已析构。此仿射回执不强持被等待的 Work；
        // 先退宿主费，再交回访问资格，最后发布独立观察来源的终态。
        drop(self.metadata.take());
        drop(self.access.take());
        // Map 片段可能已经归还，但原操作的发布/回滚责任直到此处才结束。
        // 此时不持 VM/Views 锁；资源门与等待来源均在自己的同步域内完成。
        drop(self.publication.take());
        if self.internal {
            self.source.complete_internal();
        } else {
            self.source.publish(ObjectSignals::DONE, false);
        }
    }
}
