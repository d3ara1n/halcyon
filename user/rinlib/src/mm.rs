use core::ops::Range;

use erhino_shared::{
    call::SystemCallError,
    mem::{MemoryMapRequest, MemoryMapResult, MemoryPlacement, MemoryProtection},
    proc::PROCESS_PAGE_SIZE,
};

use crate::call::{sys_memory_map, sys_memory_protect, sys_memory_unmap};

static NEXT_MAP_COOKIE: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Anywhere,
    FixedEmpty { usable_start: usize },
}

/// 一段普通地址空间所有权。类型不可复制；成功 Unmap 会消费原 token。
#[derive(Debug)]
pub struct MappedRegion {
    reservation: Range<usize>,
    usable: Option<Range<usize>>,
}

#[derive(Debug)]
pub struct UnmapRemainder {
    pub left: Option<MappedRegion>,
    pub right: Option<MappedRegion>,
}

impl MappedRegion {
    pub fn map_anonymous(
        bytes: usize,
        guard_before: usize,
        guard_after: usize,
        protection: MemoryProtection,
        placement: Placement,
    ) -> Result<Self, SystemCallError> {
        Self::map(
            |result_address, cookie, address, placement_raw| {
                Ok(MemoryMapRequest::new(
                    u64::try_from(bytes).map_err(|_| SystemCallError::IllegalArgument)?,
                    u64::try_from(guard_before).map_err(|_| SystemCallError::IllegalArgument)?,
                    u64::try_from(guard_after).map_err(|_| SystemCallError::IllegalArgument)?,
                    address,
                    result_address,
                    cookie,
                    protection,
                    placement_raw,
                ))
            },
            placement,
        )
    }

    /// 映入一个 MemoryObject 的子范围。数据来自对象，不取得新的页额度；对象由
    /// view 强引用独立保活，因此建立后可以立即关闭对象 Handle。
    pub fn map_object(
        object: &crate::memory_object::MemoryObject,
        object_offset: usize,
        bytes: usize,
        guard_before: usize,
        guard_after: usize,
        protection: MemoryProtection,
        placement: Placement,
    ) -> Result<Self, SystemCallError> {
        let source = object.handle().raw();
        Self::map(
            |result_address, cookie, address, placement_raw| {
                Ok(MemoryMapRequest::new_object_view(
                    source,
                    u64::try_from(object_offset).map_err(|_| SystemCallError::IllegalArgument)?,
                    u64::try_from(bytes).map_err(|_| SystemCallError::IllegalArgument)?,
                    u64::try_from(guard_before).map_err(|_| SystemCallError::IllegalArgument)?,
                    u64::try_from(guard_after).map_err(|_| SystemCallError::IllegalArgument)?,
                    address,
                    result_address,
                    cookie,
                    protection,
                    placement_raw,
                ))
            },
            placement,
        )
    }

    /// Map 的公共提交路径：结果槽与 cookie 的生命周期、提交观察与几何校验只有一处。
    fn map(
        build: impl FnOnce(u64, u64, u64, MemoryPlacement) -> Result<MemoryMapRequest, SystemCallError>,
        placement: Placement,
    ) -> Result<Self, SystemCallError> {
        let cookie = next_cookie()?;
        let mut result = MemoryMapResult::empty();
        let (placement_raw, address) = match placement {
            Placement::Anywhere => (MemoryPlacement::Anywhere, 0),
            Placement::FixedEmpty { usable_start } => (MemoryPlacement::FixedEmpty, usable_start),
        };
        let request = build(
            u64::try_from(core::ptr::addr_of_mut!(result) as usize)
                .map_err(|_| SystemCallError::IllegalArgument)?,
            cookie,
            u64::try_from(address).map_err(|_| SystemCallError::IllegalArgument)?,
            placement_raw,
        )?;
        // SAFETY: request/result 在整个 syscall（含 Waiting）期间位于当前线程栈上；
        // result 已清零且 committed 只经 release/acquire 原子访问。
        unsafe { sys_memory_map(&request) }?;
        assert_eq!(
            result.load_committed(),
            cookie,
            "MemoryMap returned without its committed cookie"
        );
        Ok(Self::from_result(result).expect("MemoryMap committed invalid result geometry"))
    }

