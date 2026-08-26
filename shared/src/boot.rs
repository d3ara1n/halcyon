//! BootPackage 固定外层：内核只据此定位 initial ELF 与 opaque payload。

/// BootPackage v1 固定头长度。
pub const BOOT_PACKAGE_HEADER_LEN: usize = 64;
/// BootPackage v1 版本。
pub const BOOT_PACKAGE_VERSION: u16 = 1;
/// BootPackage 魔数（`ERHBOOT\0`）。
pub const BOOT_PACKAGE_MAGIC: u64 = u64::from_le_bytes(*b"ERHBOOT\0");

/// BootPackage 固定头。磁盘字段均为 little-endian；解析后返回主机字节序值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct BootPackageHeader {
    pub magic: u64,
    pub version: u16,
    pub header_len: u16,
    pub flags: u32,
    pub total_len: u64,
    pub init_off: u64,
    pub init_len: u64,
    pub payload_off: u64,
    pub payload_len: u64,
    pub reserved: u64,
}

const _: () = {
    assert!(core::mem::size_of::<BootPackageHeader>() == BOOT_PACKAGE_HEADER_LEN);
    assert!(core::mem::align_of::<BootPackageHeader>() == 8);
};

/// BootPackage 几何校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPackageError {
    TruncatedHeader,
    BadMagic,
    UnsupportedVersion,
    BadHeaderLength,
    NonzeroFlags,
    NonzeroReserved,
    InvalidPageSize,
    GeometryOverflow,
    LengthOutOfWindow,
    NoncanonicalLayout,
    MisalignedPayload,
    NonzeroPadding,
}

/// 已验证的 BootPackage 视图。
#[derive(Debug, Clone, Copy)]
pub struct BootPackage<'a> {
    pub header: BootPackageHeader,
    pub initial_elf: &'a [u8],
    pub payload: &'a [u8],
}

/// 在外部加载器声明的最大窗口内验证 BootPackage。
///
/// v1 采用唯一规范布局：`[header][initial ELF][zero padding][payload][zero
/// padding]`。payload 与 total length 均页对齐，便于内核直接只读映射。
pub fn validate_boot_package(
    window: &[u8],
    page_size: usize,
) -> Result<BootPackage<'_>, BootPackageError> {
    if window.len() < BOOT_PACKAGE_HEADER_LEN {
        return Err(BootPackageError::TruncatedHeader);
    }
    if page_size == 0 || !page_size.is_power_of_two() {
        return Err(BootPackageError::InvalidPageSize);
    }

    // SAFETY: 长度已覆盖完整固定头；包地址不要求按 8 字节对齐。
    let raw = unsafe { core::ptr::read_unaligned(window.as_ptr().cast::<BootPackageHeader>()) };
    let header = BootPackageHeader {
        magic: u64::from_le(raw.magic),
        version: u16::from_le(raw.version),
        header_len: u16::from_le(raw.header_len),
        flags: u32::from_le(raw.flags),
        total_len: u64::from_le(raw.total_len),
        init_off: u64::from_le(raw.init_off),
        init_len: u64::from_le(raw.init_len),
        payload_off: u64::from_le(raw.payload_off),
        payload_len: u64::from_le(raw.payload_len),
        reserved: u64::from_le(raw.reserved),
    };

    if header.magic != BOOT_PACKAGE_MAGIC {
        return Err(BootPackageError::BadMagic);
    }
    if header.version != BOOT_PACKAGE_VERSION {
        return Err(BootPackageError::UnsupportedVersion);
    }
    if header.header_len as usize != BOOT_PACKAGE_HEADER_LEN {
        return Err(BootPackageError::BadHeaderLength);
    }
    if header.flags != 0 {
        return Err(BootPackageError::NonzeroFlags);
    }
    if header.reserved != 0 {
        return Err(BootPackageError::NonzeroReserved);
    }

    let total = usize::try_from(header.total_len).map_err(|_| BootPackageError::GeometryOverflow)?;
    let init_off = usize::try_from(header.init_off).map_err(|_| BootPackageError::GeometryOverflow)?;
    let init_len = usize::try_from(header.init_len).map_err(|_| BootPackageError::GeometryOverflow)?;
    let payload_off = usize::try_from(header.payload_off).map_err(|_| BootPackageError::GeometryOverflow)?;
    let payload_len = usize::try_from(header.payload_len).map_err(|_| BootPackageError::GeometryOverflow)?;
    let init_end = init_off.checked_add(init_len).ok_or(BootPackageError::GeometryOverflow)?;
    let payload_end = payload_off.checked_add(payload_len).ok_or(BootPackageError::GeometryOverflow)?;
    let canonical_payload_off = align_up(init_end, page_size)?;
    let canonical_total = align_up(payload_end, page_size)?;

    if total > window.len() || total < BOOT_PACKAGE_HEADER_LEN {
        return Err(BootPackageError::LengthOutOfWindow);
    }
    if init_len == 0 || init_off != BOOT_PACKAGE_HEADER_LEN || init_end > total {
        return Err(BootPackageError::NoncanonicalLayout);
    }
    if payload_off % page_size != 0 || total % page_size != 0 {
        return Err(BootPackageError::MisalignedPayload);
    }
    if payload_off != canonical_payload_off || canonical_total != total || payload_end > total {
        return Err(BootPackageError::NoncanonicalLayout);
    }
    if window[init_end..payload_off].iter().any(|byte| *byte != 0)
        || window[payload_end..total].iter().any(|byte| *byte != 0)
    {
        return Err(BootPackageError::NonzeroPadding);
    }

    Ok(BootPackage {
        header,
        initial_elf: &window[init_off..init_end],
        payload: &window[payload_off..payload_end],
    })
}

