//! 设备树 blob 就地访问（libfdt 范式）。
//!
//! 不构建 DOM：所有访问以游标在 blob 原始字节上推进，零分配、
//! 零拷贝。[`Fdt::new`] 做一次全树语法验证（token 文法 + 越界 +
//! 字符串表定界），通过后所有访问器在验证过的区间内推进，错误
//! 不可达——访问器仍以 `Option` 表达「不存在」，不携带错误分支。
//!
//! u32 一律大端（DTB 规范）；节点名/属性名是 NUL 结尾的 ASCII。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use core::{fmt, str};

pub mod topology;

const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_BEGIN_NODE: u32 = 0x1;
const FDT_END_NODE: u32 = 0x2;
const FDT_PROP: u32 = 0x3;
const FDT_NOP: u32 = 0x4;
const FDT_END: u32 = 0x9;

/// 头部固定 40 字节。
const HEADER_LEN: usize = 40;

/// 解析错误。启动路径遇错即致命，不区分恢复策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FdtError {
    /// 魔数不符。
    BadMagic,
    /// blob 截断：偏移或长度越过数据末尾。
    Truncated,
    /// token 序列不合法（如结构块不以 BEGIN_NODE 开头、END 后仍有内容）。
    UnexpectedToken,
    /// 字符串非法 UTF-8。
    InvalidUtf8,
}

impl fmt::Display for FdtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "设备树魔数不符"),
            Self::Truncated => write!(f, "设备树数据截断"),
            Self::UnexpectedToken => write!(f, "设备树 token 序列非法"),
            Self::InvalidUtf8 => write!(f, "设备树字符串非 UTF-8"),
        }
    }
}

fn be32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// 结构块内的语法项。
#[derive(Debug)]
enum Item<'a> {
    BeginNode(&'a str),
    EndNode,
    Prop { name: &'a str, data: &'a [u8] },
    Nop,
    End,
}

/// 设备树 blob 的已验证视图。
#[derive(Debug)]
pub struct Fdt<'a> {
    data: &'a [u8],
    dt_struct: core::ops::Range<usize>,
    dt_strings: core::ops::Range<usize>,
}

impl<'a> Fdt<'a> {
    /// 验证并创建视图：魔数、块边界、全树 token 文法。
    pub fn new(data: &'a [u8]) -> Result<Self, FdtError> {
        if be32(data, 0) != Some(FDT_MAGIC) {
            return Err(FdtError::BadMagic);
        }
        let totalsize = be32(data, 4).ok_or(FdtError::Truncated)? as usize;
        if totalsize < HEADER_LEN || totalsize > data.len() {
            return Err(FdtError::Truncated);
        }
        let off_struct = be32(data, 8).ok_or(FdtError::Truncated)? as usize;
        let off_strings = be32(data, 12).ok_or(FdtError::Truncated)? as usize;
        let size_strings = be32(data, 32).ok_or(FdtError::Truncated)? as usize;
        let size_struct = be32(data, 36).ok_or(FdtError::Truncated)? as usize;

        let dt_struct = off_struct..off_struct.checked_add(size_struct).ok_or(FdtError::Truncated)?;
        let dt_strings = off_strings..off_strings.checked_add(size_strings).ok_or(FdtError::Truncated)?;
        if dt_struct.end > totalsize || dt_strings.end > totalsize || dt_struct.is_empty() {
            return Err(FdtError::Truncated);
        }

        let fdt = Self {
            data,
            dt_struct,
            dt_strings,
        };

        // 全树验证：根 BEGIN、整棵子树闭合、其后仅 NOP* 与 END
        let mut pos = fdt.dt_struct.start;
        match fdt.item_at(&mut pos)? {
            Item::BeginNode(_) => {}
            _ => return Err(FdtError::UnexpectedToken),
        }
        pos = fdt.skip_subtree(fdt.dt_struct.start)?;
        loop {
            match fdt.item_at(&mut pos)? {
                Item::Nop => {}
                Item::End => break,
                _ => return Err(FdtError::UnexpectedToken),
            }
        }

        Ok(fdt)
    }

    /// 根节点。
    pub fn root(&self) -> Node<'_, 'a> {
        Node {
            fdt: self,
            off: self.dt_struct.start,
        }
    }

