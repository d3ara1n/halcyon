//! FAL 业务权限独立于内核运输 rights；授权快照固定已准入请求的操作上限。

use crate::resource::FalResource;
use crate::store::NodeRef;
use erhino_shared::object::Rights;
use libbudget::AccountView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FalRights(u32);

impl FalRights {
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
    pub const ACQUIRE_CAPABILITY: Self = Self(1 << 9);
    pub const ALL: Self = Self((1 << 10) - 1);

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
    pub const fn contains(self, rights: Self) -> bool {
        self.0 & rights.0 == rights.0
    }
    pub const fn intersect(self, ceiling: Self) -> Self {
        Self(self.0 & ceiling.0)
    }
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::ops::BitOr for FalRights {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl core::ops::BitAnd for FalRights {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        self.intersect(other)
    }
}

#[derive(Clone)]
pub struct AccessSnapshot {
    pub(crate) root: NodeRef,
    pub(crate) rights: FalRights,
    pub(crate) output_transport: Rights,
    pub(crate) account: AccountView<FalResource>,
}

impl AccessSnapshot {
    pub fn root(&self) -> &NodeRef {
        &self.root
    }
    pub fn rights(&self) -> FalRights {
        self.rights
    }
    pub fn output_transport(&self) -> Rights {
        self.output_transport
    }

    pub fn account(&self) -> &AccountView<FalResource> {
        &self.account
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn new_for_test(
        root: NodeRef,
        rights: FalRights,
        output_transport: Rights,
        account: AccountView<FalResource>,
    ) -> Self {
        Self {
            root,
            rights,
            output_transport,
            account,
        }
    }
}
