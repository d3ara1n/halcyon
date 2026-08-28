#![no_std]

//! 用户态 ELF process loader：解析映像并驱动 affine ProcessBuilder。
//! `race` 模块是生命周期竞态矩阵验证负载（init ↔ srv_hammer）的线协议。

extern crate alloc;

pub mod race;

use alloc::collections::BTreeMap;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{
        ExecutionProfile, HandleGrant, JobMemberKind, ProcessMapFlags,
        ProcessStartDescriptor, PROCESS_MAIN_STACK_SIZE, PROCESS_PAGE_SIZE, PROCESS_USER_TOP,
        JOB_ENUMERATE_MAX,
    },
    wait::{WaitItem, WAIT_TIMEOUT_INFINITE},
};
use rinlib::{ipc::object::close, ipc::wait::wait_many, process};

const MAX_MAP_BYTES: usize = 256 * PROCESS_PAGE_SIZE;
const MAX_WRITE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    Elf(elf::ElfError),
    Requirement(elf::IsaReqError),
    InvalidImage,
    System(SystemCallError),
}

impl From<SystemCallError> for SpawnError {
    fn from(error: SystemCallError) -> Self {
        Self::System(error)
    }
}

pub struct SpawnRequest<'a> {
    pub job: Handle,
    pub image: &'a [u8],
    pub payload: &'a [u8],
    pub grants: &'a [HandleGrant],
    pub control_rights: Rights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spawned {
    pub pid: u64,
    pub control: Handle,
}

/// 解析静态 ET_EXEC，构造地址空间并首次发布进程。
pub fn spawn(request: SpawnRequest<'_>) -> Result<Spawned, SpawnError> {
    let image = elf::parse(request.image).map_err(SpawnError::Elf)?;
    let requirement = elf::isa_requirement(request.image).map_err(SpawnError::Requirement)?;
    let plan = page_plan(&image, request.image.len())?;

    let created = process::create(request.job, request.control_rights)?;
    let builder = created.builder;

    let result = (|| {
        map_plan(builder, &plan)?;
        write_segments(builder, &image, request.image)?;
        map_stack(builder)?;

        let profile = match requirement {
            elf::IsaRequirement::Base64 => ExecutionProfile::Base64,
            elf::IsaRequirement::D64 => ExecutionProfile::D64,
        };
        let descriptor = ProcessStartDescriptor {
            entry: image.entry,
            stack_pointer: PROCESS_USER_TOP as u64,
            payload_ptr: request.payload.as_ptr() as u64,
            grants_ptr: request.grants.as_ptr() as u64,
            payload_len: u32::try_from(request.payload.len()).map_err(|_| SpawnError::InvalidImage)?,
            grant_count: u32::try_from(request.grants.len()).map_err(|_| SpawnError::InvalidImage)?,
            profile: profile as u32,
            reserved: 0,
        };
        process::start(builder, &descriptor)?;
        Ok(Spawned { pid: created.pid, control: created.control })
    })();

    if result.is_err() {
        // Start/map/write 失败保持 builder；关闭它触发 Building abandonment
        // → REAPABLE，随后持 control 把收束推进到 Complete 再关闭。
        let _ = close(builder);
        let _ = process::drain_to_completion(created.control);
        let _ = close(created.control);
    }
    result
}

/// 派生监督所需的成员/子域 control rights：kill+drain 需 MANAGE，等
/// REAPABLE/CLOSED 需 WAIT，终态查询需 READ。调用者的 JobControl 必须
/// 持其超集，否则派生返回 RightsDenied。
pub const DERIVED_CONTROL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw() | Rights::WAIT.raw() | Rights::MANAGE.raw(),
);

/// 递归 JobKill（用户态政策，内核不递归）：逐层 `JobSeal → 枚举 members
/// → 派生 kill → 等 REAPABLE → drain 至 Complete → 枚举 children 递归 →
/// 等本层 CLOSED`。先 seal 后枚举：封口后成员集单调收缩，收敛性由
/// 「枚举至空 + 派生 NotFound 即已完成」表达。派生 NotFound（目标已
/// 完成移表，ID 不复用永不错指）跳过不报错。返回时本 Job 及全部后代
/// 均已达 Dead/CLOSED（root 亦发 CLOSED 但不移除）。
/// 要求：JobControl 持 MANAGE（seal/派生）与 READ（枚举）。
pub fn job_kill(job: Handle, code: i64) -> Result<(), SystemCallError> {
    process::seal_job(job)?;
    kill_members(job, code)?;
    kill_children(job, code)?;
    // 完成屏障：直接成员全部完成后 CLOSED 置位（等待 CLOSED 即「直接
    // 成员全部完成」，含子 Job 传播）。
    wait_many(
        &[WaitItem::new(job, ObjectSignals::CLOSED, 0)],
        WAIT_TIMEOUT_INFINITE,
    )?;
    Ok(())
}

