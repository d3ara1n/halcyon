//! 通用 RPC framing（契约见 notes/ideas/rpc.md）。
//!
//! 邮箱是唯一的 RPC 传输原语，内核对请求/应答零感知。消息 payload 以
//! 固定宽、little-endian 的 [`RpcPrefix`] 起头：rpc 版本、flags 与 txid。
//! 期待回复的 request 必须把裁剪后的 send-once 回复授权放在 Handle
//! slot 0；协议层（FAL、pm、driver……）的 header 紧随前缀，不构成双层
//! 信封。消息 `kind` 字段承载协议标识，供单邮箱多路分发。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod outbound;

/// 前缀字节数。
pub const PREFIX_LEN: usize = 16;

/// 当前 framing 版本。
pub const RPC_VERSION: u16 = 1;

/// 前缀 flags：消息形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RpcMessageKind {
    Request = 1,
    Response = 2,
    Oneway = 3,
}

impl RpcMessageKind {
    pub const fn from_u16(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::Request),
            2 => Some(Self::Response),
            3 => Some(Self::Oneway),
            _ => None,
        }
    }
}

/// framing 解码错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// 缓冲短于前缀长度。
    Truncated,
    /// rpc 版本未知。
    UnknownVersion(u16),
    /// flags 不是已知的消息形态。
    UnknownKind(u16),
    /// 保留区非零（写者违约）。
    ReservedNotZero,
    /// txid 为零（wire 约束：非零、单调、不重用）。
    ZeroTxid,
}

/// 通用 RPC 前缀：request / response / oneway 的公共 framing。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcPrefix {
    pub version: u16,
    pub kind: RpcMessageKind,
    pub txid: u64,
}

impl RpcPrefix {
    pub const fn new(kind: RpcMessageKind, txid: u64) -> Self {
        Self {
            version: RPC_VERSION,
            kind,
            txid,
        }
    }

    /// 以 little-endian 编码进 `out`，返回写入字节数。
    pub fn encode(&self, out: &mut [u8]) -> usize {
        out[..2].copy_from_slice(&self.version.to_le_bytes());
        out[2..4].copy_from_slice(&(self.kind as u16).to_le_bytes());
        out[4..8].fill(0);
        out[8..16].copy_from_slice(&self.txid.to_le_bytes());
        PREFIX_LEN
    }

    /// 从 little-endian 字节解码并验证已知不变量。
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() < PREFIX_LEN {
            return Err(FrameError::Truncated);
        }
        let version = u16::from_le_bytes([bytes[0], bytes[1]]);
        if version != RPC_VERSION {
            return Err(FrameError::UnknownVersion(version));
        }
        let raw_kind = u16::from_le_bytes([bytes[2], bytes[3]]);
        let kind = RpcMessageKind::from_u16(raw_kind).ok_or(FrameError::UnknownKind(raw_kind))?;
        if bytes[4..8] != [0; 4] {
            return Err(FrameError::ReservedNotZero);
        }
        let txid = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        if txid == 0 {
            return Err(FrameError::ZeroTxid);
        }
        Ok(Self {
            version,
            kind,
            txid,
        })
    }
}

/// per-process 单调 txid 分配：非零起始、不重用，耗尽后永久失败。
static NEXT_TXID: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

