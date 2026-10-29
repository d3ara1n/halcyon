//! Job 与进程管理面：Building kill、枚举派生、封口、组合收束与自动退休。
use super::*;

/// Building 目标的确定性 kill：Create 后未 Start，kill 冻结终因
/// (Killed, code)，builder 关闭的 abandonment 竞争不覆盖；自动退休完成后
/// shell 快照应稳定报 Dead/Killed/code。
pub(super) fn test_building_kill(job: Handle) {
    let created = match process::create(job, memory_pool(), SUPERVISOR_RIGHTS) {
        Ok(created) => created,
        Err(error) => {
            debug!("building kill: create failed: {:?}", error);
            return;
        }
    };
    let before = process::query(created.control);
    if let Ok(snapshot) = before {
        debug!("building kill: initial state={}", snapshot.state);
    }
    if let Err(error) = process::kill(created.control) {
        debug!("building kill: kill failed: {:?}", error);
        let _ = unsafe { close(created.builder) };
        let _ = unsafe { close(created.control) };
        return;
    }
    let _ = unsafe { close(created.builder) };
    let drained = wait_until(
        created.control,
        ObjectSignals::DONE,
        libkernel::time::Deadline::INFINITE,
    );
    let snapshot = process::query(created.control);
    match (drained, snapshot) {
        (Ok(_), Ok(snapshot))
            if snapshot.state == ProcessState::Dead as u32
                && snapshot.cause == TerminationCause::Killed as u32
                && snapshot.detail
                    == libkernel::shared::proc::TerminationDetail::PROCESS_CONTROL_KILL =>
        {
            debug!(
                "building kill passed: pid {} Dead/Killed/{:#x}",
                created.pid, snapshot.detail.word0
            );
        }
        (work, snapshot) => {
            debug!(
                "building kill FAILED: drain={:?} snapshot={:?}",
                work, snapshot
            );
        }
    }
    let _ = unsafe { close(created.control) };
}

