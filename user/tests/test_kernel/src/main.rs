//! 内核契约验收；仅持自身付款域、验收 Job 子树与受托平台根别名。
#![no_std]

extern crate alloc;
extern crate libruntime;

use erhino_shared::sync::spin::SimpleLock;
use erhino_shared::time::Deadline;
use libbudget::Budget;
use libexecution::{
    ExecutionResource,
    runtime::{
        Advance, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind, Step, Task,
    },
};
use libkernel::ipc::wait_set::WaitSet;
use libkernel::ipc::{
    capability::Capability,
    message::{MailboxSender, SendOnce},
    packet::Packet,
};
use libkernel::memory_pool::MemoryPool;
use libkernel::shared::proc::Pid;
use libkernel::{
    ipc::{
        message::{create, discard, make_send_once, mint_sender, receive, send_raw, wait_message},
        notification,
        object::{close, duplicate},
        wait::wait_until,
    },
    memory_object::MemoryObject,
    mm::{MappedRegion, Placement},
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        mem::MemoryProtection,
        memory_object::MemoryObjectState,
        message::{HandleMove, MAILBOX_CAPACITY},
        object::{Handle, ObjectSignals, Rights},
        proc::{HandleGrant, JobMemberKind, JobState, ProcessState, TerminationCause},
        wait::WaitReason,
        wait_set::MemberRequest,
    },
};
use libprocess::{
    DEFAULT_SUPERVISION_POLICY, DERIVED_CONTROL_RIGHTS, SpawnRequest, SupervisionCause,
    SupervisionStage, SupervisionTarget, collect_process, enumerate_members, spawn,
};
use librpc::{
    CallCause, CallError, CallPhase, Caller, FrameRejection, Outbox, OutboxResult, Request,
    RequestContext, RpcMessageKind, RpcPrefix,
};
use librunnel::{ConsumerReady, blocking};
use libthread::ThreadOutcome;
use lock_api::Mutex;

mod harness;
use harness::Harness;
mod startup;
use startup::Mode;
use startup::{current_pid, current_process, memory_pool, mode};
mod building;
mod dma_checks;
mod ipc_checks;
mod ipc_thread_checks;
mod irq_checks;
mod job_checks;
mod memory_checks;
mod mmio_checks;
mod once_checks;
mod race_checks;
mod rpc_checks;
mod time_checks;
mod tunnel_checks;
use building::build_spin_building;

/// 本进程保留 Control 的子进程。
struct Supervised {
    pid: u64,
    control: Handle,
}
/// 服务 ProcessControl 的监督基准权利：验收进程保留 control 用于查询、等待、
/// 终止与受保护收束，并可复制/直接转授。
const SUPERVISOR_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::GRANT.raw(),
);

/// JobControl 满权（scratch child Job 请求基准；root Handle 持超集）。
const JOB_FULL_RIGHTS: Rights = Rights::from_raw(
    Rights::CREATE.raw()
        | Rights::MANAGE.raw()
        | Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

/// 多页流与生命周期 fixture 各自使用正式自动选址。
const TUNNEL_BYTES: usize = 3 * 4096;
/// 超过多倍环容量，覆盖真实背压、分页与回绕。
const STREAM_LEN: usize = 65536;
const CONTROL_STRESS: usize = 128;

const TUNNEL_STRESS: usize = 64;
fn take_thread_result<T>(outcome: ThreadOutcome<T, libthread::NoEnvironment>) -> T {
    match outcome {
        ThreadOutcome::Returned {
            value,
            leave: Ok(()),
        } => value,
        ThreadOutcome::Returned { leave: Err(()), .. }
        | ThreadOutcome::EnterFailed(())
        | ThreadOutcome::Incomplete => panic!("runtime worker did not complete"),
    }
}
/// 小集合收束辅助：对保留 control 的服务 kill → 等 DONE → 核验终态 → close。
fn kill_and_collect(
    harness: &mut Harness,
    supervised: alloc::vec::Vec<Supervised>,
) -> Result<(), &'static str> {
    for target in &supervised {
        let _ = process::kill(target.control);
    }
    harness.collect_targets(supervised)
}

