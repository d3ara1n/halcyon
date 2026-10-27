#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    mem::MemoryProtection,
    object::{Handle, ObjectSignals},
    proc::{PROCESS_PAGE_SIZE, ThreadCreateResult, ThreadStartContext, Tid},
    startup::EntryThreadDescriptor,
};
use libkernel::{
    ThreadControl, ThreadStartTicket,
    mm::{MappedRegion, Placement},
    thread as kernel_thread,
};
use lock_api::Mutex;

type Lock = erhino_shared::sync::spin::SimpleLock;

const EMPTY: u8 = 0;
const PUBLISHED: u8 = 1;
const RECEIVER_GONE: u8 = 2;
const DEFAULT_STACK_BYTES: usize = 1024 * 1024;

/// One-shot result storage shared by the producer and its observer.
pub struct ResultCell<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

unsafe impl<T: Send> Send for ResultCell<T> {}
unsafe impl<T: Send> Sync for ResultCell<T> {}

pub struct ResultProducer<T> {
    cell: Arc<ResultCell<T>>,
}

unsafe impl<T: Send> Send for ResultProducer<T> {}

impl<T> ResultCell<T> {
    pub fn new() -> (Arc<Self>, ResultProducer<T>) {
        let cell = Arc::new(Self {
            state: AtomicU8::new(EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        });
        let producer = ResultProducer {
            cell: Arc::clone(&cell),
        };
        (cell, producer)
    }
}

impl<T> ResultProducer<T> {
    /// Publish at most once. A receiver that was dropped wins the race and
    /// makes the producer responsible for dropping `value`.
    pub fn publish(self, value: T) -> Result<(), T> {
        if self.cell.state.load(Ordering::Acquire) != EMPTY {
            return Err(value);
        }
        // SAFETY: only this producer writes the value while the state is
        // Empty. A receiver that wins `abandon` never reads the storage.
        unsafe { (*self.cell.value.get()).write(value) };
        if self
            .cell
            .state
            .compare_exchange(EMPTY, PUBLISHED, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            Ok(())
        } else {
            // SAFETY: ReceiverGone is only written by the receiver and never
            // reads the value. The producer owns the failed publication.
            Err(unsafe { (*self.cell.value.get()).assume_init_read() })
        }
    }
}

impl<T> ResultCell<T> {
    pub fn take(&self) -> Option<T> {
        if self
            .state
            .compare_exchange(
                PUBLISHED,
                RECEIVER_GONE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            // SAFETY: Published is established with release ordering.
            Some(unsafe { (*self.value.get()).assume_init_read() })
        } else {
            None
        }
    }

    pub fn abandon(&self) {
        loop {
            let state = self.state.load(Ordering::Acquire);
            if state == RECEIVER_GONE {
                return;
            }
            if self
                .state
                .compare_exchange(state, RECEIVER_GONE, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if state == PUBLISHED {
                    // SAFETY: this receiver won ownership of the published value.
                    unsafe { (*self.value.get()).assume_init_drop() };
                }
                return;
            }
        }
    }
}

impl<T> Drop for ResultCell<T> {
    fn drop(&mut self) {
        if self.state.load(Ordering::Acquire) == PUBLISHED {
            // SAFETY: Drop has exclusive access to the cell.
            unsafe { self.value.get_mut().assume_init_drop() };
        }
    }
}

/// Runtime-specific state that can be prepared before a thread is created.
pub trait ThreadEnvironment: Send + Sync + 'static {
    type Child: Send + 'static;
    type Residue: Send + 'static;
    type EnterError: Send + 'static;
    type LeaveError: Send + 'static;
    type RetireError: Send + 'static;

    fn prepare(&self) -> Result<Self::Child, Self::EnterError>;
    fn enter(&self, child: Self::Child) -> Result<Self::Residue, Self::EnterError>;
    fn leave(&self, residue: &mut Self::Residue) -> Result<(), Self::LeaveError>;
    fn retire(&self, residue: Self::Residue) -> Result<(), Self::RetireError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoEnvironment;

impl ThreadEnvironment for NoEnvironment {
    type Child = ();
    type Residue = ();
    type EnterError = ();
    type LeaveError = ();
    type RetireError = ();

