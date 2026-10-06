//! 接入授权的 affine owner；Attach 与内存映射分别提交。

use super::{capability::Capability, tunnel::Endpoint};
use erhino_shared::{call::SystemCallError, object::Handle};

#[derive(Debug)]
#[must_use = "an Invitation must be transferred, attached, or closed"]
pub struct Invitation {
    handle: Option<Handle>,
}
#[derive(Debug)]
pub enum CreateFailure {
    System(SystemCallError),
    Published {
        error: SystemCallError,
        endpoint: Option<Endpoint>,
        invitation: Option<Invitation>,
    },
}
#[derive(Debug)]
pub struct InvitationFailure {
    pub owner: Capability,
    pub error: SystemCallError,
}
#[derive(Debug)]
pub struct AttachFailure {
    pub error: SystemCallError,
    pub invitation: Invitation,
}
impl Invitation {
    pub fn create(pool: Handle, bytes: usize) -> Result<(Endpoint, Self, usize), CreateFailure> {
        super::tunnel::create_owned(pool, bytes)
            .map(|(endpoint, invitation, bytes)| {
                (
                    endpoint,
                    Self {
                        handle: Some(invitation),
                    },
                    bytes,
                )
            })
            .map_err(|failure| {
                if failure.endpoint.is_none() && failure.invitation.is_none() {
                    CreateFailure::System(failure.error)
                } else {
                    CreateFailure::Published {
                        error: failure.error,
                        endpoint: failure.endpoint,
                        invitation: failure.invitation.map(|handle| Self {
                            handle: Some(handle),
                        }),
                    }
                }
            })
    }
    pub fn from_capability(capability: Capability) -> Result<Self, InvitationFailure> {
        match capability.description() {
            Ok(description)
                if description.kind
                    == erhino_shared::object::ObjectKind::TunnelInvitation as u32 => {}
            Ok(_) => {
                return Err(InvitationFailure {
                    owner: capability,
                    error: SystemCallError::WrongObjectType,
                });
            }
            Err(error) => {
                return Err(InvitationFailure {
                    owner: capability,
                    error,
                });
            }
        }
        let raw = capability.into_raw();
        Ok(Self { handle: Some(raw) })
    }
    pub fn into_capability(mut self) -> Capability {
        let raw = self.handle.take().expect("Invitation already consumed");
        // SAFETY: 通过 self 消费表项唯一 owner。
        unsafe { Capability::from_raw(raw) }
    }
    pub fn attach(mut self) -> Result<Endpoint, AttachFailure> {
        // SAFETY: 本 owner 独占接入权；成功后从自己移走旧句柄。
        match unsafe { super::tunnel::attach(self.handle.expect("Invitation already consumed")) } {
            Ok(endpoint) => {
                self.handle = None;
                Ok(endpoint)
            }
            Err(error) => Err(AttachFailure {
                error,
                invitation: self,
            }),
        }
    }
    pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
        // SAFETY: self 唯一拥有接入权，关闭不操作另一侧的 View。
        match unsafe { super::object::close(self.handle.expect("Invitation already consumed")) } {
            Ok(()) => {
                self.handle = None;
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }
    /// # Safety
    /// 不得消费或重新拥有返回的借用句柄。
    pub unsafe fn raw_handle(&self) -> Handle {
        self.handle.expect("Invitation already consumed")
    }
}
impl Drop for Invitation {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: 未移交邀请的唯一 owner 释放该表项。
            let _ = unsafe { super::object::close(handle) };
        }
    }
}
