// —— 生命周期多核竞态矩阵（step 9）——
//
// 双锤（test_hammer HAMMER 模式）常驻 acceptance 域：指令与 handle 经
// Mailbox 投递，发令枪（notification READABLE 电平）连发后双锤/靶在
// 各自 hart 上同刻开打，锤真跨核竞态窗口。剧本与断言全在 init：每轮
// 以「终因组合合法 + Dead 收束 + 无泄漏」为强断言，终因胜负分布只作
// 观察报告（证明锤进了窗口，不作通过条件）。

use core::{cell::Cell, sync::atomic::Ordering};

use crate::{
    JOB_FULL_RIGHTS, RootSupervisor, SUPERVISOR_RIGHTS, Supervised, building::build_spin_building,
};
use libkernel::ipc::message::{create, send_raw, wait_message};
use libkernel::ipc::object::{close, duplicate};
use libkernel::ipc::wait::wait_until;
use libkernel::preclude::*;
use libkernel::process;
use libkernel::shared::call::SystemCallError;
use libkernel::shared::message::HandleMove;
use libkernel::shared::object::{Handle, ObjectSignals, Rights};
use libkernel::shared::proc::{
    HandleGrant, JobMemberKind, JobState, ProcessState, TerminationCause,
};

use libkernel::sys_sleep;
use libprocess::{
    DERIVED_CONTROL_RIGHTS, SpawnRequest, Spawned,
    race::{self, Cmd, Report},
    spawn,
};

// 竞态Control会通过有缓冲指令运输；出生时取得该许可，不能在Duplicate时增权。
const RACE_CONTROL_RIGHTS: Rights =
    Rights::from_raw(SUPERVISOR_RIGHTS.raw() | Rights::TRANSIT.raw());

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandState {
    Idle,
    Pending,
    Failed,
}

/// 竞态锤编队：两执行器 + 每锤独立指令箱/回执箱/发令枪（回执按锤
/// 分箱，天然归属，无需锤标识）。
struct RaceHammers {
    cmd: [Handle; 2],
    guns: [Handle; 2],
    report: [Handle; 2],
    controls: [Handle; 2],
    pids: [u64; 2],
    report_pending: [Cell<CommandState>; 2],
}

impl RaceHammers {
    fn spawn_pair(job: Handle, image: &[u8]) -> Result<Self, SystemCallError> {
        let mut set = Self {
            cmd: [Handle::INVALID; 2],
            guns: [Handle::INVALID; 2],
            report: [Handle::INVALID; 2],
            controls: [Handle::INVALID; 2],
            pids: [0; 2],
            report_pending: [Cell::new(CommandState::Idle), Cell::new(CommandState::Idle)],
        };
        for i in 0..2 {
            let cmd_pair = create(
                Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
                // 指令携带 HandleMove（transit 暂存）需要 TRANSIT 位。
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            )?;
            let report_pair = create(
                Rights::READ | Rights::WAIT | Rights::MANAGE,
                Rights::WRITE | Rights::WAIT | Rights::GRANT | Rights::DUPLICATE,
            )?;
            let gun_pair = {
                let mut pair =
                    libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */
                unsafe {
                    libkernel::sys_notification_create(
                        Rights::READ | Rights::WAIT | Rights::GRANT,
                        Rights::SIGNAL,
                        &mut pair,
                    )
                }
                .map(|()| pair)
            }?;
            let grants = [
                HandleGrant {
                    handle: cmd_pair.owner,
                    rights: Rights::READ | Rights::WAIT,
                },
                HandleGrant {
                    handle: report_pair.peer,
                    rights: Rights::WRITE,
                },
                HandleGrant {
                    handle: gun_pair.owner,
                    rights: Rights::READ | Rights::WAIT,
                },
            ];
            let payload = race::encode_payload(&[race::MODE_HAMMER]);
            let spawned = spawn(SpawnRequest {
                job,
                memory_pool: crate::root_memory_pool(),
                parent: crate::init_process_info(),
                image,
                payload: &payload,
                grants: &grants,
                control_rights: SUPERVISOR_RIGHTS,
            })
            .map_err(|_| SystemCallError::Unknown)?;
            debug!("race hammer {} spawned as pid {}", i, spawned.pid);
            set.cmd[i] = cmd_pair.peer;
            set.report[i] = report_pair.owner;
            set.guns[i] = gun_pair.peer;
            set.controls[i] = spawned.control;
            set.pids[i] = spawned.pid;
        }
        Ok(set)
    }

    fn send_cmd(&self, hammer: usize, cmd: &Cmd, moves: &[HandleMove]) -> bool {
        if self.report_pending[hammer].get() != CommandState::Idle {
            return false;
        }
        if matches!(
            cmd.action,
            race::ACTION_CREATE | race::ACTION_CREATE_ABANDON
        ) {
            if moves.len() != 1 {
                return false;
            }
            let Ok(pool) = duplicate(crate::root_memory_pool(), Rights::CREATE | Rights::TRANSIT)
            else {
                return false;
            };
            let funding = HandleMove {
                handle: pool,
                rights: Rights::CREATE,
            };
            let result = unsafe {
                send_raw(
                    self.cmd[hammer],
                    race::MSG_CMD,
                    &race::encode_cmd(cmd),
                    &[moves[0], funding],
                )
            };
            if result.is_err() {
                let _ = unsafe { close(pool) };
            }
            self.report_pending[hammer].set(if result.is_ok() {
                CommandState::Pending
            } else {
                CommandState::Idle
            });
            return result.is_ok();
        }
        let result = unsafe {
            send_raw(
                self.cmd[hammer],
                race::MSG_CMD,
                &race::encode_cmd(cmd),
                moves,
            )
        };
        self.report_pending[hammer].set(if result.is_ok() {
            CommandState::Pending
        } else {
            CommandState::Idle
        });
        if let Err(error) = result {
            debug!(
                "race command send failed: hammer={}, action={}, error={:?}",
                hammer, cmd.action, error
            );
        }
        result.is_ok()
    }

    fn fire(&self, hammers: &[usize]) {
        for &i in hammers {
            let _ = /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */ unsafe { libkernel::sys_notification_signal(self.guns[i], 1) };
        }
    }

    fn report(&self, hammer: usize) -> Option<(Report, alloc::vec::Vec<u64>)> {
        if self.report_pending[hammer].get() != CommandState::Pending {
            return None;
        }
        let message = match wait_message(self.report[hammer]) {
            Ok(message) => message,
            Err(error) => {
                // 失败可能保留旧消息；隔离此锤到监督退出，禁止下一指令复用回执。
                self.report_pending[hammer].set(CommandState::Failed);
                debug!(
                    "race report receive failed: hammer={}, error={:?}",
                    hammer, error
                );
                return None;
            }
        };
        self.report_pending[hammer].set(CommandState::Idle);
        if message.header.kind != race::MSG_REPORT {
            return None;
        }
        race::decode_report(&message.payload)
    }