    fn prepare(&self) -> Result<Self::Child, Self::EnterError> {
        Ok(())
    }

    fn enter(&self, _: Self::Child) -> Result<Self::Residue, Self::EnterError> {
        Ok(())
    }

    fn leave(&self, _: &mut Self::Residue) -> Result<(), Self::LeaveError> {
        Ok(())
    }

    fn retire(&self, _: Self::Residue) -> Result<(), Self::RetireError> {
        Ok(())
    }
}

pub enum ThreadOutcome<T, E: ThreadEnvironment> {
    Returned {
        value: T,
        leave: Result<(), E::LeaveError>,
    },
    EnterFailed(E::EnterError),
    Incomplete,
}

/// Resources that outlive a thread's execution. A record without a Control
/// is already DONE: either its thread departed and was observed, or it was
/// never started and its last Control was closed before installation.
struct ThreadRecord<E: ThreadEnvironment> {
    stack: Option<MappedRegion>,
    control: Option<Arc<ThreadControl>>,
    environment: Arc<E>,
    residue: Arc<ResidueCell<E::Residue>>,
}

enum SlotState<E: ThreadEnvironment> {
    Free,
    /// Admission held by a PreparedThread; not visible to reap.
    Reserved,
    Live(ThreadRecord<E>),
    /// Exclusively owned by one reap call until it publishes the outcome.
    Claimed,
    /// Unrecoverable cleanup evidence; never reused. The record is held only
    /// to keep its owners alive until the integrity abort that always follows.
    Faulted {
        _retained: ThreadRecord<E>,
    },
}

struct RegistryInner<E: ThreadEnvironment> {
    slots: Vec<SlotState<E>>,
    sealed: bool,
}

/// Fixed-capacity cooperative owner for thread records.
pub struct ThreadRegistry<E: ThreadEnvironment> {
    inner: Mutex<Lock, RegistryInner<E>>,
    cursor: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    Sealed,
    Full,
    InvalidEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapProgress {
    Idle,
    Progress { completed: usize },
    Wait,
    Failed(CleanupFault),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupFault {
    Wait(SystemCallError),
    Retire,
    MissingStack,
    Unmap(SystemCallError),
    InvalidSlot,
}

impl<E: ThreadEnvironment> ThreadRegistry<E> {
    pub fn new(capacity: usize) -> Result<Self, SystemCallError> {
        if capacity == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        slots.resize_with(capacity, || SlotState::Free);
        Ok(Self {
            inner: Mutex::new(RegistryInner {
                slots,
                sealed: false,
            }),
            cursor: AtomicUsize::new(0),
        })
    }

    pub fn capacity(&self) -> usize {
        self.inner.lock().slots.len()
    }

    /// Stop new admissions. Reservations already granted may still install.
    pub fn seal(&self) {
        self.inner.lock().sealed = true;
    }

    fn reserve(&self) -> Result<usize, RegistryError> {
        let mut inner = self.inner.lock();
        if inner.sealed {
            return Err(RegistryError::Sealed);
        }
        let Some((index, slot)) = inner
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| matches!(slot, SlotState::Free))
        else {
            return Err(RegistryError::Full);
        };
        *slot = SlotState::Reserved;
        Ok(index)
    }

    /// Return an admission that never acquired a stack.
    fn release_reserved(&self, index: usize) {
        self.replace_exclusive(index, SlotState::Free);
    }

    /// Turn a reservation into a record. This is the only way a record enters
    /// the registry: started, entry, abandoned and failed threads all land here
    /// and share one cleanup path.
    fn install(&self, index: usize, record: ThreadRecord<E>) {
        self.replace_exclusive(index, SlotState::Live(record));
    }

    /// Replace a slot the caller owns exclusively (Reserved or Claimed).
    fn replace_exclusive(&self, index: usize, state: SlotState<E>) {
        let mut inner = self.inner.lock();
        match inner.slots.get_mut(index) {
            Some(slot @ (SlotState::Reserved | SlotState::Claimed)) => *slot = state,
            _ => cleanup_integrity_failure(CleanupFault::InvalidSlot),
        }
    }

    fn claim_next(&self) -> Option<(usize, ThreadRecord<E>)> {
        let mut inner = self.inner.lock();
        let len = inner.slots.len();
        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % len;
        for offset in 0..len {
            let index = (start + offset) % len;
            let slot = &mut inner.slots[index];
            if matches!(slot, SlotState::Live(_)) {
                let SlotState::Live(record) = core::mem::replace(slot, SlotState::Claimed) else {
                    unreachable!("slot matched Live under the same guard");
                };
                return Some((index, record));
            }
        }
        None
    }

    /// 直接读取仍持有准备/执行/退休责任的原槽；不维护镜像计数。
    pub fn retained_records(&self) -> usize {
        self.inner
            .lock()
            .slots
            .iter()
            .filter(|slot| !matches!(slot, SlotState::Free))
            .count()
    }

    /// Reap at most `budget` records. It never waits for a child or runs user
    /// code; undone threads and Busy mappings return to Live.
    pub fn reap(&self, budget: usize) -> ReapProgress {
        let mut completed = 0;
        let mut saw_wait = false;
        for _ in 0..budget {
            let Some((index, mut record)) = self.claim_next() else {
                break;
            };
            if let Some(control) = record.control.as_ref() {
                match observe_done(control, erhino_shared::time::Deadline::at(0)) {
                    Ok(true) => {}
                    Ok(false) => {
                        self.install_claimed(index, record);
                        saw_wait = true;
                        continue;
                    }
                    Err(error) => {
                        self.replace_exclusive(index, SlotState::Faulted { _retained: record });
                        return ReapProgress::Failed(CleanupFault::Wait(error));
                    }
                }
            }
            // residue 的唯一消费权即退休进度：取走后 Busy 重试不会再次 retire。
            if let Some(residue) = record.residue.take()
                && record.environment.retire(residue).is_err()
            {
                self.replace_exclusive(index, SlotState::Faulted { _retained: record });
                return ReapProgress::Failed(CleanupFault::Retire);
            }
            drop(record.control.take());
            let Some(stack) = record.stack.take() else {
                self.replace_exclusive(index, SlotState::Faulted { _retained: record });
                return ReapProgress::Failed(CleanupFault::MissingStack);
            };
            match stack.unmap() {
                Ok(()) => {
                    self.replace_exclusive(index, SlotState::Free);
                    completed += 1;
                }
                Err((stack, SystemCallError::ObjectBusy)) => {
                    record.stack = Some(stack);
                    self.install_claimed(index, record);
                    saw_wait = true;
                }
                Err((stack, error)) => {
                    record.stack = Some(stack);
                    self.replace_exclusive(index, SlotState::Faulted { _retained: record });
                    return ReapProgress::Failed(CleanupFault::Unmap(error));
                }
            }
        }
        if completed != 0 {
            ReapProgress::Progress { completed }
        } else if saw_wait {
            ReapProgress::Wait
        } else {
            ReapProgress::Idle
        }
    }

    fn install_claimed(&self, index: usize, record: ThreadRecord<E>) {
        self.replace_exclusive(index, SlotState::Live(record));
    }

    /// Adopt the already-created entry thread as an ordinary record.
    ///
    /// # Safety
    ///
    /// `control` must be the descriptor-selected unique ThreadControl owner,
    /// and the descriptor geometry must refer to the live entry mapping.
    pub unsafe fn adopt_entry(
        &self,
        desc: EntryThreadDescriptor,
        control: ThreadControl,
        environment: E,
        residue: E::Residue,
    ) -> Result<Thread, RegistryError> {
        let slot = self.reserve()?;
        let stack = match unsafe {
            MappedRegion::from_reservation(
                desc.stack_reservation_start as usize,
                desc.stack_reservation_len as usize,
                desc.usable_start as usize,
                desc.usable_len as usize,
            )
        } {
            Ok(stack) => stack,
            Err(_) => {
                self.release_reserved(slot);
                return Err(RegistryError::InvalidEntry);
            }
        };
        let thread = Thread {
            id: 0,
            control: Arc::new(control),
        };
        self.install(
            slot,
            ThreadRecord {
                stack: Some(stack),
                control: Some(Arc::clone(&thread.control)),
                environment: Arc::new(environment),
                residue: Arc::new(ResidueCell::new(residue)),
            },
        );
        Ok(thread)
    }
}

fn registry_error_to_system(error: RegistryError) -> SystemCallError {
    match error {
        RegistryError::Sealed => SystemCallError::ObjectClosed,
        RegistryError::Full => SystemCallError::ReachLimit,
        RegistryError::InvalidEntry => SystemCallError::InternalError,
    }
}

fn cleanup_integrity_failure(fault: CleanupFault) -> ! {
    libkernel::dbg::debug(format_args!(
        "libthread cleanup integrity failure: {:?}",
        fault
    ));
    // SAFETY: cleanup lost a committed owner or an irreversible progress
    // invariant; continuing would make the process resource state unsound.
    unsafe { libkernel::sys_process_abort() }
}

fn reap_or_abort<E: ThreadEnvironment>(registry: &'static ThreadRegistry<E>) -> ReapProgress {
    match registry.reap(registry.capacity()) {
        ReapProgress::Failed(fault) => cleanup_integrity_failure(fault),
        progress => progress,
    }
}

fn observe_done(
    control: &ThreadControl,
    deadline: erhino_shared::time::Deadline,
) -> Result<bool, SystemCallError> {
    let result =
        libkernel::ipc::wait::wait_until(control.as_handle(), ObjectSignals::DONE, deadline)?;
    Ok(result
        .observed
        .contains(erhino_shared::object::ObjectSignals::DONE))
}

#[derive(Clone)]
pub struct Thread {
    id: Tid,
    control: Arc<ThreadControl>,
}

impl Thread {
    pub fn id(&self) -> Tid {
        self.id
    }

