//! 出生输入采用与进程角色分派：主验收进程由组态启动，保留 fixture 子进程由主进程自启动。
use super::*;
use libkernel::shared::object::ObjectKind;
use libpci::platform::{DESCRIPTION_RIGHTS, DescriptionObject, Platform, ROOT_RIGHTS};
use libprocess::birth::Birth;
use libprocess::context::CurrentProcessInfo;
use libprocess::image::{IMAGE_RIGHTS, Image, ImageView};
use libservice::instance;

/// 平台根别名的权利；与组态默认签发一致。
pub(super) const PLATFORM_ROOT_RIGHTS: Rights = ROOT_RIGHTS;
/// 主付款 Pool 的权利：race 需 TRANSIT 把付款授权交给 hammer。
const MAIN_POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
/// retention Pool 与保留子进程付款 Pool 的权利。
const RETENTION_POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw() | Rights::CREATE.raw() | Rights::DUPLICATE.raw() | Rights::GRANT.raw(),
);
const POOL: &str = "memory.pool";
const JOB: &str = "kernel.job";
const RETENTION: &str = "kernel.retention";
const DMA_ROOT: &str = "kernel.dma";
const IMAGE_PM: &str = "kernel.image.srv_pm";
const IMAGE_TARGET: &str = "kernel.image.test_target";
const IMAGE_HAMMER: &str = "kernel.image.test_hammer";
const IMAGE_SELF: &str = "kernel.image.test_kernel";

/// 部署模式位，由组态设定 `mode` 列表选择。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Mode(u32);
impl Mode {
    pub const STRESS: u32 = 1;
    pub const MEMORY: u32 = 1 << 1;
    pub const MMIO_PUBLICATION: u32 = 1 << 2;
    pub const MMIO_FORCED: u32 = 1 << 3;
    pub const DMA_RACES: u32 = 1 << 4;
    pub const DMA_SHARED: u32 = 1 << 5;
    pub const DMA_QUOTA: u32 = 1 << 6;
    pub const DMA_INSTALL_FAILURE: u32 = 1 << 7;
    pub const IRQ_EXIT: u32 = 1 << 8;
    const NAMES: [(&'static str, u32); 9] = [
        ("stress", Self::STRESS),
        ("memory", Self::MEMORY | Self::STRESS),
        ("mmio_publication", Self::MMIO_PUBLICATION),
        ("mmio_forced", Self::MMIO_FORCED),
        ("dma_races", Self::DMA_RACES),
        ("dma_shared", Self::DMA_SHARED),
        ("dma_quota", Self::DMA_QUOTA),
        ("dma_install_failure", Self::DMA_INSTALL_FAILURE),
        ("irq_exit", Self::IRQ_EXIT),
    ];
    pub fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
    pub fn stress(self) -> bool {
        self.has(Self::STRESS)
    }
    /// memory 部署复用 stress 的线程套件。
    pub fn memory(self) -> bool {
        self.has(Self::MEMORY)
    }
}

/// 进程角色：主验收进程，或主进程为跨 reset 保留 fixture 自启动的子进程。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Main,
    RetainExposed,
    RetainFault,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::RetainExposed => "retain_exposed",
            Self::RetainFault => "retain_fault",
        }
    }
}

struct Settings {
    mode: Mode,
    role: Role,
}
fn parse_settings(bytes: &[u8]) -> Settings {
    let root = settings::Settings::parse(bytes)
        .expect("kernel acceptance settings invalid")
        .root();
    root.only(&["mode", "role"])
        .expect("kernel acceptance settings key unknown");
    let mut mode = 0;
    if let Some(list) = root.get("mode") {
        for value in list.as_list().expect("mode is a list").iter() {
            let name = value.as_str().expect("mode entry is a string");
            mode |= Mode::NAMES
                .iter()
                .find(|(text, _)| *text == name)
                .expect("mode entry unknown")
                .1;
        }
    }
    let role = match root.get("role").map(|value| value.as_str()) {
        None => Role::Main,
        Some(Ok("retain_exposed")) => Role::RetainExposed,
        Some(Ok("retain_fault")) => Role::RetainFault,
        _ => panic!("kernel acceptance role invalid"),
    };
    Settings {
        mode: Mode(mode),
        role,
    }
}