fn kill_members(job: Handle, code: i64) -> Result<(), SystemCallError> {
    for pid in enumerate_members(job, JobMemberKind::MemberProcesses)? {
        let control = match process::derive_job(
            job,
            JobMemberKind::MemberProcesses,
            pid,
            DERIVED_CONTROL_RIGHTS,
        ) {
            Ok(control) => control,
            // 成员已完成移表：收敛方向，跳过。
            Err(SystemCallError::ObjectNotFound) => continue,
            Err(error) => return Err(error),
        };
        process::kill(control, code)?;
        // kill 是异步请求：等 REAPABLE/CLOSED 后 drain 至 Complete。
        wait_many(
            &[WaitItem::new(control, ObjectSignals::REAPABLE | ObjectSignals::CLOSED, 0)],
            WAIT_TIMEOUT_INFINITE,
        )?;
        process::drain_to_completion(control)?;
        close(control)?;
    }
    Ok(())
}

fn kill_children(job: Handle, code: i64) -> Result<(), SystemCallError> {
    for jid in enumerate_members(job, JobMemberKind::ChildJobs)? {
        let child = match process::derive_job(
            job,
            JobMemberKind::ChildJobs,
            jid,
            DERIVED_CONTROL_RIGHTS,
        ) {
            Ok(child) => child,
            // child 已完成移表：跳过。
            Err(SystemCallError::ObjectNotFound) => continue,
            Err(error) => return Err(error),
        };
        job_kill(child, code)?;
        close(child)?;
    }
    Ok(())
}

/// 枚举至耗尽。占位屏障的零进展批（more=1 ∧ actual=0）以原 cursor
/// 重试——占位窗口在创建方单个 syscall 内，重试不活锁。
pub fn enumerate_members(
    job: Handle,
    kind: JobMemberKind,
) -> Result<alloc::vec::Vec<u64>, SystemCallError> {
    let mut ids = alloc::vec::Vec::new();
    let mut buf = [0u64; JOB_ENUMERATE_MAX];
    let mut cursor = 0u64;
    loop {
        let result = process::enumerate_job(job, kind, cursor, &mut buf)?;
        ids.extend_from_slice(&buf[..result.actual as usize]);
        if result.more == 0 {
            return Ok(ids);
        }
        cursor = result.next_cursor;
    }
}

fn page_plan(
    image: &elf::Elf,
    file_len: usize,
) -> Result<BTreeMap<usize, ProcessMapFlags>, SpawnError> {
    let mut plan: BTreeMap<usize, ProcessMapFlags> = BTreeMap::new();
    let image_limit = PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE;
    let entry = usize::try_from(image.entry).map_err(|_| SpawnError::InvalidImage)?;
    let mut entry_executable = false;
    let mut previous_end = 0usize;
    for segment in &image.segments {
        if segment.filesz > segment.memsz {
            return Err(SpawnError::InvalidImage);
        }
        let start = usize::try_from(segment.vaddr).map_err(|_| SpawnError::InvalidImage)?;
        let offset = usize::try_from(segment.offset).map_err(|_| SpawnError::InvalidImage)?;
        if start % PROCESS_PAGE_SIZE != offset % PROCESS_PAGE_SIZE
            || segment.writable && !segment.readable
        {
            return Err(SpawnError::InvalidImage);
        }
        let memsz = usize::try_from(segment.memsz).map_err(|_| SpawnError::InvalidImage)?;
        let filesz = usize::try_from(segment.filesz).map_err(|_| SpawnError::InvalidImage)?;
        let end = start.checked_add(memsz).ok_or(SpawnError::InvalidImage)?;
        let file_end = offset.checked_add(filesz).ok_or(SpawnError::InvalidImage)?;
        if memsz == 0 || end > image_limit || file_end > file_len || start < previous_end {
            return Err(SpawnError::InvalidImage);
        }
        previous_end = end;
        if segment.executable && start <= entry && entry < end {
            entry_executable = true;
        }
        let mut permissions = ProcessMapFlags::from_raw(0);
        if segment.readable {
            permissions = permissions | ProcessMapFlags::READ;
        }
        if segment.writable {
            permissions = permissions | ProcessMapFlags::WRITE;
        }
        if segment.executable {
            permissions = permissions | ProcessMapFlags::EXECUTE;
        }
        if permissions.raw() == 0 {
            return Err(SpawnError::InvalidImage);
        }
        for vpn in start / PROCESS_PAGE_SIZE..end.div_ceil(PROCESS_PAGE_SIZE) {
            let previous = plan
                .get(&vpn)
                .copied()
                .unwrap_or(ProcessMapFlags::from_raw(0));
            let combined = previous | permissions;
            if combined.contains(ProcessMapFlags::WRITE | ProcessMapFlags::EXECUTE) {
                return Err(SpawnError::InvalidImage);
            }
            plan.insert(vpn, combined);
        }
    }
    if plan.is_empty() || !entry_executable {
        return Err(SpawnError::InvalidImage);
    }
    Ok(plan)
}

