// —— 生命周期多核竞态矩阵（step 9）——
//
// 双锤（srv_hammer HAMMER 模式）常驻 acceptance 域：指令与 handle 经
// Mailbox 投递，发令枪（notification READABLE 电平）连发后双锤/靶在
// 各自 hart 上同刻开打，锤真跨核竞态窗口。剧本与断言全在 init：每轮
// 以「终因组合合法 + Dead 收束 + 无泄漏」为强断言，终因胜负分布只作
// 观察报告（证明锤进了窗口，不作通过条件）。

use crate::{JOB_FULL_RIGHTS, SUPERVISOR_RIGHTS, Supervised, supervise_services};
use libprocess::{
    DERIVED_CONTROL_RIGHTS, SpawnRequest, Spawned, job_kill, race::{self, Cmd, Report},
    spawn,
};
use rinlib::ipc::message::{create, send, wait_message};
use rinlib::ipc::notification;
use rinlib::ipc::object::{close, duplicate};
use rinlib::ipc::wait::wait_many;
use rinlib::preclude::*;
use rinlib::process;
use rinlib::shared::proc::ProcessAttachDescriptor;
use rinlib::shared::call::SystemCallError;
use rinlib::shared::message::HandleMove;
use rinlib::shared::object::{Handle, ObjectSignals, Rights};
use rinlib::shared::proc::{
    HandleGrant, JobMemberKind, JobState, ProcessCreateResult, ProcessExitReason,
    ProcessMapFlags, ProcessState, PROCESS_PAGE_SIZE, PROCESS_USER_TOP,
};
use rinlib::shared::wait::{WaitItem, WAIT_TIMEOUT_INFINITE};

/// 竞态锤编队：两执行器 + 每锤独立指令箱/回执箱/发令枪（回执按锤
/// 分箱，天然归属，无需锤标识）。
struct RaceHammers {
    cmd: [Handle; 2],
    guns: [Handle; 2],
    report: [Handle; 2],
    controls: [Handle; 2],
    pids: [u64; 2],
}

impl RaceHammers {
    fn spawn_pair(job: Handle, image: &[u8]) -> Result<Self, SystemCallError> {
        let mut set = Self {
            cmd: [Handle::INVALID; 2],
            guns: [Handle::INVALID; 2],
            report: [Handle::INVALID; 2],
            controls: [Handle::INVALID; 2],
            pids: [0; 2],
        };
        for i in 0..2 {
            let cmd_pair = create(
                Rights::READ | Rights::WAIT | Rights::GRANT,
                // 指令携带 HandleMove（transit 暂存）需要 TRANSIT 位。
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            )?;
            let report_pair = create(
                Rights::READ | Rights::WAIT,
                Rights::WRITE | Rights::WAIT | Rights::GRANT | Rights::DUPLICATE,
            )?;
            let gun_pair = notification::create(
                Rights::READ | Rights::WAIT | Rights::GRANT,
                Rights::SIGNAL,
            )?;
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
        send(self.cmd[hammer], race::MSG_CMD, &race::encode_cmd(cmd), moves).is_ok()
    }

    fn fire(&self, hammers: &[usize]) {
        for &i in hammers {
            let _ = notification::signal(self.guns[i], 1);
        }
    }

    fn report(&self, hammer: usize) -> Option<(Report, alloc::vec::Vec<u64>)> {
        let message = wait_message(self.report[hammer]).ok()?;
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
            let exit = Cmd { action: race::ACTION_EXIT, code: 0, entry: 0, sp: 0, aux: 0 };
            let _ = self.send_cmd(i, &exit, &[]);
            self.fire(&[i]);
            let _ = self.report(i);
            let _ = close(self.cmd[i]);
            let _ = close(self.report[i]);
            let _ = close(self.guns[i]);
        }
    }
}

fn race_cmd(action: u64, code: u64) -> Cmd {
    Cmd { action, code, entry: 0, sp: 0, aux: 0 }
}

