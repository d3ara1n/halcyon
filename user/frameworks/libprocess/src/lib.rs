#![no_std]

//! 用户态 ELF process loader：解析映像并驱动 affine ProcessBuilder。
//! `race` 模块是生命周期竞态矩阵验证负载（init ↔ test_hammer）的线协议。

extern crate alloc;

pub mod race;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{
        ExecutionProfile, HandleGrant, JOB_ENUMERATE_MAX, JobMemberKind, JobState,
        PROCESS_DRAIN_MAX, PROCESS_MAIN_STACK_SIZE, PROCESS_MAX_GRANTS, PROCESS_PAGE_SIZE,
        PROCESS_USER_TOP, ProcessDrainStatus, ProcessMapFlags, ProcessSnapshot, ProcessState,
        ThreadStartContext,
    },
    wait::{WaitItem, WaitReason},
};
use rinlib::{
    ipc::object::{close, duplicate},
    ipc::wait::wait_many,
    process,
};

const MAX_MAP_BYTES: usize = 256 * PROCESS_PAGE_SIZE;
const MAX_WRITE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    Elf(elf::ElfError),
    InvalidImage,
    /// grants 数量超出 shared ABI 上界，不截断、不创建目标。
    /// 直接拒绝——静默丢句柄会让调用方误以为全部装入。
    TooManyGrants,
    System(SystemCallError),
}

impl From<SystemCallError> for SpawnError {
    fn from(error: SystemCallError) -> Self {
        Self::System(error)
    }
}

/// spawn 返回失败时，输入 grants 在调用者表中的最终所有权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantOutcome {
    /// ProcessGrant 未提交，输入 handles 仍由调用者持有。
    Retained,
    /// ProcessGrant 已提交；目标随后收束，输入 handles 已被消费。
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnFailure {
    pub error: SpawnError,
    pub grants: GrantOutcome,
    /// None 表示无需清理或 abandon/drain/close 已全部完成。
    pub cleanup_error: Option<SystemCallError>,
}

impl SpawnFailure {
    fn retained(error: SpawnError) -> Self {
        Self {
            error,
            grants: GrantOutcome::Retained,
            cleanup_error: None,
        }
    }
}

