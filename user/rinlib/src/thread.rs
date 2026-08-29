use erhino_shared::proc::{ThreadStartContext, Tid};

use crate::call::sys_thread_spawn;

pub enum ThreadSpawnError {
    KernelError,
}

pub struct Thread {
    handle: Tid,
}

impl Thread {
    fn new(handle: Tid) -> Self {
        Self { handle }
    }

    pub fn id(&self) -> Tid {
        self.handle
    }
}

/// Running 进程的内部线程出生通道。栈与现场均由调用进程提供，
/// 与 Building 期 ProcessAttach 共用 ThreadStartContext。
pub fn spawn(context: &ThreadStartContext) -> Result<Thread, ThreadSpawnError> {
    // SAFETY: context 在 syscall 期间保持有效；地址语义由内核按当前进程校验。
    match unsafe { sys_thread_spawn(context) } {
        Ok(tid) => Ok(Thread::new(tid)),
        Err(_) => Err(ThreadSpawnError::KernelError),
    }
}
