//! 瞬态 ObjectInfo 的借用视图；只验证表示，不解释开放标记的领域含义。

use crate::protocol::{BODY_MAX, Status};
use librpc::bytes::{Reader, Writer};

pub const NAMESPACE: u128 = 0x48414c43594f4e00000000004f414c00;
pub const DIRECTORY: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerId {
    pub namespace: u128,
    pub local: u32,
}

impl MarkerId {
    pub const ENCODED_LEN: usize = 20;

    fn read(reader: &mut Reader<'_>) -> Result<Self, Status> {
        Ok(Self {
            namespace: reader.u128()?,
            local: reader.u32()?,
        })
    }

    fn write(self, writer: &mut Writer<'_>) {
        writer.u128(self.namespace);
        writer.u32(self.local);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tag<'a> {
    pub key: MarkerId,
    pub value: &'a [u8],
}

impl<'a> Tag<'a> {
    fn read(reader: &mut Reader<'a>) -> Result<Self, Status> {
        Ok(Self {
            key: MarkerId::read(reader)?,
            value: reader.bytes32()?,
        })
    }
}

/// 已完整验证的编码视图，所有引用随原报文存活；不分配或复制 backing。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectInfo<'a> {
    bytes: &'a [u8],
    trait_count: usize,
    tags_start: usize,
    tag_count: usize,
}

#[cfg(target_arch = "riscv64")]
pub(crate) struct InfoLayout {
    trait_count: usize,
    tags_start: usize,
    tag_count: usize,
}

impl<'a> ObjectInfo<'a> {
    #[cfg(target_arch = "riscv64")]
    pub(crate) fn layout(self) -> InfoLayout {
        InfoLayout {
            trait_count: self.trait_count,
            tags_start: self.tags_start,
            tag_count: self.tag_count,
        }
    }

    /// payload owner 只能保存完整验证所得的布局，并以同一不可变字节恢复借用视图。
    #[cfg(target_arch = "riscv64")]
    pub(crate) fn from_layout(bytes: &'a [u8], layout: &InfoLayout) -> Self {
        Self {
            bytes,
            trait_count: layout.trait_count,
            tags_start: layout.tags_start,
            tag_count: layout.tag_count,
        }
    }

    /// 仅供已完整验证且保持不可变的 Page 迭代器使用；不重复扫描键或 tag。
    pub(crate) fn view_validated(bytes: &'a [u8]) -> Self {
        // Page 构造前已调用 ObjectInfo::decode，两个计数字段及其偏移必在报文内。
        let trait_count =
            u32::from_le_bytes(bytes[..4].try_into().expect("validated trait count")) as usize;
        let count_offset = 4 + trait_count * MarkerId::ENCODED_LEN;
        let tag_count = u32::from_le_bytes(
            bytes[count_offset..count_offset + 4]
                .try_into()
                .expect("validated tag count"),
        ) as usize;
        Self {
            bytes,
            trait_count,
            tags_start: count_offset + 4,
            tag_count,
        }
    }

    pub fn decode(bytes: &'a [u8]) -> Result<Self, Status> {
        if bytes.len() > BODY_MAX - 4 {
            return Err(Status::Invalid);
        }
        let mut reader = Reader::new(bytes);
        let trait_count = usize::try_from(reader.u32()?).map_err(|_| Status::Invalid)?;
        let trait_bytes = trait_count
            .checked_mul(MarkerId::ENCODED_LEN)
            .ok_or(Status::Invalid)?;
        // 连 tag_count 都必须在剩余报文内，先限长度再按 count 扫描。
        if trait_bytes.checked_add(4).ok_or(Status::Invalid)? > reader.remaining() {
            return Err(Status::Invalid);
        }
        let traits_start = reader.consumed();
        for index in 0..trait_count {
            let marker = MarkerId::read(&mut reader)?;
            let mut previous =
                Reader::new(&bytes[traits_start..traits_start + index * MarkerId::ENCODED_LEN]);
            for _ in 0..index {
                if MarkerId::read(&mut previous)? == marker {
                    return Err(Status::Invalid);
                }
            }
        }
        let tag_count = usize::try_from(reader.u32()?).map_err(|_| Status::Invalid)?;
        if tag_count.checked_mul(24).ok_or(Status::Invalid)? > reader.remaining() {
            return Err(Status::Invalid);
        }
        let tags_start = reader.consumed();
        for _ in 0..tag_count {
            let start = reader.consumed();
            let tag = Tag::read(&mut reader)?;
            let mut previous = Reader::new(&bytes[tags_start..start]);
            while previous.remaining() != 0 {
                if Tag::read(&mut previous)?.key == tag.key {
                    return Err(Status::Invalid);
                }
            }
        }
        reader.finish()?;
        Ok(Self {
            bytes,
            trait_count,
            tags_start,
            tag_count,
        })
    }