struct Context {
    process: CurrentProcessInfo,
    pool: Handle,
    mode: Mode,
    child: bool,
    roots: [Option<Handle>; 3],
    retention: Option<Handle>,
    self_image: Option<(usize, usize)>,
    target: Option<Handle>,
    /// 最近一次保留子进程收集后的 retention 页数与已运行的保留 fixture 数。
    retained: Option<(u64, u64)>,
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
    with(|ctx| ctx.child)
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

fn image(birth: &mut Birth, name: &str) -> Option<(Image, ImageView)> {
    birth
        .take(name, ObjectKind::MemoryObject, IMAGE_RIGHTS)
        .expect("kernel acceptance image input invalid")
        .map(|cap| {
            let image = Image::adopt_sealed(cap).expect("kernel acceptance image invalid");
            let view = image.map().expect("kernel acceptance image mapping failed");
            (image, view)
        })
}

/// 采用全部出生输入后按角色分派；任何输入不符即 panic，由 init 判为验收失败。
pub(super) fn run() {
    let mut birth =
        Birth::adopt(libruntime::startup::take().expect("kernel acceptance startup missing"))
            .expect("kernel acceptance birth frame invalid");
    let settings = parse_settings(birth.settings());
    // task 不报告就绪；生命周期输入只取用即关闭。
    for (name, kind, rights) in [
        (
            instance::REPORT,
            instance::REPORT_KIND,
            instance::REPORT_RIGHTS,
        ),
        (instance::STOP, instance::STOP_KIND, instance::STOP_RIGHTS),
    ] {
        drop(
            birth
                .take(name, kind, rights)
                .expect("kernel acceptance lifecycle input invalid"),
        );
    }
    let child = settings.role != Role::Main;
    let pool = birth
        .require(
            POOL,
            ObjectKind::MemoryPool,
            if child {
                RETENTION_POOL_RIGHTS
            } else {
                MAIN_POOL_RIGHTS
            },
        )
        .expect("kernel acceptance Pool input invalid");
    let job = birth
        .require(JOB, ObjectKind::Job, JOB_FULL_RIGHTS)
        .expect("kernel acceptance Job input invalid");
    let retention = birth
        .take(RETENTION, ObjectKind::MemoryPool, RETENTION_POOL_RIGHTS)
        .expect("kernel acceptance retention input invalid");
    let self_image = image(&mut birth, IMAGE_SELF);
    let pm = image(&mut birth, IMAGE_PM);
    let target = image(&mut birth, IMAGE_TARGET);
    let hammer = image(&mut birth, IMAGE_HAMMER);
    // 主进程按平台描述识别 ECAM/IRQ/DMA 根；保留子进程只收 DMA 根别名。
    let child_dma = birth
        .take(DMA_ROOT, ObjectKind::DmaDomainControl, PLATFORM_ROOT_RIGHTS)
        .expect("kernel acceptance DMA root input invalid");
    let platform = birth
        .take(
            "platform.description",
            ObjectKind::MemoryObject,
            DESCRIPTION_RIGHTS,
        )
        .expect("kernel acceptance platform description input invalid")
        .map(|description| {
            let roots = birth
                .take_list(
                    "platform.root",
                    &[
                        ObjectKind::MmioControl,
                        ObjectKind::IrqControl,
                        ObjectKind::DmaDomainControl,
                    ],
                    PLATFORM_ROOT_RIGHTS,
                )
                .expect("kernel acceptance platform root input invalid");
            Platform::adopt(
                DescriptionObject::adopt(description).expect("platform description invalid"),
                roots,
            )
            .expect("platform roots invalid")
        });
    let process = birth.process();
    birth
        .finish()
        .expect("kernel acceptance birth has unclaimed inputs");
    let roots = match (&platform, &child_dma) {
        (Some(platform), _) => [
            platform.ecam_handle(),
            platform.irq_handle(),
            platform.dma_handle(),
        ],
        (None, dma) => [None, None, dma.as_ref().map(Capability::as_handle)],
    };
    *CONTEXT.lock() = Some(Context {
        process,
        pool: pool.as_handle(),
        mode: settings.mode,
        child,
        roots,
        retention: retention.as_ref().map(Capability::as_handle),
        self_image: self_image
            .as_ref()
            .map(|(_, view)| (view.bytes().as_ptr() as usize, view.bytes().len())),
        target: target.as_ref().map(|(image, _)| image.handle()),
        retained: None,
    });
    let fixture = match settings.role {
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
    if let Some((pages, fixtures)) = with(|ctx| ctx.retained) {
        assert_eq!(
            pages,
            fixtures * dma_checks::RETAINED_PAGES_PER_FIXTURE,
            "retention Pool did not preserve exactly the retained DMA pages"
        );
    }
    drop(platform);
    debug!("kernel acceptance completed");
}

/// 在绑定 retention Pool 的自启动子进程中运行跨 reset 保留的 fixture；主 Pool 不留下跨收集计费。
pub(super) fn retained_fixture(job: Handle, fixture: &[u8]) -> Result<(), &'static str> {
    use alloc::{collections::BTreeMap, string::String};
    use libprocess::GrantOutcome;
    let (pool, (address, length), target) = with(|ctx| {
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
    let mut table = BTreeMap::new();
    table.insert(
        String::from("role"),
        settings::Owned::Str(role.name().into()),
    );
    let encoded = settings::encode(&table).map_err(|_| "retention settings encoding failed")?;
    let mut owners = alloc::vec::Vec::new();
    let mut grants = alloc::vec::Vec::new();
    let mut inputs = alloc::vec::Vec::new();
    for (name, source, rights) in [
        (POOL, pool, RETENTION_POOL_RIGHTS),
        (JOB, job, JOB_FULL_RIGHTS),
        (IMAGE_TARGET, target, IMAGE_RIGHTS),
        (
            DMA_ROOT,
            dma_handle().ok_or("retention DMA root missing")?,
            PLATFORM_ROOT_RIGHTS,
        ),
    ] {
        let handle = duplicate(source, rights).map_err(|_| "retention grant duplicate failed")?;
        // SAFETY: duplicate 返回的新表项由此唯一采用，按 GrantOutcome 兑现移交。
        owners.push(unsafe { Capability::from_raw(handle) });
        inputs.push((name, grants.len() as u8));
        grants.push(HandleGrant { handle, rights });
    }
    let payload = libprocess::birth::encode(&inputs, &encoded)
        .map_err(|_| "retention birth frame encoding failed")?;
    // SAFETY: self_image 与只读 ImageView 由 run 栈持有至进程结束。
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
    ctx.retained = Some((snapshot.allocated, fixtures));
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
