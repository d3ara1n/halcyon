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
}

/// `seal` 的结果。状态机不保存等待者：完成事实以 `Published` 报告一次，
/// 调用方据此发布对象的 `EXECUTABLE` 电平，等待复用通用等待面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealOutcome {
    /// 本次调用把对象推进到 Executable（含已在终态的幂等成功）。
    Published,
    /// 仍有在途可写 view；最后一个 permit 退役时由 `retire_*` 报告完成。
    Pending,
}

/// 在对象状态锁内取得的 view 准入快照。它不持写许可；实际含 W 的 view
/// 还必须取得 [`WritePermit`]。
#[derive(Debug, PartialEq, Eq)]
pub struct ObjectViewAuthorization {
    object: ObjectId,
    maximum: Protection,
    object_bytes: usize,
}

impl ObjectViewAuthorization {
    pub const fn object(&self) -> ObjectId {
        self.object
    }

    pub const fn maximum(&self) -> Protection {
        self.maximum
    }

    /// 对象的固定长度。view 越界以此为准，调用方不另传一份长度。
    pub const fn object_bytes(&self) -> usize {
        self.object_bytes
    }

    /// 校验 view 区间落在对象几何内并返回其页数。offset 与 length 都必须页对齐；
    /// 越界以对象自身长度为准，不接受调用方另传的长度。
    pub const fn view_pages(&self, offset: usize, bytes: usize, page_size: usize) -> Option<usize> {
        if bytes == 0 || !offset.is_multiple_of(page_size) || !bytes.is_multiple_of(page_size) {
            return None;
        }
        match offset.checked_add(bytes) {
            Some(end) if end <= self.object_bytes => Some(bytes / page_size),
            _ => None,
        }
    }
}

/// 一个 reserved/published/retiring writable view 的 affine 计数凭据。
#[derive(Debug, PartialEq, Eq)]
#[must_use = "write permits must be cancelled before commit or retired after synchronization"]
pub struct WritePermit {
    object: ObjectId,
}

impl WritePermit {
    pub const fn object(&self) -> ObjectId {
        self.object
    }
}

/// MemoryObject 的纯逻辑可执行发布状态。调用方负责在对象 state lock 内访问。
#[derive(Debug)]
pub struct MemoryObjectState {
    object: ObjectId,
    object_bytes: usize,
    state: ExecutableState,
    permits: usize,
    permit_limit: usize,
}

impl MemoryObjectState {
    pub const fn new(object: ObjectId, object_bytes: usize, permit_limit: usize) -> Self {
        Self {
            object,
            object_bytes,
            state: ExecutableState::Mutable,
            permits: 0,
            permit_limit,
        }
    }

    pub const fn object(&self) -> ObjectId {
        self.object
    }

    /// 对象固定长度；创建后不可改变。
    pub const fn object_bytes(&self) -> usize {
        self.object_bytes
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
            object_bytes: self.object_bytes,
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

        let mut permits = Vec::new();
        permits
            .try_reserve_exact(count)
            .map_err(|_| ObjectError::AllocationFailed)?;
        for _ in 0..count {
            permits.push(WritePermit {
                object: self.object,
            });
        }
        self.permits = new_count;
        Ok(permits)
    }

    /// 放弃尚未提交的写许可。返回 true 表示本次释放使 Sealing 完成。
    pub fn cancel_write(&mut self, permit: WritePermit) -> bool {
        self.release_one(permit)
    }

    /// 同步确认后退役写许可。返回 true 表示本次退役使 Sealing 完成。
    pub fn retire_write(&mut self, permit: WritePermit) -> bool {
        self.release_one(permit)
    }

    /// 单向请求可执行发布。permit 为零时同点进入 Executable；否则转 Sealing 并
    /// 拒绝新写入口，由最后一个 permit 的退役完成推进。已在终态时幂等成功。
    pub fn seal(&mut self) -> SealOutcome {
        match self.state {
            ExecutableState::Mutable => {
                if self.permits == 0 {
                    self.state = ExecutableState::Executable;
                    return SealOutcome::Published;
                }
                self.state = ExecutableState::Sealing;
                SealOutcome::Pending
            }
            ExecutableState::Sealing => SealOutcome::Pending,
            ExecutableState::Executable => SealOutcome::Published,
        }
    }

    fn release_one(&mut self, permit: WritePermit) -> bool {
        assert_eq!(
            permit.object, self.object,
            "write permit belongs to another memory object"
        );
        assert!(self.permits != 0, "write permit accounting underflow");
        self.permits -= 1;
        self.publish_if_quiescent()
    }

    /// Sealing 下最后一个 permit 消失即单向推进到 Executable。
    fn publish_if_quiescent(&mut self) -> bool {
        if self.state == ExecutableState::Sealing && self.permits == 0 {
            self.state = ExecutableState::Executable;
            true
        } else {
            false
        }
    }
}
