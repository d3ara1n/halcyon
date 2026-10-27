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
    /// caller 已通过所附真实 BindingUse 的 RPC 取得 snapshot，任务/记录/输入存储已预付。
    pub fn prepare(
        reference: BindingRef,
        identity: &[u8],
        usage: Arc<MailboxSender>,
        registrar: Option<Arc<MailboxSender>>,
        snapshot: BindingSnapshot,
    ) -> Result<Self, Error> {
        if snapshot.reference != reference
            || snapshot.phase != BindingPhase::ResourcesReady
            || identity.is_empty()
        {
            return Err(Error::Closed);
        }
        Self::new(reference, identity, usage, registrar, Phase::Starting)
    }
    /// Stop 先到时仍持真实 use 观察原轮；后续 Activate 不得恢复业务。
    pub fn cancel(
        reference: BindingRef,
        usage: Arc<MailboxSender>,
        snapshot: BindingSnapshot,
    ) -> Result<Self, Error> {
        if snapshot.reference != reference
            || !matches!(
                snapshot.phase,
                BindingPhase::ResourcesReady | BindingPhase::Stopping
            )
        {
            return Err(Error::Closed);
        }
        Self::new(reference, &[], usage, None, Phase::Stopping)
    }
    fn new(
        reference: BindingRef,
        identity: &[u8],
        usage: Arc<MailboxSender>,
        registrar: Option<Arc<MailboxSender>>,
        phase: Phase,
    ) -> Result<Self, Error> {
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
            phase,
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
    pub fn same_use(&self, usage: &MailboxSender) -> Result<bool, Error> {
        Ok(self.use_identity == EndpointIdentity::from_description(&usage.description()?)?)
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
        if self.identity.is_empty() && reference == self.reference && candidate == self.use_identity
        {
            return Err(Error::Closed);
        }
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
    /// 本地激活工作已结束；硬件停止事实须在此前记录，终态不再追加可观察事实。
    pub fn terminal(&mut self, error: Option<Error>) {
        if self.phase != Phase::Terminal {
            self.phase = Phase::Terminal;
            self.error = error;
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
