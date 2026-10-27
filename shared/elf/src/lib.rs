//! 静态 RISC-V ELF64 的唯一 admission：头部、program headers、装载几何、
//! 页级权限、入口与 ISA 需求一次验证（host 可测）。
//!
//! 只接受当前系统能完整解释的 ET_EXEC 面；动态链接、TLS、未知 segment 与
//! 不可表达的权限明确拒绝。字段按 little-endian ELF64 布局就地读取，输出
//! 同时供内核 bootstrap、用户态 launcher 与 host audit 使用。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod isa;
use isa::IsaReader;

/// 一个 PT_LOAD 段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSegment {
    /// 装载目标虚拟地址；页内偏移须与文件 offset 同余。
    pub vaddr: u64,
    /// 段内容在文件中的偏移。
    pub offset: u64,
    /// 文件内容长度。
    pub filesz: u64,
    /// 内存映像长度（> filesz 部分为 BSS，清零）。
    pub memsz: u64,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

/// 页对齐、权限相同的连续装载区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadRun {
    pub vaddr: u64,
    pub memsz: u64,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

/// 地址空间相关的 admission 边界。page_size 必须是二次幂，image_limit
/// 是首个不可用于映像的虚拟地址（主栈与出生块由调用者保留在其上方）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadLimits {
    pub page_size: u64,
    pub image_limit: u64,
}

/// 已验证映像：调用者不得重新解释原始 program headers。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elf {
    entry: u64,
    requirement: IsaRequirement,
    /// 非空 PT_LOAD，保持规范要求的 vaddr 升序。
    segments: alloc::vec::Vec<LoadSegment>,
    /// PT_LOAD 页投影的权限并集，已拒绝 W+X。
    runs: alloc::vec::Vec<LoadRun>,
    /// 最高映像字节向上对齐到 page_size。
    image_end: u64,
}

impl Elf {
    pub const fn entry(&self) -> u64 {
        self.entry
    }

    pub const fn requirement(&self) -> IsaRequirement {
        self.requirement
    }

    pub fn segments(&self) -> &[LoadSegment] {
        &self.segments
    }

    pub fn runs(&self) -> &[LoadRun] {
        &self.runs
    }

    pub const fn image_end(&self) -> u64 {
        self.image_end
    }
}

/// 用户执行需求档位（notes/execution-context.md「内核与用户 ABI」）。
///
/// 由 ELF `e_flags` 与 `.riscv.attributes` 的 Tag_RISCV_arch 共同决定；
/// 调度 eligibility 另由 [`IsaRequirement::compatible`] 对 hart 能力判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsaRequirement {
    /// 项目定义的 RV64 base 用户环境：LP64，FS=Off。
    Base64,
    /// LP64D：F/D 状态可用，只能运行于有效 FLEN 恰为 64 的 hart。
    D64,
}

impl IsaRequirement {
    /// 与 hart 能力的兼容判定：能力决定 domain eligibility。
    pub fn compatible(self, flen: usize) -> bool {
        match self {
            Self::Base64 => true,
            // 有效 FLEN 恰为 64：Q-capable（FLEN 128 模型未建立）不进入。
            Self::D64 => flen == 64,
        }
    }
}

/// ISA 需求判定错误。全部在 load 时明确拒绝，不降级为 Base。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsaReqError {
    /// 缺少 .riscv.attributes 或其中的 Tag_RISCV_arch。
    MissingArch,
    /// attributes 结构非法（vendor/子节/tag 编码错误）。
    MalformedAttributes,
    /// 非 rv64i base。
    BadBase,
    /// RVE（E ABI）：本内核不支持 16 寄存器 ABI。
    Rve,
    /// EF_RISCV_TSO：Ztso 未进入 capability/domain 模型。
    Tso,
    /// e_flags 保留位非零（标准软件不得置位）。
    BadFlags,
    /// 浮点 ABI 为 Single（F-only 档位未建模）。
    FOnly,
    /// 浮点 ABI 为 Quad（Q 状态模型未建立）。
    QuadAbi,
    /// 双精度 ABI 却缺 f/d 扩展声明。
    AbiArchMismatch,
    /// 出现本内核未建模的状态扩展（含 Q/V 及一切白名单之外的扩展名）。
    UnsupportedExtension,
}

