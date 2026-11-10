//! 服务实例的通用撤销协议；领域拥有实际活跃使用与退休责任。

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

#[cfg(test)]
mod tests {
    use super::*;
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
