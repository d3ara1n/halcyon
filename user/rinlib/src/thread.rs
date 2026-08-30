use alloc::boxed::Box;
use core::{
    mem::MaybeUninit,
    ptr::NonNull,
    sync::atomic::{Ordering, fence},
};

use erhino_shared::{
    call::SystemCallError,
    mem::MemoryProtection,
    object::{Handle, ObjectSignals},
    proc::{PROCESS_PAGE_SIZE, ThreadSpawnResult, ThreadStartContext, Tid},
    wait::WaitItem,
};

use crate::{
    call::{sys_sleep, sys_thread_exit, sys_thread_spawn, sys_thread_yield},
    ipc::{object, wait},
    mm::{MappedRegion, Placement},
};

const DEFAULT_STACK_BYTES: usize = 1024 * 1024;

/// 直接提交 ThreadSpawn 原语，不接管入口参数、用户栈或返回的 ThreadControl。
///
/// # Safety
///
/// 调用者必须保证入口及栈在新线程离场前有效，并为结果壳和全部用户资源建立
/// 唯一收束路径。通常应使用 [`Builder::spawn`]。
pub unsafe fn spawn_raw(
    context: &ThreadStartContext,
    result: &mut ThreadSpawnResult,
) -> Result<(), SystemCallError> {
    // SAFETY: 借用保证 syscall 期间的输入/输出地址有效；跨调用资源责任由调用者承担。
    unsafe { sys_thread_spawn(context, result) }
}

struct UserStack {
    region: Option<MappedRegion>,
}

impl UserStack {
    fn map(bytes: usize) -> Result<Self, SystemCallError> {
        let bytes = bytes
            .checked_add(PROCESS_PAGE_SIZE - 1)
            .ok_or(SystemCallError::IllegalArgument)?
            / PROCESS_PAGE_SIZE
            * PROCESS_PAGE_SIZE;
        if bytes == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let region = MappedRegion::map_anonymous(
            bytes,
            PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            MemoryProtection::ReadWrite,
            Placement::Anywhere,
        )?;
        Ok(Self {
            region: Some(region),
        })
    }

    fn stack_pointer(&self) -> usize {
        self.region
            .as_ref()
            .and_then(MappedRegion::usable)
            .expect("UserStack lost its usable mapping")
            .end
    }

    fn release(&mut self) {
        let mut region = self.region.take().expect("UserStack released twice");
        loop {
            match region.unmap() {
                Ok(()) => return,
                Err((returned, SystemCallError::ObjectBusy)) => {
                    region = returned;
                    // SAFETY: sleep carries no borrowed user pointer.
                    unsafe { sys_sleep(1) }.expect("UserStack cleanup sleep failed");
                }
                Err((_returned, error)) => panic!("UserStack cleanup failed: {error:?}"),
            }
        }
    }
}

struct Packet<F, T> {
    function: Option<F>,
    result: MaybeUninit<T>,
}

extern "C" fn thread_entry<F, T>(packet: usize, _unused: usize) -> !
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // SAFETY: packet 由 JoinHandle 独占持有到内核 DONE；子线程是 DONE 前唯一访问者。
    let packet = unsafe { &mut *(packet as *mut Packet<F, T>) };
    let function = packet
        .function
        .take()
        .expect("thread entry consumed its function twice");
    packet.result.write(function());
    fence(Ordering::Release);
    // SAFETY: result 已完整发布；成功的 ThreadExit 不返回本线程。
    unsafe { sys_thread_exit(0) }
}

unsafe fn take_packet<F, T>(packet: NonNull<()>) -> T {
    // SAFETY: DONE 后子线程不再访问，且本函数消费唯一 erased Box identity。
    let packet = unsafe { Box::from_raw(packet.cast::<Packet<F, T>>().as_ptr()) };
    // SAFETY: wrapper 只在写完 result 后调用 ThreadExit，DONE 晚于该调用。
    unsafe { packet.result.assume_init_read() }
}

