//! 只读投影原ProcessRecord/Run；不驱动清理、不复制成员表，不交付能力。
use super::*;
use libdriver::view as info;
use liboal::{
    ObjectInfo,
    protocol::{self as oal, DirectoryEntry, Page, Response, Status},
};
use librpc::bytes::{Reader, Writer};

#[derive(Clone, Copy)]
enum View {
    Root,
    Instance(u64),
    Bindings(u64),
    Binding(u64, u64),
}
impl World<'_> {
    fn instance_id(&self, serial: u64) -> Result<InstanceId, Status> {
        Ok(self
            .processes
            .get(serial)
            .ok_or(Status::NotFound)?
            .borrow()
            .id)
    }
    fn binding_member(&self, instance: u64, serial: u64) -> Result<&Rc<RefCell<Run>>, Status> {
        let id = self.instance_id(instance)?;
        let run = self.runs.get(serial).ok_or(Status::NotFound)?;
        if run.borrow().process != Some(id) {
            return Err(Status::NotFound);
        }
        Ok(run)
    }
    fn resolve_view(&self, path: &str) -> Result<View, Status> {
        let mut names = path.split('/').filter(|name| !name.is_empty());
        let Some(name) = names.next() else {
            return Ok(View::Root);
        };
        let serial = info::parse_instance_name(name).ok_or(Status::NotFound)?;
        self.instance_id(serial)?;
        match names.next() {
            None => Ok(View::Instance(serial)),
            Some(info::BINDINGS_NAME) => {
                let Some(name) = names.next() else {
                    return Ok(View::Bindings(serial));
                };
                let device = info::parse_binding_name(name).ok_or(Status::NotFound)?;
                self.binding_member(serial, device)?;
                if names.next().is_some() {
                    return Err(Status::NotDirectory);
                }
                Ok(View::Binding(serial, device))
            }
            Some(_) => Err(Status::NotFound),
        }
    }
    fn view_info(&self, view: View, out: &mut [u8]) -> Result<usize, Status> {
        match view {
            View::Root => info::encode_root(self.incarnation, out),
            View::Instance(serial) => {
                let record = self.processes.get(serial).ok_or(Status::NotFound)?.borrow();
                let declaration = self
                    .catalog
                    .records()
                    .nth(record.implementation)
                    .ok_or(Status::NotFound)?
                    .map_err(|_| Status::Invalid)?;
                info::InstanceInfo {
                    id: record.id,
                    pid: record.pid,
                    implementation: declaration.key,
                    capacity: declaration.capacity,
                    bindings: u32::try_from(worker::member_count(self, record.id))
                        .map_err(|_| Status::Resource)?,
                    accepting: record.accepting,
                    cpu_done: record.dead || record.collected,
                    collected: record.collected,
                    error: record.error,
                }
                .encode(out)
            }
            View::Bindings(serial) => info::encode_bindings(self.instance_id(serial)?, out),
            View::Binding(instance, serial) => {
                let run = self.binding_member(instance, serial)?.borrow();
                info::BindingInfo {
                    instance: self.instance_id(instance)?,
                    reference: run.reference,
                    phase: run.phase,
                    released: run.released,
                }
                .encode(out)
            }
        }
    }
    fn next_view(&self, view: View, after: u64) -> Result<Option<(u64, View)>, Status> {
        match view {
            View::Root => Ok(self
                .processes
                .next_after(Some(&after))
                .map(|(&serial, _)| (serial, View::Instance(serial)))),
            View::Instance(serial) => Ok((after == 0).then_some((1, View::Bindings(serial)))),
            View::Bindings(serial) => {
                let id = self.instance_id(serial)?;
                let mut cursor = after;
                while let Some((&device, run)) = self.runs.next_after(Some(&cursor)) {
                    cursor = device;
                    if run.borrow().process == Some(id) {
                        return Ok(Some((device, View::Binding(serial, device))));
                    }
                }
                Ok(None)
            }
            View::Binding(..) => Err(Status::NotDirectory),
        }
    }
    fn cursor_view(&self, cursor: &[u8]) -> Result<(View, u64), Status> {
        let mut r = Reader::new(cursor);
        if r.u64().map_err(|_| Status::CursorInvalid)? != self.incarnation {
            return Err(Status::CursorInvalid);
        }
        let kind = r.u32().map_err(|_| Status::CursorInvalid)?;
        let instance = r.u64().map_err(|_| Status::CursorInvalid)?;
        let after = r.u64().map_err(|_| Status::CursorInvalid)?;
        r.finish().map_err(|_| Status::CursorInvalid)?;
        let view = match (kind, instance) {
            (0, 0) => View::Root,
            (2, id) if id != 0 => {
                self.instance_id(id)?;
                View::Bindings(id)
            }
            _ => return Err(Status::CursorInvalid),
        };
        Ok((view, after))
    }
    /// 调用方已按实际Control/Observer context准入；本函数从不发行sender/控制能力。
    pub(super) fn project(&self, wire: &[u8], out: &mut [u8]) -> Result<usize, Status> {
        let (header, request) = oal::decode_request(wire)?;
        if self.stopping {
            return Err(Status::Revoked);
        }
        let (view, after) = match request {
            oal::Request::Enumerate { cursor, .. } if !cursor.is_empty() => {
                self.cursor_view(cursor)?
            }
            _ => (self.resolve_view(request.path())?, 0),
        };
        let mut summary = [0; oal::BODY_MAX];
        match request {
            oal::Request::Lookup { .. } => {
                let used = self.view_info(view, &mut summary)?;
                oal::encode_response(
                    header.op,
                    &Response::Found(ObjectInfo::decode(&summary[..used])?),
                    header.deadline,
                    out,
                )
            }
            oal::Request::Enumerate { .. } => {
                let mut page = [0; oal::BODY_MAX];
                let mut cursor = [0; info::CURSOR_BYTES];
                let mut name_bytes = [0; info::NAME_BYTES];
                let used = if let Some((serial, child)) = self.next_view(view, after)? {
                    let done = self.next_view(view, serial)?.is_none();
                    let next = if done {
                        &[][..]
                    } else {
                        let mut w = Writer::new(&mut cursor);
                        w.u64(self.incarnation);
                        match view {
                            View::Root => {
                                w.u32(0);
                                w.u64(0);
                            }
                            View::Bindings(id) => {
                                w.u32(2);
                                w.u64(id);
                            }
                            _ => return Err(Status::Invalid),
                        }
                        w.u64(serial);
                        let used = w.written();
                        &cursor[..used]
                    };
                    let name = match child {
                        View::Instance(id) => info::instance_name(id, &mut name_bytes),
                        View::Bindings(_) => info::BINDINGS_NAME,
                        View::Binding(_, device) => info::binding_name(device, &mut name_bytes),
                        View::Root => return Err(Status::Invalid),
                    };
                    let used = self.view_info(child, &mut summary)?;
                    oal::encode_page(
                        next,
                        done,
                        &[DirectoryEntry {
                            name,
                            info: Some(ObjectInfo::decode(&summary[..used])?),
                        }],
                        &mut page,
                    )?
                } else {
                    oal::encode_page(&[], true, &[], &mut page)?
                };
                oal::encode_response(
                    header.op,
                    &Response::Page(Page::decode(&page[..used])?),
                    header.deadline,
                    out,
                )
            }
        }
    }
}
