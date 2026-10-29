//! 出生输入采用与进程角色分派；命名出生契约接通前只接受 init 签发的私有帧。
use super::*;
use libkernel::shared::memory_pool::MemoryPoolSnapshot;
use libkernel::shared::object::ObjectKind;
use libprocess::context::{CurrentProcessInfo, InitialGrants};
use libprocess::image::{IMAGE_RIGHTS, Image, ImageView};
use test_kernel::birth::{Frame, Input, Role};

/// 平台根别名的权利；与 init 签发时一致，采用时逐项核验。
pub(super) const PLATFORM_ROOT_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
/// retention Pool 与保留子进程付款 Pool 的权利；不含 TRANSIT，别名不再外传。
const RETENTION_POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw() | Rights::CREATE.raw() | Rights::DUPLICATE.raw() | Rights::GRANT.raw(),
);

struct Context {
    process: CurrentProcessInfo,
    pool: Handle,
    mode: Mode,
    role: Role,
    roots: [Option<Handle>; 3],
    retention: Option<Handle>,
    self_image: Option<(usize, usize)>,
    target: Option<(Handle, usize)>,
    /// 最近一次保留子进程收集后的 retention 快照与已运行的保留 fixture 数。
    retained: Option<(MemoryPoolSnapshot, u64)>,
}
static CONTEXT: Mutex<SimpleLock, Option<Context>> = Mutex::const_new(SimpleLock::new(), None);

fn with<R>(f: impl FnOnce(&Context) -> R) -> R {
    f(CONTEXT
        .lock()
        .as_ref()
        .expect("kernel acceptance startup missing"))
}
pub(super) fn current_process() -> CurrentProcessInfo {
    with(|ctx| ctx.process)
}
pub(super) fn current_pid() -> Pid {
    current_process().pid()
}
pub(super) fn memory_pool() -> Handle {
    with(|ctx| ctx.pool)
}
pub(super) fn mode() -> Mode {
    with(|ctx| ctx.mode)
}
/// 本进程是否为保留 fixture 子进程。
pub(super) fn retention_child() -> bool {
    with(|ctx| ctx.role != Role::Main)
}
pub(super) fn ecam_handle() -> Option<Handle> {
    with(|ctx| ctx.roots[0])
}
pub(super) fn irq_handle() -> Option<Handle> {
    with(|ctx| ctx.roots[1])
}
pub(super) fn dma_handle() -> Option<Handle> {
    with(|ctx| ctx.roots[2])
}

fn take(grants: &mut InitialGrants, frame: &Frame, input: Input) -> Option<Capability> {
    frame.slot(input).map(|slot| {
        grants
            .take(slot as usize)
            .expect("kernel acceptance input missing or duplicated")
    })
}
fn checked(cap: Capability, kind: ObjectKind, rights: Rights) -> Capability {
    let description = cap
        .description()
        .expect("kernel acceptance input query failed");
    assert_eq!(
        description.kind, kind as u32,
        "kernel acceptance input kind mismatch"
    );
    assert_eq!(
        description.rights, rights,
        "kernel acceptance input rights mismatch"
    );
    cap
}
fn image(grants: &mut InitialGrants, frame: &Frame, input: Input) -> Option<(Image, ImageView)> {
    let length = frame.image_length(input);
    let capability = take(grants, frame, input);
    assert_eq!(
        capability.is_some(),
        length != 0,
        "kernel acceptance image length mismatch"
    );
    capability.map(|cap| {
        let image = Image::adopt(
            cap,
            usize::try_from(length).expect("kernel acceptance image too long"),
        )
        .expect("kernel acceptance image invalid");
        let view = image.map().expect("kernel acceptance image mapping failed");
        (image, view)
    })
}