    /// 读 `pos` 处的 token 并推进。所有偏移经 `get` 越界检查。
    fn item_at(&self, pos: &mut usize) -> Result<Item<'a>, FdtError> {
        let tok = be32(self.data, *pos).ok_or(FdtError::Truncated)?;
        *pos += 4;
        match tok {
            FDT_BEGIN_NODE => {
                let start = *pos;
                let mut end = start;
                while end < self.data.len() && self.data[end] != 0 {
                    end += 1;
                }
                if end >= self.data.len() {
                    return Err(FdtError::Truncated);
                }
                let name = str::from_utf8(&self.data[start..end]).map_err(|_| FdtError::InvalidUtf8)?;
                *pos = align4(end + 1);
                Ok(Item::BeginNode(name))
            }
            FDT_END_NODE => Ok(Item::EndNode),
            FDT_PROP => {
                let len = be32(self.data, *pos).ok_or(FdtError::Truncated)? as usize;
                *pos += 4;
                let nameoff = be32(self.data, *pos).ok_or(FdtError::Truncated)? as usize;
                *pos += 4;
                let data_start = *pos;
                let data_end = data_start.checked_add(len).ok_or(FdtError::Truncated)?;
                if data_end > self.data.len() {
                    return Err(FdtError::Truncated);
                }
                let name = self.string_at(nameoff)?;
                *pos = align4(data_end);
                Ok(Item::Prop {
                    name,
                    data: &self.data[data_start..data_end],
                })
            }
            FDT_NOP => Ok(Item::Nop),
            FDT_END => Ok(Item::End),
            _ => Err(FdtError::UnexpectedToken),
        }
    }

    /// 字符串表中 `nameoff` 处的 NUL 结尾字符串。
    fn string_at(&self, nameoff: usize) -> Result<&'a str, FdtError> {
        let start = self
            .dt_strings
            .start
            .checked_add(nameoff)
            .filter(|s| *s < self.dt_strings.end)
            .ok_or(FdtError::Truncated)?;
        let mut end = start;
        while end < self.dt_strings.end && self.data[end] != 0 {
            end += 1;
        }
        if end == self.dt_strings.end {
            return Err(FdtError::Truncated);
        }
        str::from_utf8(&self.data[start..end]).map_err(|_| FdtError::InvalidUtf8)
    }

    /// 跳过 `begin_off`（指向 BEGIN_NODE token）起的整棵子树，
    /// 返回其 END_NODE 之后的位置。迭代实现，深度任意。
    fn skip_subtree(&self, begin_off: usize) -> Result<usize, FdtError> {
        let mut pos = begin_off;
        let _ = self.item_at(&mut pos)?; // 消费 BEGIN + 节点名
        let mut depth = 1usize;
        while depth > 0 {
            match self.item_at(&mut pos)? {
                Item::BeginNode(_) => depth += 1,
                Item::EndNode => depth -= 1,
                Item::End => return Err(FdtError::UnexpectedToken),
                _ => {}
            }
        }
        Ok(pos)
    }
}

/// 树节点视图：`off` 指向该节点的 BEGIN_NODE token。
/// `'a` 为 blob 数据生命周期，`'f` 为对 [`Fdt`] 的借用。
pub struct Node<'f, 'a: 'f> {
    fdt: &'f Fdt<'a>,
    off: usize,
}