/// 延迟变体指令：锤醒后先 sleep `delay_ms` 再执行，把窗口让给对侧
/// 先行（时序变体用，见 race::Cmd::aux）。
fn race_cmd_delayed(action: u64, code: u64, delay_ms: u64) -> Cmd {
    Cmd { action, code, entry: 0, sp: 0, aux: delay_ms }
}

/// 竞态靶（srv_hammer TARGET 模式）：等枪后按 subrole 自灭或高频 park。
/// 返回 (spawned, 靶枪 signaler)。
fn spawn_race_target(
    job: Handle,
    image: &[u8],
    subrole: u64,
    code: u64,
) -> Result<(Spawned, Handle), SystemCallError> {
    let gun_pair = notification::create(
        Rights::READ | Rights::WAIT | Rights::GRANT,
        Rights::SIGNAL,
    )?;
    let payload = race::encode_payload(&[race::MODE_TARGET, subrole, code]);
    let grants = [HandleGrant {
        handle: gun_pair.owner,
        rights: Rights::READ | Rights::WAIT,
    }];
    let spawned = spawn(SpawnRequest {
        job,
        image,
        payload: &payload,
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    })
    .map_err(|_| SystemCallError::Unknown)?;
    Ok((spawned, gun_pair.peer))
}

/// 入口页指令：`j .`（`0x0000006f`，JAL x0,0 自我跳转）。不用 wfi——
/// wfi 是特权指令，U-mode 执行触发 illegal instruction 异常
/// （riscv-isa machine.adoc：executing WFI in U-mode causes an
/// illegal-instruction exception），靶一旦上核首条指令即 Fault，会与
/// kill 争抢终因冻结，污染「Running 后被 kill 收束」的场景语义。
pub(crate) const SPIN_FOREVER: [u8; 4] = [0x6f, 0x00, 0x00, 0x00];

/// 手工 Building：入口页（自旋）+ 栈顶页 + 附入首线程，供
/// kill-vs-Start/abandonment 的提交前窗口竞速与 seal gate 验证。
/// attach 由组装者完成（线程是组装资源）；锤只持 builder 在竞速时刻
/// 拉 Start——Building→Running 线性化仍是唯一竞速点。失败路径自清理
/// 两个句柄，调用者只需处理自己持有的资源。
pub(crate) fn build_spin_building(job: Handle) -> Result<ProcessCreateResult, SystemCallError> {
    let created = process::create(job, SUPERVISOR_RIGHTS)?;
    let built = (|| {
        process::map(
            created.builder,
            0x1000,
            PROCESS_PAGE_SIZE,
            ProcessMapFlags::READ | ProcessMapFlags::EXECUTE,
        )?;
        process::write(created.builder, 0x1000, &SPIN_FOREVER)?;
        process::map(
            created.builder,
            PROCESS_USER_TOP - PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            ProcessMapFlags::READ | ProcessMapFlags::WRITE,
        )?;
        process::attach(
            created.builder,
            &ProcessAttachDescriptor {
                entry: 0x1000,
                stack_pointer: PROCESS_USER_TOP as u64,
                arg1: 0,
                arg2: 0,
            },
        )
    })();
    match built {
        Ok(_) => Ok(created),
        Err(error) => {
            let _ = close(created.builder);
            let _ = close(created.control);
            Err(error)
        }
    }
}

/// 收束到 Dead 后断言终因在允许集合内；code 通配用 None。返回终因
/// code（通配时回传实际值），违约返回 None。先等 REAPABLE|CLOSED 电平
/// （ProcessDrain 仅在 REAPABLE/Dead 上适用，kill 后线程退出路径是
/// 异步的，不能立即 drain）。
fn drain_expect_dead(control: Handle, allowed: &[(u32, Option<i64>)]) -> Option<i64> {
    if !await_reapable(control) {
        return None;
    }
    if let Err(error) = process::drain_to_completion(control) {
        debug!("race: drain failed: {:?}", error);
        return None;
    }
    let Ok(snapshot) = process::query(control) else {
        return None;
    };
    if snapshot.state != ProcessState::Dead as u32 {
        debug!("race: terminal state {} is not Dead", snapshot.state);
        return None;
    }
    for (reason, code) in allowed {
        if snapshot.reason == *reason {
            match code {
                None => return Some(snapshot.code),
                Some(expected) if snapshot.code == *expected => return Some(snapshot.code),
                _ => {}
            }
        }
    }
    debug!(
        "race: terminal reason {} code {:#x} not in allowed set",
        snapshot.reason, snapshot.code
    );
    None
}