/// 采用全部出生输入后按角色分派；任何输入不符即 panic，由 init 判为验收失败。
pub(super) fn run() {
    let (process, payload, mut grants) = libruntime::startup::take()
        .expect("kernel acceptance startup missing")
        .into_parts();
    let frame = Frame::decode(payload.as_bytes()).expect("kernel acceptance birth frame invalid");
    let pool_rights = if frame.role == Role::Main {
        RETENTION_POOL_RIGHTS | Rights::TRANSIT
    } else {
        RETENTION_POOL_RIGHTS
    };
    let pool = checked(
        take(&mut grants, &frame, Input::Pool).expect("kernel acceptance Pool missing"),
        ObjectKind::MemoryPool,
        pool_rights,
    );
    let job = checked(
        take(&mut grants, &frame, Input::Job).expect("kernel acceptance Job missing"),
        ObjectKind::Job,
        JOB_FULL_RIGHTS,
    );
    let reset = take(&mut grants, &frame, Input::Reset)
        .map(|cap| checked(cap, ObjectKind::SystemReset, Rights::DUPLICATE));
    let retention = take(&mut grants, &frame, Input::Retention)
        .map(|cap| checked(cap, ObjectKind::MemoryPool, RETENTION_POOL_RIGHTS));
    let self_image = image(&mut grants, &frame, Input::SelfImage);
    let report = take(&mut grants, &frame, Input::Report);
    let pm = image(&mut grants, &frame, Input::Pm);
    let target = image(&mut grants, &frame, Input::Target);
    let hammer = image(&mut grants, &frame, Input::Hammer);
    let roots: [Option<Capability>; 3] = core::array::from_fn(|index| {
        let (input, kind) = [
            (Input::Mmio, ObjectKind::MmioControl),
            (Input::Irq, ObjectKind::IrqControl),
            (Input::Dma, ObjectKind::DmaDomainControl),
        ][index];
        take(&mut grants, &frame, input).map(|cap| checked(cap, kind, PLATFORM_ROOT_RIGHTS))
    });
    assert_eq!(grants.remaining(), 0, "kernel acceptance has extra inputs");
    *CONTEXT.lock() = Some(Context {
        process,
        pool: pool.as_handle(),
        mode: frame.mode,
        role: frame.role,
        roots: core::array::from_fn(|index| roots[index].as_ref().map(Capability::as_handle)),
        retention: retention.as_ref().map(Capability::as_handle),
        self_image: self_image
            .as_ref()
            .map(|(_, view)| (view.bytes().as_ptr() as usize, view.bytes().len())),
        target: target
            .as_ref()
            .map(|(image, view)| (image.handle(), view.bytes().len())),
        retained: None,
    });
    let fixture = match frame.role {
        Role::Main => None,
        Role::RetainExposed => Some(b"dma-exposed".as_slice()),
        Role::RetainFault => Some(b"dma-output-fault".as_slice()),
    };
    if let Some(fixture) = fixture {
        log_retention("birth");
        dma_checks::process_fixture(
            dma_handle().expect("retention DMA root missing"),
            job.as_handle(),
            target.as_ref().expect("retention target missing").1.bytes(),
            fixture,
        )
        .expect("retention fixture failed");
        debug!(
            "DMA retention fixture completed: mode={}",
            core::str::from_utf8(fixture).expect("literal mode is UTF-8")
        );
        return;
    }
    let mut harness = Harness::new();
    if let Err(message) = super::run(
        &mut harness,
        job.as_handle(),
        pm.as_ref().map_or(&[], |(_, view)| view.bytes()),
        target.as_ref().map_or(&[], |(_, view)| view.bytes()),
        hammer.as_ref().map(|(_, view)| view.bytes()),
    ) {
        panic!("kernel acceptance failed: {}", message);
    }
    harness
        .collect_job(job.as_handle())
        .expect("kernel acceptance Job collection failed");
    check_reset_authority(
        job.as_handle(),
        reset
            .as_ref()
            .expect("kernel acceptance reset alias missing")
            .as_handle(),
    );
    if let Some((snapshot, fixtures)) = with(|ctx| ctx.retained) {
        // 每个保留 fixture 跨 reset 留下的页数由 dma_checks 拥有；init 只比对报告后不变。
        assert_eq!(
            snapshot.allocated,
            fixtures * dma_checks::RETAINED_PAGES_PER_FIXTURE,
            "retention Pool did not preserve exactly the retained DMA pages"
        );
        let bytes = test_kernel::report::encode_pool(snapshot);
        // SAFETY: report 是本进程出生专属 sender，无能力移交；错误使验收失败。
        unsafe {
            send_raw(
                report
                    .as_ref()
                    .expect("kernel acceptance report missing")
                    .as_handle(),
                test_kernel::report::KIND,
                &bytes,
                &[],
            )
        }
        .expect("kernel acceptance retention report failed");
    }
    debug!("kernel acceptance completed");
}

