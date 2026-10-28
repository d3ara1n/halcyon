//! ISA属性的增量读取；一次只处理一个节头、ULEB值或字符串字节。
use super::{IsaReqError as Error, IsaRequirement, u16_at, u32_at, u64_at};

const ATTRIBUTES: u32 = 0x7000_0003;
const ARCH: u64 = 5;

/// 相对原映像的范围；跨步不保存任何内部借用。
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}
impl Span {
    fn len(self) -> usize {
        self.end - self.start
    }
    fn bytes(self, buf: &[u8]) -> Result<&[u8], Error> {
        buf.get(self.start..self.end)
            .ok_or(Error::MalformedAttributes)
    }
    fn slice(self, start: usize, end: usize) -> Result<Self, Error> {
        if start > end || end > self.len() {
            return Err(Error::MalformedAttributes);
        }
        Ok(Self {
            start: self.start + start,
            end: self.start + end,
        })
    }
}
pub(super) struct IsaReader {
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    double: bool,
    phase: Phase,
}
enum Phase {
    Section(usize),
    VendorBlock(Span),
    Vendor {
        data: Span,
        pos: usize,
        rest: Span,
    },
    Subsection {
        data: Span,
        pos: usize,
        rest: Span,
    },
    Attribute {
        data: Span,
        pos: usize,
    },
    String {
        data: Span,
        start: usize,
        pos: usize,
        arch: bool,
    },
    Arch(ArchReader),
    Done,
}
impl IsaReader {
    pub(super) fn new(buf: &[u8]) -> Result<Self, Error> {
        if buf.len() < 64 {
            return Err(Error::MalformedAttributes);
        }
        let flags = u32_at(buf, 48);
        if flags & 0xffff_ffe0 != 0 {
            return Err(Error::BadFlags);
        }
        if flags & 8 != 0 {
            return Err(Error::Rve);
        }
        if flags & 16 != 0 {
            return Err(Error::Tso);
        }
        match flags & 6 {
            6 => return Err(Error::QuadAbi),
            2 => return Err(Error::FOnly),
            _ => {}
        }
        let shoff = usize::try_from(u64_at(buf, 40)).map_err(|_| Error::MalformedAttributes)?;
        let shentsize = u16_at(buf, 58) as usize;
        let shnum = u16_at(buf, 60) as usize;
        if shentsize < 64
            || shoff
                .checked_add(
                    shnum
                        .checked_mul(shentsize)
                        .ok_or(Error::MalformedAttributes)?,
                )
                .ok_or(Error::MalformedAttributes)?
                > buf.len()
        {
            return Err(Error::MalformedAttributes);
        }
        Ok(Self {
            shoff,
            shentsize,
            shnum,
            double: flags & 6 == 4,
            phase: Phase::Section(0),
        })
    }
    pub(super) fn advance(&mut self, buf: &[u8]) -> Result<Option<IsaRequirement>, Error> {
        self.phase = match core::mem::replace(&mut self.phase, Phase::Done) {
            Phase::Section(index) => {
                if index == self.shnum {
                    return Err(Error::MissingArch);
                }
                let sh = buf
                    .get(self.shoff + index * self.shentsize..)
                    .and_then(|bytes| bytes.get(..64))
                    .ok_or(Error::MalformedAttributes)?;
                if u32_at(sh, 4) != ATTRIBUTES {
                    Phase::Section(index + 1)
                } else {
                    let off =
                        usize::try_from(u64_at(sh, 24)).map_err(|_| Error::MalformedAttributes)?;
                    let size =
                        usize::try_from(u64_at(sh, 32)).map_err(|_| Error::MalformedAttributes)?;
                    let span = Span {
                        start: off,
                        end: off.checked_add(size).ok_or(Error::MalformedAttributes)?,
                    };
                    let data = span.bytes(buf)?;
                    if data.len() < 5 || data[0] != b'A' {
                        return Err(Error::MalformedAttributes);
                    }
                    Phase::VendorBlock(span.slice(1, size)?)
                }
            }
            Phase::VendorBlock(bytes) => {
                if bytes.len() == 0 {
                    return Err(Error::MissingArch);
                }
                let length = u32_at(
                    bytes
                        .bytes(buf)?
                        .get(..4)
                        .ok_or(Error::MalformedAttributes)?,
                    0,
                ) as usize;
                // aaelf32 Build Attributes：vendor长度含自身4字节、名称NTBS及属性数据。
                let data = bytes.slice(4, length)?;
                let rest = bytes.slice(length, bytes.len())?;
                Phase::Vendor { data, pos: 0, rest }
            }
            Phase::Vendor { data, pos, rest } => {
                let bytes = data.bytes(buf)?;
                let byte = *bytes.get(pos).ok_or(Error::MalformedAttributes)?;
                if byte == 0 {
                    if &bytes[..pos] == b"riscv" {
                        Phase::Subsection {
                            data,
                            pos: pos + 1,
                            rest,
                        }
                    } else {
                        // 其它vendor的同号tag不是RISC-V执行需求。
                        Phase::VendorBlock(rest)
                    }
                } else {
                    Phase::Vendor {
                        data,
                        pos: pos + 1,
                        rest,
                    }
                }
            }
            Phase::Subsection { data, pos, rest } => {
                if pos == data.len() {
                    self.phase = Phase::VendorBlock(rest);
                    return Ok(None);
                }
                let bytes = data.bytes(buf)?;
                let (tag, next) = uleb(bytes, pos)?;
                let size_bytes = bytes
                    .get(next..next + 4)
                    .ok_or(Error::MalformedAttributes)?;
                let size = u32_at(size_bytes, 0) as usize;
                let end = pos.checked_add(size).ok_or(Error::MalformedAttributes)?;
                // 长度包含tag与长度字段自身，内容必须从该字段之后开始。
                let body = data.slice(next + 4, end)?;
                if tag == 1 {
                    Phase::Attribute { data: body, pos: 0 }
                } else {
                    Phase::Subsection {
                        data,
                        pos: end,
                        rest,
                    }
                }
            }
            Phase::Attribute { data, pos } => {
                if pos == data.len() {
                    return Err(Error::MissingArch);
                }
                let bytes = data.bytes(buf)?;
                let (tag, next) = uleb(bytes, pos)?;
                if tag % 2 == 0 {
                    let (_, next) = uleb(bytes, next)?;
                    Phase::Attribute { data, pos: next }
                } else {
                    Phase::String {
                        data,
                        start: next,
                        pos: next,
                        arch: tag == ARCH,
                    }
                }
            }
            Phase::String {
                data,
                start,
                pos,
                arch,
            } => {
                let byte = *data
                    .bytes(buf)?
                    .get(pos)
                    .ok_or(Error::MalformedAttributes)?;
                if byte != 0 {
                    Phase::String {
                        data,
                        start,
                        pos: pos + 1,
                        arch,
                    }
                } else if arch {
                    Phase::Arch(ArchReader::new(data.slice(start, pos)?, buf)?)
                } else {
                    Phase::Attribute { data, pos: pos + 1 }
                }
            }
            Phase::Arch(mut reader) => {
                if reader.advance(buf)? {
                    if self.double != (reader.has_f && reader.has_d)
                        || !self.double && (reader.has_f || reader.has_d)
                    {
                        return Err(Error::AbiArchMismatch);
                    }
                    return Ok(Some(if self.double {
                        IsaRequirement::D64
                    } else {
                        IsaRequirement::Base64
                    }));
                }
                Phase::Arch(reader)
            }
            Phase::Done => panic!("ISA validation already completed"),
        };
        Ok(None)
    }
}