/// 验收线 1：枚举→派生→kill 通路。test_target 的 pid 经 acceptance Job
/// 枚举可见，JobDerive 派生 MANAGE control 后 kill 并监督收束；原保留
/// control 在派生接管后关闭（关闭 control 永不隐式终止）。
pub(super) fn test_derive_kill(
    harness: &mut Harness,
    job: Handle,
    pid: u64,
    retained: Handle,
) -> Result<(), &'static str> {
    let derived = (|| {
        let members = enumerate_members(job, JobMemberKind::MemberProcesses)?;
        if !members.contains(&pid) {
            debug!(
                "derive kill: pid {} missing among {} members",
                pid,
                members.len()
            );
            return Err(SystemCallError::ObjectNotFound);
        }
        debug!(
            "derive kill: pid {} visible among {} members",
            pid,
            members.len()
        );
        process::derive_job(
            job,
            JobMemberKind::MemberProcesses,
            pid,
            DERIVED_CONTROL_RIGHTS,
        )
    })();
    match derived {
        Ok(control) => {
            let _ = unsafe { close(retained) };
            let mut exhausted = DEFAULT_SUPERVISION_POLICY;
            exhausted.query_attempts = 0;
            let failure = collect_process(SupervisionTarget::new(pid, control), exhausted)
                .expect_err("invalid query policy must retain supervision authority");
            assert_eq!(failure.stage, SupervisionStage::WaitDone);
            assert_eq!(failure.cause, SupervisionCause::InvalidPolicy);
            assert_eq!(failure.progress.query_attempts, 0);
            debug!(
                "supervision policy rejection retained authority: pid={}, queries={}",
                pid, failure.progress.query_attempts
            );
            let mut machine = failure.collector;
            let mut finite = DEFAULT_SUPERVISION_POLICY;
            finite.wait_timeout_ms = 1;
            finite.wait_attempts = 1;
            machine.replenish(finite);
            let failure = loop {
                let now = libkernel::time::snapshot()
                    .expect("finite collection clock failed")
                    .now_ns;
                match machine.step(now) {
                    libprocess::StepOutcome::Continue(next) => machine = next,
                    libprocess::StepOutcome::RetryAt(at, next) => {
                        libkernel::time::sleep_until(libkernel::shared::time::Deadline::at(at))
                            .expect("finite collection retry wait failed");
                        machine = next;
                    }
                    libprocess::StepOutcome::Observe(request, mut next) => {
                        let observed =
                            wait_until(request.control, request.signals, request.deadline)
                                .expect("finite collection observation failed");
                        let now = libkernel::time::snapshot()
                            .expect("finite collection clock failed")
                            .now_ns;
                        let result = if observed.observed.raw() == 0 {
                            libprocess::observation::ObservationResult::TimedOut
                        } else {
                            libprocess::observation::ObservationResult::Ready(observed.observed)
                        };
                        next.observe(request, result, now)
                            .expect("finite collection observation rejected");
                        machine = next;
                    }
                    libprocess::StepOutcome::Escalate(failure) => break failure,
                    libprocess::StepOutcome::Done(_) => {
                        panic!("live target unexpectedly completed")
                    }
                }
            };
            assert_eq!(failure.stage, SupervisionStage::WaitDone);
            assert_eq!(failure.cause, SupervisionCause::Timeout);
            assert_eq!(failure.progress.wait_attempts, 1);
            assert_eq!(failure.progress.query_attempts, 0);
            assert_eq!(
                process::query(failure.collector.wait_control())
                    .unwrap()
                    .pid,
                pid
            );
            debug!(
                "supervision budget exhaustion retained authority: pid={}, waits={}, queries={}",
                pid, failure.progress.wait_attempts, failure.progress.query_attempts
            );
            // 超时交回同一机器和 authority；Kill 后续作不重建完成责任。
            process::kill(failure.collector.wait_control())
                .expect("derived-control kill must be accepted");
            let mut machine = failure.collector;
            machine.replenish(DEFAULT_SUPERVISION_POLICY);
            let collected = harness.collect_machine(machine)?;
            assert_eq!(collected.snapshot.cause, TerminationCause::Killed as u32);
            assert_eq!(
                collected.snapshot.detail,
                libkernel::shared::proc::TerminationDetail::PROCESS_CONTROL_KILL
            );
            debug!(
                "pid {} supervised: queries={}, state={}, cause={}, detail={}",
                pid,
                collected.progress.query_attempts,
                collected.snapshot.state,
                collected.snapshot.cause,
                collected.snapshot.detail.word0
            );
        }
        Err(error) => {
            debug!("derive kill FAILED: enumerate/derive {:?}", error);
            process::kill(retained).expect("live kill of a fresh process must be accepted");
            kill_and_collect(
                harness,
                alloc::vec::Vec::from([Supervised {
                    pid,
                    control: retained,
                }]),
            )?;
            return Err("derive kill enumeration or control derivation failed");
        }
    }
    Ok(())
}

/// Job 管理面验收入口（step 5）。真跨核竞态矩阵（含并发 Create/枚举
/// 乱序窗口）归 step 9 验证矩阵，此处只覆盖可确定性制造的场景。
pub(super) fn test_job_management(
    harness: &mut Harness,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    test_job_seal_completion(job);
    test_derive_fallback(job);
    test_job_kill_composition(harness, job, image)?;
    seal_before_start(job);
    seal_before_create(job);
    enumerate_convergence(job);
    let ecam = startup::ecam_handle();
    if let Some(ecam) = ecam {
        mmio_checks::process_retirement(ecam, job, image)?;
    }
    if mode().has(Mode::IRQ_EXIT) {
        let irq = startup::irq_handle();
        if let Some(irq) = irq {
            irq_checks::process_retirement(irq, job, image)?;
        }
    }
    let dma = startup::dma_handle();
    if let Some(dma) = dma {
        dma_checks::process_retirement(dma, job, image)?;
    }
    if mode().stress() {
        test_automatic_retirement(job, image)?;
    }
    Ok(())
}

