//! 设备管理者的真实启动与停止装配；init 保留根管理、独立 Job/Pool 及进程监督。
use super::{
    JOB_FULL_RIGHTS, RootSupervisor, SUPERVISOR_RIGHTS, TopologyNames, init_process_info,
    root_memory_pool,
};
use alloc::vec::Vec;
use erhino_shared::proc::HandleGrant;
use erhino_shared::{
    memory_pool::{METADATA_SLOT_BYTES, MemoryPoolBudget},
    object::{Handle, ObjectSignals, Rights},
    proc::{
        PROCESS_MAIN_STACK_SIZE, PROCESS_PAGE_SIZE, ProcessState, TerminationCause,
        TerminationDetail,
    },
};
use libdevice::startup::{self, Delegation};
use libdevice::{
    Error,
    protocol::{self, Command, Header},
    registry::{Access, Scope},
};
use libkernel::{
    debug,
    ipc::{message::MailboxSender, object, wait},
    process,
};
use libprocess::{GrantOutcome, SpawnRequest};
use librpc::Exchange;

const DEADLINE_MS: u64 = 5_000;
const POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw() | Rights::CREATE.raw() | Rights::DUPLICATE.raw() | Rights::GRANT.raw(),
);
pub(super) struct Manager {
    pid: u64,
    job: Handle,
    pool: Handle,
    root: Option<MailboxSender>,
    observer: Option<MailboxSender>,
    observer_admin: Option<MailboxSender>,
    watch: Option<libdevice::client::Watch>,
}
pub(super) fn call(
    root: &mut dyn Exchange,
    sender: &MailboxSender,
    command: Command<'_>,
) -> Result<librpc::exchange::Reply, &'static str> {
    let deadline = libkernel::time::timeout_millis(DEADLINE_MS)
        .map_err(|_| "device request deadline failed")?;
    call_until(root, sender, command, deadline)
}
pub(super) fn call_until(
    root: &mut dyn Exchange,
    sender: &MailboxSender,
    command: Command<'_>,
    deadline: erhino_shared::time::Deadline,
) -> Result<librpc::exchange::Reply, &'static str> {
    let mut bytes = [0; protocol::HEADER_LEN + protocol::BODY_MAX];
    let used = command
        .encode(deadline, &mut bytes)
        .map_err(|_| "device request encoding failed")?;
    let request = librpc::Request::new(protocol::ID, &bytes[..used])
        .map_err(|_| "device request preparation failed")?;
    root.call(sender, deadline, request).map_err(|error| {
        debug!(
            "device management RPC failed: phase={:?}, cause={:?}",
            error.phase, error.cause
        );
        "device management RPC unavailable"
    })
}
fn empty(
    reply: &librpc::exchange::Reply,
    pid: u64,
    op: u16,
    status: Option<Error>,
) -> Result<(), &'static str> {
    let (header, body) =
        Header::decode(&reply.payload).map_err(|_| "device management reply invalid")?;
    if reply.sender_pid != pid
        || header.op != op
        || header.error != status
        || !body.is_empty()
        || reply.handles.remaining() != 0
    {
        return Err("device management reply mismatch");
    }
    Ok(())
}
fn sender(
    cap: libkernel::ipc::capability::Capability,
    mailbox: u64,
) -> Result<MailboxSender, &'static str> {
    let (sender, description) =
        MailboxSender::from_capability(cap).map_err(|_| "device entry has invalid role")?;
    if description.related_object_id != mailbox
        || !description.rights.contains(
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
        )
    {
        return Err("device entry authority mismatch");
    }
    Ok(sender)
}
impl Manager {
    pub(super) fn pid(&self) -> u64 {
        self.pid
    }
    pub(super) fn issue_driver_source(
        &self,
        root: &mut RootSupervisor,
    ) -> Result<(MailboxSender, MailboxSender), &'static str> {
        self.issue_source(
            root,
            Scope::Root,
            Access::OBSERVE
                .union(Access::CLAIM)
                .union(Access::CHILD_REGISTER)
                .union(Access::REPORT_STOPPED),
        )
    }
    #[cfg(feature = "acceptance-device")]
    pub(super) fn issue_function_source(
        &self,
        root: &mut RootSupervisor,
    ) -> Result<(MailboxSender, MailboxSender), &'static str> {
        self.issue_source(root, Scope::Root, Access::OBSERVE.union(Access::FUNCTIONS))
    }
    #[cfg(feature = "acceptance-device-function-session")]
    pub(super) fn controller_source(
        &self,
        root: &mut RootSupervisor,
        id: libdevice::identity::DeviceId,
    ) -> Result<(MailboxSender, MailboxSender), &'static str> {
        self.issue_source(
            root,
            Scope::Subtree(id),
            Access::OBSERVE
                .union(Access::CLAIM)
                .union(Access::REPORT_STOPPED),
        )
    }
    #[cfg(feature = "acceptance-device-function-session")]
    pub(super) fn observer(&self) -> Result<&MailboxSender, &'static str> {
        self.observer.as_ref().ok_or("device observer closed")
    }
    fn issue_source(
        &self,
        root: &mut RootSupervisor,
        scope: Scope,
        access: Access,
    ) -> Result<(MailboxSender, MailboxSender), &'static str> {
        let result = (|| {
            let mut reply = call(
                root,
                self.root.as_ref().ok_or("device root entry closed")?,
                Command::IssueSource { scope, access },
            )?;
            let (header, body) =
                Header::decode(&reply.payload).map_err(|_| "driver source reply invalid")?;
            if reply.sender_pid != self.pid
                || header.op != 1
                || header.error.is_some()
                || !body.is_empty()
                || reply.handles.remaining() != 2
            {
                return Err("driver source issuance failed");
            }
            let mailbox = self
                .root
                .as_ref()
                .ok_or("device root entry closed")?
                .description()
                .map_err(|_| "device registry identity failed")?
                .related_object_id;
            let claim = sender(
                reply
                    .handles
                    .take(0)
                    .map_err(|_| "driver ClaimSource missing")?,
                mailbox,
            )?;
            let admin = sender(
                reply
                    .handles
                    .take(1)
                    .map_err(|_| "driver SourceAdmin missing")?,
                mailbox,
            )?;
            Ok((claim, admin))
        })();
        if result.is_err() {
            // 非幂等签发 Unknown 封原根，不以另发来源掩盖可能已提交的权利。
            if let Ok(end) = libkernel::time::timeout_millis(DEADLINE_MS) {
                let _ = root.manager_source_call(
                    super::device_supervision::Manager::Device,
                    Command::SealSource,
                    end,
                );
            }
        }
        result
    }
    pub fn launch(
        root: &mut RootSupervisor,
        services: Handle,
        names: &mut TopologyNames,
        image: &[u8],
    ) -> Result<Self, &'static str> {
        // 公共 ABI 的完整主栈之外，再留 8 MiB 覆盖映像、2 MiB 领域记录、执行/控制存储及 allocator。
        let pool = root
            .derive_pool(
                root_memory_pool(),
                MemoryPoolBudget {
                    pages: ((PROCESS_MAIN_STACK_SIZE + 8 * 1024 * 1024) / PROCESS_PAGE_SIZE) as u64,
                    metadata: 1024 * 1024 / METADATA_SLOT_BYTES,
                },
                POOL_RIGHTS,
            )
            .map_err(|_| "device Pool admission failed")?;
        let job = root
            .create_job(services, pool, JOB_FULL_RIGHTS)
            .map_err(|_| "device management Job failed")?;
        names.register_job(job, "device_management");
        let bootstrap = root
            .create_mailbox(
                Rights::READ | Rights::WAIT | Rights::MANAGE,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT,
            )
            .map_err(|_| "device bootstrap mailbox failed")?;
        let expected = object::query(bootstrap.peer)
            .map_err(|_| "device bootstrap identity query failed")?
            .object_id;
        let pool_grant = root
            .duplicate(pool, Rights::CREATE | Rights::GRANT)
            .map_err(|_| "device Pool grant failed")?;
        root.reserve_process()
            .map_err(|_| "device supervision admission failed")?;
        let (description, platform_roots) = {
            let inputs = super::INIT_STARTUP.lock();
            let platform = inputs
                .as_ref()
                .ok_or("init platform input missing")?
                ._platform
                .platform();
            let mut handles = Vec::new();
            handles
                .try_reserve_exact(platform.controls().len())
                .map_err(|_| "platform delegation storage failed")?;
            handles.extend(platform.controls().iter().map(|control| control.handle()));
            (platform.description_object().handle(), handles)
        };
        let mut grants = Vec::new();
        grants
            .try_reserve_exact(startup::ROOT_BASE + platform_roots.len())
            .map_err(|_| "device grants storage failed")?;
        grants.push(HandleGrant {
            handle: bootstrap.peer,
            rights: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        });
        grants.push(HandleGrant {
            handle: pool_grant,
            rights: Rights::CREATE,
        });
        grants.push(HandleGrant {
            handle: root
                .duplicate(description, libpci::platform::DESCRIPTION_RIGHTS)
                .map_err(|_| "device platform description grant failed")?,
            rights: libpci::platform::DESCRIPTION_RIGHTS,
        });
        for handle in platform_roots {
            grants.push(HandleGrant {
                handle: root
                    .duplicate(handle, libpci::platform::ROOT_RIGHTS)
                    .map_err(|_| "device platform root grant failed")?,
                rights: libpci::platform::ROOT_RIGHTS,
            });
        }
        let payload = Delegation::encode(grants.len() - startup::ROOT_BASE)
            .map_err(|_| "device platform configuration invalid")?;
        let prepared = root.prepare_process(SpawnRequest {
            job,
            parent: init_process_info(),
            memory_pool: pool,
            image,
            payload: &payload,
            grants: &grants,
            control_rights: SUPERVISOR_RIGHTS,
        });
        if prepared.is_ok()
            || matches!(&prepared,Err(failure) if failure.grants==GrantOutcome::Consumed)
        {
            for grant in &grants {
                root.transferred(grant.handle);
            }
        }
        let prepared = prepared.map_err(|failure| {
            debug!(
                "device manager spawn failed: error={:?}, grants={:?}, cleanup={:?}",
                failure.error, failure.grants, failure.cleanup_error
            );
            "device manager spawn failed"
        })?;
        let spawned = root.start_manager(super::device_supervision::Manager::Device, prepared)?;
        names.register_process(spawned.pid, "srv_device");
        let deadline = libkernel::time::timeout_millis(DEADLINE_MS)
            .map_err(|_| "device startup deadline failed")?;
        let mut ready = root
            .receive_until(bootstrap.owner, deadline)
            .map_err(|_| "device manager readiness failed")?;
        if ready.header.kind != protocol::BOOT_READY
            || ready.header.sender_context_id != expected
            || ready.header.sender_pid != spawned.pid
            || ready.payload.len() != 8
            || ready.handles.remaining() != 2
        {
            return Err("device manager readiness identity mismatch");
        }
        let incarnation = u64::from_le_bytes(
            ready.payload[..8]
                .try_into()
                .map_err(|_| "device registry identity invalid")?,
        );
        if incarnation == 0 {
            return Err("device registry identity is zero");
        }
        let root_entry = sender(
            ready
                .handles
                .take(0)
                .map_err(|_| "device root entry missing")?,
            incarnation,
        )?;
        let admin = sender(
            ready
                .handles
                .take(1)
                .map_err(|_| "device root administration missing")?,
            incarnation,
        )?;
        if root_entry
            .description()
            .map_err(|_| "device root identity failed")?
            .object_id
            == admin
                .description()
                .map_err(|_| "device administration identity failed")?
                .object_id
        {
            return Err("device root roles share one context");
        }
        root.attach_manager_source(
            super::device_supervision::Manager::Device,
            admin,
            spawned.pid,
        )?;
        let state =
            process::query(spawned.control).map_err(|_| "device process readiness query failed")?;
        if state.state != ProcessState::Running as u32
            || state.cause != TerminationCause::None as u32
            || state.detail != TerminationDetail::NONE
        {
            return Err("device manager terminated before readiness");
        }
        drop(ready);
        root.close_control(bootstrap.owner)?;
        // 根观察角色是正式装配出口；签发 Unknown 时封旧根，不另发行第二份替代来源。
        let observer_result = (|| {
            let mut reply = call(
                root,
                &root_entry,
                Command::IssueSource {
                    scope: Scope::Root,
                    access: Access::OBSERVE,
                },
            )?;
            let (header, body) = Header::decode(&reply.payload)
                .map_err(|_| "device observer issuance reply invalid")?;
            if reply.sender_pid != spawned.pid
                || header.op != 1
                || header.error.is_some()
                || !body.is_empty()
                || reply.handles.remaining() != 2
            {
                return Err("device observer issuance failed");
            }
            let observer = sender(
                reply
                    .handles
                    .take(0)
                    .map_err(|_| "device observer entry missing")?,
                incarnation,
            )?;
            let observer_admin = sender(
                reply
                    .handles
                    .take(1)
                    .map_err(|_| "device observer administration missing")?,
                incarnation,
            )?;
            let denied = call(
                root,
                &observer,
                Command::IssueSource {
                    scope: Scope::Root,
                    access: Access::OBSERVE,
                },
            )?;
            empty(&denied, spawned.pid, 1, Some(Error::Permission))?;
            let watch = libdevice::client::Watch::from_reply(
                call(root, &observer, Command::WatchScope { scope: Scope::Root })?,
                incarnation,
                15,
            )
            .map_err(|_| "device Watch adoption failed")?;
            let rejected = call(
                root,
                watch.control(),
                Command::WatchScope { scope: Scope::Root },
            )?;
            if !matches!(
                libdevice::client::Watch::from_reply(rejected, incarnation, 15),
                Err(Error::Permission)
            ) {
                return Err("device Watch control acquired subscription authority");
            }
            let first = libdevice::client::Watch::decode_reply(
                &call(root, watch.control(), Command::QueryWatch)?,
                17,
            )
            .map_err(|_| "device Watch query failed")?;
            if first.terminal() || first.pending == 0 {
                return Err("device Watch missed initial state");
            }
            let page = call(
                root,
                &observer,
                Command::Enumerate {
                    scope: Scope::Root,
                    after: 0,
                    end: 0,
                },
            )?;
            let (header, body) =
                Header::decode(&page.payload).map_err(|_| "device enumeration reply invalid")?;
            if header.op != 14 || header.error.is_some() || page.handles.remaining() != 0 {
                return Err("device enumeration failed");
            }
            let scan =
                protocol::decode_scan(body, 0, 0).map_err(|_| "device scan receipt invalid")?;
            let expected_host = grants.len() > startup::ROOT_BASE;
            if scan.device.is_some() != expected_host {
                return Err("device platform import disagrees with delegated roots");
            }
            if let Some(id) = scan.device {
                if id.incarnation() != incarnation {
                    return Err("platform device registry identity mismatch");
                }
                let reply = call(root, &observer, Command::QueryDevice { id })?;
                let info = protocol::decode_device(&reply.payload, id, reply.handles.remaining())
                    .map_err(|_| "platform device description invalid")?;
                if !info.present
                    || info.parent.is_some()
                    || info.identity != libpci::host::HOST_IDENTITY
                    || info.name != libpci::host::HOST_NAME
                    || info.limits.entries != libpci::host::HOST_RESOURCES.len() as u32
                {
                    return Err("platform device description mismatch");
                }
            }
            let acknowledged = libdevice::client::Watch::decode_reply(
                &call(
                    root,
                    watch.control(),
                    Command::AckWatch {
                        sequence: first.sequence,
                    },
                )?,
                18,
            )
            .map_err(|_| "device Watch acknowledgement failed")?;
            if acknowledged.pending != 0
                || !wait::probe(watch.observer().as_handle(), ObjectSignals::READABLE)
                    .is_ok_and(|signals| signals == ObjectSignals::NONE)
            {
                return Err("device Watch acknowledgement retained notification");
            }
            check_discovery(root, &observer, expected_host)?;
            debug!(
                "device discovery ready: finite enumeration, OAL root and Watch acknowledgement"
            );
            Ok((observer, observer_admin, watch))
        })();
        let (observer, observer_admin, watch) = match observer_result {
            Ok(roles) => roles,
            Err(error) => {
                let sealed = root
                    .manager_source_call(
                        super::device_supervision::Manager::Device,
                        Command::SealSource,
                        libkernel::time::timeout_millis(DEADLINE_MS)
                            .map_err(|_| "device seal deadline failed")?,
                    )
                    .and_then(|reply| empty(&reply, spawned.pid, 2, None));
                if sealed.is_err() {
                    debug!("device assembly source closure is unconfirmed");
                }
                return Err(error);
            }
        };
        debug!(
            "device manager assembly ready: pid={}, registry={}, authenticated bootstrap and observer authority",
            spawned.pid, incarnation
        );
        Ok(Self {
            pid: spawned.pid,
            job,
            pool,
            root: Some(root_entry),
            observer: Some(observer),
            observer_admin: Some(observer_admin),
            watch: Some(watch),
        })
    }
    pub fn stop(&mut self, root: &mut RootSupervisor) -> Result<(), &'static str> {
        let deadline =
            libkernel::time::timeout_millis(40_000).map_err(|_| "device stop deadline failed")?;
        let deadline =
            root.manager_stopping(super::device_supervision::Manager::Device, deadline)?;
        let Self { pid, job, pool, .. } = *self;
        process::seal_job(job).map_err(|_| "device assembly JobSeal failed")?;
        if let Some(watch) = self.watch.as_ref() {
            let observer_admin = self
                .observer_admin
                .as_ref()
                .ok_or("device observer administration missing")?;
            empty(
                &call_until(root, observer_admin, Command::SealSource, deadline)?,
                pid,
                2,
                None,
            )?;
            let notified = root
                .wait_until(
                    watch.observer().as_handle(),
                    ObjectSignals::READABLE,
                    deadline,
                )
                .map_err(|_| "device Watch terminal notification failed")?;
            if !notified.observed.contains(ObjectSignals::READABLE) {
                return Err("device Watch terminal notification missing");
            }
            let terminal = libdevice::client::Watch::decode_reply(
                &call_until(root, watch.control(), Command::QueryWatch, deadline)?,
                17,
            )
            .map_err(|_| "device Watch terminal query failed")?;
            if terminal.reason != libdevice::watch::Reason::Revoked || terminal.pending == 0 {
                return Err("device Watch lost source closure");
            }
            let acknowledged = libdevice::client::Watch::decode_reply(
                &call_until(
                    root,
                    watch.control(),
                    Command::AckWatch {
                        sequence: terminal.sequence,
                    },
                    deadline,
                )?,
                18,
            )
            .map_err(|_| "device Watch terminal acknowledgement failed")?;
            if !acknowledged.terminal() || acknowledged.pending != 0 {
                return Err("device Watch terminal receipt invalid");
            }
            self.watch = None;
            debug!(
                "device discovery stopped: source revocation observed and terminal sequence acknowledged"
            );
        }
        self.observer = None;
        self.observer_admin = None;
        empty(
            &root.manager_source_call(
                super::device_supervision::Manager::Device,
                Command::SealSource,
                deadline,
            )?,
            pid,
            2,
            None,
        )?;
        let source_deadline = deadline;
        loop {
            let reply = root.manager_source_call(
                super::device_supervision::Manager::Device,
                Command::QuerySourceStop,
                source_deadline,
            )?;
            if reply.sender_pid != pid {
                return Err("device source stop reply identity mismatch");
            }
            match protocol::decode_source_stop(&reply.payload, reply.handles.remaining())
                .map_err(|_| "device source stop result invalid")?
            {
                libdevice::registry::SourceStop::Closed => break,
                libdevice::registry::SourceStop::Open => {
                    return Err("device source reopened after sealing");
                }
                libdevice::registry::SourceStop::Stopping => {
                    if libkernel::time::expired(source_deadline)
                        .map_err(|_| "device source clock failed")?
                    {
                        return Err("device source stop timed out");
                    }
                    let retry = libkernel::time::timeout_millis(1)
                        .map_err(|_| "device source retry deadline failed")?;
                    root.sleep_until(erhino_shared::time::Deadline::at(
                        retry.at_ns.min(source_deadline.at_ns),
                    ))
                    .map_err(|_| "device source retry sleep failed")?;
                }
            }
        }
        root.manager_source_closed(super::device_supervision::Manager::Device)?;
        self.root = None;
        root.collect_manager(super::device_supervision::Manager::Device)?;
        root.collect_job(job)?;
        root.close_control(job)?;
        root.close_control(pool)?;
        debug!(
            "device manager assembly stopped: source sealing and normal process completion confirmed"
        );
        Ok(())
    }
}

