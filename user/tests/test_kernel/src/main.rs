//! 内核契约验收；仅持自身付款域、验收 Job 子树与受托平台根别名。
#![no_std]

extern crate alloc;
extern crate libruntime;

use erhino_shared::sync::spin::SimpleLock;
use erhino_shared::time::Deadline;
use libkernel::ipc::capability::Capability;
use libkernel::memory_pool::MemoryPool;
use libkernel::shared::proc::Pid;
use libkernel::{
    ipc::{
        message::{create, discard, make_send_once, mint_sender, receive, send_raw},
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
        wait_set::MemberRequest,
    },
};
use libprocess::{
    DEFAULT_SUPERVISION_POLICY, DERIVED_CONTROL_RIGHTS, SpawnRequest, SupervisionCause,
    SupervisionStage, SupervisionTarget, collect_process, enumerate_members, spawn,
};
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

fn run(
    harness: &mut Harness,
    services: Handle,
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
        "tunnel" => {
            tunnel_checks::test_tunnel_failure_owners();
            tunnel_checks::test_tunnel_geometry();
            if mode().stress() {
                tunnel_checks::test_tunnel_lifecycle();
            }
        }
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

fn main() {
    startup::run();
}
