//! 实例设定：类型化值树的规范二进制编码与零拷贝只读访问。
//!
//! 设定由组态编译器在 host 侧从组态源生成，经出生帧交给服务；服务按自己的键解释，
//! 本库不认识任何服务键。值只有五种：布尔、整数、字符串、列表、表；根恒为表。
//!
//! 编码（小端）：
//! - 布尔 `1, b`（b 为 0 或 1）；整数 `2, i64`；字符串 `3, u32 长度, UTF-8 字节`；
//! - 列表 `4, u32 项数, u32 体长, 项…`；表 `5, u32 项数, u32 体长, (u8 键长, 键, 值)…`。
//!
//! 表键按字节严格递增、不重复，键为 `[a-z][a-z0-9_]*` 且不超过 32 字节，故同一内容只有一种
//! 编码。[`Settings::parse`] 一次校验整棵树；之后的访问不再失败于格式，只报缺键、类型或值域。

#![cfg_attr(not(test), no_std)]

#[cfg(any(test, feature = "encode"))]
extern crate alloc;

const TAG_BOOL: u8 = 1;
const TAG_INT: u8 = 2;
const TAG_STR: u8 = 3;
const TAG_LIST: u8 = 4;
const TAG_TABLE: u8 = 5;
/// 嵌套上限；解析递归深度由此有界。
pub const MAX_DEPTH: usize = 16;
/// 表键长度上限，与组态名字语法一致。
pub const MAX_KEY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    TrailingBytes,
    BadTag,
    BadBool,
    BadUtf8,
    BadKey,
    KeyOrder,
    LengthMismatch,
    TooDeep,
    TooLong,
    NotTable,
    /// 访问的键不存在。
    Missing,
    /// 值类型与访问不符。
    WrongType,
    /// 整数超出目标类型的值域。
    OutOfRange,
    /// 表中出现调用者未声明的键。
    UnknownKey,
}

