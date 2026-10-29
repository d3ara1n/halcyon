//! 唯一系统故障终局：封 services、撤销已收到的来源管理端、封平台根、有界收集后冷重启。
//! 终局不等待撤销完成，也不把进程 DONE 当作硬件静止证明；内核独立保留活 DMA。
use alloc::vec::Vec;
use erhino_shared::{
    object::Handle,
    reset::{ResetAction, ResetReason},
    time::Deadline,
};
use libkernel::{debug, ipc::capability::Capability, ipc::message::MailboxSender, process};
use libservice::instance::revoke;

/// 终局所需的 owner；由执行器持有，故障时一次交出。
pub(super) struct Terminal<'a> {
    pub services: Handle,
    pub reset: Handle,
    pub platform: &'a super::platform::Platform,
    pub admins: Vec<&'a Capability>,
}

pub(super) fn run(terminal: Terminal<'_>, reason: &str) -> ! {
    debug!(
        "system failure terminal entered: {}; hardware completion is unproven",
        reason
    );
    match process::seal_job(terminal.services) {
        Ok(()) => debug!("system failure services JobSeal committed"),
        Err(error) => debug!("system failure services JobSeal unconfirmed: {:?}", error),
    }
    let deadline = libkernel::time::timeout_millis(super::executor::TERMINAL_REVOKE_MS)
        .unwrap_or(Deadline::at(0));
    let mut caller = librpc::Caller::new();
    for admin in &terminal.admins {
        let revoked = (|| {
            let sender = MailboxSender::from_capability(super::executor::owned(
                libkernel::ipc::object::duplicate(admin.as_handle(), super::executor::CALL_RIGHTS)?,
            ))
            .map_err(|failure| failure.error)?
            .0;
            let body = revoke::encode_request(revoke::REVOKE, deadline)
                .ok_or(erhino_shared::call::SystemCallError::IllegalArgument)?;
            let request = librpc::Request::new(revoke::PROTOCOL, &body)?;
            caller
                .call(&sender, deadline, request)
                .map_err(|_| erhino_shared::call::SystemCallError::ObjectClosed)
        })();
        match revoked {
            Ok(_) => debug!("system failure source Revoke submitted"),
            Err(error) => debug!("system failure source Revoke unconfirmed: {:?}", error),
        }
    }
    terminal.platform.seal_all();
    match libprocess::job_kill_with_policy(
        terminal.services,
        libprocess::DEFAULT_SUPERVISION_POLICY,
    ) {
        Ok(()) => debug!("system failure services collection complete"),
        Err(error) => debug!("system failure services collection incomplete: {:?}", error),
    }
    debug!("system failure requesting cold reboot: SystemFailure");
    match libkernel::system::reset(
        terminal.reset,
        ResetAction::Reboot,
        ResetReason::SystemFailure,
    ) {
        Ok(never) => match never {},
        Err(error) => {
            debug!(
                "system failure cold reboot rejected: {:?}; terminating init",
                error
            );
            // SAFETY: 终局已有界推进；内核独立保留活 DMA，boot 观察 init DONE。
            unsafe { libkernel::sys_process_abort() }
        }
    }
}