pub fn next_txid() -> Option<u64> {
    NEXT_TXID.allocate()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseRejection {
    UnknownVersion,
    NotResponse,
    TxidMismatch,
    ProtocolMismatch,
}

pub fn validate_response(
    expected_protocol: u64,
    expected_txid: u64,
    protocol: u64,
    payload: &[u8],
) -> Result<RpcPrefix, ResponseRejection> {
    if protocol != expected_protocol {
        return Err(ResponseRejection::ProtocolMismatch);
    }
    let prefix = RpcPrefix::decode(payload).map_err(|_| ResponseRejection::UnknownVersion)?;
    if prefix.kind != RpcMessageKind::Response {
        return Err(ResponseRejection::NotResponse);
    }
    if prefix.txid != expected_txid {
        return Err(ResponseRejection::TxidMismatch);
    }
    Ok(prefix)
}

#[cfg(target_arch = "riscv64")]
pub mod caller;
#[cfg(target_arch = "riscv64")]
pub mod dispatcher;
/// 同步调用层依赖 ecall，仅内核目标编译；framing 核心 host 可测。
#[cfg(target_arch = "riscv64")]
pub mod exchange;
#[cfg(target_arch = "riscv64")]
pub mod outbox;
#[cfg(target_arch = "riscv64")]
pub use caller::Caller;
#[cfg(target_arch = "riscv64")]
pub use exchange::{
    CallCause, CallError, CallPhase, FrameRejection, PreparedResponse, Reply, Request,
    RequestContext,
};
#[cfg(target_arch = "riscv64")]
pub use outbox::{Outbox, OutboxResult};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_roundtrip_all_kinds() {
        let mut buffer = [0u8; PREFIX_LEN];
        for kind in [
            RpcMessageKind::Request,
            RpcMessageKind::Response,
            RpcMessageKind::Oneway,
        ] {
            let prefix = RpcPrefix::new(kind, 0x0102_0304_0506_0708);
            let len = prefix.encode(&mut buffer);
            assert_eq!(len, PREFIX_LEN);
            assert_eq!(RpcPrefix::decode(&buffer).unwrap(), prefix);
        }
    }

    #[test]
    fn decode_rejects_truncated_and_unknown() {
        assert_eq!(RpcPrefix::decode(&[0; 8]), Err(FrameError::Truncated));
        let mut buffer = [0u8; PREFIX_LEN];
        RpcPrefix::new(RpcMessageKind::Request, 1).encode(&mut buffer);
        buffer[0] = 9;
        assert_eq!(
            RpcPrefix::decode(&buffer),
            Err(FrameError::UnknownVersion(9))
        );
        buffer[0] = 1;
        buffer[2] = 7;
        assert_eq!(RpcPrefix::decode(&buffer), Err(FrameError::UnknownKind(7)));
    }

    #[test]
    fn decode_rejects_nonzero_reserved() {
        let mut buffer = [0u8; PREFIX_LEN];
        RpcPrefix::new(RpcMessageKind::Response, 2).encode(&mut buffer);
        buffer[7] = 1;
        assert_eq!(RpcPrefix::decode(&buffer), Err(FrameError::ReservedNotZero));
    }

    #[test]
    fn decode_rejects_zero_txid() {
        let mut buffer = [0u8; PREFIX_LEN];
        RpcPrefix::new(RpcMessageKind::Request, 0).encode(&mut buffer);
        assert_eq!(RpcPrefix::decode(&buffer), Err(FrameError::ZeroTxid));
    }

    #[test]
    fn txids_are_nonzero_and_monotonic() {
        let first = next_txid().unwrap();
        let second = next_txid().unwrap();
        assert!(second > first);
    }

    #[test]
    fn response_validation_classifies_every_rejection() {
        let mut response = [0u8; PREFIX_LEN];
        RpcPrefix::new(RpcMessageKind::Response, 7).encode(&mut response);
        assert!(validate_response(3, 7, 3, &response).is_ok());
        assert_eq!(
            validate_response(3, 7, 4, &response),
            Err(ResponseRejection::ProtocolMismatch)
        );
        assert_eq!(
            validate_response(3, 7, 3, &response[..8]),
            Err(ResponseRejection::UnknownVersion)
        );
        RpcPrefix::new(RpcMessageKind::Request, 7).encode(&mut response);
        assert_eq!(
            validate_response(3, 7, 3, &response),
            Err(ResponseRejection::NotResponse)
        );
        RpcPrefix::new(RpcMessageKind::Response, 8).encode(&mut response);
        assert_eq!(
            validate_response(3, 7, 3, &response),
            Err(ResponseRejection::TxidMismatch)
        );
    }
}
