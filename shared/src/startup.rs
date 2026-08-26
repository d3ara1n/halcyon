//! 版本化启动块（StartupBlock）：launch 事务把授权方组装的清单字节只读
//! 映射进新进程地址空间，入口 `a0` 指向块基。内容对内核不透明——tag、
//! descriptor 与 payload 的语义属于授权方与接收方运行时（rinlib）之间的
//! 版本化协议；内核只负责复制字节与按数组序安装 Handle。

use alloc::vec::Vec;

use crate::object::Handle;
use crate::proc::Pid;

/// 块格式版本。
pub const STARTUP_VERSION: u16 = 1;

/// 块基魔数（"STARTUPB"）。
pub const STARTUP_BLOCK_MAGIC: u64 = u64::from_le_bytes(*b"STARTUPB");

/// descriptor 无关联 Handle 的 `handle_index` 哨兵。
pub const NO_HANDLE: u32 = u32::MAX;

// —— 标准 tag（授权方与接收方库共享的常量；内核不解释语义）——

/// 服务出生自带的邮箱 owner（授权方惯例，非内核机制）。
pub const TAG_MAILBOX_OWNER: u64 = 1;
/// init 持有的 pm 邮箱 sender（内核 boot loader ↔ init 私有协议）。
pub const TAG_PM_MAILBOX: u64 = 2;
/// init 的 initfs 归档字节（boot loader ↔ init 私有协议；服务化阶段启用）。
pub const TAG_INITFS_ARCHIVE: u64 = 3;

/// launch 槽位约定：按数组顺序安装的第 `index` 个 Handle（0 起）。
/// 新进程 Handle 表为空表，顺序安装必落槽位 `index + 1`、generation 1；
/// 内核在安装时断言成立，接收方据此从块内 index 复原 Handle 数值。
pub const fn startup_handle(index: u32) -> Handle {
    Handle::from_parts(index + 1, 1)
}

/// 启动块头：位于块基，随后是 `descriptor_count` 个 descriptor，最后是
/// 各 descriptor 经 `data_off`/`data_len` 引用的 payload 字节区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct StartupBlockHeader {
    pub magic: u64,
    /// 块总长（字节，含头、descriptor 表与 payload）。
    pub block_len: u32,
    pub version: u16,
    pub reserved0: u16,
    pub pid: Pid,
    pub parent_pid: Pid,
    pub descriptor_count: u32,
    /// launch 安装的 Handle 总数；descriptor 的 `handle_index` 上界。
    pub handle_count: u32,
    pub reserved: [u32; 2],
}

/// 一项启动资源描述：tag 语义属授权方 ↔ 接收方协议；Handle 引用与
/// payload 引用可独立或并存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct StartupDescriptor {
    pub tag: u64,
    /// 关联的 Handle 序号（见 [`startup_handle`]），或 [`NO_HANDLE`]。
    pub handle_index: u32,
    /// payload 区相对块基的偏移（字节）。
    pub data_off: u32,
    /// payload 长度（字节）。
    pub data_len: u32,
    pub reserved: u32,
}

const _: () = {
    assert!(core::mem::size_of::<StartupBlockHeader>() == 40);
    assert!(core::mem::size_of::<StartupDescriptor>() == 24);
};

/// 组装失败：授权方构造出接收方必拒绝的块，或无法表示的几何。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupBuildError {
    /// 区段偏移/长度超出 u32 表示域或总长溢出。
    Overflow,
    /// tag 重复（查找键必须唯一）。
    DuplicateTag,
    /// `handle_index` 超出 `handle_count` 上界。
    HandleIndexOutOfRange,
}

/// 启动块组装器：授权方（当前为内核 boot loader，未来为 init/pm）构造
/// manifest 字节。`handle_index` 必须与 launch 传入的 Handle 数组序一致。
/// payload 区偏移在 [`StartupManifest::finish`] 统一计算——descriptor 表
/// 长度随后续 add 增长，提前计算会与表区重叠。
#[derive(Debug, Default)]
pub struct StartupManifest {
    entries: Vec<ManifestEntry>,
    payload: Vec<u8>,
}

#[derive(Debug)]
struct ManifestEntry {
    tag: u64,
    handle_index: u32,
    /// payload 区内的 [start, end) 字节区间。
    data: (usize, usize),
}

