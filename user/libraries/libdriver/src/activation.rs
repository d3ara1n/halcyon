//! 一次真实绑定的驱动激活事实；不保存设备占用，也不执行硬件或创建运行体。
use crate::protocol::{Phase, Snapshot};
use alloc::{sync::Arc, vec::Vec};
use libdevice::{
    Error,
    binding::{Phase as BindingPhase, Snapshot as BindingSnapshot},
    identity::BindingRef,
    registry::EndpointIdentity,
};
use libkernel::ipc::message::MailboxSender;

/// 逐轮 IO 准入与执行身份；退场后不再提供任何可唤醒任务。
#[cfg(any(target_arch = "riscv64", test))]
pub(crate) enum Lifecycle {
    Pending,
    Preparing(u64),
    Active(u64),
    Retired,
}
#[cfg(any(target_arch = "riscv64", test))]
impl Lifecycle {
    pub(crate) fn task(&self) -> Option<u64> {
        match *self {
            Self::Preparing(task) | Self::Active(task) => Some(task),
            Self::Pending | Self::Retired => None,
        }
    }
    pub(crate) fn attach(&mut self, task: u64) {
        assert_ne!(task, 0, "activation lifecycle identity missing");
        match *self {
            Self::Pending => *self = Self::Preparing(task),
            Self::Preparing(old) | Self::Active(old) => {
                assert_eq!(old, task, "activation lifecycle changed");
            }
            Self::Retired => panic!("retired activation lifecycle resumed"),
        }
    }
    pub(crate) fn allow_io(&mut self) -> Result<(), Error> {
        match *self {
            Self::Preparing(task) => *self = Self::Active(task),
            Self::Active(_) => {}
            Self::Pending => return Err(Error::NotReady),
            Self::Retired => return Err(Error::Closed),
        }
        Ok(())
    }
    pub(crate) fn io_ready(&self) -> bool {
        matches!(self, Self::Active(_))
    }
    pub(crate) fn retire(&mut self) {
        *self = Self::Retired;
    }
    pub(crate) fn retired(&self) -> bool {
        matches!(self, Self::Retired)
    }
}

pub struct Activation {
    reference: BindingRef,
    usage: Option<Arc<MailboxSender>>,
    registrar: Option<Arc<MailboxSender>>,
    identity: Vec<u8>,
    use_identity: EndpointIdentity,
    registrar_identity: Option<EndpointIdentity>,
    phase: Phase,
    error: Option<Error>,
    consumer_stopped: bool,
    sequence: u64,
}

