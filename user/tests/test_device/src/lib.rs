//! 正式设备验收客户端的具体装配槽；不属于设备领域协议。
#![no_std]
pub const DEVICES: usize = 0;
pub const DRIVERS: usize = 1;
pub const REPLY_BASE: usize = 2;
/// 两个各16条的未读回复队列，跨越服务单批请求容量；保留到管理者停止后。
pub const BACKLOG_PORTS: usize = 2;
pub const GRANTS: usize = REPLY_BASE + BACKLOG_PORTS;
