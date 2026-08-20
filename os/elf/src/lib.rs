//! ELF64 就地解析：ELF 头 + program header 遍历（host 可测）。
//!
//! 只覆盖静态执行文件（ET_EXEC）的 PT_LOAD 段——装载用户程序所需的最小面。
//! 结构字段按 little-endian RISC-V ELF64 布局就地读，不拷贝。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

/// 一个 PT_LOAD 段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSegment {
    /// 装载目标虚拟地址（页对齐）。
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

/// 解析结果：入口地址 + 全部 PT_LOAD 段（按 vaddr 升序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elf {
    pub entry: u64,
    pub segments: alloc::vec::Vec<LoadSegment>,
}

/// 解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    BadMagic,
    BadClass,
    BadEndian,
    BadType,
    BadMachine,
    BadPhoff,
}

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

/// 解析 ELF64 little-endian RISC-V 执行文件。
pub fn parse(buf: &[u8]) -> Result<Elf, ElfError> {
    use core::mem::size_of;

    type Ehdr = [u8; 64];
    type Phdr = [u8; 56];
    const _: () = assert!(size_of::<Ehdr>() == 64 && size_of::<Phdr>() == 56);

    if buf.len() < size_of::<Ehdr>() {
        return Err(ElfError::TooShort);
    }
    let e: &Ehdr = buf[..size_of::<Ehdr>()].try_into().unwrap();
    if &e[..4] != b"\x7fELF" {
        return Err(ElfError::BadMagic);
    }
    if e[4] != 2 {
        return Err(ElfError::BadClass); // ELF64
    }
    if e[5] != 1 {
        return Err(ElfError::BadEndian); // little-endian
    }
    let e_type = u16_at(e, 16);
    if e_type != 2 {
        return Err(ElfError::BadType); // ET_EXEC
    }
    if u16_at(e, 18) != 243 {
        return Err(ElfError::BadMachine); // EM_RISCV
    }
    let entry = u64_at(e, 24);
    let phoff = u64_at(e, 32) as usize;
    let phentsize = u16_at(e, 54) as usize;
    let phnum = u16_at(e, 56) as usize;
    if phentsize != size_of::<Phdr>() {
        return Err(ElfError::BadPhoff);
    }
    let table_end = phoff.checked_add(phnum.checked_mul(phentsize).ok_or(ElfError::BadPhoff)?)
        .ok_or(ElfError::BadPhoff)?;
    if table_end > buf.len() {
        return Err(ElfError::BadPhoff);
    }

    let mut segments = alloc::vec::Vec::new();
    for i in 0..phnum {
        let p: &Phdr = buf[phoff + i * phentsize..][..size_of::<Phdr>()]
            .try_into()
            .unwrap();
        if u32_at(p, 0) != PT_LOAD {
            continue;
        }
        segments.push(LoadSegment {
            vaddr: u64_at(p, 16),
            offset: u64_at(p, 8),
            filesz: u64_at(p, 32),
            memsz: u64_at(p, 40),
            readable: u32_at(p, 4) & PF_R != 0,
            writable: u32_at(p, 4) & PF_W != 0,
            executable: u32_at(p, 4) & PF_X != 0,
        });
    }
    segments.sort_by_key(|s| s.vaddr);
    Ok(Elf { entry, segments })
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
