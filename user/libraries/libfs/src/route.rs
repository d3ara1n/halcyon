//! 文件宿主管理协议；独立于普通目录 sender 的实际管理角色。

use crate::{
    FsRights,
    protocol::{self, Header, Status},
};
use erhino_shared::{object::Rights, time::Deadline};
use librpc::bytes::{Reader, Writer};

pub const ID: u64 = 0x4653_3031_5254_4501;
pub const MANAGEMENT_GRANT_KIND: u64 = 0x4653_3031_4d47_5201;
#[derive(Debug, Clone, Copy)]
pub enum Request<'a> {
    Bind {
        path: &'a str,
        max_rights: FsRights,
        output_transport: Rights,
        replace: bool,
    },
    Unbind {
        path: &'a str,
    },
}
impl Request<'_> {
    pub fn op(&self) -> u16 {
        match self {
            Self::Bind { .. } => 1,
            Self::Unbind { .. } => 2,
        }
    }
    pub fn path(&self) -> &str {
        match self {
            Self::Bind { path, .. } | Self::Unbind { path } => path,
        }
    }
    fn validate(&self) -> Result<(), Status> {
        if self.path().is_empty() {
            return Err(Status::Invalid);
        }
        liboal::path::validate_path(self.path()).map_err(|_| Status::Invalid)?;
        if let Self::Bind {
            output_transport, ..
        } = self
            && !output_transport.is_known()
        {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    pub fn body_len(&self) -> Result<usize, Status> {
        self.validate()?;
        let base = if matches!(self, Self::Bind { .. }) {
            20usize
        } else {
            4
        };
        let len = base.checked_add(self.path().len()).ok_or(Status::Invalid)?;
        if len > protocol::BODY_MAX {
            return Err(Status::Invalid);
        }
        Ok(len)
    }
}
pub fn encode_request(
    request: &Request<'_>,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Status> {
    deadline.instant().map_err(|_| Status::Invalid)?;
    let len = request.body_len()?;
    if out.len() < protocol::HEADER_LEN + len {
        return Err(Status::Resource);
    }
    let mut w = Writer::new(out);
    protocol::write_header(request.op(), Status::Ok, deadline, len, &mut w);
    w.bytes32(request.path().as_bytes());
    if let Request::Bind {
        max_rights,
        output_transport,
        replace,
        ..
    } = request
    {
        w.u32(max_rights.raw());
        w.u64(output_transport.raw());
        w.u32(u32::from(*replace));
    }
    Ok(w.written())
}
pub fn decode_request(bytes: &[u8], slots: u64) -> Result<(Header, Request<'_>), Status> {
    let (h, body) = Header::decode(bytes)?;
    if h.status != 0 {
        return Err(Status::Invalid);
    }
    if h.version != protocol::VERSION {
        return Err(Status::Unsupported);
    }
    if !matches!(h.op, 1 | 2) {
        return Err(Status::Unsupported);
    }
    let mut r = Reader::new(body);
    let path = r.text32()?;
    let req = match h.op {
        1 => {
            let max_rights = FsRights::from_raw(r.u32()?).ok_or(Status::Invalid)?;
            let output_transport = Rights::from_raw(r.u64()?);
            let replace = match r.u32()? {
                0 => false,
                1 => true,
                _ => return Err(Status::Invalid),
            };
            Request::Bind {
                path,
                max_rights,
                output_transport,
                replace,
            }
        }
        2 => Request::Unbind { path },
        _ => return Err(Status::Unsupported),
    };
    r.finish()?;
    req.validate()?;
    if slots != if h.op == 1 { 3 } else { 1 } {
        return Err(Status::Invalid);
    }
    Ok((h, req))
}

pub fn encode_response(
    op: u16,
    status: Status,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Status> {
    deadline.instant().map_err(|_| Status::Invalid)?;
    if out.len() < protocol::HEADER_LEN {
        return Err(Status::Resource);
    }
    let mut w = Writer::new(out);
    protocol::write_header(op, status, deadline, 0, &mut w);
    Ok(w.written())
}
pub fn decode_response(
    bytes: &[u8],
    op: u16,
    deadline: Deadline,
    slots: u64,
) -> Result<Status, Status> {
    let (header, body) = Header::decode(bytes)?;
    if header.version != protocol::VERSION
        || header.op != op
        || header.deadline != deadline
        || !body.is_empty()
        || slots != 0
    {
        return Err(Status::Invalid);
    }
    Status::from_raw(header.status).ok_or(Status::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_operation_does_not_decode_known_operation_body() {
        let until = Deadline::INFINITE;
        for body in [&[][..], &[0xff][..]] {
            let mut bytes = [0; protocol::HEADER_LEN + 1];
            let mut writer = Writer::new(&mut bytes);
            protocol::write_header(99, Status::Ok, until, body.len(), &mut writer);
            writer.bytes(body);
            let used = writer.written();
            assert!(matches!(
                decode_request(&bytes[..used], 1),
                Err(Status::Unsupported)
            ));
        }
    }
}
