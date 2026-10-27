//! 原资源验收中的真实控制器消费者；两轮能力身份与硬件责任均由原Binding承担。
use super::device::{Manager, call_until};
use alloc::sync::Arc;
use erhino_shared::{
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libdevice::{
    Error,
    binding::Phase,
    client::Watch,
    identity::{BindingRef, DeviceId},
    protocol::{self, Command, Header},
    registry::{Scope, SourceStop},
    resource::Entry,
};
use libkernel::{
    debug,
    dma::DmaUse,
    ipc::{message::MailboxSender, object, wait},
    mmio::MmioControl,
    time,
};
use libusb::xhci::{Controller, Inputs, Progress};

// 验收防挂死界覆盖一次完整真实控制器轮次，不改变硬件halt/reset限值。
const ROUND_MILLIS: u64 = 120_000;
fn deadline() -> Result<Deadline, &'static str> {
    time::timeout_millis(ROUND_MILLIS).map_err(|_| "function check deadline failed")
}
fn now(end: Deadline) -> Result<u64, &'static str> {
    let now = time::Instant::now()
        .map_err(|_| "function check clock failed")?
        .as_nanos();
    if now >= end.at_ns {
        return Err("function check deadline expired");
    }
    Ok(now)
}
fn pause(end: Deadline) -> Result<(), &'static str> {
    let at = now(end)?.saturating_add(1_000_000).min(end.at_ns);
    time::sleep_until(Deadline { at_ns: at, ..end }).map_err(|_| "function check sleep failed")
}
fn checked(
    reply: &librpc::Reply,
    pid: u64,
    op: u16,
    status: Option<Error>,
    caps: usize,
) -> Result<&[u8], &'static str> {
    let (header, body) =
        Header::decode(&reply.payload).map_err(|_| "function check malformed reply")?;
    if reply.sender_pid != pid
        || header.op != op
        || header.error != status
        || reply.handles.len() != caps
    {
        debug!(
            "function check reply mismatch: pid={}, expected_pid={}, op={}, error={:?}, caps={}",
            reply.sender_pid,
            pid,
            header.op,
            header.error,
            reply.handles.len()
        );
        return Err("function check reply identity mismatch");
    }
    Ok(body)
}
fn empty(
    reply: &librpc::Reply,
    pid: u64,
    op: u16,
    status: Option<Error>,
) -> Result<(), &'static str> {
    if !checked(reply, pid, op, status, 0)?.is_empty() {
        return Err("function check unexpected body");
    }
    Ok(())
}
fn watch(
    sender: &MailboxSender,
    command: Command<'_>,
    manager: &Manager,
    op: u16,
    end: Deadline,
) -> Result<Watch, &'static str> {
    let reply = call_until(sender, command, end)?;
    if reply.sender_pid != manager.pid() {
        return Err("function check Watch sender mismatch");
    }
    Watch::from_reply(
        reply,
        sender
            .description()
            .map_err(|_| "function check registry identity failed")?
            .related_object_id,
        op,
    )
    .map_err(|_| "function check Watch adoption failed")
}
fn wait_change(watch: &Watch, pid: u64, sequence: u64, end: Deadline) -> Result<(), &'static str> {
    let reply = call_until(watch.control(), Command::AckWatch { sequence }, end)?;
    if reply.sender_pid != pid {
        return Err("function check Watch ack sender mismatch");
    }
    let info = Watch::decode_reply(&reply, 18).map_err(|_| "function check Watch ack invalid")?;
    if info.terminal() {
        return Err("function check Watch unexpectedly terminal");
    }
    if info.pending == 0 {
        let result = wait::wait_until(watch.observer().as_handle(), ObjectSignals::READABLE, end)
            .map_err(|_| "function check Watch wait failed")?;
        if !result.observed.contains(ObjectSignals::READABLE) {
            return Err("function check Watch notification missing");
        }
    }
    Ok(())
}
fn sequence(watch: &Watch, pid: u64, end: Deadline) -> Result<u64, &'static str> {
    let reply = call_until(watch.control(), Command::QueryWatch, end)?;
    if reply.sender_pid != pid {
        return Err("function check Watch query sender mismatch");
    }
    Ok(Watch::decode_reply(&reply, 17)
        .map_err(|_| "function check Watch query invalid")?
        .sequence)
}
fn discover(manager: &Manager) -> Result<DeviceId, &'static str> {
    let end = deadline()?;
    let observer = manager.observer()?;
    let watch = watch(
        observer,
        Command::WatchScope { scope: Scope::Root },
        manager,
        15,
        end,
    )?;
    loop {
        let seq = sequence(&watch, manager.pid(), end)?;
        let (mut after, mut scan_end) = (0, 0);
        loop {
            now(end)?;
            let reply = call_until(
                observer,
                Command::Enumerate {
                    scope: Scope::Root,
                    after,
                    end: scan_end,
                },
                end,
            )?;
            let body = checked(&reply, manager.pid(), 14, None, 0)?;
            let scan = protocol::decode_scan(body, after, scan_end)
                .map_err(|_| "function check scan invalid")?;
            after = scan.next;
            scan_end = scan.end;
            let Some(id) = scan.device else {
                break;
            };
            let reply = call_until(observer, Command::QueryDevice { id }, end)?;
            if reply.sender_pid != manager.pid() {
                return Err("function check discovery sender mismatch");
            }
            match protocol::decode_device(&reply.payload, id, reply.handles.len()) {
                Ok(info)
                    if info.present
                        && info.identity == libpci::config::XHCI_IDENTITY
                        && info.binding.is_none() =>
                {
                    return Ok(id);
                }
                Ok(_) | Err(Error::StaleInstance) => {}
                Err(_) => return Err("function check device description failed"),
            }
        }
        wait_change(&watch, manager.pid(), seq, end)?;
    }
}
fn phase(
    control: &MailboxSender,
    manager: &Manager,
    reference: BindingRef,
    end: Deadline,
) -> Result<Phase, &'static str> {
    let reply = call_until(control, Command::QueryBinding, end)?;
    let snapshot = protocol::decode_snapshot(checked(&reply, manager.pid(), 8, None, 0)?)
        .map_err(|_| "function check binding snapshot invalid")?;
    if snapshot.reference != reference {
        return Err("function check binding changed identity");
    }
    Ok(snapshot.phase)
}
fn wait_phase(
    control: &MailboxSender,
    watch: &Watch,
    manager: &Manager,
    reference: BindingRef,
    desired: Phase,
    end: Deadline,
) -> Result<(), &'static str> {
    loop {
        now(end)?;
        let seq = sequence(watch, manager.pid(), end)?;
        let current = phase(control, manager, reference, end)?;
        if current == desired {
            return Ok(());
        }
        if current == Phase::Released
            || (desired == Phase::ResourcesReady && current == Phase::Stopping)
        {
            return Err("function check binding stopped prematurely");
        }
        wait_change(watch, manager.pid(), seq, end)?;
    }
}
fn master(
    sender: &MailboxSender,
    provider: &mut Option<u64>,
    end: Deadline,
) -> Result<Option<Error>, &'static str> {
    let mut bytes = [0; protocol::HEADER_LEN];
    let used = Header {
        op: 1,
        error: None,
        deadline: end,
    }
    .encode(&[], &mut bytes)
    .map_err(|_| "function check master encoding failed")?;
    let request = librpc::Request::new(libpci::function::ID, &bytes[..used])
        .map_err(|_| "function check master request failed")?;
    let reply = librpc::Caller::new()
        .call(sender, end, request)
        .map_err(|_| "function check master outcome unknown")?;
    let (header, body) =
        Header::decode(&reply.payload).map_err(|_| "function check master reply invalid")?;
    if reply.sender_pid == 0
        || provider.is_some_and(|pid| pid != reply.sender_pid)
        || header.op != 1
        || !body.is_empty()
        || !reply.handles.is_empty()
    {
        return Err("function check master reply identity mismatch");
    }
    // 原Caller核真实一次回复能力；首份回复取实际执行者PID，后续只核本轮同一执行者。
    *provider = Some(reply.sender_pid);
    Ok(header.error)
}
fn command_address(info: libpci::function::FunctionInfo) -> Result<u64, &'static str> {
    let handle = super::INIT_STARTUP
        .lock()
        .as_ref()
        .ok_or("function check platform missing")?
        ._platform
        .ecam_handle()
        .ok_or("function check ECAM missing")?;
    let handle = object::duplicate(handle, Rights::READ)
        .map_err(|_| "function check ECAM observer failed")?;
    // SAFETY: 原平台已核实际MmioControl；新只读alias的唯一关闭责任移入typed owner。
    let root = unsafe { MmioControl::from_handle(handle) };
    let base = root
        .query()
        .map_err(|_| "function check ECAM geometry failed")?
        .physical;
    base.checked_add(
        info.address
            .offset(4)
            .map_err(|_| "function check BDF invalid")? as u64,
    )
    .ok_or("function check ECAM address overflow")
}
pub(super) fn run(manager: &Manager) -> Result<(), &'static str> {
    let id = discover(manager)?;
    let (source, admin) = manager.controller_source(id)?;
    let mut old: Option<(BindingRef, Arc<MailboxSender>, MailboxSender)> = None;
    let mut provider = None;
    #[cfg(feature = "acceptance-irq-retirement")]
    let mut irq_tail: Option<(libkernel::irq::IrqUse, erhino_shared::irq::IrqToken)> = None;
    for round in 1..=2 {
        let end = deadline()?;
        let reply = call_until(&source, Command::QueryDevice { id }, end)?;
        if reply.sender_pid != manager.pid() {
            return Err("function check query sender mismatch");
        }
        let info = protocol::decode_device(&reply.payload, id, reply.handles.len())
            .map_err(|_| "function check current device unavailable")?;
        if !info.present || info.binding.is_some() {
            return Err("function check device not vacant");
        }
        let reference = BindingRef::new(id, info.epoch.ok_or("function check missing epoch")?)
            .map_err(|_| "function check invalid ref")?;
        let mut reply = call_until(&source, Command::Claim { reference }, end)?;
        let snapshot = protocol::decode_snapshot(checked(&reply, manager.pid(), 6, None, 1)?)
            .map_err(|_| "function check Claim invalid")?;
        if snapshot.reference != reference {
            return Err("function check Claim ref mismatch");
        }
        let (control, _) = MailboxSender::from_capability(
            reply
                .handles
                .take(0)
                .map_err(|_| "function check Control missing")?,
        )
        .map_err(|_| "function check Control role invalid")?;
        let binding_watch = watch(&control, Command::WatchBinding, manager, 16, end)?;
        wait_phase(
            &control,
            &binding_watch,
            manager,
            reference,
            Phase::ResourcesReady,
            end,
        )?;
        empty(
            &call_until(&control, Command::IssueRegistrar, end)?,
            manager.pid(),
            20,
            Some(Error::Permission),
        )?;
        let mut inputs = Inputs::new();
        let mut dma_observer = None;
        for (index, name) in libpci::function::RESOURCES.iter().enumerate() {
            let reply = call_until(&control, Command::GetResource { name }, end)?;
            let body = checked(&reply, manager.pid(), 10, None, 1)?;
            let entry = Entry::decode(body, protocol::HEADER_LEN, reply.handles.len())
                .map_err(|_| "function check resource invalid")?;
            let function_info = libpci::function::FunctionInfo::decode(entry.metadata)
                .map_err(|_| "function check PCI metadata invalid")?;
            if index == 0 {
                debug!(
                    "function session round prepared: round={}, device={}, epoch={}, command={:#x}",
                    round,
                    id.serial(),
                    reference.epoch(),
                    command_address(function_info)?
                );
            }
            if index == 2 {
                entry
                    .descriptor
                    .check_handle(
                        &reply
                            .handles
                            .get(0)
                            .map_err(|_| "function check DMA missing")?
                            .description()
                            .map_err(|_| "function check DMA identity failed")?,
                    )
                    .map_err(|_| "function check DMA descriptor mismatch")?;
                let cap = reply
                    .handles
                    .get(0)
                    .map_err(|_| "function check DMA missing")?
                    .duplicate(Rights::READ | Rights::WAIT)
                    .map_err(|_| "function check DMA observer failed")?;
                if cap
                    .description()
                    .map_err(|_| "function check DMA observer identity failed")?
                    .kind
                    != erhino_shared::object::ObjectKind::DmaUse as u32
                {
                    return Err("function check DMA observer wrong kind");
                }
                // SAFETY: 真实Descriptor与HandleDescribe已核DmaUse，唯一新alias移入只读观察owner。
                dma_observer = Some(unsafe { DmaUse::from_handle(cap.into_raw()) });
            }
            inputs
                .adopt(index, reply)
                .map_err(|_| "function check resource adoption failed")?;
        }
        let dma = dma_observer.ok_or("function check DMA observer missing")?;
        let mut controller =
            Controller::adopt(inputs).map_err(|_| "function check controller adoption failed")?;
        let context = controller
            .function
            .description()
            .map_err(|_| "function check function identity failed")?
            .object_id;
        if let Some((old_ref, old_function, old_control)) = &old {
            if reference.epoch() <= old_ref.epoch() {
                return Err("function check epoch did not advance");
            }
            empty(
                &call_until(
                    old_control,
                    Command::GetResource {
                        name: "pci-function",
                    },
                    end,
                )?,
                manager.pid(),
                10,
                Some(Error::Closed),
            )?;
            if phase(old_control, manager, *old_ref, end)? != Phase::Released {
                return Err("function check old control revived");
            }
            if old_function
                .description()
                .map_err(|_| "function check old alias identity failed")?
                .object_id
                == context
            {
                return Err("function check context reused");
            }
            if master(old_function, &mut provider, end)? != Some(Error::Permission) {
                return Err("function check old context accepted new Prepared");
            }
            debug!(
                "function session old alias rejected: device={}, old_epoch={}, new_epoch={}, context={}, error=Permission",
                id.serial(),
                old_ref.epoch(),
                reference.epoch(),
                old_function
                    .description()
                    .map_err(|_| "function check old context query failed")?
                    .object_id
            );
        }
        loop {
            match controller
                .step(now(end)?)
                .map_err(|_| "function check controller start failed")?
            {
                Progress::Starting => pause(end)?,
                Progress::EnableMaster => {
                    if master(&controller.function, &mut provider, end)?.is_some() {
                        return Err("function check new context rejected");
                    }
                    controller
                        .master_enabled()
                        .map_err(|_| "function check master confirmation failed")?;
                }
                Progress::Ready => break,
                _ => return Err("function check unexpected controller start phase"),
            }
        }
        if dma
            .query()
            .map_err(|_| "function check live DMA query failed")?
            .members
            != 4
        {
            return Err("function check live ring set mismatch");
        }
        debug!(
            "function session controller running: round={}, device={}, epoch={}, context={}, dma_members=4",
            round,
            id.serial(),
            reference.epoch(),
            context
        );
        #[cfg(feature = "acceptance-irq-retirement")]
        {
            // 尚未发行USB session/命令；消耗准备期间的真实端口提示，不将token当事件事实。
            while controller
                .poll_event()
                .map_err(|_| "IRQ preparation event drain failed")?
                .is_some()
            {
                now(end)?;
            }
            controller
                .acknowledge_interrupt()
                .map_err(|_| "IRQ preparation acknowledgement failed")?;
            let info = controller
                .interrupt
                .query()
                .map_err(|_| "IRQ active use query failed")?;
            debug!(
                "IRQ retirement awaiting real insertion: round={}, device={}, epoch={}, source={}, target={}, generation={}",
                round,
                id.serial(),
                reference.epoch(),
                info.scope.first,
                info.scope.targets.trailing_zeros(),
                info.generation
            );
            let observed =
                wait::wait_until(controller.interrupt.handle(), ObjectSignals::READABLE, end)
                    .map_err(|_| "IRQ real insertion wait failed")?;
            if !observed.observed.contains(ObjectSignals::READABLE) {
                return Err("IRQ insertion notification missing");
            }
            // 本消费者尚未Take/Ack：真实READABLE保持期间，让另一正式actor处理原查询。
            // 这证明持续电平下的实际业务推进，不把一次等待调用当作已park证据。
            let reply = call_until(&control, Command::QueryBinding, end)?;
            let snapshot = protocol::decode_snapshot(checked(
                &reply,
                manager.pid(),
                Command::QueryBinding.op(),
                None,
                0,
            )?)
            .map_err(|_| "IRQ persistent-level query snapshot invalid")?;
            if snapshot.reference != reference
                || snapshot.phase != Phase::ResourcesReady
                || !wait::probe(controller.interrupt.handle(), ObjectSignals::READABLE)
                    .map_err(|_| "IRQ persistent-level probe failed")?
                    .contains(ObjectSignals::READABLE)
            {
                return Err("IRQ persistent level or original binding changed during query");
            }
            debug!(
                "IRQ persistent READABLE allowed Control progress: round={}, device={}, epoch={}, source={}, generation={}",
                round,
                reference.device().serial(),
                reference.epoch(),
                info.scope.first,
                info.generation
            );
            let token = controller
                .interrupt
                .take()
                .map_err(|_| "IRQ actual Take failed")?
                .ok_or("IRQ insertion had no real token")?;
            debug!(
                "IRQ retirement real token held: round={}, generation={}, sequence={}",
                round, token.generation, token.sequence
            );
            if round == 1 {
                let handle = object::duplicate(
                    controller.interrupt.handle(),
                    Rights::READ | Rights::WRITE | Rights::WAIT,
                )
                .map_err(|_| "IRQ retained use duplication failed")?;
                // SAFETY: Inputs已用actualHandleDescribe核原IrqUse；正式duplicate保持类型，
                // 仅缩权READ/WRITE/WAIT，新alias唯一关闭责任移入typed owner。
                irq_tail = Some((
                    unsafe { libkernel::irq::IrqUse::from_handle(handle) },
                    token,
                ));
            } else {
                let (old_use, old_token) = irq_tail.as_ref().ok_or("IRQ old token missing")?;
                if old_token.generation == token.generation {
                    return Err("IRQ generation reused");
                }
                if old_use.ack(*old_token)
                    != Err(erhino_shared::call::SystemCallError::ObjectClosed)
                {
                    return Err("IRQ retired use accepted old Ack");
                }
                if controller.interrupt.ack(*old_token)
                    != Err(erhino_shared::call::SystemCallError::IllegalArgument)
                {
                    return Err("IRQ old token affected live new use");
                }
                debug!(
                    "IRQ retirement old token rejected while new taken: old_generation={}, old_sequence={}, new_generation={}, new_sequence={}",
                    old_token.generation, old_token.sequence, token.generation, token.sequence
                );
                controller
                    .interrupt
                    .ack(token)
                    .map_err(|_| "IRQ new token Ack failed")?;
                debug!(
                    "IRQ retirement new token acknowledged: generation={}, sequence={}",
                    token.generation, token.sequence
                );
            }
        }
        empty(
            &call_until(&control, Command::StopBinding, end)?,
            manager.pid(),
            11,
            None,
        )?;
        loop {
            match master(&controller.function, &mut provider, end)? {
                Some(Error::Closed) => break,
                None => pause(end)?,
                _ => return Err("function check stopped function returned unexpected status"),
            }
        }
        debug!(
            "function session provider Stop observed: round={}, device={}, epoch={}, context={}, error=Closed",
            round,
            id.serial(),
            reference.epoch(),
            context
        );
        // SAFETY: 此消费者从未准入USB子session/TD/command，所有成员为空；仍由原Controller核usb_idle。
        unsafe { controller.begin_stop() }
            .map_err(|_| "function check controller stop admission failed")?;
        loop {
            match controller
                .step(now(end)?)
                .map_err(|_| "function check controller stop failed")?
            {
                Progress::Stopping => pause(end)?,
                Progress::Closed => break,
                _ => return Err("function check unexpected controller stop phase"),
            }
        }
        let result = wait::wait_until(dma.handle(), ObjectSignals::DONE, end)
            .map_err(|_| "function check DMA completion wait failed")?;
        if !result.observed.contains(ObjectSignals::DONE)
            || dma
                .query()
                .map_err(|_| "function check retired DMA query failed")?
                .members
                != 0
        {
            return Err("function check DMA did not retire");
        }
        let retained = controller.function.clone();
        // SAFETY: Controller原Closed已证明HCH/残事件/四份DMA DONE；ReportStopped尚未发，上级仍持MMIO。
        unsafe { controller.unmap() }.map_err(|_| "function check controller unmap failed")?;
        debug!(
            "function session hardware stopped: round={}, device={}, epoch={}, dma_members=0, unmapped=true",
            round,
            id.serial(),
            reference.epoch()
        );
        empty(
            &call_until(&control, Command::ReportStopped, end)?,
            manager.pid(),
            12,
            None,
        )?;
        wait_phase(
            &control,
            &binding_watch,
            manager,
            reference,
            Phase::Released,
            end,
        )?;
        debug!(
            "function session Binding released: round={}, device={}, epoch={}, context={}",
            round,
            id.serial(),
            reference.epoch(),
            context
        );
        if round == 1 {
            old = Some((reference, retained, control));
        }
    }
    let end = deadline()?;
    empty(
        &call_until(&admin, Command::SealSource, end)?,
        manager.pid(),
        2,
        None,
    )?;
    loop {
        let reply = call_until(&admin, Command::QuerySourceStop, end)?;
        if reply.sender_pid != manager.pid() {
            return Err("function check source query sender mismatch");
        }
        match protocol::decode_source_stop(&reply.payload, reply.handles.len())
            .map_err(|_| "function check source closure invalid")?
        {
            SourceStop::Closed => break,
            SourceStop::Stopping => pause(end)?,
            SourceStop::Open => return Err("function check source reopened"),
        }
    }
    if let Some((_, function, control)) = old {
        Arc::try_unwrap(function)
            .map_err(|_| "function check unexpected function owner")?
            .close()
            .map_err(|_| "function check old function close failed")?;
        control
            .close()
            .map_err(|_| "function check old Control close failed")?;
    }
    #[cfg(feature = "acceptance-irq-retirement")]
    drop(irq_tail);
    drop(source);
    drop(admin);
    debug!(
        "function session checks passed: two epochs, old aliases isolated, real controller stop and source Closed"
    );
    Ok(())
}