    pub fn control(&self) -> &Arc<ThreadControl> {
        &self.control
    }

    pub fn wait(&self, deadline: erhino_shared::time::Deadline) -> Result<bool, SystemCallError> {
        observe_done(&self.control, deadline)
    }
}

struct ResidueCell<T> {
    value: Mutex<Lock, Option<T>>,
}

impl<T> ResidueCell<T> {
    fn new(value: T) -> Self {
        Self {
            value: Mutex::const_new(Lock::new(), Some(value)),
        }
    }

    fn empty() -> Self {
        Self {
            value: Mutex::const_new(Lock::new(), None),
        }
    }

    fn publish(&self, value: T) {
        *self.value.lock() = Some(value);
    }

    fn take(&self) -> Option<T> {
        self.value.lock().take()
    }
}

#[repr(C)]
struct Packet<E, F, T>
where
    E: ThreadEnvironment,
{
    entry: extern "C" fn(usize, usize) -> !,
    environment: Arc<E>,
    child: Option<E::Child>,
    function: Option<F>,
    producer: Option<ResultProducer<ThreadOutcome<T, E>>>,
    residue: Arc<ResidueCell<E::Residue>>,
}

extern "C" fn trampoline<E, F, T>(packet: usize, registry: usize) -> !
where
    E: ThreadEnvironment,
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // SAFETY: ThreadFactory only encodes a `'static` Registry reference in
    // arg2, and the packet is transferred exactly once after ThreadStart.
    let registry = unsafe { &*(registry as *const ThreadRegistry<E>) };
    let mut packet = unsafe { Box::from_raw(packet as *mut Packet<E, F, T>) };
    let child = packet
        .child
        .take()
        .expect("thread child environment consumed twice");
    let mut residue = match packet.environment.enter(child) {
        Ok(residue) => residue,
        Err(error) => {
            if let Some(function) = packet.function.take() {
                drop(function);
            }
            let outcome = ThreadOutcome::EnterFailed(error);
            if let Err(outcome) = packet
                .producer
                .take()
                .expect("thread result producer consumed twice")
                .publish(outcome)
            {
                drop(outcome);
            }
            drop(packet);
            reap_or_abort(registry);
            // SAFETY: the result is published before kernel departure.
            unsafe { libkernel::sys_thread_exit() }
        }
    };
    let function = packet
        .function
        .take()
        .expect("thread packet consumed twice");
    let result = function();
    let leave = packet.environment.leave(&mut residue);
    packet.residue.publish(residue);
    let outcome = ThreadOutcome::Returned {
        value: result,
        leave,
    };
    if let Err(outcome) = packet
        .producer
        .take()
        .expect("thread result producer consumed twice")
        .publish(outcome)
    {
        drop(outcome);
    }
    drop(packet);
    reap_or_abort(registry);
    // SAFETY: ResultCell has been resolved before the kernel departure.
    unsafe { libkernel::sys_thread_exit() }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
unsafe extern "C" fn __libthread_worker_entry(_: usize, _: usize) -> ! {
    core::arch::naked_asm!(
        ".option push",
        ".option norelax",
        "la gp, __global_pointer$",
        ".option pop",
        "ld t0, 0(a0)",
        "jr t0",
    )
}

#[cfg(not(target_arch = "riscv64"))]
extern "C" fn __libthread_worker_entry(_: usize, _: usize) -> ! {
    panic!("worker entry is only available on riscv64 targets")
}

pub struct ThreadFactory<E: ThreadEnvironment> {
    registry: &'static ThreadRegistry<E>,
    environment: Arc<E>,
    stack_bytes: usize,
}

impl<E: ThreadEnvironment> ThreadFactory<E> {
    pub fn new(registry: &'static ThreadRegistry<E>, environment: E) -> Self {
        Self {
            registry,
            environment: Arc::new(environment),
            stack_bytes: DEFAULT_STACK_BYTES,
        }
    }

