//! 信号原语（契约见 notes/ideas/signal.md）：对象切面状态位 + 等待者队列。
//!
//! 信号状态挂在可等待对象上（进程 / 邮箱 / 隧道端点），不设独立的
//! 信号对象类型；本模块只定义等待项的 ABI 编码与各对象的位义。

use crate::proc::SignalMap;

/// 进程级信号位：终止请求（REQUEST 而非强制；强制终结走进程回收通路）。
pub const TERMINATE: SignalMap = 1 << 0;
/// 进程级信号的用户自定义区起点（bit0 为 TERMINATE 保留）。
pub const PROCESS_USER_MASK: SignalMap = !TERMINATE;

/// 邮箱对象的信号位：有未取消息。与消息队列保持不变量
/// `NONEMPTY ⇔ 队列非空`，由内核维护。
pub const NONEMPTY: SignalMap = 1 << 0;

/// 隧道端点的信号位：对端摇了门铃（含义由协议解读，唤醒只是提示）。
/// 消费式清除，清铃前置条件见 Runnel 规格。
pub const TUNNEL_DATA: SignalMap = 1 << 0;
/// 隧道端点的信号位：对端已消亡或拆除（终态位，持续可见至本端销毁）。
pub const TUNNEL_PEER_CLOSED: SignalMap = 1 << 1;

/// 单次 SignalWait 的等待项数上限。
pub const SIGNAL_WAIT_MAX: usize = 64;

/// 可等待对象类别（[`SignalItem::kind`] 的取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum ObjectKind {
    /// 自身进程的信号状态（TERMINATE + 用户自定义区）。
    SelfProcess = 0,
    /// 自身邮箱（NONEMPTY）。
    Mailbox = 1,
    /// 自身已挂接的隧道端点（DATA / PEER_CLOSED），id 为隧道 id。
    TunnelEndpoint = 2,
}

/// 一个等待项：关注哪个对象 + 关注哪些位。
///
/// [`crate::call::SystemCall::SignalWait`] 接受本结构的数组；任一对象
/// 的信号状态命中关注位即唤醒，返回
/// `(命中项下标 << 56) | 命中位`。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignalItem {
    pub kind: u64,
    /// 对象实例标识（SelfProcess / Mailbox 恒为自身，预留隧道端点号等）。
    pub id: u64,
    pub interest: SignalMap,
}