/// 解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    BadMagic,
    BadClass,
    BadEndian,
    BadVersion,
    BadOsAbi,
    BadType,
    BadMachine,
    BadHeaderSize,
    BadProgramHeaders,
    TooManyProgramHeaders,
    TooManyLoadSegments,
    UnsupportedProgramHeader,
    BadProgramHeaderFlags,
    BadProgramHeaderOrder,
    BadSegmentGeometry,
    BadSegmentAlignment,
    UnsupportedProtection,
    OverlappingSegments,
    WriteExecutePage,
    BadEntry,
    BadLimits,
    OutOfMemory,
    Isa(IsaReqError),
}

const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const PF_RWX: u32 = PF_R | PF_W | PF_X;
const MAX_PROGRAM_HEADERS: usize = 128;
const MAX_LOAD_SEGMENTS: usize = 64;

/// 同步入口与可分步入口共用同一验证器。
pub fn validate(buf: &[u8], limits: LoadLimits) -> Result<Elf, ElfError> {
    let mut validation = Validation::new(buf, limits)?;
    loop {
        if let Some(image) = validation.advance()? {
            return Ok(image);
        }
    }
}

/// 一步处理一个program header、一个页权限区间或一个ISA解析单元。
/// 所有输入借用贯穿验证，调用者不能在两次推进之间更换字节。
pub struct Validation<'a> {
    buf: &'a [u8],
    limits: LoadLimits,
    entry: u64,
    phoff: usize,
    phnum: usize,
    index: usize,
    segments: alloc::vec::Vec<LoadSegment>,
    previous_end: Option<u64>,
    seen_load: bool,
    phdr: Option<(u64, u64, u64)>,
    plan: Option<RunPlan>,
    isa: Option<IsaReader<'a>>,
    done: bool,
    failure: Option<ElfError>,
}
impl<'a> Validation<'a> {
    pub fn new(buf: &'a [u8], limits: LoadLimits) -> Result<Self, ElfError> {
        use core::mem::size_of;

        type Ehdr = [u8; 64];
        type Phdr = [u8; 56];
        const _: () = assert!(size_of::<Ehdr>() == 64 && size_of::<Phdr>() == 56);

        if !limits.page_size.is_power_of_two()
            || limits.image_limit == 0
            || !limits.image_limit.is_multiple_of(limits.page_size)
        {
            return Err(ElfError::BadLimits);
        }
        if buf.len() < size_of::<Ehdr>() {
            return Err(ElfError::TooShort);
        }
        let e: &Ehdr = buf[..size_of::<Ehdr>()].try_into().unwrap();
        if &e[..4] != b"\x7fELF" {
            return Err(ElfError::BadMagic);
        }
        if e[4] != 2 {
            return Err(ElfError::BadClass);
        }
        if e[5] != 1 {
            return Err(ElfError::BadEndian);
        }
        if e[6] != 1 || u32_at(e, 20) != 1 {
            return Err(ElfError::BadVersion);
        }
        if e[7] != 0 || e[8] != 0 {
            return Err(ElfError::BadOsAbi);
        }
        if u16_at(e, 16) != 2 {
            return Err(ElfError::BadType);
        }
        if u16_at(e, 18) != 243 {
            return Err(ElfError::BadMachine);
        }
        if u16_at(e, 52) as usize != size_of::<Ehdr>() {
            return Err(ElfError::BadHeaderSize);
        }

        let entry = u64_at(e, 24);
        let phoff = usize::try_from(u64_at(e, 32)).map_err(|_| ElfError::BadProgramHeaders)?;
        let phentsize = u16_at(e, 54) as usize;
        let phnum = u16_at(e, 56) as usize;
        if phentsize != size_of::<Phdr>() || phnum == 0 {
            return Err(ElfError::BadProgramHeaders);
        }
        if phnum > MAX_PROGRAM_HEADERS {
            return Err(ElfError::TooManyProgramHeaders);
        }
        let table_end = phoff
            .checked_add(
                phnum
                    .checked_mul(phentsize)
                    .ok_or(ElfError::BadProgramHeaders)?,
            )
            .ok_or(ElfError::BadProgramHeaders)?;
        if table_end > buf.len() {
            return Err(ElfError::BadProgramHeaders);
        }

        let mut segments = alloc::vec::Vec::new();
        segments
            .try_reserve_exact(MAX_LOAD_SEGMENTS)
            .map_err(|_| ElfError::OutOfMemory)?;
        Ok(Self {
            buf,
            limits,
            entry,
            phoff,
            phnum,
            index: 0,
            segments,
            previous_end: None,
            seen_load: false,
            phdr: None,
            plan: None,
            isa: None,
            done: false,
            failure: None,
        })
    }
    pub fn advance(&mut self) -> Result<Option<Elf>, ElfError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.step();
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
    fn step(&mut self) -> Result<Option<Elf>, ElfError> {
        assert!(!self.done, "ELF validation already completed");
        if self.index < self.phnum {
            self.header()?;
            self.index += 1;
            return Ok(None);
        }
        if self.plan.is_none() {
            self.finish_headers()?;
            self.plan = Some(RunPlan::new(&self.segments, self.limits.page_size)?);
            return Ok(None);
        }
        let plan = self.plan.as_mut().expect("ELF plan remains owned");
        if !plan.advance(&self.segments, self.limits.page_size)? {
            return Ok(None);
        }
        if self.isa.is_none() {
            self.isa = Some(IsaReader::new(self.buf).map_err(ElfError::Isa)?);
            return Ok(None);
        }
        let Some(requirement) = self
            .isa
            .as_mut()
            .expect("ELF ISA reader remains owned")
            .advance()
            .map_err(ElfError::Isa)?
        else {
            return Ok(None);
        };
        self.done = true;
        Ok(Some(Elf {
            entry: self.entry,
            requirement,
            segments: core::mem::take(&mut self.segments),
            runs: core::mem::take(&mut plan.runs),
            image_end: plan.image_end,
        }))
    }
    fn header(&mut self) -> Result<(), ElfError> {
        let buf = self.buf;
        let p: &[u8; 56] = buf[self.phoff + self.index * 56..][..56]
            .try_into()
            .unwrap();
        let kind = u32_at(p, 0);
        // gABI 明定 PT_NULL 除类型外所有成员均未定义，必须直接忽略。
        if kind == PT_NULL {
            return Ok(());
        }
        let flags = u32_at(p, 4);
        if flags & !PF_RWX != 0 {
            return Err(ElfError::BadProgramHeaderFlags);
        }
        let offset = u64_at(p, 8);
        let vaddr = u64_at(p, 16);
        let filesz = u64_at(p, 32);
        let memsz = u64_at(p, 40);
        let align = u64_at(p, 48);
        if align > 1 && (!align.is_power_of_two() || vaddr % align != offset % align) {
            return Err(ElfError::BadSegmentAlignment);
        }
        match kind {
            PT_LOAD => {
                self.seen_load = true;
                if self.segments.len() == MAX_LOAD_SEGMENTS {
                    return Err(ElfError::TooManyLoadSegments);
                }
                if memsz == 0 || filesz > memsz {
                    return Err(ElfError::BadSegmentGeometry);
                }
                let file_end = offset
                    .checked_add(filesz)
                    .ok_or(ElfError::BadSegmentGeometry)?;
                if usize::try_from(file_end).map_or(true, |end| end > buf.len()) {
                    return Err(ElfError::BadSegmentGeometry);
                }
                let end = vaddr
                    .checked_add(memsz)
                    .ok_or(ElfError::BadSegmentGeometry)?;
                if end > self.limits.image_limit
                    || vaddr % self.limits.page_size != offset % self.limits.page_size
                {
                    return Err(ElfError::BadSegmentAlignment);
                }
                if self.previous_end.is_some_and(|previous| vaddr < previous) {
                    return Err(ElfError::OverlappingSegments);
                }
                self.previous_end = Some(end);
                let readable = flags & PF_R != 0;
                let writable = flags & PF_W != 0;
                let executable = flags & PF_X != 0;
                if flags == 0 || writable && !readable {
                    return Err(ElfError::UnsupportedProtection);
                }
                self.segments.push(LoadSegment {
                    vaddr,
                    offset,
                    filesz,
                    memsz,
                    readable,
                    writable,
                    executable,
                });
            }
            PT_NOTE => validate_file_segment(buf, offset, filesz)?,
            PT_PHDR => {
                let table_bytes = (self.phnum * 56) as u64;
                if self.seen_load
                    || self.phdr.is_some()
                    || flags != PF_R
                    || offset != self.phoff as u64
                    || filesz != table_bytes
                    || memsz != table_bytes
                {
                    return Err(ElfError::BadProgramHeaderOrder);
                }
                validate_file_segment(buf, offset, filesz)?;
                self.phdr = Some((vaddr, offset, filesz));
            }
            PT_GNU_STACK => {
                if filesz != 0 || memsz != 0 || flags & PF_X != 0 {
                    return Err(ElfError::UnsupportedProtection);
                }
            }
            PT_RISCV_ATTRIBUTES => {
                if flags != PF_R {
                    return Err(ElfError::UnsupportedProtection);
                }
                validate_file_segment(buf, offset, filesz)?;
            }
            _ => return Err(ElfError::UnsupportedProgramHeader),
        }
        Ok(())
    }
    fn finish_headers(&self) -> Result<(), ElfError> {
        if self.segments.is_empty() {
            return Err(ElfError::BadSegmentGeometry);
        }
        if let Some((phdr_vaddr, phdr_offset, phdr_size)) = self.phdr {
            let covered = self.segments.iter().any(|segment| {
                segment.offset <= phdr_offset
                    && segment
                        .offset
                        .checked_add(segment.filesz)
                        .is_some_and(|end| phdr_offset + phdr_size <= end)
                    && segment.vaddr.checked_add(phdr_offset - segment.offset) == Some(phdr_vaddr)
            });
            if !covered {
                return Err(ElfError::BadSegmentGeometry);
            }
        }
        let entry_valid = self.segments.iter().any(|segment| {
            segment.executable
                && segment
                    .vaddr
                    .checked_add(segment.filesz)
                    .is_some_and(|end| segment.vaddr <= self.entry && self.entry < end)
        });
        if !entry_valid {
            return Err(ElfError::BadEntry);
        }

        Ok(())
    }
}