    pub const fn stack_size(mut self, bytes: usize) -> Self {
        self.stack_bytes = bytes;
        self
    }

    pub fn spawn<F, T>(&self, function: F) -> Result<ResultReceiver<T, E>, SystemCallError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.prepare(function)?.start().map_err(|(_, error)| error)
    }

    pub fn prepare<F, T>(&self, function: F) -> Result<PreparedThread<F, T, E>, SystemCallError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // Dead threads are reclaimed before their capacity is needed: a full
        // registry after a full-budget reap holds only live or Busy records.
        reap_or_abort(self.registry);
        let slot = match self.registry.reserve() {
            Ok(slot) => slot,
            Err(RegistryError::Full) => match reap_or_abort(self.registry) {
                ReapProgress::Progress { .. } => {
                    self.registry.reserve().map_err(registry_error_to_system)?
                }
                _ => return Err(SystemCallError::ReachLimit),
            },
            Err(error) => return Err(registry_error_to_system(error)),
        };
        let child = match self.environment.prepare() {
            Ok(child) => child,
            Err(_) => {
                self.registry.release_reserved(slot);
                return Err(SystemCallError::OutOfMemory);
            }
        };
        let Some(stack_bytes) = self.stack_bytes.checked_add(PROCESS_PAGE_SIZE - 1) else {
            self.registry.release_reserved(slot);
            return Err(SystemCallError::IllegalArgument);
        };
        let stack_bytes = stack_bytes / PROCESS_PAGE_SIZE * PROCESS_PAGE_SIZE;
        let stack = loop {
            match MappedRegion::map_anonymous(
                stack_bytes,
                PROCESS_PAGE_SIZE,
                PROCESS_PAGE_SIZE,
                MemoryProtection::ReadWrite,
                Placement::Anywhere,
            ) {
                Ok(region) => break region,
                Err(SystemCallError::ObjectBusy) => {
                    // Another mapping publication owns the current page-table
                    // generation; yield before retrying the construction.
                    if let Err(error) = unsafe { libkernel::sys_sleep(1) } {
                        self.registry.release_reserved(slot);
                        return Err(error);
                    }
                }
                Err(error) => {
                    self.registry.release_reserved(slot);
                    return Err(error);
                }
            }
        };
        let residue = Arc::new(ResidueCell::empty());
        // From here on the stack exists; every failure installs it as a DONE
        // record (no Control) so the ordinary reap path owns the Unmap.
        let fail = |stack: MappedRegion, residue, error| {
            self.registry.install(
                slot,
                ThreadRecord {
                    stack: Some(stack),
                    control: None,
                    environment: Arc::clone(&self.environment),
                    residue,
                },
            );
            Err(error)
        };
        let Some(usable) = stack.usable() else {
            return fail(stack, residue, SystemCallError::InternalError);
        };
        let (result, producer) = ResultCell::<ThreadOutcome<T, E>>::new();
        let packet = Box::new(Packet {
            entry: trampoline::<E, F, T>,
            environment: Arc::clone(&self.environment),
            child: Some(child),
            function: Some(function),
            producer: Some(producer),
            residue: Arc::clone(&residue),
        });
        let context = ThreadStartContext {
            entry: __libthread_worker_entry as *const () as usize as u64,
            stack_pointer: usable.end as u64,
            arg1: (&*packet as *const Packet<E, F, T>) as usize as u64,
            arg2: self.registry as *const ThreadRegistry<E> as usize as u64,
        };
        let mut created = ThreadCreateResult {
            tid: 0,
            reserved: 0,
            control: Handle::INVALID,
            start: Handle::INVALID,
        };
        // SAFETY: all syscall pointers refer to live values; the stack and
        // packet were prepared before creation.
        if let Err(error) =
            unsafe { kernel_thread::create(Handle::INVALID, &context, &mut created) }
        {
            drop(packet);
            return fail(stack, residue, error);
        }
        // SAFETY: ThreadCreate 返回两份独立的新 capability owner。
        let control = Arc::new(unsafe { ThreadControl::from_raw(created.control) });
        let ticket = unsafe { ThreadStartTicket::from_raw(created.start) };
        Ok(PreparedThread {
            parts: Some(PreparedParts {
                result,
                id: created.tid,
                control,
                ticket,
                packet,
                stack,
                residue,
            }),
            environment: Arc::clone(&self.environment),
            registry: self.registry,
            slot,
        })
    }
}

