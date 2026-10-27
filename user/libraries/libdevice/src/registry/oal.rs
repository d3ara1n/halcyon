//! 设备登记树的 OAL 投影；不另存目录树，Boundary 与显式功能取得共用授权门。
use super::*;
use liboal::{
    Boundary, ObjectInfo,
    info::{self, DIRECTORY, MarkerId, Tag},
    protocol::{self as oal, DirectoryEntry, Page, Response, Status},
};
use librpc::bytes::{Reader, Writer};

const VIEW_VALUE_BYTES: usize = DeviceId::ENCODED_LEN + 8 + 4 + 4;
// 最多一个 DIRECTORY trait、一个领域 tag；两份计数、tag 长度和原值全部计入。
const INFO_BYTES: usize =
    4 + MarkerId::ENCODED_LEN + 4 + MarkerId::ENCODED_LEN + 4 + VIEW_VALUE_BYTES;
// 非终页同时携带完整名字和按名游标；保证任何已准入名字至少能交付一项。
pub const MAX_DISCOVERY_NAME: usize = (oal::BODY_MAX - 4 - 12 - 24 - 8 - INFO_BYTES) / 2;
const MARKER: MarkerId = MarkerId {
    namespace: 0x4841_4c43_594f_4e00_0000_0044_4556_4943,
    local: 1,
};
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Root,
    Device(DeviceId),
    Children(DeviceId),
    Binding(DeviceId),
    Functions(DeviceId),
}
impl View {
    fn identity(self) -> (u64, u32) {
        match self {
            Self::Root => (0, 0),
            Self::Device(id) => (id.serial(), 1),
            Self::Children(id) => (id.serial(), 2),
            Self::Binding(id) => (id.serial(), 3),
            Self::Functions(id) => (id.serial(), 4),
        }
    }
}
pub struct Projection<C> {
    pub used: usize,
    pub entry: Option<C>,
}
impl<C: Clone> Registry<C> {
    fn oal_root(&self, context: u64) -> Result<View, Error> {
        let source = self.source(context)?;
        self.authorize(context, Access::OBSERVE, source.scope, false)?;
        self.source_open(context)?;
        Ok(match source.scope {
            Scope::Root => View::Root,
            Scope::Subtree(id) => View::Device(id),
        })
    }
    fn named_child(&self, parent: Option<DeviceId>, name: &str) -> Result<DeviceId, Error> {
        let mut after = 0;
        while let Some((key, record)) = self.devices.next_after(Some(&after)) {
            after = *key;
            let d = &record.value;
            if d.parent == parent && d.name == name && self.device_open(d.id).is_ok() {
                return Ok(d.id);
            }
        }
        Err(Error::NotFound)
    }
    fn active_ref(&self, id: DeviceId) -> Result<BindingRef, Error> {
        self.device(id)?
            .seat
            .as_ref()
            .and_then(Seat::active)
            .map(|s| s.reference)
            .ok_or(Error::NotReady)
    }
    fn view_info(&self, view: View, out: &mut [u8]) -> Result<usize, Status> {
        let mut value = [0; VIEW_VALUE_BYTES];
        let mut w = Writer::new(&mut value);
        if let View::Device(id) | View::Binding(id) | View::Functions(id) | View::Children(id) =
            view
        {
            let d = self.control_snapshot(id).map_err(status)?;
            id.encode(&mut w);
            w.u64(d.epoch.unwrap_or(0));
            w.u32(u32::from(d.present));
            w.u32(d.binding.map_or(0, |b| b.phase as u32 + 1));
        }
        let used = w.written();
        let tags = [Tag {
            key: MARKER,
            value: &value[..used],
        }];
        info::encode(
            if matches!(view, View::Binding(_)) {
                &[]
            } else {
                &[DIRECTORY]
            },
            &tags,
            out,
        )
    }
    fn next_name(&self, view: View, after: &str) -> Result<Option<(&str, Option<View>)>, Status> {
        match view {
            View::Root | View::Children(_) => {
                let parent = if let View::Children(id) = view {
                    Some(id)
                } else {
                    None
                };
                let mut cursor = 0;
                let mut found: Option<(&str, Option<View>)> = None;
                while let Some((key, record)) = self.devices.next_after(Some(&cursor)) {
                    cursor = *key;
                    let d = &record.value;
                    if d.parent == parent
                        && self.device_open(d.id).is_ok()
                        && d.name.as_str() > after
                        && found.is_none_or(|old| d.name.as_str() < old.0)
                    {
                        found = Some((&d.name, Some(View::Device(d.id))));
                    }
                }
                Ok(found)
            }
            View::Device(id) => Ok([
                ("binding", View::Binding(id)),
                ("children", View::Children(id)),
                ("functions", View::Functions(id)),
            ]
            .into_iter()
            .find(|(name, _)| *name > after)
            .map(|(name, view)| (name, Some(view)))),
            View::Functions(id) => Ok(self
                .active_ref(id)
                .ok()
                .and_then(|r| self.next_function(r, after))
                .map(|f| (f.name, None))),
            View::Binding(_) => Err(Status::NotDirectory),
        }
    }
    fn cursor_view<'a>(&self, context: u64, cursor: &'a [u8]) -> Result<(View, &'a str), Status> {
        let mut r = Reader::new(cursor);
        if r.u64()? != self.incarnation {
            return Err(Status::CursorInvalid);
        }
        let serial = r.u64()?;
        let kind = r.u32()?;
        let (view, scope) = if serial == 0 && kind == 0 {
            (View::Root, Scope::Root)
        } else {
            let id = DeviceId::new(self.incarnation, serial).map_err(|_| Status::CursorInvalid)?;
            let view = match kind {
                1 => View::Device(id),
                2 => View::Children(id),
                4 => View::Functions(id),
                _ => return Err(Status::CursorInvalid),
            };
            (view, Scope::Subtree(id))
        };
        self.authorize(context, Access::OBSERVE, scope, false)
            .map_err(|e| {
                if e == Error::StaleInstance {
                    Status::CursorInvalid
                } else {
                    status(e)
                }
            })?;
        let name = r.text32()?;
        liboal::path::validate_name(name)?;
        r.finish()?;
        Ok((view, name))
    }
    /// 返回的 entry 是同一具名功能根的别名 owner，宿主只导出固定运输权。
    pub fn discover(
        &self,
        context: u64,
        bytes: &[u8],
        handle_count: usize,
        out: &mut [u8],
    ) -> Result<Projection<C>, Status> {
        let (header, request) = oal::decode_request(bytes)?;
        if handle_count != 0 {
            return Err(Status::Invalid);
        }
        let path = match request {
            oal::Request::Lookup { path } | oal::Request::Enumerate { path, .. } => path,
        };
        let mut view = self.oal_root(context).map_err(status)?;
        let mut after_name = "";
        if let oal::Request::Enumerate { cursor, .. } = request
            && !cursor.is_empty()
        {
            (view, after_name) = self.cursor_view(context, cursor)?;
        }
        let mut consumed = 0;
        for component in path.split('/').filter(|p| !p.is_empty()) {
            let start = if consumed == 0 { 0 } else { consumed + 1 };
            consumed = start + component.len();
            view = match view {
                View::Root => View::Device(self.named_child(None, component).map_err(status)?),
                View::Children(parent) => {
                    View::Device(self.named_child(Some(parent), component).map_err(status)?)
                }
                View::Device(id) => match component {
                    "binding" => View::Binding(id),
                    "children" => View::Children(id),
                    "functions" => View::Functions(id),
                    _ => return Err(Status::NotFound),
                },
                View::Binding(_) => return Err(Status::NotDirectory),
                View::Functions(id) => {
                    if matches!(request, oal::Request::Enumerate { cursor, .. } if !cursor.is_empty())
                    {
                        return Err(Status::CursorInvalid);
                    }
                    let reference = self.active_ref(id).map_err(status)?;
                    let function = self
                        .open_function(context, reference, component)
                        .map_err(status)?;
                    let remaining = if consumed == path.len() {
                        ""
                    } else {
                        &path[consumed + 1..]
                    };
                    let boundary = Boundary::Provider(liboal::boundary::ProviderBoundary {
                        consumed_bytes: consumed as u32,
                        remaining,
                    });
                    boundary.validate_for(path)?;
                    let used = oal::encode_response(
                        header.op,
                        &Response::Boundary(boundary),
                        header.deadline,
                        out,
                    )?;
                    return Ok(Projection {
                        used,
                        entry: Some(function.entry.clone()),
                    });
                }
            };
        }
        let mut info_bytes = [0; INFO_BYTES];
        let used = match request {
            oal::Request::Lookup { .. } => {
                let len = self.view_info(view, &mut info_bytes)?;
                oal::encode_response(
                    header.op,
                    &Response::Found(ObjectInfo::decode(&info_bytes[..len])?),
                    header.deadline,
                    out,
                )?
            }
            oal::Request::Enumerate { max_entries, .. } => {
                if max_entries == 0 {
                    return Err(Status::Invalid);
                }
                let mut page = [0; oal::BODY_MAX];
                let mut cursor_bytes = [0; oal::BODY_MAX];
                let count = if let Some((name, child)) = self.next_name(view, after_name)? {
                    let done = self.next_name(view, name)?.is_none();
                    let next = if done {
                        &[][..]
                    } else {
                        let (serial, kind) = view.identity();
                        let mut w = Writer::new(&mut cursor_bytes);
                        w.u64(self.incarnation);
                        w.u64(serial);
                        w.u32(kind);
                        w.bytes32(name.as_bytes());
                        {
                            let used = w.written();
                            &cursor_bytes[..used]
                        }
                    };
                    let info = if let Some(view) = child {
                        let n = self.view_info(view, &mut info_bytes)?;
                        Some(ObjectInfo::decode(&info_bytes[..n])?)
                    } else {
                        None
                    };
                    oal::encode_page(next, done, &[DirectoryEntry { name, info }], &mut page)?
                } else {
                    oal::encode_page(&[], true, &[], &mut page)?
                };
                oal::encode_response(
                    header.op,
                    &Response::Page(Page::decode(&page[..count])?),
                    header.deadline,
                    out,
                )?
            }
        };
        Ok(Projection { used, entry: None })
    }
}
pub fn status(error: Error) -> Status {
    match error {
        Error::Permission => Status::Permission,
        Error::NotFound | Error::StaleInstance => Status::NotFound,
        Error::Closed => Status::Revoked,
        Error::Quota => Status::Quota,
        Error::Resource => Status::Resource,
        Error::Busy | Error::NotReady => Status::Busy,
        Error::Unsupported => Status::Unsupported,
        Error::Unavailable => Status::Unavailable,
        Error::Conflict => Status::Conflict,
        _ => Status::Invalid,
    }
}
