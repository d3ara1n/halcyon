//! 服务实例契约：监督方（init、`srv_service`）与实例之间的就绪、停止与撤销协议。
//!
//! 出生输入由 `libprocess::birth` 交付；本模块定义其中两个生命周期输入的用途：
//! - `instance.report`：实例向监督方发送 `Export`（每条恰一个句柄）、`Ready` 与 `Stopped`；
//! - `instance.stop`：监督方 signal 即请求停止，该输入关闭（监督方消失）同样视为停止请求。
//!
//! 可撤销导出的管理端只实现两个操作：`Revoke`（幂等）与 `QueryRevoked`。`Closed`
//! 表示提供方账本中经该来源派生的活跃使用为空；“活跃使用”的含义由领域拥有。
//! 本模块不认识任何领域，也不依赖服务目录。

use erhino_shared::object::{ObjectKind, ObjectSignals, Rights};

/// 生命周期输入名；与组态的保留输入名一致。
pub const REPORT: &str = "instance.report";
pub const STOP: &str = "instance.stop";

/// 生命周期输入的类型与最小权利。
pub const REPORT_KIND: ObjectKind = ObjectKind::MailboxSender;
pub const REPORT_RIGHTS: Rights =
    Rights::from_raw(Rights::WRITE.raw() | Rights::WAIT.raw() | Rights::TRANSIT.raw());
pub const STOP_KIND: ObjectKind = ObjectKind::NotificationOwner;
pub const STOP_RIGHTS: Rights = Rights::from_raw(Rights::READ.raw() | Rights::WAIT.raw());
/// `instance.stop` 上表示停止请求的信号：READABLE（监督方 signal）或 CLOSED（监督方消失）。
pub const STOP_SIGNALS: ObjectSignals =
    ObjectSignals::from_raw(ObjectSignals::READABLE.raw() | ObjectSignals::CLOSED.raw());

/// 报告消息种类。
pub const EXPORT_KIND: u64 = u64::from_le_bytes(*b"INSTEXPT");
pub const READY_KIND: u64 = u64::from_le_bytes(*b"INSTREDY");
pub const STOPPED_KIND: u64 = u64::from_le_bytes(*b"INSTSTOP");

/// `Ready` 附带的领域摘要上限；监督方不解释，可转交消费者。
pub const SUMMARY_MAX: usize = 256;
const READY_HEADER: usize = 4;

