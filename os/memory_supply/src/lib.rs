//! 启动期物理供给规划器。
//!
//! 输入是平台已经接纳的 managed、permanent 与 boot-held 区间；输出在固定容量内
//! 确定性地隔离 FramePool metadata、内核 heap chunks、recovery tickets 与 user
//! inventory。调用者提供固定 workspace，规划过程不分配内存；失败只使未发布的
//! workspace 内容无效，不产生可消费的部分计划。

#![no_std]
#![forbid(unsafe_code)]

/// 页对齐的物理半开区间。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    start: usize,
    end: usize,
}

impl Range {
    pub const EMPTY: Self = Self { start: 0, end: 0 };

    pub fn new(start: usize, end: usize) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }

    pub const fn start(self) -> usize {
        self.start
    }

    pub const fn end(self) -> usize {
        self.end
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }
}

/// 系统储备的编译期容量政策。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requirements {
    pub page_size: usize,
    pub metadata_bytes: usize,
    pub heap_chunk_size: usize,
    pub heap_chunk_count: usize,
    pub recovery_ticket_size: usize,
    pub recovery_ticket_count: usize,
}

/// 规划失败原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    InvalidRequirements,
    InvalidRange,
    MemoryOverlap,
    CapacityExhausted,
    ArithmeticOverflow,
    InsufficientMetadata,
    InsufficientHeap,
    InsufficientRecovery,
    ClassificationMismatch,
}

/// FramePool 外置 metadata 的唯一物理所有权。
#[derive(Debug)]
pub struct FramePoolMetadata {
    range: Range,
}

impl FramePoolMetadata {
    pub const fn range(&self) -> Range {
        self.range
    }
}

/// 已清零后可单向交给内核 heap 的连续物理块。
#[derive(Debug)]
pub struct HeapChunkTicket {
    range: Range,
}

impl HeapChunkTicket {
    pub const fn range(&self) -> Range {
        self.range
    }

    pub const fn into_range(self) -> Range {
        self.range
    }
}

/// 只允许显式完成/恢复机制消费的物理票据。
#[derive(Debug)]
pub struct RecoveryTicket {
    range: Range,
}

impl RecoveryTicket {
    pub const fn range(&self) -> Range {
        self.range
    }

    pub const fn into_range(self) -> Range {
        self.range
    }
}

/// 物理隔离的系统供给。不同用途没有公共的可消费 ticket 类型。
pub struct SystemSupply<const HEAP: usize, const RECOVERY: usize> {
    metadata: FramePoolMetadata,
    heap: [Option<HeapChunkTicket>; HEAP],
    heap_next: usize,
    heap_count: usize,
    recovery: [Option<RecoveryTicket>; RECOVERY],
    recovery_next: usize,
    recovery_count: usize,
}

impl<const HEAP: usize, const RECOVERY: usize> SystemSupply<HEAP, RECOVERY> {
    pub const fn metadata(&self) -> &FramePoolMetadata {
        &self.metadata
    }

    /// 按物理地址顺序单向消费一个 heap chunk。
    pub fn take_heap_chunk(&mut self) -> Option<HeapChunkTicket> {
        if self.heap_next == self.heap_count {
            return None;
        }
        let ticket = self.heap[self.heap_next]
            .take()
            .expect("unconsumed heap ticket missing");
        self.heap_next += 1;
        Some(ticket)
    }

    pub const fn remaining_heap_chunks(&self) -> usize {
        self.heap_count - self.heap_next
    }

    /// 启动期预清零使用的只读几何，不转移 ticket 所有权。
    pub fn heap_ranges(&self) -> impl Iterator<Item = Range> + '_ {
        self.heap[..self.heap_count]
            .iter()
            .map(|ticket| ticket.as_ref().expect("heap ticket missing").range())
    }

    /// 按物理地址顺序单向消费一个 recovery ticket。
    pub fn take_recovery_ticket(&mut self) -> Option<RecoveryTicket> {
        if self.recovery_next == self.recovery_count {
            return None;
        }
        let ticket = self.recovery[self.recovery_next]
            .take()
            .expect("unconsumed recovery ticket missing");
        self.recovery_next += 1;
        Some(ticket)
    }

    pub const fn remaining_recovery_tickets(&self) -> usize {
        self.recovery_count - self.recovery_next
    }

    /// 启动期准备恢复资源使用的只读几何，不转移 ticket 所有权。
    pub fn recovery_ranges(&self) -> impl Iterator<Item = Range> + '_ {
        self.recovery[..self.recovery_count]
            .iter()
            .map(|ticket| ticket.as_ref().expect("recovery ticket missing").range())
    }
}