impl<'f, 'a: 'f> Node<'f, 'a> {
    /// 节点名（根为空串；单元地址含在内，如 `cpu@0`）。
    pub fn name(&self) -> Result<&'a str, FdtError> {
        let mut pos = self.off;
        match self.fdt.item_at(&mut pos)? {
            Item::BeginNode(name) => Ok(name),
            _ => Err(FdtError::UnexpectedToken),
        }
    }

    /// 本节点的属性原始字节。同名属性多次出现时取首个。
    pub fn prop(&self, name: &str) -> Option<&'a [u8]> {
        self.each_prop(|n, data| if n == name { Some(data) } else { None })
    }

    /// 属性值为 NUL 结尾字符串（不含结尾 NUL）。
    pub fn prop_str(&self, name: &str) -> Option<&'a str> {
        let data = self.prop(name)?;
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        str::from_utf8(&data[..end]).ok()
    }

    /// 属性值为单个 u32（大端）。
    pub fn prop_u32(&self, name: &str) -> Option<u32> {
        let data = self.prop(name)?;
        be32(data, 0)
    }

    /// 属性值为字符串列表（DT 规范 string-array：NUL 结尾字符串依此相连，
    /// 列表以数据末尾或空段结束）。任一段非合法 UTF-8 视为属性缺失——
    /// 与全树验证同哲学：错误在访问器边界收敛为「不存在」。
    pub fn prop_str_list(&self, name: &str) -> Option<StringList<'a>> {
        let data = self.prop(name)?;
        let mut pos = 0;
        while pos < data.len() {
            let end = pos + data[pos..].iter().position(|&b| b == 0)?; // 每段必须 NUL 结尾
            str::from_utf8(&data[pos..end]).ok()?;
            let step = end - pos + 1;
            if step == 1 {
                break; // 空段：列表终止
            }
            pos += step;
        }
        Some(StringList { data, pos: 0 })
    }

    /// 首个名为 `name` 的直接子节点。
    pub fn child(&self, name: &str) -> Option<Node<'f, 'a>> {
        self.children().find(|n| n.name().is_ok_and(|n| n == name))
    }

    /// 直接子节点迭代（本节点成员中的全部 BEGIN_NODE，子树已跳过）。
    pub fn children(&self) -> Children<'f, 'a> {
        let mut pos = self.off;
        // 消费自身 BEGIN+名字，pos 落在首个成员
        let _ = self.fdt.item_at(&mut pos);
        Children {
            fdt: self.fdt,
            pos,
            done: false,
        }
    }

    /// 按声明序访问本节点属性直到回调返回 `Some`。
    fn each_prop<R>(&self, mut f: impl FnMut(&'a str, &'a [u8]) -> Option<R>) -> Option<R> {
        let mut pos = self.off;
        let _ = self.fdt.item_at(&mut pos).ok()?; // 自身 BEGIN
        loop {
            match self.fdt.item_at(&mut pos).ok()? {
                Item::Prop { name, data } => {
                    if let Some(r) = f(name, data) {
                        return Some(r);
                    }
                }
                // 子节点开始或本节点结束：属性区结束（已验证文法，
                // 防御性继续扫描子树外无属性，直接终止）
                Item::BeginNode(_) | Item::EndNode | Item::End => return None,
                Item::Nop => {}
            }
        }
    }
}

/// 子节点游标。
pub struct Children<'f, 'a: 'f> {
    fdt: &'f Fdt<'a>,
    pos: usize,
    done: bool,
}

impl<'f, 'a: 'f> Iterator for Children<'f, 'a> {
    type Item = Node<'f, 'a>;

    fn next(&mut self) -> Option<Node<'f, 'a>> {
        if self.done {
            return None;
        }
        loop {
            let start = self.pos;
            match self.fdt.item_at(&mut self.pos).ok()? {
                Item::Nop => {}
                // 属性理论上先于子节点；防御性跳过（文法已验证）
                Item::Prop { .. } => {}
                Item::BeginNode(_) => {
                    // 跳过这棵子树，pos 落到其 END 之后（即兄弟 token）
                    self.pos = self.fdt.skip_subtree(start).ok()?;
                    return Some(Node {
                        fdt: self.fdt,
                        off: start,
                    });
                }
                Item::EndNode | Item::End => {
                    self.done = true;
                    return None;
                }
            }
        }
    }
}

/// 字符串列表视图（[`Node::prop_str_list`] 产生）。
#[derive(Debug, Clone, Copy)]
pub struct StringList<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for StringList<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.pos >= self.data.len() {
            return None;
        }
        let end = self.pos + self.data[self.pos..].iter().position(|&b| b == 0)?;
        let s = str::from_utf8(&self.data[self.pos..end]).ok()?;
        if s.is_empty() {
            return None; // 空段：列表终止
        }
        self.pos = end + 1;
        Some(s)
    }
}

/// 从属性字节中取前 `cells` 个 cell（每 cell 4 字节大端）拼成 u64。
/// `cells` 为 0..=2；宽度不足返回 `None`。
pub fn cells_u64(data: &[u8], cells: usize) -> Option<u64> {
    match cells {
        0 => Some(0),
        1 => be32(data, 0).map(|v| v as u64),
        2 => {
            let hi = be32(data, 0)?;
            let lo = be32(data, 4)?;
            Some((hi as u64) << 32 | lo as u64)
        }
        _ => None,
    }
}
