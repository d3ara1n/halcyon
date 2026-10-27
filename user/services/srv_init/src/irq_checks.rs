//! 真实平台 IRQ 根的授权与收束验收；硬件事件交付由后续 xHCI 消费者验证。
use erhino_shared::{
    call::SystemCallError as Error,
    irq::{IrqScope, IrqToken},
    object::{Handle, ObjectSignals, Rights},
    wait_set::MemberRequest,
};
use libkernel::{
    ipc::{object, wait, wait_set::WaitSet},
    irq::IrqControl,
};
const FULL: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
fn done(handle: Handle) -> Result<(), &'static str> {
    let result =
        wait::wait_one(handle, ObjectSignals::DONE, 0).map_err(|_| "IRQ DONE wait failed")?;
    if !result.observed.contains(ObjectSignals::DONE) {
        return Err("IRQ completion omitted DONE");
    }
    Ok(())
}
pub(super) fn run(root: Handle) -> Result<(), &'static str> {
    let full = FULL;
    let duplicate = object::duplicate(root, full).map_err(|_| "IRQ root duplicate failed")?;
    // SAFETY: 平台采用时已核角色；新 duplicate 的关闭责任移入。
    let root = unsafe { IrqControl::from_handle(duplicate) };
    let available = root.query().map_err(|_| "IRQ root query failed")?.scope;
    let target = available.targets.trailing_zeros();
    let scope = root
        .create_control(
            IrqScope {
                first: available.first,
                count: 1,
                targets: available.targets,
            },
            full,
        )
        .map_err(|_| "IRQ control create failed")?;
    let usage = scope
        .create_use(available.first, target, full & !Rights::CREATE)
        .map_err(|_| "IRQ use create failed")?;
    if !matches!(
        scope.create_use(available.first, target, full & !Rights::CREATE),
        Err(Error::ObjectBusy)
    ) {
        return Err("IRQ source admitted two consumers");
    }
    if !matches!(
        root.create_control(
            IrqScope {
                first: available.first - 1,
                count: 1,
                targets: available.targets
            },
            full
        ),
        Err(Error::RightsDenied)
    ) {
        return Err("IRQ control widened its source range");
    }
    let generation = usage
        .query()
        .map_err(|_| "IRQ use query failed")?
        .generation;
    if generation == 0 {
        return Err("IRQ use generation is zero");
    }
    // 此处未初始化任何 PCI 中断设备；只验证空消费与伪造 token 的拒绝。
    if usage.take().map_err(|_| "IRQ empty Take failed")?.is_some() {
        return Err("unexpected PCI IRQ before device activation");
    }
    let invalid = IrqToken {
        generation,
        sequence: 1,
    };
    if usage.ack(invalid) != Err(Error::IllegalArgument) {
        return Err("IRQ Ack accepted an unclaimed event");
    }
    let set = WaitSet::create(2).map_err(|_| "IRQ WaitSet create failed")?;
    let removed = set
        .register(MemberRequest::new(
            usage.handle(),
            ObjectSignals::READABLE,
            1,
        ))
        .map_err(|_| "IRQ wait registration failed")?;
    set.remove(removed)
        .map_err(|_| "IRQ wait cancellation failed")?;
    let token = set
        .register(MemberRequest::new(usage.handle(), ObjectSignals::DONE, 2))
        .map_err(|_| "IRQ DONE registration failed")?;
    scope.seal().map_err(|_| "IRQ Seal failed")?;
    if !matches!(
        scope.create_use(available.first, target, full & !Rights::CREATE),
        Err(Error::ObjectClosed)
    ) {
        return Err("sealed IRQ control admitted a use");
    }
    if usage
        .take()
        .map_err(|_| "IRQ Seal invalidated existing use")?
        .is_some()
    {
        return Err("unexpected IRQ after Seal");
    }
    scope.revoke().map_err(|_| "IRQ Revoke failed")?;
    done(scope.handle())?;
    done(usage.handle())?;
    if usage.take() != Err(Error::ObjectClosed) || usage.ack(invalid) != Err(Error::ObjectClosed) {
        return Err("closed IRQ use still accepts operations");
    }
    set.wait(libkernel::time::Deadline::INFINITE)
        .map_err(|_| "IRQ WaitSet wake failed")?;
    let ready = set.receive(2).map_err(|_| "IRQ WaitSet receive failed")?;
    if !ready.iter().any(|item| {
        item.token == token && item.cookie == 2 && item.observed.contains(ObjectSignals::DONE)
    }) {
        return Err("IRQ WaitSet lost completion");
    }
    set.remove(token)
        .map_err(|_| "IRQ completion detach failed")?;
    // 完成观察壳仍存在时，source 已经可以交给新 use。
    let next = root
        .create_use(available.first, target, full & !Rights::CREATE)
        .map_err(|_| "IRQ source reuse failed")?;
    if next
        .query()
        .map_err(|_| "IRQ next query failed")?
        .generation
        == generation
    {
        return Err("IRQ generation was reused");
    }
    if next.ack(invalid) != Err(Error::IllegalArgument) {
        return Err("old IRQ token affected a new use");
    }
    let observer = WaitSet::create(1).map_err(|_| "IRQ close observer create failed")?;
    observer
        .register(MemberRequest::new(next.handle(), ObjectSignals::DONE, 3))
        .map_err(|_| "IRQ close observer register failed")?;
    drop(next);
    observer
        .wait(libkernel::time::Deadline::INFINITE)
        .map_err(|_| "IRQ last Close did not wake observer")?;
    let ready = observer
        .receive(1)
        .map_err(|_| "IRQ close observation failed")?;
    if ready.len() != 1 || ready[0].cookie != 3 || !ready[0].observed.contains(ObjectSignals::DONE)
    {
        return Err("IRQ last Close did not complete");
    }
    let final_use = root
        .create_use(available.first, target, full & !Rights::CREATE)
        .map_err(|_| "IRQ last Close retained source")?;
    final_use.revoke().map_err(|_| "IRQ final revoke failed")?;
    done(final_use.handle())?;
    libkernel::debug!(
        "IRQ platform acceptance passed: exclusive source, Seal, Revoke, WaitSet and last Close"
    );
    Ok(())
}