/// FramePool 可见的启动分类借用视图。
pub struct InventoryPlan<'a> {
    permanent: &'a [Range],
    boot_held: &'a [Range],
    user_free: &'a [Range],
    managed_bytes: usize,
    permanent_bytes: usize,
    boot_held_bytes: usize,
    system_bytes: usize,
    user_free_bytes: usize,
}

impl InventoryPlan<'_> {
    pub const fn permanent(&self) -> &[Range] {
        self.permanent
    }

    pub const fn boot_held(&self) -> &[Range] {
        self.boot_held
    }

    pub const fn user_free(&self) -> &[Range] {
        self.user_free
    }

    pub const fn managed_bytes(&self) -> usize {
        self.managed_bytes
    }

    pub const fn permanent_bytes(&self) -> usize {
        self.permanent_bytes
    }

    pub const fn boot_held_bytes(&self) -> usize {
        self.boot_held_bytes
    }

    pub const fn system_bytes(&self) -> usize {
        self.system_bytes
    }

    pub const fn user_free_bytes(&self) -> usize {
        self.user_free_bytes
    }
}

/// 原子规划成功后才能拆出的完整分类与系统票据。
pub struct Plan<'a, const HEAP: usize, const RECOVERY: usize> {
    inventory: InventoryPlan<'a>,
    system: SystemSupply<HEAP, RECOVERY>,
}

impl<'a, const HEAP: usize, const RECOVERY: usize> Plan<'a, HEAP, RECOVERY> {
    pub fn into_parts(self) -> (InventoryPlan<'a>, SystemSupply<HEAP, RECOVERY>) {
        (self.inventory, self.system)
    }
}

struct RangeBuffer<const N: usize> {
    entries: [Range; N],
    len: usize,
}

