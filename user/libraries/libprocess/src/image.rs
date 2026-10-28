//! 启动来源的不可变程序映像；构造、封口与只读借用分别拥有自己的寿命。

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
    length: usize,
}

/// 尚未发布的映像构造。每次 advance 只复制一页或执行一个封口操作；
/// EXECUTABLE 等待由宿主登记，构造器不建立运行体或阻塞等待。
#[must_use = "image construction must be advanced or cancelled"]
pub struct ImageCopy<'a> {
    bytes: &'a [u8],
    object: Option<MemoryObject>,
    mapping: Option<MappedRegion>,
    offset: usize,
    sealed: bool,
    finished: bool,
}
impl<'a> ImageCopy<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, SystemCallError> {
        if bytes.is_empty()
            || bytes
                .len()
                .checked_next_multiple_of(PROCESS_PAGE_SIZE)
                .is_none()
        {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Self {
            bytes,
            object: None,
            mapping: None,
            offset: 0,
            sealed: false,
            finished: false,
        })
    }
    /// Some 表示唯一完成结果；None 时由 waiting 区分本地可推进与真实封口等待。
    pub fn advance(&mut self) -> Result<Option<Image>, SystemCallError> {
        if self.finished {
            return Err(SystemCallError::ObjectClosed);
        }
        if self.object.is_none() {
            self.object = Some(MemoryObject::create(
                self.bytes.len().next_multiple_of(PROCESS_PAGE_SIZE),
            )?);
            return Ok(None);
        }
        let object = self
            .object
            .as_ref()
            .expect("image construction owns object");
        if self.offset < self.bytes.len() {
            if self.mapping.is_none() {
                self.mapping = Some(MappedRegion::map_object(
                    object,
                    self.offset,
                    PROCESS_PAGE_SIZE,
                    0,
                    0,
                    MemoryProtection::ReadWrite,
                    Placement::Anywhere,
                )?);
                let start = self
                    .mapping
                    .as_ref()
                    .expect("image page mapped")
                    .usable()
                    .expect("image mapping missing")
                    .start;
                let length = (self.bytes.len() - self.offset).min(PROCESS_PAGE_SIZE);
                // SAFETY: 此未导出对象的唯一可写页无并发访问者；来源借用固定且不重叠，尾页由内核清零。
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.bytes.as_ptr().add(self.offset),
                        start as *mut u8,
                        length,
                    );
                }
            }
            unmap(&mut self.mapping)?;
            self.offset += PROCESS_PAGE_SIZE;
            return Ok(None);
        }
        if !self.sealed {
            object.seal()?;
            self.sealed = true;
            return Ok(None);
        }
        if object.query()?.state != MemoryObjectState::Executable as u32 {
            return Ok(None);
        }
        let handle = object::duplicate(object.handle(), IMAGE_RIGHTS)?;
        // SAFETY: duplicate 返回本进程唯一的新 entry；这里只缩权移交，原构造能力随后关闭。
        let image = Image {
            object: unsafe { MemoryObject::from_handle(handle) },
            length: self.bytes.len(),
        };
        self.object.take();
        self.finished = true;
        Ok(Some(image))
    }
    pub fn waiting(&self) -> Option<Handle> {
        self.object
            .as_ref()
            .filter(|_| self.sealed)
            .map(MemoryObject::handle)
    }
    /// 取消未发布构造；失败保留原页映射和对象，调用者可继续或升级其进程终局。
    pub fn cancel(&mut self) -> Result<(), SystemCallError> {
        unmap(&mut self.mapping)?;
        self.object.take();
        self.finished = true;
        Ok(())
    }
}
impl Drop for ImageCopy<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.cancel() {
            // 无宿主接管时终止当前进程，由VM退休承担剩余映射；不能静默泄漏写许可。
            panic!("image construction cleanup failed: {:?}", error);
        }
    }
}
impl Image {
    pub fn copy(bytes: &[u8]) -> Result<Self, SystemCallError> {
        let mut copying = ImageCopy::new(bytes)?;
        loop {
            if let Some(image) = copying.advance()? {
                return Ok(image);
            }
            if let Some(handle) = copying.waiting() {
                wait::wait_until(
                    handle,
                    ObjectSignals::EXECUTABLE,
                    erhino_shared::time::Deadline::INFINITE,
                )?;
            }
        }
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
        Ok(Self { object, length })
    }
    /// 映像读取方按需要持有只读映射；构造方和能力转交者无需映入完整映像。
    pub fn map(&self) -> Result<ImageView, SystemCallError> {
        let mapping = MappedRegion::map_object(
            &self.object,
            0,
            self.length.next_multiple_of(PROCESS_PAGE_SIZE),
            0,
            0,
            MemoryProtection::ReadOnly,
            Placement::Anywhere,
        )?;
        Ok(ImageView {
            length: self.length,
            mapping: Some(mapping),
        })
    }
    pub fn handle(&self) -> Handle {
        self.object.handle()
    }
}

pub struct ImageView {
    length: usize,
    mapping: Option<MappedRegion>,
}
impl ImageView {
    /// 真实解除失败仍保留view；重复close接续同一映射。
    pub fn close(&mut self) -> Result<(), SystemCallError> {
        unmap(&mut self.mapping)
    }
    /// 借用当前映射；只能在 close 成功前调用。
    ///
    /// # Panics
    ///
    /// view 已成功关闭时没有可借用映射。
    pub fn bytes(&self) -> &[u8] {
        let start = self
            .mapping
            .as_ref()
            .expect("image mapping missing")
            .usable()
            .expect("image mapping lost range")
            .start;
        // SAFETY: Executable 是不可逆的写许可封闭终态；只读映射在本借用期间保活，没有DMA写者。
        unsafe { core::slice::from_raw_parts(start as *const u8, self.length) }
    }
}
impl Drop for ImageView {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            panic!("image view cleanup failed: {:?}", error);
        }
    }
}
fn unmap(mapping: &mut Option<MappedRegion>) -> Result<(), SystemCallError> {
    if let Some(region) = mapping.take()
        && let Err((region, error)) = region.unmap()
    {
        *mapping = Some(region);
        return Err(error);
    }
    Ok(())
}