/// 名称逐字节核验，版本数字也逐步消费；超长版本串不会占用一次推进。
struct ArchReader {
    bytes: Span,
    pos: usize,
    start: usize,
    name_end: Option<usize>,
    base: bool,
    version: Version,
    has_f: bool,
    has_d: bool,
}
#[derive(Clone, Copy)]
enum Version {
    Name,
    Major,
    MinorStart,
    Minor,
}
impl ArchReader {
    fn new(bytes: Span, buf: &[u8]) -> Result<Self, Error> {
        if !bytes.bytes(buf)?.starts_with(b"rv64") {
            return Err(Error::BadBase);
        }
        Ok(Self {
            bytes,
            pos: 4,
            start: 4,
            name_end: None,
            base: true,
            version: Version::Name,
            has_f: false,
            has_d: false,
        })
    }
    fn advance(&mut self, buf: &[u8]) -> Result<bool, Error> {
        let bytes = self.bytes.bytes(buf)?;
        let byte = bytes.get(self.pos).copied();
        if byte.is_none() || byte == Some(b'_') {
            if matches!(self.version, Version::Major | Version::MinorStart) {
                return Err(Error::UnsupportedExtension);
            }
            let end = self.name_end.unwrap_or(self.pos);
            if end == self.start {
                return Err(if self.base {
                    Error::BadBase
                } else {
                    Error::UnsupportedExtension
                });
            }
            if !self.base {
                match &bytes[self.start..end] {
                    b"f" => self.has_f = true,
                    b"d" | b"zcd" => self.has_d = true,
                    b"m" | b"a" | b"c" | b"zicsr" | b"zifencei" | b"zicntr" | b"zmmul"
                    | b"zaamo" | b"zalrsc" | b"zca" => {}
                    _ => return Err(Error::UnsupportedExtension),
                }
            }
            if byte.is_none() {
                return Ok(true);
            }
            self.pos += 1;
            self.start = self.pos;
            self.name_end = None;
            self.base = false;
            self.version = Version::Name;
            return Ok(false);
        }
        let byte = byte.expect("nonterminal architecture byte");
        match self.version {
            Version::Name if byte.is_ascii_digit() => {
                self.name_end = Some(self.pos);
                self.version = Version::Major;
            }
            Version::Name => {
                if self.base {
                    match byte {
                        b'i' | b'm' | b'a' | b'c' => {}
                        b'f' => self.has_f = true,
                        b'd' => self.has_d = true,
                        _ => return Err(Error::UnsupportedExtension),
                    }
                } else if !byte.is_ascii_lowercase() {
                    return Err(Error::UnsupportedExtension);
                }
            }
            Version::Major if byte == b'p' => self.version = Version::MinorStart,
            Version::Major | Version::Minor if byte.is_ascii_digit() => {}
            Version::MinorStart if byte.is_ascii_digit() => self.version = Version::Minor,
            _ => return Err(Error::UnsupportedExtension),
        }
        self.pos += 1;
        Ok(false)
    }
}

/// 一个u64的ULEB最多十字节；拒绝最高组溢出与无结束符。
fn uleb(data: &[u8], mut pos: usize) -> Result<(u64, usize), Error> {
    let mut result = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(pos).ok_or(Error::MalformedAttributes)?;
        pos += 1;
        if shift == 63 && byte > 1 {
            return Err(Error::MalformedAttributes);
        }
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, pos));
        }
    }
    Err(Error::MalformedAttributes)
}
