//! pm：受托进程管理服务 + IPC 集成验证负载（执行核心任务驱动形态）。
//!
//! 责任全部作为执行核心任务运行：邮箱分发（派生流写入与流控任务）、
//! Runnel 流写入、流控发送与委托域收束。顺序语义由消息到达与来源状态
//! 保持，验收锚点与原顺序脚本一致；流的完成物由 main 在全部任务收空
//! 后显式关闭（发布 PEER_CLOSED，控制线程生命周期点）。
//!
//! pm 只持委托域的 JobControl（MANAGE|READ|WAIT，经 StartupBlock grants
//! 交付），域外无任何进程管理 authority；递归收束是本服务组合内核原语
//! 的用户态政策，init 保留直接收束权作兜底。

#![no_std]

use libprocess::job_driver::JobDriver;
use libprocess::{DEFAULT_SUPERVISION_POLICY, JobCollector};
use librunnel::blocking;
use libsrv::budget::{Budget, CoreResource};
use libsrv::runtime::{
    Advance, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind, Step, Task,
};
use rinlib::{
    env,
    ipc::{
        invitation::Invitation,
        message::{MailboxSender, MessageStorage, ReceiveBuffer},
        notification,
        wait_set::WaitSet,
    },
    preclude::*,
    shared::{
        call::SystemCallError,
        message::MAILBOX_CAPACITY,
        object::{Handle, ObjectSignals},
        time::Deadline,
    },
    sys_exit, sys_sleep,
};

const STREAM_LEN: usize = 65536;
/// 与 init 约定的流控验证消息号（见 init 的 WRITABLE_WAKE_* 常量）。
const WRITABLE_WAKE_REQUEST: u64 = 640;
const WRITABLE_WAKE_FILL: u64 = 641;
const WRITABLE_WAKE_TAIL: u64 = 642;

const KIND_MAILBOX: SourceKind = 1;
const KIND_STREAM: SourceKind = 2;
const KIND_WRITABLE: SourceKind = 3;

struct PmWorld {
    mailbox: Handle,
    domain: Handle,
    /// 流任务交付的完成物；全部任务收空后由 main 显式关闭。
    producer: Option<blocking::Producer>,
    fatal: bool,
    domain_done: bool,
}

impl PmWorld {
    fn fail(&mut self) {
        self.fatal = true;
    }
}

/// 邮箱分发任务：接收 Invitation 与流控请求，派生对应任务。
struct MailboxTask {
    buffer: ReceiveBuffer,
    registered: bool,
    dispatched: usize,
    failed: bool,
    stopping: bool,
}

impl MailboxTask {
    fn new() -> Self {
        Self {
            buffer: ReceiveBuffer::new().expect("pm: receive buffer"),
            registered: false,
            dispatched: 0,
            failed: false,
            stopping: false,
        }
    }
}

