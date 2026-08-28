//! 出生块（BirthBlock）：组装者（libprocess / 内核 bootstrap）构造、
//! 经外部写通道交付、由接收进程运行时（rinlib）解析的用户约定数据。
//! 内核不为普通进程构造或映射出生块——唯一例外是 init 的内核内嵌组装。
//!
//! 块承载组装者写入的进程身份、安装到目标 HandleTable 的句柄值数组，
//! 以及组装者与接收进程自行解释的不透明 payload；内核不解释 payload，
//! 也不为 Handle 赋予业务 tag，两者的关联由 payload 协议按数组索引表达。

use alloc::vec::Vec;

use crate::object::Handle;
use crate::proc::Pid;

/// 块格式版本。
pub const STARTUP_VERSION: u16 = 2;

/// 块基魔数（"STARTUPB"）。
pub const STARTUP_BLOCK_MAGIC: u64 = u64::from_le_bytes(*b"STARTUPB");

/// 启动块头。其后紧跟 `handle_count` 个实际 child-local Handle；允许以
/// 零字节填充到 payload_off，再放置不透明 payload。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct StartupBlockHeader {
    pub magic: u64,
    /// 块总长（字节，含头、Handle 数组与 payload）。
    pub block_len: u32,
    pub version: u16,
    pub reserved0: u16,
    /// 仅表示 provenance，不能推导管理、继承或回收权。
    pub pid: Pid,
    /// 仅表示创建关系，不能推导管理、继承或回收权。
    pub parent_pid: Pid,
    pub handle_count: u32,
    /// payload 相对块基的偏移；不得早于 Handle 数组末尾。
    pub payload_off: u32,
    pub payload_len: u32,
    pub reserved: u32,
}

const _: () = {
    assert!(core::mem::size_of::<StartupBlockHeader>() == 48);
    assert!(core::mem::align_of::<StartupBlockHeader>() == 8);
};

/// 内核构造启动块时遇到的几何或分配错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupBuildError {
    /// 区段偏移/长度超出 u32 表示域或总长溢出。
    Overflow,
    /// 无法为完整块预留内存。
    AllocationFailed,
}

/// StartupBlock 外层校验错误；payload 内容不参与校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupParseError {
    TruncatedHeader,
    LengthMismatch,
    BadMagic,
    UnsupportedVersion,
    NonzeroReserved,
    GeometryOverflow,
    PayloadBeforeHandles,
    NonzeroPadding,
    InvalidHandle,
}

/// 校验内核定义的 outer 几何并返回已解码 header。输入无需对齐。
pub fn validate_startup_block(block: &[u8]) -> Result<StartupBlockHeader, StartupParseError> {
    if block.len() < core::mem::size_of::<StartupBlockHeader>() {
        return Err(StartupParseError::TruncatedHeader);
    }
    // SAFETY: 长度已覆盖完整 header；使用 unaligned 读取，不要求输入对齐。
    let header = unsafe { core::ptr::read_unaligned(block.as_ptr().cast::<StartupBlockHeader>()) };
    if header.block_len as usize != block.len() {
        return Err(StartupParseError::LengthMismatch);
    }
    if header.magic != STARTUP_BLOCK_MAGIC {
        return Err(StartupParseError::BadMagic);
    }
    if header.version != STARTUP_VERSION {
        return Err(StartupParseError::UnsupportedVersion);
    }
    if header.reserved0 != 0 || header.reserved != 0 {
        return Err(StartupParseError::NonzeroReserved);
    }
    let handle_bytes = (header.handle_count as usize)
        .checked_mul(core::mem::size_of::<Handle>())
        .ok_or(StartupParseError::GeometryOverflow)?;
    let handles_end = core::mem::size_of::<StartupBlockHeader>()
        .checked_add(handle_bytes)
        .ok_or(StartupParseError::GeometryOverflow)?;
    let payload_off = header.payload_off as usize;
    if payload_off < handles_end {
        return Err(StartupParseError::PayloadBeforeHandles);
    }
    let payload_end = payload_off
        .checked_add(header.payload_len as usize)
        .ok_or(StartupParseError::GeometryOverflow)?;
    if payload_end != block.len() {
        return Err(StartupParseError::LengthMismatch);
    }
    if block[handles_end..payload_off].iter().any(|byte| *byte != 0) {
        return Err(StartupParseError::NonzeroPadding);
    }
    for index in 0..header.handle_count as usize {
        let offset = core::mem::size_of::<StartupBlockHeader>()
            + index * core::mem::size_of::<Handle>();
        // SAFETY: 规范几何已证明本项完整位于 block 内；输入无需对齐。
        let handle = unsafe {
            core::ptr::read_unaligned(block.as_ptr().byte_add(offset).cast::<Handle>())
        };
        if !handle.is_valid() {
            return Err(StartupParseError::InvalidHandle);
        }
    }
    Ok(header)
}

