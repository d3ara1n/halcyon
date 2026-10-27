//! 启动来源的不可变程序映像；字节与有效长度配对，不通过可变路径重新取得。
use erhino_shared::{
    call::SystemCallError,
    mem::MemoryProtection,
    memory_object::MemoryObjectState,
    object::{Handle, ObjectKind, ObjectSignals, Rights},
    proc::PROCESS_PAGE_SIZE,
};
use libkernel::{
    ipc::{capability::Capability, object, wait},
    memory_object::MemoryObject,
    mm::{MappedRegion, Placement},
};

pub const IMAGE_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
pub struct Image {
    object: MemoryObject,
    mapping: Option<MappedRegion>,
    length: usize,
}
impl Image {
    pub fn copy(bytes: &[u8]) -> Result<Self, SystemCallError> {
        if bytes.is_empty() {
            return Err(SystemCallError::IllegalArgument);
        }
        let size = bytes
            .len()
            .checked_next_multiple_of(PROCESS_PAGE_SIZE)
            .ok_or(SystemCallError::IllegalArgument)?;
        let object = MemoryObject::create(size)?;
        let mapping = MappedRegion::map_object(
            &object,
            0,
            size,
            0,
            0,
            MemoryProtection::ReadWrite,
            Placement::Anywhere,
        )?;
        let start = mapping.usable().expect("image mapping missing").start;
        // SAFETY: 新对象尚无导出、DMA 或并发访问者，独占可写映射覆盖 size；尾页由内核清零。
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), start as *mut u8, bytes.len());
        }
        mapping.unmap().map_err(|(_, error)| error)?;
        object.seal()?;
        wait::wait_until(
            object.handle(),
            ObjectSignals::EXECUTABLE,
            erhino_shared::time::Deadline::INFINITE,
        )?;
        let handle = object::duplicate(object.handle(), IMAGE_RIGHTS)?;
        // SAFETY: duplicate 返回本进程独占的新 entry，移入只读采用路径。
        Self::adopt(unsafe { Capability::from_raw(handle) }, bytes.len())
    }
    pub fn adopt(capability: Capability, length: usize) -> Result<Self, SystemCallError> {
        let info = capability.description()?;
        if info.kind != ObjectKind::MemoryObject as u32
            || info.rights != IMAGE_RIGHTS
            || length == 0
        {
            return Err(SystemCallError::RightsDenied);
        }
        // SAFETY: 实际类型已核，唯一关闭责任移入。
        let object = unsafe { MemoryObject::from_handle(capability.into_raw()) };
        let snapshot = object.query()?;
        let size = usize::try_from(snapshot.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
        if snapshot.state != MemoryObjectState::Executable as u32
            || length > size
            || size > isize::MAX as usize
        {
            return Err(SystemCallError::IllegalArgument);
        }
        let mapping = MappedRegion::map_object(
            &object,
            0,
            size,
            0,
            0,
            MemoryProtection::ReadOnly,
            Placement::Anywhere,
        )?;
        Ok(Self {
            object,
            mapping: Some(mapping),
            length,
        })
    }
    pub fn handle(&self) -> Handle {
        self.object.handle()
    }
    pub fn bytes(&self) -> &[u8] {
        let start = self
            .mapping
            .as_ref()
            .expect("image mapping missing")
            .usable()
            .expect("image mapping lost range")
            .start;
        // SAFETY: Executable 是不可逆的写许可封闭终态；只读映射在本借用期间保活，没有 DMA 写者。
        unsafe { core::slice::from_raw_parts(start as *const u8, self.length) }
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        if let Some(mapping) = self.mapping.take() {
            let _ = mapping.unmap();
        }
    }
}