fn launch_children(
    harness: &mut Harness,
    services: Handle,
    pm_domain: Handle,
    pm_image: &[u8],
    target_image: &[u8],
) -> Result<Handle, &'static str> {
    let mailbox = harness
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE,
        )
        .map_err(|_| "pm mailbox failed")?;
    let delegated = harness
        .duplicate(pm_domain, JOB_FULL_RIGHTS)
        .map_err(|_| "pm domain delegation failed")?;
    let target = spawn(SpawnRequest {
        memory_pool: memory_pool(),
        parent: current_process(),
        job: pm_domain,
        image: target_image,
        payload: &[],
        grants: &[],
        control_rights: SUPERVISOR_RIGHTS,
    })
    .map_err(|_| "pm target spawn failed")?;
    // SAFETY: spawn 交付的唯一 Control 尚未进入账本；pm 持真实域管理权。
    unsafe { close(target.control) }.map_err(|_| "pm target control close failed")?;
    let grants = [
        HandleGrant {
            handle: mailbox.owner,
            rights: Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        },
        HandleGrant {
            handle: delegated,
            rights: DERIVED_CONTROL_RIGHTS,
        },
    ];
    harness
        .reserve_process()
        .map_err(|_| "pm supervision admission failed")?;
    let pm = spawn(SpawnRequest {
        memory_pool: memory_pool(),
        parent: current_process(),
        job: services,
        image: pm_image,
        payload: &[],
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    });
    if pm.is_ok()
        || matches!(&pm, Err(failure) if failure.grants == libprocess::GrantOutcome::Consumed)
    {
        for grant in grants {
            harness.transferred(grant.handle);
        }
    }
    let pm = pm.map_err(|_| "pm spawn failed")?;
    harness.track_process(Supervised {
        pid: pm.pid,
        control: pm.control,
    });
    Ok(mailbox.peer)
}

fn run(
    harness: &mut Harness,
    services: Handle,
    pm_image: &[u8],
    target: &[u8],
    hammer: Option<&[u8]>,
) -> Result<(), &'static str> {
    match startup::suite() {
        "memory" => {
            if mode().memory() {
                let job = harness
                    .create_job(services, memory_pool(), JOB_FULL_RIGHTS)
                    .map_err(|_| "memory workload job create failed")?;
                if !race_checks::thread_memory_suite(
                    job,
                    hammer.ok_or("memory workload hammer missing")?,
                ) {
                    return Err("memory workload thread suite failed");
                }
                harness.collect_job(job)?;
                harness.close_control(job)?;
            } else {
                memory_checks::test_memory_mapping()?;
                memory_checks::test_process_funding(services)?;
                memory_checks::test_pool_budget();
            }
        }
        "ipc" => {
            let job = harness
                .create_job(services, memory_pool(), JOB_FULL_RIGHTS)
                .map_err(|_| "IPC workload job create failed")?;
            ipc_thread_checks::shared_sender();
            ipc_thread_checks::notification_roles();
            once_checks::run(harness, job, target)?;
            ipc_checks::test_capability_badges_and_affine_owners();
            ipc_checks::test_send_once();
            ipc_checks::test_wait_set_retirement();
            ipc_checks::test_writable_level();
            ipc_thread_checks::threaded_receive();
            if mode().stress() {
                ipc_checks::stress_control_plane();
            }
            harness.collect_job(job)?;
            harness.close_control(job)?;
        }
        "process" => {
            let job = harness
                .create_job(services, memory_pool(), JOB_FULL_RIGHTS)
                .map_err(|_| "process workload job create failed")?;
            let child = spawn(SpawnRequest {
                memory_pool: memory_pool(),
                parent: current_process(),
                job,
                image: target,
                payload: &[],
                grants: &[],
                control_rights: SUPERVISOR_RIGHTS,
            })
            .map_err(|_| "derive target spawn failed")?;
            job_checks::test_derive_kill(harness, job, child.pid, child.control)?;
            ipc_thread_checks::committed_kill(job, target);
            job_checks::test_building_kill(job);
            job_checks::test_job_management(harness, job, target)?;
            if mode().stress() {
                race_checks::race_matrix(
                    harness,
                    job,
                    target,
                    hammer.ok_or("race hammer missing")?,
                )?;
            }
            harness.collect_job(job)?;
            harness.close_control(job)?;
        }
        "transport" => run_transport(harness, services, pm_image, target)?,
        "mmio" => {
            let root = startup::ecam_handle().ok_or("MMIO platform root missing")?;
            if mode().ordinary() {
                mmio_checks::run(root)?;
            }
            mmio_checks::process_retirement(root, services, target)?;
        }
        "irq" => {
            let root = startup::irq_handle().ok_or("IRQ platform root missing")?;
            if mode().ordinary() {
                irq_checks::run(root)?;
            }
            if mode().has(Mode::IRQ_EXIT) {
                irq_checks::process_retirement(root, services, target)?;
            }
        }
        "dma" => {
            let root = startup::dma_handle().ok_or("DMA platform root missing")?;
            if mode().ordinary() {
                dma_checks::run(root)?;
            }
            dma_checks::process_retirement(root, services, target)?;
        }
        "time" => time_checks::run(),
        // 有限组态的 task：出生采用由 startup 完成，随后执行同一 Job 收集与完成报告。
        "startup" => {}
        _ => return Err("kernel acceptance suite unknown"),
    }
    harness.collect_services()?;
    Ok(())
}