fn validate_file_segment(buf: &[u8], offset: u64, filesz: u64) -> Result<(), ElfError> {
    let end = offset
        .checked_add(filesz)
        .ok_or(ElfError::BadSegmentGeometry)?;
    if usize::try_from(end).map_or(true, |end| end > buf.len()) {
        return Err(ElfError::BadSegmentGeometry);
    }
    Ok(())
}

struct RunPlan {
    boundaries: alloc::vec::Vec<u64>,
    runs: alloc::vec::Vec<LoadRun>,
    image_end: u64,
    index: usize,
}
impl RunPlan {
    fn new(segments: &[LoadSegment], page_size: u64) -> Result<Self, ElfError> {
        let mut boundaries = alloc::vec::Vec::new();
        boundaries
            .try_reserve_exact(segments.len() * 2)
            .map_err(|_| ElfError::OutOfMemory)?;
        let mut image_end = 0;
        for segment in segments {
            let start = segment.vaddr / page_size * page_size;
            let end = align_up(
                segment
                    .vaddr
                    .checked_add(segment.memsz)
                    .ok_or(ElfError::BadSegmentGeometry)?,
                page_size,
            )?;
            boundaries.push(start);
            boundaries.push(end);
            image_end = image_end.max(end);
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let mut runs: alloc::vec::Vec<LoadRun> = alloc::vec::Vec::new();
        runs.try_reserve_exact(boundaries.len().saturating_sub(1))
            .map_err(|_| ElfError::OutOfMemory)?;
        Ok(Self {
            boundaries,
            runs,
            image_end,
            index: 0,
        })
    }
    fn advance(&mut self, segments: &[LoadSegment], page_size: u64) -> Result<bool, ElfError> {
        if self.index + 1 >= self.boundaries.len() {
            return Ok(true);
        }
        let window = &self.boundaries[self.index..self.index + 2];
        self.index += 1;
        let start = window[0];
        let end = window[1];
        let mut flags = 0;
        for segment in segments {
            let segment_start = segment.vaddr / page_size * page_size;
            let segment_end = align_up(
                segment
                    .vaddr
                    .checked_add(segment.memsz)
                    .ok_or(ElfError::BadSegmentGeometry)?,
                page_size,
            )?;
            if segment_start < end && start < segment_end {
                if segment.readable {
                    flags |= PF_R;
                }
                if segment.writable {
                    flags |= PF_W;
                }
                if segment.executable {
                    flags |= PF_X;
                }
            }
        }
        if flags == 0 {
            return Ok(false);
        }
        if flags & (PF_W | PF_X) == PF_W | PF_X {
            return Err(ElfError::WriteExecutePage);
        }
        let writable = flags & PF_W != 0;
        let executable = flags & PF_X != 0;
        // gABI 允许系统把 X-only 段提升为 R+X；当前页表保护恰采用该上界。
        let readable = flags & PF_R != 0 || executable;
        if let Some(previous) = self.runs.last_mut()
            && previous.vaddr + previous.memsz == start
            && previous.readable == readable
            && previous.writable == writable
            && previous.executable == executable
        {
            previous.memsz += end - start;
            return Ok(false);
        }
        self.runs.push(LoadRun {
            vaddr: start,
            memsz: end - start,
            readable,
            writable,
            executable,
        });
        Ok(false)
    }
}

fn align_up(value: u64, align: u64) -> Result<u64, ElfError> {
    value
        .checked_add(align - 1)
        .map(|value| value / align * align)
        .ok_or(ElfError::BadSegmentGeometry)
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().unwrap())
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}