pub struct SpawnRequest<'a> {
    pub job: Handle,
    /// 本次 spawn 的 backing Pool；库复制 GRANT-only authority 并由 Bind 消费。
    pub memory_pool: Handle,
    pub image: &'a [u8],
    pub payload: &'a [u8],
    pub grants: &'a [HandleGrant],
    /// 必须含 MANAGE，保证任意组装失败都能完成目标资源收束。
    pub control_rights: Rights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spawned {
    pub pid: u64,
    pub control: Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredLaunchSet {
    required_present: u64,
    required_started: u64,
    present: u64,
    started: u64,
}

impl RequiredLaunchSet {
    pub const fn new(required_present: u64, required_started: u64) -> Self {
        Self {
            required_present,
            required_started,
            present: 0,
            started: 0,
        }
    }

    pub fn mark_present(&mut self, item: u64) {
        self.present |= item;
    }

    pub fn mark_started(&mut self, item: u64) {
        self.started |= item;
    }

    pub const fn missing_present(&self) -> u64 {
        self.required_present & !self.present
    }

    pub const fn missing_started(&self) -> u64 {
        self.required_started & !self.started
    }

    pub const fn is_complete(&self) -> bool {
        self.missing_present() == 0 && self.missing_started() == 0
    }

    pub const fn present(&self) -> u64 {
        self.present
    }

    pub const fn started(&self) -> u64 {
        self.started
    }
}

/// 解析静态 ET_EXEC，构造地址空间并首次发布进程。
pub fn spawn(request: SpawnRequest<'_>) -> Result<Spawned, SpawnFailure> {
    if !request.control_rights.contains(Rights::MANAGE) {
        return Err(SpawnFailure::retained(SpawnError::System(
            SystemCallError::RightsDenied,
        )));
    }
    let image = elf::validate(
        request.image,
        elf::LoadLimits {
            page_size: PROCESS_PAGE_SIZE as u64,
            image_limit: (PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE) as u64,
        },
    )
    .map_err(|error| SpawnFailure::retained(SpawnError::Elf(error)))?;
    // 上界先筛：超限在建进程前拒绝，无需回滚任何已建资源。
    if request.grants.len() > PROCESS_MAX_GRANTS {
        return Err(SpawnFailure::retained(SpawnError::TooManyGrants));
    }

    let created = process::create(request.job, request.control_rights)
        .map_err(|error| SpawnFailure::retained(SpawnError::System(error)))?;
    let builder = created.builder;

    let mut grants_consumed = false;
    let result: Result<Spawned, SpawnError> = (|| {
        let binding_pool = duplicate(request.memory_pool, Rights::GRANT)?;
        if let Err(error) = process::bind_memory(builder, binding_pool) {
            // Bind 失败契约保留 pool Handle。
            let _ = unsafe { close(binding_pool) };
            return Err(error.into());
        }
        map_plan(builder, &image)?;
        write_segments(builder, &image, request.image)?;
        map_stack(builder)?;

        let profile = match image.requirement() {
            elf::IsaRequirement::Base64 => ExecutionProfile::Base64,
            elf::IsaRequirement::D64 => ExecutionProfile::D64,
        };
        // 组装序列（线程是组装资源）：Grant 装句柄 → 组装者自构造出生块
        // （shared::startup 线格式）→ Write 写入约定区 → Attach 首线程
        // （arg1/arg2 = 块基/块长）→ Start 入册。
        let grant_len = request.grants.len();
        let mut granted = [Handle::INVALID; PROCESS_MAX_GRANTS];
        if grant_len > 0 {
            process::grant(builder, request.grants, &mut granted[..grant_len])?;
            grants_consumed = true;
        }
        let block = build_birth_block(created.pid, &granted[..grant_len], request.payload)?;
        let image_top = usize::try_from(image.image_end()).map_err(|_| SpawnError::InvalidImage)?;
        let block_va = write_birth_block(builder, &block, image_top)?;
        let descriptor = ThreadStartContext {
            entry: image.entry(),
            stack_pointer: PROCESS_USER_TOP as u64,
            arg1: block_va as u64,
            arg2: block.len() as u64,
        };
        process::attach(builder, &descriptor)?;
        process::start(builder, profile as u32)?;
        Ok(Spawned {
            pid: created.pid,
            control: created.control,
        })
    })();

    let cleanup_error = if result.is_err() {
        // SAFETY: 本函数独占真实 Create 结果；失败发生在 Start 消费 builder 之前。
        unsafe { process::abandon_to_completion(created) }.err()
    } else {
        None
    };
    result.map_err(|error| SpawnFailure {
        error,
        grants: if grants_consumed {
            GrantOutcome::Consumed
        } else {
            GrantOutcome::Retained
        },
        cleanup_error,
    })
}

/// 派生监督所需的成员/子域 control rights：kill+drain 需 MANAGE，等
/// REAPABLE/CLOSED 需 WAIT，终态查询需 READ。调用者的 JobControl 必须
/// 持其超集，否则派生返回 RightsDenied。
pub const DERIVED_CONTROL_RIGHTS: Rights =
    Rights::from_raw(Rights::READ.raw() | Rights::WAIT.raw() | Rights::MANAGE.raw());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisionPolicy {
    pub wait_timeout_ms: u64,
    pub wait_attempts: u32,
    pub drain_work: u32,
    pub drain_attempts: u32,
    pub query_attempts: u32,
    pub enumerate_stalls: u32,
}

pub const DEFAULT_SUPERVISION_POLICY: SupervisionPolicy = SupervisionPolicy {
    wait_timeout_ms: 100,
    wait_attempts: 100,
    drain_work: PROCESS_DRAIN_MAX,
    drain_attempts: 64,
    query_attempts: 4,
    enumerate_stalls: 32,
};

#[must_use = "supervision authority must be collected or handed to another supervisor"]
#[derive(Debug)]
pub struct SupervisionTarget {
    pid: u64,
    control: Handle,
}

impl SupervisionTarget {
    pub const fn new(pid: u64, control: Handle) -> Self {
        Self { pid, control }
    }

    pub const fn pid(&self) -> u64 {
        self.pid
    }

    pub const fn control(&self) -> Handle {
        self.control
    }

    pub fn into_control(self) -> Handle {
        self.control
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisionStage {
    WaitReapable,
    Drain,
    VerifyDead,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisionCause {
    InvalidPolicy,
    Timeout,
    System(SystemCallError),
    InconsistentSnapshot { pid: u64, state: u32 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupervisionProgress {
    pub wait_attempts: u32,
    pub drain_attempts: u32,
    pub work_done: u32,
    pub query_attempts: u32,
}

#[must_use = "failed supervision retains live authority"]
#[derive(Debug)]
pub struct SupervisionFailure {
    pub target: SupervisionTarget,
    pub stage: SupervisionStage,
    pub cause: SupervisionCause,
    pub progress: SupervisionProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectedProcess {
    pub pid: u64,
    pub snapshot: ProcessSnapshot,
    pub progress: SupervisionProgress,
}

/// 有限预算地等待、Drain 并核验一个 Process；成功后关闭 control，失败原样返还
/// authority 与进度，调用者可重试、handoff 或升级到所属 Job。
pub fn collect_process(
    target: SupervisionTarget,
    policy: SupervisionPolicy,
) -> Result<CollectedProcess, SupervisionFailure> {
    let mut progress = SupervisionProgress::default();
    if !valid_policy(policy) {
        return Err(SupervisionFailure {
            target,
            stage: SupervisionStage::WaitReapable,
            cause: SupervisionCause::InvalidPolicy,
            progress,
        });
    }

    let mut reapable = false;
    for attempt in 1..=policy.wait_attempts {
        progress.wait_attempts = attempt;
        match wait_many(
            &[WaitItem::new(
                target.control,
                ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
                0,
            )],
            policy.wait_timeout_ms,
        ) {
            Ok(result)
                if result
                    .observed
                    .intersects(ObjectSignals::REAPABLE | ObjectSignals::CLOSED) =>
            {
                reapable = true;
                break;
            }
            Ok(result) if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) => {}
            Ok(_) => {
                return Err(SupervisionFailure {
                    target,
                    stage: SupervisionStage::WaitReapable,
                    cause: SupervisionCause::System(SystemCallError::InternalError),
                    progress,
                });
            }
            Err(SystemCallError::ObjectBusy) => {}
            Err(error) => {
                return Err(SupervisionFailure {
                    target,
                    stage: SupervisionStage::WaitReapable,
                    cause: SupervisionCause::System(error),
                    progress,
                });
            }
        }
    }
    if !reapable {
        return Err(SupervisionFailure {
            target,
            stage: SupervisionStage::WaitReapable,
            cause: SupervisionCause::Timeout,
            progress,
        });
    }

    let mut complete = false;
    for attempt in 1..=policy.drain_attempts {
        progress.drain_attempts = attempt;
        match process::drain(target.control, policy.drain_work) {
            Ok(result) => {
                progress.work_done = progress.work_done.saturating_add(result.work_done);
                if result.status == ProcessDrainStatus::Complete as u32 {
                    complete = true;
                    break;
                }
            }
            Err(SystemCallError::ObjectBusy) => {}
            Err(error) => {
                return Err(SupervisionFailure {
                    target,
                    stage: SupervisionStage::Drain,
                    cause: SupervisionCause::System(error),
                    progress,
                });
            }
        }
    }
    if !complete {
        return Err(SupervisionFailure {
            target,
            stage: SupervisionStage::Drain,
            cause: SupervisionCause::Timeout,
            progress,
        });
    }

    let mut snapshot = None;
    for attempt in 1..=policy.query_attempts {
        progress.query_attempts = attempt;
        match process::query(target.control) {
            Ok(value) => {
                snapshot = Some(value);
                break;
            }
            Err(SystemCallError::ObjectBusy) => {}
            Err(error) => {
                return Err(SupervisionFailure {
                    target,
                    stage: SupervisionStage::VerifyDead,
                    cause: SupervisionCause::System(error),
                    progress,
                });
            }
        }
    }
    let Some(snapshot) = snapshot else {
        return Err(SupervisionFailure {
            target,
            stage: SupervisionStage::VerifyDead,
            cause: SupervisionCause::Timeout,
            progress,
        });
    };
    if snapshot.pid != target.pid || snapshot.state != ProcessState::Dead as u32 {
        return Err(SupervisionFailure {
            target,
            stage: SupervisionStage::VerifyDead,
            cause: SupervisionCause::InconsistentSnapshot {
                pid: snapshot.pid,
                state: snapshot.state,
            },
            progress,
        });
    }
    // SAFETY: 完整 Wait/Drain/Query 已验证 ProcessControl role 和稳定终态；不会关闭 Endpoint。
    if let Err(error) = unsafe { close(target.control) } {
        return Err(SupervisionFailure {
            target,
            stage: SupervisionStage::Close,
            cause: SupervisionCause::System(error),
            progress,
        });
    }
    Ok(CollectedProcess {
        pid: snapshot.pid,
        snapshot,
        progress,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKillStage {
    Seal,
    EnumerateMembers,
    DeriveMember,
    KillMember,
    CollectMember(SupervisionStage),
    EnumerateChildren,
    DeriveChild,
    WaitClosed,
    VerifyDead,
    CloseChild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKillCause {
    Timeout,
    System(SystemCallError),
    Process(SupervisionCause),
    InconsistentState(u32),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobKillProgress {
    pub members_collected: u32,
    pub children_collected: u32,
    pub process_work: u32,
}

#[must_use = "failed JobKill retains the reported child/process authority"]
#[derive(Debug)]
pub struct JobKillFailure {
    pub job: Handle,
    pub process: Option<SupervisionTarget>,
    pub stage: JobKillStage,
    pub cause: JobKillCause,
    pub progress: JobKillProgress,
}

pub fn job_kill(job: Handle, code: i64) -> Result<(), JobKillFailure> {
    job_kill_with_policy(job, code, DEFAULT_SUPERVISION_POLICY)
}

/// 递归 JobKill（用户态政策，内核不递归）。每个 wait、enumeration 与 drain
/// 都受 policy 约束；失败保留当前 JobControl，并在派生 ProcessControl 尚未收束时
/// 随错误返还，不以无限等待或默认 close 掩盖残留。
pub fn job_kill_with_policy(
    job: Handle,
    code: i64,
    policy: SupervisionPolicy,
) -> Result<(), JobKillFailure> {
    let mut progress = JobKillProgress::default();
    if !valid_policy(policy) {
        return Err(job_failure(
            job,
            JobKillStage::Seal,
            JobKillCause::System(SystemCallError::IllegalArgument),
            progress,
        ));
    }
    process::seal_job(job).map_err(|error| {
        job_failure(
            job,
            JobKillStage::Seal,
            JobKillCause::System(error),
            progress,
        )
    })?;

    let members =
        enumerate_members_with_limit(job, JobMemberKind::MemberProcesses, policy.enumerate_stalls)
            .map_err(|error| {
                job_failure(
                    job,
                    JobKillStage::EnumerateMembers,
                    JobKillCause::System(error),
                    progress,
                )
            })?;
    for pid in members {
        let control = match process::derive_job(
            job,
            JobMemberKind::MemberProcesses,
            pid,
            DERIVED_CONTROL_RIGHTS,
        ) {
            Ok(control) => control,
            Err(SystemCallError::ObjectNotFound) => continue,
            Err(error) => {
                return Err(job_failure(
                    job,
                    JobKillStage::DeriveMember,
                    JobKillCause::System(error),
                    progress,
                ));
            }
        };
        let target = SupervisionTarget::new(pid, control);
        if let Err(error) = process::kill(control, code) {
            return Err(JobKillFailure {
                job,
                process: Some(target),
                stage: JobKillStage::KillMember,
                cause: JobKillCause::System(error),
                progress,
            });
        }
        match collect_process(target, policy) {
            Ok(collected) => {
                progress.members_collected = progress.members_collected.saturating_add(1);
                progress.process_work = progress
                    .process_work
                    .saturating_add(collected.progress.work_done);
            }
            Err(failure) => {
                progress.process_work = progress
                    .process_work
                    .saturating_add(failure.progress.work_done);
                return Err(JobKillFailure {
                    job,
                    process: Some(failure.target),
                    stage: JobKillStage::CollectMember(failure.stage),
                    cause: JobKillCause::Process(failure.cause),
                    progress,
                });
            }
        }
    }

    let children =
        enumerate_members_with_limit(job, JobMemberKind::ChildJobs, policy.enumerate_stalls)
            .map_err(|error| {
                job_failure(
                    job,
                    JobKillStage::EnumerateChildren,
                    JobKillCause::System(error),
                    progress,
                )
            })?;
    for jid in children {
        let child =
            match process::derive_job(job, JobMemberKind::ChildJobs, jid, DERIVED_CONTROL_RIGHTS) {
                Ok(child) => child,
                Err(SystemCallError::ObjectNotFound) => continue,
                Err(error) => {
                    return Err(job_failure(
                        job,
                        JobKillStage::DeriveChild,
                        JobKillCause::System(error),
                        progress,
                    ));
                }
            };
        job_kill_with_policy(child, code, policy)?;
        // SAFETY: 本路径取得并完成收束的派生 child JobControl，不包含映射 owner。
        if let Err(error) = unsafe { close(child) } {
            return Err(job_failure(
                child,
                JobKillStage::CloseChild,
                JobKillCause::System(error),
                progress,
            ));
        }
        progress.children_collected = progress.children_collected.saturating_add(1);
    }

    let mut closed = false;
    for _ in 0..policy.wait_attempts {
        match wait_many(
            &[WaitItem::new(job, ObjectSignals::CLOSED, 0)],
            policy.wait_timeout_ms,
        ) {
            Ok(result) if result.observed.intersects(ObjectSignals::CLOSED) => {
                closed = true;
                break;
            }
            Ok(result) if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) => {}
            Ok(_) => {
                return Err(job_failure(
                    job,
                    JobKillStage::WaitClosed,
                    JobKillCause::System(SystemCallError::InternalError),
                    progress,
                ));
            }
            Err(SystemCallError::ObjectBusy) => {}
            Err(error) => {
                return Err(job_failure(
                    job,
                    JobKillStage::WaitClosed,
                    JobKillCause::System(error),
                    progress,
                ));
            }
        }
    }
    if !closed {
        return Err(job_failure(
            job,
            JobKillStage::WaitClosed,
            JobKillCause::Timeout,
            progress,
        ));
    }
    let snapshot = process::query_job(job).map_err(|error| {
        job_failure(
            job,
            JobKillStage::VerifyDead,
            JobKillCause::System(error),
            progress,
        )
    })?;
    if snapshot.state != JobState::Dead as u32 {
        return Err(job_failure(
            job,
            JobKillStage::VerifyDead,
            JobKillCause::InconsistentState(snapshot.state),
            progress,
        ));
    }
    Ok(())
}

fn valid_policy(policy: SupervisionPolicy) -> bool {
    policy.wait_timeout_ms != 0
        && policy.wait_attempts != 0
        && policy.drain_work != 0
        && policy.drain_work <= PROCESS_DRAIN_MAX
        && policy.drain_attempts != 0
        && policy.query_attempts != 0
        && policy.enumerate_stalls != 0
}

fn job_failure(
    job: Handle,
    stage: JobKillStage,
    cause: JobKillCause,
    progress: JobKillProgress,
) -> JobKillFailure {
    JobKillFailure {
        job,
        process: None,
        stage,
        cause,
        progress,
    }
}

/// 枚举至耗尽；占位屏障的零进展批以有限 stall budget 重试。
pub fn enumerate_members(
    job: Handle,
    kind: JobMemberKind,
) -> Result<alloc::vec::Vec<u64>, SystemCallError> {
    enumerate_members_with_limit(job, kind, DEFAULT_SUPERVISION_POLICY.enumerate_stalls)
}

pub fn enumerate_members_with_limit(
    job: Handle,
    kind: JobMemberKind,
    max_stalls: u32,
) -> Result<alloc::vec::Vec<u64>, SystemCallError> {
    if max_stalls == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut ids = alloc::vec::Vec::new();
    let mut buf = [0u64; JOB_ENUMERATE_MAX];
    let mut cursor = 0u64;
    let mut stalls = 0u32;
    loop {
        let result = process::enumerate_job(job, kind, cursor, &mut buf)?;
        ids.extend_from_slice(&buf[..result.actual as usize]);
        if result.more == 0 {
            return Ok(ids);
        }
        if result.actual == 0 {
            stalls = stalls.saturating_add(1);
            if stalls >= max_stalls {
                return Err(SystemCallError::ObjectBusy);
            }
        } else {
            stalls = 0;
            cursor = result.next_cursor;
        }
    }
}

fn map_plan(builder: Handle, image: &elf::Elf) -> Result<(), SpawnError> {
    for run in image.runs() {
        let start = usize::try_from(run.vaddr).map_err(|_| SpawnError::InvalidImage)?;
        let bytes = usize::try_from(run.memsz).map_err(|_| SpawnError::InvalidImage)?;
        let mut permissions = ProcessMapFlags::READ;
        if run.writable {
            permissions = permissions | ProcessMapFlags::WRITE;
        }
        if run.executable {
            permissions = permissions | ProcessMapFlags::EXECUTE;
        }
        for offset in (0..bytes).step_by(MAX_MAP_BYTES) {
            process::map(
                builder,
                start + offset,
                MAX_MAP_BYTES.min(bytes - offset),
                permissions,
            )?;
        }
    }
    Ok(())
}

fn write_segments(builder: Handle, image: &elf::Elf, file: &[u8]) -> Result<(), SpawnError> {
    for segment in image.segments() {
        let offset = usize::try_from(segment.offset).map_err(|_| SpawnError::InvalidImage)?;
        let filesz = usize::try_from(segment.filesz).map_err(|_| SpawnError::InvalidImage)?;
        let source = file
            .get(offset..offset.checked_add(filesz).ok_or(SpawnError::InvalidImage)?)
            .ok_or(SpawnError::InvalidImage)?;
        let target = usize::try_from(segment.vaddr).map_err(|_| SpawnError::InvalidImage)?;
        for (index, chunk) in source.chunks(MAX_WRITE_BYTES).enumerate() {
            process::write(builder, target + index * MAX_WRITE_BYTES, chunk)?;
        }
    }
    Ok(())
}

/// 组装者侧出生块构造（shared::startup 线格式：Header + 句柄数组 +
/// payload）。内核不参与构造——出生块是组装者与接收进程的用户约定，
/// 也是接收进程得知自身身份的途径（它未必持有自己的 ProcessControl，
/// 不能靠 ProcessQuery 反查）。
///
/// **`parent_pid` 不作参数，由组装者身份内部推导**：它描述目标进程的
/// 创建关系，而创建者恒为本进程——「谁创建了它」不是调用方可自由选择
/// 的信息。参数化曾使误传 `env::parent_pid()`（目标的祖父）成为可能，
/// 现在该错误在结构上不存在。`target_pid` 仍为参数：它来自
/// ProcessCreate 的内核返回值，组装者无法推导。
fn build_birth_block(
    target_pid: u64,
    handles: &[Handle],
    payload: &[u8],
) -> Result<alloc::vec::Vec<u8>, SpawnError> {
    erhino_shared::startup::build_startup_block(target_pid, rinlib::env::pid(), handles, payload)
        .map_err(|_| SpawnError::InvalidImage)
}

/// 出生块写入约定区：validated ELF 的映像顶之上页对齐放置——组装者
/// 掌握映像布局（Map 由其驱动），无需查询目标布局游标；块的只读性由接收方
/// 运行时自行遵守（v1 约定，见计划篇 D6c）。逐页映射并单次回填，
/// 返回块基址。
fn write_birth_block(builder: Handle, block: &[u8], image_top: usize) -> Result<usize, SpawnError> {
    let base = image_top.div_ceil(PROCESS_PAGE_SIZE) * PROCESS_PAGE_SIZE;
    let end = base
        .checked_add(block.len())
        .ok_or(SpawnError::InvalidImage)?;
    if end > PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE {
        return Err(SpawnError::InvalidImage);
    }
    let span_pages = block.len().div_ceil(PROCESS_PAGE_SIZE);
    let span = span_pages * PROCESS_PAGE_SIZE;
    let permissions = ProcessMapFlags::READ | ProcessMapFlags::WRITE;
    let mut mapped = 0usize;
    while mapped < span {
        let len = MAX_MAP_BYTES.min(span - mapped);
        process::map(builder, base + mapped, len, permissions).map_err(SpawnError::System)?;
        mapped += len;
    }
    process::write(builder, base, block).map_err(SpawnError::System)?;
    Ok(base)
}

fn map_stack(builder: Handle) -> Result<(), SpawnError> {
    let base = PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE;
    let permissions = ProcessMapFlags::READ | ProcessMapFlags::WRITE;
    for offset in (0..PROCESS_MAIN_STACK_SIZE).step_by(MAX_MAP_BYTES) {
        let len = MAX_MAP_BYTES.min(PROCESS_MAIN_STACK_SIZE - offset);
        process::map(builder, base + offset, len, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_requires_cleanup_authority_before_parsing_or_syscalls() {
        assert_eq!(
            spawn(SpawnRequest {
                job: Handle::INVALID,
                memory_pool: Handle::INVALID,
                image: &[],
                payload: &[],
                grants: &[],
                control_rights: Rights::READ,
            }),
            Err(SpawnFailure {
                error: SpawnError::System(SystemCallError::RightsDenied),
                grants: GrantOutcome::Retained,
                cleanup_error: None,
            })
        );
    }

    #[test]
    fn required_launch_set_rejects_missing_or_failed_items() {
        let mut set = RequiredLaunchSet::new(0b11, 0b111);
        set.mark_present(0b01);
        set.mark_started(0b001);
        assert_eq!(set.missing_present(), 0b10);
        assert_eq!(set.missing_started(), 0b110);
        assert!(!set.is_complete());

        set.mark_present(0b10);
        set.mark_started(0b010);
        assert!(!set.is_complete());
        set.mark_started(0b100);
        assert!(set.is_complete());
    }

    #[test]
    fn invalid_supervision_policy_returns_authority_before_syscalls() {
        let mut policy = DEFAULT_SUPERVISION_POLICY;
        policy.wait_timeout_ms = 0;
        let failure = collect_process(
            SupervisionTarget::new(7, Handle::from_raw(0x1_0000_0001)),
            policy,
        )
        .unwrap_err();
        assert_eq!(failure.target.pid(), 7);
        assert_eq!(failure.target.control().raw(), 0x1_0000_0001);
        assert_eq!(failure.stage, SupervisionStage::WaitReapable);
        assert_eq!(failure.cause, SupervisionCause::InvalidPolicy);

        let failure = job_kill_with_policy(Handle::from_raw(0x2_0000_0001), 1, policy).unwrap_err();
        assert_eq!(failure.job.raw(), 0x2_0000_0001);
        assert_eq!(failure.stage, JobKillStage::Seal);
        assert!(failure.process.is_none());
    }
}