    pub fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub fn traits(self) -> Traits<'a> {
        Traits {
            reader: Reader::new(&self.bytes[4..4 + self.trait_count * MarkerId::ENCODED_LEN]),
        }
    }

    pub fn tags(self) -> Tags<'a> {
        Tags {
            reader: Reader::new(&self.bytes[self.tags_start..]),
            remaining: self.tag_count,
        }
    }

    pub fn contains(self, marker: MarkerId) -> bool {
        self.traits().any(|value| value == marker)
    }
}

pub struct Traits<'a> {
    reader: Reader<'a>,
}
impl Iterator for Traits<'_> {
    type Item = MarkerId;
    fn next(&mut self) -> Option<Self::Item> {
        if self.reader.remaining() == 0 {
            return None;
        }
        // 只有完整解码后的不可变 ObjectInfo 能构造迭代器，长度已验证。
        Some(MarkerId::read(&mut self.reader).expect("validated marker encoding"))
    }
}

pub struct Tags<'a> {
    reader: Reader<'a>,
    remaining: usize,
}
impl<'a> Iterator for Tags<'a> {
    type Item = Tag<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        // bytes 不可变且所有 tag 已由 ObjectInfo::decode 验证。
        Some(Tag::read(&mut self.reader).expect("validated tag encoding"))
    }
}