    /// 指令一发：投递 → 发令 → 回执。
    fn shoot(
        &self,
        hammer: usize,
        cmd: &Cmd,
        moves: &[HandleMove],
    ) -> Option<(Report, alloc::vec::Vec<u64>)> {
        if !self.send_cmd(hammer, cmd, moves) {
            return None;
        }
        self.fire(&[hammer]);
        self.report(hammer)
    }

    /// 退场指令 + 锤侧自有 handle 由进程退出收束；init 端点随后清理。
    fn shutdown(&self) {
        for i in 0..2 {
            let exit = Cmd {
                action: race::ACTION_EXIT,
                token: 0,
                aux: 0,
            };
            if self.send_cmd(i, &exit, &[]) {
                self.fire(&[i]);
                if self.report(i).is_none() {
                    let _ = process::kill(self.controls[i]);
                }
            } else {
                let _ = process::kill(self.controls[i]);
            }
            let _ = unsafe { close(self.cmd[i]) };
            let _ = unsafe { close(self.report[i]) };
            let _ = unsafe { close(self.guns[i]) };
        }
    }
}

fn race_cmd(action: u64, token: u64) -> Cmd {
    Cmd {
        action,
        token,
        aux: 0,
    }
}

/// 延迟变体指令：锤醒后先 sleep `delay_ms` 再执行，把窗口让给对侧
/// 先行（时序变体用，见 race::Cmd::aux）。
fn race_cmd_delayed(action: u64, token: u64, delay_ms: u64) -> Cmd {
    Cmd {
        action,
        token,
        aux: delay_ms,
    }
}

/// 竞态靶（test_hammer TARGET 模式）：等枪后按 subrole 自灭或高频 park。
/// 返回 (spawned, 靶枪 signaler)。
fn spawn_race_target(
    job: Handle,
    image: &[u8],
    subrole: u64,
    code: u64,
) -> Result<(Spawned, Handle), SystemCallError> {
    let gun_pair = {
        let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */
        unsafe {
            libkernel::sys_notification_create(
                Rights::READ | Rights::WAIT | Rights::GRANT,
                Rights::SIGNAL,
                &mut pair,
            )
        }
        .map(|()| pair)
    }?;
    let payload = race::encode_payload(&[race::MODE_TARGET, subrole, code]);
    let mut grants = alloc::vec![HandleGrant {
        handle: gun_pair.owner,
        rights: Rights::READ | Rights::WAIT
    }];
    let pool_grant = if subrole == race::TARGET_THREAD_SUITE {
        let pool = duplicate(
            crate::root_memory_pool(),
            Rights::CREATE | Rights::READ | Rights::GRANT,
        )?;
        grants.push(HandleGrant {
            handle: pool,
            rights: Rights::CREATE | Rights::READ,
        });
        Some(pool)
    } else {
        None
    };
    let spawned = spawn(SpawnRequest {
        job,
        memory_pool: crate::root_memory_pool(),
        parent: crate::init_process_info(),
        image,
        payload: &payload,
        grants: &grants,
        control_rights: RACE_CONTROL_RIGHTS,
    })
    .map_err(|failure| {
        if failure.grants == libprocess::GrantOutcome::Retained
            && let Some(pool) = pool_grant
        {
            // SAFETY: 未移交的新增付款表项仍由该工厂持有。
            let _ = unsafe { close(pool) };
        }
        SystemCallError::Unknown
    })?;
    Ok((spawned, gun_pair.peer))
}

fn spawn_tunnel_exit_target(
    job: Handle,
    image: &[u8],
    invitation: Handle,
    mapped_bytes: usize,
) -> Result<(Spawned, Handle), SystemCallError> {
    let gun_pair = {
        let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */
        unsafe {
            libkernel::sys_notification_create(
                Rights::READ | Rights::WAIT | Rights::GRANT,
                Rights::SIGNAL,
                &mut pair,
            )
        }
        .map(|()| pair)
    }?;
    let grants = [
        HandleGrant {
            handle: gun_pair.owner,
            rights: Rights::READ | Rights::WAIT,
        },
        HandleGrant {
            handle: invitation,
            // affine Attach 消费 MANAGE；派生 Endpoint 的 View 需要共享读写。
            rights: Rights::READ | Rights::WRITE | Rights::MANAGE,
        },
    ];
    let payload = race::encode_payload(&[
        race::MODE_TARGET,
        race::TARGET_TUNNEL_EXIT,
        mapped_bytes as u64,
    ]);
    match spawn(SpawnRequest {
        job,
        memory_pool: crate::root_memory_pool(),
        parent: crate::init_process_info(),
        image,
        payload: &payload,
        grants: &grants,
        control_rights: RACE_CONTROL_RIGHTS,
    }) {
        Ok(spawned) => Ok((spawned, gun_pair.peer)),
        Err(_) => {
            let _ = unsafe { close(gun_pair.owner) };
            let _ = unsafe { close(gun_pair.peer) };
            let _ = unsafe { close(invitation) };
            Err(SystemCallError::Unknown)
        }
    }
}

/// 收束到 Dead 后断言终因在允许集合内；返回实际终因，违约返回 None。
/// 先等 DONE 电平
/// （自动退休 仅在 终态/Dead 上适用，kill 后线程退出路径是
/// 异步的，不能立即 drain）。
fn observe_dead(control: Handle, allowed: &[u32]) -> Option<u32> {
    if !await_done(control) {
        return None;
    }
    let Ok(snapshot) = process::query(control) else {
        return None;
    };
    if snapshot.state != ProcessState::Dead as u32 {
        debug!("race: terminal state {} is not Dead", snapshot.state);
        return None;
    }
    for cause in allowed {
        if snapshot.cause == *cause {
            return Some(snapshot.cause);
        }
    }
    debug!("race: terminal cause {} not in allowed set", snapshot.cause);
    None
}

/// DONE 表示自动退休已完成，调用者只观察，不承担推进责任。
fn await_done(control: Handle) -> bool {
    match wait_until(
        control,
        ObjectSignals::DONE,
        libkernel::time::Deadline::INFINITE,
    ) {
        Ok(_) => true,
        Err(error) => {
            debug!("race: completion wait failed: {:?}", error);
            false
        }
    }
}

/// 观察仍在Job内的对象；DONE后已移除成员同样表示收束完成。
/// pid来自已成功创建的同域靶，不能把历史墓碑作为派生前置。
fn observe_member_completion(job: Handle, pid: u64, allowed: &[u32]) -> bool {
    match process::derive_job(
        job,
        JobMemberKind::MemberProcesses,
        pid,
        DERIVED_CONTROL_RIGHTS,
    ) {
        Ok(control) => {
            let completed = observe_dead(control, allowed).is_some();
            let _ = unsafe { close(control) };
            completed
        }
        Err(SystemCallError::ObjectNotFound) => true,
        Err(error) => {
            debug!("race: derive pid {} failed: {:?}", pid, error);
            false
        }
    }
}