struct PreparedParts<F, T, E: ThreadEnvironment> {
    result: Arc<ResultCell<ThreadOutcome<T, E>>>,
    id: Tid,
    control: Arc<ThreadControl>,
    ticket: ThreadStartTicket,
    packet: Box<Packet<E, F, T>>,
    stack: MappedRegion,
    residue: Arc<ResidueCell<E::Residue>>,
}

/// A created, not yet started thread. It owns a registry reservation and all
/// thread resources; nothing is visible to reap until start or drop installs
/// the record.
pub struct PreparedThread<F, T, E: ThreadEnvironment> {
    parts: Option<PreparedParts<F, T, E>>,
    environment: Arc<E>,
    registry: &'static ThreadRegistry<E>,
    slot: usize,
}

impl<F, T, E: ThreadEnvironment> PreparedThread<F, T, E> {
    #[allow(
        clippy::result_large_err,
        reason = "失败返还已预付线程资源 owner，错误路径不再分配"
    )]
    pub fn start(mut self) -> Result<ResultReceiver<T, E>, (Self, SystemCallError)>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let mut parts = self.parts.take().expect("PreparedThread parts missing");
        // The packet is handed to the child by address; ownership transfers
        // only if ThreadStart succeeds.
        let packet = Box::into_raw(parts.packet);
        if let Err(error) = parts.ticket.start() {
            // SAFETY: ThreadStart rejected the Created control, so the
            // trampoline never ran and the packet is still ours.
            let packet = unsafe { Box::from_raw(packet) };
            self.parts = Some(PreparedParts { packet, ..parts });
            return Err((self, error));
        }
        drop(parts.ticket);
        self.registry.install(
            self.slot,
            ThreadRecord {
                stack: Some(parts.stack),
                control: Some(Arc::clone(&parts.control)),
                environment: Arc::clone(&self.environment),
                residue: parts.residue,
            },
        );
        Ok(ResultReceiver {
            result: parts.result,
            control: parts.control,
            registry: self.registry,
            id: parts.id,
        })
    }
}

