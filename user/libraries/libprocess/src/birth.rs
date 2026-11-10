//! 命名出生帧 v2：能力表、独立 settings 与通用 named data sections。
//! 输入和 section 分别按名字字节序递增；采用时以真实能力查询核类型和权利。
//! 本模块不解释任一数据 section 的领域含义。v1 不再接受。
use crate::context::{CurrentProcessInfo, InitialGrants, StartupContext};
use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectKind, Rights},
};
use libkernel::ipc::capability::Capability;

pub const MAGIC: [u8; 8] = *b"ERBIRTH\0";
pub const VERSION: u32 = 2;
pub const MAX_NAME: usize = 32;
const HEADER: usize = 24;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Format(&'static str),
    Grants(&'static str),
    WrongKind,
    RightsDenied,
    Query(SystemCallError),
}
fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.iter().all(|b| b.is_ascii_graphic())
}
fn word(out: &mut Vec<u8>, n: usize) -> Result<(), Error> {
    out.extend_from_slice(
        &u32::try_from(n)
            .map_err(|_| Error::Format("length overflow"))?
            .to_le_bytes(),
    );
    Ok(())
}
fn put_name(out: &mut Vec<u8>, name: &str) -> Result<(), Error> {
    if !valid_name(name.as_bytes()) {
        return Err(Error::Format("invalid name"));
    }
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    Ok(())
}
/// 编码不依赖原始顺序；能力与 section 名属于各自独立的命名空间。
pub fn encode(
    inputs: &[(&str, u8)],
    settings: &[u8],
    sections: &[(&str, &[u8])],
) -> Result<Vec<u8>, Error> {
    if inputs.len() > erhino_shared::proc::PROCESS_MAX_GRANTS {
        return Err(Error::Format("too many inputs"));
    }
    let mut inputs = inputs.to_vec();
    inputs.sort_unstable_by(|a, b| a.0.cmp(b.0));
    if inputs.windows(2).any(|p| p[0].0 == p[1].0) {
        return Err(Error::Format("duplicate input name"));
    }
    let mut slots = [false; 256];
    let mut sections = sections.to_vec();
    sections.sort_unstable_by(|a, b| a.0.cmp(b.0));
    if sections.windows(2).any(|p| p[0].0 == p[1].0) {
        return Err(Error::Format("duplicate section name"));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    word(&mut out, inputs.len())?;
    word(&mut out, settings.len())?;
    word(&mut out, sections.len())?;
    for (name, slot) in inputs {
        if core::mem::replace(&mut slots[slot as usize], true) {
            return Err(Error::Format("duplicate input slot"));
        }
        put_name(&mut out, name)?;
        out.push(slot);
    }
    out.extend_from_slice(settings);
    for (name, bytes) in sections {
        put_name(&mut out, name)?;
        word(&mut out, bytes.len())?;
        out.extend_from_slice(bytes);
    }
    Ok(out)
}
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    table: &'a [u8],
    count: usize,
    settings: &'a [u8],
    sections: &'a [u8],
    section_count: usize,
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(Error::Format("length overflow"))?;
        let result = self
            .bytes
            .get(self.pos..end)
            .ok_or(Error::Format("truncated"))?;
        self.pos = end;
        Ok(result)
    }
    fn word(&mut self) -> Result<usize, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("fixed width")) as usize)
    }
    fn name(&mut self) -> Result<&'a str, Error> {
        let len = self.take(1)?[0] as usize;
        let bytes = self.take(len)?;
        if !valid_name(bytes) {
            return Err(Error::Format("invalid name"));
        }
        Ok(core::str::from_utf8(bytes).expect("ASCII checked"))
    }
}
impl<'a> Frame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(8)? != MAGIC {
            return Err(Error::Format("bad magic"));
        }
        if r.word()? != VERSION as usize {
            return Err(Error::Format("unsupported version"));
        }
        let count = r.word()?;
        let settings_len = r.word()?;
        let section_count = r.word()?;
        if count > erhino_shared::proc::PROCESS_MAX_GRANTS || count > (bytes.len() - r.pos) / 3 {
            return Err(Error::Format("too many inputs"));
        }
        let mut previous = None;
        let mut slots = [false; 256];
        for _ in 0..count {
            let name = r.name()?;
            let slot = r.take(1)?[0];
            if previous.is_some_and(|p| p >= name) {
                return Err(Error::Format("inputs not strictly ordered"));
            }
            if core::mem::replace(&mut slots[slot as usize], true) {
                return Err(Error::Format("duplicate input slot"));
            }
            previous = Some(name);
        }
        let table = &bytes[HEADER..r.pos];
        let settings = r.take(settings_len)?;
        let section_start = r.pos;
        if section_count > (bytes.len() - r.pos) / 6 {
            return Err(Error::Format("too many sections"));
        }
        let mut previous = None;
        for _ in 0..section_count {
            let name = r.name()?;
            if previous.is_some_and(|p| p >= name) {
                return Err(Error::Format("sections not strictly ordered"));
            }
            previous = Some(name);
            let len = r.word()?;
            r.take(len)?;
        }
        if r.pos != bytes.len() {
            return Err(Error::Format("trailing bytes"));
        }
        Ok(Self {
            table,
            count,
            settings,
            sections: &bytes[section_start..],
            section_count,
        })
    }
    pub fn inputs(&self) -> impl Iterator<Item = (&'a str, u8)> + 'a {
        let mut r = Reader {
            bytes: self.table,
            pos: 0,
        };
        (0..self.count).map(move |_| {
            (
                r.name().expect("validated name"),
                r.take(1).expect("validated slot")[0],
            )
        })
    }
    pub fn sections(&self) -> impl Iterator<Item = (&'a str, &'a [u8])> + 'a {
        let mut r = Reader {
            bytes: self.sections,
            pos: 0,
        };
        (0..self.section_count).map(move |_| {
            let name = r.name().expect("validated name");
            let len = r.word().expect("validated length");
            (name, r.take(len).expect("validated section"))
        })
    }
    pub fn slot(&self, name: &str) -> Option<u8> {
        self.inputs().find(|(n, _)| *n == name).map(|(_, s)| s)
    }
    pub fn settings(&self) -> &'a [u8] {
        self.settings
    }
}
fn claim_data<'a>(
    frame: Frame<'a>,
    taken: &mut [bool],
    name: &str,
) -> Result<Option<&'a [u8]>, Error> {
    let Some((i, (_, data))) = frame.sections().enumerate().find(|(_, (n, _))| *n == name) else {
        return Ok(None);
    };
    if core::mem::replace(&mut taken[i], true) {
        return Err(Error::Grants("section taken twice"));
    }
    Ok(Some(data))
}
fn finish_data(taken: &[bool]) -> Result<(), Error> {
    if taken.iter().any(|b| !b) {
        Err(Error::Grants("section left untaken"))
    } else {
        Ok(())
    }
}

