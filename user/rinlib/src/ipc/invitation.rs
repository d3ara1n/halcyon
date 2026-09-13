//! affine Invitation：提交失败保留邀请，成功后只能持有 Endpoint/清理责任。

use super::{capability::Capability, tunnel::Endpoint};
use crate::mm::Placement;
use erhino_shared::{
    call::SystemCallError,
    object::HandleRole,
    tunnel::{TunnelAttachRequest, TunnelEndpointResult},
};

#[derive(Debug)]
pub struct Invitation {
    owner: Capability,
}

#[derive(Debug)]
pub struct InvitationFailure {
    pub owner: Capability,
    pub error: SystemCallError,
}

#[derive(Debug)]
pub enum AttachFailure {
    Unconsumed {
        invitation: Invitation,
        error: SystemCallError,
    },
    Consumed {
        cleanup: Option<super::tunnel::EndpointCleanup>,
        error: SystemCallError,
    },
}

impl Invitation {
    pub fn create(bytes: usize, placement: Placement) -> Result<(Endpoint, Self), SystemCallError> {
        let (endpoint, handle) = super::tunnel::create(bytes, placement)?;
        Ok((
            endpoint,
            Self {
                owner: Capability::owned(handle),
            },
        ))
    }

    pub fn from_capability(owner: Capability) -> Result<Self, InvitationFailure> {
        let checked = owner.description().and_then(|description| {
            if description.role != HandleRole::TunnelInvitation as u32 {
                return Err(SystemCallError::WrongObjectType);
            }
            Ok(())
        });
        match checked {
            Ok(()) => Ok(Self { owner }),
            Err(error) => Err(InvitationFailure { owner, error }),
        }
    }
    pub fn into_capability(self) -> Capability {
        self.owner
    }
    pub fn attach(self, policy: Placement) -> Result<Endpoint, AttachFailure> {
        let mut output = TunnelEndpointResult::empty();
        let (address, policy) = super::tunnel::placement(policy);
        let request = TunnelAttachRequest::new(
            self.owner.as_handle(),
            address,
            core::ptr::addr_of_mut!(output) as u64,
            policy,
        );
        // SAFETY: owner 独占 Invitation，输入输出在包括内核挂起的调用期间有效。
        if let Err(error) = unsafe { crate::call::sys_tunnel_attach(&request) } {
            return Err(AttachFailure::Unconsumed {
                invitation: self,
                error,
            });
        }
        let mut owner = self.owner;
        owner.transferred();
        Endpoint::from_result_owned(output)
            .map_err(|(cleanup, error)| AttachFailure::Consumed { cleanup, error })
    }
}