impl<const N: usize> RangeBuffer<N> {
    const fn new() -> Self {
        Self {
            entries: [Range::EMPTY; N],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn as_slice(&self) -> &[Range] {
        &self.entries[..self.len]
    }

    fn push(&mut self, range: Range) -> Result<(), PlanError> {
        let slot = self
            .entries
            .get_mut(self.len)
            .ok_or(PlanError::CapacityExhausted)?;
        *slot = range;
        self.len += 1;
        Ok(())
    }

    fn sort(&mut self) {
        self.entries[..self.len].sort_unstable_by_key(|range| range.start);
    }

    fn normalize(&mut self) {
        self.sort();
        let mut output = 0usize;
        for input in 0..self.len {
            let range = self.entries[input];
            if output > 0 && range.start <= self.entries[output - 1].end {
                self.entries[output - 1].end = self.entries[output - 1].end.max(range.end);
            } else {
                self.entries[output] = range;
                output += 1;
            }
        }
        self.len = output;
    }

    fn bytes(&self) -> Result<usize, PlanError> {
        self.as_slice().iter().try_fold(0usize, |total, range| {
            total
                .checked_add(range.len())
                .ok_or(PlanError::ArithmeticOverflow)
        })
    }
}

/// 调用者提供的固定启动 workspace。内核应把它放在静态存储而不是启动栈。
pub struct Planner<const RANGES: usize, const HEAP: usize, const RECOVERY: usize> {
    memories: RangeBuffer<RANGES>,
    permanent: RangeBuffer<RANGES>,
    raw_boot: RangeBuffer<RANGES>,
    boot_held: RangeBuffer<RANGES>,
    unavailable: RangeBuffer<RANGES>,
    user_free: RangeBuffer<RANGES>,
    heap_ranges: [Range; HEAP],
    recovery_ranges: [Range; RECOVERY],
}

impl<const RANGES: usize, const HEAP: usize, const RECOVERY: usize>
    Planner<RANGES, HEAP, RECOVERY>
{
    pub const fn new() -> Self {
        Self {
            memories: RangeBuffer::new(),
            permanent: RangeBuffer::new(),
            raw_boot: RangeBuffer::new(),
            boot_held: RangeBuffer::new(),
            unavailable: RangeBuffer::new(),
            user_free: RangeBuffer::new(),
            heap_ranges: [Range::EMPTY; HEAP],
            recovery_ranges: [Range::EMPTY; RECOVERY],
        }
    }

    /// 在 workspace 内规划完整物理分类。只有 `Ok` 返回的借用视图允许发布。
    pub fn plan<'a>(
        &'a mut self,
        managed: &[Range],
        permanent: &[Range],
        boot_held: &[Range],
        requirements: Requirements,
    ) -> Result<Plan<'a, HEAP, RECOVERY>, PlanError> {
        validate_requirements::<HEAP, RECOVERY>(requirements)?;
        normalize_memories_into(managed, requirements.page_size, &mut self.memories)?;
        clip_and_normalize_into(
            permanent,
            &self.memories,
            requirements.page_size,
            &mut self.permanent,
        )?;
        clip_and_normalize_into(
            boot_held,
            &self.memories,
            requirements.page_size,
            &mut self.raw_boot,
        )?;
        subtract_into(&self.raw_boot, &self.permanent, &mut self.boot_held)?;

        copy_into(&self.permanent, &mut self.unavailable)?;
        append(&mut self.unavailable, &self.boot_held)?;
        self.unavailable.normalize();

        let metadata_len = align_up(requirements.metadata_bytes, requirements.page_size)?;
        let metadata_range = place_one(
            &self.memories,
            &self.unavailable,
            metadata_len,
            requirements.page_size,
        )?
        .ok_or(PlanError::InsufficientMetadata)?;
        self.unavailable.push(metadata_range)?;
        self.unavailable.normalize();

        for index in 0..requirements.heap_chunk_count {
            let range = place_one(
                &self.memories,
                &self.unavailable,
                requirements.heap_chunk_size,
                requirements.heap_chunk_size,
            )?
            .ok_or(PlanError::InsufficientHeap)?;
            self.heap_ranges[index] = range;
            self.unavailable.push(range)?;
            self.unavailable.normalize();
        }

        for index in 0..requirements.recovery_ticket_count {
            let range = place_one(
                &self.memories,
                &self.unavailable,
                requirements.recovery_ticket_size,
                requirements.page_size,
            )?
            .ok_or(PlanError::InsufficientRecovery)?;
            self.recovery_ranges[index] = range;
            self.unavailable.push(range)?;
            self.unavailable.normalize();
        }

        subtract_into(&self.memories, &self.unavailable, &mut self.user_free)?;
        let managed_bytes = self.memories.bytes()?;
        let permanent_bytes = self.permanent.bytes()?;
        let boot_held_bytes = self.boot_held.bytes()?;
        let system_bytes = metadata_len
            .checked_add(
                requirements
                    .heap_chunk_size
                    .checked_mul(requirements.heap_chunk_count)
                    .ok_or(PlanError::ArithmeticOverflow)?,
            )
            .and_then(|bytes| {
                requirements
                    .recovery_ticket_size
                    .checked_mul(requirements.recovery_ticket_count)
                    .and_then(|recovery| bytes.checked_add(recovery))
            })
            .ok_or(PlanError::ArithmeticOverflow)?;
        let user_free_bytes = self.user_free.bytes()?;
        let classified = permanent_bytes
            .checked_add(boot_held_bytes)
            .and_then(|bytes| bytes.checked_add(system_bytes))
            .and_then(|bytes| bytes.checked_add(user_free_bytes))
            .ok_or(PlanError::ArithmeticOverflow)?;
        if classified != managed_bytes {
            return Err(PlanError::ClassificationMismatch);
        }

        let heap = core::array::from_fn(|index| {
            (index < requirements.heap_chunk_count).then(|| HeapChunkTicket {
                range: self.heap_ranges[index],
            })
        });
        let recovery = core::array::from_fn(|index| {
            (index < requirements.recovery_ticket_count).then(|| RecoveryTicket {
                range: self.recovery_ranges[index],
            })
        });
        Ok(Plan {
            inventory: InventoryPlan {
                permanent: self.permanent.as_slice(),
                boot_held: self.boot_held.as_slice(),
                user_free: self.user_free.as_slice(),
                managed_bytes,
                permanent_bytes,
                boot_held_bytes,
                system_bytes,
                user_free_bytes,
            },
            system: SystemSupply {
                metadata: FramePoolMetadata {
                    range: metadata_range,
                },
                heap,
                heap_next: 0,
                heap_count: requirements.heap_chunk_count,
                recovery,
                recovery_next: 0,
                recovery_count: requirements.recovery_ticket_count,
            },
        })
    }
}

impl<const RANGES: usize, const HEAP: usize, const RECOVERY: usize> Default
    for Planner<RANGES, HEAP, RECOVERY>
{
    fn default() -> Self {
        Self::new()
    }
}

