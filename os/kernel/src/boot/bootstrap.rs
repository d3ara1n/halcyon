//! 初始进程的有界装载记录；与用户 Builder 共用 VM 修改、线程出生和启动责任。

use crate::{
    frame,
    maintenance::{self as mt, WakeToken, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::{
        self,
        process::{self, builder::ProcessBuilder, shell::ProcessShell},
        resources::{MetadataCharge, MetadataFunding},
        wait_source::{Hint, HintTarget, ObservationSource, WaitNode},
    },
    vm::operation,
};
use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{alloc::Layout, pin::Pin};
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    proc::{
        PROCESS_MAIN_STACK_SIZE as STACK, PROCESS_PAGE_SIZE as PAGE, PROCESS_USER_TOP as TOP,
        ThreadStartContext,
    },
    startup,
};
use funded_storage::Funding;

use maintenance::record::Outcome;
use memory_space::Protection;

// 出生准备/写入的每次页数跟 VM 有界发布预算一致，不拿公开最大请求量作原子量。
const CHUNK: usize = crate::vm::space::STEP_UNITS * PAGE;
#[derive(Clone, Copy)]
enum Phase {
    MapImage(usize, usize),
    WriteImage(usize, usize),
    Stack(usize),
    Block,
    WriteBlock(usize),
    Payload(usize),
    Thread,
    Start,
    Done,
}
struct Loader {
    /// Start 移交后只留完成来源，不能用宿主强根阻止其 DONE footer。
    builder: Option<Arc<ProcessBuilder>>,
    image: elf::Elf,
    bytes: &'static [u8],
    // ELF 借用只在该 owner 存续期间使用；尾部归还后不再解引用 bytes。
    prefix: Option<frame::BootHeldExtent>,
    payload: Option<frame::BootHeldExtent>,
    payload_len: usize,
    block: Vec<u8>,
    _block_metadata: MetadataCharge,
    phase: Phase,
    waiting: Option<Arc<dyn ObservationSource>>,
    attached: bool,
    epoch: u64,
}
struct Bootstrap {
    record: WorkRecord,
    this: Spinlock<Weak<Self>>,
    shell: Arc<ProcessShell>,
    control: Arc<task::thread::ThreadControl>,
    _pool: Arc<task::memory_pool::MemoryPool>,
    node: WaitNode,
    wake: Spinlock<Option<(u64, WakeToken)>>,
    loader: Spinlock<Option<Loader>>,
    _metadata: MetadataCharge,
}