impl StartupManifest {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            payload: Vec::new(),
        }
    }

    /// 追加一项资源描述：`handle_index = Some(i)` 表示第 i 个安装的
    /// Handle；`data` 为可选的 payload 字节（如字符串、路由表、归档）。
    pub fn add(&mut self, tag: u64, handle_index: Option<u32>, data: &[u8]) -> &mut Self {
        let start = self.payload.len();
        self.payload.extend_from_slice(data);
        self.entries.push(ManifestEntry {
            tag,
            handle_index: handle_index.unwrap_or(NO_HANDLE),
            data: (start, self.payload.len()),
        });
        self
    }

    /// 产出完整块字节：`[header][descriptors][payload]`。拒绝接收方必
    /// 拒绝的几何（重复 tag、越界 `handle_index`）与无法表示的尺寸。
    pub fn finish(
        self,
        pid: Pid,
        parent_pid: Pid,
        handle_count: u32,
    ) -> Result<Vec<u8>, StartupBuildError> {
        let descriptor_bytes = self
            .entries
            .len()
            .checked_mul(core::mem::size_of::<StartupDescriptor>())
            .ok_or(StartupBuildError::Overflow)?;
        let payload_base = core::mem::size_of::<StartupBlockHeader>() + descriptor_bytes;
        let total = payload_base
            .checked_add(self.payload.len())
            .ok_or(StartupBuildError::Overflow)?;
        let mut seen = alloc::collections::BTreeSet::new();
        for entry in &self.entries {
            if !seen.insert(entry.tag) {
                return Err(StartupBuildError::DuplicateTag);
            }
            if entry.handle_index != NO_HANDLE && entry.handle_index >= handle_count {
                return Err(StartupBuildError::HandleIndexOutOfRange);
            }
        }
        let header = StartupBlockHeader {
            magic: STARTUP_BLOCK_MAGIC,
            block_len: u32::try_from(total).map_err(|_| StartupBuildError::Overflow)?,
            version: STARTUP_VERSION,
            reserved0: 0,
            pid,
            parent_pid,
            descriptor_count: u32::try_from(self.entries.len())
                .map_err(|_| StartupBuildError::Overflow)?,
            handle_count,
            reserved: [0; 2],
        };
        let mut block = Vec::new();
        block
            .try_reserve_exact(total)
            .map_err(|_| StartupBuildError::Overflow)?;
        append_value(&mut block, &header);
        for entry in &self.entries {
            append_value(
                &mut block,
                &StartupDescriptor {
                    tag: entry.tag,
                    handle_index: entry.handle_index,
                    data_off: u32::try_from(payload_base + entry.data.0)
                        .map_err(|_| StartupBuildError::Overflow)?,
                    data_len: u32::try_from(entry.data.1 - entry.data.0)
                        .map_err(|_| StartupBuildError::Overflow)?,
                    reserved: 0,
                },
            );
        }
        block.extend_from_slice(&self.payload);
        Ok(block)
    }
}

fn append_value<T: Copy>(output: &mut Vec<u8>, value: &T) {
    // SAFETY: Startup ABI structs are fully initialized and contain no padding.
    let bytes = unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    };
    output.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 布局真值：块内区段几何由头与 descriptor 的偏移共同决定，
    /// 组装器输出必须与接收方（rinlib env）的重读几何完全一致。
    #[test]
    fn manifest_layout_roundtrips() {
        let mut manifest = StartupManifest::new();
        manifest
            .add(TAG_PM_MAILBOX, Some(0), &[])
            .add(TAG_INITFS_ARCHIVE, None, b"archive bytes")
            .add(0xdead_beef, Some(1), b"");
        let block = manifest
            .finish(7, 0, 2)
            .expect("valid manifest must assemble");

        // SAFETY: 块是刚构造的字节向量，头长度已由组装器保证。
        let header =
            unsafe { core::ptr::read_unaligned(block.as_ptr().cast::<StartupBlockHeader>()) };
        assert_eq!(header.magic, STARTUP_BLOCK_MAGIC);
        assert_eq!(header.version, STARTUP_VERSION);
        assert_eq!((header.pid, header.parent_pid), (7, 0));
        assert_eq!(header.descriptor_count, 3);
        assert_eq!(header.handle_count, 2);
        assert_eq!(header.block_len as usize, block.len());
        assert_eq!(header.reserved0, 0);
        assert_eq!(header.reserved, [0; 2]);

        let descriptors = unsafe {
            core::slice::from_raw_parts(
                block.as_ptr().byte_add(core::mem::size_of::<StartupBlockHeader>()).cast::<StartupDescriptor>(),
                3,
            )
        };
        let [pm, archive, extra] = descriptors else { unreachable!() };
        assert_eq!((*pm).tag, TAG_PM_MAILBOX);
        assert_eq!((*pm).handle_index, 0);
        assert_eq!((*archive).handle_index, NO_HANDLE);
        assert_eq!((*archive).data_off as usize,
            core::mem::size_of::<StartupBlockHeader>() + 3 * core::mem::size_of::<StartupDescriptor>());
        assert_eq!((*archive).data_len as usize, b"archive bytes".len());
        assert_eq!((*extra).tag, 0xdead_beef);
        for descriptor in descriptors {
            assert_eq!(descriptor.reserved, 0);
            assert!(descriptor.data_off as usize + descriptor.data_len as usize <= block.len());
        }
        assert_eq!(&block[(*archive).data_off as usize..(*archive).data_off as usize + (*archive).data_len as usize], b"archive bytes");
    }

    /// 槽位约定：handle_index i ↔ Handle::from_parts(i + 1, 1)，
    /// 与内核空表顺序安装的 slot/generation 对应。
    #[test]
    fn slot_contract_matches_fresh_table_geometry() {
        assert_eq!(startup_handle(0), Handle::from_parts(1, 1));
        assert_eq!(startup_handle(2), Handle::from_parts(3, 1));
        assert!(startup_handle(0).is_valid());
    }

    /// 组装器拒绝接收方必拒绝的几何：重复 tag、越界 handle_index。
    #[test]
    fn builder_rejects_invalid_geometry() {
        let mut manifest = StartupManifest::new();
        manifest.add(TAG_PM_MAILBOX, Some(0), &[]).add(TAG_PM_MAILBOX, None, &[]);
        assert_eq!(
            manifest.finish(1, 0, 1).unwrap_err(),
            StartupBuildError::DuplicateTag
        );

        let mut manifest = StartupManifest::new();
        manifest.add(TAG_PM_MAILBOX, Some(1), &[]);
        assert_eq!(
            manifest.finish(1, 0, 1).unwrap_err(),
            StartupBuildError::HandleIndexOutOfRange
        );

        // NO_HANDLE 哨兵不受 handle_count 约束。
        let mut manifest = StartupManifest::new();
        manifest.add(TAG_INITFS_ARCHIVE, None, b"x");
        assert!(manifest.finish(1, 0, 0).is_ok());
    }
}
