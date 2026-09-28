#![no_std]

extern crate alloc;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
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
const WRITING: u8 = 1;
const PUBLISHED: u8 = 2;
const RECEIVER_GONE: u8 = 3;
const DEFAULT_STACK_BYTES: usize = 1024 * 1024;
const DEFAULT_REGISTRY_CAPACITY: usize = 64;

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
        if self
            .state
            .compare_exchange(EMPTY, WRITING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(value);
        }
        // SAFETY: WRITING is exclusively owned by this producer; abandon can
        // mark it ReceiverGone but never accesses the value.
        unsafe { (*self.value.get()).write(value) };
        if self
            .state
            .compare_exchange(WRITING, PUBLISHED, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            Ok(())
        } else {
            // SAFETY: only the receiver can change WRITING to ReceiverGone,
            // and it does not read or drop the in-progress value.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOutcome<T> {
    Returned(T),
    Incomplete,
}

/// Runtime-specific state that can be prepared before a thread is created.
pub trait ThreadEnvironment {
    type Child: Send + 'static;
    type Residue: Send + 'static;
    type Error: Send + 'static;

    fn prepare(&self) -> Result<Self::Child, Self::Error>;
    fn enter(&self, child: Self::Child) -> Result<Self::Residue, Self::Error>;
    fn leave(&self, residue: &mut Self::Residue) -> Result<(), Self::Error>;
    fn retire(&self, residue: Self::Residue) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoEnvironment;

impl ThreadEnvironment for NoEnvironment {
    type Child = ();
    type Residue = ();
    type Error = ();

    fn prepare(&self) -> Result<Self::Child, Self::Error> {
        Ok(())
    }

    fn enter(&self, _: Self::Child) -> Result<Self::Residue, Self::Error> {
        Ok(())
    }

    fn leave(&self, _: &mut Self::Residue) -> Result<(), Self::Error> {
        Ok(())
    }

    fn retire(&self, _: Self::Residue) -> Result<(), Self::Error> {
        Ok(())
    }
}

struct ThreadRecord<E: ThreadEnvironment> {
    stack: Option<MappedRegion>,
    control: Arc<ThreadControl>,
    environment: Arc<E>,
    residue: Arc<ResidueCell<E::Residue>>,
    packet: Option<PacketOwner>,
}

struct RegistryInner<E: ThreadEnvironment> {
    slots: Vec<Option<ThreadRecord<E>>>,
}

/// Fixed-capacity cooperative owner for thread records.
pub struct ThreadRegistry<E: ThreadEnvironment> {
    inner: Mutex<Lock, RegistryInner<E>>,
    sealed: AtomicBool,
    cursor: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    Sealed,
    Full,
    InvalidEntry,
    Cleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapProgress {
    Idle,
    Progress { completed: usize },
    Wait,
    Failed,
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
        slots.resize_with(capacity, || None);
        Ok(Self {
            inner: Mutex::new(RegistryInner { slots }),
            sealed: AtomicBool::new(false),
            cursor: AtomicUsize::new(0),
        })
    }

    pub fn capacity(&self) -> usize {
        self.inner.lock().slots.len()
    }

    pub fn seal(&self) {
        self.sealed.store(true, Ordering::Release);
    }

    fn insert(
        &self,
        record: ThreadRecord<E>,
    ) -> Result<usize, (RegistryError, ThreadRecord<E>)> {
        if self.sealed.load(Ordering::Acquire) {
            return Err((RegistryError::Sealed, record));
        }
        let mut inner = self.inner.lock();
        let Some((index, slot)) = inner
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.is_none())
        else {
            return Err((RegistryError::Full, record));
        };
        *slot = Some(record);
        Ok(index)
    }

