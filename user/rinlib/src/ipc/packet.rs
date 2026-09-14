//! 出站消息拥有全部运输能力；失败完整返还，成功提交即消费 owner。

use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    message::{HandleMove, MESSAGE_HANDLE_MAX, PAYLOAD_MAX},
    object::{Handle, Rights},
    time::Deadline,
};
use super::capability::Capability;
use super::message::{MailboxSender, SendOnce};

#[derive(Debug)]
struct Transfer {
    capability: Capability,
    rights: Rights,
}

#[derive(Debug, Default)]
pub struct Packet {
    kind: u64,
    payload: Vec<u8>,
    transfers: Vec<Transfer>,
}

#[derive(Debug)]
pub struct PushFailure {
    pub error: SystemCallError,
    pub capability: Capability,
}

/// try_send 失败完整返还 Packet；修正或等待后可重试，或拆解回收全部 owner。
#[derive(Debug)]
pub struct SendFailure {
    pub error: SystemCallError,
    pub packet: Packet,
}

/// try_reply 失败完整返还 Packet 与 send-once 回复授权，二者均未被消费。
#[derive(Debug)]
pub struct ReplyFailure {
    pub error: SystemCallError,
    pub packet: Packet,
    pub reply: SendOnce,
}

impl Packet {
    pub fn new(kind: u64, payload: &[u8]) -> Result<Self, SystemCallError> {
        if payload.len() > PAYLOAD_MAX { return Err(SystemCallError::IllegalArgument) }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(payload.len()).map_err(|_| SystemCallError::OutOfMemory)?;
        bytes.extend_from_slice(payload);
        let mut transfers = Vec::new();
        transfers.try_reserve_exact(MESSAGE_HANDLE_MAX).map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(Self { kind, payload: bytes, transfers })
    }

    pub fn kind(&self) -> u64 { self.kind }
    pub fn payload(&self) -> &[u8] { &self.payload }
    pub fn payload_mut(&mut self) -> &mut [u8] { &mut self.payload }
    pub fn truncate_payload(&mut self, len: usize) -> Result<(), SystemCallError> {
        if len > self.payload.len() { return Err(SystemCallError::IllegalArgument) }
        self.payload.truncate(len);
        Ok(())
    }
    pub fn handle_count(&self) -> usize { self.transfers.len() }

    fn check_transfer(&self, capability: &Capability, rights: Rights) -> Result<(), SystemCallError> {
        if self.transfers.len() == MESSAGE_HANDLE_MAX || !rights.is_known() {
            return Err(SystemCallError::IllegalArgument);
        }
        let source = capability.description()?;
        if !source.rights.contains(Rights::TRANSIT) || !rights.is_subset_of(source.rights) {
            return Err(SystemCallError::RightsDenied);
        }
        Ok(())
    }

    pub fn push(&mut self, capability: Capability, rights: Rights) -> Result<(), PushFailure> {
        if let Err(error) = self.check_transfer(&capability, rights) {
            return Err(PushFailure { error, capability });
        }
        self.transfers.push(Transfer { capability, rights });
        Ok(())
    }

    pub fn push_front(&mut self, capability: Capability, rights: Rights) -> Result<(), PushFailure> {
        if let Err(error) = self.check_transfer(&capability, rights) {
            return Err(PushFailure { error, capability });
        }
        self.transfers.insert(0, Transfer { capability, rights });
        Ok(())
    }

    pub fn pop_front(&mut self) -> Option<(Capability, Rights)> {
        if self.transfers.is_empty() { return None }
        let transfer = self.transfers.remove(0);
        Some((transfer.capability, transfer.rights))
    }

    /// 调用方准备撤回尚未投递的末项时，完整取回 owner。
    pub fn pop(&mut self) -> Option<(Capability, Rights)> {
        self.transfers.pop().map(|transfer| (transfer.capability, transfer.rights))
    }

    /// 消费式投递：成功即消费 Packet 与全部 transit owner，失败完整返还。
    /// 目标 role 由 MailboxSender 类型保证，重试路径不再重复 Query。
    pub fn try_send(self, destination: &MailboxSender, deadline: Deadline) -> Result<(), SendFailure> {
        match self.publish(destination.as_handle(), deadline) {
            Ok(()) => Ok(()),
            Err((packet, error)) => Err(SendFailure { error, packet }),
        }
    }

    /// 消费式回复：成功即消费 Packet 与 send-once 授权，失败二者完整返还。
    pub fn try_reply(self, reply: SendOnce, deadline: Deadline) -> Result<(), ReplyFailure> {
        let mut once = reply;
        match self.publish(once.as_handle(), deadline) {
            Ok(()) => {
                once.transferred();
                Ok(())
            }
            Err((packet, error)) => Err(ReplyFailure { error, packet, reply: once }),
        }
    }

    fn publish(mut self, destination: Handle, deadline: Deadline) -> Result<(), (Packet, SystemCallError)> {
        let mut moves = [HandleMove { handle: Handle::INVALID, rights: Rights::NONE }; MESSAGE_HANDLE_MAX];
        for (item, transfer) in moves.iter_mut().zip(&self.transfers) {
            *item = HandleMove { handle: transfer.capability.as_handle(), rights: transfer.rights };
        }
        // SAFETY: Packet 独占全部源 entry，目标 role 已由 typed owner 保证，
        // 失败不消费任何 owner，成功即完整移交运输责任。
        let sent = unsafe {
            crate::call::sys_send(destination, self.kind, &self.payload, &moves[..self.transfers.len()], deadline)
        };
        match sent {
            Ok(()) => {
                for transfer in &mut self.transfers { transfer.capability.transferred() }
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_limit_is_checked_before_construction() {
        assert!(Packet::new(1, &alloc::vec![0; PAYLOAD_MAX]).is_ok());
        assert_eq!(
            Packet::new(1, &alloc::vec![0; PAYLOAD_MAX + 1]).unwrap_err(),
            SystemCallError::IllegalArgument
        );
    }

    #[test]
    fn truncate_payload_rejects_growth() {
        let mut packet = Packet::new(7, &[1, 2, 3]).unwrap();
        packet.truncate_payload(2).unwrap();
        assert_eq!(packet.payload(), &[1, 2]);
        assert_eq!(
            packet.truncate_payload(3).unwrap_err(),
            SystemCallError::IllegalArgument
        );
    }

    #[test]
    fn placeholder_packet_has_nothing_to_reclaim() {
        // 消费式投递的中转占位：没有任何可再取回的 owner。
        let mut packet = Packet::default();
        assert_eq!(packet.handle_count(), 0);
        assert!(packet.pop().is_none());
        assert!(packet.pop_front().is_none());
    }
}
