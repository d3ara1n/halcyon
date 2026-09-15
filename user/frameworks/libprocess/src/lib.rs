#![no_std]

//! 用户态 ELF process loader：解析映像并驱动 affine ProcessBuilder。
//! `race` 模块是生命周期竞态矩阵验证负载（init ↔ test_hammer）的线协议。

extern crate alloc;

use alloc::vec::Vec;

pub mod race;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{
        ExecutionProfile, HandleGrant, JOB_ENUMERATE_MAX, JobMemberKind, JobState,
        PROCESS_DRAIN_MAX, PROCESS_MAIN_STACK_SIZE, PROCESS_MAX_GRANTS, PROCESS_PAGE_SIZE,
        PROCESS_USER_TOP, ProcessMapFlags, ProcessSnapshot, ThreadStartContext,
    },
    time::Deadline,
};
use rinlib::{
    ipc::object::{close, duplicate},
    process,
};

pub mod job_driver;
pub mod observation;
pub mod supervise;
use observation::{Observation, ObservationResult, ObservationState};
use supervise::{ProcessOperations, RealProcessOperations};

pub use supervise::{
    Collector, SUPERVISE_SOURCE, StepOutcome, SuperviseResult, SuperviseSink, SuperviseTask,
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
    /// 任务被停止接管；authority 完整保留在失败结果中交回。
    Stopped,
    System(SystemCallError),
    InconsistentSnapshot {
        pid: u64,
        state: u32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupervisionProgress {
    pub wait_attempts: u32,
    pub drain_attempts: u32,
    pub work_done: u32,
    pub query_attempts: u32,
}

/// 失败继续持有完整 Process 机器，包含观察、Drain、Verify 或 Closing 阶段。
pub type SupervisionFailure = supervise::CollectorFailure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectedProcess {
    pub pid: u64,
    pub snapshot: ProcessSnapshot,
    pub progress: SupervisionProgress,
}

/// 有限预算地等待、Drain 并核验一个 Process；成功后关闭 control，失败原样返还
/// authority 与进度，调用者可重试、handoff 或升级到所属 Job。
#[expect(
    clippy::result_large_err,
    reason = "失败按值返还完整机器，不在错误路径分配"
)]
pub fn collect_process(
    target: SupervisionTarget,
    policy: SupervisionPolicy,
) -> Result<CollectedProcess, SupervisionFailure> {
    let machine = Collector::new(target, policy);
    if !valid_policy(policy) {
        return Err(machine.fail(
            SupervisionStage::WaitReapable,
            SupervisionCause::InvalidPolicy,
        ));
    }
    collect_remaining(machine)
}