pub(crate) fn publish(
    image: elf::Elf,
    bytes: &'static [u8],
    prefix: frame::BootHeldExtent,
    payload: Option<frame::BootHeldExtent>,
    payload_len: usize,
    pool: Arc<task::memory_pool::MemoryPool>,
) -> Result<(), SystemCallError> {
    let job = task::job::Job::root(&pool);
    let shell = process::construct(&job, &pool, 0)?;
    assert_eq!(shell.pid, 1, "initial process must receive PID 1");
    let result = (|| {
        process::bind_initial_execution(&shell, image.requirement())?;
        let builder = ProcessBuilder::try_new(shell.clone())?;
        let control = task::thread::ThreadControl::try_new(shell.header.koid(), pool.clone())?;
        let body = shell
            .lease_building()
            .ok_or(SystemCallError::ObjectClosed)?;
        let metadata = MetadataCharge::arc::<Bootstrap>(&pool)?;
        // prefix 的实际容量就是编码器显式接收的 payload_off，初始进程取一页。
        let block_metadata = MetadataFunding(&pool)
            .charge(Layout::array::<u8>(PAGE).map_err(|_| SystemCallError::ReachLimit)?)?;
        let caps_metadata = MetadataFunding(&pool).charge(
            Layout::array::<task::cap::Cap>(startup::initial::HANDLE_COUNT)
                .map_err(|_| SystemCallError::ReachLimit)?,
        )?;
        let mut caps = Vec::new();
        caps.try_reserve_exact(startup::initial::HANDLE_COUNT)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let reset = task::system_reset::SystemReset::try_new(&pool)?;
        let roots = [
            (task::job::Job::object_ref(&job), task::job::JOB_RIGHTS),
            (
                task::system_reset::SystemReset::object_ref(&reset),
                Rights::MANAGE | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
            ),
            (
                ProcessShell::object_ref(&shell),
                process::shell::CONTROL_RIGHTS,
            ),
            (
                task::memory_pool::MemoryPool::object_ref(&pool),
                Rights::CREATE | Rights::READ | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
            ),
            (
                task::thread::ThreadControl::object_ref(&control),
                task::thread::CONTROL_RIGHTS,
            ),
        ];
        for (object, rights) in roots {
            match task::handle::mint(object, rights) {
                Ok(cap) => caps.push(cap),
                Err(error) => {
                    task::handle::release_all(caps);
                    return Err(error);
                }
            }
        }
        let reserved = body
            .handles
            .lock()
            .reserve(caps.len(), &task::resources::MetadataFunding(shell.pool()));
        let reserved = match reserved {
            Ok(reserved) => reserved,
            Err(error) => {
                task::handle::release_all(caps);
                return Err(task::handle::map_error(error));
            }
        };
        let block = startup::build_startup_prefix_with_entry(
            shell.pid,
            0,
            reserved.handles(),
            PAGE,
            payload_len,
            startup::EntryThreadDescriptor {
                kind: startup::ENTRY_THREAD_SECTION_KIND,
                version: startup::ENTRY_THREAD_SECTION_VERSION,
                section_len: core::mem::size_of::<startup::EntryThreadDescriptor>() as u32,
                control_index: startup::initial::ENTRY_THREAD_CONTROL as u32,
                reserved: 0,
                stack_reservation_start: (TOP - STACK) as u64,
                stack_reservation_len: STACK as u64,
                usable_start: (TOP - STACK) as u64,
                usable_len: STACK as u64,
            },
        );
        let block = match block {
            Ok(block) => block,
            Err(error) => {
                body.handles.lock().rollback(reserved);
                task::handle::release_all(caps);
                return Err(match error {
                    startup::StartupBuildError::AllocationFailed => SystemCallError::OutOfMemory,
                    startup::StartupBuildError::Overflow => SystemCallError::IllegalArgument,
                });
            }
        };
        let end = (image.image_end() as usize)
            .checked_add(block.len())
            .and_then(|n| n.checked_add(payload_len.div_ceil(PAGE) * PAGE));
        if end.is_none_or(|end| end > TOP - STACK) {
            body.handles.lock().rollback(reserved);
            task::handle::release_all(caps);
            return Err(SystemCallError::AddressConflict);
        }
        let committed = body.handles.lock().commit(reserved, caps);
        match committed {
            Ok(capability::Commit::Installed(handles)) => {
                assert_eq!(handles.len(), startup::initial::HANDLE_COUNT)
            }
            Ok(capability::Commit::Released { events, .. }) => {
                for event in events {
                    task::cap::deliver(Some(event));
                }
                return Err(SystemCallError::ObjectClosed);
            }
            Err((caps, _)) => {
                task::handle::release_all(caps);
                unreachable!("bootstrap reservation changed");
            }
        }
        // commit 已消费 Cap Vec 的实际分配，发布 Work 前即交回其费用。
        drop(caps_metadata);
        drop(body);
        builder.arm();
        let work = Arc::try_new(Bootstrap {
            record: WorkRecord::new(pool.maintenance_account().clone()),
            this: Spinlock::new(ranks::LEAF, Weak::new()),
            shell: shell.clone(),
            control,
            _pool: pool,
            node: WaitNode::new(),
            wake: Spinlock::new(ranks::LEAF, None),
            loader: Spinlock::new(
                ranks::LEAF,
                Some(Loader {
                    builder: Some(builder),
                    image,
                    bytes,
                    prefix: Some(prefix),
                    payload,
                    payload_len,
                    block,
                    _block_metadata: block_metadata,
                    phase: Phase::MapImage(0, 0),
                    waiting: None,
                    attached: false,
                    epoch: 0,
                }),
            ),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        *work.this.lock() = Arc::downgrade(&work);
        mt::publish(work);
        mt::kick();
        Ok(())
    })();
    if result.is_err() {
        shell.freeze_and_seal(
            erhino_shared::proc::TerminationCause::Aborted,
            erhino_shared::proc::TerminationDetail::NONE,
        );
    }
    result
}

impl Bootstrap {
    fn advance(&self, loader: &mut Loader) -> Result<Outcome, SystemCallError> {
        if let Some(source) = loader.waiting.as_ref() {
            let root = self
                .this
                .lock()
                .upgrade()
                .expect("bootstrap lost work owner");
            let work: Arc<dyn Work> = root.clone();
            let token = mt::arm(&work);
            *self.wake.lock() = Some((loader.epoch, token));
            if !loader.attached {
                let target: Arc<dyn HintTarget> = root;
                // SAFETY: node 位于稳定 Arc，来源由 loader 强持；唯一 Work owner 负责 detach。
                unsafe {
                    source.attach(
                        Pin::new_unchecked(&self.node),
                        ObjectSignals::DONE,
                        Hint::new(Arc::downgrade(&target), loader.epoch),
                    );
                }
                loader.attached = true;
            }
            if !source.snapshot().signals.contains(ObjectSignals::DONE) {
                return Ok(Outcome::Blocked);
            }
            // SAFETY: 同一 Work owner 摘除本轮唯一已挂节点，来源强根尚未释放。
            unsafe {
                source.detach(Pin::new_unchecked(&self.node));
            }
            loader.attached = false;
            self.wake.lock().take();
            loader.waiting = None;
            return Ok(Outcome::Ready);
        }
        let body = self
            .shell
            .lease_building()
            .ok_or(SystemCallError::ObjectClosed)?;
        let wait = match loader.phase {
            Phase::MapImage(index, offset) => {
                if let Some(run) = loader.image.runs().get(index) {
                    let size = (run.memsz as usize - offset).min(CHUNK);
                    let protection = if run.executable {
                        Protection::ReadExecute
                    } else if run.writable {
                        Protection::ReadWrite
                    } else {
                        Protection::ReadOnly
                    };
                    let wait = operation::initial_map(
                        &body,
                        run.vaddr as usize + offset,
                        size,
                        protection,
                        None,
                    )?;
                    loader.phase = if offset + size == run.memsz as usize {
                        Phase::MapImage(index + 1, 0)
                    } else {
                        Phase::MapImage(index, offset + size)
                    };
                    Some(wait)
                } else {
                    loader.phase = Phase::WriteImage(0, 0);
                    None
                }
            }
            Phase::WriteImage(index, offset) => {
                if let Some(segment) = loader.image.segments().get(index) {
                    let size = (segment.filesz as usize - offset)
                        .min(CHUNK)
                        .min(crate::uaccess::MAX_USER_ACCESS);
                    let start = segment.offset as usize + offset;
                    operation::initial_write(
                        &body,
                        segment.vaddr as usize + offset,
                        &loader.bytes[start..start + size],
                    )?;
                    loader.phase = if offset + size == segment.filesz as usize {
                        Phase::WriteImage(index + 1, 0)
                    } else {
                        Phase::WriteImage(index, offset + size)
                    };
                } else {
                    loader.phase = Phase::Stack(0);
                }
                None
            }
            Phase::Stack(offset) => {
                let size = (STACK - offset).min(CHUNK);
                let wait = operation::initial_map(
                    &body,
                    TOP - STACK + offset,
                    size,
                    Protection::ReadWrite,
                    None,
                )?;
                loader.phase = if offset + size == STACK {
                    Phase::Block
                } else {
                    Phase::Stack(offset + size)
                };
                Some(wait)
            }
            Phase::Block => {
                let wait = operation::initial_map(
                    &body,
                    loader.image.image_end() as usize,
                    loader.block.len(),
                    Protection::ReadOnly,
                    None,
                )?;
                loader.phase = Phase::WriteBlock(0);
                Some(wait)
            }
            Phase::WriteBlock(offset) => {
                let size = (loader.block.len() - offset)
                    .min(CHUNK)
                    .min(crate::uaccess::MAX_USER_ACCESS);
                operation::initial_write(
                    &body,
                    loader.image.image_end() as usize + offset,
                    &loader.block[offset..offset + size],
                )?;
                loader.phase = if offset + size == loader.block.len() {
                    Phase::Payload(0)
                } else {
                    Phase::WriteBlock(offset + size)
                };
                None
            }
            Phase::Payload(offset) => {
                if let Some(extent) = loader.payload.take() {
                    let extent = if extent.pages() > CHUNK / PAGE {
                        let (chunk, rest) = extent.split_at(CHUNK / PAGE);
                        loader.payload = Some(rest);
                        chunk
                    } else {
                        extent
                    };
                    let bytes = extent.pages() * PAGE;
                    loader.phase = Phase::Payload(offset + bytes);
                    let backing =
                        crate::vm::backing::Backing::from_boot(self._pool.clone(), extent)?;
                    Some(operation::initial_map(
                        &body,
                        loader.image.image_end() as usize + loader.block.len() + offset,
                        bytes,
                        Protection::ReadOnly,
                        Some(backing),
                    )?)
                } else {
                    loader.phase = Phase::Thread;
                    None
                }
            }
            Phase::Thread => {
                let reservation = task::job::Job::reserve_thread(&self.shell, &body)?;
                let context = ThreadStartContext {
                    entry: loader.image.entry(),
                    stack_pointer: TOP as u64,
                    arg1: loader.image.image_end(),
                    arg2: (loader.block.len() + loader.payload_len) as u64,
                };
                body.vm
                    .space
                    .lock()
                    .space
                    .as_mut()
                    .ok_or(SystemCallError::ObjectClosed)?
                    .validate_initial_context(context.entry as usize, TOP)?;
                let domain = body
                    .execution
                    .lock()
                    .ok_or(SystemCallError::ObjectNotAvailable)?;
                let ready = domain.reserve_ready(1)?;
                let thread = task::Thread::try_new(
                    reservation.key(),
                    &self.shell,
                    context,
                    domain,
                    body.vm.satp(),
                    loader.image.requirement() == elf::IsaRequirement::D64,
                    Some(&self.control),
                    ready,
                )?;
                reservation.commit(process::members::MemberState::Created(thread));
                loader.phase = Phase::Start;
                None
            }
            Phase::Start => {
                drop(body);
                // 原始 ELF 已复制，prefix 页与残余借用在首次执行前一并结束。
                loader.bytes = &[];
                drop(loader.prefix.take());
                loader.phase = Phase::Done;
                let builder = loader
                    .builder
                    .take()
                    .expect("bootstrap lost pending Builder");
                let wait = builder.start();
                // Loader 已从私有锁取出；已提交根由 Builder 队列稳定持有。
                drop(builder);
                Some(wait?)
            }
            Phase::Done => return Ok(Outcome::Done),
        };
        if let Some(wait) = wait {
            loader.epoch = loader
                .epoch
                .checked_add(1)
                .expect("bootstrap wait identity exhausted");
            loader.waiting = Some(wait.into_completion());
        }
        Ok(Outcome::Ready)
    }
}
impl Work for Bootstrap {
    fn cost_name(&self) -> &'static &'static str {
        let loader = self.loader.lock();
        let loader = loader
            .as_ref()
            .expect("bootstrap cost sampled outside step");
        if loader.waiting.is_some() {
            return &"bootstrap/wait";
        }
        match loader.phase {
            Phase::MapImage(..) => &"bootstrap/map-image",
            Phase::WriteImage(..) => &"bootstrap/write-image",
            Phase::Stack(..) => &"bootstrap/map-stack",
            Phase::Block => &"bootstrap/map-block",
            Phase::WriteBlock(..) => &"bootstrap/write-block",
            Phase::Payload(..) => &"bootstrap/map-payload",
            Phase::Thread => &"bootstrap/create-thread",
            Phase::Start => &"bootstrap/start",
            Phase::Done => &"bootstrap/done",
        }
    }
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(&self) -> Outcome {
        let mut loader = self.loader.lock().take().expect("bootstrap stepped twice");
        // Start 提交后不再借 Building Body；DONE 只等同一启动记录的完成。
        let outcome = if matches!(loader.phase, Phase::Done) && loader.waiting.is_none() {
            Outcome::Done
        } else {
            match self.advance(&mut loader) {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.shell.freeze_and_seal(
                        erhino_shared::proc::TerminationCause::Aborted,
                        erhino_shared::proc::TerminationDetail::NONE,
                    );
                    log!(Boot, "initial process construction failed: {:?}", error);
                    drop(loader);
                    // 调度环境已经 Ready，不能回退启动 gate；走既有全 hart 停驻。
                    crate::runtime_stop::request();
                    crate::runtime_stop::check();
                    unreachable!("runtime stop returned after initial construction failure");
                }
            }
        };
        if outcome != Outcome::Done {
            *self.loader.lock() = Some(loader);
        } else {
            log!(
                Boot,
                "initial process pid {} started (payload {} bytes)",
                self.shell.pid,
                loader.payload_len
            );
        }
        outcome
    }
}
impl HintTarget for Bootstrap {
    fn hint(&self, identity: u64) {
        let token = {
            let mut wake = self.wake.lock();
            if wake.as_ref().is_some_and(|(epoch, _)| *epoch == identity) {
                wake.take().map(|(_, token)| token)
            } else {
                None
            }
        };
        if let Some(token) = token {
            mt::wake(token);
            mt::kick();
        }
    }
}