/// 以实际 child-local Handle 构造完整、紧凑的启动块。
///
/// Handle 数值由 child HandleTable reservation 提供，不能由数组下标推导。
pub fn build_startup_block(
    pid: Pid,
    parent_pid: Pid,
    handles: &[Handle],
    payload: &[u8],
) -> Result<Vec<u8>, StartupBuildError> {
    let handle_bytes = handles
        .len()
        .checked_mul(core::mem::size_of::<Handle>())
        .ok_or(StartupBuildError::Overflow)?;
    let payload_off = core::mem::size_of::<StartupBlockHeader>()
        .checked_add(handle_bytes)
        .ok_or(StartupBuildError::Overflow)?;
    let mut block = build_startup_prefix(pid, parent_pid, handles, payload_off, payload.len())?;
    block
        .try_reserve_exact(payload.len())
        .map_err(|_| StartupBuildError::AllocationFailed)?;
    block.extend_from_slice(payload);
    Ok(block)
}

/// 构造 `[header][Handles][zero padding]` prefix，payload 由调用方以其他
/// backing 紧接在 `payload_off` 映射。返回 Vec 长度恒为 payload_off。
pub fn build_startup_prefix(
    pid: Pid,
    parent_pid: Pid,
    handles: &[Handle],
    payload_off: usize,
    payload_len: usize,
) -> Result<Vec<u8>, StartupBuildError> {
    let handle_bytes = handles
        .len()
        .checked_mul(core::mem::size_of::<Handle>())
        .ok_or(StartupBuildError::Overflow)?;
    let handles_end = core::mem::size_of::<StartupBlockHeader>()
        .checked_add(handle_bytes)
        .ok_or(StartupBuildError::Overflow)?;
    if payload_off < handles_end {
        return Err(StartupBuildError::Overflow);
    }
    let total = payload_off
        .checked_add(payload_len)
        .ok_or(StartupBuildError::Overflow)?;
    let header = StartupBlockHeader {
        magic: STARTUP_BLOCK_MAGIC,
        block_len: u32::try_from(total).map_err(|_| StartupBuildError::Overflow)?,
        version: STARTUP_VERSION,
        reserved0: 0,
        pid,
        parent_pid,
        handle_count: u32::try_from(handles.len()).map_err(|_| StartupBuildError::Overflow)?,
        payload_off: u32::try_from(payload_off).map_err(|_| StartupBuildError::Overflow)?,
        payload_len: u32::try_from(payload_len).map_err(|_| StartupBuildError::Overflow)?,
        reserved: 0,
    };

    let mut prefix = Vec::new();
    prefix
        .try_reserve_exact(payload_off)
        .map_err(|_| StartupBuildError::AllocationFailed)?;
    append_value(&mut prefix, &header);
    append_values(&mut prefix, handles);
    prefix.resize(payload_off, 0);
    Ok(prefix)
}

fn append_value<T: Copy>(output: &mut Vec<u8>, value: &T) {
    // SAFETY: StartupBlockHeader 所有字段均已初始化且布局无 padding。
    let bytes = unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    };
    output.extend_from_slice(bytes);
}