unsafe fn drop_packet<F, T>(packet: NonNull<()>) {
    // SAFETY: 与 take_packet 相同；Drop 路径同样只在 DONE 后进入。
    let mut packet = unsafe { Box::from_raw(packet.cast::<Packet<F, T>>().as_ptr()) };
    // SAFETY: wrapper 在 DONE 前已经初始化 result。
    unsafe { packet.result.assume_init_drop() };
}

#[derive(Debug, Clone, Copy)]
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
        let mut stack = UserStack::map(self.stack_bytes)?;
        let packet: Box<Packet<F, T>> = Box::new(Packet {
            function: Some(function),
            result: MaybeUninit::uninit(),
        });
        let packet = NonNull::from(Box::leak(packet));
        let context = ThreadStartContext {
            entry: thread_entry::<F, T> as *const () as usize as u64,
            stack_pointer: stack.stack_pointer() as u64,
            arg1: packet.as_ptr() as usize as u64,
            arg2: 0,
        };
        let mut result = ThreadSpawnResult {
            tid: 0,
            reserved: 0,
            control: Handle::INVALID,
        };
        // SAFETY: context/result 在同步 syscall 期间稳定；stack 与 packet 已先发布。
        if let Err(error) = unsafe { spawn_raw(&context, &mut result) } {
            // SAFETY: 失败契约保证线程未入册运行，packet 仍由调用方独占。
            unsafe { drop(Box::from_raw(packet.cast::<Packet<F, T>>().as_ptr())) };
            stack.release();
            return Err(error);
        }
        assert!(
            result.tid != 0 && result.reserved == 0 && result.control.is_valid(),
            "ThreadSpawn returned an invalid fixed-width result"
        );
        Ok(JoinHandle {
            tid: result.tid,
            control: Some(result.control),
            stack: Some(stack),
            packet: Some(packet.cast()),
            take_result: take_packet::<F, T>,
            drop_result: drop_packet::<F, T>,
        })
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use = "dropping a JoinHandle waits for the thread and releases its stack"]
pub struct JoinHandle<T> {
    tid: Tid,
    control: Option<Handle>,
    stack: Option<UserStack>,
    packet: Option<NonNull<()>>,
    take_result: unsafe fn(NonNull<()>) -> T,
    drop_result: unsafe fn(NonNull<()>),
}

// SAFETY: T 与启动函数均要求 Send；DONE 前 packet 只由子线程访问，DONE 后只由
// 持有 JoinHandle 的线程访问。UserStack 和 Handle 均为 affine 值所有权。
unsafe impl<T: Send> Send for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    pub const fn id(&self) -> Tid {
        self.tid
    }

    fn wait_and_release(&mut self) {
        let control = self.control.take().expect("JoinHandle waited twice");
        let item = WaitItem::new(control, ObjectSignals::DONE, 1);
        let observed =
            wait::wait_many(core::slice::from_ref(&item), 0).expect("ThreadControl wait failed");
        assert!(
            observed.observed.contains(ObjectSignals::DONE),
            "ThreadControl closed before DONE"
        );
        fence(Ordering::Acquire);
        object::close(control).expect("ThreadControl close failed");
        self.stack
            .as_mut()
            .expect("JoinHandle lost its UserStack")
            .release();
        self.stack.take();
    }

    pub fn join(mut self) -> T {
        self.wait_and_release();
        let packet = self
            .packet
            .take()
            .expect("JoinHandle consumed its result twice");
        // SAFETY: wait_and_release observed DONE with acquire ordering and consumed the stack.
        unsafe { (self.take_result)(packet) }
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        let Some(packet) = self.packet.take() else {
            return;
        };
        self.wait_and_release();
        // SAFETY: structured Drop observes DONE before destroying the initialized result packet.
        unsafe { (self.drop_result)(packet) };
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
    // SAFETY: ThreadYield carries no borrowed user pointer.
    unsafe { sys_thread_yield() }
}

pub fn exit(code: i64) -> ! {
    // SAFETY: ThreadExit is terminal for the current thread.
    unsafe { sys_thread_exit(code) }
}
