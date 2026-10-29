//! 根监督：读取 initfs 组态，装配初始系统，持续监督并承担唯一系统终局。
//! init 只认识组态、出生帧与实例契约，不链接任何领域协议。
#![no_std]

extern crate alloc;
extern crate libruntime;

mod executor;
mod platform;
mod terminal;

use formation::Finish;
use libkernel::{
    debug,
    ipc::capability::Capability,
    process,
    shared::{
        reset::{ResetAction, ResetReason},
        startup::initial,
    },
    system,
};

fn take(grants: &mut libprocess::context::InitialGrants, slot: usize, what: &str) -> Capability {
    grants
        .take(slot)
        .unwrap_or_else(|| panic!("init startup input missing: {what}"))
}

fn main() {
    let (process, payload, mut grants) = libruntime::startup::take()
        .expect("init startup context missing")
        .into_parts();
    let description = take(
        &mut grants,
        initial::PLATFORM_DESCRIPTION,
        "platform description",
    );
    let platform = platform::Platform::adopt(description, &mut grants)
        .expect("initial platform description invalid");
    let root_job = take(&mut grants, initial::ROOT_JOB, "root Job");
    let reset = take(&mut grants, initial::SYSTEM_RESET, "SystemReset");
    let root_pool = take(&mut grants, initial::ROOT_MEMORY_POOL, "root MemoryPool");
    let archive = payload.as_static_bytes();
    // 组态在任何进程出生前完整校验；失败时尚无服务责任，直接退出交 boot 观察。
    let formation = match executor::load(archive) {
        Ok(formation) => formation,
        Err(reason) => {
            debug!("init formation rejected: {}", reason);
            // SAFETY: 尚未建立任何子进程或委托责任。
            unsafe { libkernel::sys_process_abort() }
        }
    };
    let services = owned_job(root_job.as_handle(), root_pool.as_handle());
    let mut executor = executor::Executor::new(
        formation,
        archive,
        process,
        services.as_handle(),
        root_pool.as_handle(),
        &platform,
    )
    .unwrap_or_else(|reason| fail(&services, &reset, &platform, &[], reason));
    let result = executor
        .start_all()
        .and_then(|()| executor.supervise())
        .and_then(|()| executor.stop_all());
    if let Err(reason) = result {
        let admins = executor.admins();
        fail(&services, &reset, &platform, &admins, reason);
    }
    let action = match executor.finish() {
        Finish::Shutdown => ResetAction::Shutdown,
        Finish::Reboot => ResetAction::Reboot,
        Finish::Stay => unreachable!("a staying formation never finishes supervision"),
    };
    debug!("init: formation complete, submitting {:?}", action);
    match system::reset(reset.as_handle(), action, ResetReason::Requested) {
        Ok(never) => match never {},
        Err(error) => {
            debug!("system reset failed: {:?}", error);
            // SAFETY: reset 失败是不可恢复终局；内核稳定持 DMA 责任，boot 观察 init DONE。
            unsafe { libkernel::sys_process_terminate() }
        }
    }
}

fn owned_job(
    root_job: libkernel::shared::object::Handle,
    pool: libkernel::shared::object::Handle,
) -> Capability {
    let handle = process::create_job(root_job, pool, executor::JOB_RIGHTS)
        .expect("services Job creation failed");
    // SAFETY: create_job 刚返回的表项，关闭责任唯一移交。
    unsafe { Capability::from_raw(handle) }
}

fn fail(
    services: &Capability,
    reset: &Capability,
    platform: &platform::Platform,
    admins: &[&Capability],
    reason: &str,
) -> ! {
    terminal::run(
        terminal::Terminal {
            services: services.as_handle(),
            reset: reset.as_handle(),
            platform,
            admins: admins.to_vec(),
        },
        reason,
    )
}