/// 子进程侧的出生采用：按名字取用初始授权，`finish` 拒绝残留。
pub struct Birth {
    process: CurrentProcessInfo,
    grants: InitialGrants,
    frame: Frame<'static>,
    taken: Vec<bool>,
    data_taken: Vec<bool>,
}

impl Birth {
    /// 采用 runtime 交出的出生上下文。payload 映射在进程生命期内有效。
    pub fn adopt(context: StartupContext) -> Result<Self, Error> {
        let (process, payload, grants) = context.into_parts();
        let bytes: &'static [u8] = payload.as_static_bytes();
        let frame = Frame::parse(bytes)?;
        if frame
            .inputs()
            .any(|(_, slot)| slot as usize >= grants.len())
        {
            return Err(Error::Grants("input slot outside initial grants"));
        }
        Ok(Self {
            process,
            grants,
            taken: alloc::vec![false; frame.count],
            data_taken: alloc::vec![false; frame.section_count],
            frame,
        })
    }

    pub fn process(&self) -> CurrentProcessInfo {
        self.process
    }

    pub fn settings(&self) -> &'static [u8] {
        self.frame.settings()
    }

    /// 一次领取通用数据；不存在返回 None，重复领取报错。
    pub fn take_data(&mut self, name: &str) -> Result<Option<&'static [u8]>, Error> {
        claim_data(self.frame, &mut self.data_taken, name)
    }

    fn position(&self, name: &str) -> Option<(usize, u8)> {
        self.frame
            .inputs()
            .enumerate()
            .find(|(_, (n, _))| *n == name)
            .map(|(index, (_, slot))| (index, slot))
    }

    /// 取用一个输入并核实际种类与所需权利；缺省输入返回 `None`。
    pub fn take(
        &mut self,
        name: &str,
        kind: ObjectKind,
        rights: Rights,
    ) -> Result<Option<Capability>, Error> {
        self.take_any(name, &[kind], rights)
    }

    /// 取用一个输入，实际种类须属于 `kinds` 之一。
    fn take_any(
        &mut self,
        name: &str,
        kinds: &[ObjectKind],
        rights: Rights,
    ) -> Result<Option<Capability>, Error> {
        let Some((index, slot)) = self.position(name) else {
            return Ok(None);
        };
        if core::mem::replace(&mut self.taken[index], true) {
            return Err(Error::Grants("input taken twice"));
        }
        let capability = self
            .grants
            .take(slot as usize)
            .ok_or(Error::Grants("input grant missing"))?;
        let description = capability.description().map_err(Error::Query)?;
        if !kinds.iter().any(|kind| description.kind == *kind as u32) {
            return Err(Error::WrongKind);
        }
        if !description.rights.contains(rights) {
            return Err(Error::RightsDenied);
        }
        Ok(Some(capability))
    }

    /// 取用必需输入。
    pub fn require(
        &mut self,
        name: &str,
        kind: ObjectKind,
        rights: Rights,
    ) -> Result<Capability, Error> {
        self.take(name, kind, rights)?
            .ok_or(Error::Grants("required input missing"))
    }

    /// 取用列表输入 `<prefix>.0`、`<prefix>.1`…，直到第一个缺省序号；列表须连续。
    /// 每项实际种类须属于 `kinds` 之一（如平台根的 MMIO/IRQ/DMA 控制）。
    pub fn take_list(
        &mut self,
        prefix: &str,
        kinds: &[ObjectKind],
        rights: Rights,
    ) -> Result<Vec<Capability>, Error> {
        let mut items = Vec::new();
        let mut name = alloc::string::String::new();
        loop {
            name.clear();
            core::fmt::write(&mut name, format_args!("{prefix}.{}", items.len()))
                .expect("String write cannot fail");
            match self.take_any(&name, kinds, rights)? {
                Some(capability) => items.push(capability),
                None => return Ok(items),
            }
        }
    }

    /// 结束采用：帧中每个输入都已取用，初始授权中除 runtime 采用的入口外不留残余。
    pub fn finish(self) -> Result<(), Error> {
        finish_data(&self.data_taken)?;
        if self.taken.iter().any(|taken| !taken) {
            return Err(Error::Grants("input left untaken"));
        }
        if self.grants.remaining() != 0 {
            return Err(Error::Grants("grant not named by the birth frame"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_roundtrip_sections_and_settings_are_independent() {
        let a = encode(
            &[("z", 1), ("a", 0)],
            b"settings",
            &[("z", b"Z"), ("a", b"A")],
        )
        .unwrap();
        let b = encode(
            &[("a", 0), ("z", 1)],
            b"settings",
            &[("a", b"A"), ("z", b"Z")],
        )
        .unwrap();
        assert_eq!(a, b);
        let f = Frame::parse(&a).unwrap();
        assert_eq!(f.inputs().collect::<Vec<_>>(), [("a", 0), ("z", 1)]);
        assert_eq!(
            f.sections().collect::<Vec<_>>(),
            [("a", &b"A"[..]), ("z", &b"Z"[..])]
        );
        assert_eq!(f.settings(), b"settings");
        assert_eq!(f.slot("z"), Some(1));
    }
    #[test]
    fn sections_are_claimed_once_and_finish_rejects_leftovers() {
        let bytes = encode(&[], &[], &[("a", b"A"), ("b", b"")]).unwrap();
        let f = Frame::parse(&bytes).unwrap();
        let mut taken = [false; 2];
        assert!(finish_data(&taken).is_err());
        assert_eq!(claim_data(f, &mut taken, "absent"), Ok(None));
        assert_eq!(claim_data(f, &mut taken, "a"), Ok(Some(&b"A"[..])));
        assert!(claim_data(f, &mut taken, "a").is_err());
        assert!(finish_data(&taken).is_err());
        assert_eq!(claim_data(f, &mut taken, "b"), Ok(Some(&b""[..])));
        assert_eq!(finish_data(&taken), Ok(()));
    }
    #[test]
    fn malformed_and_legacy_frames_rejected() {
        assert!(encode(&[("a", 0), ("a", 1)], &[], &[]).is_err());
        assert!(encode(&[("a", 0), ("b", 0)], &[], &[]).is_err());
        for name in ["", "a b", "雪"] {
            assert!(encode(&[(name, 0)], &[], &[]).is_err());
            assert!(encode(&[], &[], &[(name, b"")]).is_err());
        }
        assert!(encode(&[], &[], &[("a", b""), ("a", b"")]).is_err());
        let good = encode(&[("a", 0), ("b", 1)], b"xy", &[("data", b"payload")]).unwrap();
        for n in 0..good.len() {
            assert!(Frame::parse(&good[..n]).is_err());
        }
        let mut old = good.clone();
        old[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert!(Frame::parse(&old).is_err());
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(Frame::parse(&trailing).is_err());
        let mut unordered = good.clone();
        unordered[25] = b'b';
        unordered[28] = b'a';
        assert!(Frame::parse(&unordered).is_err());
        let mut duplicate_slot = good;
        duplicate_slot[29] = 0;
        assert!(Frame::parse(&duplicate_slot).is_err());
    }
}