impl RootSupervisor {
    pub(super) fn stop_devices(&mut self) -> Result<(), &'static str> {
        let mut device = self
            .devices
            .take()
            .ok_or("device assembly record missing")?;
        match device.stop(self) {
            Ok(()) => {
                self.finish_device_supervision()?;
                Ok(())
            }
            Err(error) => {
                self.devices = Some(device);
                Err(error)
            }
        }
    }
}

fn check_discovery(
    root: &mut RootSupervisor,
    observer: &MailboxSender,
    has_host: bool,
) -> Result<(), &'static str> {
    use liboal::protocol as oal;
    for command in [
        oal::Request::Lookup { path: "" },
        oal::Request::Enumerate {
            path: "",
            cursor: &[],
            max_entries: 8,
        },
    ] {
        let deadline = libkernel::time::timeout_millis(DEADLINE_MS)
            .map_err(|_| "device OAL deadline failed")?;
        let mut bytes = [0; oal::HEADER_LEN + oal::BODY_MAX];
        let used = oal::encode_request(&command, deadline, &mut bytes)
            .map_err(|_| "device OAL encoding failed")?;
        let request = librpc::Request::new(oal::ID, &bytes[..used])
            .map_err(|_| "device OAL request failed")?;
        let reply = root
            .call(observer, deadline, request)
            .map_err(|_| "device OAL transport failed")?;
        if reply.handles.remaining() != 0 {
            return Err("device OAL root exported authority");
        }
        match oal::decode_response(&reply.payload, &command, deadline, 0)
            .map_err(|_| "device OAL reply invalid")?
        {
            oal::Response::Found(info) if info.contains(liboal::info::DIRECTORY) => {}
            oal::Response::Page(page) if page.count() == u32::from(has_host) => {}
            _ => return Err("device OAL platform root mismatch"),
        }
    }
    Ok(())
}