fn append_values<T: Copy>(output: &mut Vec<u8>, values: &[T]) {
    // SAFETY: Handle 是无 padding 的 u64 newtype；切片长度按 size_of_val 取值。
    let bytes = unsafe {
        core::slice::from_raw_parts(values.as_ptr().cast::<u8>(), core::mem::size_of_val(values))
    };
    output.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_carries_actual_handles_and_opaque_payload() {
        let handles = [
            Handle::from_parts(7, 3),
            Handle::from_parts(2, u32::MAX - 1),
        ];
        let payload = b"\0launcher\xffpayload";
        let block = build_startup_block(11, 4, &handles, payload)
            .expect("valid startup block must assemble");

        // SAFETY: 块由组装器构造且至少包含完整头。
        let header =
            unsafe { core::ptr::read_unaligned(block.as_ptr().cast::<StartupBlockHeader>()) };
        assert_eq!(header.magic, STARTUP_BLOCK_MAGIC);
        assert_eq!(header.version, STARTUP_VERSION);
        assert_eq!((header.pid, header.parent_pid), (11, 4));
        assert_eq!(header.handle_count, 2);
        assert_eq!(header.payload_off as usize, core::mem::size_of::<StartupBlockHeader>() + 2 * core::mem::size_of::<Handle>());
        assert_eq!(header.payload_len as usize, payload.len());
        assert_eq!(header.block_len as usize, block.len());
        assert_eq!(header.reserved0, 0);
        assert_eq!(header.reserved, 0);

        let mut actual = [Handle::INVALID; 2];
        for (index, output) in actual.iter_mut().enumerate() {
            let offset = core::mem::size_of::<StartupBlockHeader>()
                + index * core::mem::size_of::<Handle>();
            // SAFETY: builder 已输出完整 Handle 字节；测试 Vec 基址无需对齐。
            *output = unsafe {
                core::ptr::read_unaligned(block.as_ptr().byte_add(offset).cast::<Handle>())
            };
        }
        assert_eq!(actual, handles);
        assert_eq!(validate_startup_block(&block), Ok(header));
        assert_eq!(&block[header.payload_off as usize..], payload);
    }

    #[test]
    fn empty_handles_and_payload_are_valid() {
        let block = build_startup_block(1, 0, &[], &[])
            .expect("empty startup resources must be valid");
        let header = validate_startup_block(&block).expect("empty block must validate");
        assert_eq!(header.handle_count, 0);
        assert_eq!(header.payload_len, 0);
        assert_eq!(header.payload_off as usize, core::mem::size_of::<StartupBlockHeader>());
        assert_eq!(block.len(), core::mem::size_of::<StartupBlockHeader>());
    }

    #[test]
    fn padded_prefix_can_back_an_external_payload() {
        let handles = [Handle::from_parts(3, 7)];
        let mut block = build_startup_prefix(2, 1, &handles, 4096, 7)
            .expect("page-aligned prefix must assemble");
        assert_eq!(block.len(), 4096);
        block.extend_from_slice(b"payload");
        let header = validate_startup_block(&block).expect("padded block must validate");
        assert_eq!(header.payload_off, 4096);
        assert_eq!(&block[4096..], b"payload");

        block[128] = 1;
        assert_eq!(
            validate_startup_block(&block),
            Err(StartupParseError::NonzeroPadding)
        );
    }

    #[test]
    fn validator_rejects_outer_geometry_corruption() {
        assert_eq!(
            validate_startup_block(&[0; 8]),
            Err(StartupParseError::TruncatedHeader)
        );

        let valid = build_startup_block(1, 0, &[Handle::from_parts(9, 4)], b"payload")
            .expect("fixture must assemble");
        let mut corrupted = valid.clone();
        corrupted[0] ^= 1;
        assert_eq!(
            validate_startup_block(&corrupted),
            Err(StartupParseError::BadMagic)
        );

        let mut corrupted = valid.clone();
        let payload_off = core::mem::offset_of!(StartupBlockHeader, payload_off);
        corrupted[payload_off..payload_off + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            validate_startup_block(&corrupted),
            Err(StartupParseError::PayloadBeforeHandles)
        );

        let mut corrupted = valid.clone();
        let reserved = core::mem::offset_of!(StartupBlockHeader, reserved);
        corrupted[reserved..reserved + 4].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            validate_startup_block(&corrupted),
            Err(StartupParseError::NonzeroReserved)
        );

        let mut corrupted = valid;
        corrupted.truncate(corrupted.len() - 1);
        assert_eq!(
            validate_startup_block(&corrupted),
            Err(StartupParseError::LengthMismatch)
        );
    }
}