/// SystemReset 的对象类型与权限负路径：Job 句柄类型不符，衰减别名权利不足；不提交成功动作。
fn check_reset_authority(job: Handle, reset: Handle) {
    assert!(matches!(
        system::reset(job, ResetAction::Shutdown, ResetReason::Requested),
        Err(SystemCallError::WrongObjectType)
    ));
    assert!(matches!(
        system::reset(reset, ResetAction::Shutdown, ResetReason::Requested),
        Err(SystemCallError::RightsDenied)
    ));
    debug!("system reset authority checks passed");
}

/// 在绑定 retention Pool 的自启动子进程中运行跨 reset 保留的 fixture；主 Pool 不留下跨收集计费。
pub(super) fn retained_fixture(job: Handle, fixture: &[u8]) -> Result<(), &'static str> {
    use libprocess::GrantOutcome;
    let (pool, (address, length), (target, target_length)) = with(|ctx| {
        Ok((
            ctx.retention.ok_or("retention Pool missing")?,
            ctx.self_image.ok_or("retention self image missing")?,
            ctx.target.ok_or("retention target image missing")?,
        ))
    })?;
    let role = if fixture == b"dma-output-fault" {
        Role::RetainFault
    } else {
        Role::RetainExposed
    };
    let mut frame = Frame::new(Mode(0), role);
    frame.set_image_length(Input::Target, target_length as u64);
    let mut owners = alloc::vec::Vec::new();
    let mut grants = alloc::vec::Vec::new();
    for (input, source, rights) in [
        (Input::Pool, pool, RETENTION_POOL_RIGHTS),
        (Input::Job, job, JOB_FULL_RIGHTS),
        (Input::Target, target, IMAGE_RIGHTS),
        (
            Input::Dma,
            dma_handle().ok_or("retention DMA root missing")?,
            PLATFORM_ROOT_RIGHTS,
        ),
    ] {
        let handle = duplicate(source, rights).map_err(|_| "retention grant duplicate failed")?;
        // SAFETY: duplicate 返回的新表项由此唯一采用，按 GrantOutcome 兑现移交。
        owners.push(unsafe { Capability::from_raw(handle) });
        frame.set_slot(input, grants.len() as u8);
        grants.push(HandleGrant { handle, rights });
    }
    let payload = frame.encode();
    // SAFETY: self_image 与只读 ImageView 由 run 栈持有至进程结束，已核 sealed 长度。
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    let result = spawn(SpawnRequest {
        memory_pool: pool,
        parent: current_process(),
        job,
        image: bytes,
        payload: &payload,
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    });
    if result.is_ok() || matches!(&result, Err(failure) if failure.grants == GrantOutcome::Consumed)
    {
        for owner in owners {
            owner.into_raw();
        }
    }
    let started = result.map_err(|_| "retention child spawn failed")?;
    let done = collect_process(
        SupervisionTarget::new(started.pid, started.control),
        DEFAULT_SUPERVISION_POLICY,
    )
    .unwrap_or_else(|failure| panic!("retention child collection failed: {:?}", failure));
    if done.snapshot.cause != TerminationCause::Completed as u32
        || done.snapshot.detail != libkernel::shared::proc::TerminationDetail::NONE
    {
        return Err("retention child did not complete normally");
    }
    let snapshot =
        libkernel::memory_pool::query(pool).map_err(|_| "retention collected Pool query failed")?;
    debug!(
        "DMA retention Pool sample: phase=child-collected, pid={}, pages={}, metadata_slots={}",
        started.pid, snapshot.allocated, snapshot.metadata.allocated
    );
    let mut guard = CONTEXT.lock();
    let ctx = guard.as_mut().expect("kernel acceptance startup missing");
    let fixtures = ctx.retained.map_or(0, |(_, count)| count) + 1;
    ctx.retained = Some((snapshot, fixtures));
    Ok(())
}

/// 保留子进程内的 retention 取证点；子进程的付款 Pool 即 retention Pool。
pub(super) fn log_retention(phase: &str) {
    let snapshot =
        libkernel::memory_pool::query(memory_pool()).expect("retention Pool sample failed");
    debug!(
        "DMA retention Pool sample: phase={}, pid={}, pages={}, metadata_slots={}",
        phase,
        current_pid(),
        snapshot.allocated,
        snapshot.metadata.allocated
    );
}