fn valid_key(key: &[u8]) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY
        && key[0].is_ascii_lowercase()
        && key
            .iter()
            .all(|&byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn take(bytes: &[u8], pos: usize, len: usize) -> Result<&[u8], Error> {
    let end = pos.checked_add(len).ok_or(Error::Truncated)?;
    bytes.get(pos..end).ok_or(Error::Truncated)
}

fn u32_at(bytes: &[u8], pos: usize) -> Result<usize, Error> {
    let raw = take(bytes, pos, 4)?;
    Ok(u32::from_le_bytes(raw.try_into().expect("four bytes taken")) as usize)
}

/// 校验 `pos` 处的一个值，返回其后的偏移。
fn validate(bytes: &[u8], pos: usize, depth: usize) -> Result<usize, Error> {
    let tag = *bytes.get(pos).ok_or(Error::Truncated)?;
    let pos = pos + 1;
    match tag {
        TAG_BOOL => match take(bytes, pos, 1)?[0] {
            0 | 1 => Ok(pos + 1),
            _ => Err(Error::BadBool),
        },
        TAG_INT => take(bytes, pos, 8).map(|_| pos + 8),
        TAG_STR => {
            let len = u32_at(bytes, pos)?;
            let text = take(bytes, pos + 4, len)?;
            core::str::from_utf8(text).map_err(|_| Error::BadUtf8)?;
            Ok(pos + 4 + len)
        }
        TAG_LIST | TAG_TABLE => {
            if depth >= MAX_DEPTH {
                return Err(Error::TooDeep);
            }
            let count = u32_at(bytes, pos)?;
            let body = u32_at(bytes, pos + 4)?;
            let start = pos + 8;
            let end = start.checked_add(body).ok_or(Error::Truncated)?;
            take(bytes, start, body)?;
            let body = &bytes[..end];
            let mut at = start;
            let mut previous: Option<&[u8]> = None;
            for _ in 0..count {
                if tag == TAG_TABLE {
                    let len = *body.get(at).ok_or(Error::LengthMismatch)? as usize;
                    let key = take(body, at + 1, len).map_err(|_| Error::LengthMismatch)?;
                    if !valid_key(key) {
                        return Err(Error::BadKey);
                    }
                    if previous.is_some_and(|previous| previous >= key) {
                        return Err(Error::KeyOrder);
                    }
                    previous = Some(key);
                    at += 1 + len;
                }
                at = validate(body, at, depth + 1).map_err(|error| match error {
                    Error::Truncated => Error::LengthMismatch,
                    other => other,
                })?;
            }
            if at != end {
                return Err(Error::LengthMismatch);
            }
            Ok(end)
        }
        _ => Err(Error::BadTag),
    }
}

/// 已校验的值；格式错误在 [`Settings::parse`] 时已经排除。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    Bool(bool),
    Int(i64),
    Str(&'a str),
    List(List<'a>),
    Table(Table<'a>),
}

/// 解码已校验的值；只用于校验通过的字节，故格式失败不可达。
fn decode(bytes: &[u8], pos: usize) -> (Value<'_>, usize) {
    const VALIDATED: &str = "settings validated at parse";
    let pos = pos + 1;
    match bytes[pos - 1] {
        TAG_BOOL => (Value::Bool(bytes[pos] == 1), pos + 1),
        TAG_INT => (
            Value::Int(i64::from_le_bytes(
                bytes[pos..pos + 8].try_into().expect(VALIDATED),
            )),
            pos + 8,
        ),
        TAG_STR => {
            let len = u32_at(bytes, pos).expect(VALIDATED);
            let text = core::str::from_utf8(&bytes[pos + 4..pos + 4 + len]).expect(VALIDATED);
            (Value::Str(text), pos + 4 + len)
        }
        tag => {
            let count = u32_at(bytes, pos).expect(VALIDATED);
            let len = u32_at(bytes, pos + 4).expect(VALIDATED);
            let body = &bytes[pos + 8..pos + 8 + len];
            let next = pos + 8 + len;
            if tag == TAG_LIST {
                (Value::List(List { count, body }), next)
            } else {
                (Value::Table(Table { count, body }), next)
            }
        }
    }
}

impl<'a> Value<'a> {
    pub fn as_bool(self) -> Result<bool, Error> {
        match self {
            Self::Bool(value) => Ok(value),
            _ => Err(Error::WrongType),
        }
    }
    pub fn as_int(self) -> Result<i64, Error> {
        match self {
            Self::Int(value) => Ok(value),
            _ => Err(Error::WrongType),
        }
    }
    /// 整数转为目标类型；超出值域报 [`Error::OutOfRange`]。
    pub fn as_integer<T: TryFrom<i64>>(self) -> Result<T, Error> {
        T::try_from(self.as_int()?).map_err(|_| Error::OutOfRange)
    }
    pub fn as_str(self) -> Result<&'a str, Error> {
        match self {
            Self::Str(value) => Ok(value),
            _ => Err(Error::WrongType),
        }
    }
    pub fn as_list(self) -> Result<List<'a>, Error> {
        match self {
            Self::List(value) => Ok(value),
            _ => Err(Error::WrongType),
        }
    }
    pub fn as_table(self) -> Result<Table<'a>, Error> {
        match self {
            Self::Table(value) => Ok(value),
            _ => Err(Error::WrongType),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct List<'a> {
    count: usize,
    body: &'a [u8],
}

impl<'a> List<'a> {
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = Value<'a>> + 'a {
        let body = self.body;
        let mut at = 0;
        (0..self.count).map(move |_| {
            let (value, next) = decode(body, at);
            at = next;
            value
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table<'a> {
    count: usize,
    body: &'a [u8],
}

impl<'a> Table<'a> {
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    /// 按键序遍历 `(键, 值)`。
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, Value<'a>)> + 'a {
        let body = self.body;
        let mut at = 0;
        (0..self.count).map(move |_| {
            let len = body[at] as usize;
            let key = core::str::from_utf8(&body[at + 1..at + 1 + len])
                .expect("settings keys validated as ASCII");
            let (value, next) = decode(body, at + 1 + len);
            at = next;
            (key, value)
        })
    }
    pub fn get(&self, key: &str) -> Option<Value<'a>> {
        self.iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(_, value)| value)
    }
    pub fn value(&self, key: &str) -> Result<Value<'a>, Error> {
        self.get(key).ok_or(Error::Missing)
    }
    pub fn bool(&self, key: &str) -> Result<bool, Error> {
        self.value(key)?.as_bool()
    }
    pub fn integer<T: TryFrom<i64>>(&self, key: &str) -> Result<T, Error> {
        self.value(key)?.as_integer()
    }
    pub fn str(&self, key: &str) -> Result<&'a str, Error> {
        self.value(key)?.as_str()
    }
    pub fn list(&self, key: &str) -> Result<List<'a>, Error> {
        self.value(key)?.as_list()
    }
    pub fn table(&self, key: &str) -> Result<Table<'a>, Error> {
        self.value(key)?.as_table()
    }
    /// 拒绝 `known` 以外的键，使设定拼写错误在采用时失败而不是被静默忽略。
    pub fn only(&self, known: &[&str]) -> Result<(), Error> {
        if self.iter().all(|(key, _)| known.contains(&key)) {
            Ok(())
        } else {
            Err(Error::UnknownKey)
        }
    }
}

