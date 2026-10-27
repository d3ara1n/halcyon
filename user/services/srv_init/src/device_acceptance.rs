//! 设备路线的正式功能客户端装配；授权沿原设备来源，出生/回收沿原根监督账本。
use super::*;
use alloc::vec::Vec;
use libkernel::ipc::message::Mailbox;
use libkernel::ipc::{object, wait};
pub(super) fn exercise(
    root: &mut RootSupervisor,
    job: Handle,
    sources: (
        libkernel::ipc::message::MailboxSender,
        libkernel::ipc::message::MailboxSender,
    ),
    drivers: Handle,
) -> Result<Vec<Mailbox>, &'static str> {
    let mut image = None;
    with_init_payload(|payload| {
        tar::walk(payload, |entry| {
            if entry.name == "bin/test_device" {
                image = Some(StartupImage::from_slice(entry.data));
            }
        })
    })
    .map_err(|_| "device client image archive invalid")?;
    let image = image.ok_or("device client image missing")?;
    root.reserve_process()
        .map_err(|_| "device client supervision admission failed")?;
    let (source, admin) = sources;
    let rights = Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT;
    let grant = root
        .duplicate(source.as_handle(), rights)
        .map_err(|_| "device client grant failed")?;
    let driver_grant = root
        .duplicate(drivers, rights)
        .map_err(|_| "driver observer grant failed")?;
    let mut grants = alloc::vec![
        HandleGrant {
            handle: grant,
            rights,
        },
        HandleGrant {
            handle: driver_grant,
            rights,
        },
    ];
    let mut backlog = Vec::new();
    for _ in 0..test_device::BACKLOG_PORTS {
        let inbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
            .map_err(|_| "Drivers backlog inbox failed")?;
        let sender = inbox
            .mint(0, rights)
            .map_err(|_| "Drivers backlog sender failed")?;
        let handle = root
            .duplicate(sender.sender.as_handle(), rights)
            .map_err(|_| "Drivers backlog grant failed")?;
        grants.push(HandleGrant { handle, rights });
        backlog.push(inbox);
    }
    if grants.len() != test_device::GRANTS {
        return Err("device client startup slot mismatch");
    }
    let spawned = spawn(SpawnRequest {
        memory_pool: root_memory_pool(),
        parent: init_process_info(),
        job,
        image: image.as_slice(),
        payload: if cfg!(feature = "acceptance-device-reuse") {
            b"reuse"
        } else {
            b""
        },
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    });
    if spawned.is_ok() || matches!(&spawned,Err(f) if f.grants==libprocess::GrantOutcome::Consumed)
    {
        for grant in &grants {
            root.transferred(grant.handle);
        }
    }
    let result = (|| {
        let spawned = spawned.map_err(|_| "device client spawn failed")?;
        root.track_process(Supervised {
            pid: spawned.pid,
            control: spawned.control,
        });
        let deadline = libkernel::time::timeout_millis(240_000)
            .map_err(|_| "device client deadline failed")?;
        debug!(
            "device client synchronous wait entered: pid={}, deadline={}",
            spawned.pid, deadline.at_ns
        );
        wait::wait_until(spawned.control, ObjectSignals::DONE, deadline)
            .map_err(|_| "device client did not finish")?;
        let snapshot =
            process::query(spawned.control).map_err(|_| "device client status unavailable")?;
        if snapshot.state != ProcessState::Dead as u32
            || snapshot.cause != TerminationCause::Completed as u32
        {
            return Err("device client failed");
        }
        root.collect_process(spawned.pid)?;
        Ok(())
    })();
    let policy = if result.is_ok() {
        (|| {
            let reply = device::call(
                &source,
                libdevice::protocol::Command::Enumerate {
                    scope: libdevice::registry::Scope::Root,
                    after: 0,
                    end: 0,
                },
            )?;
            let (header, body) = libdevice::protocol::Header::decode(&reply.payload)
                .map_err(|_| "Drivers policy device page invalid")?;
            if header.op != 14 || header.error.is_some() || reply.handles.remaining() != 0 {
                return Err("Drivers policy device page rejected");
            }
            let page = libdevice::protocol::decode_scan(body, 0, 0)
                .map_err(|_| "Drivers policy scan invalid")?;
            let device = page.device.ok_or("Drivers policy live PCI host missing")?;
            let result = root
                .drivers
                .as_ref()
                .ok_or("Drivers policy manager missing")?
                .retry(
                    device,
                    libkernel::time::timeout_millis(5_000)
                        .map_err(|_| "Drivers Retry deadline failed")?,
                )?;
            if result != Some(libdevice::Error::Busy) {
                return Err("Drivers Retry reset an active binding");
            }
            debug!(
                "DEVICE Drivers Control Retry rejected active binding: serial={}",
                device.serial()
            );
            #[cfg(feature = "acceptance-device-reuse")]
            if !cfg!(feature = "acceptance-device-candidates") {
                retry_terminal(root, &source)?;
            }
            Ok(())
        })()
    } else {
        Ok(())
    };
    // 同一原来源收束；客户端失败/Grant已消费不再另发来源替代。
    let sealed = device::call(&admin, libdevice::protocol::Command::SealSource);
    result?;
    sealed?;
    policy?;
    drop(source);
    drop(admin);
    for inbox in &backlog {
        let head = libkernel::ipc::message::peek(inbox.as_handle())
            .map_err(|_| "Drivers unread backlog missing")?;
        if head.kind != liboal::protocol::ID || head.handle_count != 0 {
            return Err("Drivers backlog message invalid");
        }
    }
    root.drivers
        .as_ref()
        .ok_or("Drivers manager missing")?
        .query()?;
    debug!("DEVICE Drivers Control query passed with unread Observer replies");
    Ok(backlog)
}

