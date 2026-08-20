use erhino_shared::{
    call::SystemCallError,
    proc::{Pid, SignalMap, SystemSignal},
};
use flagset::FlagSet;
use num_traits::FromPrimitive;

use crate::call::{sys_signal_return, sys_signal_send, sys_signal_set};

static mut SIGNAL_HANDLER: Option<fn(SystemSignal)> = None;

#[derive(Debug)]
pub enum SignalError {
    InternalError,
    ProcessNotFound,
}

pub fn set_handler<S: Into<FlagSet<SystemSignal>>>(mask: S, handler: fn(SystemSignal)) {
    unsafe {
        SIGNAL_HANDLER = Some(handler);
        sys_signal_set(mask.into(), signal_handler_wrapper as *const () as usize).expect("this wont failed");
    }
}

pub fn send(pid: Pid, signal: SystemSignal) -> Result<bool, SignalError> {
    unsafe {
        sys_signal_send(pid, signal).map_err(|e| match e {
            SystemCallError::ObjectNotFound => SignalError::ProcessNotFound,
            _ => SignalError::InternalError,
        })
    }
}

fn signal_handler_wrapper(signal: SignalMap) {
    if let Some(handler) = unsafe { SIGNAL_HANDLER } {
        if let Some(signal) = SystemSignal::from_u64(signal) {
            handler(signal)
        }
    }
    unsafe {
        sys_signal_return().expect("wont failed if signal_handler called only by kernel");
    }
}