impl Task<PmWorld> for MailboxTask {
    type Family = PmTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut PmWorld,
        requests: &mut Requests<PmTask>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.failed {
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if !self.registered {
            self.registered = true;
            requests.add_source(
                world.mailbox,
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_MAILBOX,
            )?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let mut source = None;
        while let Some(event) = input.pull() {
            if event.error != 0 {
                self.failed = true;
                world.fail();
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            source = Some(event.source);
        }
        let Some(source) = source else {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        };
        match self.buffer.receive(world.mailbox) {
            Ok(()) => (),
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                // 虚假唤醒：保持登记继续停驻。
                requests.rearm(source)?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => {
                debug!("pm: mailbox receive failed: {:?}", error);
                world.fail();
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
        }
        let mut message =
            match MessageStorage::new().and_then(|storage| storage.take(&mut self.buffer)) {
                Ok(message) => message,
                Err(error) => {
                    debug!("pm: message extraction failed: {:?}", error);
                    world.fail();
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            };
        let header = message.header;
        if header.payload_len == 0 && header.handle_count == 1 {
            let owner = message.handles.take(0).expect("pm Invitation slot missing");
            match Invitation::from_capability(owner) {
                Ok(invitation) => {
                    if let Err(task) = requests.spawn(
                        PmTask::Stream(StreamTask {
                            invitation: Some(invitation),
                            producer: None,
                            cancelled_invitation: None,
                            removing: false,
                            source: None,
                            source_failed: None,
                            sent: 0,
                            stopping: false,
                        }),
                        2,
                    ) {
                        drop(task);
                        world.fail();
                        self.failed = true;
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Complete,
                        });
                    }
                    self.dispatched += 1;
                }
                Err(failure) => {
                    debug!("invalid Tunnel Invitation: {:?}", failure.error);
                    world.fail();
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        } else if header.kind == WRITABLE_WAKE_REQUEST && header.handle_count == 3 {
            let owner = message.handles.take(0).expect("pm target slot missing");
            let (target, _) = match MailboxSender::from_capability(owner) {
                Ok(adopted) => adopted,
                Err(failure) => {
                    debug!(
                        "pm: wake target is not a mailbox sender: {:?}",
                        failure.error
                    );
                    world.fail();
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            };
            let done = message.handles.take(1).expect("pm completion slot missing");
            let spin = message.handles.take(2).expect("pm wake slot missing");
            if let Err(task) = requests.spawn(
                PmTask::Flow(FlowTask {
                    target,
                    done: Some(done),
                    spin: Some(spin),
                    filled: 0,
                    verified_full: false,
                    woke: false,
                    source: None,
                    source_failed: None,
                    stopping: false,
                }),
                2,
            ) {
                drop(task);
                world.fail();
                self.failed = true;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            self.dispatched += 1;
        } else {
            debug!("unexpected message kind {}", header.kind);
            world.fail();
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.dispatched == 2 {
            Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            })
        } else {
            requests.rearm(source)?;
            Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            })
        }
    }

    fn refused(&mut self, world: &mut PmWorld, failure: RequestFailure<PmTask>) {
        match failure {
            RequestFailure::Spawn { task, error } => {
                drop(task);
                debug!("pm task spawn refused: {:?}", error);
                self.failed = true;
            }
            RequestFailure::Source { error, .. } => {
                debug!("pm mailbox source refused: {:?}", error);
                self.failed = true;
            }
        }
        world.fail();
    }

    fn stop(&mut self, _world: &mut PmWorld) {
        self.stopping = true;
    }
}

/// 流写入任务：attach 校验模式写入、EOF 发布后交付完成物。
struct StreamTask {
    invitation: Option<Invitation>,
    producer: Option<blocking::Producer>,
    cancelled_invitation: Option<rinlib::ipc::capability::Capability>,
    removing: bool,
    source: Option<SourceId>,
    source_failed: Option<SystemCallError>,
    stopping: bool,
    sent: usize,
}

impl Task<PmWorld> for StreamTask {
    type Family = PmTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut PmWorld,
        requests: &mut Requests<PmTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(invitation) = self.invitation.take() {
                self.cancelled_invitation = Some(invitation.into_capability());
            }
            if let Some(owner) = self.cancelled_invitation.take()
                && let Err((owner, error)) = owner.close()
            {
                self.cancelled_invitation = Some(owner);
                return Err(error);
            }
            if let Some(source) = self.source {
                if !self.removing {
                    requests.remove(source)?;
                    self.removing = true;
                }
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            if let Some(producer) = self.producer.take() {
                match producer.close() {
                    Ok(()) => {}
                    Err((producer, error)) => {
                        debug!("stream stop close failed: {:?}", error);
                        self.producer = Some(producer);
                        world.fail();
                        return Err(SystemCallError::InternalError);
                    }
                }
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.producer.is_none() {
            debug!("pm: stream attach phase");
            let invitation = self
                .invitation
                .take()
                .expect("stream task lost its invitation");
            match blocking::Producer::attach(invitation, rinlib::mm::Placement::Anywhere) {
                Ok(producer) => {
                    self.producer = Some(producer);
                    debug!("tunnel attached");
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
                Err(failure) => {
                    debug!("Tunnel attach failed: {:?}", failure);
                    world.fail();
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        if let Some(error) = self.source_failed.take() {
            debug!("stream source registration failed: {:?}", error);
            world.fail();
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        let producer = self
            .producer
            .as_mut()
            .expect("stream task lost its producer");
        // 事件先经 poll 重查（含 acknowledge 与终态判定）。
        let mut event_source = None;
        while let Some(event) = input.pull() {
            if event.kind != KIND_STREAM {
                continue;
            }
            event_source = Some(event.source);
            if event.error != 0 {
                debug!("stream source failed: error={}", event.error);
                world.fail();
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            match producer.poll(event.observed) {
                Ok(Some(condition)) => {
                    if matches!(condition, librunnel::ProducerReady::EofConsumed) {
                        // 未发布 EOF 前不应观察到全部消费；数据条件按 Writable 继续。
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    debug!("stream poll failed: {:?}", error);
                    world.fail();
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        if let Some(source) = event_source {
            self.source = Some(source);
        }
        // 校验模式写入：i%251+1，跨回绕分批，每批落页即通知。
        let mut work = 0;
        let mut chunk = [0u8; 512];
        while self.sent < STREAM_LEN && work < budget {
            let n = (STREAM_LEN - self.sent).min(chunk.len());
            for (i, byte) in chunk.iter_mut().enumerate().take(n) {
                *byte = ((self.sent + i) % 251 + 1) as u8;
            }
            match producer.write(&chunk[..n]) {
                Ok(0) => break,
                Ok(written) => {
                    self.sent += written;
                    work += 1;
                }
                Err(error) => {
                    debug!("stream write failed at {}: {:?}", self.sent, error);
                    world.fail();
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        if self.sent == STREAM_LEN {
            if let Err(error) = producer.finish() {
                debug!("finish failed: {:?}", error);
                world.fail();
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            debug!("stream written {} bytes", self.sent);
            world.producer = self.producer.take();
            return Ok(Advance {
                work_done: work.max(1),
                step: Step::Complete,
            });
        }
        // 未写满：按当前条件登记或重新武装。
        if let Some(source) = self.source {
            requests.rearm(source)?;
        } else {
            let plan = match producer.wait_plan() {
                Ok(plan) => plan,
                Err(error) => {
                    debug!("stream wait plan failed: {:?}", error);
                    world.fail();
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            };
            requests
                .arm_source(plan, KIND_STREAM)
                .map_err(|_| SystemCallError::ReachLimit)?;
        }
        Ok(Advance {
            work_done: work.max(1),
            step: Step::Parked,
        })
    }

    fn registered(&mut self, _world: &mut PmWorld, kind: SourceKind, source: SourceId) {
        if kind == KIND_STREAM {
            self.source = Some(source);
        }
    }
    fn unregistered(&mut self, _world: &mut PmWorld, _kind: SourceKind, source: SourceId) {
        if self.source == Some(source) {
            self.source = None;
            self.removing = false;
        }
    }

    fn refused(&mut self, _world: &mut PmWorld, failure: RequestFailure<PmTask>) {
        let error = match failure {
            RequestFailure::Spawn { error, .. } | RequestFailure::Source { error, .. } => error,
        };
        self.source_failed = Some(error);
    }

    fn stop(&mut self, _world: &mut PmWorld) {
        self.stopping = true;
    }
}

/// 流控任务：填满目标邮箱、确认后在 WRITABLE 上停驻，腾位唤醒补发末尾。
struct FlowTask {
    target: MailboxSender,
    done: Option<rinlib::ipc::capability::Capability>,
    spin: Option<rinlib::ipc::capability::Capability>,
    filled: usize,
    verified_full: bool,
    woke: bool,
    source: Option<SourceId>,
    source_failed: Option<SystemCallError>,
    stopping: bool,
}

impl Task<PmWorld> for FlowTask {
    type Family = PmTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut PmWorld,
        requests: &mut Requests<PmTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(done) = self.done.take() {
                match done.close() {
                    Ok(()) => {}
                    Err((done, error)) => {
                        self.done = Some(done);
                        return Err(error);
                    }
                }
            }
            if let Some(spin) = self.spin.take() {
                match spin.close() {
                    Ok(()) => {}
                    Err((spin, error)) => {
                        self.spin = Some(spin);
                        return Err(error);
                    }
                }
            }
            // MailboxSender remains owned by this task; its Drop closes the
            // non-mapping capability after the task reaches Retiring.
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if let Some(error) = self.source_failed.take() {
            debug!("writable source registration failed: {:?}", error);
            world.fail();
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.filled < MAILBOX_CAPACITY {
            let work = (MAILBOX_CAPACITY - self.filled).min(budget);
            for _ in 0..work {
                self.target
                    .send(WRITABLE_WAKE_FILL, &[])
                    .expect("wake fill failed");
                self.filled += 1;
            }
            return Ok(Advance {
                work_done: work.max(1),
                step: Step::Runnable,
            });
        }
        if !self.verified_full {
            assert!(matches!(
                self.target.send(WRITABLE_WAKE_FILL, &[]),
                Err(SystemCallError::MailboxFull)
            ));
            notification::signal(
                self.done
                    .as_ref()
                    .expect("flow completion owner missing")
                    .as_handle(),
                1,
            )
            .expect("wake confirm signal failed");
            self.verified_full = true;
            requests.add_source(
                self.target.as_handle(),
                ObjectSignals::WRITABLE | ObjectSignals::CLOSED,
                KIND_WRITABLE,
            )?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let mut source = None;
        while let Some(event) = input.pull() {
            if event.kind != KIND_WRITABLE {
                continue;
            }
            if event.error != 0 {
                world.fail();
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            if !event.observed.intersects(ObjectSignals::WRITABLE) {
                world.fail();
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            source = Some(event.source);
        }
        let Some(source) = source else {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        };
        self.source = Some(source);
        match self.target.send(WRITABLE_WAKE_TAIL, &[]) {
            Ok(()) => {
                debug!("writable wake passed");
                // 流控完成后进入委托域收束：保持原脚本顺序，避免与
                // init 装载域成员并发（域封口过早会拒绝后续 spawn）。
                if let Err(task) = requests.spawn(
                    PmTask::Domain(DomainTask::new(DEFAULT_SUPERVISION_POLICY)),
                    2,
                ) {
                    drop(task);
                    world.fail();
                }
                Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                })
            }
            Err(SystemCallError::MailboxFull) => {
                // 醒来后再撞满箱即为虚假唤醒（唤醒必须由腾位引起）。
                if self.woke {
                    notification::signal(
                        self.spin
                            .as_ref()
                            .expect("flow wake owner missing")
                            .as_handle(),
                        1,
                    )
                    .expect("spurious wake signal failed");
                }
                self.woke = true;
                requests.rearm(source)?;
                Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                })
            }
            Err(error) => {
                debug!("writable tail send failed: {:?}", error);
                world.fail();
                Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                })
            }
        }
    }

    fn refused(&mut self, world: &mut PmWorld, failure: RequestFailure<PmTask>) {
        match failure {
            RequestFailure::Spawn { task, error } => {
                drop(task);
                debug!("flow domain spawn refused: {:?}", error);
                world.fail();
            }
            RequestFailure::Source { error, .. } => self.source_failed = Some(error),
        }
    }

    fn stop(&mut self, _world: &mut PmWorld) {
        self.stopping = true;
    }
}

/// 委托域仅用正式 Job 机器收束；pm 的异常升级由进程退出交 init 监督。
struct DomainTask {
    policy: libprocess::SupervisionPolicy,
    driver: Option<JobDriver>,
    closing: bool,
}

impl DomainTask {
    fn new(policy: libprocess::SupervisionPolicy) -> Self {
        Self {
            policy,
            driver: None,
            closing: false,
        }
    }
}

impl Task<PmWorld> for DomainTask {
    type Family = PmTask;
    fn advance(
        &mut self,
        _id: u64,
        world: &mut PmWorld,
        requests: &mut Requests<PmTask>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.closing {
            // SAFETY: domain 是启动委托的唯一 control；Job 机器已完成且观察已注销。
            unsafe { rinlib::ipc::object::close(world.domain) }?;
            world.domain_done = true;
            debug!("pm: delegated domain managed");
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.driver.is_none() {
            self.driver = Some(JobDriver::new(JobCollector::new(
                world.domain,
                0x66,
                self.policy,
            )?));
        }
        let driver = self.driver.as_mut().expect("domain task owns its driver");
        let advance = driver.advance(requests, input, input.now_ns())?;
        if advance.step == Step::Complete {
            self.closing = true;
            debug!("pm: delegated domain seal passed");
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(advance)
    }
    fn registered(&mut self, _world: &mut PmWorld, kind: SourceKind, source: SourceId) {
        if let Some(driver) = &mut self.driver {
            driver.registered(kind, source);
        }
    }
    fn unregistered(&mut self, _world: &mut PmWorld, kind: SourceKind, source: SourceId) {
        if let Some(driver) = &mut self.driver {
            driver.unregistered(kind, source);
        }
    }
    fn refused(&mut self, world: &mut PmWorld, failure: RequestFailure<PmTask>) {
        match failure {
            RequestFailure::Source { kind, error } => {
                if let Some(driver) = &mut self.driver {
                    driver.refused(kind, error);
                }
            }
            RequestFailure::Spawn { .. } => world.fail(),
        }
    }
    // 已开始收束的域仍须完成，停止不取消管理责任。
    fn stop(&mut self, _world: &mut PmWorld) {}
    fn deadline(&self) -> Deadline {
        self.driver
            .as_ref()
            .map_or(Deadline::INFINITE, JobDriver::deadline)
    }
}

enum PmTask {
    Mailbox(MailboxTask),
    Stream(StreamTask),
    Flow(FlowTask),
    Domain(DomainTask),
}

impl Task<PmWorld> for PmTask {
    type Family = PmTask;

    fn advance(
        &mut self,
        id: u64,
        world: &mut PmWorld,
        requests: &mut Requests<PmTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Mailbox(task) => task.advance(id, world, requests, input, budget),
            Self::Stream(task) => task.advance(id, world, requests, input, budget),
            Self::Flow(task) => task.advance(id, world, requests, input, budget),
            Self::Domain(task) => task.advance(id, world, requests, input, budget),
        }
    }

    fn refused(&mut self, world: &mut PmWorld, failure: RequestFailure<PmTask>) {
        match self {
            Self::Mailbox(task) => task.refused(world, failure),
            Self::Stream(task) => task.refused(world, failure),
            Self::Flow(task) => task.refused(world, failure),
            Self::Domain(task) => task.refused(world, failure),
        }
    }

    fn registered(&mut self, world: &mut PmWorld, kind: SourceKind, source: SourceId) {
        match self {
            Self::Domain(task) => task.registered(world, kind, source),
            Self::Stream(task) => task.registered(world, kind, source),
            _ => {}
        }
    }
    fn unregistered(&mut self, world: &mut PmWorld, kind: SourceKind, source: SourceId) {
        match self {
            Self::Domain(task) => task.unregistered(world, kind, source),
            Self::Stream(task) => task.unregistered(world, kind, source),
            _ => {}
        }
    }

    fn stop(&mut self, world: &mut PmWorld) {
        match self {
            Self::Mailbox(task) => task.stop(world),
            Self::Stream(task) => task.stop(world),
            Self::Flow(task) => task.stop(world),
            Self::Domain(task) => task.stop(world),
        }
    }

    fn deadline(&self) -> Deadline {
        match self {
            Self::Mailbox(_) | Self::Stream(_) | Self::Flow(_) => Deadline::INFINITE,
            Self::Domain(task) => task.deadline(),
        }
    }
}

fn main() {
    debug!("Hello, pm!");
    // 服务出生自带的邮箱 owner（StartupBlock Handle[0]）。
    let mailbox = env::startup_handle(0).expect("pm: mailbox owner grant is missing");
    // sleep 异步通路验证：登记期限 → Waiting → timer 唤醒 → 继续。
    unsafe {
        sys_sleep(30).expect("sleep");
        sys_sleep(10).expect("sleep again");
    }
    debug!("awake after two sleeps");
    // StartupBlock Handle[1] = init 授出的 pm_domain JobControl。
    let domain = env::startup_handle(1).expect("pm: delegated domain control is missing");

    // 执行核心：任务与输入额度在公开前准备。
    let budget = Budget::<CoreResource>::new(&[16, 64 * 1024], 1).expect("pm: budget");
    let account = budget.account(&[16, 64 * 1024]).expect("pm: account");
    let set = WaitSet::create(64).expect("pm: wait set");
    let mut runtime =
        Runtime::<PmTask, WaitSet>::new(set, 16, 16, CoreResource::EXECUTION_SLOTS, &account)
            .expect("pm: runtime");

    let mut world = PmWorld {
        mailbox,
        domain,
        producer: None,
        fatal: false,
        domain_done: false,
    };
    if let Err(failure) = runtime.spawn(PmTask::Mailbox(MailboxTask::new()), 2) {
        debug!("pm: mailbox task refused: {:?}", failure.error);
        match unsafe { sys_exit(-1) } {
            Ok(()) => unreachable!("pm: mailbox failure exit unexpectedly returned"),
            Err(error) => panic!("pm: mailbox failure exit failed: {:?}", error),
        }
    }

    let mut stopping = false;
    loop {
        if world.fatal {
            exit_failed();
        }
        if world.domain_done && !stopping {
            runtime.seal();
            stopping = true;
        }
        let result = if stopping {
            runtime.shutdown_turn(&mut world, 1)
        } else {
            runtime.turn(&mut world, 1)
        };
        if let Err(failure) = result {
            debug!(
                "pm: runtime failure: task={}, error={:?}",
                failure.task, failure.error
            );
            exit_failed();
        }
        if world.fatal {
            exit_failed();
        }
        match runtime.drive_state() {
            libsrv::runtime::DriveState::Drained => break,
            libsrv::runtime::DriveState::Runnable => {}
            libsrv::runtime::DriveState::Waiting(deadline) => {
                if world.domain_done && !stopping {
                    continue;
                }
                if let Err(error) = rinlib::ipc::wait::wait_until(&[runtime.wait_item(0)], deadline)
                {
                    debug!("pm: runtime wait failed: {:?}", error);
                    exit_failed();
                }
                runtime.notified();
            }
        }
    }

    // 显式收尾：完成物在全部任务收空后关闭（发布 PEER_CLOSED）。
    if let Some(producer) = world.producer.take()
        && let Err((_retained_producer, error)) = producer.close()
    {
        debug!("pm: producer close failed: {:?}", error);
        exit_failed();
    }
    if let Err((_runtime, error)) = runtime.close() {
        debug!("pm: runtime close failed: {:?}", error);
        match unsafe { sys_exit(-1) } {
            Ok(()) => unreachable!("pm: close failure exit unexpectedly returned"),
            Err(error) => panic!("pm: close failure exit failed: {:?}", error),
        }
    }
    debug!("pm: shutdown complete");
}

/// pm 有明确上级监督者；不可恢复失败在原 owner 仍存活时退出进程。
fn exit_failed() -> ! {
    match unsafe { sys_exit(-1) } {
        Ok(()) => unreachable!("pm failure exit returned"),
        Err(error) => panic!("pm failure exit failed: {:?}", error),
    }
}