/// 等 REAPABLE|CLOSED 电平。
fn await_reapable(control: Handle) -> bool {
    let items = [WaitItem::new(control, ObjectSignals::REAPABLE | ObjectSignals::CLOSED, 0)];
    match wait_many(&items, WAIT_TIMEOUT_INFINITE) {
        Ok(_) => true,
        Err(error) => {
            debug!("race: reapable wait failed: {:?}", error);
            false
        }
    }
}

/// 派生兜底收束（REAPABLE 无主靶）：枚举+派生+drain，断言同
/// [`drain_expect_dead`]。
fn derive_and_reap(job: Handle, pid: u64, allowed: &[(u32, Option<i64>)]) -> Option<i64> {
    let control = match process::derive_job(
        job,
        JobMemberKind::MemberProcesses,
        pid,
        DERIVED_CONTROL_RIGHTS,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            debug!("race: derive pid {} failed: {:?}", pid, error);
            return None;
        }
    };
    let code = drain_expect_dead(control, allowed);
    let _ = close(control);
    code
}

/// 靶枪与锤枪连发（fire 顺序无关紧要：电平语义不丢令，窗口差只是一个
/// ecall）。
fn fire_race(h: &RaceHammers, target_gun: Handle, hammers: &[usize]) {
    let _ = notification::signal(target_gun, 1);
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
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            },
            HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
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
        let kills_ok = rep_a.status == 0 && rep_b.status == 0;
        let allowed = [
            (ProcessExitReason::Killed as u32, Some(codes[0])),
            (ProcessExitReason::Killed as u32, Some(codes[1])),
        ];
        let terminal = drain_expect_dead(target.control, &allowed);
        match terminal {
            Some(code) if code == codes[0] => wins[0] += 1,
            Some(_) => wins[1] += 1,
            None => ok = false,
        }
        if !sent || !kills_ok {
            debug!(
                "race kill-vs-kill: round {} sent={} rep={:#x}/{:#x}",
                round, sent, rep_a.status, rep_b.status
            );
            ok = false;
        }
        let _ = close(target.control);
        let _ = close(gun);
    }
    debug!(
        "race kill-vs-kill {} (code wins {}/{})",
        if ok { "passed" } else { "FAILED" },
        wins[0],
        wins[1]
    );
    ok
}