/// 含大量 WaitSet 成员的进程退出后，观察 DONE 核验独立退休完成。
pub(super) fn test_automatic_retirement(job: Handle, image: &[u8]) -> Result<(), &'static str> {
    let marker = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::SIGNAL | Rights::WAIT,
    )
    .map_err(|_| "automatic retirement source creation failed")?;
    let observer = marker
        .owner
        .mint_observer(Rights::WAIT | Rights::DUPLICATE | Rights::GRANT)
        .map_err(|_| "automatic retirement observer mint failed")?;
    let child_observer = observer
        .duplicate(Rights::WAIT | Rights::GRANT)
        .map_err(|_| "automatic retirement observer duplicate failed")?;
    let ready = notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT)
        .map_err(|_| "automatic retirement ready creation failed")?;
    let grants = [
        HandleGrant {
            handle: marker.owner.as_handle(),
            rights: Rights::READ | Rights::WAIT,
        },
        HandleGrant {
            handle: ready.peer.as_handle(),
            rights: Rights::SIGNAL,
        },
        HandleGrant {
            handle: child_observer.as_handle(),
            rights: Rights::WAIT,
        },
    ];
    let launched = spawn(SpawnRequest {
        memory_pool: memory_pool(),
        parent: current_process(),
        job,
        image,
        payload: b"retirement",
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    });
    let started = match launched {
        Ok(started) => {
            marker.owner.into_raw();
            ready.peer.into_raw();
            child_observer.into_raw();
            started
        }
        Err(failure) => {
            if failure.grants == libprocess::GrantOutcome::Consumed {
                marker.owner.into_raw();
                ready.peer.into_raw();
                child_observer.into_raw();
            }
            return Err("automatic retirement spawn failed");
        }
    };
    let deadline =
        libkernel::time::timeout_millis(30_000).map_err(|_| "retirement deadline failed")?;
    let result = (|| {
        let ready_state = wait_until(ready.owner.as_handle(), ObjectSignals::READABLE, deadline)
            .map_err(|_| "automatic retirement target readiness failed")?;
        if !ready_state.observed.intersects(ObjectSignals::READABLE) {
            return Err("automatic retirement target readiness timed out");
        }
        process::kill(started.control).map_err(|_| "automatic retirement kill failed")?;
        let completion = wait_until(started.control, ObjectSignals::DONE, deadline)
            .map_err(|_| "automatic retirement wait failed")?;
        if !completion.observed.intersects(ObjectSignals::DONE) {
            return Err("automatic retirement completion timed out");
        }
        let snapshot = process::query(started.control).map_err(|_| "retirement snapshot failed")?;
        if snapshot.state != ProcessState::Dead as u32
            || snapshot.cause != TerminationCause::Killed as u32
        {
            return Err("automatic retirement terminal snapshot incorrect");
        }
        let observed = wait_until(observer.as_handle(), ObjectSignals::CLOSED, deadline)
            .map_err(|_| "retired Notification observer wait failed")?;
        if !observed.observed.intersects(ObjectSignals::CLOSED) {
            return Err("retired Notification owner remained alive");
        }
        let signaler = wait_until(marker.peer.as_handle(), ObjectSignals::CLOSED, deadline)
            .map_err(|_| "retired Notification signaler wait failed")?;
        if !signaler.observed.intersects(ObjectSignals::CLOSED)
            || marker.peer.signal(1) != Err(SystemCallError::ObjectClosed)
        {
            return Err("retired Notification signaler state incorrect");
        }
        debug!("automatic retirement acceptance passed: retired WaitSet with 256 registrations");
        debug!(
            "Notification observer process exit passed: owner closed with observer and signaler alive"
        );
        Ok(())
    })();
    if result.is_err() {
        let _ = process::kill(started.control);
        let _ = wait_until(started.control, ObjectSignals::DONE, deadline);
    }
    // SAFETY: 唯一父方 Control；退出/失败收束已等待同一完成来源。
    unsafe { close(started.control) }.map_err(|_| "automatic retirement control close failed")?;
    result
}

