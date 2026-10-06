//! 扁平集合协议及来源登记借用；执行运行体与接收装配共同依赖此处。

use alloc::rc::Rc;
use core::cell::Cell;
use erhino_shared::{
    call::SystemCallError,
    time::Deadline,
    wait_set::{MemberRequest, ReadyRecord},
};
use libbudget::Charge;

/// 内核集合与可移植验证共用的真实集合契约；同一集合的 token 不复用。
/// Add 成功后必须由其登记 owner Remove；Receive/Wait/Close 仅由 Assembly 调用。
pub trait SourceOps: Sized {
    fn register_item(&self, item: MemberRequest) -> Result<u64, SystemCallError>;
    fn remove_source(&self, token: u64) -> Result<(), SystemCallError>;
    fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError>;
    fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError>;
    fn now_ns(&self) -> Result<u64, SystemCallError>;
    fn close_set(self) -> Result<(), (Self, SystemCallError)>;
}
pub(crate) struct SetState<S> {
    pub(crate) set: Option<S>,
    pub(crate) failed: Cell<bool>,
    pub(crate) _charge: Charge,
}
pub struct Registration<S> {
    pub(crate) state: Rc<SetState<S>>,
}
impl<S: SourceOps> Registration<S> {
    pub(crate) fn set(&self) -> &S {
        self.state.set.as_ref().expect("live binding lost set")
    }
    pub(crate) fn failed(&self) -> bool {
        self.state.failed.get()
    }
    pub(crate) fn invalidate(&self) {
        self.state.failed.set(true);
    }
}

#[cfg(target_arch = "riscv64")]
impl SourceOps for libkernel::ipc::wait_set::WaitSet {
    fn register_item(&self, item: MemberRequest) -> Result<u64, SystemCallError> {
        self.register(item)
    }
    fn remove_source(&self, token: u64) -> Result<(), SystemCallError> {
        self.remove(token)
    }
    fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError> {
        self.receive_into(records)
    }
    fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError> {
        self.wait(deadline).map(|_| ())
    }
    fn now_ns(&self) -> Result<u64, SystemCallError> {
        libkernel::time::snapshot().map(|snapshot| snapshot.now_ns)
    }
    fn close_set(self) -> Result<(), (Self, SystemCallError)> {
        self.close()
    }
}