impl Activation {
    /// 本地接纳冻结输入；独立绑定观察完成核验以前，宿主不得据 Starting 开始 IO。
    pub fn prepare(
        reference: BindingRef,
        identity: &[u8],
        usage: Arc<MailboxSender>,
        registrar: Option<Arc<MailboxSender>>,
    ) -> Result<Self, Error> {
        if identity.is_empty() {
            return Err(Error::Invalid);
        }
        let use_identity = EndpointIdentity::from_description(&usage.description()?)?;
        let registrar_identity = registrar
            .as_ref()
            .map(|entry| {
                entry
                    .description()
                    .map_err(Error::from)
                    .and_then(|description| EndpointIdentity::from_description(&description))
            })
            .transpose()?;
        let mut frozen = Vec::new();
        frozen
            .try_reserve_exact(identity.len())
            .map_err(|_| Error::Resource)?;
        frozen.extend_from_slice(identity);
        Ok(Self {
            reference,
            usage: Some(usage),
            registrar,
            identity: frozen,
            use_identity,
            registrar_identity,
            phase: Phase::Starting,
            error: None,
            consumer_stopped: false,
            sequence: 1,
        })
    }
    pub fn reference(&self) -> BindingRef {
        self.reference
    }
    pub fn usage(&self) -> Result<&Arc<MailboxSender>, Error> {
        self.usage.as_ref().ok_or(Error::Closed)
    }
    pub fn retire_access(&mut self, snapshot: BindingSnapshot) -> Result<(), Error> {
        if snapshot.reference != self.reference
            || snapshot.phase != BindingPhase::Released
            || self.phase != Phase::Terminal
        {
            return Err(Error::NotReady);
        }
        self.usage = None;
        self.registrar = None;
        Ok(())
    }
    /// 仅供逐轮 Preparing owner：没有获得 IO 许可，且所有核验/观察运输已脱离。
    #[cfg(target_arch = "riscv64")]
    pub(crate) fn retire_unstarted_access(&mut self) {
        assert_eq!(
            self.phase,
            Phase::Terminal,
            "unstarted activation is not terminal"
        );
        self.usage = None;
        self.registrar = None;
    }
    pub fn registrar(&self) -> Option<&Arc<MailboxSender>> {
        self.registrar.as_ref()
    }
    pub fn same_input(
        &self,
        reference: BindingRef,
        identity: &[u8],
        usage: &MailboxSender,
        registrar: Option<&MailboxSender>,
    ) -> Result<(), Error> {
        let candidate = EndpointIdentity::from_description(&usage.description()?)?;
        let registrar = registrar
            .map(|entry| {
                entry
                    .description()
                    .map_err(Error::from)
                    .and_then(|description| EndpointIdentity::from_description(&description))
            })
            .transpose()?;
        if reference != self.reference
            || identity != self.identity
            || candidate != self.use_identity
            || registrar != self.registrar_identity
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            reference: self.reference,
            sequence: self.sequence,
            pending: false,
            phase: self.phase,
            error: self.error,
            consumer_stopped: self.consumer_stopped,
        }
    }
    fn changed(&mut self) {
        if self.sequence == u64::MAX {
            return;
        }
        self.sequence += 1;
        if self.sequence == u64::MAX {
            self.phase = Phase::Terminal;
            self.error.get_or_insert(Error::Exhausted);
        }
    }
    pub fn ready(&mut self) -> Result<(), Error> {
        match self.phase {
            Phase::Starting => {
                self.phase = Phase::Ready;
                self.changed();
                if self.phase == Phase::Ready {
                    Ok(())
                } else {
                    Err(Error::Exhausted)
                }
            }
            Phase::Ready => Ok(()),
            Phase::Stopping | Phase::Terminal => Err(Error::Closed),
        }
    }
    pub fn stop(&mut self) {
        if matches!(self.phase, Phase::Starting | Phase::Ready) {
            self.phase = Phase::Stopping;
            self.changed();
        }
    }
    /// 自主失败立即传播停止及首错；后续硬件清理不能阻挡管理者提交 Binding Stop。
    pub fn fail(&mut self, error: Error) {
        if self.phase == Phase::Terminal {
            return;
        }
        self.stop();
        if self.error.is_none() {
            self.error = Some(error);
            self.changed();
        }
    }
    /// 本地激活工作已结束；硬件停止事实须在此前记录，终态不再追加可观察事实。
    pub fn terminal(&mut self, error: Option<Error>) {
        if self.phase != Phase::Terminal {
            self.phase = Phase::Terminal;
            self.error = self.error.or(error);
            self.changed();
        }
    }
    /// # Safety
    /// 原轮已永久禁止再次提交消费者访问，所有实际设备/配置访问与在途责任已结束；
    /// 父 provider 仍须自行收束其准备效果和资源。Terminal/进程 DONE 不满足此前提。
    pub unsafe fn consumer_stopped(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Stopping {
            return Err(Error::NotReady);
        }
        if !self.consumer_stopped {
            if self.sequence == u64::MAX {
                return Err(Error::Exhausted);
            }
            self.consumer_stopped = true;
            self.changed();
        }
        Ok(())
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn terminal_receipt_has_no_execution_identity_or_io_permission() {
        let mut lifecycle = Lifecycle::Pending;
        assert_eq!(lifecycle.allow_io(), Err(Error::NotReady));
        lifecycle.attach(7);
        assert_eq!(lifecycle.task(), Some(7));
        lifecycle.allow_io().unwrap();
        assert!(lifecycle.io_ready());
        lifecycle.retire();
        assert_eq!(lifecycle.task(), None);
        assert!(!lifecycle.io_ready());
        assert!(lifecycle.retired());
        assert_eq!(lifecycle.allow_io(), Err(Error::Closed));
    }

    #[test]
    fn rejected_preparation_cannot_be_reopened_by_a_late_completion() {
        let mut lifecycle = Lifecycle::Pending;
        lifecycle.attach(9);
        assert!(!lifecycle.io_ready());
        lifecycle.retire();
        assert_eq!(lifecycle.allow_io(), Err(Error::Closed));
        assert_eq!(lifecycle.task(), None);
    }
}