/// 靶枪与锤枪连发（fire 顺序无关紧要：电平语义不丢令，窗口差只是一个
/// ecall）。
fn fire_race(h: &RaceHammers, target_gun: Handle, hammers: &[usize]) {
    let _ = /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */ unsafe { libkernel::sys_notification_signal(target_gun, 1) };
    h.fire(hammers);
}

/// kill vs kill（重复 kill 竞争）：双锤同时对高频 park 靶 kill。断言双
/// Ok（幂等仲裁）+ 终因为两 code 之一 + Dead。观察：code 胜负分布。
fn race_kill_kill(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    let mut wins = [0usize; 2];
    for round in 0..4u64 {
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_PARK, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race kill-vs-kill: target spawn failed: {:?}", error);
                return false;
            }
        };
        let codes = [0x100 + round as i64, 0x200 + round as i64];
        // control 双 dup，原件留 init 收束断言。
        let moves = [
            HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                    .unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            },
            HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                    .unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            },
        ];
        let kill_a = race_cmd(race::ACTION_KILL, codes[0] as u64);
        let kill_b = race_cmd(race::ACTION_KILL, codes[1] as u64);
        let sent = h.send_cmd(0, &kill_a, &[moves[0]]) && h.send_cmd(1, &kill_b, &[moves[1]]);
        fire_race(h, gun, &[0, 1]);
        let (rep_a, _) = match h.report(0) {
            Some(pair) => pair,
            None => {
                debug!("race kill-vs-kill: hammer 0 report missing");
                ok = false;
                break;
            }
        };
        let (rep_b, _) = match h.report(1) {
            Some(pair) => pair,
            None => {
                debug!("race kill-vs-kill: hammer 1 report missing");
                ok = false;
                break;
            }
        };
        let kills_ok =
            rep_a.status == 0 && rep_b.status == 0 && (rep_a.aux0 == 0) != (rep_b.aux0 == 0);
        let allowed = [
            TerminationCause::Killed as u32,
            TerminationCause::Killed as u32,
        ];
        let terminal = observe_dead(target.control, &allowed);
        if terminal != Some(TerminationCause::Killed as u32) {
            ok = false;
        } else if rep_a.aux0 == 0 {
            wins[0] += 1;
        } else if rep_b.aux0 == 0 {
            wins[1] += 1;
        }
        if !sent || !kills_ok {
            debug!(
                "race kill-vs-kill: round {} sent={} rep={:#x}/{:#x}",
                round, sent, rep_a.status, rep_b.status
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
    }
    debug!(
        "race kill-vs-kill {} (accepted wins {}/{})",
        if ok { "passed" } else { "FAILED" },
        wins[0],
        wins[1]
    );
    ok
}

