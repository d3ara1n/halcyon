//! 本轮冻结资源表的只读OAL投影；零能力、无Boundary，不改变资源暴露事实。
use super::{Descriptor, Frozen};
use crate::identity::BindingRef;
use liboal::{
    ObjectInfo,
    info::{self, DIRECTORY, MarkerId, Tag},
    protocol::{self as wire, DirectoryEntry, Page, Response, Status},
};
use librpc::bytes::{Reader, Writer};

pub const RESOURCE_INFO: MarkerId = MarkerId {
    namespace: 0x4841_4c43_594f_4e00_0000_0044_4556_4943,
    local: 2,
};
const VALUE_BYTES: usize = BindingRef::ENCODED_LEN + Descriptor::ENCODED_LEN;
const INFO_BYTES: usize = 8 + MarkerId::ENCODED_LEN + 4 + VALUE_BYTES;
const CURSOR_BYTES: usize = BindingRef::ENCODED_LEN + 4;
// OAL正文的response前缀、page固定头、完整游标、单项两长度和完整摘要。
pub const MAX_RESOURCE_NAME: usize = wire::BODY_MAX - 4 - 12 - CURSOR_BYTES - 8 - INFO_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub reference: BindingRef,
    pub descriptor: Option<Descriptor>,
}
impl Info {
    pub fn decode(info: ObjectInfo<'_>) -> Result<Self, Status> {
        let tag = info
            .tags()
            .find(|tag| tag.key == RESOURCE_INFO)
            .ok_or(Status::Invalid)?;
        let mut r = Reader::new(tag.value);
        let reference = BindingRef::decode(&mut r).map_err(|_| Status::Invalid)?;
        let descriptor = if r.remaining() == 0 {
            None
        } else {
            Some(Descriptor::decode(&mut r).map_err(|_| Status::Invalid)?)
        };
        r.finish()?;
        if info.contains(DIRECTORY) != descriptor.is_none() {
            return Err(Status::Invalid);
        }
        Ok(Self {
            reference,
            descriptor,
        })
    }
    fn encode(self, out: &mut [u8]) -> Result<usize, Status> {
        let mut value = [0; VALUE_BYTES];
        let mut w = Writer::new(&mut value);
        self.reference.encode(&mut w);
        if let Some(descriptor) = self.descriptor {
            descriptor.encode(&mut w);
        }
        let used = w.written();
        info::encode(
            if self.descriptor.is_none() {
                &[DIRECTORY]
            } else {
                &[]
            },
            &[Tag {
                key: RESOURCE_INFO,
                value: &value[..used],
            }],
            out,
        )
    }
}
impl<C> Frozen<C> {
    fn next_resource(&self, after: &str) -> Option<usize> {
        (0..self.len())
            .filter(|&index| self.at(index).is_some_and(|item| item.name > after))
            .min_by_key(|&index| {
                self.at(index)
                    .expect("frozen resource index disappeared")
                    .name
            })
    }
    /// 宿主须先核真实BindingControl/Use及原轮可用性；这里不铸造或复制C。
    pub fn discover(
        &self,
        reference: BindingRef,
        bytes: &[u8],
        handle_count: usize,
        out: &mut [u8],
    ) -> Result<usize, Status> {
        let (header, request) = wire::decode_request(bytes)?;
        if handle_count != 0 {
            return Err(Status::Invalid);
        }
        let mut components = request.path().split('/').filter(|name| !name.is_empty());
        let leaf = components
            .next()
            .map(|name| self.get(name).ok_or(Status::NotFound))
            .transpose()?;
        if components.next().is_some() {
            return Err(Status::NotDirectory);
        }
        let mut summary = [0; INFO_BYTES];
        match request {
            wire::Request::Lookup { .. } => {
                let used = Info {
                    reference,
                    descriptor: leaf.map(|item| item.descriptor),
                }
                .encode(&mut summary)?;
                wire::encode_response(
                    header.op,
                    &Response::Found(ObjectInfo::decode(&summary[..used])?),
                    header.deadline,
                    out,
                )
            }
            wire::Request::Enumerate { cursor, .. } => {
                if leaf.is_some() {
                    return Err(Status::NotDirectory);
                }
                let after = if cursor.is_empty() {
                    ""
                } else {
                    let mut r = Reader::new(cursor);
                    if BindingRef::decode(&mut r).map_err(|_| Status::CursorInvalid)? != reference {
                        return Err(Status::CursorInvalid);
                    }
                    let index = r.u32().map_err(|_| Status::CursorInvalid)? as usize;
                    r.finish().map_err(|_| Status::CursorInvalid)?;
                    self.at(index).ok_or(Status::CursorInvalid)?.name
                };
                let mut page = [0; wire::BODY_MAX];
                let mut next = [0; CURSOR_BYTES];
                let used = if let Some(index) = self.next_resource(after) {
                    let item = self.at(index).ok_or(Status::CursorInvalid)?;
                    let done = self.next_resource(item.name).is_none();
                    let cursor = if done {
                        &[][..]
                    } else {
                        let mut w = Writer::new(&mut next);
                        reference.encode(&mut w);
                        w.u32(u32::try_from(index).map_err(|_| Status::Resource)?);
                        let used = w.written();
                        &next[..used]
                    };
                    let used = Info {
                        reference,
                        descriptor: Some(item.descriptor),
                    }
                    .encode(&mut summary)?;
                    wire::encode_page(
                        cursor,
                        done,
                        &[DirectoryEntry {
                            name: item.name,
                            info: Some(ObjectInfo::decode(&summary[..used])?),
                        }],
                        &mut page,
                    )?
                } else {
                    wire::encode_page(&[], true, &[], &mut page)?
                };
                wire::encode_response(
                    header.op,
                    &Response::Page(Page::decode(&page[..used])?),
                    header.deadline,
                    out,
                )
            }
        }
    }
}