/// 监督方收到的一条已解码报告。句柄由调用方从原消息中取出，本类型只描述载荷。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report<'a> {
    /// 一个命名导出；消息恰带一个句柄。
    Export { name: &'a str },
    /// 全部导出已发送；`exports` 为此前 `Export` 的条数。
    Ready { exports: u32, summary: &'a [u8] },
    /// 实例已完成收尾，随后以 Completed 退出。
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportError {
    UnknownKind,
    BadPayload,
    /// 句柄数与报告种类不符。
    Handles,
}

/// 名字只需非空 ASCII 且不超过 32 字节；完整名字语法由组态校验拥有。
pub const NAME_MAX: usize = 32;

/// 编码 `Export` 载荷（名字字节）。
pub fn export_payload(name: &str) -> Option<&[u8]> {
    (!name.is_empty() && name.len() <= NAME_MAX && name.is_ascii()).then_some(name.as_bytes())
}

/// 编码 `Ready` 载荷：u32 导出条数 + 摘要。
pub fn ready_payload(exports: u32, summary: &[u8], out: &mut [u8]) -> Option<usize> {
    let len = READY_HEADER.checked_add(summary.len())?;
    if summary.len() > SUMMARY_MAX || out.len() < len {
        return None;
    }
    out[..READY_HEADER].copy_from_slice(&exports.to_le_bytes());
    out[READY_HEADER..len].copy_from_slice(summary);
    Some(len)
}

/// 解码一条报告；`handles` 为消息实际携带的句柄数。
pub fn decode_report(kind: u64, payload: &[u8], handles: usize) -> Result<Report<'_>, ReportError> {
    match kind {
        EXPORT_KIND => {
            if handles != 1 {
                return Err(ReportError::Handles);
            }
            let name = core::str::from_utf8(payload).map_err(|_| ReportError::BadPayload)?;
            export_payload(name).ok_or(ReportError::BadPayload)?;
            Ok(Report::Export { name })
        }
        READY_KIND => {
            if handles != 0 {
                return Err(ReportError::Handles);
            }
            if payload.len() < READY_HEADER || payload.len() - READY_HEADER > SUMMARY_MAX {
                return Err(ReportError::BadPayload);
            }
            let exports = u32::from_le_bytes(
                payload[..READY_HEADER]
                    .try_into()
                    .map_err(|_| ReportError::BadPayload)?,
            );
            Ok(Report::Ready {
                exports,
                summary: &payload[READY_HEADER..],
            })
        }
        STOPPED_KIND => {
            if handles != 0 {
                return Err(ReportError::Handles);
            }
            if !payload.is_empty() {
                return Err(ReportError::BadPayload);
            }
            Ok(Report::Stopped)
        }
        _ => Err(ReportError::UnknownKind),
    }
}

/// 撤销协议：管理端上的请求与状态。
pub mod revoke {
    use erhino_shared::time::Deadline;
    /// RPC 协议标识。
    pub const PROTOCOL: u64 = u64::from_le_bytes(*b"INSTRVKE");
    pub const REVOKE: u16 = 1;
    pub const QUERY: u16 = 2;

    /// `QueryRevoked` 的结果。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u32)]
    pub enum State {
        /// 来源仍开放。
        Open = 0,
        /// 已撤销，仍有活跃派生使用。
        Revoking = 1,
        /// 已撤销且活跃派生使用为空。
        Closed = 2,
    }

    impl State {
        pub fn encode(self) -> [u8; 4] {
            (self as u32).to_le_bytes()
        }
        pub fn decode(bytes: &[u8]) -> Option<Self> {
            Some(match u32::from_le_bytes(bytes.try_into().ok()?) {
                0 => Self::Open,
                1 => Self::Revoking,
                2 => Self::Closed,
                _ => return None,
            })
        }
    }

    /// 请求体：小端 u16 操作码与 u64 绝对期限（纳秒，`u64::MAX` 为无限）；相关性由 librpc 帧承载。
    pub const REQUEST_BYTES: usize = 10;
    /// 回复体：小端 u16 操作码、u16 结果；`QUERY` 成功时另附 4 字节 [`State`]。
    pub const REPLY_MAX: usize = 8;

    /// 回复结果。拒绝表示请求方不是该来源的管理端。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Reply {
        Revoked,
        State(State),
        Denied,
    }

    pub fn encode_request(op: u16, deadline: Deadline) -> Option<[u8; REQUEST_BYTES]> {
        if !matches!(op, REVOKE | QUERY) {
            return None;
        }
        let mut out = [0; REQUEST_BYTES];
        out[..2].copy_from_slice(&op.to_le_bytes());
        out[2..].copy_from_slice(&deadline.at_ns.to_le_bytes());
        Some(out)
    }

    pub fn decode_request(bytes: &[u8]) -> Option<(u16, Deadline)> {
        let bytes: &[u8; REQUEST_BYTES] = bytes.try_into().ok()?;
        let op = u16::from_le_bytes([bytes[0], bytes[1]]);
        let at = u64::from_le_bytes(bytes[2..].try_into().ok()?);
        matches!(op, REVOKE | QUERY).then_some((op, Deadline::at(at)))
    }

    pub fn encode_reply(op: u16, reply: Reply, out: &mut [u8; REPLY_MAX]) -> usize {
        out[..2].copy_from_slice(&op.to_le_bytes());
        match reply {
            Reply::Revoked => {
                out[2..4].copy_from_slice(&0u16.to_le_bytes());
                4
            }
            Reply::State(state) => {
                out[2..4].copy_from_slice(&0u16.to_le_bytes());
                out[4..8].copy_from_slice(&state.encode());
                8
            }
            Reply::Denied => {
                out[2..4].copy_from_slice(&1u16.to_le_bytes());
                4
            }
        }
    }

    /// 按原请求操作码解码回复；操作码或长度不符即拒绝。
    pub fn decode_reply(op: u16, bytes: &[u8]) -> Option<Reply> {
        if bytes.len() < 4 || u16::from_le_bytes(bytes[..2].try_into().ok()?) != op {
            return None;
        }
        match (
            u16::from_le_bytes(bytes[2..4].try_into().ok()?),
            op,
            bytes.len(),
        ) {
            (1, _, 4) => Some(Reply::Denied),
            (0, REVOKE, 4) => Some(Reply::Revoked),
            (0, QUERY, 8) => State::decode(&bytes[4..8]).map(Reply::State),
            _ => None,
        }
    }
}

