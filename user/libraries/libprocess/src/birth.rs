//! 命名出生输入：启动者在 payload 帧中写入“输入名 → 初始授权槽号”表与实例设定，
//! 子进程按名字取用能力并在采用结束时拒绝未取用的输入。
//!
//! 帧（小端）：`ERBIRTH\0`、u32 版本、u32 输入数、u32 设定长度，随后每项
//! `u8 名长、名字、u8 槽号`，最后是设定字节。输入按名字字节序严格递增，名字与槽号
//! 均不重复，故同一组输入只有一种编码。能力的种类与权利不写入帧，采用时以内核
//! query 的实际结果为准。名字语法由组态拥有，本模块只要求非空 ASCII 且不超过 32 字节。

use alloc::vec::Vec;

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectKind, Rights},
};
use libkernel::ipc::capability::Capability;

use crate::context::{CurrentProcessInfo, InitialGrants, StartupContext};

pub const MAGIC: [u8; 8] = *b"ERBIRTH\0";
pub const VERSION: u32 = 1;
pub const MAX_NAME: usize = 32;
const HEADER: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// 帧格式错误。
    Format(&'static str),
    /// 输入表与实际初始授权不一致：缺失、重复取用或残留。
    Grants(&'static str),
    /// 输入的实际对象类型不符。
    WrongKind,
    /// 输入的实际权利不覆盖所需权利。
    RightsDenied,
    /// 查询能力描述失败。
    Query(SystemCallError),
}

fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.iter().all(|b| b.is_ascii_graphic())
}

/// 编码出生帧；`inputs` 为（输入名，初始授权槽号），顺序不限。
pub fn encode(inputs: &[(&str, u8)], settings: &[u8]) -> Result<Vec<u8>, Error> {
    let mut sorted: Vec<(&str, u8)> = inputs.to_vec();
    sorted.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    for pair in sorted.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(Error::Format("duplicate input name"));
        }
    }
    let mut slots: Vec<u8> = sorted.iter().map(|(_, slot)| *slot).collect();
    slots.sort_unstable();
    if slots.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::Format("duplicate input slot"));
    }
    let count = u32::try_from(sorted.len()).map_err(|_| Error::Format("too many inputs"))?;
    let length = u32::try_from(settings.len()).map_err(|_| Error::Format("settings too long"))?;
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&length.to_le_bytes());
    for (name, slot) in sorted {
        if !valid_name(name.as_bytes()) {
            return Err(Error::Format("invalid input name"));
        }
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(slot);
    }
    out.extend_from_slice(settings);
    Ok(out)
}

/// 已校验的出生帧视图。
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    table: &'a [u8],
    count: usize,
    settings: &'a [u8],
}

impl<'a> Frame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER || bytes[..8] != MAGIC {
            return Err(Error::Format("bad magic"));
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        if word(8) != VERSION {
            return Err(Error::Format("unsupported version"));
        }
        let count = word(12) as usize;
        let settings_len = word(16) as usize;
        let mut pos = HEADER;
        let mut previous: Option<&[u8]> = None;
        let mut slots = [false; 256];
        for _ in 0..count {
            let len = *bytes.get(pos).ok_or(Error::Format("truncated"))? as usize;
            let name = bytes
                .get(pos + 1..pos + 1 + len)
                .ok_or(Error::Format("truncated"))?;
            let slot = *bytes.get(pos + 1 + len).ok_or(Error::Format("truncated"))?;
            if !valid_name(name) {
                return Err(Error::Format("invalid input name"));
            }
            if previous.is_some_and(|prev| prev >= name) {
                return Err(Error::Format("inputs not strictly ordered"));
            }
            if core::mem::replace(&mut slots[slot as usize], true) {
                return Err(Error::Format("duplicate input slot"));
            }
            previous = Some(name);
            pos += 2 + len;
        }
        let table = &bytes[HEADER..pos];
        let settings = bytes
            .get(
                pos..pos
                    .checked_add(settings_len)
                    .ok_or(Error::Format("truncated"))?,
            )
            .ok_or(Error::Format("truncated"))?;
        if pos + settings_len != bytes.len() {
            return Err(Error::Format("trailing bytes"));
        }
        Ok(Self {
            table,
            count,
            settings,
        })
    }

    /// 全部（输入名，槽号），按名字字节序。
    pub fn inputs(&self) -> impl Iterator<Item = (&'a str, u8)> + 'a {
        let mut pos = 0;
        let table = self.table;
        (0..self.count).map(move |_| {
            let len = table[pos] as usize;
            let name = core::str::from_utf8(&table[pos + 1..pos + 1 + len]).expect("ASCII checked");
            let slot = table[pos + 1 + len];
            pos += 2 + len;
            (name, slot)
        })
    }

    pub fn slot(&self, name: &str) -> Option<u8> {
        self.inputs()
            .find(|(n, _)| *n == name)
            .map(|(_, slot)| slot)
    }

    /// 实例设定的原始字节（`settings` 编码），由服务自行解析。
    pub fn settings(&self) -> &'a [u8] {
        self.settings
    }
}

/// 子进程侧的出生采用：按名字取用初始授权，`finish` 拒绝残留。
pub struct Birth {
    process: CurrentProcessInfo,
    grants: InitialGrants,
    frame: Frame<'static>,
    taken: Vec<bool>,
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
            frame,
        })
    }

    pub fn process(&self) -> CurrentProcessInfo {
        self.process
    }

    pub fn settings(&self) -> &'static [u8] {
        self.frame.settings()
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
    fn frame_round_trips_in_canonical_order() {
        let bytes = encode(
            &[
                ("memory.pool", 2),
                ("instance.report", 0),
                ("instance.stop", 1),
            ],
            b"S",
        )
        .unwrap();
        let frame = Frame::parse(&bytes).unwrap();
        let inputs: Vec<_> = frame.inputs().collect();
        assert_eq!(
            inputs,
            [
                ("instance.report", 0),
                ("instance.stop", 1),
                ("memory.pool", 2)
            ]
        );
        assert_eq!(frame.slot("memory.pool"), Some(2));
        assert_eq!(frame.slot("absent"), None);
        assert_eq!(frame.settings(), b"S");
        let again = encode(
            &[
                ("instance.stop", 1),
                ("memory.pool", 2),
                ("instance.report", 0),
            ],
            b"S",
        )
        .unwrap();
        assert_eq!(bytes, again);
    }

    #[test]
    fn malformed_frames_are_rejected() {
        assert!(encode(&[("a", 0), ("a", 1)], &[]).is_err());
        assert!(encode(&[("a", 0), ("b", 0)], &[]).is_err());
        assert!(encode(&[("", 0)], &[]).is_err());
        let good = encode(&[("a", 0), ("b", 1)], b"xy").unwrap();
        assert!(Frame::parse(&good[..good.len() - 1]).is_err());
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(Frame::parse(&trailing).is_err());
        let mut magic = good.clone();
        magic[0] ^= 1;
        assert!(Frame::parse(&magic).is_err());
        let mut unordered = good.clone();
        // 交换两个单字节名字，使表不再递增。
        unordered[21] = b'b';
        unordered[24] = b'a';
        assert_eq!(
            Frame::parse(&unordered).err(),
            Some(Error::Format("inputs not strictly ordered"))
        );
        let mut slot = good;
        slot[25] = 0;
        assert_eq!(
            Frame::parse(&slot).err(),
            Some(Error::Format("duplicate input slot"))
        );
    }
}
