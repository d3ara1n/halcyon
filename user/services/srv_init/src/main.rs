//! 根监督：读取 initfs 组态，装配初始系统，持续监督并承担唯一系统终局。
//! init 只认识组态、出生帧与实例契约，不链接任何领域协议。
#![no_std]

extern crate alloc;
extern crate libruntime;

mod executor;
mod platform;
mod terminal;

use libkernel::{debug, ipc::capability::Capability, process, shared::startup::initial};

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
    let formation = match executor::load(archive, &platform, root_pool.as_handle()) {
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
    let result = executor.start_all().and_then(|()| executor.supervise());
    if let Err(reason) = result {
        let admins = executor.admins();
        fail(&services, &reset, &platform, &admins, reason);
    }
    // 持续服务的根监督没有正常完成结果；仅系统故障或受授权电源操作结束它。
    fail(
        &services,
        &reset,
        &platform,
        &[],
        "root supervision unexpectedly returned",
    );
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
    admins: &[erhino_shared::object::Handle],
    reason: &str,
) -> ! {
    terminal::run(
        terminal::Terminal {
            services: services.as_handle(),
            reset: reset.as_handle(),
            platform,
            admins,
        },
        reason,
    )
}