fn map_plan(
    builder: Handle,
    plan: &BTreeMap<usize, ProcessMapFlags>,
) -> Result<(), SpawnError> {
    let mut entries = plan.iter().peekable();
    while let Some((&start_vpn, &permissions)) = entries.next() {
        let mut pages = 1usize;
        while let Some(&(&next_vpn, &next_permissions)) = entries.peek() {
            if next_vpn != start_vpn + pages
                || next_permissions != permissions
                || (pages + 1) * PROCESS_PAGE_SIZE > MAX_MAP_BYTES
            {
                break;
            }
            entries.next();
            pages += 1;
        }
        process::map(
            builder,
            start_vpn * PROCESS_PAGE_SIZE,
            pages * PROCESS_PAGE_SIZE,
            permissions,
        )?;
    }
    Ok(())
}

fn write_segments(
    builder: Handle,
    image: &elf::Elf,
    file: &[u8],
) -> Result<(), SpawnError> {
    for segment in &image.segments {
        let offset = usize::try_from(segment.offset).map_err(|_| SpawnError::InvalidImage)?;
        let filesz = usize::try_from(segment.filesz).map_err(|_| SpawnError::InvalidImage)?;
        let source = file
            .get(offset..offset.checked_add(filesz).ok_or(SpawnError::InvalidImage)?)
            .ok_or(SpawnError::InvalidImage)?;
        let target = usize::try_from(segment.vaddr).map_err(|_| SpawnError::InvalidImage)?;
        for (index, chunk) in source.chunks(MAX_WRITE_BYTES).enumerate() {
            process::write(builder, target + index * MAX_WRITE_BYTES, chunk)?;
        }
    }
    Ok(())
}

fn map_stack(builder: Handle) -> Result<(), SpawnError> {
    let base = PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE;
    let permissions = ProcessMapFlags::READ | ProcessMapFlags::WRITE;
    for offset in (0..PROCESS_MAIN_STACK_SIZE).step_by(MAX_MAP_BYTES) {
        let len = MAX_MAP_BYTES.min(PROCESS_MAIN_STACK_SIZE - offset);
        process::map(builder, base + offset, len, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn segment(
        vaddr: u64,
        offset: u64,
        memsz: u64,
        readable: bool,
        writable: bool,
        executable: bool,
    ) -> elf::LoadSegment {
        elf::LoadSegment {
            vaddr,
            offset,
            filesz: 0,
            memsz,
            readable,
            writable,
            executable,
        }
    }

    #[test]
    fn entry_must_lie_in_executable_segment() {
        let image = elf::Elf {
            entry: 0x3000,
            segments: vec![segment(0x1000, 0, 0x1000, true, false, true)],
        };
        assert_eq!(page_plan(&image, 0), Err(SpawnError::InvalidImage));
    }

    #[test]
    fn overlapping_segment_bytes_are_rejected() {
        let image = elf::Elf {
            entry: 0x1000,
            segments: vec![
                segment(0x1000, 0, 0x1800, true, false, true),
                segment(0x2000, 0, 0x1000, true, false, false),
            ],
        };
        assert_eq!(page_plan(&image, 0), Err(SpawnError::InvalidImage));
    }

    #[test]
    fn page_level_write_execute_union_is_rejected() {
        let image = elf::Elf {
            entry: 0x1000,
            segments: vec![
                segment(0x1000, 0, 0x800, true, false, true),
                segment(0x1800, 0x800, 0x800, true, true, false),
            ],
        };
        assert_eq!(page_plan(&image, 0x800), Err(SpawnError::InvalidImage));
    }

    #[test]
    fn valid_split_permissions_produce_page_plan() {
        let image = elf::Elf {
            entry: 0x1000,
            segments: vec![
                segment(0x1000, 0, 0x1000, true, false, true),
                segment(0x2000, 0, 0x1000, true, true, false),
            ],
        };
        let plan = page_plan(&image, 0).unwrap();
        assert_eq!(plan[&1], ProcessMapFlags::READ | ProcessMapFlags::EXECUTE);
        assert_eq!(plan[&2], ProcessMapFlags::READ | ProcessMapFlags::WRITE);
    }
}
