//! 文件宿主私有报告 owner；与 Process DONE 分开保存就绪/业务停止事实。
use super::*;
use crate::supervision::Instance;
use alloc::vec::Vec;
use libkernel::ipc::message::{MessageStorage, ReceiveBuffer};
use srv_fs::startup::Profile;

pub(crate) struct Host {
    pub instance: Instance,
    pub grant: Option<MailboxSender>,
    pub sender_identity: Option<u64>,
    pub mailbox_identity: Option<u64>,
    pub directory: Option<MailboxSender>,
    pub registration_root: Option<MailboxSender>,
    pub grant_b: Option<MailboxSender>,
    pub directory_b: Option<MailboxSender>,
    pub registration_root_b: Option<MailboxSender>,
    pub management: Option<MailboxSender>,
    pub management_b: Option<MailboxSender>,
}
pub(crate) struct Bootstrap {
    pub receiver: Handle,
    pub release: Handle,
    pub pool: Handle,
    pub start_deadline: Deadline,
    pub ready: bool,
    pub stopped: bool,
    context: u64,
    profile: Profile,
    host: Option<Host>,
    roots: u8,
    buffer: ReceiveBuffer,
    storage: Vec<MessageStorage>,
}
impl Bootstrap {
    pub fn new(
        instance: Instance,
        receiver: Handle,
        release: Handle,
        pool: Handle,
        context: u64,
        profile: Profile,
        start_deadline: Deadline,
    ) -> Result<Self, &'static str> {
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(profile.bootstrap_messages())
            .map_err(|_| "filesystem report storage failed")?;
        for _ in 0..profile.bootstrap_messages() {
            storage.push(MessageStorage::new().map_err(|_| "filesystem report storage failed")?);
        }
        Ok(Self {
            receiver,
            release,
            pool,
            start_deadline,
            ready: false,
            stopped: false,
            context,
            profile,
            host: Some(Host {
                instance,
                grant: None,
                sender_identity: None,
                mailbox_identity: None,
                directory: None,
                registration_root: None,
                grant_b: None,
                directory_b: None,
                registration_root_b: None,
                management: None,
                management_b: None,
            }),
            roots: 0,
            buffer: ReceiveBuffer::new().map_err(|_| "filesystem receive buffer failed")?,
            storage,
        })
    }
    pub fn release_roots(&mut self) {
        if let Some(host) = &mut self.host {
            host.release_roots();
        }
    }
    pub fn take_host(&mut self) -> Option<Host> {
        if self.ready { self.host.take() } else { None }
    }
    /// 每轮最多领取一条；返回 true 表示下一轮继续尝试排空原 mailbox。
    pub fn receive_one(
        &mut self,
        pid: u64,
        stopping: bool,
    ) -> Result<bool, erhino_shared::call::SystemCallError> {
        use erhino_shared::call::SystemCallError as E;
        match self.buffer.receive(self.receiver) {
            Err(E::ObjectNotAvailable | E::ObjectBusy) => return Ok(false),
            result => result?,
        }
        let storage = self.storage.pop().ok_or(E::IllegalArgument)?;
        let mut message = storage.take(&mut self.buffer)?;
        if message.header.sender_pid != pid
            || message.header.sender_context_id != self.context
            || !message.payload.is_empty()
        {
            return Err(E::PermissionDenied);
        }
        match message.header.kind {
            libfs::protocol::PROVIDER_READY_KIND => {
                let expected = if self.profile.registry() { 15 } else { 2 };
                if self.ready || self.roots != expected || !message.handles.is_empty() {
                    return Err(E::IllegalArgument);
                }
                self.ready = true;
            }
            libfs::protocol::PROVIDER_STOPPED_KIND => {
                if !stopping || self.stopped || !message.handles.is_empty() {
                    return Err(E::IllegalArgument);
                }
                self.stopped = true;
            }
            kind => {
                let bit = match kind {
                    libfs::protocol::ROOT_GRANT_KIND if self.profile.registry() => 1,
                    libfs::route::MANAGEMENT_GRANT_KIND => 2,
                    service::DIRECTORY_GRANT_KIND if self.profile.registry() => 4,
                    service::AUTHORITY_GRANT_KIND if self.profile.registry() => 8,
                    _ => return Err(E::IllegalArgument),
                };
                if self.ready
                    || self.roots & bit != 0
                    || message.handles.remaining() != self.profile.entries()
                {
                    return Err(E::IllegalArgument);
                }
                let host = self.host.as_mut().ok_or(E::IllegalArgument)?;
                for entry in 0..self.profile.entries() {
                    let capability = message.handles.take(entry)?;
                    let (sender, description) =
                        MailboxSender::from_capability(capability).map_err(|f| f.error)?;
                    let required = Rights::WRITE
                        | Rights::WAIT
                        | Rights::TRANSIT
                        | Rights::DUPLICATE
                        | Rights::GRANT;
                    if description.rights != required {
                        return Err(E::PermissionDenied);
                    }
                    if stopping {
                        drop(sender);
                        continue;
                    }
                    if entry == 0 && bit == 1 {
                        host.sender_identity = Some(description.object_id);
                        host.mailbox_identity = Some(description.related_object_id);
                    }
                    let slot = match (bit, entry) {
                        (1, 0) => &mut host.grant,
                        (1, _) => &mut host.grant_b,
                        (2, 0) => &mut host.management,
                        (2, _) => &mut host.management_b,
                        (4, 0) => &mut host.directory,
                        (4, _) => &mut host.directory_b,
                        (8, 0) => &mut host.registration_root,
                        _ => &mut host.registration_root_b,
                    };
                    *slot = Some(sender);
                }
                self.roots |= bit;
            }
        }
        Ok(true)
    }
}

impl Host {
    pub fn release_roots(&mut self) {
        self.grant = None;
        self.grant_b = None;
        self.management = None;
        self.management_b = None;
        self.directory = None;
        self.directory_b = None;
        self.registration_root = None;
        self.registration_root_b = None;
    }
}

/// 生命周期边界的实际账本样本；不是高水位，也不参与正常停止判据。
pub(crate) fn pool_sample(pool: Handle, phase: &str) {
    match libkernel::memory_pool::query(pool) {
        Ok(snapshot) => libkernel::debug!(
            "FS host Pool sample: phase={}, pages(total/allocated/reserved)={}/{}/{}, metadata(total/allocated/reserved)={}/{}/{}",
            phase,
            snapshot.total,
            snapshot.allocated,
            snapshot.reserved,
            snapshot.metadata.total,
            snapshot.metadata.allocated,
            snapshot.metadata.reserved
        ),
        Err(error) => libkernel::debug!(
            "FS host Pool sample unavailable: phase={}, error={:?}",
            phase,
            error
        ),
    }
}