fn align_up(value: usize, alignment: usize) -> Result<usize, BootPackageError> {
    value
        .checked_add(alignment - 1)
        .map(|end| end & !(alignment - 1))
        .ok_or(BootPackageError::GeometryOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const PAGE: usize = 4096;

    fn package(init: &[u8], payload: &[u8]) -> alloc::vec::Vec<u8> {
        let init_off = BOOT_PACKAGE_HEADER_LEN;
        let payload_off = (init_off + init.len()).div_ceil(PAGE) * PAGE;
        let total = (payload_off + payload.len()).div_ceil(PAGE) * PAGE;
        let header = BootPackageHeader {
            magic: BOOT_PACKAGE_MAGIC.to_le(),
            version: BOOT_PACKAGE_VERSION.to_le(),
            header_len: (BOOT_PACKAGE_HEADER_LEN as u16).to_le(),
            flags: 0,
            total_len: (total as u64).to_le(),
            init_off: (init_off as u64).to_le(),
            init_len: (init.len() as u64).to_le(),
            payload_off: (payload_off as u64).to_le(),
            payload_len: (payload.len() as u64).to_le(),
            reserved: 0,
        };
        let mut bytes = vec![0u8; total];
        // SAFETY: 目标覆盖完整 64 字节头且不重叠。
        unsafe {
            core::ptr::copy_nonoverlapping(
                (&header as *const BootPackageHeader).cast::<u8>(),
                bytes.as_mut_ptr(),
                BOOT_PACKAGE_HEADER_LEN,
            );
        }
        bytes[init_off..init_off + init.len()].copy_from_slice(init);
        bytes[payload_off..payload_off + payload.len()].copy_from_slice(payload);
        bytes
    }

    #[test]
    fn validates_canonical_package_and_empty_payload() {
        let bytes = package(b"ELF", b"archive");
        let view = validate_boot_package(&bytes, PAGE).unwrap();
        assert_eq!(view.initial_elf, b"ELF");
        assert_eq!(view.payload, b"archive");
        assert_eq!(view.header.total_len as usize, bytes.len());

        let empty = package(b"ELF", b"");
        assert!(validate_boot_package(&empty, PAGE).unwrap().payload.is_empty());
    }

    #[test]
    fn rejects_window_escape_and_nonzero_padding() {
        let mut bytes = package(b"ELF", b"archive");
        let truncated = &bytes[..bytes.len() - 1];
        assert_eq!(
            validate_boot_package(truncated, PAGE).unwrap_err(),
            BootPackageError::LengthOutOfWindow
        );

        bytes[BOOT_PACKAGE_HEADER_LEN + 3] = 1;
        assert_eq!(
            validate_boot_package(&bytes, PAGE).unwrap_err(),
            BootPackageError::NonzeroPadding
        );
    }

    #[test]
    fn rejects_noncanonical_offsets() {
        let mut bytes = package(b"ELF", b"archive");
        let payload_off_field = 40;
        bytes[payload_off_field..payload_off_field + 8]
            .copy_from_slice(&(8192u64).to_le_bytes());
        assert_eq!(
            validate_boot_package(&bytes, PAGE).unwrap_err(),
            BootPackageError::NoncanonicalLayout
        );
    }
}
