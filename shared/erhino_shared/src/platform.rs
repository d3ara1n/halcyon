//! 初始平台描述的定宽小端格式。描述只关联 DT 节点与初始句柄索引，不授予权限。

use crate::startup::initial;

pub const MAGIC: u64 = u64::from_le_bytes(*b"ERHPLATF");
pub const VERSION: u32 = 1;
pub const HEADER_BYTES: usize = 48;
pub const RESOURCE_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ResourceKind {
    MmioControl = 1,
    IrqControl = 2,
    DmaDomainControl = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub kind: ResourceKind,
    /// 相对 DTB 起始位置的 BEGIN_NODE token 偏移。
    pub dt_node_offset: u32,
    pub handle_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    InvalidHeader,
    InvalidResource,
    Overflow,
}

/// 无借用的几何；调用者在映射整个对象前可先解析固定头。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub total_bytes: usize,
    pub dtb_offset: usize,
    pub dtb_bytes: usize,
    pub resource_count: usize,
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Error> {
    let value = bytes.get(at..at + 4).ok_or(Error::Truncated)?;
    Ok(u32::from_le_bytes(
        value.try_into().map_err(|_| Error::Truncated)?,
    ))
}
fn u64_at(bytes: &[u8], at: usize) -> Result<u64, Error> {
    let value = bytes.get(at..at + 8).ok_or(Error::Truncated)?;
    Ok(u64::from_le_bytes(
        value.try_into().map_err(|_| Error::Truncated)?,
    ))
}
fn size(value: u64) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::Overflow)
}

impl Header {
    pub fn new(dtb_bytes: usize, resource_count: usize) -> Result<Self, Error> {
        if dtb_bytes == 0
            || initial::BASE_HANDLE_COUNT
                .checked_add(resource_count)
                .is_none_or(|count| u32::try_from(count).is_err())
        {
            return Err(Error::InvalidHeader);
        }
        let dtb_offset = resource_count
            .checked_mul(RESOURCE_BYTES)
            .and_then(|bytes| bytes.checked_add(HEADER_BYTES))
            .ok_or(Error::Overflow)?;
        let total_bytes = dtb_offset.checked_add(dtb_bytes).ok_or(Error::Overflow)?;
        Ok(Self {
            total_bytes,
            dtb_offset,
            dtb_bytes,
            resource_count,
        })
    }

    pub fn encode(self) -> Result<[u8; HEADER_BYTES], Error> {
        if Self::new(self.dtb_bytes, self.resource_count)? != self {
            return Err(Error::InvalidHeader);
        }
        let mut bytes = [0; HEADER_BYTES];
        bytes[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&(self.total_bytes as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(self.dtb_offset as u64).to_le_bytes());
        bytes[32..40].copy_from_slice(&(self.dtb_bytes as u64).to_le_bytes());
        bytes[40..44].copy_from_slice(&(self.resource_count as u32).to_le_bytes());
        bytes[44..48].copy_from_slice(&(RESOURCE_BYTES as u32).to_le_bytes());
        Ok(bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_BYTES {
            return Err(Error::Truncated);
        }
        if u64_at(bytes, 0)? != MAGIC
            || u32_at(bytes, 8)? != VERSION
            || u32_at(bytes, 12)? != HEADER_BYTES as u32
            || u32_at(bytes, 44)? != RESOURCE_BYTES as u32
        {
            return Err(Error::InvalidHeader);
        }
        let header = Self {
            total_bytes: size(u64_at(bytes, 16)?)?,
            dtb_offset: size(u64_at(bytes, 24)?)?,
            dtb_bytes: size(u64_at(bytes, 32)?)?,
            resource_count: u32_at(bytes, 40)? as usize,
        };
        if Self::new(header.dtb_bytes, header.resource_count)? != header {
            return Err(Error::InvalidHeader);
        }
        Ok(header)
    }
}

impl Resource {
    pub fn encode(self) -> [u8; RESOURCE_BYTES] {
        let mut bytes = [0; RESOURCE_BYTES];
        bytes[0..4].copy_from_slice(&(self.kind as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&self.dt_node_offset.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.handle_index.to_le_bytes());
        bytes
    }

    fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let kind = match u32_at(bytes, 0)? {
            1 => ResourceKind::MmioControl,
            2 => ResourceKind::IrqControl,
            3 => ResourceKind::DmaDomainControl,
            _ => return Err(Error::InvalidResource),
        };
        if u32_at(bytes, 12)? != 0 {
            return Err(Error::InvalidResource);
        }
        Ok(Self {
            kind,
            dt_node_offset: u32_at(bytes, 4)?,
            handle_index: u32_at(bytes, 8)?,
        })
    }
}

/// 仅校验格式与索引关系；DT token 和 capability kind/rights 由各实际消费者核验。
pub struct Description<'a> {
    bytes: &'a [u8],
    header: Header,
}
impl<'a> Description<'a> {
    pub fn parse(bytes: &'a [u8], handle_count: usize) -> Result<Self, Error> {
        let description = Self::decode(bytes)?;
        if initial::BASE_HANDLE_COUNT.checked_add(description.header.resource_count)
            != Some(handle_count)
        {
            return Err(Error::InvalidResource);
        }
        for index in 0..description.header.resource_count {
            if description
                .resource(index)
                .ok_or(Error::InvalidResource)?
                .handle_index as usize
                != initial::BASE_HANDLE_COUNT + index
            {
                return Err(Error::InvalidResource);
            }
        }
        Ok(description)
    }