fn validate_requirements<const HEAP: usize, const RECOVERY: usize>(
    requirements: Requirements,
) -> Result<(), PlanError> {
    let page = requirements.page_size;
    if !page.is_power_of_two()
        || requirements.metadata_bytes == 0
        || requirements.heap_chunk_count > HEAP
        || requirements.recovery_ticket_count > RECOVERY
        || (requirements.heap_chunk_count > 0
            && (!requirements.heap_chunk_size.is_power_of_two()
                || requirements.heap_chunk_size < page))
        || (requirements.recovery_ticket_count > 0
            && (requirements.recovery_ticket_size == 0
                || requirements.recovery_ticket_size % page != 0))
    {
        return Err(PlanError::InvalidRequirements);
    }
    Ok(())
}

fn normalize_memories_into<const N: usize>(
    input: &[Range],
    page_size: usize,
    output: &mut RangeBuffer<N>,
) -> Result<(), PlanError> {
    output.clear();
    for &range in input {
        validate_range(range, page_size)?;
        output.push(range)?;
    }
    output.sort();
    for pair in output.as_slice().windows(2) {
        if pair[0].end > pair[1].start {
            return Err(PlanError::MemoryOverlap);
        }
    }
    Ok(())
}

fn clip_and_normalize_into<const N: usize>(
    input: &[Range],
    memories: &RangeBuffer<N>,
    page_size: usize,
    output: &mut RangeBuffer<N>,
) -> Result<(), PlanError> {
    output.clear();
    for &range in input {
        validate_range(range, page_size)?;
        for &memory in memories.as_slice() {
            let start = range.start.max(memory.start);
            let end = range.end.min(memory.end);
            if start < end {
                output.push(Range { start, end })?;
            }
        }
    }
    output.normalize();
    Ok(())
}

fn validate_range(range: Range, page_size: usize) -> Result<(), PlanError> {
    if range.start >= range.end || range.start % page_size != 0 || range.end % page_size != 0 {
        return Err(PlanError::InvalidRange);
    }
    Ok(())
}

fn copy_into<const N: usize>(
    source: &RangeBuffer<N>,
    target: &mut RangeBuffer<N>,
) -> Result<(), PlanError> {
    target.clear();
    append(target, source)
}

fn append<const N: usize>(
    target: &mut RangeBuffer<N>,
    source: &RangeBuffer<N>,
) -> Result<(), PlanError> {
    for &range in source.as_slice() {
        target.push(range)?;
    }
    Ok(())
}

fn subtract_into<const N: usize>(
    source: &RangeBuffer<N>,
    reserved: &RangeBuffer<N>,
    output: &mut RangeBuffer<N>,
) -> Result<(), PlanError> {
    output.clear();
    for &range in source.as_slice() {
        let mut cursor = range.start;
        for &cut in reserved.as_slice() {
            if cut.end <= cursor || cut.start >= range.end {
                continue;
            }
            let cut_start = cut.start.max(cursor);
            if cursor < cut_start {
                output.push(Range {
                    start: cursor,
                    end: cut_start,
                })?;
            }
            cursor = cut.end.min(range.end);
            if cursor == range.end {
                break;
            }
        }
        if cursor < range.end {
            output.push(Range {
                start: cursor,
                end: range.end,
            })?;
        }
    }
    Ok(())
}

fn place_one<const N: usize>(
    memories: &RangeBuffer<N>,
    unavailable: &RangeBuffer<N>,
    len: usize,
    alignment: usize,
) -> Result<Option<Range>, PlanError> {
    for &memory in memories.as_slice() {
        let mut cursor = memory.start;
        for &cut in unavailable.as_slice() {
            if cut.end <= cursor || cut.start >= memory.end {
                continue;
            }
            if let Some(range) = fit(cursor, cut.start.min(memory.end), len, alignment)? {
                return Ok(Some(range));
            }
            cursor = cut.end.min(memory.end);
            if cursor == memory.end {
                break;
            }
        }
        if let Some(range) = fit(cursor, memory.end, len, alignment)? {
            return Ok(Some(range));
        }
    }
    Ok(None)
}

fn fit(start: usize, end: usize, len: usize, alignment: usize) -> Result<Option<Range>, PlanError> {
    if start >= end {
        return Ok(None);
    }
    let aligned = align_up(start, alignment)?;
    let Some(candidate_end) = aligned.checked_add(len) else {
        return Err(PlanError::ArithmeticOverflow);
    };
    Ok((candidate_end <= end).then_some(Range {
        start: aligned,
        end: candidate_end,
    }))
}

fn align_up(value: usize, alignment: usize) -> Result<usize, PlanError> {
    value
        .checked_add(alignment - 1)
        .map(|end| end & !(alignment - 1))
        .ok_or(PlanError::ArithmeticOverflow)
}
