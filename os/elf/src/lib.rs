//! ELF64 就地解析：ELF 头 + program header 遍历 + ISA 需求判定（host 可测）。
//!
//! 只覆盖静态执行文件（ET_EXEC）的 PT_LOAD 段与用户执行需求
//! （`e_flags` + `.riscv.attributes`，见 references/normative/
//! riscv-psabi-v1.0/riscv-elf.adoc「Attributes」）——装载用户程序所需的最小面。
//! 结构字段按 little-endian RISC-V ELF64 布局就地读，不拷贝。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

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

/// 解析结果：入口地址 + 全部 PT_LOAD 段（按 vaddr 升序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elf {
    pub entry: u64,
    pub segments: alloc::vec::Vec<LoadSegment>,
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

// ---------------------------------------------------------------------------
// ISA 需求判定：e_flags + .riscv.attributes（psABI「e_flags layout」与
// 「Tag_RISCV_arch, 5, NTBS=subarch」）
// ---------------------------------------------------------------------------

const EF_RISCV_FLOAT_ABI: u32 = 0x0006;
const EF_RISCV_FLOAT_ABI_SINGLE: u32 = 0x0002;
const EF_RISCV_FLOAT_ABI_DOUBLE: u32 = 0x0004;
const EF_RISCV_FLOAT_ABI_QUAD: u32 = 0x0006;
const EF_RISCV_RVE: u32 = 0x0008;
const EF_RISCV_TSO: u32 = 0x0010;
/// 标准软件不得置位的保留位（bits 5-23）。
const EF_RISCV_RESERVED: u32 = 0x00FF_FFE0;

const SHT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;
const TAG_FILE: u64 = 1;
const TAG_RISCV_ARCH: u64 = 5;

/// Base64 档位允许的扩展集合（i 由 base 隐含；zmmul/zaamo/zalrsc/
/// zca/zcd 是 m/a/c 的规范子集扩展，工具链会显式展开）。
const BASE_EXTENSIONS: [&str; 11] = [
    "m", "a", "c", "zicsr", "zifencei", "zicntr", "zmmul", "zaamo", "zalrsc", "zca", "zcd",
];
/// D64 在 Base 之上额外允许的扩展。
const D64_EXTENSIONS: [&str; 2] = ["f", "d"];

/// 判定 ELF 的用户执行需求。`buf` 为完整 ELF 映像。
pub fn isa_requirement(buf: &[u8]) -> Result<IsaRequirement, IsaReqError> {
    if buf.len() < 64 {
        return Err(IsaReqError::MalformedAttributes);
    }
    // ELF64 ehdr：e_flags 位于偏移 48（e_shoff 之后）。
    let e_flags = u32_at(buf, 48);
    let shoff = u64_at(buf, 40) as usize;
    let shentsize = u16_at(buf, 58) as usize;
    let shnum = u16_at(buf, 60) as usize;

    // ---- e_flags 契约面（riscv-elf.adoc「Layout of e_flags」）----
    if e_flags & EF_RISCV_RESERVED != 0 {
        return Err(IsaReqError::BadFlags);
    }
    if e_flags & EF_RISCV_RVE != 0 {
        return Err(IsaReqError::Rve);
    }
    if e_flags & EF_RISCV_TSO != 0 {
        return Err(IsaReqError::Tso);
    }
    let float_abi = e_flags & EF_RISCV_FLOAT_ABI;
    match float_abi {
        EF_RISCV_FLOAT_ABI_QUAD => return Err(IsaReqError::QuadAbi),
        EF_RISCV_FLOAT_ABI_SINGLE => return Err(IsaReqError::FOnly),
        _ => {}
    }

    // ---- .riscv.attributes：取 Tag_RISCV_arch 字符串 ----
    let arch = find_arch_string(buf, shoff, shentsize, shnum)?;

    // ---- 规范化 arch 字符串：剥版本后缀，按白名单分档 ----
    let mut tokens = arch.split('_');
    let base = tokens.next().ok_or(IsaReqError::MissingArch)?;
    let base_name = strip_version(base).ok_or(IsaReqError::BadBase)?;
    if !base_name.starts_with("rv64") {
        return Err(IsaReqError::BadBase);
    }
    // base token 内打包的单字母扩展（规范要求展开为下划线分隔，
    // 打包形式按其字母逐一核验；出现无法识别的字母即拒绝）。
    let mut has_f = false;
    let mut has_d = false;
    for c in base_name["rv64".len()..].chars() {
        match c {
            'i' | 'm' | 'a' | 'c' => {}
            'f' => has_f = true,
            'd' => has_d = true,
            _ => return Err(IsaReqError::UnsupportedExtension),
        }
    }
    for token in tokens {
        let name = strip_version(token).ok_or(IsaReqError::UnsupportedExtension)?;
        match name {
            "f" => has_f = true,
            "d" | "zcd" => has_d = true, // zcd ⊂ d：压缩 FP 存取同样依赖 FP 状态
            n if BASE_EXTENSIONS.contains(&n) => {}
            "g" => return Err(IsaReqError::UnsupportedExtension), // 缩写未展开：非规范形式
            n if D64_EXTENSIONS.contains(&n) && float_abi == EF_RISCV_FLOAT_ABI_DOUBLE => {}
            _ => return Err(IsaReqError::UnsupportedExtension),
        }
    }

    if float_abi == EF_RISCV_FLOAT_ABI_DOUBLE {
        if !has_f || !has_d {
            return Err(IsaReqError::AbiArchMismatch);
        }
        Ok(IsaRequirement::D64)
    } else {
        // soft ABI：任何 FP 状态声明都意味着可能执行 FP 指令而 FS=Off
        // 无法隔离——拒绝而非降级。
        if has_f || has_d {
            return Err(IsaReqError::AbiArchMismatch);
        }
        Ok(IsaRequirement::Base64)
    }
}