/// 验收线 2：封口与异步完成传播——空 child Job seal 后等待 CLOSED、
/// 快照转Dead；最后child capability关闭后父域仍可派生；只Seal父即收束空后代。
pub(super) fn test_job_seal_completion(job: Handle) {
    let Ok(child) = process::create_job(job, memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("job seal completion FAILED: job create failed");
        return;
    };
    let mut grandchild = None;
    let mut leaf = None;
    let nested = (|| {
        let grand = process::create_job(child, memory_pool(), JOB_FULL_RIGHTS)?;
        grandchild = Some(grand);
        leaf = Some(process::create_job(grand, memory_pool(), JOB_FULL_RIGHTS)?);
        let jid = process::query_job(grand)?.jid;
        // 原最后cap归零：重新派生依据父MANAGE，而非子旧Holders资格。
        unsafe { close(grand)? };
        grandchild = None;
        let reminted = process::derive_job(child, JobMemberKind::ChildJobs, jid, JOB_FULL_RIGHTS)?;
        grandchild = Some(reminted);
        unsafe { close(reminted)? };
        grandchild = None;
        Ok::<_, SystemCallError>(())
    })();
    let sealed = process::seal_job(child);
    let waited = wait_until(
        child,
        ObjectSignals::CLOSED,
        libkernel::time::timeout_millis(5_000).unwrap(),
    );
    let snapshot = process::query_job(child);
    let leaf_snapshot = leaf.and_then(|handle| process::query_job(handle).ok());
    // 幂管：Dead 上重复 seal 成功且不改变状态。
    let resealed = process::seal_job(child);
    let resnapshot = process::query_job(child);
    let children = enumerate_members(job, JobMemberKind::ChildJobs);
    let passed = nested.is_ok()
        && leaf_snapshot.is_some_and(|snapshot| snapshot.state == JobState::Dead as u32)
        && sealed.is_ok()
        && matches!(waited, Ok(result) if result.observed.intersects(ObjectSignals::CLOSED))
        && matches!(&snapshot, Ok(s) if s.state == JobState::Dead as u32)
        && resealed.is_ok()
        && matches!(&resnapshot, Ok(s) if s.state == JobState::Dead as u32)
        && matches!(&children, Ok(list) if !list.contains(&snapshot.as_ref().unwrap().jid));
    match (&snapshot, &children) {
        (Ok(snapshot), Ok(children)) => debug!(
            "job seal completion {} (jid {}, state {}, root children left {})",
            if passed { "passed" } else { "FAILED" },
            snapshot.jid,
            snapshot.state,
            children.len()
        ),
        (snapshot, children) => debug!(
            "job seal completion FAILED: snapshot={:?} children={:?}",
            snapshot, children
        ),
    }
    for handle in [leaf, grandchild].into_iter().flatten() {
        let _ = unsafe { close(handle) };
    }
    let _ = unsafe { close(child) };
}

/// Control 全消散不取消仍由 Builder 持有的组装；Job 枚举和派生接管终止。
pub(super) fn test_derive_fallback(job: Handle) {
    let Ok(created) = process::create(job, memory_pool(), SUPERVISOR_RIGHTS) else {
        debug!("derive fallback FAILED: create failed");
        return;
    };
    let pid = created.pid;
    // SAFETY: 原 Control 在本函数独占，Builder 仍稳定持有组装责任。
    let _ = unsafe { close(created.control) };
    let members = enumerate_members(job, JobMemberKind::MemberProcesses);
    let visible = matches!(&members, Ok(list) if list.contains(&pid));
    let minted = process::derive_job(
        job,
        JobMemberKind::MemberProcesses,
        pid,
        DERIVED_CONTROL_RIGHTS,
    );
    match minted {
        Ok(control) => {
            process::kill(control).expect("derived Control failed to stop Building target");
            // SAFETY: 冻结后退还唯一 Builder，自动退休不依赖其继续调用。
            let _ = unsafe { close(created.builder) };
            let drained = wait_until(
                control,
                ObjectSignals::DONE,
                libkernel::time::Deadline::INFINITE,
            );
            let snapshot = process::query(control);
            match (drained, snapshot) {
                (Ok(_), Ok(snapshot))
                    if snapshot.state == ProcessState::Dead as u32
                        && snapshot.cause == TerminationCause::Killed as u32
                        && snapshot.detail
                            == libkernel::shared::proc::TerminationDetail::PROCESS_CONTROL_KILL =>
                {
                    debug!(
                        "derive fallback passed: pid {} minted control completed at Dead/Killed/{:#x}",
                        pid, snapshot.detail.word0
                    );
                }
                (drained, snapshot) => debug!(
                    "derive fallback FAILED: visible={} completion={:?} snapshot={:?}",
                    visible, drained, snapshot
                ),
            }
            let _ = unsafe { close(control) };
        }
        Err(error) => {
            // SAFETY: 派生失败仍须放弃本函数拥有的 Builder。
            let _ = unsafe { close(created.builder) };
            debug!(
                "derive fallback FAILED: visible={} derive={:?}",
                visible, error
            );
        }
    }
}

