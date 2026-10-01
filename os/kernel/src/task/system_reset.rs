//! `SystemReset` primordial capability 与平台复位提交。

use alloc::sync::Arc;
use core::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    reset::{ResetAction, ResetReason},
};

use super::{
    Thread,
    object::{HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef},
    proc::Process,
};

pub struct SystemReset {
    /// capability 持有计数（ZeroPolicy 按类型固定）。
    holders: capability::Holders,
    header: ObjectHeader,
    in_flight: AtomicBool,
}

impl SystemReset {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::new(),
            in_flight: AtomicBool::new(false),
        })
    }

    pub fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    fn begin(&self) -> Result<(), SystemCallError> {
        self.in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| SystemCallError::ObjectBusy)
    }

    fn finish_failed(&self) {
        self.in_flight.store(false, Ordering::Release);
    }
}

impl KernelObject for SystemReset {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }

    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::SystemReset
    }

    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::MANAGE | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT)
    }

    fn allowed_signals(&self) -> Option<ObjectSignals> {
        None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn request(
    thread: &Thread,
    handle: Handle,
    action_raw: u64,
    reason_raw: u64,
) -> Result<(), SystemCallError> {
    let action = ResetAction::try_from(action_raw).map_err(|_| SystemCallError::IllegalArgument)?;
    let reason = ResetReason::try_from(reason_raw).map_err(|_| SystemCallError::IllegalArgument)?;
    let object = {
        let table = thread.process.handles.lock();
        let entry = table
            .get(handle, Rights::MANAGE)
            .map_err(super::handle::map_error)?;
        if entry.object().kind() != ObjectKind::SystemReset
        {
            return Err(SystemCallError::WrongObjectType);
        }
        entry.object().clone()
    };
    let reset = object
        .as_any()
        .downcast_ref::<SystemReset>()
        .ok_or(SystemCallError::WrongObjectType)?;
    reset.begin()?;

    let (platform_action, platform_reason) = map_to_sbi(action, reason);
    log!(
        SBI,
        "system reset accepted: action {:?}, reason {:?}; {} frame(s) free",
        action,
        reason,
        crate::frame::free_frames()
    );
    crate::heap::log_snapshot("reset");
    log!(
        Task,
        "metadata reset usage {:?} peaks {:?}",
        crate::task::resources::admission_usage(),
        crate::task::resources::admission_peak()
    );
    let result = crate::sbi::system_reset(platform_action, platform_reason);
    reset.finish_failed();

    match result {
        Err(crate::sbi::SbiError::NotSupported) => Err(SystemCallError::NotSupported),
        Err(_) | Ok(_) => Err(SystemCallError::InternalError),
    }
}

fn map_to_sbi(action: ResetAction, reason: ResetReason) -> (u32, u32) {
    let action = match action {
        ResetAction::Shutdown => crate::sbi::RESET_SHUTDOWN,
        ResetAction::Reboot => crate::sbi::RESET_COLD_REBOOT,
    };
    let reason = match reason {
        ResetReason::Requested => crate::sbi::RESET_REASON_NONE,
        ResetReason::SystemFailure => crate::sbi::RESET_REASON_SYSTEM_FAILURE,
    };
    (action, reason)
}
