use alloc::vec::Vec;

use crate::Protection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectId(u64);

impl ObjectId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableState {
    Mutable,
    Sealing,
    Executable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectError {
    ViewDenied,
    PermitDenied,
    PermitLimit,
    PermitOverflow,
    AllocationFailed,
    Busy,
    InvalidWaiter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealOutcome {
    Complete,
    Waiting,
}

/// 在对象状态锁内取得的 view 准入快照。它不持写许可；实际含 W 的 view
/// 还必须取得 [`WritePermit`]。
#[derive(Debug, PartialEq, Eq)]
pub struct ObjectViewAuthorization {
    object: ObjectId,
    maximum: Protection,
}

impl ObjectViewAuthorization {
    pub const fn object(&self) -> ObjectId {
        self.object
    }

    pub const fn maximum(&self) -> Protection {
        self.maximum
    }
}

/// 一个 reserved/published/retiring writable view 的 affine 计数凭据。
#[derive(Debug, PartialEq, Eq)]
#[must_use = "write permits must be cancelled before commit or retired after synchronization"]
pub struct WritePermit {
    object: ObjectId,
    serial: u64,
}

impl WritePermit {
    pub const fn object(&self) -> ObjectId {
        self.object
    }

    pub const fn serial(&self) -> u64 {
        self.serial
    }
}

/// MemoryObject 的纯逻辑可执行发布状态。调用方负责在对象 state lock 内访问。
#[derive(Debug)]
pub struct MemoryObjectState {
    object: ObjectId,
    state: ExecutableState,
    permits: usize,
    permit_limit: usize,
    next_serial: u64,
    seal_waiter: Option<u64>,
}

impl MemoryObjectState {
    pub const fn new(object: ObjectId, permit_limit: usize) -> Self {
        Self {
            object,
            state: ExecutableState::Mutable,
            permits: 0,
            permit_limit,
            next_serial: 1,
            seal_waiter: None,
        }
    }

    pub const fn object(&self) -> ObjectId {
        self.object
    }

    pub const fn state(&self) -> ExecutableState {
        self.state
    }

    pub const fn permit_count(&self) -> usize {
        self.permits
    }

    pub fn authorize_view(
        &self,
        maximum: Protection,
    ) -> Result<ObjectViewAuthorization, ObjectError> {
        let allowed = match self.state {
            ExecutableState::Mutable => maximum != Protection::ReadExecute,
            ExecutableState::Sealing => maximum == Protection::ReadOnly,
            ExecutableState::Executable => maximum != Protection::ReadWrite,
        };
        if !allowed {
            return Err(ObjectError::ViewDenied);
        }
        Ok(ObjectViewAuthorization {
            object: self.object,
            maximum,
        })
    }

    pub fn reserve_writes(&mut self, count: usize) -> Result<Vec<WritePermit>, ObjectError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        if self.state != ExecutableState::Mutable {
            return Err(ObjectError::PermitDenied);
        }
        let new_count = self
            .permits
            .checked_add(count)
            .ok_or(ObjectError::PermitOverflow)?;
        if new_count > self.permit_limit {
            return Err(ObjectError::PermitLimit);
        }
        let count_u64 = u64::try_from(count).map_err(|_| ObjectError::PermitOverflow)?;
        self.next_serial
            .checked_add(count_u64)
            .ok_or(ObjectError::PermitOverflow)?;

        let mut permits = Vec::new();
        permits
            .try_reserve_exact(count)
            .map_err(|_| ObjectError::AllocationFailed)?;
        for _ in 0..count {
            permits.push(WritePermit {
                object: self.object,
                serial: self.next_serial,
            });
            self.next_serial += 1;
        }
        self.permits = new_count;
        Ok(permits)
    }

    pub fn cancel_writes(&mut self, permits: Vec<WritePermit>) -> Option<u64> {
        self.release_writes(permits)
    }

    pub fn retire_writes(&mut self, permits: Vec<WritePermit>) -> Option<u64> {
        self.release_writes(permits)
    }

    pub fn seal(&mut self, waiter: Option<u64>) -> Result<SealOutcome, ObjectError> {
        if waiter == Some(0) {
            return Err(ObjectError::InvalidWaiter);
        }
        match self.state {
            ExecutableState::Mutable => {
                if self.permits == 0 {
                    self.state = ExecutableState::Executable;
                    return Ok(SealOutcome::Complete);
                }
                self.state = ExecutableState::Sealing;
                self.seal_waiter = waiter;
                Ok(SealOutcome::Waiting)
            }
            ExecutableState::Sealing => {
                if let Some(waiter) = waiter {
                    if self.seal_waiter.is_some() {
                        return Err(ObjectError::Busy);
                    }
                    self.seal_waiter = Some(waiter);
                }
                Ok(SealOutcome::Waiting)
            }
            ExecutableState::Executable => Ok(SealOutcome::Complete),
        }
    }

    pub fn abandon_waiter(&mut self, waiter: u64) -> bool {
        if self.seal_waiter == Some(waiter) {
            self.seal_waiter = None;
            true
        } else {
            false
        }
    }

    fn release_writes(&mut self, permits: Vec<WritePermit>) -> Option<u64> {
        assert!(
            permits.iter().all(|permit| permit.object == self.object),
            "write permits belong to another memory object"
        );
        assert!(
            permits.len() <= self.permits,
            "write permit accounting underflow"
        );
        self.permits -= permits.len();
        if self.state == ExecutableState::Sealing && self.permits == 0 {
            self.state = ExecutableState::Executable;
            self.seal_waiter.take()
        } else {
            None
        }
    }
}
