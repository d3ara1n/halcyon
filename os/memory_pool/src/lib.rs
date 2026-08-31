#![no_std]
#![forbid(unsafe_code)]

//! MemoryPool 四项守恒状态机。
//!
//! 本 crate 只拥有额度算术和线性 token，不分配物理帧、不持内核对象引用，
//! 也不决定 capability policy。内核适配层负责用 RAII 把 token 连回来源 core。

use core::{
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

pub const MAX_DEPTH: u32 = erhino_shared::memory_pool::MEMORY_POOL_MAX_DEPTH;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PoolId(NonZeroU64);

impl PoolId {
    pub const fn new(raw: u64) -> Option<Self> {
        match NonZeroU64::new(raw) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// 与诊断 PoolId 正交的 crate 内 owner key；即使调用者复用 PoolId，线性 token
/// 也不能跨状态实例提交。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnerKey(NonZeroU64);

static NEXT_OWNER_KEY: AtomicU64 = AtomicU64::new(1);

fn mint_owner_key() -> Result<OwnerKey, PoolError> {
    NEXT_OWNER_KEY
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current != 0).then(|| current.wrapping_add(1))
        })
        .ok()
        .and_then(NonZeroU64::new)
        .map(OwnerKey)
        .ok_or(PoolError::IdentityExhausted)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolError {
    ZeroAmount,
    InvalidSplit,
    QuotaExceeded,
    DepthLimit,
    ArithmeticOverflow,
    IdentityExhausted,
    InvalidTopology,
    WrongOwner,
    InvariantViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSnapshot {
    pub identity: PoolId,
    pub parent_identity: Option<PoolId>,
    pub depth: u32,
    pub total: u64,
    pub available: u64,
    pub reserved: u64,
    pub allocated: u64,
    pub delegated: u64,
}

impl PoolSnapshot {
    pub fn closes(self) -> bool {
        self.available
            .checked_add(self.reserved)
            .and_then(|value| value.checked_add(self.allocated))
            .and_then(|value| value.checked_add(self.delegated))
            == Some(self.total)
    }
}

#[derive(Debug)]
pub struct TokenError<T> {
    error: PoolError,
    token: T,
}

impl<T> TokenError<T> {
    fn new(error: PoolError, token: T) -> Self {
        Self { error, token }
    }

    pub const fn error(&self) -> PoolError {
        self.error
    }

    pub fn into_token(self) -> T {
        self.token
    }
}

#[derive(Debug)]
#[must_use = "a charge reservation must be committed or rolled back"]
pub struct ChargeReservation {
    owner_key: OwnerKey,
    owner: PoolId,
    pages: u64,
}

impl ChargeReservation {
    pub const fn pages(&self) -> u64 {
        self.pages
    }
}

#[derive(Debug)]
#[must_use = "a delegation reservation must be prepared or rolled back"]
pub struct DelegationReservation {
    parent_key: OwnerKey,
    parent: PoolId,
    child_depth: u32,
    pages: u64,
}

impl DelegationReservation {
    pub const fn parent(&self) -> PoolId {
        self.parent
    }

    pub const fn child_depth(&self) -> u32 {
        self.child_depth
    }

    pub const fn pages(&self) -> u64 {
        self.pages
    }
}

#[derive(Debug)]
#[must_use = "a prepared child must be committed into its parent or rolled back"]
pub struct PreparedChild {
    parent_key: OwnerKey,
    parent: PoolId,
    pages: u64,
    state: PoolState,
}

#[derive(Debug)]
#[must_use = "allocated credit must remain paired with backing or be returned"]
pub struct AllocatedCredit {
    owner_key: OwnerKey,
    owner: PoolId,
    pages: u64,
}

impl AllocatedCredit {
    pub const fn owner(&self) -> PoolId {
        self.owner
    }

    pub const fn pages(&self) -> u64 {
        self.pages
    }

    pub fn split(&mut self, pages: u64) -> Result<Self, PoolError> {
        if pages == 0 {
            return Err(PoolError::ZeroAmount);
        }
        if pages >= self.pages {
            return Err(PoolError::InvalidSplit);
        }
        self.pages -= pages;
        Ok(Self {
            owner_key: self.owner_key,
            owner: self.owner,
            pages,
        })
    }

    pub fn merge(&mut self, other: Self) -> Result<(), TokenError<Self>> {
        if self.owner_key != other.owner_key {
            return Err(TokenError::new(PoolError::WrongOwner, other));
        }
        let Some(pages) = self.pages.checked_add(other.pages) else {
            return Err(TokenError::new(PoolError::ArithmeticOverflow, other));
        };
        self.pages = pages;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "delegated credit can only be obtained by consuming a quiescent child"]
pub struct DelegatedCredit {
    parent_key: OwnerKey,
    parent: PoolId,
    pages: u64,
}

impl DelegatedCredit {
    pub const fn parent(&self) -> PoolId {
        self.parent
    }

    pub const fn pages(&self) -> u64 {
        self.pages
    }
}

#[derive(Debug)]
pub struct PoolState {
    owner_key: OwnerKey,
    identity: PoolId,
    parent_identity: Option<PoolId>,
    depth: u32,
    total: u64,
    available: u64,
    reserved: u64,
    allocated: u64,
    delegated: u64,
    /// child 的 parent credit 与状态不可拆分；只有消费 quiescent state 才能取回。
    parent_credit: Option<DelegatedCredit>,
}

impl PoolState {
    pub fn root(identity: PoolId, total: u64) -> Result<Self, PoolError> {
        if total == 0 {
            return Err(PoolError::ZeroAmount);
        }
        Ok(Self {
            owner_key: mint_owner_key()?,
            identity,
            parent_identity: None,
            depth: 1,
            total,
            available: total,
            reserved: 0,
            allocated: 0,
            delegated: 0,
            parent_credit: None,
        })
    }

    pub fn prepare_child(
        identity: PoolId,
        reservation: DelegationReservation,
    ) -> Result<PreparedChild, TokenError<DelegationReservation>> {
        if identity == reservation.parent
            || reservation.child_depth <= 1
            || reservation.child_depth > MAX_DEPTH
            || reservation.pages == 0
        {
            return Err(TokenError::new(PoolError::InvalidTopology, reservation));
        }
        let owner_key = match mint_owner_key() {
            Ok(owner_key) => owner_key,
            Err(error) => return Err(TokenError::new(error, reservation)),
        };
        let state = Self {
            owner_key,
            identity,
            parent_identity: Some(reservation.parent),
            depth: reservation.child_depth,
            total: reservation.pages,
            available: reservation.pages,
            reserved: 0,
            allocated: 0,
            delegated: 0,
            parent_credit: None,
        };
        Ok(PreparedChild {
            parent_key: reservation.parent_key,
            parent: reservation.parent,
            pages: reservation.pages,
            state,
        })
    }

    pub const fn identity(&self) -> PoolId {
        self.identity
    }

    pub fn snapshot(&self) -> PoolSnapshot {
        let snapshot = PoolSnapshot {
            identity: self.identity,
            parent_identity: self.parent_identity,
            depth: self.depth,
            total: self.total,
            available: self.available,
            reserved: self.reserved,
            allocated: self.allocated,
            delegated: self.delegated,
        };
        debug_assert!(snapshot.closes());
        snapshot
    }

    pub fn is_fully_available(&self) -> bool {
        self.available == self.total
            && self.reserved == 0
            && self.allocated == 0
            && self.delegated == 0
    }

    pub fn reserve_charge(&mut self, pages: u64) -> Result<ChargeReservation, PoolError> {
        self.reserve(pages)?;
        Ok(ChargeReservation {
            owner_key: self.owner_key,
            owner: self.identity,
            pages,
        })
    }

    pub fn reserve_delegation(&mut self, pages: u64) -> Result<DelegationReservation, PoolError> {
        if self.depth >= MAX_DEPTH {
            return Err(PoolError::DepthLimit);
        }
        self.reserve(pages)?;
        Ok(DelegationReservation {
            parent_key: self.owner_key,
            parent: self.identity,
            child_depth: self.depth + 1,
            pages,
        })
    }

    pub fn rollback_charge(
        &mut self,
        reservation: ChargeReservation,
    ) -> Result<(), TokenError<ChargeReservation>> {
        self.rollback(reservation.owner_key, reservation.owner, reservation.pages)
            .map_err(|error| TokenError::new(error, reservation))
    }

    pub fn rollback_delegation(
        &mut self,
        reservation: DelegationReservation,
    ) -> Result<(), TokenError<DelegationReservation>> {
        self.rollback(
            reservation.parent_key,
            reservation.parent,
            reservation.pages,
        )
        .map_err(|error| TokenError::new(error, reservation))
    }

    pub fn commit_charge(
        &mut self,
        reservation: ChargeReservation,
    ) -> Result<AllocatedCredit, TokenError<ChargeReservation>> {
        if let Err(error) = self.commit_reserved(
            reservation.owner_key,
            reservation.owner,
            reservation.pages,
            Account::Allocated,
        ) {
            return Err(TokenError::new(error, reservation));
        }
        Ok(AllocatedCredit {
            owner_key: self.owner_key,
            owner: self.identity,
            pages: reservation.pages,
        })
    }

    pub fn commit_child(
        &mut self,
        mut prepared: PreparedChild,
    ) -> Result<PoolState, TokenError<PreparedChild>> {
        if let Err(error) = self.commit_reserved(
            prepared.parent_key,
            prepared.parent,
            prepared.pages,
            Account::Delegated,
        ) {
            return Err(TokenError::new(error, prepared));
        }
        prepared.state.parent_credit = Some(DelegatedCredit {
            parent_key: self.owner_key,
            parent: self.identity,
            pages: prepared.pages,
        });
        Ok(prepared.state)
    }

    pub fn rollback_child(
        &mut self,
        prepared: PreparedChild,
    ) -> Result<(), TokenError<PreparedChild>> {
        self.rollback(prepared.parent_key, prepared.parent, prepared.pages)
            .map_err(|error| TokenError::new(error, prepared))
    }

    /// 消费已完全归零的状态，取出与 child 同寿命的 parent credit。成功后不再
    /// 存在可继续使用的 child state，因此额度不能提前归父后重放。
    pub fn into_parent_credit(self) -> Result<Option<DelegatedCredit>, TokenError<Self>> {
        if !self.is_fully_available() {
            return Err(TokenError::new(PoolError::InvariantViolation, self));
        }
        let topology_valid = match (self.parent_identity, self.parent_credit.as_ref()) {
            (None, None) => true,
            (Some(parent), Some(credit)) => credit.parent == parent && credit.pages == self.total,
            (None, Some(_)) | (Some(_), None) => false,
        };
        if !topology_valid {
            return Err(TokenError::new(PoolError::InvariantViolation, self));
        }
        Ok(self.parent_credit)
    }

    pub fn return_charge(
        &mut self,
        credit: AllocatedCredit,
    ) -> Result<(), TokenError<AllocatedCredit>> {
        self.return_credit(
            credit.owner_key,
            credit.owner,
            credit.pages,
            Account::Allocated,
        )
        .map_err(|error| TokenError::new(error, credit))
    }

    pub fn return_delegation(
        &mut self,
        credit: DelegatedCredit,
    ) -> Result<(), TokenError<DelegatedCredit>> {
        self.return_credit(
            credit.parent_key,
            credit.parent,
            credit.pages,
            Account::Delegated,
        )
        .map_err(|error| TokenError::new(error, credit))
    }

    fn reserve(&mut self, pages: u64) -> Result<(), PoolError> {
        if pages == 0 {
            return Err(PoolError::ZeroAmount);
        }
        if self.available < pages {
            return Err(PoolError::QuotaExceeded);
        }
        let reserved = self
            .reserved
            .checked_add(pages)
            .ok_or(PoolError::ArithmeticOverflow)?;
        self.available -= pages;
        self.reserved = reserved;
        Ok(())
    }

    fn rollback(
        &mut self,
        owner_key: OwnerKey,
        owner: PoolId,
        pages: u64,
    ) -> Result<(), PoolError> {
        self.validate_credit(owner_key, owner, pages)?;
        if self.reserved < pages {
            return Err(PoolError::InvariantViolation);
        }
        let available = self
            .available
            .checked_add(pages)
            .ok_or(PoolError::ArithmeticOverflow)?;
        if available > self.total {
            return Err(PoolError::InvariantViolation);
        }
        self.reserved -= pages;
        self.available = available;
        Ok(())
    }

    fn commit_reserved(
        &mut self,
        owner_key: OwnerKey,
        owner: PoolId,
        pages: u64,
        account: Account,
    ) -> Result<(), PoolError> {
        self.validate_credit(owner_key, owner, pages)?;
        if self.reserved < pages {
            return Err(PoolError::InvariantViolation);
        }
        let destination = match account {
            Account::Allocated => &mut self.allocated,
            Account::Delegated => &mut self.delegated,
        };
        let value = destination
            .checked_add(pages)
            .ok_or(PoolError::ArithmeticOverflow)?;
        self.reserved -= pages;
        *destination = value;
        Ok(())
    }

    fn return_credit(
        &mut self,
        owner_key: OwnerKey,
        owner: PoolId,
        pages: u64,
        account: Account,
    ) -> Result<(), PoolError> {
        self.validate_credit(owner_key, owner, pages)?;
        let source = match account {
            Account::Allocated => &mut self.allocated,
            Account::Delegated => &mut self.delegated,
        };
        if *source < pages {
            return Err(PoolError::InvariantViolation);
        }
        let available = self
            .available
            .checked_add(pages)
            .ok_or(PoolError::ArithmeticOverflow)?;
        if available > self.total {
            return Err(PoolError::InvariantViolation);
        }
        *source -= pages;
        self.available = available;
        Ok(())
    }

    fn validate_credit(
        &self,
        owner_key: OwnerKey,
        owner: PoolId,
        pages: u64,
    ) -> Result<(), PoolError> {
        if owner_key != self.owner_key || owner != self.identity {
            return Err(PoolError::WrongOwner);
        }
        if pages == 0 {
            return Err(PoolError::ZeroAmount);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Account {
    Allocated,
    Delegated,
}
