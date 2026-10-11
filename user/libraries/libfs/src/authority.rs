//! 文件权利与已准入请求的真实授权使用引用；发现标记不参与授权。

use crate::{Funding, resource::FsResource, store::NodeRef};
use alloc::rc::Rc;
use core::cell::Cell;
use erhino_shared::object::Rights;
use libbudget::{AccountView, Charge};
use metadata_admission::Permit;

#[derive(Debug, Clone, Copy)]
pub struct Issuance {
    pub rights: FsRights,
    pub sender_transport: Rights,
    pub output_transport: Rights,
}
impl Issuance {
    pub fn validate_policy(self) -> Result<(), crate::protocol::Status> {
        if !self.sender_transport.is_known() || !self.output_transport.is_known() {
            return Err(crate::protocol::Status::Invalid);
        }
        if !self
            .sender_transport
            .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        {
            return Err(crate::protocol::Status::Permission);
        }
        Ok(())
    }
    pub fn validate_derived(
        self,
        parent: &AccessSnapshot,
        ceiling: FsRights,
    ) -> Result<(), crate::protocol::Status> {
        self.validate_policy()?;
        if !parent.is_active() {
            return Err(crate::protocol::Status::Revoked);
        }
        if !parent.rights().contains(FsRights::DERIVE) || !ceiling.contains(FsRights::DERIVE) {
            return Err(crate::protocol::Status::Permission);
        }
        if !self
            .rights
            .raw()
            .eq(&(self.rights & parent.rights() & ceiling).raw())
            || !self
                .sender_transport
                .is_subset_of(parent.output_transport())
            || !self
                .output_transport
                .is_subset_of(parent.output_transport())
        {
            return Err(crate::protocol::Status::Permission);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsRights(u32);
impl FsRights {
    pub const NONE: Self = Self(0);
    pub const TRAVERSE: Self = Self(1 << 0);
    pub const ENUMERATE: Self = Self(1 << 1);
    pub const READ_PROPERTY: Self = Self(1 << 2);
    pub const WRITE_PROPERTY: Self = Self(1 << 3);
    pub const READ_STREAM: Self = Self(1 << 4);
    pub const WRITE_STREAM: Self = Self(1 << 5);
    pub const CREATE: Self = Self(1 << 6);
    pub const REMOVE: Self = Self(1 << 7);
    pub const WATCH: Self = Self(1 << 8);
    pub const DESCRIBE: Self = Self(1 << 9);
    pub const DERIVE: Self = Self(1 << 10);
    pub const ALL: Self = Self((1 << 11) - 1);
    pub const fn from_raw(raw: u32) -> Option<Self> {
        if raw & !Self::ALL.0 == 0 {
            Some(Self(raw))
        } else {
            None
        }
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl core::ops::BitOr for FsRights {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl core::ops::BitAnd for FsRights {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        self.intersect(other)
    }
}

pub(crate) struct GrantLease {
    pub active: Cell<bool>,
    pub _slot: Permit,
    pub _grant_charge: Charge,
    pub _observer_charge: Charge,
    pub _storage_charge: Charge,
}

#[derive(Clone)]
pub struct AccessSnapshot {
    pub(crate) root: NodeRef,
    pub(crate) rights: FsRights,
    pub(crate) output_transport: Rights,
    pub(crate) funding: Funding,
    pub(crate) lease: Rc<GrantLease>,
}
impl AccessSnapshot {
    pub fn root(&self) -> &NodeRef {
        &self.root
    }
    pub fn rights(&self) -> FsRights {
        self.rights
    }
    pub fn output_transport(&self) -> Rights {
        self.output_transport
    }
    pub fn account(&self) -> &AccountView<FsResource> {
        self.funding.domain()
    }
    pub fn funding(&self) -> &Funding {
        &self.funding
    }
    pub fn is_active(&self) -> bool {
        self.lease.active.get()
    }
}