    fn cancel(&self, index: usize) -> Result<(), SystemCallError> {
        let mut record = self
            .inner
            .lock()
            .slots
            .get_mut(index)
            .and_then(Option::take)
            .ok_or(SystemCallError::IllegalArgument)?;
        let stack = record.stack.take().ok_or(SystemCallError::InternalError)?;
        match stack.unmap() {
            Ok(()) => {
                drop(record.packet.take());
                drop(record.control);
                Ok(())
            }
            Err((stack, error)) => {
                record.stack = Some(stack);
                self.inner.lock().slots[index] = Some(record);
                Err(error)
            }
        }
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
                let start = self.cursor.fetch_add(1, Ordering::Relaxed) % len;
                let mut found = None;
                for offset in 0..len {
                    let index = (start + offset) % len;
                    if let Some(record) = inner.slots[index].take() {
                        found = Some((index, record));
                        break;
                    }
                }
                found
            }) else {
                break;
            };
            let mut record = record;
            let observed = libkernel::ipc::wait::wait_until(
                &[erhino_shared::wait::WaitItem::new(
                    record.control.as_handle(),
                    erhino_shared::object::ObjectSignals::DONE,
                    0,
                )],
                erhino_shared::time::Deadline::at(0),
            );
            if !matches!(
                observed,
                Ok(result)
                    if result.observed.contains(erhino_shared::object::ObjectSignals::DONE)
            ) {
                self.inner.lock().slots[index] = Some(record);
                saw_wait = true;
                continue;
            }
            if let Some(residue) = record.residue.take()
                    && record.environment.retire(residue).is_err()
            {
                self.inner.lock().slots[index] = Some(record);
                return ReapProgress::Failed;
            }
            let Some(stack) = record.stack.take() else {
                self.inner.lock().slots[index] = Some(record);
                return ReapProgress::Failed;
            };
            match stack.unmap() {
                Ok(()) => {
                    drop(record.packet.take());
                    drop(record.control);
                    completed += 1;
                }
                Err((stack, SystemCallError::ObjectBusy)) => {
                    record.stack = Some(stack);
                    self.inner.lock().slots[index] = Some(record);
                    saw_wait = true;
                }
                Err((stack, _)) => {
                    record.stack = Some(stack);
                    self.inner.lock().slots[index] = Some(record);
                    return ReapProgress::Failed;
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
        let stack = unsafe {
            MappedRegion::from_reservation(
                desc.stack_reservation_start as usize,
                desc.stack_reservation_len as usize,
                desc.usable_start as usize,
                desc.usable_len as usize,
            )
        }
        .map_err(|_| RegistryError::InvalidEntry)?;
        let thread = Thread {
            id: 0,
            control: Arc::new(control),
        };
        self.insert(ThreadRecord {
            stack: Some(stack),
            control: Arc::clone(&thread.control),
            environment: Arc::new(environment),
            residue: Arc::new(ResidueCell::new(residue)),
            packet: None,
        })
        .map_err(|(error, _)| error)?;
        Ok(thread)
    }
}

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

struct Packet<E, F, T>
where
    E: ThreadEnvironment,
{
    environment: Arc<E>,
    child: Option<E::Child>,
    function: Option<F>,
    result: Arc<ResultCell<T>>,
    residue: Arc<ResidueCell<E::Residue>>,
    started: Arc<AtomicBool>,
}

struct PacketOwner {
    ptr: NonNull<()>,
    drop: unsafe fn(NonNull<()>),
}

unsafe impl Send for PacketOwner {}

impl Drop for PacketOwner {
    fn drop(&mut self) {
        // The child has consumed the function before publishing DONE. If a
        // raw exit bypasses the wrapper, retaining the allocation is safer
        // than dropping user closure state on the reaper thread.
        unsafe { (self.drop)(self.ptr) }
    }
}

unsafe fn drop_packet<E, F, T>(ptr: NonNull<()>)
where
    E: ThreadEnvironment,
{
    let packet = unsafe { Box::from_raw(ptr.cast::<Packet<E, F, T>>().as_ptr()) };
    if packet.function.is_none()
        || !packet.started.load(Ordering::Acquire)
    {
        drop(packet);
    } else {
        let _ = Box::leak(packet);
    }
}

extern "C" fn trampoline<E, F, T>(packet: usize, _: usize) -> !
where
    E: ThreadEnvironment,
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // SAFETY: the packet is installed before ThreadStart and remains owned by
    // the registry until DONE.
    let packet = unsafe { &mut *(packet as *mut Packet<E, F, T>) };
    let child = packet.child.take().expect("thread child environment consumed twice");
    let mut residue = match packet.environment.enter(child) {
        Ok(residue) => residue,
        Err(_) => {
            if let Some(function) = packet.function.take() {
                drop(function);
            }
            packet.result.abandon();
            // SAFETY: an environment cannot be entered, so this process
            // cannot satisfy the thread contract.
            unsafe { libkernel::sys_process_abort() }
        }
    };
    let function = packet.function.take().expect("thread packet consumed twice");
    let result = function();
    if packet.environment.leave(&mut residue).is_err() {
        drop(result);
        // SAFETY: the environment failed to leave before result publication.
        unsafe { libkernel::sys_process_abort() }
    }
    packet.residue.publish(residue);
    if let Err(result) = packet.result.publish(result) {
        drop(result);
    }
    // SAFETY: ResultCell has been resolved before the kernel departure.
    unsafe { libkernel::sys_thread_exit() }
}

