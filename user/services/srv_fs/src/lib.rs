//! srv_fs 的部署政策与容量元数据；不表示运行时授权或额度查询。
#![no_std]
pub mod deployment {
    /// 根出口允许普通 sender、显式 grant 与 Runnel invitation 所需的运输位。
    /// 各输出角色仍须独立收窄，不自动附加 GRANT。
    pub fn root_output_transport() -> erhino_shared::object::Rights {
        use erhino_shared::object::Rights;
        Rights::READ
            | Rights::WRITE
            | Rights::SIGNAL
            | Rights::WAIT
            | Rights::MANAGE
            | Rights::TRANSIT
            | Rights::DUPLICATE
            | Rights::GRANT
    }

    pub const NODES: usize = 64;
    pub const GRANTS: usize = 16;
    pub const ROLES: usize = 16;
    pub const REGISTRATIONS: usize = 16;
    pub const WATCHES: usize = 24;
    /// 有序启动入口 A/B；各入口额度是部署政策，不是认证身份。
    pub const ENTRIES: usize = 2;
    pub const ENTRY_WATCHES: [usize; ENTRIES] = [WATCHES / ENTRIES, WATCHES];
    pub const REQUESTS: usize = 16;
    pub const SESSIONS: usize = GRANTS;
    /// A 的会话局部预算为半数，B 可使用整个共享预算。
    pub const ENTRY_SESSIONS: [usize; ENTRIES] = [SESSIONS / ENTRIES, SESSIONS];
    pub const OFFER_WINDOW_NS: u64 = 5_000_000_000;
    pub const STREAM_TUNNEL_BYTES: usize = 3 * erhino_shared::proc::PROCESS_PAGE_SIZE;
    /// 包括不可见准备、已发布和等待退休的 Binding 稳定记录。
    pub const BINDINGS: usize = GRANTS;
    /// 每个业务请求至多一项下游，另加互斥的服务发布/撤出调用。
    pub const RPC_CALLS: usize = REQUESTS + 1;
    // 固定 ingress/退休/释放/维护/Dispatcher 与各入口的四种 Root。
    pub const TASKS: usize =
        GRANTS + 2 * ROLES + REGISTRATIONS + 2 * WATCHES + REQUESTS + SESSIONS + 6 + 4 * ENTRIES;
    pub const SOURCES: usize = GRANTS
        + 2 * ROLES
        + 2 * REGISTRATIONS
        + 2 * WATCHES
        + REQUESTS
        + 2 * SESSIONS
        + 6
        + 4 * ENTRIES
        + 2 * RPC_CALLS
        + 1;
}