impl<F, T, E: ThreadEnvironment> Drop for PreparedThread<F, T, E> {
    fn drop(&mut self) {
        let Some(parts) = self.parts.take() else {
            return;
        };
        // F and the environment child are destroyed on the abandoning thread.
        drop(parts.packet);
        drop(parts.result);
        // 启动票据的最后持有放弃 Pending；Control 仅供观察。
        drop(parts.ticket);
        self.registry.install(
            self.slot,
            ThreadRecord {
                stack: Some(parts.stack),
                control: Some(parts.control),
                environment: Arc::clone(&self.environment),
                residue: parts.residue,
            },
        );
    }
}

pub struct ResultReceiver<T, E: ThreadEnvironment> {
    result: Arc<ResultCell<ThreadOutcome<T, E>>>,
    control: Arc<ThreadControl>,
    registry: &'static ThreadRegistry<E>,
    id: Tid,
}

unsafe impl<T: Send, E: ThreadEnvironment> Send for ResultReceiver<T, E> {}

impl<T, E: ThreadEnvironment> ResultReceiver<T, E> {
    pub fn id(&self) -> Tid {
        self.id
    }

    pub fn thread(&self) -> Thread {
        Thread {
            id: self.id,
            control: Arc::clone(&self.control),
        }
    }

    pub fn try_join(self) -> Result<ThreadOutcome<T, E>, Self> {
        let done = match observe_done(&self.control, erhino_shared::time::Deadline::at(0)) {
            Ok(done) => done,
            Err(error) => cleanup_integrity_failure(CleanupFault::Wait(error)),
        };
        if !done {
            return Err(self);
        }
        reap_or_abort(self.registry);
        Ok(self.result.take().unwrap_or(ThreadOutcome::Incomplete))
    }