    pub fn reservation(&self) -> Range<usize> {
        self.reservation.clone()
    }

    pub fn usable(&self) -> Option<Range<usize>> {
        self.usable.clone()
    }

    pub fn unmap(self) -> Result<(), (Self, SystemCallError)> {
        let start = self.reservation.start;
        let bytes = self.reservation.end - start;
        // SAFETY: token 唯一持有该普通 region 的解除责任。
        match unsafe { sys_memory_unmap(start, bytes) } {
            Ok(()) => Ok(()),
            Err(error) => Err((self, error)),
        }
    }

    pub fn unmap_range(
        self,
        range: Range<usize>,
    ) -> Result<UnmapRemainder, (Self, SystemCallError)> {
        if !valid_page_range(&range)
            || range.start < self.reservation.start
            || range.end > self.reservation.end
        {
            return Err((self, SystemCallError::IllegalArgument));
        }
        let left_range =
            (self.reservation.start < range.start).then_some(self.reservation.start..range.start);
        let right_range =
            (range.end < self.reservation.end).then_some(range.end..self.reservation.end);
        // SAFETY: range 是该 affine token 的页对齐子区间。
        if let Err(error) = unsafe { sys_memory_unmap(range.start, range.end - range.start) } {
            return Err((self, error));
        }
        Ok(UnmapRemainder {
            left: left_range.map(|reservation| Self {
                usable: intersect(self.usable.as_ref(), &reservation),
                reservation,
            }),
            right: right_range.map(|reservation| Self {
                usable: intersect(self.usable.as_ref(), &reservation),
                reservation,
            }),
        })
    }

    pub fn protect(
        &self,
        range: Range<usize>,
        protection: MemoryProtection,
    ) -> Result<(), SystemCallError> {
        let Some(usable) = &self.usable else {
            return Err(SystemCallError::NotMapped);
        };
        if !valid_page_range(&range) || range.start < usable.start || range.end > usable.end {
            return Err(SystemCallError::IllegalArgument);
        }
        // SAFETY: range 已由 affine token 证明属于普通 usable mapping。
        unsafe { sys_memory_protect(range.start, range.end - range.start, protection) }
    }

    fn from_result(result: MemoryMapResult) -> Option<Self> {
        let usable_start = usize::try_from(result.usable_base).ok()?;
        let usable_bytes = usize::try_from(result.usable_bytes).ok()?;
        let reservation_start = usize::try_from(result.reservation_base).ok()?;
        let reservation_bytes = usize::try_from(result.reservation_bytes).ok()?;
        let usable_end = usable_start.checked_add(usable_bytes)?;
        let reservation_end = reservation_start.checked_add(reservation_bytes)?;
        let usable = usable_start..usable_end;
        let reservation = reservation_start..reservation_end;
        if !valid_page_range(&usable)
            || !valid_page_range(&reservation)
            || usable.start < reservation.start
            || usable.end > reservation.end
            || result.reserved != [0; 3]
        {
            return None;
        }
        Some(Self {
            reservation,
            usable: Some(usable),
        })
    }
}

fn next_cookie() -> Result<u64, SystemCallError> {
    NEXT_MAP_COOKIE
        .allocate()
        .ok_or(SystemCallError::ReachLimit)
}

fn valid_page_range(range: &Range<usize>) -> bool {
    range.start < range.end
        && range.start.is_multiple_of(PROCESS_PAGE_SIZE)
        && range.end.is_multiple_of(PROCESS_PAGE_SIZE)
}

fn intersect(usable: Option<&Range<usize>>, reservation: &Range<usize>) -> Option<Range<usize>> {
    let usable = usable?;
    let start = usable.start.max(reservation.start);
    let end = usable.end.min(reservation.end);
    (start < end).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use super::{MappedRegion, intersect};

    #[test]
    fn guard_and_mapping_fragments_split_without_losing_geometry() {
        let region = MappedRegion {
            reservation: 0x1000..0x6000,
            usable: Some(0x2000..0x5000),
        };
        assert_eq!(
            intersect(region.usable.as_ref(), &(0x1000..0x3000)),
            Some(0x2000..0x3000)
        );
        assert_eq!(intersect(region.usable.as_ref(), &(0x5000..0x6000)), None);
    }
}
