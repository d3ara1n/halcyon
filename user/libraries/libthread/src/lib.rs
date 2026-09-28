#![no_std]

extern crate alloc;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    mem::MemoryProtection,
    object::Handle,
    proc::{PROCESS_PAGE_SIZE, ThreadCreateResult, ThreadStartContext, Tid},
    startup::EntryThreadDescriptor,
};
use libkernel::{
    mm::{MappedRegion, Placement},
    thread as kernel_thread,
    ThreadControl,
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

impl<T> ResultCell<T> {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Publish at most once. A receiver that was dropped wins the race and
    /// makes the producer responsible for dropping `value`.
    pub fn publish(&self, value: T) -> Result<(), T> {
        if self.state.load(Ordering::Acquire) != EMPTY {
            return Err(value);
        }
        // SAFETY: only this producer writes the value while the state is
        // Empty. A receiver that wins `abandon` never reads the storage.
        unsafe { (*self.value.get()).write(value) };
        if self
            .state
            .compare_exchange(EMPTY, PUBLISHED, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            Ok(())
        } else {
            // SAFETY: ReceiverGone is only written by the receiver and never
            // reads the value. The producer owns the failed publication.
            Err(unsafe { (*self.value.get()).assume_init_read() })
        }
    }

    pub fn take(&self) -> Option<T> {
        if self
            .state
            .compare_exchange(PUBLISHED, RECEIVER_GONE, Ordering::AcqRel, Ordering::Acquire)
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

impl<T> Default for ResultCell<T> {
    fn default() -> Self {
        Self::new()
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

#[derive(Debug)]
struct CleanupProgress {
    retired: bool,
    abandoned: bool,
    done: bool,
}

struct ThreadRecord<E: ThreadEnvironment> {
    stack: Option<MappedRegion>,
    control: Option<Arc<ThreadControl>>,
    environment: Arc<E>,
    residue: Arc<ResidueCell<E::Residue>>,
    cleanup: CleanupProgress,
}

enum SlotState<E: ThreadEnvironment> {
    Free,
    Reserved,
    Live(ThreadRecord<E>),
    Claimed(Option<ThreadRecord<E>>),
    Faulted(ThreadRecord<E>),
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
    InvalidSlot,
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
            .find(|(_, slot)| match slot {
                SlotState::Free => true,
                SlotState::Faulted(record) => {
                    let _ = record.cleanup.retired;
                    false
                }
                _ => false,
            })
        else {
            return Err(RegistryError::Full);
        };
        *slot = SlotState::Reserved;
        Ok(index)
    }

    fn release_reserved(&self, index: usize) -> Result<(), RegistryError> {
        let mut inner = self.inner.lock();
        let slot = inner.slots.get_mut(index).ok_or(RegistryError::InvalidSlot)?;
        if !matches!(slot, SlotState::Reserved) {
            return Err(RegistryError::InvalidSlot);
        }
        *slot = SlotState::Free;
        Ok(())
    }

    fn register(
        &self,
        index: usize,
        record: ThreadRecord<E>,
    ) -> Result<(), (RegistryError, ThreadRecord<E>)> {
        let mut inner = self.inner.lock();
        if inner.sealed {
            return Err((RegistryError::Sealed, record));
        }
        let Some(slot) = inner.slots.get_mut(index) else {
            return Err((RegistryError::InvalidSlot, record));
        };
        if !matches!(slot, SlotState::Reserved) {
            return Err((RegistryError::InvalidSlot, record));
        }
        *slot = SlotState::Live(record);
        Ok(())
    }

    fn cancel_prepared(&self, index: usize) -> Result<(), RegistryError> {
        let control = {
            let mut inner = self.inner.lock();
            let slot = inner.slots.get_mut(index).ok_or(RegistryError::InvalidSlot)?;
            match slot {
                SlotState::Live(record) => {
                    record.cleanup.abandoned = true;
                    record.control.take()
                }
                _ => return Err(RegistryError::InvalidSlot),
            }
        };
        drop(control);
        Ok(())
    }

    /// Reap at most `budget` completed records. It never waits for a child or
    /// performs user code; Busy mappings remain in their slot.
    pub fn reap(&self, budget: usize) -> ReapProgress {
        let mut completed = 0;
        let mut saw_wait = false;
        for _ in 0..budget {
            let Some((index, record)) = ({
                let mut inner = self.inner.lock();
                let len = inner.slots.len();
                if len == 0 {
                    break;
                }
                let start = self.cursor.fetch_add(1, Ordering::Relaxed) % len;
                let mut found = None;
                for offset in 0..len {
                    let index = (start + offset) % len;
                    let slot = &mut inner.slots[index];
                    let claim = match core::mem::replace(slot, SlotState::Free) {
                        SlotState::Live(record) => Some(record),
                        SlotState::Claimed(Some(record)) => Some(record),
                        state => {
                            *slot = state;
                            None
                        }
                    };
                    if claim.is_some() {
                        *slot = SlotState::Claimed(None);
                        found = claim.map(|record| (index, record));
                        break;
                    }
                }
                found
            }) else {
                break;
            };
            let mut record = record;
            let done = match record.control.as_ref() {
                None if record.cleanup.abandoned || record.cleanup.done => true,
                None => false,
                Some(control) => match libkernel::ipc::wait::wait_until(
                    &[erhino_shared::wait::WaitItem::new(
                        control.as_handle(),
                        erhino_shared::object::ObjectSignals::DONE,
                        0,
                    )],
                    erhino_shared::time::Deadline::at(0),
                ) {
                    Ok(result) => result
                        .observed
                        .contains(erhino_shared::object::ObjectSignals::DONE),
                    Err(error) => {
                        self.restore_claimed(index, record);
                        return ReapProgress::Failed(CleanupFault::Wait(error));
                    }
                },
            };
            if !done {
                self.restore_claimed(index, record);
                saw_wait = true;
                continue;
            }
            record.cleanup.done = true;
            if !record.cleanup.retired {
                if let Some(residue) = record.residue.take()
                    && record.environment.retire(residue).is_err()
                {
                    self.set_faulted(index, record);
                    return ReapProgress::Failed(CleanupFault::Retire);
                }
                record.cleanup.retired = true;
            }
            drop(record.control.take());
            let Some(stack) = record.stack.take() else {
                self.set_faulted(index, record);
                return ReapProgress::Failed(CleanupFault::MissingStack);
            };
            match stack.unmap() {
                Ok(()) => {
                    self.finish_claimed(index);
                    completed += 1;
                }
                Err((stack, SystemCallError::ObjectBusy)) => {
                    record.stack = Some(stack);
                    self.restore_claimed(index, record);
                    saw_wait = true;
                }
                Err((stack, error)) => {
                    record.stack = Some(stack);
                    self.set_faulted(index, record);
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

    fn restore_claimed(&self, index: usize, record: ThreadRecord<E>) {
        let mut inner = self.inner.lock();
        if let Some(slot) = inner.slots.get_mut(index) {
            *slot = SlotState::Claimed(Some(record));
        }
    }

    fn set_faulted(&self, index: usize, record: ThreadRecord<E>) {
        let mut inner = self.inner.lock();
        if let Some(slot) = inner.slots.get_mut(index) {
            *slot = SlotState::Faulted(record);
        }
    }

    fn finish_claimed(&self, index: usize) {
        let mut inner = self.inner.lock();
        if let Some(slot) = inner.slots.get_mut(index) {
            *slot = SlotState::Free;
        }
    }

    /// Adopt the already-created entry thread as an ordinary record.
    ///
    /// `control` must be the unique child-local grant selected by the
    /// descriptor's index. The reservation is converted into a normal region
    /// owner, so later cleanup does not branch on thread origin.
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
        let stack = unsafe {
            MappedRegion::from_reservation(
                desc.stack_reservation_start as usize,
                desc.stack_reservation_len as usize,
                desc.usable_start as usize,
                desc.usable_len as usize,
            )
        }
        .map_err(|_| {
            let _ = self.release_reserved(slot);
            RegistryError::InvalidEntry
        })?;
        let thread = Thread {
            id: 0,
            control: Arc::new(control),
        };
        let record = ThreadRecord {
            stack: Some(stack),
            control: Some(Arc::clone(&thread.control)),
            environment: Arc::new(environment),
            residue: Arc::new(ResidueCell::new(residue)),
            cleanup: CleanupProgress {
                retired: false,
                abandoned: false,
                done: false,
            },
        };
        if let Err((error, _record)) = self.register(slot, record) {
            let _ = self.release_reserved(slot);
            return Err(error);
        }
        Ok(thread)
    }
}

fn registry_error_to_system(error: RegistryError) -> SystemCallError {
    match error {
        RegistryError::Sealed => SystemCallError::ObjectClosed,
        RegistryError::Full => SystemCallError::ReachLimit,
        RegistryError::InvalidEntry | RegistryError::InvalidSlot => SystemCallError::InternalError,
    }
}

fn cleanup_integrity_failure(fault: CleanupFault) -> ! {
    libkernel::dbg::debug(format_args!("libthread cleanup integrity failure: {:?}", fault));
    // SAFETY: cleanup lost a committed owner or an irreversible progress
    // invariant; continuing would make the process resource state unsound.
    unsafe { libkernel::sys_process_abort() }
}

fn reap_or_abort<E: ThreadEnvironment>(registry: &'static ThreadRegistry<E>) {
    if let ReapProgress::Failed(fault) = registry.reap(1) {
        cleanup_integrity_failure(fault);
    }
}

fn observe_done(
    control: &ThreadControl,
    deadline: erhino_shared::time::Deadline,
) -> Result<bool, SystemCallError> {
    let item = erhino_shared::wait::WaitItem::new(
        control.as_handle(),
        erhino_shared::object::ObjectSignals::DONE,
        0,
    );
    let result = libkernel::ipc::wait::wait_until(core::slice::from_ref(&item), deadline)?;
    Ok(result
        .observed
        .contains(erhino_shared::object::ObjectSignals::DONE))
}

fn unmap_or_abort(stack: MappedRegion) {
    if let Err((_, error)) = stack.unmap() {
        cleanup_integrity_failure(CleanupFault::Unmap(error));
    }
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

    pub fn wait(
        &self,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<bool, SystemCallError> {
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
    result: Arc<ResultCell<ThreadOutcome<T, E>>>,
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
    let child = packet.child.take().expect("thread child environment consumed twice");
    let mut residue = match packet.environment.enter(child) {
        Ok(residue) => residue,
        Err(error) => {
            if let Some(function) = packet.function.take() {
                drop(function);
            }
            let outcome = ThreadOutcome::EnterFailed(error);
            if let Err(outcome) = packet.result.publish(outcome) {
                drop(outcome);
            }
            drop(packet);
            reap_or_abort(registry);
            // SAFETY: the result is published before kernel departure.
            unsafe { libkernel::sys_thread_exit() }
        }
    };
    let function = packet.function.take().expect("thread packet consumed twice");
    let result = function();
    let leave = packet.environment.leave(&mut residue);
    packet.residue.publish(residue);
    let outcome = ThreadOutcome::Returned {
        value: result,
        leave,
    };
    if let Err(outcome) = packet.result.publish(outcome) {
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
        self.prepare(function)?
            .start()
            .map_err(|(_, error)| error)
    }

    pub fn prepare<F, T>(&self, function: F) -> Result<PreparedThread<F, T, E>, SystemCallError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        reap_or_abort(self.registry);
        let slot = self
            .registry
            .reserve()
            .map_err(registry_error_to_system)?;
        let child = self.environment.prepare()
            .map_err(|_| {
                let _ = self.registry.release_reserved(slot);
                SystemCallError::OutOfMemory
            })?;
        let stack_bytes = self
            .stack_bytes
            .checked_add(PROCESS_PAGE_SIZE - 1)
            .ok_or_else(|| {
                let _ = self.registry.release_reserved(slot);
                SystemCallError::IllegalArgument
            })?
            / PROCESS_PAGE_SIZE
            * PROCESS_PAGE_SIZE;
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
                        let _ = self.registry.release_reserved(slot);
                        return Err(error);
                    }
                }
                Err(error) => {
                    let _ = self.registry.release_reserved(slot);
                    return Err(error);
                }
            }
        };
        let result = Arc::new(ResultCell::<ThreadOutcome<T, E>>::new());
        let residue = Arc::new(ResidueCell::empty());
        let packet = Box::new(Packet {
            entry: trampoline::<E, F, T>,
            environment: Arc::clone(&self.environment),
            child: Some(child),
            function: Some(function),
            result: Arc::clone(&result),
            residue: Arc::clone(&residue),
        });
        let packet_pointer = (&*packet as *const Packet<E, F, T>) as usize;
        let stack_pointer = match stack.usable() {
            Some(usable) => usable.end,
            None => {
                drop(packet);
                unmap_or_abort(stack);
                let _ = self.registry.release_reserved(slot);
                return Err(SystemCallError::InternalError);
            }
        };
        let context = ThreadStartContext {
            entry: __libthread_worker_entry as *const () as usize as u64,
            stack_pointer: stack_pointer as u64,
            arg1: packet_pointer as u64,
            arg2: self.registry as *const ThreadRegistry<E> as usize as u64,
        };
        let mut created = ThreadCreateResult {
            tid: 0,
            reserved: 0,
            control: Handle::INVALID,
        };
        // SAFETY: all syscall pointers refer to live stack values; the stack
        // and packet were prepared before creation.
        if let Err(error) = unsafe { kernel_thread::create(Handle::INVALID, &context, &mut created) } {
            drop(packet);
            unmap_or_abort(stack);
            let _ = self.registry.release_reserved(slot);
            return Err(error);
        }
        let control = unsafe { ThreadControl::from_raw(created.control) };
        let thread = Thread {
            id: created.tid,
            control: Arc::new(control),
        };
        let record = ThreadRecord {
            stack: Some(stack),
            control: Some(Arc::clone(&thread.control)),
            environment: Arc::clone(&self.environment),
            residue,
            cleanup: CleanupProgress {
                retired: false,
                abandoned: false,
                done: false,
            },
        };
        if let Err((error, record)) = self.registry.register(slot, record) {
            drop(packet);
            drop(thread);
            let stack = record.stack.expect("new thread record stack missing");
            unmap_or_abort(stack);
            let _ = self.registry.release_reserved(slot);
            return Err(registry_error_to_system(error));
        }
        Ok(PreparedThread {
            result,
            thread: Some(thread),
            packet: Some(packet),
            registry: self.registry,
            slot,
            started: false,
        })
    }
}

pub struct PreparedThread<F, T, E: ThreadEnvironment> {
    result: Arc<ResultCell<ThreadOutcome<T, E>>>,
    thread: Option<Thread>,
    packet: Option<Box<Packet<E, F, T>>>,
    registry: &'static ThreadRegistry<E>,
    slot: usize,
    started: bool,
}

impl<F, T, E: ThreadEnvironment> PreparedThread<F, T, E> {
    pub fn start(mut self) -> Result<ResultReceiver<T, E>, (Self, SystemCallError)>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let packet = self.packet.take().expect("prepared packet missing");
        let packet = Box::into_raw(packet) as usize;
        if let Err(error) = self
            .thread
            .as_ref()
            .expect("PreparedThread started twice")
            .control
            .start()
        {
            // SAFETY: ThreadStart rejected the Created control, so the
            // trampoline could not have consumed this packet.
            self.packet = Some(unsafe { Box::from_raw(packet as *mut Packet<E, F, T>) });
            return Err((self, error));
        }
        let thread = self.thread.take().expect("PreparedThread started twice");
        self.started = true;
        let receiver = ResultReceiver {
            result: self.result.clone(),
            control: Arc::clone(&thread.control),
            registry: self.registry,
            id: thread.id,
        };
        drop(thread);
        Ok(receiver)
    }
}

impl<F, T, E: ThreadEnvironment> Drop for PreparedThread<F, T, E> {
    fn drop(&mut self) {
        if !self.started {
            let thread = self.thread.take();
            if self.registry.cancel_prepared(self.slot).is_err() {
                cleanup_integrity_failure(CleanupFault::InvalidSlot);
            }
            drop(thread);
        }
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


/// Raw ThreadCreate/ThreadStart escape hatches used only by integration
/// workloads that deliberately test the kernel boundary.
///
/// # Safety
///
/// The caller owns all stack, packet and completion responsibilities.
pub unsafe fn spawn_raw(
    context: &ThreadStartContext,
    result: &mut ThreadCreateResult,
) -> Result<(), SystemCallError> {
    // SAFETY: forwarded with the caller's ownership proof.
    unsafe { kernel_thread::create(Handle::INVALID, context, result) }
}

/// # Safety
///
/// `control` must be the MANAGE-capable result of `spawn_raw`.
pub unsafe fn start_raw(control: Handle) -> Result<(), SystemCallError> {
    // SAFETY: forwarded with the caller's ownership proof.
    unsafe { kernel_thread::start(control) }
}

#[cfg(test)]
mod tests {
    use super::ResultCell;

    #[test]
    fn result_cell_publishes_once() {
        let cell = ResultCell::new();
        assert!(cell.publish(7_u32).is_ok());
        assert_eq!(cell.publish(9_u32), Err(9));
        assert_eq!(cell.take(), Some(7));
        assert_eq!(cell.take(), None);
    }

    #[test]
    fn result_cell_abandon_drops_published_value() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use alloc::sync::Arc;

        struct DropProbe(Arc<AtomicUsize>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let cell = ResultCell::new();
        assert!(cell.publish(DropProbe(Arc::clone(&drops))).is_ok());
        cell.abandon();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

}
