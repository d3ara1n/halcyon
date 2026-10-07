//! 已登记线程离场回执：执行 owner 结束后的 Body 成员摘除与 Shell 强持。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

use erhino_shared::call::SystemCallError;
use maintenance::record::Outcome;

use crate::maintenance::{self as mt, Work, WorkRecord};

use super::{super::thread::ThreadControl, members::MemberKey, shell::ProcessShell};

pub(crate) struct ThreadDeparture {
    record: WorkRecord,
    release: DepartureRelease,
}

struct DepartureRelease {
    metadata: Option<super::super::resources::MetadataCharge>,
    shell: Arc<ProcessShell>,
    member: MemberKey,
    done_source: Option<Arc<super::super::wait_source::SignalSource>>,
    completed: AtomicBool,
}

impl Drop for DepartureRelease {
    fn drop(&mut self) {
        drop(self.metadata.take());
        if self.completed.load(Ordering::Acquire) {
            if let Some(source) = &self.done_source {
                source.publish(erhino_shared::object::ObjectSignals::DONE, false);
            }
            // Thread/Departure 私有宿主已退款，最后离场才解除父进程资金屏障。
            self.shell.member_departed();
        }
    }
}

impl ThreadDeparture {
    /// 成员提交前出生；提交后由 Shell 的 member_count 与此强持共同保活。
    pub(crate) fn try_new(
        shell: &Arc<ProcessShell>,
        member: MemberKey,
        control: Option<&Arc<ThreadControl>>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let pool = shell.funding_pool();
        let account = pool.maintenance_account().clone();
        let metadata = super::super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let departure = Arc::try_new(Self {
            record: WorkRecord::new(account),
            release: DepartureRelease {
                metadata: Some(metadata),
                shell: shell.clone(),
                member,
                done_source: control.map(|control| control.done_source()),
                completed: AtomicBool::new(false),
            },
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(departure)
    }

    /// ThreadRelease 的最后字段 Drop 单次转交已提交成员的离场责任。
    pub(crate) fn request(self: &Arc<Self>) {
        let root: Arc<dyn Work> = self.clone();
        mt::publish(root);
        mt::kick();
    }
}

impl Work for ThreadDeparture {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(self: Arc<Self>) -> Outcome {
        assert!(
            !self.release.completed.load(Ordering::Acquire),
            "thread departure completed twice"
        );
        let receipt = self
            .release
            .shell
            .lease_receipt()
            .expect("registered departure lost Body");
        receipt.members.depart(self.release.member);
        drop(receipt);
        self.release.completed.store(true, Ordering::Release);
        Outcome::Done
    }
}