/// 准备阶段计算长度及唯一性，SummaryTooLarge 不修改输出缓冲。
pub fn encoded_len(traits: &[MarkerId], tags: &[Tag<'_>]) -> Result<usize, Status> {
    u32::try_from(traits.len()).map_err(|_| Status::SummaryTooLarge)?;
    u32::try_from(tags.len()).map_err(|_| Status::SummaryTooLarge)?;
    let mut len = traits
        .len()
        .checked_mul(MarkerId::ENCODED_LEN)
        .and_then(|n| n.checked_add(8))
        .ok_or(Status::SummaryTooLarge)?;
    for (index, marker) in traits.iter().enumerate() {
        if traits[..index].contains(marker) {
            return Err(Status::Invalid);
        }
    }
    for (index, tag) in tags.iter().enumerate() {
        if tags[..index].iter().any(|other| other.key == tag.key) {
            return Err(Status::Invalid);
        }
        u32::try_from(tag.value.len()).map_err(|_| Status::SummaryTooLarge)?;
        len = len
            .checked_add(24)
            .and_then(|n| n.checked_add(tag.value.len()))
            .ok_or(Status::SummaryTooLarge)?;
    }
    if len > BODY_MAX - 4 {
        return Err(Status::SummaryTooLarge);
    }
    Ok(len)
}

pub fn encode(traits: &[MarkerId], tags: &[Tag<'_>], out: &mut [u8]) -> Result<usize, Status> {
    let len = encoded_len(traits, tags)?;
    if out.len() < len {
        return Err(Status::Resource);
    }
    let mut writer = Writer::new(out);
    writer.u32(traits.len() as u32);
    for marker in traits {
        marker.write(&mut writer);
    }
    writer.u32(tags.len() as u32);
    for tag in tags {
        tag.key.write(&mut writer);
        writer.bytes32(tag.value);
    }
    Ok(writer.written())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_markers_and_tag_bytes_preserve_unknown_and_mixed_meanings() {
        // OAL 只保证键唯一；未知数字/二进制值与混合标记都原样保留。
        let unknown = MarkerId {
            namespace: u128::MAX,
            local: u32::MAX,
        };
        let mut bytes = [0; 128];
        let tags = [
            Tag {
                key: unknown,
                value: &[0xff, 0, 0x80],
            },
            Tag {
                key: DIRECTORY,
                value: b"",
            },
        ];
        let used = encode(&[unknown, DIRECTORY], &tags, &mut bytes).unwrap();
        let info = ObjectInfo::decode(&bytes[..used]).unwrap();
        assert_eq!(info.traits().collect::<Vec<_>>(), [unknown, DIRECTORY]);
        assert_eq!(info.tags().collect::<Vec<_>>(), tags);
        assert!(info.contains(DIRECTORY));
    }

    #[test]
    fn duplicate_keys_are_invalid_in_each_independent_key_space() {
        // 同一 Trait/tag 键不可重复，但两种键空间可使用同一个数字。
        let mut bytes = [0; 128];
        let mut writer = Writer::new(&mut bytes);
        writer.u32(2);
        DIRECTORY.write(&mut writer);
        DIRECTORY.write(&mut writer);
        writer.u32(0);
        let used = writer.written();
        assert_eq!(ObjectInfo::decode(&bytes[..used]), Err(Status::Invalid));
        let mut writer = Writer::new(&mut bytes);
        writer.u32(0);
        writer.u32(2);
        for _ in 0..2 {
            DIRECTORY.write(&mut writer);
            writer.bytes32(b"");
        }
        let used = writer.written();
        assert_eq!(ObjectInfo::decode(&bytes[..used]), Err(Status::Invalid));
        assert_eq!(
            encode(&[DIRECTORY, DIRECTORY], &[], &mut bytes),
            Err(Status::Invalid)
        );
        assert_eq!(
            encode(
                &[],
                &[Tag {
                    key: DIRECTORY,
                    value: b""
                }; 2],
                &mut bytes
            ),
            Err(Status::Invalid)
        );
    }

    #[test]
    fn untrusted_counts_lengths_truncation_and_tail_are_invalid() {
        // 巨大 count 不能引起预分配；每个字段及长度段必须完整、无尾差。
        assert_eq!(
            ObjectInfo::decode(&u32::MAX.to_le_bytes()),
            Err(Status::Invalid)
        );
        let mut bytes = [0; 128];
        let used = encode(
            &[DIRECTORY],
            &[Tag {
                key: DIRECTORY,
                value: b"tag",
            }],
            &mut bytes,
        )
        .unwrap();
        for len in 0..used {
            assert_eq!(ObjectInfo::decode(&bytes[..len]), Err(Status::Invalid));
        }
        assert_eq!(ObjectInfo::decode(&bytes[..used + 1]), Err(Status::Invalid));
        bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(ObjectInfo::decode(&bytes[..used]), Err(Status::Invalid));
        let used = encode(
            &[],
            &[Tag {
                key: DIRECTORY,
                value: b"",
            }],
            &mut bytes,
        )
        .unwrap();
        bytes[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(ObjectInfo::decode(&bytes[..used]), Err(Status::Invalid));
    }

    #[test]
    fn summary_capacity_counts_result_and_all_length_prefixes() {
        // Lookup 的 result 占 4 字节；越界必须先拒绝，不能截断摘要。
        let value = vec![0; BODY_MAX - 4 - 32];
        let mut bytes = vec![0; BODY_MAX];
        assert_eq!(
            encode(
                &[],
                &[Tag {
                    key: DIRECTORY,
                    value: &value
                }],
                &mut bytes
            ),
            Ok(BODY_MAX - 4)
        );
        let over = vec![0; value.len() + 1];
        assert_eq!(
            encode(
                &[],
                &[Tag {
                    key: DIRECTORY,
                    value: &over
                }],
                &mut bytes
            ),
            Err(Status::SummaryTooLarge)
        );
    }
}