/// kill vs ProcessTerminate：靶自杀与锤 kill 同刻起跑，终因 Completed/Killed 二者
/// 恰一，cause 判定。奇数轮锤延迟 1ms 再 kill：靶的主动结束 先
/// 线性化，观察 Completed 胜出侧（kill 后到幂等）。
fn race_kill_exit(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    let mut dist = [0usize; 2];
    for round in 0..4u64 {
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_SUICIDE, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race kill-vs-exit: target spawn failed: {:?}", error);
                return false;
            }
        };
        let moves = [HandleMove {
            handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                .unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let kill = if round % 2 == 1 {
            race_cmd_delayed(race::ACTION_KILL, 0, 10)
        } else {
            race_cmd(race::ACTION_KILL, 0)
        };
        let sent = h.send_cmd(0, &kill, &moves);
        fire_race(h, gun, &[0]);
        let (rep, _) = h.report(0).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let allowed = [
            TerminationCause::Completed as u32,
            TerminationCause::Killed as u32,
        ];
        let terminal = observe_dead(target.control, &allowed);
        match terminal {
            Some(cause) if cause == TerminationCause::Completed as u32 => dist[0] += 1,
            Some(cause) if cause == TerminationCause::Killed as u32 => dist[1] += 1,
            _ => ok = false,
        }
        if !sent || rep.status != 0 {
            debug!(
                "race kill-vs-exit: round {} sent={} rep={:#x}",
                round, sent, rep.status
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
    }
    debug!(
        "race kill-vs-exit {} (exited {}/{} killed)",
        if ok { "passed" } else { "FAILED" },
        dist[0],
        dist[1]
    );
    ok
}

/// Running 期 ThreadCreate 与异 hart ProcessKill 竞速。偶数轮 kill 同刻提交，
/// 奇数轮延迟 kill 让 spawn 先入册；无论线性化次序，新增成员都必须被终止收束。
fn race_thread_spawn_kill(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    for round in 0..4u64 {
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_THREAD_SPAWN_RACE, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race spawn-vs-kill: target spawn failed: {:?}", error);
                return false;
            }
        };
        let moves = [HandleMove {
            handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                .unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let kill = if round % 2 == 1 {
            race_cmd_delayed(race::ACTION_KILL, 0, 10)
        } else {
            race_cmd(race::ACTION_KILL, 0)
        };
        let sent = h.send_cmd(0, &kill, &moves);
        fire_race(h, gun, &[0]);
        let report = h.report(0).map(|(report, _)| report);
        let terminal = observe_dead(target.control, &[TerminationCause::Killed as u32]);
        if !sent || !matches!(report, Some(report) if report.status == 0) || terminal.is_none() {
            debug!(
                "race spawn-vs-kill: round {} sent={} report={:?}",
                round,
                sent,
                report.map(|report| report.status)
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
    }
    debug!(
        "race spawn-vs-kill {}",
        if ok { "passed" } else { "FAILED" }
    );
    ok
}

/// 在真实竞速前先用明确的顺序覆盖两种合法终因：一轮只放行末线程
/// Completed，一轮在发枪前 Kill。该覆盖不依赖调度器随机选择胜者。
fn cover_last_thread_exit_kill_outcomes(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let (exit_target, exit_gun) =
        match spawn_race_target(job, image, race::TARGET_LAST_THREAD_EXIT_RACE, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!(
                    "race last-thread coverage: exit-first target spawn failed: {:?}",
                    error
                );
                return false;
            }
        };
    // 目标主线程先退出，次线程在枪上等待；发枪后终因确定为 Completed。
    unsafe { sys_sleep(30).expect("last-thread exit-first setup sleep failed") };
    let exit_signal = /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */ unsafe { libkernel::sys_notification_signal(exit_gun, 1) }.is_ok();
    let exited = observe_dead(exit_target.control, &[TerminationCause::Completed as u32]).is_some();
    let _ = unsafe { close(exit_target.control) };
    let _ = unsafe { close(exit_gun) };

    let (kill_target, kill_gun) =
        match spawn_race_target(job, image, race::TARGET_LAST_THREAD_EXIT_RACE, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!(
                    "race last-thread coverage: kill-first target spawn failed: {:?}",
                    error
                );
                return false;
            }
        };
    unsafe { sys_sleep(30).expect("last-thread kill-first setup sleep failed") };
    let report = h.shoot(
        0,
        &race_cmd(race::ACTION_KILL, 0),
        &[HandleMove {
            handle: duplicate(kill_target.control, Rights::MANAGE | Rights::TRANSIT)
                .unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }],
    );
    let killed = matches!(report, Some((report, _)) if report.status == 0)
        && observe_dead(kill_target.control, &[TerminationCause::Killed as u32]).is_some();
    let _ = unsafe { close(kill_target.control) };
    let _ = unsafe { close(kill_gun) };

    let ok = exit_signal && exited && killed;
    debug!(
        "race last-thread deterministic coverage {}: exited={} killed={}",
        if ok { "passed" } else { "FAILED" },
        exited,
        killed
    );
    ok
}

/// 主线程先离场，只剩等待发令枪的次线程；其最后 ThreadExit 与 ProcessKill
/// 竞争进程终因。随机轮次保留真实竞速，但不把胜负分布当作通过条件。
fn race_last_thread_exit_kill(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = cover_last_thread_exit_kill_outcomes(h, job, image);
    let mut dist = [0usize; 2];
    for round in 0..4u64 {
        let (target, gun) =
            match spawn_race_target(job, image, race::TARGET_LAST_THREAD_EXIT_RACE, 0) {
                Ok(pair) => pair,
                Err(error) => {
                    debug!(
                        "race last-thread-exit-vs-kill: target spawn failed: {:?}",
                        error
                    );
                    return false;
                }
            };
        // 让目标主线程完成 raw spawn 并先行 ThreadExit，确保竞速者是末线程。
        // SAFETY: 值参数；只用于建立目标角色的前置状态。
        let _ = unsafe { sys_sleep(30) };
        let moves = [HandleMove {
            handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                .unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let kill = if round % 2 == 1 {
            race_cmd_delayed(race::ACTION_KILL, 0, 10)
        } else {
            race_cmd(race::ACTION_KILL, 0)
        };
        let sent = h.send_cmd(0, &kill, &moves);
        if round % 2 == 0 {
            h.fire(&[0]);
            // SAFETY: kill 先行窗口；signal 失败表示 owner 已随终止关闭，同样合法。
            let _ = unsafe { sys_sleep(1) };
            let _ = /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */ unsafe { libkernel::sys_notification_signal(gun, 1) };
        } else {
            fire_race(h, gun, &[0]);
        }
        let report = h.report(0).map(|(report, _)| report);
        let allowed = [
            TerminationCause::Completed as u32,
            TerminationCause::Killed as u32,
        ];
        match observe_dead(target.control, &allowed) {
            Some(cause) if cause == TerminationCause::Completed as u32 => dist[0] += 1,
            Some(cause) if cause == TerminationCause::Killed as u32 => dist[1] += 1,
            _ => ok = false,
        }
        if !sent || !matches!(report, Some(report) if report.status == 0) {
            debug!(
                "race last-thread-exit-vs-kill: round {} sent={} report={:?}",
                round,
                sent,
                report.map(|report| report.status)
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
    }
    // 竞态胜负由真实调度决定；单侧偏胜仍可能完全合法。两种终因的
    // 确定性覆盖由本场景的顺序变体负责，随机轮次只报告实际分布。
    debug!(
        "race last-thread-exit-vs-kill {} (observed exited {}/{} killed)",
        if ok { "passed" } else { "FAILED" },
        dist[0],
        dist[1]
    );
    ok
}

/// kill vs fault：靶解引用空指针与锤 kill 同刻起跑，终因 Fault(code=
/// LoadAccess)/Killed 二者恰一。奇数轮锤延迟 1ms 再 kill：靶的 fault
/// 先线性化，观察 Fault 胜出侧（kill 后到幂等）。
fn race_kill_fault(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    let mut dist = [0usize; 2];
    for round in 0..4u64 {
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_FAULT, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race kill-vs-fault: target spawn failed: {:?}", error);
                return false;
            }
        };
        let moves = [HandleMove {
            handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                .unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let kill = if round % 2 == 1 {
            race_cmd_delayed(race::ACTION_KILL, 0, 10)
        } else {
            race_cmd(race::ACTION_KILL, 0)
        };
        let sent = h.send_cmd(0, &kill, &moves);
        fire_race(h, gun, &[0]);
        let (rep, _) = h.report(0).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let allowed = [
            TerminationCause::Faulted as u32,
            TerminationCause::Killed as u32,
        ];
        let terminal = observe_dead(target.control, &allowed);
        match terminal {
            Some(cause) if cause == TerminationCause::Faulted as u32 => dist[0] += 1,
            Some(cause) if cause == TerminationCause::Killed as u32 => dist[1] += 1,
            _ => ok = false,
        }
        if !sent || rep.status != 0 {
            debug!(
                "race kill-vs-fault: round {} sent={} rep={:#x}",
                round, sent, rep.status
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
    }
    debug!(
        "race kill-vs-fault {} (fault {}/{} killed)",
        if ok { "passed" } else { "FAILED" },
        dist[0],
        dist[1]
    );
    ok
}

/// kill vs Start：手工 Building 的提交窗口竞速。Kill 先行则 Start 得
/// ObjectClosed；Start 先行则 Running 后被 kill 收束。观察：两侧胜负。
fn race_kill_start(h: &RaceHammers, job: Handle) -> bool {
    let mut ok = true;
    let mut dist = [0usize; 2];
    for round in 0..4u64 {
        let created = match build_spin_building(job, RACE_CONTROL_RIGHTS) {
            Ok(created) => created,
            Err(error) => {
                debug!("race kill-vs-start: building failed: {:?}", error);
                return false;
            }
        };
        // builder 不可 duplicate（无 DUPLICATE 位）：原件经消息移交。
        let start_moves = [HandleMove {
            handle: created.builder,
            rights: Rights::MANAGE,
        }];
        let kill_moves = [HandleMove {
            handle: duplicate(created.control, Rights::MANAGE | Rights::TRANSIT)
                .unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let start_cmd = race_cmd(race::ACTION_START, 0);
        let kill_cmd = race_cmd(race::ACTION_KILL, 0);
        let start_sent = h.send_cmd(0, &start_cmd, &start_moves);
        let kill_sent = h.send_cmd(1, &kill_cmd, &kill_moves);
        let sent = start_sent && kill_sent;
        if !start_sent {
            let _ = unsafe { close(created.builder) };
        }
        if !kill_sent && kill_moves[0].handle.is_valid() {
            let _ = unsafe { close(kill_moves[0].handle) };
        }
        if !sent {
            let _ = process::kill(created.control);
        }
        h.fire(&[0, 1]);
        let (rep_start, _) = h.report(0).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let (rep_kill, _) = h.report(1).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let closed = SystemCallError::ObjectClosed as i64;
        let start_side_ok = rep_start.status == 0 || rep_start.status == closed;
        let kill_side_ok = rep_kill.status == 0;
        let allowed = [TerminationCause::Killed as u32];
        let terminal = observe_dead(created.control, &allowed);
        match rep_start.status {
            0 => dist[0] += 1,
            status if status == closed => dist[1] += 1,
            _ => {}
        }
        if !sent || !start_side_ok || !kill_side_ok || terminal.is_none() {
            debug!(
                "race kill-vs-start: round {} sent={} start={:#x} kill={:#x} terminal={}",
                round,
                sent,
                rep_start.status,
                rep_kill.status,
                terminal.is_some()
            );
            ok = false;
        }
        let _ = unsafe { close(created.control) };
    }
    debug!(
        "race kill-vs-start {} (start-first {}/{} kill-first)",
        if ok { "passed" } else { "FAILED" },
        dist[0],
        dist[1]
    );
    ok
}

/// kill vs park：高频 park 靶（park_waiting/pick gate/取消游标窗口）
/// 被单锤 kill，断言 Killed 收束不悬挂。
fn race_kill_park(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    for round in 0..2u64 {
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_PARK, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race kill-vs-park: target spawn failed: {:?}", error);
                return false;
            }
        };
        let (rep, _) = match h.shoot(
            0,
            &race_cmd(race::ACTION_KILL, 0),
            &[HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                    .unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            }],
        ) {
            Some(pair) => pair,
            None => {
                debug!("race kill-vs-park: hammer report missing");
                let _ = unsafe { close(target.control) };
                let _ = unsafe { close(gun) };
                ok = false;
                continue;
            }
        };
        fire_race(h, gun, &[]);
        let allowed = [TerminationCause::Killed as u32];
        let terminal = observe_dead(target.control, &allowed);
        if rep.status != 0 || terminal.is_none() {
            debug!(
                "race kill-vs-park: round {} rep={:#x} terminal={}",
                round,
                rep.status,
                terminal.is_some()
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
    }
    debug!("race kill-vs-park {}", if ok { "passed" } else { "FAILED" });
    ok
}

/// 公开内存事务 vs 异 hart ProcessKill：目标持续 Map/Protect/Unmap，锤与
/// 目标同刻起跑；延迟轮保证目标先进入 churn，使 kill 覆盖已发布 MemoryChange
/// 的 mandatory completion/termination 接管，而不只覆盖 syscall 前冻结。
fn race_memory_kill(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    for round in 0..4u64 {
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_MEMORY_CHURN, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race memory-vs-kill: target spawn failed: {:?}", error);
                return false;
            }
        };
        // 先让目标首线程完成两条工作线程的 spawn 并进入并发 churn。
        // SAFETY: 纯相对延迟，不携带用户指针。
        unsafe { sys_sleep(10).expect("memory race warmup sleep failed") };
        // warmup 后以短延迟覆盖大 backing 清零期间的跨 hart ack 窗口。
        let kill = race_cmd_delayed(race::ACTION_KILL, 0, round);
        let sent = h.send_cmd(
            0,
            &kill,
            &[HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                    .unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            }],
        );
        fire_race(h, gun, &[0]);
        let report = h.report(0).map(|(report, _)| report);
        let allowed = [TerminationCause::Killed as u32];
        let terminal = observe_dead(target.control, &allowed);
        if !sent || report.is_none_or(|report| report.status != 0) || terminal.is_none() {
            debug!(
                "race memory-vs-kill: round {} sent={} report={:?} terminal={}",
                round,
                sent,
                report.map(|report| report.status),
                terminal.is_some()
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
    }
    debug!(
        "race memory-vs-kill {}",
        if ok { "passed" } else { "FAILED" }
    );
    ok
}

/// guard fault 必须只冻结目标进程为 Fault/LoadAccess；其 guarded mapping 由
/// 自动退休 收束，init 与内核继续运行。
fn guard_fault_is_process_local(job: Handle, image: &[u8]) -> bool {
    let (target, gun) = match spawn_race_target(job, image, race::TARGET_GUARD_FAULT, 0) {
        Ok(pair) => pair,
        Err(error) => {
            debug!("memory guard fault: target spawn failed: {:?}", error);
            return false;
        }
    };
    let _ = /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */ unsafe { libkernel::sys_notification_signal(gun, 1) };
    let allowed = [TerminationCause::Faulted as u32];
    let terminal = observe_dead(target.control, &allowed);
    let ok = terminal.is_some();
    debug!(
        "memory guard fault {}",
        if ok { "passed" } else { "FAILED" }
    );
    let _ = unsafe { close(target.control) };
    let _ = unsafe { close(gun) };
    ok
}

/// ThreadCreate/user-memory 8B：目标进程内部建立真实多线程，完成同 AddressSpace
/// stale translation、Endpoint close/普通 Unmap 并发及结构化 stack/join 收束；
/// 末线程通过 ThreadExit 被动冻结进程 Completed 终态。
pub(crate) fn thread_memory_suite(job: Handle, image: &[u8]) -> bool {
    let (target, gun) = match spawn_race_target(job, image, race::TARGET_THREAD_SUITE, 0) {
        Ok(pair) => pair,
        Err(error) => {
            debug!("thread memory suite: target spawn failed: {:?}", error);
            return false;
        }
    };
    let _ = /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */ unsafe { libkernel::sys_notification_signal(gun, 1) };
    let allowed = [TerminationCause::Completed as u32];
    let terminal = observe_dead(target.control, &allowed);
    // SAFETY: 两个句柄均由本剧本独占；目标已完成自动退休，关闭剩余控制与发令权。
    let control_closed = unsafe { close(target.control) }.is_ok();
    let gun_closed = unsafe { close(gun) }.is_ok();
    let ok = terminal.is_some() && control_closed && gun_closed;
    debug!(
        "thread memory suite {}",
        if ok { "passed" } else { "FAILED" }
    );
    ok
}

const RACE_TUNNEL_RESPONSE_MASK: u64 = 0xa5a5_5a5a_f0f0_0f0f;
const RACE_TUNNEL_ROUNDS: usize = 16;

fn tunnel_exit_stress(job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    for round in 0..RACE_TUNNEL_ROUNDS {
        let (endpoint_owner, invitation_owner, bytes) =
            match libkernel::ipc::invitation::Invitation::create(
                crate::root_memory_pool(),
                3 * 4096,
            ) {
                Ok(created) => created,
                Err(error) => {
                    debug!("Tunnel exit stress create failed: {:?}", error);
                    return false;
                }
            };
        let endpoint = match librunnel::MappedEndpoint::map(
            endpoint_owner,
            bytes,
            libkernel::mm::Placement::Anywhere,
        ) {
            Ok(mapping) => mapping,
            Err(failure) => {
                debug!("Tunnel exit stress Map failed: {:?}", failure.error);
                return false;
            }
        };
        let invitation = invitation_owner.into_capability().into_raw();
        let shared = endpoint.memory();
        let request = 0x7475_6e6e_656c_0000 | (round as u64 + 1);
        shared.store_u64(0, request, Ordering::Release);
        let (target, gun) =
            match spawn_tunnel_exit_target(job, image, invitation, endpoint.geometry().bytes()) {
                Ok(target) => target,
                Err(error) => {
                    debug!("Tunnel exit stress target spawn failed: {:?}", error);
                    let _ = endpoint.close();
                    return false;
                }
            };
        let expected = request ^ RACE_TUNNEL_RESPONSE_MASK;
        let mut published = false;
        for _ in 0..100 {
            if shared.load_u64(0, Ordering::Acquire) == expected {
                published = true;
                break;
            }
            // SAFETY: 值参数；让目标进程完成 Attach 与共享页发布。
            let _ = unsafe { sys_sleep(1) };
        }
        for _ in 0..512 {
            let mut sample = [0u8; 32];
            shared.read(8, &mut sample);
            let _ = shared.load_u64(8, Ordering::Acquire);
            let _ = shared.load_u32(12, Ordering::Relaxed);
        }
        let _ = /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */ unsafe { libkernel::sys_notification_signal(gun, 1) };
        let terminal = observe_dead(target.control, &[TerminationCause::Completed as u32]);
        let peer_closed = endpoint
            .events()
            .wait(ObjectSignals::PEER_CLOSED, 0)
            .map(|result| result.observed.intersects(ObjectSignals::PEER_CLOSED))
            .unwrap_or(false);
        if !published || terminal.is_none() || !peer_closed {
            debug!(
                "Tunnel exit stress round {} published={} peer_closed={}",
                round, published, peer_closed
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
        let _ = unsafe { close(gun) };
        let _ = endpoint.close();
    }
    debug!(
        "Tunnel external shared-byte access checks completed: {} rounds",
        RACE_TUNNEL_ROUNDS
    );
    debug!(
        "Tunnel exit stress {}: {} rounds",
        if ok { "passed" } else { "FAILED" },
        RACE_TUNNEL_ROUNDS
    );
    ok
}

/// kill vs abandonment：锤 kill 与锤 close builder 同刻——终因冻结的
/// 先到者胜（Killed 或 Aborted），不出现混合。奇数轮 close 延迟
/// 1ms：kill 先冻结终因，观察 Killed 胜出侧（close 后到只协助收束）。
fn race_kill_abandon(h: &RaceHammers, job: Handle) -> bool {
    let mut ok = true;
    let mut dist = [0usize; 2];
    for round in 0..4u64 {
        let created = match build_spin_building(job, RACE_CONTROL_RIGHTS) {
            Ok(created) => created,
            Err(error) => {
                debug!("race kill-vs-abandon: building failed: {:?}", error);
                return false;
            }
        };
        let close_cmd = if round % 2 == 1 {
            race_cmd_delayed(race::ACTION_CLOSE, 0, 10)
        } else {
            race_cmd(race::ACTION_CLOSE, 0)
        };
        let close_moves = [HandleMove {
            handle: created.builder,
            rights: Rights::MANAGE,
        }];
        let kill_moves = [HandleMove {
            handle: duplicate(created.control, Rights::MANAGE | Rights::TRANSIT)
                .unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let close_sent = h.send_cmd(0, &close_cmd, &close_moves);
        let kill_sent = h.send_cmd(1, &race_cmd(race::ACTION_KILL, 0), &kill_moves);
        let sent = close_sent && kill_sent;
        if !close_sent {
            let _ = unsafe { close(created.builder) };
        }
        if !kill_sent && kill_moves[0].handle.is_valid() {
            let _ = unsafe { close(kill_moves[0].handle) };
        }
        if !sent {
            let _ = process::kill(created.control);
        }
        h.fire(&[0, 1]);
        let (rep_close, _) = h.report(0).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let (rep_kill, _) = h.report(1).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let allowed = [
            TerminationCause::Killed as u32,
            TerminationCause::Aborted as u32,
        ];
        let terminal = observe_dead(created.control, &allowed);
        match terminal {
            Some(cause) if cause == TerminationCause::Killed as u32 => dist[0] += 1,
            Some(_) => dist[1] += 1,
            _ => {}
        }
        if !sent || rep_close.status != 0 || rep_kill.status != 0 || terminal.is_none() {
            debug!(
                "race kill-vs-abandon: round {} sent={} close={:#x} kill={:#x} terminal={}",
                round,
                sent,
                rep_close.status,
                rep_kill.status,
                terminal.is_some()
            );
            ok = false;
        }
        let _ = unsafe { close(created.control) };
    }
    debug!(
        "race kill-vs-abandon {} (killed {}/{} aborted)",
        if ok { "passed" } else { "FAILED" },
        dist[0],
        dist[1]
    );
    ok
}

/// 双锤并发Create-即弃，枚举当前成员严格升序且只含已知身份。
/// 自动退休可能已移除新建PID；尚可派生者观察Aborted/DONE，已移除者按
/// Job在DONE后摘成员的合同成立，不要求保留历史墓碑。
fn race_create_enumerate(h: &RaceHammers, job: Handle) -> bool {
    let mut ok = true;
    let mut pids = alloc::vec::Vec::new();
    for round in 0..4u64 {
        let job_a = duplicate(job, Rights::CREATE | Rights::TRANSIT).unwrap_or(Handle::INVALID);
        let job_b = duplicate(job, Rights::CREATE | Rights::TRANSIT).unwrap_or(Handle::INVALID);
        let create = race_cmd(race::ACTION_CREATE_ABANDON, 0);
        let sent = h.send_cmd(
            0,
            &create,
            &[HandleMove {
                handle: job_a,
                rights: Rights::CREATE,
            }],
        ) && h.send_cmd(
            1,
            &create,
            &[HandleMove {
                handle: job_b,
                rights: Rights::CREATE,
            }],
        );
        h.fire(&[0, 1]);
        let (rep_a, _) = h.report(0).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let (rep_b, _) = h.report(1).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        if !sent || rep_a.status != 0 || rep_b.status != 0 {
            debug!(
                "race create-vs-enumerate: round {} sent={} rep={:#x}/{:#x}",
                round, sent, rep_a.status, rep_b.status
            );
            ok = false;
            continue;
        }
        pids.push(rep_a.aux0);
        pids.push(rep_b.aux0);
    }
    if pids.len() != 8 {
        debug!(
            "race create-vs-enumerate: only {} creates reported",
            pids.len()
        );
        ok = false;
    }
    let job_read = duplicate(job, Rights::READ | Rights::TRANSIT).unwrap_or(Handle::INVALID);
    let (rep, ids) = match h.shoot(
        0,
        &race_cmd(race::ACTION_ENUMERATE, 0),
        &[HandleMove {
            handle: job_read,
            rights: Rights::READ,
        }],
    ) {
        Some(pair) => pair,
        None => {
            debug!("race create-vs-enumerate: enumerate report missing");
            return false;
        }
    };
    if rep.status != 0 {
        debug!(
            "race create-vs-enumerate: enumerate failed {:#x}",
            rep.status
        );
        return false;
    }
    let sorted = ids.windows(2).all(|pair| pair[0] < pair[1]);
    let known = ids
        .iter()
        .all(|pid| pids.contains(pid) || h.pids.contains(pid));
    let hammers_present = ids.contains(&h.pids[0]) && ids.contains(&h.pids[1]);
    let expected_len = pids.len() + 2;
    if !sorted || !known || !hammers_present || ids.len() > expected_len {
        debug!(
            "race create-vs-enumerate: sorted={} present={} hammers={} len={} expected={}",
            sorted,
            known,
            hammers_present,
            ids.len(),
            expected_len
        );
        ok = false;
    }
    let aborted = [TerminationCause::Aborted as u32];
    for pid in &pids {
        match process::derive_job(
            job,
            JobMemberKind::MemberProcesses,
            *pid,
            DERIVED_CONTROL_RIGHTS,
        ) {
            Ok(control) => {
                if observe_dead(control, &aborted).is_none() {
                    ok = false;
                }
                let _ = unsafe { close(control) };
            }
            Err(SystemCallError::ObjectNotFound) => {} // DONE之后成员已经移除。
            Err(error) => {
                debug!(
                    "race create-vs-enumerate: derive pid {} failed: {:?}",
                    pid, error
                );
                ok = false;
            }
        }
    }
    debug!(
        "race create-vs-enumerate {} ({} members enumerated, {} orphans reaped)",
        if ok { "passed" } else { "FAILED" },
        ids.len(),
        pids.len()
    );
    ok
}

/// seal vs 并发 Create：锤 seal 与锤 Create 同刻。Seal 线性化一次后
/// 创建口永久关闭——首轮 create 与 seal 竞争（任意结果），后续轮必
/// ObjectClosed；残留 Building 由 job_kill 收束，child 完成 Dead。
fn race_seal_create(root: &mut RootSupervisor, h: &RaceHammers, job: Handle) -> bool {
    let child = match root.create_job(job, crate::root_memory_pool(), JOB_FULL_RIGHTS) {
        Ok(handle) => handle,
        Err(error) => {
            debug!("race seal-vs-create: child job failed: {:?}", error);
            return false;
        }
    };
    let closed = SystemCallError::ObjectClosed as i64;
    let seal_control = match root.duplicate(child, Rights::MANAGE | Rights::TRANSIT) {
        Ok(handle) => handle,
        Err(error) => {
            debug!("race seal-vs-create: seal control failed: {:?}", error);
            return false;
        }
    };
    let seal_moves = [HandleMove {
        handle: seal_control,
        rights: Rights::MANAGE,
    }];
    let mut ok = h.send_cmd(0, &race_cmd(race::ACTION_SEAL, 0), &seal_moves);
    if ok {
        root.transferred(seal_control);
    } else {
        return false;
    }
    let mut first_gated: Option<bool> = None;
    for round in 0..4u64 {
        let job_b = match root.duplicate(child, Rights::CREATE | Rights::TRANSIT) {
            Ok(handle) => handle,
            Err(error) => {
                debug!("race seal-vs-create: create control failed: {:?}", error);
                return false;
            }
        };
        let create = race_cmd(race::ACTION_CREATE, 0);
        let sent = h.send_cmd(
            1,
            &create,
            &[HandleMove {
                handle: job_b,
                rights: Rights::CREATE,
            }],
        );
        if sent {
            root.transferred(job_b);
        } else {
            return false;
        }
        if round == 0 {
            h.fire(&[0, 1]);
        } else {
            h.fire(&[1]);
        }
        let (rep_seal, _) = if round == 0 {
            h.report(0).unwrap_or((
                Report {
                    status: -1,
                    aux0: 0,
                    aux1: 0,
                },
                alloc::vec::Vec::new(),
            ))
        } else {
            (
                Report {
                    status: 0,
                    aux0: 0,
                    aux1: 0,
                },
                alloc::vec::Vec::new(),
            )
        };
        let (rep_create, _) = h.report(1).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let mut legal = rep_create.status == 0 || rep_create.status == closed;
        if round == 0 {
            first_gated = Some(rep_create.status == closed);
        } else if rep_create.status != closed {
            legal = false;
            debug!(
                "race seal-vs-create: round {} create {:#x} after seal",
                round, rep_create.status
            );
        }
        if !sent || !legal || (round == 0 && rep_seal.status != 0) {
            debug!(
                "race seal-vs-create: round {} sent={} seal={:#x} create={:#x}",
                round, sent, rep_seal.status, rep_create.status
            );
            ok = false;
        }
    }
    let mut close_child = false;
    match root.collect_job(child) {
        Ok(()) => match process::query_job(child) {
            Ok(snapshot) if snapshot.state == JobState::Dead as u32 => {
                close_child = true;
                debug!(
                    "race seal-vs-create {} (first create gated: {:?})",
                    if ok { "passed" } else { "FAILED" },
                    first_gated
                );
            }
            snapshot => {
                debug!(
                    "race seal-vs-create: child state {:?}",
                    snapshot.map(|s| s.state)
                );
                ok = false;
            }
        },
        Err(error) => {
            debug!(
                "race seal-vs-create: job_kill failed: stage={:?}, cause={:?}, retained_collector={}",
                "root-owned", error, true
            );
            ok = false;
        }
    }
    if close_child && root.close_control(child).is_err() {
        return false;
    }
    ok
}

/// 双终态观察：DONE之后两锤并发只读Query，两者均须成功且状态为Dead。
fn race_drain_drain(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    for round in 0..2u64 {
        let target = match spawn(SpawnRequest {
            job,
            memory_pool: crate::root_memory_pool(),
            parent: crate::init_process_info(),
            image,
            payload: &[],
            grants: &[],
            control_rights: RACE_CONTROL_RIGHTS,
        }) {
            Ok(spawned) => spawned,
            Err(error) => {
                debug!("race drain-vs-drain: target spawn failed: {:?}", error);
                return false;
            }
        };
        if let Err(error) = process::kill(target.control) {
            debug!("race drain-vs-drain: kill failed: {:?}", error);
            let _ = unsafe { close(target.control) };
            return false;
        }
        // 竞争点在双终态观察 本身：先等 终态 再同刻发双终态观察。
        if !await_done(target.control) {
            let _ = unsafe { close(target.control) };
            return false;
        }
        let moves = [
            HandleMove {
                handle: duplicate(target.control, Rights::READ | Rights::TRANSIT)
                    .unwrap_or(Handle::INVALID),
                rights: Rights::READ,
            },
            HandleMove {
                handle: duplicate(target.control, Rights::READ | Rights::TRANSIT)
                    .unwrap_or(Handle::INVALID),
                rights: Rights::READ,
            },
        ];
        let drain = race_cmd(race::ACTION_OBSERVE_TERMINAL, 0);
        let sent = h.send_cmd(0, &drain, &[moves[0]]) && h.send_cmd(1, &drain, &[moves[1]]);
        h.fire(&[0, 1]);
        let (rep_a, _) = h.report(0).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let (rep_b, _) = h.report(1).unwrap_or((
            Report {
                status: -1,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ));
        let legal = rep_a.status == 0 && rep_b.status == 0;
        let allowed = [TerminationCause::Killed as u32];
        let terminal = observe_dead(target.control, &allowed);
        if !sent || !legal || terminal.is_none() {
            debug!(
                "race drain-vs-drain: round {} sent={} rep={:#x}/{:#x} terminal={}",
                round,
                sent,
                rep_a.status,
                rep_b.status,
                terminal.is_some()
            );
            ok = false;
        }
        let _ = unsafe { close(target.control) };
    }
    debug!(
        "race drain-vs-drain {}",
        if ok { "passed" } else { "FAILED" }
    );
    ok
}

/// 最后Control随KILL移交锤并释放；仍在Job则派生观察DONE，
/// 已从Job移除则已完成私有回收，不依赖保留Control或历史墓碑。
fn race_last_control(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    for round in 0..2u64 {
        let target = match spawn(SpawnRequest {
            job,
            memory_pool: crate::root_memory_pool(),
            parent: crate::init_process_info(),
            image,
            payload: &[],
            grants: &[],
            control_rights: RACE_CONTROL_RIGHTS,
        }) {
            Ok(spawned) => spawned,
            Err(error) => {
                debug!("race last-control: target spawn failed: {:?}", error);
                return false;
            }
        };
        let pid = target.pid;
        let moves = [HandleMove {
            handle: target.control,
            rights: Rights::MANAGE,
        }];
        let (rep, _) = match h.shoot(0, &race_cmd(race::ACTION_KILL, 0), &moves) {
            Some(pair) => pair,
            None => {
                debug!("race last-control: hammer report missing");
                ok = false;
                continue;
            }
        };
        let allowed = [TerminationCause::Killed as u32];
        let terminal = observe_member_completion(job, pid, &allowed);
        if rep.status != 0 || !terminal {
            debug!(
                "race last-control: round {} rep={:#x} terminal={}",
                round, rep.status, terminal
            );
            ok = false;
        }
    }
    debug!("race last-control {}", if ok { "passed" } else { "FAILED" });
    ok
}

/// 竞态矩阵入口：双锤编队 → 16 场景 → 退场收束 → 汇总。失败场景逐个
/// 点名，汇总行是全矩阵的 grep 锚点。
pub(crate) fn race_matrix(
    root: &mut RootSupervisor,
    acceptance: Handle,
    target_image: &[u8],
    hammer_image: &[u8],
) -> Result<(), &'static str> {
    let h = match RaceHammers::spawn_pair(acceptance, hammer_image) {
        Ok(set) => set,
        Err(error) => {
            debug!("race matrix acceptance failed: hammer spawn {:?}", error);
            return Err("race matrix hammer spawn failed");
        }
    };
    macro_rules! scenario {
        ($name:literal, $run:expr) => {{
            debug!(
                "acceptance progress: phase=race-matrix scenario={} step=start",
                $name
            );
            let result = $run;
            debug!(
                "acceptance progress: phase=race-matrix scenario={} step=complete result={}",
                $name,
                if result { "passed" } else { "failed" }
            );
            result
        }};
    }
    let scenarios: [(&str, bool); 16] = [
        (
            "kill-vs-kill",
            scenario!("kill-vs-kill", race_kill_kill(&h, acceptance, hammer_image)),
        ),
        (
            "kill-vs-exit",
            scenario!("kill-vs-exit", race_kill_exit(&h, acceptance, hammer_image)),
        ),
        (
            "spawn-vs-kill",
            scenario!(
                "spawn-vs-kill",
                race_thread_spawn_kill(&h, acceptance, hammer_image)
            ),
        ),
        (
            "last-thread-exit-vs-kill",
            scenario!(
                "last-thread-exit-vs-kill",
                race_last_thread_exit_kill(&h, acceptance, hammer_image)
            ),
        ),
        (
            "kill-vs-fault",
            scenario!(
                "kill-vs-fault",
                race_kill_fault(&h, acceptance, hammer_image)
            ),
        ),
        (
            "kill-vs-start",
            scenario!("kill-vs-start", race_kill_start(&h, acceptance)),
        ),
        (
            "kill-vs-park",
            scenario!("kill-vs-park", race_kill_park(&h, acceptance, hammer_image)),
        ),
        (
            "memory-vs-kill",
            scenario!(
                "memory-vs-kill",
                race_memory_kill(&h, acceptance, hammer_image)
            ),
        ),
        (
            "memory-guard-fault",
            scenario!(
                "memory-guard-fault",
                guard_fault_is_process_local(acceptance, hammer_image)
            ),
        ),
        (
            "thread-memory-suite",
            scenario!(
                "thread-memory-suite",
                thread_memory_suite(acceptance, hammer_image)
            ),
        ),
        (
            "tunnel-exit-stress",
            scenario!(
                "tunnel-exit-stress",
                tunnel_exit_stress(acceptance, hammer_image)
            ),
        ),
        (
            "kill-vs-abandon",
            scenario!("kill-vs-abandon", race_kill_abandon(&h, acceptance)),
        ),
        (
            "create-vs-enumerate",
            scenario!("create-vs-enumerate", race_create_enumerate(&h, acceptance)),
        ),
        (
            "seal-vs-create",
            scenario!("seal-vs-create", race_seal_create(root, &h, acceptance)),
        ),
        (
            "drain-vs-drain",
            scenario!(
                "drain-vs-drain",
                race_drain_drain(&h, acceptance, target_image)
            ),
        ),
        (
            "last-control",
            scenario!(
                "last-control",
                race_last_control(&h, acceptance, target_image)
            ),
        ),
    ];
    h.shutdown();
    let hammer_targets = alloc::vec::Vec::from([
        Supervised {
            pid: h.pids[0],
            control: h.controls[0],
        },
        Supervised {
            pid: h.pids[1],
            control: h.controls[1],
        },
    ]);
    let hammer_supervision = root.collect_targets(hammer_targets);
    let supervision_ok = hammer_supervision.is_ok();
    if !supervision_ok {
        debug!("race matrix acceptance failed: hammer supervision degraded");
    }
    let passed = scenarios.iter().filter(|(_, passed)| *passed).count();
    for (name, passed) in &scenarios {
        if !passed {
            debug!("race matrix acceptance failed: scenario {}", name);
        }
    }
    let accepted = supervision_ok && passed == scenarios.len();
    debug!(
        "race matrix acceptance {}: {}/{} scenarios passed",
        if accepted { "passed" } else { "failed" },
        passed,
        scenarios.len()
    );
    if accepted {
        Ok(())
    } else {
        Err("race matrix acceptance failed")
    }
}