pub struct ThreadFactory<'r, E: ThreadEnvironment> {
    registry: &'r ThreadRegistry<E>,
    environment: Arc<E>,
    stack_bytes: usize,
}

impl<'r, E: ThreadEnvironment> ThreadFactory<'r, E> {
    pub fn new(registry: &'r ThreadRegistry<E>, environment: E) -> Self {
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

    pub fn spawn<F, T>(&self, function: F) -> Result<JoinHandle<T>, SystemCallError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.prepare(function)?
            .start()
            .map_err(|(_, error)| error)
    }

    pub fn prepare<F, T>(&self, function: F) -> Result<PreparedThread<'_, F, T, E>, SystemCallError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let child = self.environment.prepare()
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let stack_bytes = self
            .stack_bytes
            .checked_add(PROCESS_PAGE_SIZE - 1)
            .ok_or(SystemCallError::IllegalArgument)?
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
                    unsafe { libkernel::sys_sleep(1) }?;
                }
                Err(error) => return Err(error),
            }
        };
        let result = Arc::new(ResultCell::new());
        let residue = Arc::new(ResidueCell::empty());
        let started = Arc::new(AtomicBool::new(false));
        let packet = Box::new(Packet {
            environment: Arc::clone(&self.environment),
            child: Some(child),
            function: Some(function),
            result: Arc::clone(&result),
            residue: Arc::clone(&residue),
            started: Arc::clone(&started),
        });
        let packet = NonNull::from(Box::leak(packet));
        let stack_pointer = stack
            .usable()
            .ok_or(SystemCallError::InternalError)?
            .end;
        let context = ThreadStartContext {
            entry: trampoline::<E, F, T> as *const () as usize as u64,
            stack_pointer: stack_pointer as u64,
            arg1: packet.as_ptr() as usize as u64,
            arg2: 0,
        };
        let mut created = ThreadCreateResult {
            tid: 0,
            reserved: 0,
            control: Handle::INVALID,
        };
        // SAFETY: all syscall pointers refer to live stack values; the stack
        // and packet were prepared before creation.
        if let Err(error) = unsafe { kernel_thread::create(Handle::INVALID, &context, &mut created) } {
            unsafe { drop_packet::<E, F, T>(packet.cast()) };
            let _ = stack.unmap();
            return Err(error);
        }
        let control = unsafe { ThreadControl::from_raw(created.control) };
        let thread = Thread {
            id: created.tid,
            control: Arc::new(control),
        };
        let record = ThreadRecord {
                stack: Some(stack),
                control: Arc::clone(&thread.control),
                environment: Arc::clone(&self.environment),
                residue,
                packet: Some(PacketOwner {
                    ptr: packet.cast(),
                    drop: drop_packet::<E, F, T>,
                }),
            };
        let slot = match self.registry.insert(record) {
            Ok(slot) => slot,
            Err((_, record)) => {
                let stack = record.stack.expect("new thread record stack missing");
                drop(record.packet);
                drop(record.control);
                let _ = stack.unmap();
                return Err(SystemCallError::ReachLimit);
            }
        };
        Ok(PreparedThread {
            result,
            thread: Some(thread),
            registry_ref: self.registry,
            registry: self.registry as *const _ as *const (),
            reap: reap_erased::<E>,
            slot,
            started: false,
            packet_started: started,
            marker: core::marker::PhantomData,
            function: core::marker::PhantomData,
        })
    }
}

pub struct PreparedThread<'r, F, T, E: ThreadEnvironment> {
    result: Arc<ResultCell<T>>,
    thread: Option<Thread>,
    registry_ref: &'r ThreadRegistry<E>,
    registry: *const (),
    reap: unsafe fn(*const (), usize) -> ReapProgress,
    slot: usize,
    started: bool,
    packet_started: Arc<AtomicBool>,
    marker: core::marker::PhantomData<T>,
    function: core::marker::PhantomData<F>,
}