#[cfg(feature = "acceptance-irq-exit")]
pub(super) fn process_retirement(
    root: Handle,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    use erhino_shared::proc::{HandleGrant, ProcessState, TerminationCause, TerminationDetail};
    use libkernel::{
        ipc::{capability::Capability, notification},
        memory_pool::{self, MemoryPool},
        process, time,
    };
    use libprocess::{GrantOutcome, SpawnRequest, spawn};

    let handle = object::duplicate(root, FULL).map_err(|_| "IRQ exit root duplicate failed")?;
    // SAFETY: 启动采用已核IrqControl；新alias关闭责任唯一移入。
    let root = unsafe { IrqControl::from_handle(handle) };
    let available = root
        .query()
        .map_err(|_| "IRQ exit root query failed")?
        .scope;
    for mode in [b"irq-wait-exit".as_slice(), b"irq-set-exit".as_slice()] {
        let label = core::str::from_utf8(mode).map_err(|_| "IRQ exit mode invalid")?;
        // 整个消费者的防挂死预算，不作为IRQ完成或性能期限。
        let end = time::timeout_millis(60_000).map_err(|_| "IRQ exit deadline failed")?;
        let pool: MemoryPool = memory_pool::derive(
            super::root_memory_pool(),
            libdriver::boot::WORKER_POOL,
            Rights::READ | Rights::CREATE,
        )
        .map_err(|_| "IRQ exit Pool admission failed")?;
        let scope = root
            .create_control(
                IrqScope {
                    first: available.first,
                    count: 1,
                    targets: available.targets,
                },
                FULL,
            )
            .map_err(|_| "IRQ exit scope admission failed")?;
        let usage = scope
            .create_use(
                available.first,
                available.targets.trailing_zeros(),
                FULL & !Rights::CREATE,
            )
            .map_err(|_| "IRQ exit use admission failed")?;
        // 母本保留到退出/loan观察之后；子退出不能靠最后use Close代替摘链。
        let info = usage.query().map_err(|_| "IRQ exit use query failed")?;
        if usage
            .take()
            .map_err(|_| "IRQ exit empty Take failed")?
            .is_some()
        {
            return Err("IRQ exit fixture has an unexpected hardware event");
        }
        let alias = object::duplicate(usage.handle(), Rights::READ | Rights::WAIT | Rights::GRANT)
            .map_err(|_| "IRQ exit use alias failed")?;
        // SAFETY: duplicate独占的新entry；随后按GrantOutcome交接。
        let granted = unsafe { Capability::from_raw(alias) };
        let ready =
            notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT)
                .map_err(|_| "IRQ exit readiness admission failed")?;
        let result = spawn(SpawnRequest {
            job,
            parent: super::init_process_info(),
            memory_pool: pool.as_handle(),
            image,
            payload: mode,
            grants: &[
                HandleGrant {
                    handle: granted.as_handle(),
                    rights: Rights::READ | Rights::WAIT,
                },
                HandleGrant {
                    handle: ready.peer.as_handle(),
                    rights: Rights::SIGNAL,
                },
            ],
            control_rights: super::SUPERVISOR_RIGHTS,
        });
        if result.is_ok()
            || matches!(&result, Err(failure) if failure.grants == GrantOutcome::Consumed)
        {
            granted.into_raw();
            ready.peer.into_raw();
        }
        let started = result.map_err(|_| "IRQ exit target birth failed")?;
        libkernel::debug!(
            "IRQ exit target admitted: mode={}, pid={}, source={}, generation={}",
            label,
            started.pid,
            info.scope.first,
            info.generation
        );
        let result = (|| {
            let observed = wait::wait_until(ready.owner.as_handle(), ObjectSignals::READABLE, end)
                .map_err(|_| "IRQ exit target readiness wait failed")?;
            if !observed.observed.contains(ObjectSignals::READABLE) {
                return Err("IRQ exit target omitted readiness");
            }
            let state =
                process::query(started.control).map_err(|_| "IRQ exit target query failed")?;
            if state.state != ProcessState::Running as u32
                || state.cause != TerminationCause::None as u32
                || state.detail != TerminationDetail::NONE
            {
                return Err("IRQ exit target terminated before the controlled Kill");
            }
            // readiness不证明park；Kill前按变体核Waiting/source或真实Set/Line成员。
            if process::kill(started.control).map_err(|_| "IRQ exit target Kill failed")?
                != process::KillOutcome::Accepted
            {
                return Err("IRQ exit target was already terminating");
            }
            let observed = wait::wait_until(started.control, ObjectSignals::DONE, end)
                .map_err(|_| "IRQ exit target DONE wait failed")?;
            if !observed.observed.contains(ObjectSignals::DONE) {
                return Err("IRQ exit target omitted DONE");
            }
            let state =
                process::query(started.control).map_err(|_| "IRQ exit final query failed")?;
            if state.state != ProcessState::Dead as u32
                || state.cause != TerminationCause::Killed as u32
                || state.detail != TerminationDetail::PROCESS_CONTROL_KILL
            {
                return Err("IRQ exit target has an unexpected termination cause");
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = process::kill(started.control);
            let _ = process::wait_done(started.control);
        }
        // SAFETY: 此控制只有本消费者关闭；正常已DONE，失败仍由原Job/外层终局持有进程。
        unsafe { object::close(started.control) }.map_err(|_| "IRQ exit control close failed")?;
        result?;
        loop {
            let s = pool.query().map_err(|_| "IRQ exit refund query failed")?;
            if s.allocated == 0
                && s.delegated == 0
                && s.reserved == 0
                && s.metadata.allocated == 0
                && s.metadata.delegated == 0
                && s.metadata.reserved == 0
            {
                break;
            }
            if time::expired(end).map_err(|_| "IRQ exit refund clock failed")? {
                return Err("IRQ exit target Pool remains charged");
            }
            libkernel::thread::yield_now().map_err(|_| "IRQ exit refund yield failed")?;
        }
        // 原母本仍在；这里的第二次Query供只读取证关联CPU/Set成员收束后的原Line。
        let after = usage
            .query()
            .map_err(|_| "IRQ exit retained use query failed")?;
        if after.generation != info.generation {
            return Err("IRQ exit changed the retained use identity");
        }
        scope.revoke().map_err(|_| "IRQ exit scope Revoke failed")?;
        for handle in [usage.handle(), scope.handle()] {
            let observed = wait::wait_until(handle, ObjectSignals::DONE, end)
                .map_err(|_| "IRQ exit source completion wait failed")?;
            if !observed.observed.contains(ObjectSignals::DONE) {
                return Err("IRQ exit source omitted DONE");
            }
        }
        libkernel::debug!(
            "IRQ exit target closed: mode={}, pid={}, source={}, generation={}, pool={}, pages=0/0/0, metadata=0/0/0, scope=DONE",
            label,
            started.pid,
            info.scope.first,
            info.generation,
            pool.query()
                .map_err(|_| "IRQ exit Pool identity failed")?
                .identity
        );
    }
    libkernel::debug!("IRQ process retirement acceptance passed: WaitOne and WaitSet");
    Ok(())
}
