//! 可撤销 IO 访问。关闭 handle 不表示退休完成；硬件访问及停止顺序由驱动证明。
use crate::mm::{MappedRegion, Placement};
use core::mem::ManuallyDrop;
use erhino_shared::{
    call::SystemCallError,
    mem::MemoryProtection,
    mmio::MmioSnapshot,
    object::{Handle, Rights},
};

pub struct MmioControl {
    handle: Handle,
}
pub struct MmioUse {
    handle: Handle,
}
macro_rules! owner {
    ($ty:ident) => {
        impl $ty {
            /// # Safety
            /// handle 是本进程该实际类型的 capability，唯一关闭责任移交给返回值。
            pub const unsafe fn from_handle(handle: Handle) -> Self {
                Self { handle }
            }
            pub const fn handle(&self) -> Handle {
                self.handle
            }
            pub fn into_handle(self) -> Handle {
                ManuallyDrop::new(self).handle
            }
            pub fn query(&self) -> Result<MmioSnapshot, SystemCallError> {
                let mut snapshot = MmioSnapshot::default();
                crate::call::sys_mmio_query(self.handle, &mut snapshot)?;
                Ok(snapshot)
            }
            /// 提交撤销；应另外通过 WAIT/DONE 确认全部翻译已失效。
            pub fn revoke(&self) -> Result<(), SystemCallError> {
                crate::call::sys_mmio_revoke(self.handle)
            }
        }
        impl Drop for $ty {
            fn drop(&mut self) {
                crate::ipc::object::close_object_owner(self.handle);
            }
        }
    };
}
owner!(MmioControl);
owner!(MmioUse);
impl MmioControl {
    pub fn create_control(
        &self,
        offset: usize,
        bytes: usize,
        rights: Rights,
    ) -> Result<Self, SystemCallError> {
        crate::call::sys_mmio_create(self.handle, offset, bytes, rights, true)
            .map(|handle| Self { handle })
    }
    pub fn create_use(
        &self,
        offset: usize,
        bytes: usize,
        rights: Rights,
    ) -> Result<MmioUse, SystemCallError> {
        crate::call::sys_mmio_create(self.handle, offset, bytes, rights, false)
            .map(|handle| MmioUse { handle })
    }
    pub fn seal(&self) -> Result<(), SystemCallError> {
        crate::call::sys_mmio_seal(self.handle)
    }
}
impl MmioUse {
    pub fn map(
        &self,
        offset: usize,
        bytes: usize,
        writable: bool,
        placement: Placement,
    ) -> Result<MmioRegion<&Self>, SystemCallError> {
        let region = MappedRegion::map_source(
            self.handle,
            offset,
            bytes,
            0,
            0,
            if writable {
                MemoryProtection::ReadWrite
            } else {
                MemoryProtection::ReadOnly
            },
            placement,
        )?;
        Ok(MmioRegion {
            region,
            _usage: self,
            writable,
        })
    }

    /// 长期 actor 记录直接由映射持有 use，避免自引用；失败完整返还原 owner。
    pub fn map_owned(
        self,
        offset: usize,
        bytes: usize,
        writable: bool,
        placement: Placement,
    ) -> Result<MmioRegion<Self>, (Self, SystemCallError)> {
        let mapped = self
            .map(offset, bytes, writable, placement)
            .map(|mapping| mapping.region);
        match mapped {
            Ok(region) => Ok(MmioRegion {
                region,
                _usage: self,
                writable,
            }),
            Err(error) => Err((self, error)),
        }
    }
}

/// 保存 IO 几何及借用/拥有的 use，不产生普通 Rust 引用，不在析构中 Unmap。
/// 拥有形态析构会 Close 其 use；最后 capability 消散可能提交异步撤销，
/// 既不表示 DONE，也不证明硬件已停止；上级 control 仍可独立 Revoke。
pub struct MmioRegion<U> {
    region: MappedRegion,
    _usage: U,
    writable: bool,
}
pub struct MmioRemainder<U> {
    pub left: Option<MmioRegion<U>>,
    pub right: Option<MmioRegion<U>>,
}
impl<U> MmioRegion<U> {
    pub fn range(&self) -> core::ops::Range<usize> {
        self.region.usable().expect("MMIO mapping lost geometry")
    }
    pub fn bytes(&self) -> usize {
        self.region
            .usable()
            .expect("MMIO mapping lost geometry")
            .len()
    }
    fn address(&self, offset: usize, width: usize) -> Result<usize, SystemCallError> {
        let range = self.region.usable().expect("MMIO mapping lost geometry");
        if !offset.is_multiple_of(width)
            || offset
                .checked_add(width)
                .is_none_or(|end| end > range.len())
        {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(range.start + offset)
    }
    /// # Safety
    /// 调用者已排除父 control/use 的撤销及所有别名 Unmap，原 VA 仍属于此映射。
    /// 已撤销区域不能调用普通 Unmap，否则可能删除复用该 VA 的另一映射。
    pub unsafe fn unmap(self) -> Result<(), (Self, SystemCallError)> {
        match self.region.unmap() {
            Ok(()) => Ok(()),
            Err((region, error)) => Err((Self { region, ..self }, error)),
        }
    }
}
impl<'a> MmioRegion<&'a MmioUse> {
    /// # Safety
    /// 与 unmap 相同：调用方排除撤销/别名解除，range 仍属于本轮映射。
    pub unsafe fn unmap_range(
        self,
        range: core::ops::Range<usize>,
    ) -> Result<MmioRemainder<&'a MmioUse>, (Self, SystemCallError)> {
        let usage = self._usage;
        let writable = self.writable;
        match self.region.unmap_range(range) {
            Ok(remainder) => Ok(MmioRemainder {
                left: remainder.left.map(|region| Self {
                    region,
                    _usage: usage,
                    writable,
                }),
                right: remainder.right.map(|region| Self {
                    region,
                    _usage: usage,
                    writable,
                }),
            }),
            Err((region, error)) => Err((
                Self {
                    region,
                    _usage: usage,
                    writable,
                },
                error,
            )),
        }
    }
}
macro_rules! registers {
    ($read:ident, $write:ident, $ty:ty) => {
        impl<U> MmioRegion<U> {
            /// # Safety
            /// 区间仍是本轮 IO 映射且不会被并发撤销/重用；该寄存器支持此宽度读取。
            /// 驱动按设备规范承担访问顺序、I/O fence 和副作用，volatile 不表示硬件完成。
            pub unsafe fn $read(&self, offset: usize) -> Result<$ty, SystemCallError> {
                let address = self.address(offset, core::mem::size_of::<$ty>())?;
                // SAFETY: 调用者证明 IO 寿命/语义，本类型核验偏移、边界和对齐。
                Ok(unsafe { core::ptr::read_volatile(address as *const $ty) })
            }
            /// # Safety
            /// 同对应读取的寿命合同，且该写入值/宽度/时序符合寄存器规范。
            pub unsafe fn $write(&self, offset: usize, value: $ty) -> Result<(), SystemCallError> {
                if !self.writable {
                    return Err(SystemCallError::RightsDenied);
                }
                let address = self.address(offset, core::mem::size_of::<$ty>())?;
                // SAFETY: 已校验可写映射和范围，设备语义与关闭排除由调用者保证。
                unsafe {
                    core::ptr::write_volatile(address as *mut $ty, value);
                }
                Ok(())
            }
        }
    };
}
registers!(read_u8, write_u8, u8);
registers!(read_u16, write_u16, u16);
registers!(read_u32, write_u32, u32);
registers!(read_u64, write_u64, u64);