fn run_transport(
    harness: &mut Harness,
    services: Handle,
    pm_image: &[u8],
    target: &[u8],
) -> Result<(), &'static str> {
    tunnel_checks::test_tunnel_failure_owners();
    tunnel_checks::test_tunnel_geometry();
    rpc_checks::test_rpc_reject_cleanup();
    if mode().stress() {
        tunnel_checks::test_tunnel_lifecycle();
    }
    let pm_domain = harness
        .create_job(services, memory_pool(), JOB_FULL_RIGHTS)
        .map_err(|_| "pm domain job create failed")?;
    let pm_mailbox = launch_children(harness, services, pm_domain, pm_image, target)?;
    // —— 数据面：建隧道 → Invitation 经消息面转移 → 阻塞读流 ——
    let (tunnel, invitation) = match blocking::Consumer::create(
        memory_pool(),
        TUNNEL_BYTES,
        libkernel::mm::Placement::Anywhere,
    ) {
        Ok((consumer, invitation)) => (consumer, invitation),
        Err(blocking::CreateFailure::Tunnel(
            libkernel::ipc::invitation::CreateFailure::System(error),
        )) => {
            debug!("tunnel create failed: {:?}", error);
            return Err("tunnel create failed");
        }
        Err(blocking::CreateFailure::Tunnel(
            libkernel::ipc::invitation::CreateFailure::Published {
                endpoint,
                invitation,
                error,
            },
        )) => {
            harness.retain_failed_create(endpoint, invitation);
            debug!("tunnel create output invalid: {:?}", error);
            return Err("tunnel create output invalid");
        }
        Err(blocking::CreateFailure::Map {
            failure,
            invitation,
        }) => {
            harness.retain_failed_create(Some(failure.endpoint), Some(invitation));
            return Err("tunnel mapping failed");
        }
        Err(blocking::CreateFailure::Protocol {
            endpoint,
            invitation,
            error,
        }) => {
            harness.retain_transport(endpoint, invitation);
            debug!("tunnel protocol init failed: {:?}", error);
            return Err("tunnel init failed");
        }
    };
    harness.start_read(tunnel, alloc::vec::Vec::new());
    debug!("tunnel created");
    // 生产侧以 typed Packet 转移 Invitation：失败完整返还 owner，
    // 不再以裸 HandleMove 表达（原失败路径承载直接丢失）。
    let mut packet =
        Packet::new(514, &[]).map_err(|_| "tunnel invitation packet allocation failed")?;
    if let Err(failure) = packet.push(
        invitation.into_capability(),
        Rights::READ | Rights::WRITE | Rights::WAIT | Rights::SIGNAL | Rights::MANAGE,
    ) {
        harness.retain_capability(failure.capability);
        return Err("tunnel invitation push failed");
    }
    if let Err(mut failure) = packet.try_send(
        harness.sender(pm_mailbox),
        libkernel::time::Deadline::INFINITE,
    ) {
        let (invitation, _) = failure
            .packet
            .pop()
            .expect("tunnel invitation transfer disappeared");
        harness.retain_capability(invitation);
        debug!("send tunnel invitation failed: {:?}", failure.error);
        return Err("send tunnel invitation failed");
    }

    harness.await_read()?;
    let read = harness.read_world();
    let n = read.filled;
    if n != STREAM_LEN
        || !read.buffer[..n]
            .iter()
            .enumerate()
            .all(|(i, &byte)| byte == (i % 251 + 1) as u8)
    {
        return Err("stream pattern or length mismatch");
    }
    debug!("stream received {} bytes, pattern ok", n);
    debug!(
        "RNL2 multi-page stream passed: bytes={}, capacity={}",
        n,
        read.tunnel
            .as_ref()
            .expect("read duty owns its tunnel")
            .capacity()
    );

    // —— 流控唤醒面：pm 填满目标邮箱后在 WRITABLE 上阻塞，腾位唤醒 ——
    ipc_checks::test_writable_wake(harness, pm_mailbox)?;
    harness.close_control(pm_mailbox)?;

    // —— 事件面：自动退休已发布对端终态，电平等待立即返回 ——
    let peer_closed = tunnel_checks::wait_supervision_signal(
        harness
            .read_world()
            .tunnel
            .as_mut()
            .expect("root owns the tunnel"),
        "peer-closed",
    )?;
    debug!(
        "peer closed observed: bits={:#x}",
        peer_closed.observed.raw()
    );
    let tunnel_pool_before_close = if mode().stress() {
        None
    } else {
        Some(memory_checks::pool_allocated().map_err(|_| "tunnel Pool pre-close query failed")?)
    };
    harness.close_read()?;

    let tunnel_pool_after_close = if mode().stress() {
        None
    } else {
        Some(memory_checks::pool_allocated().map_err(|_| "tunnel Pool post-close query failed")?)
    };
    if !mode().stress() {
        let tunnel_pool_before_close =
            tunnel_pool_before_close.expect("non-stress baseline was queried");
        let tunnel_pool_after_close =
            tunnel_pool_after_close.expect("non-stress final balance was queried");
        let refunded = tunnel_pool_before_close.saturating_sub(tunnel_pool_after_close);
        if refunded == 0 {
            debug!(
                "tunnel Pool charge did not decrease after close: before={}, after={}",
                tunnel_pool_before_close, tunnel_pool_after_close
            );
            return Err("tunnel Pool charge did not refund after close");
        }
        debug!(
            "tunnel Pool conservation after close passed: before={}, after={}, refunded={}",
            tunnel_pool_before_close, tunnel_pool_after_close, refunded
        );
    }

    // —— 委托域终局：pm 的管理段应已把 pm_domain 收束到 Dead；降级时
    // test_kernel 以保留的直接收束权兜底（job_kill = seal + 枚举派生 kill +
    // drain + CLOSED 屏障）。 ——
    let domain_state = process::query_job(pm_domain);
    if !matches!(&domain_state, Ok(s) if s.state == JobState::Dead as u32) {
        debug!("pm delegated domain not collected by pm; test_kernel collecting");
        harness.collect_job(pm_domain)?;
    }
    let domain_state = process::query_job(pm_domain).map_err(|error| {
        debug!("pm delegated domain final query failed: {:?}", error);
        "pm delegated domain final query failed"
    })?;
    if domain_state.state != JobState::Dead as u32 {
        return Err("pm delegated domain did not reach Dead");
    }
    debug!("pm delegated domain confirmed Dead");

    harness.close_control(pm_domain)?;
    rpc_checks::exercise_cancellable_rpc(harness)?;
    Ok(())
}

fn main() {
    startup::run();
}