/// 一份已整体校验的设定；根为表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings<'a> {
    root: Table<'a>,
}

impl<'a> Settings<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.first() != Some(&TAG_TABLE) {
            return Err(if bytes.is_empty() {
                Error::Truncated
            } else {
                Error::NotTable
            });
        }
        if validate(bytes, 0, 0)? != bytes.len() {
            return Err(Error::TrailingBytes);
        }
        let (root, _) = decode(bytes, 0);
        Ok(Self {
            root: root.as_table().expect("root tag checked"),
        })
    }
    pub fn root(&self) -> Table<'a> {
        self.root
    }
}

#[cfg(any(test, feature = "encode"))]
mod encode {
    use super::*;
    use alloc::{collections::BTreeMap, string::String, vec::Vec};

    /// host 侧构造的设定值；表用有序映射，保证编码规范唯一。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Owned {
        Bool(bool),
        Int(i64),
        Str(String),
        List(Vec<Owned>),
        Table(BTreeMap<String, Owned>),
    }

    fn length(len: usize) -> Result<[u8; 4], Error> {
        u32::try_from(len)
            .map(u32::to_le_bytes)
            .map_err(|_| Error::TooLong)
    }

    fn write(value: &Owned, depth: usize, out: &mut Vec<u8>) -> Result<(), Error> {
        match value {
            Owned::Bool(value) => out.extend_from_slice(&[TAG_BOOL, u8::from(*value)]),
            Owned::Int(value) => {
                out.push(TAG_INT);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Owned::Str(text) => {
                out.push(TAG_STR);
                out.extend_from_slice(&length(text.len())?);
                out.extend_from_slice(text.as_bytes());
            }
            Owned::List(items) => {
                let mut body = Vec::new();
                for item in items {
                    nested(depth)?;
                    write(item, depth + 1, &mut body)?;
                }
                container(TAG_LIST, items.len(), &body, depth, out)?;
            }
            Owned::Table(entries) => {
                let mut body = Vec::new();
                for (key, item) in entries {
                    if !valid_key(key.as_bytes()) {
                        return Err(Error::BadKey);
                    }
                    nested(depth)?;
                    body.push(key.len() as u8);
                    body.extend_from_slice(key.as_bytes());
                    write(item, depth + 1, &mut body)?;
                }
                container(TAG_TABLE, entries.len(), &body, depth, out)?;
            }
        }
        Ok(())
    }

    fn nested(depth: usize) -> Result<(), Error> {
        if depth >= MAX_DEPTH {
            Err(Error::TooDeep)
        } else {
            Ok(())
        }
    }

    fn container(
        tag: u8,
        count: usize,
        body: &[u8],
        depth: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), Error> {
        nested(depth)?;
        out.push(tag);
        out.extend_from_slice(&length(count)?);
        out.extend_from_slice(&length(body.len())?);
        out.extend_from_slice(body);
        Ok(())
    }

    /// 编码一份设定；根为表。
    pub fn encode(root: &BTreeMap<String, Owned>) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        write(&Owned::Table(root.clone()), 0, &mut out)?;
        Ok(out)
    }
}

