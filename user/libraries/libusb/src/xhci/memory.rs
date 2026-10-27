//! xHCI 所用的一页稳定 DMA 存储；共享字节只经 volatile，不构造并发 Rust 引用。
use erhino_shared::{
    call::SystemCallError,
    dma::{self, MappingPhase, Segment},
    mem::MemoryProtection,
    object::{ObjectSignals, Rights},
    proc::PROCESS_PAGE_SIZE,
};
use libkernel::{
    dma::{DmaMapping, DmaUse},
    ipc::wait,
    memory_object::MemoryObject,
    mm::{MappedRegion, Placement},
};
pub(super) const PAGE: usize = PROCESS_PAGE_SIZE;
pub(super) struct Page {
    memory: MemoryObject,
    cpu: Option<MappedRegion>,
    mapping: Option<DmaMapping>,
    address: Option<u64>,
}
impl Page {
    pub fn allocate() -> Result<Self, SystemCallError> {
        let memory = MemoryObject::create(PAGE)?;
        let cpu = MappedRegion::map_object(
            &memory,
            0,
            PAGE,
            0,
            0,
            MemoryProtection::ReadWrite,
            Placement::Anywhere,
        )?;
        Ok(Self {
            memory,
            cpu: Some(cpu),
            mapping: None,
            address: None,
        })
    }
    pub fn prepare(&mut self, usage: &DmaUse) -> Result<bool, SystemCallError> {
        if self.mapping.is_none() {
            self.mapping = Some(usage.pin(
                &self.memory,
                0,
                PAGE,
                dma::BIDIRECTIONAL,
                Rights::READ | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
            )?);
            return Ok(false);
        }
        let mapping = self
            .mapping
            .as_ref()
            .ok_or(SystemCallError::InternalError)?;
        if self.address.is_none() {
            let phase = mapping.query()?.phase;
            if phase == MappingPhase::Preparing as u32 {
                return Ok(false);
            }
            if !matches!(phase,x if x==MappingPhase::ReadyUnexposed as u32 || x==MappingPhase::MayBeExposed as u32)
            {
                return Err(SystemCallError::ObjectClosed);
            }
            let mut segments = [Segment::default()];
            let result = mapping.read_segments(0, &mut segments)?;
            if result.count != 1
                || result.next_offset != PAGE as u64
                || segments[0].bytes != PAGE as u64
                || !segments[0].address.is_multiple_of(PAGE as u64)
            {
                return Err(SystemCallError::IllegalArgument);
            }
            self.address = Some(segments[0].address);
        }
        Ok(true)
    }
    pub fn address(&self) -> Result<u64, SystemCallError> {
        self.address.ok_or(SystemCallError::ObjectNotAvailable)
    }
    fn pointer(&self, offset: usize) -> Result<*mut u32, SystemCallError> {
        if !offset.is_multiple_of(4) || offset.checked_add(4).is_none_or(|end| end > PAGE) {
            return Err(SystemCallError::IllegalArgument);
        }
        let start = self
            .cpu
            .as_ref()
            .and_then(|cpu| cpu.usable())
            .ok_or(SystemCallError::ObjectClosed)?
            .start;
        Ok((start + offset) as *mut u32)
    }
    pub fn read(&self, offset: usize) -> Result<u32, SystemCallError> {
        // SAFETY: 独占 CPU 映射在本 owner 保活，DMA 保留独立存在；只以固定宽度volatile访问，
        // 不产生普通引用。TRB/context 的设备并发语义与发布/完成顺序由 Controller 校验。
        Ok(u32::from_le(unsafe {
            core::ptr::read_volatile(self.pointer(offset)?)
        }))
    }
    pub fn write(&self, offset: usize, value: u32) -> Result<(), SystemCallError> {
        // SAFETY: 同 read；字段写入仅由本 actor 进行，不存在其他 CPU 写者。
        unsafe {
            core::ptr::write_volatile(self.pointer(offset)?, value.to_le());
        }
        Ok(())
    }
    pub fn write64(&self, offset: usize, value: u64) -> Result<(), SystemCallError> {
        self.write(offset, value as u32)?;
        self.write(offset + 4, (value >> 32) as u32)
    }
    pub fn zero(&self) -> Result<(), SystemCallError> {
        for at in (0..PAGE).step_by(4) {
            self.write(at, 0)?;
        }
        Ok(())
    }
    /// # Safety
    /// 该页全部硬件访问已结束且其缓存地址不会再次提交；关CPU/清BME不能提供该证明。
    pub unsafe fn retire(&self, usage: &DmaUse) -> Result<bool, SystemCallError> {
        let Some(mapping) = &self.mapping else {
            return Ok(true);
        };
        // SAFETY: 调用者提供 Controller halt 或 Disable Slot 完成并收清事件的真实证明。
        unsafe {
            usage.retire(mapping)?;
        }
        Ok(wait::probe(mapping.handle(), ObjectSignals::DONE)?.contains(ObjectSignals::DONE))
    }
    pub fn close_cpu(&mut self) -> Result<(), SystemCallError> {
        if let Some(cpu) = self.cpu.take()
            && let Err((cpu, error)) = cpu.unmap()
        {
            self.cpu = Some(cpu);
            return Err(error);
        }
        Ok(())
    }
}
impl Drop for Page {
    fn drop(&mut self) {
        let _ = self.close_cpu();
    }
}
pub(super) fn fence() {
    // SAFETY: RV64 FENCE排序此hart的共享ring与设备寄存器；不证明设备完成，也不替代非一致cache维护。
    unsafe {
        core::arch::asm!("fence iorw,iorw", options(nostack, preserves_flags));
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}
impl Trb {
    pub fn kind(self) -> u32 {
        (self.control >> 10) & 63
    }
    pub fn completion(self) -> u8 {
        (self.status >> 24) as u8
    }
    pub fn slot(self) -> u8 {
        (self.control >> 24) as u8
    }
}
pub(super) struct Ring {
    pub page: Page,
    index: usize,
    cycle: bool,
}
impl Ring {
    pub fn allocate() -> Result<Self, SystemCallError> {
        Ok(Self {
            page: Page::allocate()?,
            index: 0,
            cycle: true,
        })
    }
    pub fn prepare(&mut self, usage: &DmaUse) -> Result<bool, SystemCallError> {
        self.page.prepare(usage)
    }
    pub fn reset(&mut self) -> Result<(), SystemCallError> {
        self.page.zero()?;
        self.index = 0;
        self.cycle = true;
        Ok(())
    }
    pub fn dequeue(&self) -> Result<u64, SystemCallError> {
        self.page
            .address()
            .map(|a| (a + (self.index * 16) as u64) | u64::from(self.cycle))
    }
    pub fn push(&mut self, trb: Trb) -> Result<u64, SystemCallError> {
        // 首批每ring只有一份已预留的串行TD/command；调用者收完成前不会覆写未消费项。
        if self.index == PAGE / 16 - 1 {
            self.publish(Trb {
                parameter: self.page.address()?,
                status: 0,
                control: (6 << 10) | 2,
            })?;
            self.index = 0;
            self.cycle = !self.cycle;
        }
        self.publish(trb)
    }
    fn publish(&mut self, trb: Trb) -> Result<u64, SystemCallError> {
        let at = self.index * 16;
        self.page.write64(at, trb.parameter)?;
        self.page.write(at + 8, trb.status)?;
        fence();
        self.page
            .write(at + 12, (trb.control & !1) | u32::from(self.cycle))?;
        fence();
        self.index += 1;
        Ok(self.page.address()? + at as u64)
    }
}