/// 实例侧报告通道：发送命名导出、Ready 与 Stopped。停止请求由服务在自己的运行体中观察
/// `instance.stop`（见 [`STOP_SIGNALS`]），不经本类型。
#[cfg(target_arch = "riscv64")]
pub mod child {
    use super::*;
    use erhino_shared::{call::SystemCallError, time::Deadline};
    use libkernel::ipc::{capability::Capability, message::MailboxSender, packet::Packet};

    pub struct Reporter {
        report: MailboxSender,
        exports: u32,
    }

    impl Reporter {
        /// 由 `libprocess::birth::Birth::take` 取得的 `instance.report` 构造。
        pub fn new(report: Capability) -> Result<Self, SystemCallError> {
            let report = MailboxSender::from_capability(report)
                .map_err(|failure| failure.error)?
                .0;
            Ok(Self { report, exports: 0 })
        }

        /// 发送一个命名导出；失败时返还原能力。
        pub fn export(
            &mut self,
            name: &str,
            capability: Capability,
            rights: Rights,
            deadline: Deadline,
        ) -> Result<(), (Capability, SystemCallError)> {
            let Some(payload) = export_payload(name) else {
                return Err((capability, SystemCallError::IllegalArgument));
            };
            let mut packet = match Packet::new(EXPORT_KIND, payload) {
                Ok(packet) => packet,
                Err(error) => return Err((capability, error)),
            };
            if let Err(failure) = packet.push(capability, rights) {
                return Err((failure.capability, failure.error));
            }
            match packet.try_send(&self.report, deadline) {
                Ok(()) => {
                    self.exports += 1;
                    Ok(())
                }
                Err(mut failure) => {
                    let (capability, _) = failure
                        .packet
                        .pop()
                        .expect("unsent export keeps its capability");
                    Err((capability, failure.error))
                }
            }
        }

        /// 全部导出已发送。
        pub fn ready(&self, summary: &[u8], deadline: Deadline) -> Result<(), SystemCallError> {
            let mut bytes = [0; READY_HEADER + SUMMARY_MAX];
            let len = ready_payload(self.exports, summary, &mut bytes)
                .ok_or(SystemCallError::IllegalArgument)?;
            self.send(READY_KIND, &bytes[..len], deadline)
        }

        /// 收尾完成；随后实例以 Completed 退出。
        pub fn stopped(&self, deadline: Deadline) -> Result<(), SystemCallError> {
            self.send(STOPPED_KIND, &[], deadline)
        }

        fn send(
            &self,
            kind: u64,
            payload: &[u8],
            deadline: Deadline,
        ) -> Result<(), SystemCallError> {
            Packet::new(kind, payload)?
                .try_send(&self.report, deadline)
                .map_err(|failure| failure.error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_round_trip_and_reject_mismatches() {
        assert_eq!(
            decode_report(EXPORT_KIND, b"device.root", 1),
            Ok(Report::Export {
                name: "device.root"
            })
        );
        assert_eq!(
            decode_report(EXPORT_KIND, b"device.root", 0),
            Err(ReportError::Handles)
        );
        assert_eq!(
            decode_report(EXPORT_KIND, b"", 1),
            Err(ReportError::BadPayload)
        );
        let mut bytes = [0; 16];
        let len = ready_payload(3, b"id", &mut bytes).unwrap();
        assert_eq!(
            decode_report(READY_KIND, &bytes[..len], 0),
            Ok(Report::Ready {
                exports: 3,
                summary: b"id"
            })
        );
        assert_eq!(
            decode_report(READY_KIND, &[1, 0], 0),
            Err(ReportError::BadPayload)
        );
        assert_eq!(decode_report(STOPPED_KIND, &[], 0), Ok(Report::Stopped));
        assert_eq!(
            decode_report(STOPPED_KIND, &[0], 0),
            Err(ReportError::BadPayload)
        );
        assert_eq!(decode_report(7, &[], 0), Err(ReportError::UnknownKind));
    }

    #[test]
    fn revoke_state_round_trips() {
        use revoke::State;
        for state in [State::Open, State::Revoking, State::Closed] {
            assert_eq!(State::decode(&state.encode()), Some(state));
        }
        assert_eq!(State::decode(&3u32.to_le_bytes()), None);
        assert_eq!(State::decode(&[0; 3]), None);
    }
}