/// 剥离 token 尾部的版本后缀（`\d+p\d+` 可重复，如 `2p1`、`1p12_0p7p1`）。
fn strip_version(token: &str) -> Option<&str> {
    let mut s = token;
    loop {
        // 结尾是 digit p digit* 的模式：找最后一个非版本边界
        let Some(last_digit_end) = s.rfind(|c: char| c.is_ascii_digit()).map(|i| i + 1) else {
            break;
        };
        if last_digit_end != s.len() {
            break;
        }
        // 找到数字段起点
        let num_start = s[..last_digit_end]
            .rfind(|c: char| !c.is_ascii_digit())
            .map(|i| i + 1)
            .unwrap_or(0);
        if num_start == 0 || s.as_bytes()[num_start - 1] != b'p' {
            break;
        }
        // 'p' 之前必须还有一段数字
        let before_p = &s[..num_start - 1];
        let major_start = before_p
            .rfind(|c: char| !c.is_ascii_digit())
            .map(|i| i + 1)
            .unwrap_or(0);
        if major_start == num_start - 1 {
            break;
        }
        s = &s[..major_start];
    }
    (!s.is_empty()).then_some(s)
}

/// 在 section header 表中定位 SHT_RISCV_ATTRIBUTES 节并解析出
/// Tag_RISCV_arch 的 NTBS 值。
fn find_arch_string(
    buf: &[u8],
    shoff: usize,
    shentsize: usize,
    shnum: usize,
) -> Result<&str, IsaReqError> {
    if shentsize < 64 || shoff.checked_add(shnum.checked_mul(shentsize).ok_or(IsaReqError::MalformedAttributes)?).ok_or(IsaReqError::MalformedAttributes)? > buf.len() {
        return Err(IsaReqError::MalformedAttributes);
    }
    for i in 0..shnum {
        let sh = &buf[shoff + i * shentsize..][..64];
        if u32_at(sh, 4) != SHT_RISCV_ATTRIBUTES {
            continue;
        }
        let off = u64_at(sh, 24) as usize;
        let size = u64_at(sh, 32) as usize;
        let data = buf
            .get(off..off.checked_add(size).ok_or(IsaReqError::MalformedAttributes)?)
            .ok_or(IsaReqError::MalformedAttributes)?;
        return parse_attributes(data);
    }
    Err(IsaReqError::MissingArch)
}

/// 解析 attributes 节内容（ELF attributes 'A' 格式）。
///
/// 布局：首字节 'A'、u32 LE 总长，随后若干子节；每子节为
/// vendor NTBS、Tag_File(uleb)、u32 LE 内容长、属性序列。属性序列内
/// 奇数 tag 为 NTBS、偶数 tag 为 uleb128 值。
fn parse_attributes(data: &[u8]) -> Result<&str, IsaReqError> {
    if data.first() != Some(&b'A') || data.len() < 5 {
        return Err(IsaReqError::MalformedAttributes);
    }
    let mut pos = 5; // 'A' + u32 总长
    while pos < data.len() {
        // vendor NTBS
        let vlen = data[pos..]
            .iter()
            .position(|&b| b == 0)
            .ok_or(IsaReqError::MalformedAttributes)?;
        pos += vlen + 1;
        let (tag, next) = uleb128(data, pos).ok_or(IsaReqError::MalformedAttributes)?;
        // 子节 size 自 tag 字段起计（tag + 长度字段 + 内容）。
        let tag_pos = pos;
        pos = next;
        if pos + 4 > data.len() {
            return Err(IsaReqError::MalformedAttributes);
        }
        let size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let end = tag_pos.checked_add(size).ok_or(IsaReqError::MalformedAttributes)?;
        if end > data.len() {
            return Err(IsaReqError::MalformedAttributes);
        }
        let body = &data[pos..end];
        if tag == TAG_FILE {
            return arch_from_subsection(body);
        }
        pos = end;
    }
    Err(IsaReqError::MissingArch)
}

/// 在 Tag_File 子节内扫描属性：奇数 tag 为 NTBS、偶数 tag 为 uleb128。
fn arch_from_subsection(mut body: &[u8]) -> Result<&str, IsaReqError> {
    while !body.is_empty() {
        let (tag, next) = uleb128(body, 0).ok_or(IsaReqError::MalformedAttributes)?;
        body = &body[next..];
        if tag == TAG_RISCV_ARCH {
            let len = body.iter().position(|&b| b == 0).ok_or(IsaReqError::MalformedAttributes)?;
            return core::str::from_utf8(&body[..len]).map_err(|_| IsaReqError::MalformedAttributes);
        }
        if tag % 2 == 0 {
            let (_, next) = uleb128(body, 0).ok_or(IsaReqError::MalformedAttributes)?;
            body = &body[next..];
        } else {
            let len = body.iter().position(|&b| b == 0).ok_or(IsaReqError::MalformedAttributes)?;
            body = &body[len + 1..];
        }
    }
    Err(IsaReqError::MissingArch)
}

/// uleb128 就地解码，返回 (值, 下一位置)。
fn uleb128(data: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = *data.get(pos)?;
        pos += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, pos));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}
