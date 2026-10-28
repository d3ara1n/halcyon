//! init 的驱动装配记录；映像/政策/来源管理和执行域由原 RootSupervisor 持有。
pub(super) const STOP_TIMEOUT_NS: u64 = 40_000_000_000;
use super::{
    JOB_FULL_RIGHTS, RootSupervisor, SUPERVISOR_RIGHTS, TopologyNames, init_process_info,
    root_memory_pool,
};
use alloc::vec::Vec;
#[cfg(feature = "acceptance-device-retry-race")]
use erhino_shared::object::ObjectSignals;
use erhino_shared::{
    memory_pool::MemoryPoolBudget,
    object::{Handle, Rights},
    proc::HandleGrant,
};
use libdevice::{protocol::Command, registry::SourceStop};
use libdriver::{
    boot::{self, Declaration},
    protocol,
};
use libkernel::{
    ipc::{message::MailboxSender, object},
    process,
};
use libprocess::{
    GrantOutcome, SpawnRequest,
    image::{IMAGE_RIGHTS, Image},
};
use librpc::Exchange;
const POOL: Rights = Rights::from_raw(
    Rights::READ.raw() | Rights::CREATE.raw() | Rights::DUPLICATE.raw() | Rights::GRANT.raw(),
);
const ENTRY: Rights = Rights::from_raw(
    Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
pub(super) struct Manager {
    pid: u64,
    job: Handle,
    worker_job: Handle,
    pool: Handle,
    entry: Option<MailboxSender>,
    observer: Option<MailboxSender>,
    images: Vec<Image>,
    registry_pid: u64,
}
impl Manager {
    #[cfg(feature = "acceptance-device")]
    pub(super) fn observer_handle(&self) -> Result<Handle, &'static str> {
        self.observer
            .as_ref()
            .map(MailboxSender::as_handle)
            .ok_or("driver observer missing")
    }
    #[cfg(feature = "acceptance-device")]
    pub(super) fn query(&self, root: &mut RootSupervisor) -> Result<(bool, u64), &'static str> {
        let deadline =
            libkernel::time::timeout_millis(5_000).map_err(|_| "Drivers query deadline failed")?;
        let mut wire = [0; protocol::HEADER_LEN];
        let used = protocol::Header {
            op: 2,
            error: None,
            deadline,
        }
        .encode(&[], &mut wire)
        .map_err(|_| "Drivers query encoding failed")?;
        let reply = root
            .call(
                self.entry.as_ref().ok_or("Drivers Control missing")?,
                deadline,
                librpc::Request::new(protocol::MANAGER_ID, &wire[..used])
                    .map_err(|_| "Drivers query framing failed")?,
            )
            .map_err(|_| "Drivers Control query failed")?;
        let (header, body) = protocol::Header::decode(&reply.payload)
            .map_err(|_| "Drivers query response invalid")?;
        if reply.sender_pid != self.pid
            || header.op != 2
            || header.error.is_some()
            || reply.handles.remaining() != 0
            || body.len() != 16
        {
            return Err("Drivers query response mismatched");
        }
        let mut reader = librpc::bytes::Reader::new(body);
        let stopping = match reader.u32().map_err(|_| "Drivers query state missing")? {
            0 => false,
            1 => true,
            _ => return Err("Drivers query state invalid"),
        };
        if reader.u32().map_err(|_| "Drivers query reserved missing")? != 0 {
            return Err("Drivers query reserved invalid");
        }
        let bindings = reader.u64().map_err(|_| "Drivers query count missing")?;
        reader.finish().map_err(|_| "Drivers query trailing data")?;
        Ok((stopping, bindings))
    }
    #[cfg(feature = "acceptance-device")]
    pub(super) fn retry(
        &self,
        root: &mut RootSupervisor,
        device: libdevice::identity::DeviceId,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<Option<libdevice::Error>, &'static str> {
        Self::retry_on(
            root,
            self.entry.as_ref().ok_or("Drivers Control missing")?,
            self.pid,
            device,
            deadline,
        )
    }
    #[cfg(feature = "acceptance-device")]
    fn retry_on(
        root: &mut dyn Exchange,
        entry: &MailboxSender,
        pid: u64,
        device: libdevice::identity::DeviceId,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<Option<libdevice::Error>, &'static str> {
        let mut wire = [0; protocol::HEADER_LEN + libdevice::identity::DeviceId::ENCODED_LEN];
        let used = protocol::encode_manager_retry(device, deadline, &mut wire)
            .map_err(|_| "Drivers Retry encoding failed")?;
        // 非幂等政策：运输失败只报告，不在此重发；调用者先观察实际结果再作新的决定。
        let reply = root
            .call(
                entry,
                deadline,
                librpc::Request::new(protocol::MANAGER_ID, &wire[..used])
                    .map_err(|_| "Drivers Retry framing failed")?,
            )
            .map_err(|_| "Drivers Retry outcome unavailable; do not blindly retry")?;
        let (header, body) = protocol::Header::decode(&reply.payload)
            .map_err(|_| "Drivers Retry response invalid")?;
        if reply.sender_pid != pid
            || header.op != protocol::MANAGER_RETRY
            || !body.is_empty()
            || reply.handles.remaining() != 0
        {
            return Err("Drivers Retry response mismatched");
        }
        Ok(header.error)
    }
    #[cfg(feature = "acceptance-device-retry-race")]
    pub(super) fn retry_concurrent(
        &self,
        root: &mut RootSupervisor,
        device: libdevice::identity::DeviceId,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<(), &'static str> {
        use libdevice::Error;
        use libkernel::ipc::capability::Capability;
        let handle = object::duplicate(
            self.entry
                .as_ref()
                .ok_or("Drivers Control missing")?
                .as_handle(),
            Rights::WRITE | Rights::WAIT,
        )
        .map_err(|_| "Drivers concurrent Control alias failed")?;
        // SAFETY: 原认证Control sender的新独占缩权entry；线程只拥有这份alias。
        let (entry, description) =
            MailboxSender::from_capability(unsafe { Capability::from_raw(handle) })
                .map_err(|_| "Drivers concurrent sender role invalid")?;
        if description.rights != Rights::WRITE | Rights::WAIT {
            return Err("Drivers concurrent sender rights mismatched");
        }
        let pid = self.pid;
        let records = libruntime::thread::retained_records();
        // 此点只有root主线程；新增项只能属于本次并发Caller。
        if records != 1 {
            return Err("Drivers concurrent Caller requires only the main thread");
        }
        let worker = libruntime::thread::Builder::new()
            .spawn(move || {
                Self::retry_on(&mut librpc::Caller::new(), &entry, pid, device, deadline)
            })
            .map_err(|_| "Drivers concurrent Caller thread admission failed")?;
        let caller_id = worker.id();
        // 两条均为显式非幂等操作；没有成功/Unknown后重发，也没有产品调度闸门。
        let main_result = self.retry(root, device, deadline);
        let thread = worker.thread();
        let done = root
            .wait_until(thread.control().as_handle(), ObjectSignals::DONE, deadline)
            .map_err(|_| "Drivers concurrent Caller wait failed")?;
        if !done.observed.contains(ObjectSignals::DONE) {
            return Err("Drivers concurrent Caller timed out");
        }
        let worker_result = worker
            .try_join()
            .unwrap_or_else(|_| panic!("observed thread DONE disappeared"));
        drop(thread);
        let worker_result = match worker_result {
            libthread::ThreadOutcome::Returned {
                value,
                leave: Ok(()),
            } => value,
            _ => Err("Drivers concurrent Caller thread did not return normally"),
        };
        let end = libkernel::time::timeout_millis(5_000)
            .map_err(|_| "Drivers concurrent Caller retirement clock failed")?;
        while libruntime::thread::retained_records() > records {
            let count = libruntime::thread::retained_records();
            if matches!(
                libruntime::thread::reap(count),
                libthread::ReapProgress::Failed(_)
            ) {
                return Err("Drivers concurrent Caller thread retirement failed");
            }
            if libkernel::time::expired(end)
                .map_err(|_| "Drivers concurrent Caller retirement clock failed")?
            {
                return Err("Drivers concurrent Caller thread record remains");
            }
            let retry = libkernel::time::timeout_millis(1)
                .map_err(|_| "Drivers concurrent Caller retirement clock failed")?;
            root.sleep_until(erhino_shared::time::Deadline::at(
                retry.at_ns.min(end.at_ns),
            ))
            .map_err(|_| "Drivers concurrent Caller retirement wait failed")?;
        }
        let main_result = main_result?;
        let worker_result = worker_result?;
        libkernel::debug!(
            "DEVICE Drivers concurrent Retry replies: serial={}, main={:?}, worker={:?}, caller_tid={}, thread_joined=true, retained_records={}",
            device.serial(),
            main_result,
            worker_result,
            caller_id,
            libruntime::thread::retained_records()
        );
        match (main_result, worker_result) {
            (None, Some(Error::Busy | Error::Conflict | Error::NotFound))
            | (Some(Error::Busy | Error::Conflict | Error::NotFound), None) => Ok(()),
            _ => Err("Drivers concurrent Retry did not return one reset and one bounded rejection"),
        }
    }
    #[cfg(feature = "acceptance-device-reuse")]
    pub(super) fn worker_delegation(&self) -> Result<MemoryPoolBudget, &'static str> {
        let snapshot = libkernel::memory_pool::query(self.pool)
            .map_err(|_| "driver worker delegation query failed")?;
        Ok(MemoryPoolBudget {
            pages: snapshot.delegated,
            metadata: snapshot.metadata.delegated,
        })
    }
    pub fn launch(
        root: &mut RootSupervisor,
        services: Handle,
        names: &mut TopologyNames,
        manager_image: &[u8],
        worker_images: [&[u8]; 3],
    ) -> Result<Self, &'static str> {
        // 根授权决定此机器可出现的首批硬件工作集；零ECAM根不预付永远不能启动的硬件链。
        let hardware = super::INIT_STARTUP
            .lock()
            .as_ref()
            .ok_or("init platform input missing")?
            ._platform
            .ecam_handle()
            .is_some();
        let pool_units = if hardware { 1 + 3 + 1 } else { 1 } as u64;
        let [pci_image, xhci_image, hid_image] = worker_images;
        // 一份manager；有硬件根时再付三份活worker与一次真实退休重叠。
        // 每份成本由原WORKER_POOL拥有；外部旧入口保活的尾部不能提前退款。
        // ProcessWrite的临时副本由调用者manager付款，不能计入已委托给新worker的Pool。
        let loader_metadata = if hardware {
            (libprocess::LOADER_WRITE_BUFFER_BYTES as u64)
                .div_ceil(erhino_shared::memory_pool::METADATA_SLOT_BYTES)
        } else {
            0
        };
        let pool = root
            .derive_pool(
                root_memory_pool(),
                MemoryPoolBudget {
                    pages: boot::WORKER_POOL.pages * pool_units,
                    metadata: boot::WORKER_POOL.metadata * pool_units + loader_metadata,
                },
                POOL,
            )
            .map_err(|_| "driver Pool failed")?;
        let job = root
            .create_job(services, pool, JOB_FULL_RIGHTS)
            .map_err(|_| "driver manager Job failed")?;
        let worker_job = root
            .create_job(services, pool, JOB_FULL_RIGHTS)
            .map_err(|_| "driver workers Job failed")?;
        names.register_job(job, "driver_management");
        names.register_job(worker_job, "drivers");
        let bootstrap = root
            .create_mailbox(Rights::READ | Rights::WAIT | Rights::MANAGE, ENTRY)
            .map_err(|_| "driver bootstrap failed")?;
        let expected = object::query(bootstrap.peer)
            .map_err(|_| "driver bootstrap identity failed")?
            .object_id;
        let mut images = Vec::new();
        images
            .try_reserve_exact(3 + usize::from(cfg!(feature = "acceptance-device-candidates")))
            .map_err(|_| "driver image record failed")?;
        images.push(
            root.copy_image(pci_image)
                .map_err(|_| "PCI readonly image failed")?,
        );
        images.push(
            root.copy_image(xhci_image)
                .map_err(|_| "xHCI readonly image failed")?,
        );
        images.push(
            root.copy_image(hid_image)
                .map_err(|_| "HID readonly image failed")?,
        );
        let mut declarations = alloc::vec![
            Declaration {
                key: "pci-generic",
                identity: libpci::host::HOST_IDENTITY,
                priority: 1,
                capacity: 1,
                reusable: false,
                child_registrar: true,
                trusted_physical: true,
                image_bytes: pci_image.len() as u64,
            },
            Declaration {
                key: "usb-xhci",
                identity: libpci::config::XHCI_IDENTITY,
                priority: 1,
                capacity: 1,
                reusable: false,
                child_registrar: true,
                trusted_physical: true,
                image_bytes: xhci_image.len() as u64,
            },
            Declaration {
                key: "usb-hid",
                identity: libusb::descriptor::BOOT_KEYBOARD_IDENTITY,
                priority: 1,
                capacity: 2,
                reusable: true,
                child_registrar: false,
                trusted_physical: false,
                image_bytes: hid_image.len() as u64,
            },
        ];
        if cfg!(feature = "acceptance-device-function-session") {
            // 独立能力跨轮验收由init承担真实控制器责任；自动选择仍只有srv_driver。
            images.truncate(1);
            declarations.truncate(1);
        }
        if cfg!(feature = "acceptance-device-candidates") {
            // 负装配只用于显式验收：真实PCI映像不支持USB identity，必须在任何硬件使用前拒绝。
            // 它不取得物理/子登记授权；真实HID仍是唯一实际支持实现，正常声明不变。
            images.push(
                root.copy_image(pci_image)
                    .map_err(|_| "incompatible candidate readonly image failed")?,
            );
            declarations.push(Declaration {
                key: "usb-incompatible-pci",
                identity: libusb::descriptor::BOOT_KEYBOARD_IDENTITY,
                priority: 2,
                capacity: 1,
                reusable: false,
                child_registrar: false,
                trusted_physical: false,
                image_bytes: pci_image.len() as u64,
            });
        }
        let mut payload = Vec::new();
        let length =
            boot::encoded_bytes(&declarations).map_err(|_| "driver manifest length invalid")?;
        payload
            .try_reserve_exact(length)
            .map_err(|_| "driver manifest storage failed")?;
        payload.resize(length, 0);
        boot::encode(&declarations, &mut payload).map_err(|_| "driver manifest invalid")?;
        // 来源发行前预留输入/映像/监督容量，发行后原管理记录立即持 SourceAdmin。
        root.reserve_process()
            .map_err(|_| "driver supervision admission failed")?;
        let device = root
            .devices
            .take()
            .ok_or("device manager unavailable for driver delegation")?;
        let issued = device.issue_driver_source(root);
        let registry_pid = device.pid();
        root.devices = Some(device);
        let (claim, admin) = issued?;
        root.attach_manager_source(super::supervision::Instance::Driver, admin, registry_pid)?;
        let mut manager = Self {
            pid: 0,
            job,
            worker_job,
            pool,
            entry: None,
            observer: None,
            images,
            registry_pid,
        };
        let result = (|| {
            let mut grants = Vec::new();
            grants
                .try_reserve_exact(boot::IMAGE_BASE + manager.images.len())
                .map_err(|_| "driver grant records allocation failed")?;
            grants.extend_from_slice(&[
                HandleGrant {
                    handle: bootstrap.peer,
                    rights: ENTRY,
                },
                HandleGrant {
                    handle: root
                        .duplicate(pool, POOL)
                        .map_err(|_| "driver Pool grant failed")?,
                    rights: POOL,
                },
                HandleGrant {
                    handle: root
                        .duplicate(worker_job, JOB_FULL_RIGHTS)
                        .map_err(|_| "driver Job grant failed")?,
                    rights: JOB_FULL_RIGHTS,
                },
                HandleGrant {
                    handle: root
                        .duplicate(claim.as_handle(), ENTRY)
                        .map_err(|_| "driver ClaimSource grant failed")?,
                    rights: ENTRY,
                },
            ]);
            for image in &manager.images {
                grants.push(HandleGrant {
                    handle: root
                        .duplicate(image.handle(), IMAGE_RIGHTS)
                        .map_err(|_| "driver image grant failed")?,
                    rights: IMAGE_RIGHTS,
                });
            }
            let prepared = root.prepare_process(SpawnRequest {
                job,
                parent: init_process_info(),
                memory_pool: pool,
                image: manager_image,
                payload: &payload,
                grants: &grants,
                control_rights: SUPERVISOR_RIGHTS,
            });
            if prepared.is_ok() || matches!(&prepared,Err(f) if f.grants==GrantOutcome::Consumed) {
                for grant in &grants {
                    root.transferred(grant.handle);
                }
            }
            let prepared = prepared.map_err(|failure| {
                libkernel::debug!("driver manager preparation failed: {:?}", failure);
                "driver manager preparation failed"
            })?;
            let pid = root.start_instance(super::supervision::Instance::Driver, prepared)?;
            manager.pid = pid;
            names.register_process(pid, "srv_driver");
            let deadline = libkernel::time::timeout_millis(5_000)
                .map_err(|_| "driver ready deadline failed")?;
            let mut ready = root
                .receive_until(bootstrap.owner, deadline)
                .map_err(|_| "driver readiness failed")?;
            if ready.header.kind != protocol::BOOT_READY
                || ready.header.sender_pid != pid
                || ready.header.sender_context_id != expected
                || ready.payload.len() != boot::MANAGER_READY_BYTES
                || ready.handles.remaining() != boot::MANAGER_READY_SLOTS
            {
                return Err("driver readiness identity invalid");
            }
            let mailboxes = boot::ManagerReady::decode(&ready.payload)
                .map_err(|_| "driver mailbox identity invalid")?;
            let (entry, info) = MailboxSender::from_capability(
                ready
                    .handles
                    .take(boot::MANAGER_READY_CONTROL)
                    .map_err(|_| "driver control missing")?,
            )
            .map_err(|_| "driver control invalid")?;
            let (observer, view_info) = MailboxSender::from_capability(
                ready
                    .handles
                    .take(boot::MANAGER_READY_OBSERVER)
                    .map_err(|_| "driver observer missing")?,
            )
            .map_err(|_| "driver observer invalid")?;
            if info.related_object_id != mailboxes.control
                || view_info.related_object_id != mailboxes.observer
                || info.object_id == view_info.object_id
                || !info.rights.contains(ENTRY)
                || !view_info.rights.contains(ENTRY)
                || ready.handles.remaining() != 0
            {
                return Err("driver control authority invalid");
            }
            manager.entry = Some(entry);
            manager.observer = Some(observer);
            root.close_control(bootstrap.owner)?;
            libkernel::debug!(
                "driver manager assembly ready: pid={}, boot declarations paired with sealed images",
                manager.pid
            );
            Ok(())
        })();
        if let Err(error) = result {
            // 已发行来源不重签。记录由根保留；失败终局先封原来源，CPU 死亡不算物理完成。
            let _ = manager.seal(root);
            root.drivers = Some(manager);
            return Err(error);
        }
        Ok(manager)
    }
    fn seal(&self, root: &mut RootSupervisor) -> Result<(), &'static str> {
        let deadline = libkernel::time::timeout_millis(5_000)
            .map_err(|_| "driver fault seal deadline failed")?;
        self.seal_until(root, deadline)
    }
    fn seal_until(
        &self,
        root: &mut RootSupervisor,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<(), &'static str> {
        let reply = root.manager_source_call(
            super::supervision::Instance::Driver,
            Command::SealSource,
            deadline,
        )?;
        let (header, body) = libdevice::protocol::Header::decode(&reply.payload)
            .map_err(|_| "driver seal reply invalid")?;
        if reply.sender_pid != self.registry_pid
            || header.op != 2
            || header.error.is_some()
            || !body.is_empty()
            || reply.handles.remaining() != 0
        {
            return Err("driver source seal unconfirmed");
        }
        Ok(())
    }
    pub fn stop(&mut self, root: &mut RootSupervisor) -> Result<(), &'static str> {
        let entry = self
            .entry
            .as_ref()
            .ok_or("driver manager control missing")?;
        // 首批三层Binding清理及管理者DONE等待各留一个10s诊断工作窗；一次绝对期限，
        // 不随Query刷新，也不改变硬件halt/reset或任何安全完成判据。
        let deadline = libkernel::time::timeout_millis(STOP_TIMEOUT_NS / 1_000_000)
            .map_err(|_| "driver stop deadline failed")?;
        let deadline = root.manager_stopping(super::supervision::Instance::Driver, deadline)?;
        process::seal_job(self.worker_job).map_err(|_| "driver worker JobSeal failed")?;
        process::seal_job(self.job).map_err(|_| "driver manager JobSeal failed")?;
        libkernel::debug!(
            "driver assembly stop started: now_ns={}, end_ns={:?}",
            libkernel::time::snapshot()
                .map_err(|_| "driver stop clock failed")?
                .now_ns,
            deadline.instant()
        );
        let mut bytes = [0; protocol::HEADER_LEN];
        let used = protocol::Header {
            op: 1,
            error: None,
            deadline,
        }
        .encode(&[], &mut bytes)
        .map_err(|_| "driver stop encoding failed")?;
        let result = root
            .call(
                entry,
                deadline,
                librpc::Request::new(protocol::MANAGER_ID, &bytes[..used])
                    .map_err(|_| "driver stop request failed")?,
            )
            .map_err(|_| "driver stop RPC unconfirmed");
        // 私有 Stop 先接管选择，随后封来源纳入未知 Claim；即使 Stop 回复 Unknown 也须封。
        self.seal_until(root, deadline)?;
        let reply = result?;
        let (header, body) =
            protocol::Header::decode(&reply.payload).map_err(|_| "driver stop reply invalid")?;
        if reply.sender_pid != self.pid
            || header.op != 1
            || header.error.is_some()
            || body.len() != 16
            || reply.handles.remaining() != 0
        {
            return Err("driver stop reply mismatch");
        }
        loop {
            let reply = root.manager_source_call(
                super::supervision::Instance::Driver,
                Command::QuerySourceStop,
                deadline,
            )?;
            if reply.sender_pid != self.registry_pid {
                return Err("driver source completion identity invalid");
            }
            match libdevice::protocol::decode_source_stop(&reply.payload, reply.handles.remaining())
                .map_err(|_| "driver source completion invalid")?
            {
                SourceStop::Closed => {
                    root.manager_device_closed(super::supervision::Instance::Driver)?;
                    libkernel::debug!(
                        "driver assembly source Closed: now_ns={}",
                        libkernel::time::snapshot()
                            .map_err(|_| "driver stop clock failed")?
                            .now_ns
                    );
                    break;
                }
                SourceStop::Open => return Err("driver source unexpectedly open"),
                SourceStop::Stopping => {
                    if libkernel::time::expired(deadline).map_err(|_| "driver stop clock failed")? {
                        libkernel::debug!(
                            "driver assembly source stop deadline expired: now_ns={}, end_ns={:?}",
                            libkernel::time::snapshot()
                                .map_err(|_| "driver stop clock failed")?
                                .now_ns,
                            deadline.instant()
                        );
                        return Err("driver source stop timed out");
                    }
                    let retry = libkernel::time::timeout_millis(1)
                        .map_err(|_| "driver retry clock failed")?;
                    root.sleep_until(erhino_shared::time::Deadline::at(
                        retry.at_ns.min(deadline.at_ns),
                    ))
                    .map_err(|_| "driver stop retry failed")?;
                }
            }
        }
        root.collect_manager(super::supervision::Instance::Driver)?;
        root.collect_job(self.worker_job)?;
        root.close_control(self.worker_job)?;
        root.collect_job(self.job)?;
        root.close_control(self.job)?;
        self.entry = None;
        self.observer = None;
        self.images.clear();
        #[cfg(feature = "acceptance-device")]
        super::device_acceptance::verify_worker_pool_refund(self.pool)?;
        root.close_control(self.pool)?;
        libkernel::debug!(
            "driver assembly collection completed: now_ns={}",
            libkernel::time::snapshot()
                .map_err(|_| "driver collection clock failed")?
                .now_ns
        );
        libkernel::debug!(
            "driver assembly stopped: source Closed, bindings Released and worker processes collected"
        );
        Ok(())
    }
}
impl RootSupervisor {
    pub(super) fn stop_drivers(&mut self) -> Result<(), &'static str> {
        let mut driver = self
            .drivers
            .take()
            .ok_or("driver assembly record missing")?;
        match driver.stop(self) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.drivers = Some(driver);
                Err(error)
            }
        }
    }
}