#[cfg(feature = "acceptance-device-reuse")]
fn retry_terminal(
    root: &RootSupervisor,
    source: &libkernel::ipc::message::MailboxSender,
) -> Result<(), &'static str> {
    use libdevice::{Error, protocol::Command, registry::Scope};
    let drivers = root
        .drivers
        .as_ref()
        .ok_or("Drivers policy manager missing")?;
    let end = libkernel::time::timeout_millis(30_000)
        .map_err(|_| "Drivers Retry fixture deadline failed")?;
    // 客户端已关闭旧功能/Watch并已收集；先观察原HID Pool委托真实返还，
    // 不把allocated_pages=0或CPU DONE当作可再出生的额度。
    loop {
        let delegated = drivers.worker_delegation()?;
        let unit = libdriver::boot::WORKER_POOL;
        if delegated.pages == unit.pages * 2 && delegated.metadata == unit.metadata * 2 {
            break;
        }
        let now = libkernel::time::snapshot()
            .map_err(|_| "Drivers Retry clock failed")?
            .now_ns;
        if now >= end.at_ns {
            return Err("retired HID Pool delegation did not refund");
        }
        libkernel::time::sleep_until(erhino_shared::time::Deadline::at(
            end.at_ns.min(now.saturating_add(10_000_000)),
        ))
        .map_err(|_| "Drivers Retry refund wait failed")?;
    }
    let mut selected = None;
    let (mut after, mut scan_end) = (0, 0);
    loop {
        let reply = device::call_until(
            source,
            Command::Enumerate {
                scope: Scope::Root,
                after,
                end: scan_end,
            },
            end,
        )?;
        let (header, body) = libdevice::protocol::Header::decode(&reply.payload)
            .map_err(|_| "Drivers Retry page invalid")?;
        if header.op != 14 || header.error.is_some() || reply.handles.remaining() != 0 {
            return Err("Drivers Retry page rejected");
        }
        let page = libdevice::protocol::decode_scan(body, after, scan_end)
            .map_err(|_| "Drivers Retry scan invalid")?;
        after = page.next;
        scan_end = page.end;
        let Some(id) = page.device else { break };
        let reply = device::call_until(source, Command::QueryDevice { id }, end)?;
        let info =
            match libdevice::protocol::decode_device(&reply.payload, id, reply.handles.remaining())
            {
                Ok(info) => info,
                Err(Error::StaleInstance | Error::NotFound) => continue,
                Err(_) => return Err("Drivers Retry device query failed"),
            };
        if info.present
            && info.binding.is_none()
            && info.identity == libusb::descriptor::BOOT_KEYBOARD_IDENTITY
        {
            selected = Some(id);
        }
    }
    let id = selected.ok_or("Drivers Retry terminal keyboard missing")?;
    // 只发一次非幂等政策操作；Unknown不重发，取样本身也不保证后续Claim成功。
    #[cfg(feature = "acceptance-device-retry-race")]
    drivers.retry_concurrent(id, end)?;
    #[cfg(not(feature = "acceptance-device-retry-race"))]
    if drivers.retry(id, end)?.is_some() {
        return Err("Drivers Retry rejected the released detached terminal selection");
    }
    debug!(
        "DEVICE Drivers Control Retry reset terminal selection: serial={}",
        id.serial()
    );
    Ok(())
}

/// 此路线唯一外部客户端已经退出，物理Binding/进程/Job也已收集；核实际子Pool信用返还。
pub(super) fn verify_worker_pool_refund(handle: Handle) -> Result<(), &'static str> {
    let alias =
        object::duplicate(handle, Rights::READ).map_err(|_| "driver Pool observer failed")?;
    // SAFETY: 原装配记录仍持真实Pool，duplicate的新entry关闭责任交给typed观察owner。
    let observer = unsafe { MemoryPool::from_handle(alias) };
    let snapshot = observer
        .query()
        .map_err(|_| "driver Pool refund query failed")?;
    debug!(
        "DEVICE worker Pool refund observed: delegated_pages={}, delegated_metadata={}",
        snapshot.delegated, snapshot.metadata.delegated
    );
    if snapshot.delegated != 0 || snapshot.metadata.delegated != 0 {
        return Err("driver worker Pool credit remains after all client aliases closed");
    }
    Ok(())
}