/// 同步门面恢复原机器，绝不由裸 control 重建已进行中的关闭阶段。
#[expect(
    clippy::result_large_err,
    reason = "失败按值返还完整机器，不在错误路径分配"
)]
pub fn collect_remaining(mut machine: Collector) -> Result<CollectedProcess, SupervisionFailure> {
    loop {
        let now = match rinlib::time::snapshot() {
            Ok(snapshot) => snapshot.now_ns,
            Err(error) => {
                return Err(machine.fail_current(SupervisionCause::System(error)));
            }
        };
        match machine.step(now) {
            StepOutcome::Continue(next) => machine = next,
            StepOutcome::Observe(request, mut next) => {
                let result = observation::wait(request)
                    .map_err(SupervisionCause::System)
                    .and_then(|(now, result)| next.observe(request, result, now));
                if let Err(cause) = result {
                    return Err(next.fail_current(cause));
                }
                machine = next;
            }
            StepOutcome::RetryAt(at, next) => {
                if let Err(error) = rinlib::time::sleep_until(Deadline::at(at)) {
                    return Err(next.fail_current(SupervisionCause::System(error)));
                }
                machine = next;
            }
            StepOutcome::Done(done) => return Ok(done),
            StepOutcome::Escalate(failure) => return Err(failure),
        }
    }
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
    pub collector: Option<JobCollector<RealJobOperations>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCollectorEvent {
    Progress,
    RetryAt(u64),
    Observe(Observation),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobCollectorError {
    pub stage: JobKillStage,
    pub cause: JobKillCause,
    pub progress: JobKillProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobPhase {
    Seal,
    Members,
    DeriveMember,
    KillMember,
    CollectMember,
    Children,
    DeriveChild,
    CloseChild,
    WaitClosed,
    VerifyDead,
    Done,
}

#[derive(Debug)]
struct JobFrame<O: ProcessOperations> {
    job: Handle,
    phase: JobPhase,
    members: Vec<u64>,
    member_cursor: usize,
    member_enum_cursor: u64,
    member_stalls: u32,
    children: Vec<u64>,
    child_cursor: usize,
    child_enum_cursor: u64,
    child_stalls: u32,
    current: Option<SupervisionTarget>,
    process: Option<Collector<O>>,
    child_job: Option<Handle>,
    observation: ObservationState,
}

pub trait JobOperations: ProcessOperations {
    fn seal_job(&self, job: Handle) -> Result<(), SystemCallError>;
    fn enumerate_job(
        &self,
        job: Handle,
        kind: JobMemberKind,
        cursor: u64,
        output: &mut [u64],
    ) -> Result<erhino_shared::proc::JobEnumerateResult, SystemCallError>;
    fn derive_job(
        &self,
        job: Handle,
        kind: JobMemberKind,
        id: u64,
        rights: Rights,
    ) -> Result<Handle, SystemCallError>;
    fn kill(&self, control: Handle, code: i64) -> Result<(), SystemCallError>;
    fn query_job(&self, job: Handle) -> Result<erhino_shared::proc::JobSnapshot, SystemCallError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RealJobOperations;

impl JobOperations for RealJobOperations {
    fn seal_job(&self, job: Handle) -> Result<(), SystemCallError> {
        process::seal_job(job)
    }
    fn enumerate_job(
        &self,
        job: Handle,
        kind: JobMemberKind,
        cursor: u64,
        output: &mut [u64],
    ) -> Result<erhino_shared::proc::JobEnumerateResult, SystemCallError> {
        process::enumerate_job(job, kind, cursor, output)
    }
    fn derive_job(
        &self,
        job: Handle,
        kind: JobMemberKind,
        id: u64,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        process::derive_job(job, kind, id, rights)
    }
    fn kill(&self, control: Handle, code: i64) -> Result<(), SystemCallError> {
        process::kill(control, code)
    }
    fn query_job(&self, job: Handle) -> Result<erhino_shared::proc::JobSnapshot, SystemCallError> {
        process::query_job(job)
    }
}

impl ProcessOperations for RealJobOperations {
    fn probe(
        &self,
        control: Handle,
        signals: ObjectSignals,
    ) -> Result<ObservationResult, SystemCallError> {
        RealProcessOperations.probe(control, signals)
    }
    fn drain(
        &self,
        control: Handle,
        work: u32,
    ) -> Result<erhino_shared::proc::ProcessDrainResult, SystemCallError> {
        RealProcessOperations.drain(control, work)
    }
    fn query(&self, control: Handle) -> Result<ProcessSnapshot, SystemCallError> {
        RealProcessOperations.query(control)
    }
    fn close(&self, control: Handle) -> Result<(), SystemCallError> {
        RealProcessOperations.close(control)
    }
}

#[derive(Debug)]
pub struct JobCollector<O: JobOperations + Clone + core::fmt::Debug = RealJobOperations> {
    frames: Vec<JobFrame<O>>,
    code: i64,
    policy: SupervisionPolicy,
    progress: JobKillProgress,
    ops: O,
}

impl JobCollector<RealJobOperations> {
    pub fn new(job: Handle, code: i64, policy: SupervisionPolicy) -> Result<Self, SystemCallError> {
        Self::new_with_ops(job, code, policy, RealJobOperations)
    }
}

impl<O: JobOperations + Clone + core::fmt::Debug> JobCollector<O> {
    fn new_frame(job: Handle) -> Result<JobFrame<O>, SystemCallError> {
        let mut members = Vec::new();
        members
            .try_reserve_exact(JOB_ENUMERATE_MAX)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(JOB_ENUMERATE_MAX)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(JobFrame {
            job,
            phase: JobPhase::Seal,
            members,
            member_cursor: 0,
            member_enum_cursor: 0,
            member_stalls: 0,
            children,
            child_cursor: 0,
            child_enum_cursor: 0,
            child_stalls: 0,
            current: None,
            process: None,
            child_job: None,
            observation: ObservationState::default(),
        })
    }

    pub fn new_with_ops(
        job: Handle,
        code: i64,
        policy: SupervisionPolicy,
        ops: O,
    ) -> Result<Self, SystemCallError> {
        if !valid_policy(policy) {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(1)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        frames.push(Self::new_frame(job)?);
        Ok(Self {
            frames,
            code,
            policy,
            progress: JobKillProgress::default(),
            ops,
        })
    }

    pub fn root_job(&self) -> Handle {
        self.frames.first().expect("job collector has no root").job
    }

    pub fn progress(&self) -> JobKillProgress {
        let mut progress = self.progress;
        if let Some(process) = self.frames.last().and_then(|frame| frame.process.as_ref()) {
            progress.process_work = progress
                .process_work
                .saturating_add(process.progress().work_done);
        }
        progress
    }

    pub fn probe(&self, request: Observation) -> Result<ObservationResult, SystemCallError> {
        self.ops.probe(request.control, request.signals)
    }

    pub fn stage(&self) -> JobKillStage {
        let frame = self
            .frames
            .last()
            .expect("Job collector owns its active frame");
        match frame.phase {
            JobPhase::Seal => JobKillStage::Seal,
            JobPhase::Members => JobKillStage::EnumerateMembers,
            JobPhase::DeriveMember => JobKillStage::DeriveMember,
            JobPhase::KillMember => JobKillStage::KillMember,
            JobPhase::CollectMember => JobKillStage::CollectMember(
                frame
                    .process
                    .as_ref()
                    .map_or(SupervisionStage::WaitReapable, Collector::stage),
            ),
            JobPhase::Children => JobKillStage::EnumerateChildren,
            JobPhase::DeriveChild => JobKillStage::DeriveChild,
            JobPhase::CloseChild => JobKillStage::CloseChild,
            JobPhase::WaitClosed => JobKillStage::WaitClosed,
            JobPhase::VerifyDead | JobPhase::Done => JobKillStage::VerifyDead,
        }
    }

    fn fail_current(&self, cause: JobKillCause) -> JobCollectorError {
        self.fail(self.stage(), cause)
    }

    fn fail(&self, stage: JobKillStage, cause: JobKillCause) -> JobCollectorError {
        JobCollectorError {
            stage,
            cause,
            progress: self.progress(),
        }
    }

    pub fn step(&mut self, now_ns: u64) -> Result<JobCollectorEvent, JobCollectorError> {
        let i = self.frames.len() - 1;
        if self.frames[i].phase == JobPhase::Done {
            if i == 0 {
                return Ok(JobCollectorEvent::Done);
            }
            self.frames.pop();
            self.frames
                .last_mut()
                .expect("parent frame missing after child completion")
                .phase = JobPhase::CloseChild;
            return Ok(JobCollectorEvent::Progress);
        }
        match self.frames[i].phase {
            JobPhase::Seal => {
                self.ops
                    .seal_job(self.frames[i].job)
                    .map_err(|e| self.fail(JobKillStage::Seal, JobKillCause::System(e)))?;
                self.frames[i].phase = JobPhase::Members;
                Ok(JobCollectorEvent::Progress)
            }
            JobPhase::Members | JobPhase::Children => {
                let children = self.frames[i].phase == JobPhase::Children;
                let (kind, cursor) = if children {
                    (JobMemberKind::ChildJobs, self.frames[i].child_enum_cursor)
                } else {
                    (
                        JobMemberKind::MemberProcesses,
                        self.frames[i].member_enum_cursor,
                    )
                };
                let mut page = [0u64; JOB_ENUMERATE_MAX];
                let result = self
                    .ops
                    .enumerate_job(self.frames[i].job, kind, cursor, &mut page)
                    .map_err(|e| {
                        self.fail(
                            if children {
                                JobKillStage::EnumerateChildren
                            } else {
                                JobKillStage::EnumerateMembers
                            },
                            JobKillCause::System(e),
                        )
                    })?;
                let actual = result.actual as usize;
                let stalls = if children {
                    &mut self.frames[i].child_stalls
                } else {
                    &mut self.frames[i].member_stalls
                };
                if actual == 0 && result.more != 0 {
                    *stalls = stalls.saturating_add(1);
                    if *stalls >= self.policy.enumerate_stalls {
                        return Err(self.fail(
                            if children {
                                JobKillStage::EnumerateChildren
                            } else {
                                JobKillStage::EnumerateMembers
                            },
                            JobKillCause::Timeout,
                        ));
                    }
                } else {
                    *stalls = 0;
                    let reserve_failed = {
                        let target = if children {
                            &mut self.frames[i].children
                        } else {
                            &mut self.frames[i].members
                        };
                        if target.try_reserve_exact(actual).is_err() {
                            true
                        } else {
                            target.extend_from_slice(&page[..actual]);
                            false
                        }
                    };
                    if reserve_failed {
                        return Err(self.fail(
                            if children {
                                JobKillStage::EnumerateChildren
                            } else {
                                JobKillStage::EnumerateMembers
                            },
                            JobKillCause::System(SystemCallError::OutOfMemory),
                        ));
                    }
                }
                if result.more == 0 {
                    self.frames[i].phase = if children {
                        JobPhase::DeriveChild
                    } else {
                        JobPhase::DeriveMember
                    };
                } else if children {
                    self.frames[i].child_enum_cursor = result.next_cursor;
                } else {
                    self.frames[i].member_enum_cursor = result.next_cursor;
                }
                Ok(JobCollectorEvent::Progress)
            }
            JobPhase::DeriveMember => {
                if self.frames[i].member_cursor == self.frames[i].members.len() {
                    self.frames[i].phase = JobPhase::Children;
                    return Ok(JobCollectorEvent::Progress);
                }
                let pid = self.frames[i].members[self.frames[i].member_cursor];
                match self.ops.derive_job(
                    self.frames[i].job,
                    JobMemberKind::MemberProcesses,
                    pid,
                    DERIVED_CONTROL_RIGHTS,
                ) {
                    Ok(control) => {
                        self.frames[i].current = Some(SupervisionTarget::new(pid, control));
                        self.frames[i].phase = JobPhase::KillMember;
                    }
                    Err(SystemCallError::ObjectNotFound) => self.frames[i].member_cursor += 1,
                    Err(error) => {
                        return Err(
                            self.fail(JobKillStage::DeriveMember, JobKillCause::System(error))
                        );
                    }
                }
                Ok(JobCollectorEvent::Progress)
            }
            JobPhase::KillMember => {
                let target = self.frames[i]
                    .current
                    .take()
                    .expect("kill lost process authority");
                match self.ops.kill(target.control(), self.code) {
                    Ok(()) => {
                        self.frames[i].current = Some(target);
                        self.frames[i].phase = JobPhase::CollectMember;
                        Ok(JobCollectorEvent::Progress)
                    }
                    Err(error) => {
                        self.frames[i].current = Some(target);
                        Err(self.fail(JobKillStage::KillMember, JobKillCause::System(error)))
                    }
                }
            }
            JobPhase::CollectMember => {
                if self.frames[i].process.is_none() {
                    let target = self.frames[i]
                        .current
                        .take()
                        .expect("member authority must remain owned");
                    self.frames[i].process = Some(Collector::new_with_ops(
                        target,
                        self.policy,
                        self.ops.clone(),
                    ));
                }
                let collector = self.frames[i]
                    .process
                    .take()
                    .expect("member machine must remain owned");
                match collector.step(now_ns) {
                    StepOutcome::Observe(request, next) => {
                        self.frames[i].process = Some(next);
                        Ok(JobCollectorEvent::Observe(request))
                    }
                    StepOutcome::Continue(next) => {
                        self.frames[i].process = Some(next);
                        Ok(JobCollectorEvent::Progress)
                    }
                    StepOutcome::RetryAt(at, next) => {
                        self.frames[i].process = Some(next);
                        Ok(JobCollectorEvent::RetryAt(at))
                    }
                    StepOutcome::Done(collected) => {
                        self.progress.members_collected =
                            self.progress.members_collected.saturating_add(1);
                        self.progress.process_work = self
                            .progress
                            .process_work
                            .saturating_add(collected.progress.work_done);
                        self.frames[i].member_cursor += 1;
                        self.frames[i].phase = JobPhase::DeriveMember;
                        Ok(JobCollectorEvent::Progress)
                    }
                    StepOutcome::Escalate(failure) => {
                        self.frames[i].process = Some(failure.collector);
                        Err(self.fail(
                            JobKillStage::CollectMember(failure.stage),
                            JobKillCause::Process(failure.cause),
                        ))
                    }
                }
            }
            JobPhase::DeriveChild => {
                if self.frames[i].child_cursor == self.frames[i].children.len() {
                    self.frames[i].phase = JobPhase::WaitClosed;
                    return Ok(JobCollectorEvent::Progress);
                }
                let jid = self.frames[i].children[self.frames[i].child_cursor];
                self.frames.try_reserve_exact(1).map_err(|_| {
                    self.fail(
                        JobKillStage::DeriveChild,
                        JobKillCause::System(SystemCallError::OutOfMemory),
                    )
                })?;
                let mut prepared = Self::new_frame(Handle::INVALID).map_err(|error| {
                    self.fail(JobKillStage::DeriveChild, JobKillCause::System(error))
                })?;
                match self.ops.derive_job(
                    self.frames[i].job,
                    JobMemberKind::ChildJobs,
                    jid,
                    DERIVED_CONTROL_RIGHTS,
                ) {
                    Ok(child) => {
                        self.frames[i].child_job = Some(child);
                        self.frames[i].phase = JobPhase::CloseChild;
                        prepared.job = child;
                        self.frames.push(prepared);
                    }
                    Err(SystemCallError::ObjectNotFound) => self.frames[i].child_cursor += 1,
                    Err(error) => {
                        return Err(
                            self.fail(JobKillStage::DeriveChild, JobKillCause::System(error))
                        );
                    }
                }
                Ok(JobCollectorEvent::Progress)
            }
            JobPhase::CloseChild => {
                let child_job = self.frames[i].child_job.expect("child job missing");
                // SAFETY: child_job was derived by this frame and is closed only after success.
                self.ops
                    .close(child_job)
                    .map_err(|e| self.fail(JobKillStage::CloseChild, JobKillCause::System(e)))?;
                self.frames[i].child_job = None;
                self.frames[i].child_cursor += 1;
                self.progress.children_collected =
                    self.progress.children_collected.saturating_add(1);
                self.frames[i].phase = JobPhase::DeriveChild;
                Ok(JobCollectorEvent::Progress)
            }
            JobPhase::WaitClosed => {
                let job = self.frames[i].job;
                self.frames[i]
                    .observation
                    .request(job, ObjectSignals::CLOSED, now_ns, self.policy)
                    .map(JobCollectorEvent::Observe)
                    .map_err(|cause| {
                        self.fail(JobKillStage::WaitClosed, JobKillCause::Process(cause))
                    })
            }
            JobPhase::VerifyDead => {
                let snapshot = self
                    .ops
                    .query_job(self.frames[i].job)
                    .map_err(|e| self.fail(JobKillStage::VerifyDead, JobKillCause::System(e)))?;
                if snapshot.state != JobState::Dead as u32 {
                    return Err(self.fail(
                        JobKillStage::VerifyDead,
                        JobKillCause::InconsistentState(snapshot.state),
                    ));
                }
                self.frames[i].phase = JobPhase::Done;
                if i == 0 {
                    Ok(JobCollectorEvent::Done)
                } else {
                    Ok(JobCollectorEvent::Progress)
                }
            }
            JobPhase::Done => unreachable!("completed frame handled before dispatch"),
        }
    }

    /// 消费指定观察，只改变状态；下一步事件必须由 step 明确取得。
    pub fn observe(
        &mut self,
        request: Observation,
        result: ObservationResult,
        now: u64,
    ) -> Result<(), JobCollectorError> {
        let frame = self
            .frames
            .last_mut()
            .expect("job collector owns a root frame");
        let outcome = match frame.phase {
            JobPhase::CollectMember => frame
                .process
                .as_mut()
                .ok_or(SupervisionCause::System(SystemCallError::IllegalArgument))
                .and_then(|process| process.observe(request, result, now)),
            JobPhase::WaitClosed => frame.observation.accept(request, result, now).map(|ready| {
                if ready {
                    frame.phase = JobPhase::VerifyDead;
                }
            }),
            _ => Err(SupervisionCause::System(SystemCallError::IllegalArgument)),
        };
        outcome.map_err(|cause| self.fail_current(JobKillCause::Process(cause)))
    }

    pub fn replenish(&mut self, policy: SupervisionPolicy) {
        self.policy = policy;
        let frame = self
            .frames
            .last_mut()
            .expect("job collector owns a root frame");
        frame.observation.renew();
        if let Some(process) = &mut frame.process {
            process.replenish(policy);
        }
    }
}

pub fn job_kill_with_policy(
    job: Handle,
    code: i64,
    policy: SupervisionPolicy,
) -> Result<(), JobKillFailure> {
    let collector = JobCollector::new(job, code, policy).map_err(|error| {
        job_failure(
            job,
            JobKillStage::Seal,
            JobKillCause::System(error),
            JobKillProgress::default(),
        )
    })?;
    collect_job_remaining(collector)
}

/// 同步驱动原 Job 机器；失败结果按值返还，不需要错误包装分配。
pub fn collect_job_remaining(mut collector: JobCollector) -> Result<(), JobKillFailure> {
    loop {
        let result: Result<bool, JobCollectorError> = (|| {
            let now = rinlib::time::snapshot()
                .map_err(|error| collector.fail_current(JobKillCause::System(error)))?
                .now_ns;
            match collector.step(now)? {
                JobCollectorEvent::Progress => Ok(false),
                JobCollectorEvent::Done => Ok(true),
                JobCollectorEvent::Observe(request) => {
                    let (now, result) = observation::wait(request)
                        .map_err(|error| collector.fail_current(JobKillCause::System(error)))?;
                    collector.observe(request, result, now)?;
                    Ok(false)
                }
                JobCollectorEvent::RetryAt(at) => {
                    rinlib::time::sleep_until(Deadline::at(at))
                        .map_err(|error| collector.fail_current(JobKillCause::System(error)))?;
                    Ok(false)
                }
            }
        })();
        match result {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                return Err(JobKillFailure {
                    job: collector.root_job(),
                    collector: Some(collector),
                    stage: error.stage,
                    cause: error.cause,
                    progress: error.progress,
                });
            }
        }
    }
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
        collector: None,
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
    use alloc::rc::Rc;
    use core::cell::RefCell;

    #[derive(Debug, Default)]
    struct FakeJobState {
        close_failures: usize,
        close_successes: usize,
        member: bool,
        drain_calls: usize,
    }

    #[derive(Debug, Clone)]
    struct FakeJobOps {
        state: Rc<RefCell<FakeJobState>>,
    }

    impl JobOperations for FakeJobOps {
        fn seal_job(&self, _job: Handle) -> Result<(), SystemCallError> {
            Ok(())
        }

        fn enumerate_job(
            &self,
            job: Handle,
            kind: JobMemberKind,
            cursor: u64,
            output: &mut [u64],
        ) -> Result<erhino_shared::proc::JobEnumerateResult, SystemCallError> {
            if self.state.borrow().member
                && kind == JobMemberKind::MemberProcesses
                && job.raw() == 1
            {
                output[0] = 7;
                return Ok(erhino_shared::proc::JobEnumerateResult {
                    next_cursor: 7,
                    actual: 1,
                    more: 0,
                });
            }
            if kind == JobMemberKind::ChildJobs && (job.raw() == 1 || job.raw() == 2) && cursor == 0
            {
                output[0] = if job.raw() == 1 { 42 } else { 43 };
                Ok(erhino_shared::proc::JobEnumerateResult {
                    next_cursor: 42,
                    actual: 1,
                    more: 0,
                })
            } else {
                Ok(erhino_shared::proc::JobEnumerateResult {
                    next_cursor: cursor,
                    actual: 0,
                    more: 0,
                })
            }
        }

        fn derive_job(
            &self,
            _job: Handle,
            kind: JobMemberKind,
            id: u64,
            _rights: Rights,
        ) -> Result<Handle, SystemCallError> {
            if kind == JobMemberKind::MemberProcesses && id == 7 {
                return Ok(Handle::from_raw(70));
            }
            if kind == JobMemberKind::ChildJobs && id == 42 {
                Ok(Handle::from_raw(2))
            } else if kind == JobMemberKind::ChildJobs && id == 43 {
                Ok(Handle::from_raw(3))
            } else {
                Err(SystemCallError::ObjectNotFound)
            }
        }

        fn kill(&self, _control: Handle, _code: i64) -> Result<(), SystemCallError> {
            Ok(())
        }

        fn query_job(
            &self,
            job: Handle,
        ) -> Result<erhino_shared::proc::JobSnapshot, SystemCallError> {
            Ok(erhino_shared::proc::JobSnapshot {
                jid: job.raw(),
                parent_jid: 0,
                state: JobState::Dead as u32,
                live_processes: 0,
                live_children: 0,
                reserved: 0,
                reserved2: 0,
            })
        }
    }

    impl ProcessOperations for FakeJobOps {
        fn probe(
            &self,
            _: Handle,
            signals: ObjectSignals,
        ) -> Result<ObservationResult, SystemCallError> {
            Ok(ObservationResult::Ready(signals))
        }
        fn drain(
            &self,
            _control: Handle,
            _work: u32,
        ) -> Result<erhino_shared::proc::ProcessDrainResult, SystemCallError> {
            let mut state = self.state.borrow_mut();
            assert!(state.member);
            state.drain_calls += 1;
            match state.drain_calls {
                1 => Ok(erhino_shared::proc::ProcessDrainResult {
                    work_done: 3,
                    status: erhino_shared::proc::ProcessDrainStatus::More as u32,
                    reserved: 0,
                }),
                2 => Err(SystemCallError::InternalError),
                _ => Ok(erhino_shared::proc::ProcessDrainResult {
                    work_done: 2,
                    status: erhino_shared::proc::ProcessDrainStatus::Complete as u32,
                    reserved: 0,
                }),
            }
        }
        fn query(&self, _control: Handle) -> Result<ProcessSnapshot, SystemCallError> {
            Ok(ProcessSnapshot {
                pid: 7,
                parent_pid: 0,
                state: erhino_shared::proc::ProcessState::Dead as u32,
                reason: 0,
                code: 0,
                reserved: 0,
            })
        }
        fn close(&self, _control: Handle) -> Result<(), SystemCallError> {
            let mut state = self.state.borrow_mut();
            if state.close_failures != 0 {
                state.close_failures -= 1;
                return Err(SystemCallError::ObjectBusy);
            }
            state.close_successes += 1;
            Ok(())
        }
    }

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
        assert_eq!(failure.collector.pid(), 7);
        assert_eq!(failure.collector.wait_control().raw(), 0x1_0000_0001);
        assert_eq!(failure.stage, SupervisionStage::WaitReapable);
        assert_eq!(failure.cause, SupervisionCause::InvalidPolicy);

        let failure = job_kill_with_policy(Handle::from_raw(0x2_0000_0001), 1, policy).unwrap_err();
        assert_eq!(failure.job.raw(), 0x2_0000_0001);
        assert_eq!(failure.stage, JobKillStage::Seal);
        assert!(failure.collector.is_none());
    }

    #[test]
    fn job_failure_reports_current_process_work_without_double_counting_recovery() {
        let state = Rc::new(RefCell::new(FakeJobState {
            member: true,
            ..FakeJobState::default()
        }));
        let mut collector = JobCollector::new_with_ops(
            Handle::from_raw(1),
            0,
            DEFAULT_SUPERVISION_POLICY,
            FakeJobOps { state },
        )
        .unwrap();
        let mut failed = false;
        loop {
            match collector.step(0) {
                Ok(JobCollectorEvent::Observe(request)) => collector
                    .observe(request, ObservationResult::Ready(request.signals), 0)
                    .unwrap(),
                Ok(JobCollectorEvent::Progress) => {}
                Ok(JobCollectorEvent::Done) => break,
                Ok(JobCollectorEvent::RetryAt(_)) => panic!("fixture uses a hard failure"),
                Err(error) => {
                    assert!(!failed);
                    failed = true;
                    assert_eq!(
                        error.stage,
                        JobKillStage::CollectMember(SupervisionStage::Drain)
                    );
                    assert_eq!(error.progress.process_work, 3);
                    assert_eq!(collector.progress().process_work, 3);
                }
            }
        }
        assert!(failed);
        assert_eq!(collector.progress().process_work, 5);
        assert_eq!(collector.progress().members_collected, 1);
    }

    #[test]
    fn job_collector_recovers_nested_child_close_failure_without_double_close() {
        let state = Rc::new(RefCell::new(FakeJobState {
            close_failures: 1,
            close_successes: 0,
            ..FakeJobState::default()
        }));
        let ops = FakeJobOps {
            state: state.clone(),
        };
        let mut collector =
            JobCollector::new_with_ops(Handle::from_raw(1), 0x55, DEFAULT_SUPERVISION_POLICY, ops)
                .unwrap();
        let mut now = 0;
        loop {
            match collector.step(now) {
                Ok(JobCollectorEvent::RetryAt(at)) => now = at,
                Ok(JobCollectorEvent::Observe(request)) => {
                    collector
                        .observe(request, ObservationResult::Ready(request.signals), now)
                        .unwrap();
                }
                Ok(JobCollectorEvent::Progress) => {}
                Ok(JobCollectorEvent::Done) => break,
                Err(error) => {
                    assert_eq!(error.stage, JobKillStage::CloseChild);
                    assert_eq!(state.borrow().close_successes, 0);
                    now = now.saturating_add(1_000_000);
                }
            }
        }
        assert_eq!(state.borrow().close_successes, 2);
    }
}