    pub fn join(self) -> ThreadOutcome<T, E> {
        if let Err(error) = observe_done(&self.control, erhino_shared::time::Deadline::INFINITE) {
            cleanup_integrity_failure(CleanupFault::Wait(error));
        }
        reap_or_abort(self.registry);
        self.result.take().unwrap_or(ThreadOutcome::Incomplete)
    }
}

impl<T, E: ThreadEnvironment> Drop for ResultReceiver<T, E> {
    fn drop(&mut self) {
        self.result.abandon();
    }
}

pub fn yield_now() -> Result<(), SystemCallError> {
    libkernel::thread::yield_now()
}

#[cfg(test)]
mod tests {
    use super::{
        CleanupFault, NoEnvironment, ReapProgress, ResidueCell, ResultCell, SlotState,
        ThreadRecord, ThreadRegistry,
    };
    use alloc::sync::Arc;

    #[test]
    fn result_cell_publishes_once() {
        let (cell, producer) = ResultCell::new();
        assert!(producer.publish(7_u32).is_ok());
        assert_eq!(cell.take(), Some(7));
        assert_eq!(cell.take(), None);
    }

    #[test]
    fn result_cell_abandon_drops_published_value() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        struct DropProbe(Arc<AtomicUsize>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let (cell, producer) = ResultCell::new();
        assert!(producer.publish(DropProbe(Arc::clone(&drops))).is_ok());
        cell.abandon();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn result_cell_publish_races_receiver_abandon_without_double_drop() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Barrier;

        struct DropProbe(Arc<AtomicUsize>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let (cell, producer) = ResultCell::new();
        let drops = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let receiver_cell = Arc::clone(&cell);
        let receiver_barrier = Arc::clone(&barrier);
        let receiver = std::thread::spawn(move || {
            receiver_barrier.wait();
            receiver_cell.abandon();
        });
        barrier.wait();
        if let Err(value) = producer.publish(DropProbe(Arc::clone(&drops))) {
            drop(value);
        }
        receiver.join().unwrap();
        drop(cell.take());
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    /// Reserved admissions are invisible to reap; an installed DONE record
    /// (no Control) is claimed on the next scan wherever it sits.
    #[test]
    fn reap_ignores_reservations_and_finds_done_record_behind_them() {
        let registry = ThreadRegistry::<NoEnvironment>::new(2).unwrap();
        let reserved = registry.reserve().unwrap();
        let done = registry.reserve().unwrap();
        assert_eq!(registry.reap(2), ReapProgress::Idle);
        registry.install(
            done,
            ThreadRecord {
                stack: None,
                control: None,
                environment: Arc::new(NoEnvironment),
                residue: Arc::new(ResidueCell::empty()),
            },
        );
        // The host has no stack to unmap; MissingStack proves the record was
        // claimed and processed, and the fault is retained as evidence.
        assert_eq!(
            registry.reap(2),
            ReapProgress::Failed(CleanupFault::MissingStack)
        );
        let inner = registry.inner.lock();
        assert!(matches!(inner.slots[reserved], SlotState::Reserved));
        assert!(matches!(inner.slots[done], SlotState::Faulted { .. }));
    }
}