/// 递归 JobKill 组合：child Job 内一个 Running 成员（Waiting 取消路径）
/// 以及一个 Building 成员，两者 control 均消散——libprocess::job_kill 一把
/// 收束（seal → 枚举 → 派生 kill → 等 DONE → 等 CLOSED 全链，派生走铸造
/// 路径）。
pub(super) fn test_job_kill_composition(
    harness: &mut Harness,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    let Ok(child) = harness.create_job(job, memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("job kill composition FAILED: job create failed");
        return Err("job kill composition job create failed");
    };
    let running = spawn(SpawnRequest {
        memory_pool: memory_pool(),
        parent: current_process(),
        job: child,
        image,
        payload: &[],
        grants: &[],
        control_rights: SUPERVISOR_RIGHTS,
    });
    let building = process::create(child, memory_pool(), SUPERVISOR_RIGHTS);
    match (running, building) {
        (Ok(running), Ok(building)) => {
            let running_pid = running.pid;
            let _ = unsafe { close(running.control) };
            let _ = unsafe { close(building.control) };
            match harness.collect_job(child) {
                Ok(()) => {
                    let snapshot = process::query_job(child);
                    match snapshot {
                        Ok(snapshot) if snapshot.state == JobState::Dead as u32 => debug!(
                            "job kill composition passed (member pid {}, child jid {} Dead)",
                            running_pid, snapshot.jid
                        ),
                        snapshot => {
                            debug!("job kill composition FAILED: child snapshot={:?}", snapshot);
                            return Err("job kill composition child snapshot failed");
                        }
                    }
                }
                Err(error) => {
                    debug!(
                        "job kill composition FAILED: stage={:?}, cause={:?}, retained_collector={}",
                        "root-owned", error, true
                    );
                    return Err("job kill composition collection failed");
                }
            }
        }
        (running, building) => {
            debug!(
                "job kill composition FAILED: running={:?} building={:?}",
                running.err(),
                building.err()
            );
            return Err("job kill composition member creation failed");
        }
    }
    harness.close_control(child)?;
    Ok(())
}

/// seal 先于 Start 的提交闸门：Building 成员在 seal 后 Start 返回
/// ObjectClosed（链锁内上行检查，两种线性化顺序的另一侧）；随后
/// kill/drain 收束，Job 因 sealed+空完成并发布 CLOSED。
pub(super) fn seal_before_start(job: Handle) {
    let Ok(child) = process::create_job(job, memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("seal gate (start) FAILED: job create failed");
        return;
    };
    // 手工构建可启动的 Building：入口页（自旋）+ 栈顶页（失败自清理）。
    let Ok(created) = build_spin_building(child, SUPERVISOR_RIGHTS) else {
        debug!("seal gate (start) FAILED: building");
        let _ = unsafe { close(child) };
        return;
    };
    let sealed = process::seal_job(child);
    let started = process::start(created.builder);
    let gated = matches!(started, Err(SystemCallError::ObjectClosed));
    // 收束：kill → drain → sealed+空完成 → CLOSED。
    let _ = process::kill(created.control);
    let _ = wait_until(
        created.control,
        ObjectSignals::DONE,
        libkernel::time::Deadline::INFINITE,
    );
    let _ = unsafe { close(created.control) };
    let _ = unsafe { close(created.builder) };
    let waited = wait_until(
        child,
        ObjectSignals::CLOSED,
        libkernel::time::timeout_millis(0).unwrap(),
    );
    let snapshot = process::query_job(child);
    let passed = sealed.is_ok()
        && gated
        && waited.is_ok()
        && matches!(&snapshot, Ok(s) if s.state == JobState::Dead as u32);
    debug!(
        "seal gate (start) {} (gated={}, state {:?})",
        if passed { "passed" } else { "FAILED" },
        gated,
        snapshot.as_ref().map(|s| s.state)
    );
    let _ = unsafe { close(child) };
}