#[cfg(any(test, feature = "encode"))]
pub use encode::{Owned, encode};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{collections::BTreeMap, string::String, vec, vec::Vec};

    fn table(entries: &[(&str, Owned)]) -> BTreeMap<String, Owned> {
        entries
            .iter()
            .map(|(key, value)| (String::from(*key), value.clone()))
            .collect()
    }

    fn sample() -> BTreeMap<String, Owned> {
        let declaration = Owned::Table(table(&[
            ("implementation", Owned::Str("drv_usb_hid".into())),
            ("capacity", Owned::Int(2)),
            ("reusable", Owned::Bool(true)),
        ]));
        table(&[
            ("declarations", Owned::List(vec![declaration])),
            ("registry", Owned::Bool(false)),
            ("watches", Owned::Int(24)),
        ])
    }

    #[test]
    fn round_trip_reads_every_value_kind() {
        let bytes = encode(&sample()).unwrap();
        let root = Settings::parse(&bytes).unwrap().root();
        assert_eq!(root.len(), 3);
        assert_eq!(root.integer::<u32>("watches"), Ok(24));
        assert_eq!(root.bool("registry"), Ok(false));
        let declarations = root.list("declarations").unwrap();
        let first = declarations.iter().next().unwrap().as_table().unwrap();
        assert_eq!(first.str("implementation"), Ok("drv_usb_hid"));
        assert_eq!(first.integer::<u32>("capacity"), Ok(2));
        assert!(first.bool("reusable").unwrap());
        let keys: Vec<_> = root.iter().map(|(key, _)| key).collect();
        assert_eq!(keys, ["declarations", "registry", "watches"]);
    }

    #[test]
    fn access_reports_missing_type_range_and_unknown_keys() {
        let bytes = encode(&table(&[
            ("count", Owned::Int(-1)),
            ("name", Owned::Str("x".into())),
        ]))
        .unwrap();
        let root = Settings::parse(&bytes).unwrap().root();
        assert_eq!(root.bool("absent"), Err(Error::Missing));
        assert_eq!(root.bool("name"), Err(Error::WrongType));
        assert_eq!(root.integer::<u32>("count"), Err(Error::OutOfRange));
        assert_eq!(root.only(&["count"]), Err(Error::UnknownKey));
        assert_eq!(root.only(&["count", "name"]), Ok(()));
    }

    #[test]
    fn encoder_rejects_bad_keys_and_excess_depth() {
        assert_eq!(
            encode(&table(&[("Bad", Owned::Int(1))])),
            Err(Error::BadKey)
        );
        assert_eq!(encode(&table(&[("", Owned::Int(1))])), Err(Error::BadKey));
        let mut deep = Owned::Int(0);
        for _ in 0..MAX_DEPTH {
            deep = Owned::List(vec![deep]);
        }
        assert_eq!(encode(&table(&[("deep", deep)])), Err(Error::TooDeep));
    }

    /// 手工构造的表：`entries` 已含键长与键。
    fn raw_table(count: u32, body: &[u8]) -> Vec<u8> {
        let mut out = vec![TAG_TABLE];
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn parser_rejects_noncanonical_and_malformed_input() {
        let entry = |key: &[u8], value: &[u8]| [&[key.len() as u8], key, value].concat();
        let one = [TAG_BOOL, 1];
        // 键序与重复。
        let unsorted = [entry(b"b", &one), entry(b"a", &one)].concat();
        assert_eq!(
            Settings::parse(&raw_table(2, &unsorted)),
            Err(Error::KeyOrder)
        );
        let duplicate = [entry(b"a", &one), entry(b"a", &one)].concat();
        assert_eq!(
            Settings::parse(&raw_table(2, &duplicate)),
            Err(Error::KeyOrder)
        );
        // 值格式。
        assert_eq!(
            Settings::parse(&raw_table(1, &entry(b"a", &[TAG_BOOL, 2]))),
            Err(Error::BadBool)
        );
        assert_eq!(
            Settings::parse(&raw_table(1, &entry(b"a", &[9]))),
            Err(Error::BadTag)
        );
        let bad_utf8 = [&[TAG_STR][..], &1u32.to_le_bytes(), &[0xff]].concat();
        assert_eq!(
            Settings::parse(&raw_table(1, &entry(b"a", &bad_utf8))),
            Err(Error::BadUtf8)
        );
        assert_eq!(
            Settings::parse(&raw_table(1, &entry(b"A", &one))),
            Err(Error::BadKey)
        );
        // 计数、体长与边界。
        assert_eq!(
            Settings::parse(&raw_table(2, &entry(b"a", &one))),
            Err(Error::LengthMismatch)
        );
        assert_eq!(
            Settings::parse(&raw_table(0, &entry(b"a", &one))),
            Err(Error::LengthMismatch)
        );
        let mut trailing = raw_table(1, &entry(b"a", &one));
        trailing.push(0);
        assert_eq!(Settings::parse(&trailing), Err(Error::TrailingBytes));
        let whole = raw_table(1, &entry(b"a", &one));
        assert_eq!(
            Settings::parse(&whole[..whole.len() - 1]),
            Err(Error::Truncated)
        );
        assert_eq!(Settings::parse(&[]), Err(Error::Truncated));
        assert_eq!(Settings::parse(&one), Err(Error::NotTable));
    }

    #[test]
    fn parser_bounds_nesting_depth() {
        let mut value = vec![TAG_INT];
        value.extend_from_slice(&0i64.to_le_bytes());
        for _ in 0..MAX_DEPTH {
            let mut list = vec![TAG_LIST];
            list.extend_from_slice(&1u32.to_le_bytes());
            list.extend_from_slice(&(value.len() as u32).to_le_bytes());
            list.extend_from_slice(&value);
            value = list;
        }
        let bytes = raw_table(1, &[&[1, b'a'][..], &value].concat());
        assert_eq!(Settings::parse(&bytes), Err(Error::TooDeep));
    }

    #[test]
    fn same_content_has_one_encoding() {
        let mut reversed = BTreeMap::new();
        for (key, value) in sample().into_iter().rev() {
            reversed.insert(key, value);
        }
        assert_eq!(encode(&sample()), encode(&reversed));
    }
}
