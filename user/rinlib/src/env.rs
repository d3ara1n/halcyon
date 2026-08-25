use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use erhino_shared::{object::Handle, proc::Pid, startup::StartupGrant};

/// 0 = 未初始化（启动契约：lang_start 在任何用户代码前写入）。
static PID: AtomicU32 = AtomicU32::new(0);
static PARENT_PID: AtomicU32 = AtomicU32::new(0);
static STARTUP_MAILBOX: AtomicU64 = AtomicU64::new(0);
const STARTUP_GRANT_MAX: usize = 8;
static STARTUP_GRANT_COUNT: AtomicUsize = AtomicUsize::new(0);
static STARTUP_GRANT_TAGS: [AtomicU64; STARTUP_GRANT_MAX] = [const { AtomicU64::new(0) }; STARTUP_GRANT_MAX];
static STARTUP_GRANT_HANDLES: [AtomicU64; STARTUP_GRANT_MAX] =
    [const { AtomicU64::new(0) }; STARTUP_GRANT_MAX];

pub(crate) fn set_pid(pid: Pid) {
    PID.store(pid, Ordering::Relaxed);
}

pub(crate) fn set_parent_pid(pid: Pid) {
    PARENT_PID.store(pid, Ordering::Relaxed);
}

pub(crate) fn set_startup_mailbox(handle: Handle) {
    STARTUP_MAILBOX.store(handle.raw(), Ordering::Relaxed);
}

pub(crate) fn set_startup_grants(grants: &[StartupGrant], handles: &[Handle]) {
    let count = grants.len().min(STARTUP_GRANT_MAX);
    for (slot, grant) in grants.iter().take(count).enumerate() {
        let handle = handles
            .get(grant.handle_index as usize)
            .copied()
            .unwrap_or(Handle::INVALID);
        STARTUP_GRANT_TAGS[slot].store(grant.tag, Ordering::Relaxed);
        STARTUP_GRANT_HANDLES[slot].store(handle.raw(), Ordering::Relaxed);
    }
    STARTUP_GRANT_COUNT.store(count, Ordering::Release);
}

pub fn pid() -> Pid {
    PID.load(Ordering::Relaxed)
}

pub fn parent_pid() -> Pid {
    PARENT_PID.load(Ordering::Relaxed)
}

/// 控制面迁移期间的启动 Mailbox；后续由通用 startup-resource 枚举替代。
pub fn startup_mailbox() -> Handle {
    Handle::from_raw(STARTUP_MAILBOX.load(Ordering::Acquire))
}

pub fn startup_handle(tag: u64) -> Option<Handle> {
    let count = STARTUP_GRANT_COUNT.load(Ordering::Acquire);
    (0..count).find_map(|index| {
        (STARTUP_GRANT_TAGS[index].load(Ordering::Relaxed) == tag).then(|| {
            Handle::from_raw(STARTUP_GRANT_HANDLES[index].load(Ordering::Relaxed))
        })
    })
}
