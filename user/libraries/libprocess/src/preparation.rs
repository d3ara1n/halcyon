//! 同一ELF装配的可让出owner；每步一页复制或一个ABI有界出生操作，失败仍持Building进程。

use super::*;
use libkernel::ipc::capability::Capability;

#[derive(Clone, Copy)]
enum Phase {
    Validate,
    Create,
    MapImage { run: usize, offset: usize },
    WriteImage { segment: usize, offset: usize },
    Stack(usize),
    Grant,
    BirthMap(usize),
    Execution,
    Thread,
    ThreadGrant,
    Prefix,
    WritePrefix(usize),
    WritePayload(usize),
    Done,
}

/// 借用固定装配输入，独占验证游标及创建后的 Building 进程。
///
/// `advance` 成功后交出唯一 PreparedProcess；失败保留首错和未完成 owner。
/// 需要持续推进其他职责的宿主须显式 `cancel` 并接管返回的 Collector；
/// 直接丢弃会由内部 PreparedProcess 的 Drop 同步完成取消。
#[must_use = "process preparation must finish or hand off its cancellation"]
pub struct Preparation<'a> {
    request: SpawnRequest<'a>,
    validation: Option<elf::Validation<'a>>,
    image: Option<elf::Elf>,
    prepared: Option<PreparedProcess>,
    retired_grants: GrantOutcome,
    phase: Phase,
    failure: Option<SpawnError>,
    granted: [Handle; PROCESS_MAX_GRANTS + 1],
    thread: Option<Capability>,
    prefix: Vec<u8>,
    block_len: usize,
    block_va: usize,
}
impl<'a> Preparation<'a> {
    /// 校验输入几何及收束权限；此时尚未创建进程或消费 grants。
    pub fn new(request: SpawnRequest<'a>) -> Result<Self, SpawnError> {
        if !request
            .control_rights
            .contains(Rights::MANAGE | Rights::WAIT | Rights::READ)
        {
            return Err(SystemCallError::RightsDenied.into());
        }
        if request.grants.len() > PROCESS_MAX_GRANTS {
            return Err(SpawnError::TooManyGrants);
        }
        let block_len = core::mem::size_of::<StartupBlockHeader>()
            .checked_add((request.grants.len() + 1) * core::mem::size_of::<Handle>())
            .and_then(|len| len.checked_add(core::mem::size_of::<EntryThreadDescriptor>()))
            .and_then(|len| len.checked_add(request.payload.len()))
            .filter(|len| u32::try_from(*len).is_ok())
            .ok_or(SpawnError::InvalidImage)?;
        let validation = elf::Validation::new(
            request.image,
            elf::LoadLimits {
                page_size: PROCESS_PAGE_SIZE as u64,
                image_limit: (PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE) as u64,
            },
        )
        .map_err(SpawnError::Elf)?;
        Ok(Self {
            request,
            validation: Some(validation),
            image: None,
            prepared: None,
            retired_grants: GrantOutcome::Retained,
            phase: Phase::Validate,
            failure: None,
            granted: [Handle::INVALID; PROCESS_MAX_GRANTS + 1],
            thread: None,
            prefix: Vec::new(),
            block_len,
            block_va: 0,
        })
    }
    /// 原输入 grants 的消费事实；完成或取消后仍保留。
    pub fn grants(&self) -> GrantOutcome {
        self.prepared
            .as_ref()
            .map_or(self.retired_grants, PreparedProcess::grants)
    }
    /// 推进一步，Some 交出唯一完成结果，None 表示仍可推进。
    ///
    /// 首次错误冻结原状态，后续 advance 返回同一错误；调用者仍须取消。
    pub fn advance(&mut self) -> Result<Option<PreparedProcess>, SpawnError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.step();
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
    /// 关闭未发布 Builder 并移交实际 DONE/Verify/Close 责任。
    ///
    /// None 表示没有待收集进程；失败保留原 Building owner，允许再次取消。
    pub fn cancel(&mut self) -> Result<Option<Collector>, SystemCallError> {
        self.retired_grants = self.grants();
        self.thread.take();
        let collector = self
            .prepared
            .as_mut()
            .map(PreparedProcess::cancel_to_collector)
            .transpose()?
            .flatten();
        self.prepared.take();
        self.phase = Phase::Done;
        Ok(collector)
    }
    pub(super) fn cancel_blocking(&mut self) -> Option<SystemCallError> {
        self.retired_grants = self.grants();
        self.thread.take();
        self.phase = Phase::Done;
        self.prepared
            .take()
            .and_then(|prepared| prepared.cancel().err())
            .and_then(|failure| failure.cleanup_error)
    }
    fn builder(&self) -> Handle {
        self.prepared
            .as_ref()
            .and_then(|p| p.created.as_ref())
            .expect("Building owner remains present")
            .builder
    }
    fn image(&self) -> &elf::Elf {
        self.image.as_ref().expect("ELF admission completed")
    }
    fn step(&mut self) -> Result<Option<PreparedProcess>, SpawnError> {
        self.phase = match self.phase {
            Phase::Validate => {
                if let Some(image) = self
                    .validation
                    .as_mut()
                    .expect("ELF validation remains present")
                    .advance()
                    .map_err(SpawnError::Elf)?
                {
                    self.block_va =
                        usize::try_from(image.image_end()).map_err(|_| SpawnError::InvalidImage)?;
                    if self
                        .block_va
                        .checked_add(self.block_len)
                        .is_none_or(|end| end > PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE)
                    {
                        return Err(SpawnError::InvalidImage);
                    }
                    self.image = Some(image);
                    self.validation = None;
                    Phase::Create
                } else {
                    Phase::Validate
                }
            }
            Phase::Create => {
                let created = process::create(
                    self.request.job,
                    self.request.memory_pool,
                    self.request.control_rights,
                )?;
                self.prepared = Some(PreparedProcess {
                    created: Some(created),
                    grants: GrantOutcome::Retained,
                });
                Phase::MapImage { run: 0, offset: 0 }
            }
            Phase::MapImage { run, offset } => {
                if let Some(span) = self.image().runs().get(run) {
                    let mut flags = ProcessMapFlags::READ;
                    if span.writable {
                        flags = flags | ProcessMapFlags::WRITE;
                    }
                    if span.executable {
                        flags = flags | ProcessMapFlags::EXECUTE;
                    }
                    let start = span.vaddr as usize;
                    let bytes = span.memsz as usize;
                    let length = (bytes - offset).min(erhino_shared::proc::PROCESS_MAP_MAX_BYTES);
                    process::map(self.builder(), start + offset, length, flags)?;
                    if offset + length == bytes {
                        Phase::MapImage {
                            run: run + 1,
                            offset: 0,
                        }
                    } else {
                        Phase::MapImage {
                            run,
                            offset: offset + length,
                        }
                    }
                } else {
                    Phase::WriteImage {
                        segment: 0,
                        offset: 0,
                    }
                }
            }
            Phase::WriteImage { segment, offset } => {
                if let Some(span) = self.image().segments().get(segment) {
                    let bytes = span.filesz as usize;
                    if offset == bytes {
                        Phase::WriteImage {
                            segment: segment + 1,
                            offset: 0,
                        }
                    } else {
                        let target = span.vaddr as usize + offset;
                        let length =
                            (PROCESS_PAGE_SIZE - target % PROCESS_PAGE_SIZE).min(bytes - offset);
                        let source = span.offset as usize + offset;
                        process::write(
                            self.builder(),
                            target,
                            &self.request.image[source..source + length],
                        )?;
                        Phase::WriteImage {
                            segment,
                            offset: offset + length,
                        }
                    }
                } else {
                    Phase::Stack(0)
                }
            }
            Phase::Stack(offset) => {
                let length = (PROCESS_MAIN_STACK_SIZE - offset)
                    .min(erhino_shared::proc::PROCESS_MAP_MAX_BYTES);
                process::map(
                    self.builder(),
                    PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE + offset,
                    length,
                    ProcessMapFlags::READ | ProcessMapFlags::WRITE,
                )?;
                if offset + length == PROCESS_MAIN_STACK_SIZE {
                    Phase::Grant
                } else {
                    Phase::Stack(offset + length)
                }
            }
            Phase::Grant => {
                let len = self.request.grants.len();
                if len > 0 {
                    process::grant(
                        self.builder(),
                        self.request.grants,
                        &mut self.granted[..len],
                    )?;
                    self.prepared
                        .as_mut()
                        .expect("Building owner present")
                        .grants = GrantOutcome::Consumed;
                }
                Phase::BirthMap(0)
            }
            Phase::BirthMap(offset) => {
                let length = (self.block_len.next_multiple_of(PROCESS_PAGE_SIZE) - offset)
                    .min(erhino_shared::proc::PROCESS_MAP_MAX_BYTES);
                process::map(
                    self.builder(),
                    self.block_va + offset,
                    length,
                    ProcessMapFlags::READ,
                )?;
                if offset + length >= self.block_len {
                    Phase::Execution
                } else {
                    Phase::BirthMap(offset + length)
                }
            }
            Phase::Execution => {
                let profile = match self.image().requirement() {
                    elf::IsaRequirement::Base64 => ExecutionProfile::Base64,
                    elf::IsaRequirement::D64 => ExecutionProfile::D64,
                };
                process::bind_execution(self.builder(), profile as u32)?;
                Phase::Thread
            }
            Phase::Thread => {
                let descriptor = ThreadStartContext {
                    entry: self.image().entry(),
                    stack_pointer: PROCESS_USER_TOP as u64,
                    arg1: self.block_va as u64,
                    arg2: self.block_len as u64,
                };
                let mut created = erhino_shared::proc::ThreadCreateResult {
                    tid: 0,
                    reserved: 0,
                    control: Handle::INVALID,
                    start: Handle::INVALID,
                };
                process::thread_create(self.builder(), &descriptor, &mut created)?;
                assert!(
                    !created.start.is_valid(),
                    "Building thread returned a start ticket"
                );
                // SAFETY: ThreadCreate返回唯一新entry；Grant前由此字段持完整关闭责任。
                self.thread = Some(unsafe { Capability::from_raw(created.control) });
                Phase::ThreadGrant
            }
            Phase::ThreadGrant => {
                let index = self.request.grants.len();
                let control = self
                    .thread
                    .as_ref()
                    .expect("entry thread remains owned")
                    .as_handle();
                process::grant(
                    self.builder(),
                    &[HandleGrant {
                        handle: control,
                        rights: Rights::WAIT | Rights::DUPLICATE | Rights::GRANT | Rights::TRANSIT,
                    }],
                    &mut self.granted[index..=index],
                )?;
                self.thread
                    .take()
                    .expect("entry thread grant consumed")
                    .into_raw();
                Phase::Prefix
            }
            Phase::Prefix => {
                let len = self.request.grants.len();
                let entry = EntryThreadDescriptor {
                    kind: erhino_shared::startup::ENTRY_THREAD_SECTION_KIND,
                    version: erhino_shared::startup::ENTRY_THREAD_SECTION_VERSION,
                    section_len: core::mem::size_of::<EntryThreadDescriptor>() as u32,
                    control_index: len as u32,
                    reserved: 0,
                    stack_reservation_start: (PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE) as u64,
                    stack_reservation_len: PROCESS_MAIN_STACK_SIZE as u64,
                    usable_start: (PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE) as u64,
                    usable_len: PROCESS_MAIN_STACK_SIZE as u64,
                };
                // prefix只含ABI有界的handles和固定描述符，不复制任意长度payload。
                self.prefix = erhino_shared::startup::build_startup_prefix_with_entry(
                    self.prepared
                        .as_ref()
                        .expect("Building owner present")
                        .pid(),
                    self.request.parent.pid(),
                    &self.granted[..=len],
                    self.block_len - self.request.payload.len(),
                    self.request.payload.len(),
                    entry,
                )
                .map_err(|_| SpawnError::InvalidImage)?;
                Phase::WritePrefix(0)
            }
            Phase::WritePrefix(offset) => {
                let length = PROCESS_PAGE_SIZE.min(self.prefix.len() - offset);
                process::write(
                    self.builder(),
                    self.block_va + offset,
                    &self.prefix[offset..offset + length],
                )?;
                if offset + length == self.prefix.len() {
                    Phase::WritePayload(0)
                } else {
                    Phase::WritePrefix(offset + length)
                }
            }
            Phase::WritePayload(offset) => {
                if offset == self.request.payload.len() {
                    self.phase = Phase::Done;
                    self.retired_grants = self.grants();
                    return Ok(self.prepared.take());
                }
                let target = self.block_va + self.prefix.len() + offset;
                let length = (PROCESS_PAGE_SIZE - target % PROCESS_PAGE_SIZE)
                    .min(self.request.payload.len() - offset);
                process::write(
                    self.builder(),
                    target,
                    &self.request.payload[offset..offset + length],
                )?;
                Phase::WritePayload(offset + length)
            }
            Phase::Done => return Err(SystemCallError::ObjectClosed.into()),
        };
        Ok(None)
    }
}