impl<'r, F, T, E: ThreadEnvironment> PreparedThread<'r, F, T, E> {
    pub fn start(mut self) -> Result<JoinHandle<T>, (Self, SystemCallError)>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        if let Err(error) = self
            .thread
            .as_ref()
            .expect("PreparedThread started twice")
            .control
            .start()
        {
            return Err((self, error));
        }
        self.packet_started.store(true, Ordering::Release);
        let thread = self.thread.take().expect("PreparedThread started twice");
        self.started = true;
        Ok(JoinHandle {
            result: self.result.clone(),
            thread: Some(thread),
            registry: self.registry,
            reap: self.reap,
        })
    }
}

impl<'r, F, T, E: ThreadEnvironment> Drop for PreparedThread<'r, F, T, E> {
    fn drop(&mut self) {
        if !self.started {
            let _ = self.registry_ref.cancel(self.slot);
        }
    }
}

pub struct JoinHandle<T> {
    result: Arc<ResultCell<T>>,
    thread: Option<Thread>,
    registry: *const (),
    reap: unsafe fn(*const (), usize) -> ReapProgress,
}

unsafe impl<T: Send> Send for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    pub fn id(&self) -> Tid {
        self.thread.as_ref().expect("joined thread owner missing").id()
    }

    pub fn try_join(&self) -> JoinOutcome<T> {
        let _ = unsafe { (self.reap)(self.registry, 1) };
        match self.result.take() {
            Some(value) => JoinOutcome::Returned(value),
            None => JoinOutcome::Incomplete,
        }
    }

    /// Wait for DONE and return the published value. Dropping a handle remains
    /// non-blocking; only an explicit join opts into observation.
    pub fn join(mut self) -> T {
        let item = erhino_shared::wait::WaitItem::new(
            self.thread
                .as_ref()
                .expect("joined thread owner missing")
                .control()
                .as_handle(),
            erhino_shared::object::ObjectSignals::DONE,
            0,
        );
        let observed = libkernel::ipc::wait::wait_until(
            core::slice::from_ref(&item),
            erhino_shared::time::Deadline::INFINITE,
        )
        .expect("ThreadControl wait failed");
        assert!(
            observed.observed.contains(erhino_shared::object::ObjectSignals::DONE),
            "ThreadControl closed before DONE"
        );
        self.thread.take();
        let _ = unsafe { (self.reap)(self.registry, 1) };
        match self.result.take() {
            Some(value) => value,
            None => panic!("thread wrapper exited without publishing a result"),
        }
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        self.result.abandon();
        self.thread.take();
        // Drop never waits. A completed record may be reclaimed opportunistically.
        let _ = unsafe { (self.reap)(self.registry, 1) };
    }
}

/// Compatibility builder used by existing programs while they are migrated to
/// an explicitly owned runtime registry.
pub struct Builder {
    stack_bytes: usize,
}

impl Builder {
    pub const fn new() -> Self {
        Self {
            stack_bytes: DEFAULT_STACK_BYTES,
        }
    }

    pub const fn stack_size(mut self, bytes: usize) -> Self {
        self.stack_bytes = bytes;
        self
    }

    pub fn spawn<F, T>(self, function: F) -> Result<JoinHandle<T>, SystemCallError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        static REGISTRY: Mutex<Lock, Option<ThreadRegistry<NoEnvironment>>> = Mutex::const_new(
            Lock::new(),
            None,
        );
        let mut guard = REGISTRY.lock();
        if guard.is_none() {
            *guard = Some(
                ThreadRegistry::new(DEFAULT_REGISTRY_CAPACITY)
                    .expect("default thread registry capacity is valid"),
            );
        }
        let registry = guard.as_ref().expect("registry initialized");
        ThreadFactory::new(registry, NoEnvironment)
            .stack_size(self.stack_bytes)
            .spawn(function)
            .map(|mut handle| {
                handle.registry = registry as *const _ as *const ();
                handle.reap = reap_erased::<NoEnvironment>;
                handle
            })
    }
}

unsafe fn reap_erased<E: ThreadEnvironment>(registry: *const (), budget: usize) -> ReapProgress {
    // SAFETY: callers install a pointer to the matching live registry.
    unsafe { &*(registry as *const ThreadRegistry<E>) }.reap(budget)
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn spawn<F, T>(function: F) -> Result<JoinHandle<T>, SystemCallError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(function)
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
    use super::{JoinOutcome, ResultCell};

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

    #[test]
    fn join_outcome_is_explicit_for_unpublished_result() {
        let _: JoinOutcome<u32> = JoinOutcome::Incomplete;
    }
}