/// seal 先于 Create：封口后成员/子 Job 创建口永久关闭（ObjectClosed）；
/// 空封口立即完成并发布 CLOSED。
pub(super) fn seal_before_create(job: Handle) {
    let Ok(child) = process::create_job(job, memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("seal gate (create) FAILED: job create failed");
        return;
    };
    let sealed = process::seal_job(child);
    let waited = wait_until(
        child,
        ObjectSignals::CLOSED,
        libkernel::time::timeout_millis(0).unwrap(),
    );
    let member = process::create(child, memory_pool(), SUPERVISOR_RIGHTS);
    let subjob = process::create_job(child, memory_pool(), JOB_FULL_RIGHTS);
    let passed = sealed.is_ok()
        && waited.is_ok()
        && matches!(member, Err(SystemCallError::ObjectClosed))
        && matches!(subjob, Err(SystemCallError::ObjectClosed));
    debug!(
        "seal gate (create) {} (member={:?} subjob={:?})",
        if passed { "passed" } else { "FAILED" },
        member.err(),
        subjob.err()
    );
    if let Ok(handle) = subjob {
        let _ = unsafe { close(handle) };
    }
    let _ = unsafe { close(child) };
}

/// 枚举收敛（可制造子集）：创建/收束交错下，新 Pid 单调递增、Dead
/// 后从成员表消失；连续两轮全量枚举无残留。真跨核并发窗口归 step 9。
pub(super) fn enumerate_convergence(job: Handle) {
    let mut ok = true;
    let mut previous = 0u64;
    for round in 0..4u64 {
        let Ok(created) = process::create(job, memory_pool(), SUPERVISOR_RIGHTS) else {
            ok = false;
            debug!(
                "enumerate convergence FAILED: create failed at round {}",
                round
            );
            break;
        };
        if created.pid <= previous {
            ok = false;
            debug!(
                "enumerate convergence FAILED: pid {} not monotonic after {}",
                created.pid, previous
            );
        }
        previous = created.pid;
        if let Err(error) = process::kill(created.control) {
            ok = false;
            debug!("enumerate convergence FAILED: kill {:?}", error);
            let _ = unsafe { close(created.builder) };
            let _ = unsafe { close(created.control) };
            continue;
        }
        let _ = unsafe { close(created.builder) };
        if let Err(error) = wait_until(
            created.control,
            ObjectSignals::DONE,
            libkernel::time::Deadline::INFINITE,
        ) {
            ok = false;
            debug!("enumerate convergence FAILED: drain {:?}", error);
        }
        let _ = unsafe { close(created.control) };
        match enumerate_members(job, JobMemberKind::MemberProcesses) {
            Ok(members) => {
                if members.contains(&created.pid) {
                    ok = false;
                    debug!(
                        "enumerate convergence FAILED: dead pid {} still enumerable",
                        created.pid
                    );
                }
            }
            Err(error) => {
                ok = false;
                debug!("enumerate convergence FAILED: enumerate {:?}", error);
            }
        }
    }
    debug!(
        "enumerate convergence {}",
        if ok { "passed" } else { "FAILED" }
    );
}