/// kill vs Exit：靶自杀与锤 kill 同刻起跑，终因 Exited/Killed 二者
/// 恰一，code 与胜者匹配。奇数轮锤延迟 1ms 再 kill：靶的 Exit 先
/// 线性化，观察 Exited 胜出侧（kill 后到幂等）。
fn race_kill_exit(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    let mut dist = [0usize; 2];
    for round in 0..4u64 {
        let exit_code = 0x300 + round as i64;
        let kill_code = 0x400 + round as i64;
        let (target, gun) =
            match spawn_race_target(job, image, race::TARGET_SUICIDE, exit_code as u64) {
                Ok(pair) => pair,
                Err(error) => {
                    debug!("race kill-vs-exit: target spawn failed: {:?}", error);
                    return false;
                }
            };
        let moves = [HandleMove {
            handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let kill = if round % 2 == 1 {
            race_cmd_delayed(race::ACTION_KILL, kill_code as u64, 10)
        } else {
            race_cmd(race::ACTION_KILL, kill_code as u64)
        };
        let sent = h.send_cmd(0, &kill, &moves);
        fire_race(h, gun, &[0]);
        let (rep, _) = h.report(0).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let allowed = [
            (ProcessExitReason::Exited as u32, Some(exit_code)),
            (ProcessExitReason::Killed as u32, Some(kill_code)),
        ];
        let terminal = drain_expect_dead(target.control, &allowed);
        match terminal {
            Some(code) if code == exit_code => dist[0] += 1,
            Some(_) => dist[1] += 1,
            _ => ok = false,
        }
        if !sent || rep.status != 0 {
            debug!(
                "race kill-vs-exit: round {} sent={} rep={:#x}",
                round, sent, rep.status
            );
            ok = false;
        }
        let _ = close(target.control);
        let _ = close(gun);
    }
    debug!(
        "race kill-vs-exit {} (exited {}/{} killed)",
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
        let kill_code = 0x500 + round as i64;
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_FAULT, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race kill-vs-fault: target spawn failed: {:?}", error);
                return false;
            }
        };
        let moves = [HandleMove {
            handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let kill = if round % 2 == 1 {
            race_cmd_delayed(race::ACTION_KILL, kill_code as u64, 10)
        } else {
            race_cmd(race::ACTION_KILL, kill_code as u64)
        };
        let sent = h.send_cmd(0, &kill, &moves);
        fire_race(h, gun, &[0]);
        let (rep, _) = h.report(0).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let allowed = [
            (ProcessExitReason::Fault as u32, Some(4)),
            (ProcessExitReason::Killed as u32, Some(kill_code)),
        ];
        let terminal = drain_expect_dead(target.control, &allowed);
        match terminal {
            Some(code) if code == 4 => dist[0] += 1,
            Some(_) => dist[1] += 1,
            _ => ok = false,
        }
        if !sent || rep.status != 0 {
            debug!(
                "race kill-vs-fault: round {} sent={} rep={:#x}",
                round, sent, rep.status
            );
            ok = false;
        }
        let _ = close(target.control);
        let _ = close(gun);
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
        let created = match build_spin_building(job) {
            Ok(created) => created,
            Err(error) => {
                debug!("race kill-vs-start: building failed: {:?}", error);
                return false;
            }
        };
        let kill_code = 0x600 + round as i64;
        // builder 不可 duplicate（无 DUPLICATE 位）：原件经消息移交。
        let start_moves = [HandleMove {
            handle: created.builder,
            rights: Rights::MAP | Rights::WRITE | Rights::MANAGE,
        }];
        let kill_moves = [HandleMove {
            handle: duplicate(created.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let start_cmd = race_cmd(race::ACTION_START, 0);
        let kill_cmd = race_cmd(race::ACTION_KILL, kill_code as u64);
        let sent = h.send_cmd(0, &start_cmd, &start_moves) && h.send_cmd(1, &kill_cmd, &kill_moves);
        h.fire(&[0, 1]);
        let (rep_start, _) = h.report(0).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let (rep_kill, _) = h.report(1).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let closed = SystemCallError::ObjectClosed as i64;
        let start_side_ok = rep_start.status == 0 || rep_start.status == closed;
        let kill_side_ok = rep_kill.status == 0;
        let allowed = [(ProcessExitReason::Killed as u32, Some(kill_code))];
        let terminal = drain_expect_dead(created.control, &allowed);
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
        let _ = close(created.control);
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
        let kill_code = 0x700 + round as i64;
        let (target, gun) = match spawn_race_target(job, image, race::TARGET_PARK, 0) {
            Ok(pair) => pair,
            Err(error) => {
                debug!("race kill-vs-park: target spawn failed: {:?}", error);
                return false;
            }
        };
        let (rep, _) = match h.shoot(
            0,
            &race_cmd(race::ACTION_KILL, kill_code as u64),
            &[HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT)
                    .unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            }],
        ) {
            Some(pair) => pair,
            None => {
                debug!("race kill-vs-park: hammer report missing");
                let _ = close(target.control);
                let _ = close(gun);
                ok = false;
                continue;
            }
        };
        fire_race(h, gun, &[]);
        let allowed = [(ProcessExitReason::Killed as u32, Some(kill_code))];
        let terminal = drain_expect_dead(target.control, &allowed);
        if rep.status != 0 || terminal.is_none() {
            debug!(
                "race kill-vs-park: round {} rep={:#x} terminal={}",
                round,
                rep.status,
                terminal.is_some()
            );
            ok = false;
        }
        let _ = close(target.control);
        let _ = close(gun);
    }
    debug!("race kill-vs-park {}", if ok { "passed" } else { "FAILED" });
    ok
}

/// kill vs abandonment：锤 kill 与锤 close builder 同刻——终因冻结的
/// 先到者胜（Killed 或 Abandoned），不出现混合。奇数轮 close 延迟
/// 1ms：kill 先冻结终因，观察 Killed 胜出侧（close 后到只协助收束）。
fn race_kill_abandon(h: &RaceHammers, job: Handle) -> bool {
    let mut ok = true;
    let mut dist = [0usize; 2];
    for round in 0..4u64 {
        let created = match build_spin_building(job) {
            Ok(created) => created,
            Err(error) => {
                debug!("race kill-vs-abandon: building failed: {:?}", error);
                return false;
            }
        };
        let kill_code = 0x800 + round as i64;
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
            handle: duplicate(created.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
            rights: Rights::MANAGE,
        }];
        let sent = h.send_cmd(0, &close_cmd, &close_moves)
            && h.send_cmd(1, &race_cmd(race::ACTION_KILL, kill_code as u64), &kill_moves);
        h.fire(&[0, 1]);
        let (rep_close, _) = h.report(0).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let (rep_kill, _) = h.report(1).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let allowed = [
            (ProcessExitReason::Killed as u32, Some(kill_code)),
            (ProcessExitReason::Abandoned as u32, Some(0)),
        ];
        let terminal = drain_expect_dead(created.control, &allowed);
        match terminal {
            Some(code) if code == kill_code => dist[0] += 1,
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
        let _ = close(created.control);
    }
    debug!(
        "race kill-vs-abandon {} (killed {}/{} abandoned)",
        if ok { "passed" } else { "FAILED" },
        dist[0],
        dist[1]
    );
    ok
}

/// 并发 Create + 枚举：双锤同刻 Create-即弃（abandonment 自灭到
/// REAPABLE），第三发枚举全量——断言无漏项（多核 ID 乱序窗口）、严格
/// 升序、恰好 = 新建 8 + 双锤；随后派生兜底收束全部无主靶。
fn race_create_enumerate(h: &RaceHammers, job: Handle) -> bool {
    let mut ok = true;
    let mut pids = alloc::vec::Vec::new();
    for round in 0..4u64 {
        let job_a = duplicate(job, Rights::CREATE | Rights::TRANSIT).unwrap_or(Handle::INVALID);
        let job_b = duplicate(job, Rights::CREATE | Rights::TRANSIT).unwrap_or(Handle::INVALID);
        let create = race_cmd(race::ACTION_CREATE_ABANDON, 0);
        let sent = h.send_cmd(0, &create, &[HandleMove { handle: job_a, rights: Rights::CREATE }])
            && h.send_cmd(1, &create, &[HandleMove { handle: job_b, rights: Rights::CREATE }]);
        h.fire(&[0, 1]);
        let (rep_a, _) = h.report(0).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let (rep_b, _) = h.report(1).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
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
        debug!("race create-vs-enumerate: only {} creates reported", pids.len());
        ok = false;
    }
    let job_read = duplicate(job, Rights::READ | Rights::TRANSIT).unwrap_or(Handle::INVALID);
    let (rep, ids) = match h.shoot(
        0,
        &race_cmd(race::ACTION_ENUMERATE, 0),
        &[HandleMove { handle: job_read, rights: Rights::READ }],
    ) {
        Some(pair) => pair,
        None => {
            debug!("race create-vs-enumerate: enumerate report missing");
            return false;
        }
    };
    if rep.status != 0 {
        debug!("race create-vs-enumerate: enumerate failed {:#x}", rep.status);
        return false;
    }
    let sorted = ids.windows(2).all(|pair| pair[0] < pair[1]);
    let all_present = pids.iter().all(|pid| ids.contains(pid));
    let hammers_present = ids.contains(&h.pids[0]) && ids.contains(&h.pids[1]);
    let expected_len = pids.len() + 2;
    if !sorted || !all_present || !hammers_present || ids.len() != expected_len {
        debug!(
            "race create-vs-enumerate: sorted={} present={} hammers={} len={} expected={}",
            sorted,
            all_present,
            hammers_present,
            ids.len(),
            expected_len
        );
        ok = false;
    }
    let abandoned = [(ProcessExitReason::Abandoned as u32, Some(0))];
    for pid in &pids {
        if derive_and_reap(job, *pid, &abandoned).is_none() {
            ok = false;
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
fn race_seal_create(h: &RaceHammers, job: Handle) -> bool {
    let child = match process::create_job(job, JOB_FULL_RIGHTS) {
        Ok(handle) => handle,
        Err(error) => {
            debug!("race seal-vs-create: child job failed: {:?}", error);
            return false;
        }
    };
    let closed = SystemCallError::ObjectClosed as i64;
    let seal_moves = [HandleMove {
        handle: duplicate(child, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
        rights: Rights::MANAGE,
    }];
    let mut ok = h.send_cmd(0, &race_cmd(race::ACTION_SEAL, 0), &seal_moves);
    let mut first_gated: Option<bool> = None;
    for round in 0..4u64 {
        let job_b = duplicate(child, Rights::CREATE | Rights::TRANSIT).unwrap_or(Handle::INVALID);
        let create = race_cmd(race::ACTION_CREATE, 0);
        let sent = h.send_cmd(1, &create, &[HandleMove { handle: job_b, rights: Rights::CREATE }]);
        if round == 0 {
            h.fire(&[0, 1]);
        } else {
            h.fire(&[1]);
        }
        let (rep_seal, _) = if round == 0 {
            h.report(0).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()))
        } else {
            (Report { status: 0, aux0: 0, aux1: 0 }, alloc::vec::Vec::new())
        };
        let (rep_create, _) = h.report(1).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let mut legal = rep_create.status == 0 || rep_create.status == closed;
        if round == 0 {
            first_gated = Some(rep_create.status == closed);
        } else if rep_create.status != closed {
            legal = false;
            debug!(
                "race seal-vs-create: round {} create {:#x} after seal",
                round,
                rep_create.status
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
    match job_kill(child, 0xB00) {
        Ok(()) => match process::query_job(child) {
            Ok(snapshot) if snapshot.state == JobState::Dead as u32 => {
                debug!(
                    "race seal-vs-create {} (first create gated: {:?})",
                    if ok { "passed" } else { "FAILED" },
                    first_gated
                );
            }
            snapshot => {
                debug!("race seal-vs-create: child state {:?}", snapshot.map(|s| s.state));
                ok = false;
            }
        },
        Err(error) => {
            debug!("race seal-vs-create: job_kill failed: {:?}", error);
            ok = false;
        }
    }
    let _ = close(child);
    ok
}

/// 双 Drain 并发：REAPABLE 靶上双锤同刻 drain——单批互斥以 ObjectBusy
/// 仲裁，两批都成功或一让一进，最终 Dead；init 收束至 Complete。
fn race_drain_drain(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    let busy = SystemCallError::ObjectBusy as i64;
    for round in 0..2u64 {
        let kill_code = 0x900 + round as i64;
        let target = match spawn(SpawnRequest {
            job,
            image,
            payload: &[],
            grants: &[],
            control_rights: SUPERVISOR_RIGHTS,
        }) {
            Ok(spawned) => spawned,
            Err(error) => {
                debug!("race drain-vs-drain: target spawn failed: {:?}", error);
                return false;
            }
        };
        if let Err(error) = process::kill(target.control, kill_code) {
            debug!("race drain-vs-drain: kill failed: {:?}", error);
            let _ = close(target.control);
            return false;
        }
        // 竞争点在双 drain 本身：先等 REAPABLE 再同刻发双 drain。
        if !await_reapable(target.control) {
            let _ = close(target.control);
            return false;
        }
        let moves = [
            HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            },
            HandleMove {
                handle: duplicate(target.control, Rights::MANAGE | Rights::TRANSIT).unwrap_or(Handle::INVALID),
                rights: Rights::MANAGE,
            },
        ];
        let drain = race_cmd(race::ACTION_DRAIN, 0);
        let sent = h.send_cmd(0, &drain, &[moves[0]]) && h.send_cmd(1, &drain, &[moves[1]]);
        h.fire(&[0, 1]);
        let (rep_a, _) = h.report(0).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let (rep_b, _) = h.report(1).unwrap_or((Report { status: -1, aux0: 0, aux1: 0 }, alloc::vec::Vec::new()));
        let legal = (rep_a.status == 0 || rep_a.status == busy)
            && (rep_b.status == 0 || rep_b.status == busy)
            && (rep_a.status == 0 || rep_b.status == 0);
        let allowed = [(ProcessExitReason::Killed as u32, Some(kill_code))];
        let terminal = drain_expect_dead(target.control, &allowed);
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
        let _ = close(target.control);
    }
    debug!("race drain-vs-drain {}", if ok { "passed" } else { "FAILED" });
    ok
}

/// 最后 control 消散：control 原件随 KILL 指令移交锤（kill 后即
/// close），init 不留任何权——REAPABLE 无主窗口只能经枚举+派生兜底
/// 接管，drain 至 Dead。
fn race_last_control(h: &RaceHammers, job: Handle, image: &[u8]) -> bool {
    let mut ok = true;
    for round in 0..2u64 {
        let kill_code = 0xA00 + round as i64;
        let target = match spawn(SpawnRequest {
            job,
            image,
            payload: &[],
            grants: &[],
            control_rights: SUPERVISOR_RIGHTS,
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
        let (rep, _) = match h.shoot(0, &race_cmd(race::ACTION_KILL, kill_code as u64), &moves) {
            Some(pair) => pair,
            None => {
                debug!("race last-control: hammer report missing");
                ok = false;
                continue;
            }
        };
        let allowed = [(ProcessExitReason::Killed as u32, Some(kill_code))];
        let terminal = derive_and_reap(job, pid, &allowed);
        if rep.status != 0 || terminal.is_none() {
            debug!(
                "race last-control: round {} rep={:#x} terminal={}",
                round,
                rep.status,
                terminal.is_some()
            );
            ok = false;
        }
    }
    debug!("race last-control {}", if ok { "passed" } else { "FAILED" });
    ok
}

/// 竞态矩阵入口：双锤编队 → 10 场景 → 退场收束 → 汇总。失败场景逐个
/// 点名，汇总行是全矩阵的 grep 锚点。
pub(crate) fn race_matrix(
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
    let scenarios: [(&str, bool); 10] = [
        ("kill-vs-kill", race_kill_kill(&h, acceptance, hammer_image)),
        ("kill-vs-exit", race_kill_exit(&h, acceptance, hammer_image)),
        ("kill-vs-fault", race_kill_fault(&h, acceptance, hammer_image)),
        ("kill-vs-start", race_kill_start(&h, acceptance)),
        ("kill-vs-park", race_kill_park(&h, acceptance, hammer_image)),
        ("kill-vs-abandon", race_kill_abandon(&h, acceptance)),
        ("create-vs-enumerate", race_create_enumerate(&h, acceptance)),
        ("seal-vs-create", race_seal_create(&h, acceptance)),
        ("drain-vs-drain", race_drain_drain(&h, acceptance, target_image)),
        ("last-control", race_last_control(&h, acceptance, target_image)),
    ];
    h.shutdown();
    let hammer_supervision = supervise_services(alloc::vec::Vec::from([
        Supervised { pid: h.pids[0], control: h.controls[0] },
        Supervised { pid: h.pids[1], control: h.controls[1] },
    ]));
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
