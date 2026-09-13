//! 出站消息拥有全部运输能力；失败保留，成功提交后才解除本地关闭责任。

use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    message::{HandleMove, MESSAGE_HANDLE_MAX, PAYLOAD_MAX},
    object::{HandleRole, Rights},
    time::Deadline,
};
use super::capability::Capability;

#[derive(Debug)]
struct Transfer {
    capability: Capability,
    rights: Rights,
}

#[derive(Debug)]
pub struct Packet {
    kind: u64,
    payload: Vec<u8>,
    transfers: Vec<Transfer>,
    delivered: bool,
}

#[derive(Debug)]
pub struct PushFailure {
    pub error: SystemCallError,
    pub capability: Capability,
}

impl Packet {
    pub fn new(kind: u64, payload: &[u8]) -> Result<Self, SystemCallError> {
        if payload.len() > PAYLOAD_MAX { return Err(SystemCallError::IllegalArgument) }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(payload.len()).map_err(|_| SystemCallError::OutOfMemory)?;
        bytes.extend_from_slice(payload);
        let mut transfers = Vec::new();
        transfers.try_reserve_exact(MESSAGE_HANDLE_MAX).map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(Self { kind, payload: bytes, transfers, delivered: false })
    }

    pub fn kind(&self) -> u64 { self.kind }
    pub fn payload(&self) -> &[u8] { &self.payload }
    pub fn payload_mut(&mut self) -> &mut [u8] { &mut self.payload }
    pub fn truncate_payload(&mut self, len: usize) -> Result<(), SystemCallError> {
        if self.delivered || len > self.payload.len() { return Err(SystemCallError::IllegalArgument) }
        self.payload.truncate(len);
        Ok(())
    }
    pub fn handle_count(&self) -> usize { self.transfers.len() }
    pub fn delivered(&self) -> bool { self.delivered }

    fn check_transfer(&self, capability: &Capability, rights: Rights) -> Result<(), SystemCallError> {
        if self.delivered || self.transfers.len() == MESSAGE_HANDLE_MAX || !rights.is_known() {
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
        if self.delivered || self.transfers.is_empty() { return None }
        let transfer = self.transfers.remove(0);
        Some((transfer.capability, transfer.rights))
    }

    /// 调用方准备撤回尚未投递的末项时，完整取回 owner。
    pub fn pop(&mut self) -> Option<(Capability, Rights)> {
        if self.delivered { return None }
        self.transfers.pop().map(|transfer| (transfer.capability, transfer.rights))
    }

    pub fn try_send(&mut self, destination: &Capability, deadline: Deadline) -> Result<(), SystemCallError> {
        if destination.description()?.role != HandleRole::MailboxSender as u32 {
            return Err(SystemCallError::WrongObjectType);
        }
        self.publish(destination.as_handle(), deadline)
    }

    pub fn try_reply(&mut self, destination: &mut Capability, deadline: Deadline) -> Result<(), SystemCallError> {
        if destination.description()?.role != HandleRole::MailboxSenderOnce as u32 {
            return Err(SystemCallError::WrongObjectType);
        }
        self.publish(destination.as_handle(), deadline)?;
        destination.transferred();
        Ok(())
    }

    fn publish(&mut self, destination: erhino_shared::object::Handle, deadline: Deadline) -> Result<(), SystemCallError> {
        if self.delivered { return Err(SystemCallError::ObjectNotAvailable) }
        let mut moves = [HandleMove { handle: erhino_shared::object::Handle::INVALID, rights: Rights::NONE }; MESSAGE_HANDLE_MAX];
        for (item, transfer) in moves.iter_mut().zip(&self.transfers) {
            *item = HandleMove { handle: transfer.capability.as_handle(), rights: transfer.rights };
        }
        // SAFETY: Packet 独占全部源 entry，目标 role 已验证，失败不消费任何 owner。
        unsafe { crate::call::sys_send(destination, self.kind, &self.payload, &moves[..self.transfers.len()], deadline) }?;
        for transfer in &mut self.transfers { transfer.capability.transferred() }
        self.delivered = true;
        Ok(())
    }
}