    /// 解析只读事实，不解释其中的内核初始槽为当前进程的授权。
    /// 后继进程必须另核用户态委托的记录序号、实际槽位、kind/rights 和几何。
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        let header = Header::parse(bytes)?;
        let bytes = bytes.get(..header.total_bytes).ok_or(Error::Truncated)?;
        let description = Self { bytes, header };
        for index in 0..header.resource_count {
            let resource = description.resource(index).ok_or(Error::InvalidResource)?;
            if resource.dt_node_offset as usize >= header.dtb_bytes
                || !resource.dt_node_offset.is_multiple_of(4)
            {
                return Err(Error::InvalidResource);
            }
        }
        Ok(description)
    }
    pub fn header(&self) -> Header {
        self.header
    }
    pub fn dtb(&self) -> &'a [u8] {
        &self.bytes[self.header.dtb_offset..]
    }
    pub fn resource(&self, index: usize) -> Option<Resource> {
        if index >= self.header.resource_count {
            return None;
        }
        let start = HEADER_BYTES + index * RESOURCE_BYTES;
        Resource::parse(&self.bytes[start..start + RESOURCE_BYTES]).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // 固定格式合同：拒绝范围溢出、槽位伪造与重复资源，不用编码器输出作为唯一预期。
    #[test]
    fn header_has_fixed_little_endian_geometry() {
        let header = Header::new(40, 1).unwrap();
        let bytes = header.encode().unwrap();
        assert_eq!(&bytes[0..16], b"ERHPLATF\x01\0\0\0\x30\0\0\0");
        assert_eq!(u64_at(&bytes, 16), Ok(104));
        assert_eq!(u64_at(&bytes, 24), Ok(64));
        assert_eq!(Header::parse(&bytes).unwrap(), header);
        let mut invalid = bytes;
        invalid[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Header::parse(&invalid).is_err());
        assert!(Header::new(usize::MAX, 1).is_err());
        assert!(Header::parse(&bytes[..47]).is_err());
    }

    #[test]
    fn permits_multiple_ranges_per_node_but_rejects_duplicate_handle_indices() {
        let header = Header::new(64, 2).unwrap();
        let mut bytes = vec![0; header.total_bytes];
        bytes[..HEADER_BYTES].copy_from_slice(&header.encode().unwrap());
        let first = Resource {
            kind: ResourceKind::MmioControl,
            dt_node_offset: 40,
            handle_index: initial::BASE_HANDLE_COUNT as u32,
        };
        bytes[48..64].copy_from_slice(&first.encode());
        let mut second = Resource {
            handle_index: first.handle_index + 1,
            ..first
        };
        bytes[64..80].copy_from_slice(&second.encode());
        assert!(Description::parse(&bytes, initial::BASE_HANDLE_COUNT + 2).is_ok());
        second.kind = ResourceKind::IrqControl;
        bytes[64..80].copy_from_slice(&second.encode());
        assert!(Description::parse(&bytes, initial::BASE_HANDLE_COUNT + 2).is_ok());
        assert!(Description::parse(&bytes, initial::BASE_HANDLE_COUNT + 1).is_err());
        assert!(
            Description::parse(&bytes[..bytes.len() - 1], initial::BASE_HANDLE_COUNT + 2).is_err()
        );
        second.handle_index = first.handle_index;
        bytes[64..80].copy_from_slice(&second.encode());
        assert!(Description::parse(&bytes, initial::BASE_HANDLE_COUNT + 2).is_err());
        // 同一只读对象可以被后继进程解码；初始槽错误仍须由严格入口拒绝，
        // 后继授权由用户态委托合同另核，不能传虚构 initial handle_count。
        assert!(Description::decode(&bytes).is_ok());
    }
}
