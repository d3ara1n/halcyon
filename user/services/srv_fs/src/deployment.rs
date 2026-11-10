//! 有限部署的入口、工作集与故障检测政策。
use crate::startup::Config;
use erhino_shared::call::SystemCallError;

pub const NODES: usize = 64;
pub const GRANTS: usize = 16;
pub const ROLES: usize = 16;
pub const REGISTRATIONS: usize = 16;
pub const WATCHES: usize = 24;
/// 非 registry 的最小固定输入为四项，每入口两组 sender/Lifetime。
/// registry 每入口四组，另由 Config 按实际角色数收紧；Namespace 仍计入出生总限制。
pub const MAX_ENTRIES: usize = (erhino_shared::proc::PROCESS_MAX_GRANTS - 4) / 4;
pub const REQUESTS: usize = 16;
pub const SESSIONS: usize = GRANTS;
pub const OFFER_WINDOW_NS: u64 = 5_000_000_000;
pub const STREAM_TUNNEL_BYTES: usize = 3 * erhino_shared::proc::PROCESS_PAGE_SIZE;
pub const BINDINGS: usize = GRANTS;
pub const RPC_CALLS: usize = REQUESTS + 1;
/// ELF/主栈/出生映射由装载布局另加；本项为内存后端、流与运行体的显式工作集。
pub const DOMAIN_BYTES: usize = 2 * 1024 * 1024;
pub const HEAP_PAGES: usize = 4096;
/// 映射与对象/能力/来源/退休峰值的部署 metadata 工作集。
pub const METADATA_BYTES: usize = 1024 * 1024;

/// 运行体容量：固定 ingress/退休/维护/Dispatcher 与实际入口数的 Root 推导。
#[derive(Clone, Copy)]
pub struct Capacity {
    pub tasks: usize,
    pub sources: usize,
}
impl Capacity {
    pub fn new(config: &Config<'_>) -> Result<Self, SystemCallError> {
        let roots = config
            .entries()
            .len()
            .checked_mul(if config.registry { 4 } else { 2 })
            .ok_or(SystemCallError::ReachLimit)?;
        let fixed_tasks = if config.registry { 6 } else { 4 };
        let fixed_sources = 2 * 2 + 1 + 2 * RPC_CALLS + 1 + if config.registry { 2 } else { 0 };
        let tasks =
            (GRANTS + 2 * ROLES + REGISTRATIONS + 2 * WATCHES + REQUESTS + SESSIONS + fixed_tasks)
                .checked_add(roots)
                .ok_or(SystemCallError::ReachLimit)?;
        let sources = (GRANTS
            + 2 * ROLES
            + 2 * REGISTRATIONS
            + 2 * WATCHES
            + REQUESTS
            + 2 * SESSIONS
            + fixed_sources)
            .checked_add(roots)
            .ok_or(SystemCallError::ReachLimit)?;
        Ok(Self { tasks, sources })
    }
}
