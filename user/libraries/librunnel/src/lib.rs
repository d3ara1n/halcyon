//! RNL2：有界共享映射上的单工 SPSC 字节流。

#![cfg_attr(not(test), no_std)]
#![cfg(any(test, target_arch = "riscv64"))]

extern crate alloc;

use core::sync::atomic::Ordering;

use erhino_shared::{call::SystemCallError, time::Deadline};

pub const HEADER_BYTES: usize = 128;
pub const MAGIC: u32 = 0x324c_4e52;
pub const VERSION_HEADER: u32 = (128 << 16) | 3;
const PRODUCER_START: usize = 0x2c;
const CONSUMER_START: usize = 0x30;

#[cfg(target_arch = "riscv64")]
mod mapping;
#[cfg(target_arch = "riscv64")]
pub use mapping::{MapFailure, MappedEndpoint, MappingGeometry};
const HEAD: usize = 0x18;
const TAIL: usize = 0x20;
const EOF: usize = 0x28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnelError {
    BadFormat,
    Broken,
    Closed,
    TimedOut,
    Syscall(SystemCallError),
}

/// 生产者侧可继续推进的等待条件；终态统一经 [`IoError`] 报告。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProducerReady {
    Writable {
        bytes: usize,
    },
    /// 对端已在成功映射/协议校验后 release 发布 Start。
    Started,
    /// 已发布 EOF 且对端已消费至最终 head。
    EofConsumed,
}

/// 消费者侧等待条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerReady {
    Readable {
        bytes: usize,
        /// 本次首次观察到对端协议 Start。
        peer_started: bool,
    },
    /// 对端协议 Start 已发布但尚无数据。
    Started,
    /// 已观察 EOF 且本地已消费至最终 head。
    EofDrained,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoError {
    pub error: RunnelError,
    pub completed: usize,
}

/// 永久的协议/承载分工；host 使用合规原子存储，guest 使用 owner 约束的平台访问。
trait Transport {
    fn bytes(&self) -> usize;
    fn load32(&self, offset: usize, order: Ordering) -> u32;
    fn load64(&self, offset: usize, order: Ordering) -> u64;
    fn store32(&self, offset: usize, value: u32, order: Ordering);
    fn store64(&self, offset: usize, value: u64, order: Ordering);
    fn read(&self, offset: usize, output: &mut [u8]);
    fn write(&self, offset: usize, input: &[u8]);
    fn notify(&self) -> Result<(), SystemCallError>;
    fn acknowledge(&self) -> Result<(), SystemCallError>;
    fn wait(&self, deadline: Deadline) -> Result<Option<bool>, SystemCallError>;
    fn close(&mut self) -> Result<(), SystemCallError>;
}

pub struct InitFailure<T> {
    pub owner: T,
    pub error: RunnelError,
}

impl<T> core::fmt::Debug for InitFailure<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InitFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

struct Channel<T> {
    transport: T,
    capacity: usize,
    terminal: Option<RunnelError>,
}

impl<T: Transport> Channel<T> {
    fn new(transport: T, creator: bool) -> Result<Self, InitFailure<T>> {
        let bytes = transport.bytes();
        if bytes < 4096
            || !bytes.is_multiple_of(4096)
            || bytes > erhino_shared::tunnel::TUNNEL_MAX_PAGES as usize * 4096
        {
            return Err(InitFailure {
                owner: transport,
                error: RunnelError::BadFormat,
            });
        }
        let capacity = bytes - HEADER_BYTES;
        if creator {
            transport.store32(0, 0, Ordering::Relaxed);
            transport.store32(4, VERSION_HEADER.to_le(), Ordering::Relaxed);
            transport.store64(8, (bytes as u64).to_le(), Ordering::Relaxed);
            transport.store64(0x10, (capacity as u64).to_le(), Ordering::Relaxed);
            transport.store64(HEAD, 0, Ordering::Relaxed);
            transport.store64(TAIL, 0, Ordering::Relaxed);
            transport.store32(EOF, 0, Ordering::Relaxed);
            transport.store32(0x2c, 0, Ordering::Relaxed);
            transport.store32(CONSUMER_START, 0, Ordering::Relaxed);
            transport.write(0x34, &[0; 76]);
            transport.store32(0, MAGIC.to_le(), Ordering::Release);
        } else if u32::from_le(transport.load32(0, Ordering::Acquire)) != MAGIC
            || u32::from_le(transport.load32(4, Ordering::Relaxed)) != VERSION_HEADER
            || u64::from_le(transport.load64(8, Ordering::Relaxed)) != bytes as u64
            || u64::from_le(transport.load64(0x10, Ordering::Relaxed)) != capacity as u64
            || u32::from_le(transport.load32(PRODUCER_START, Ordering::Acquire)) > 1
            || u32::from_le(transport.load32(CONSUMER_START, Ordering::Acquire)) > 1
        {
            return Err(InitFailure {
                owner: transport,
                error: RunnelError::BadFormat,
            });
        }
        Ok(Self {
            transport,
            capacity,
            terminal: None,
        })
    }

    fn check(&self) -> Result<(), IoError> {
        match self.terminal {
            Some(error) => Err(IoError {
                error,
                completed: 0,
            }),
            None => Ok(()),
        }
    }

    fn fail(&mut self, error: RunnelError, completed: usize) -> IoError {
        if self.terminal.is_none() {
            self.terminal = Some(error);
        }
        IoError {
            error: self.terminal.unwrap(),
            completed,
        }
    }

    fn notify(&mut self, completed: usize) -> Result<(), IoError> {
        match self.transport.notify() {
            Ok(()) | Err(SystemCallError::ObjectNotAvailable) => Ok(()),
            Err(SystemCallError::ObjectClosed) => Err(self.fail(RunnelError::Closed, completed)),
            Err(error) => Err(self.fail(RunnelError::Syscall(error), completed)),
        }
    }

    fn acknowledge(&mut self) -> Result<(), IoError> {
        self.check()?;
        self.transport
            .acknowledge()
            .map_err(|error| self.fail(RunnelError::Syscall(error), 0))
    }

    fn wait(&mut self, deadline: Deadline) -> Result<(), IoError> {
        self.check()?;
        match self.transport.wait(deadline) {
            Ok(Some(false)) => Ok(()),
            Ok(None) => Err(IoError {
                error: RunnelError::TimedOut,
                completed: 0,
            }),
            Ok(Some(true)) | Err(SystemCallError::ObjectClosed) => {
                Err(self.fail(RunnelError::Closed, 0))
            }
            Err(error) => Err(self.fail(RunnelError::Syscall(error), 0)),
        }
    }

    fn close(&mut self) -> Result<(), SystemCallError> {
        self.transport.close()
    }

    fn copy_in(&self, cursor: usize, input: &[u8]) {
        let first = input.len().min(self.capacity - cursor);
        self.transport.write(HEADER_BYTES + cursor, &input[..first]);
        self.transport.write(HEADER_BYTES, &input[first..]);
    }
    fn copy_out(&self, cursor: usize, output: &mut [u8]) {
        let first = output.len().min(self.capacity - cursor);
        let (left, right) = output.split_at_mut(first);
        self.transport.read(HEADER_BYTES + cursor, left);
        self.transport.read(HEADER_BYTES, right);
    }
}

struct ProducerCore<T> {
    channel: Channel<T>,
    head: u64,
    tail_shadow: u64,
    cursor: usize,
    eof: bool,
    start_observed: bool,
}
impl<T: Transport> ProducerCore<T> {
    fn new(transport: T, creator: bool) -> Result<Self, InitFailure<T>> {
        let channel = Channel::new(transport, creator)?;
        if channel.transport.load64(HEAD, Ordering::Relaxed) != 0
            || channel.transport.load64(TAIL, Ordering::Acquire) != 0
            || channel.transport.load32(EOF, Ordering::Acquire) != 0
        {
            return Err(InitFailure {
                owner: channel.transport,
                error: RunnelError::Broken,
            });
        }
        channel
            .transport
            .store32(PRODUCER_START, 1u32.to_le(), Ordering::Release);
        if let Err(error) = channel.transport.notify()
            && error != SystemCallError::ObjectNotAvailable
        {
            return Err(InitFailure {
                owner: channel.transport,
                error: RunnelError::Syscall(error),
            });
        }
        Ok(Self {
            channel,
            head: 0,
            tail_shadow: 0,
            cursor: 0,
            eof: false,
            start_observed: false,
        })
    }

    fn writable(&mut self) -> Result<usize, IoError> {
        self.channel.check()?;
        let transport = &self.channel.transport;
        if u64::from_le(transport.load64(HEAD, Ordering::Relaxed)) != self.head
            || u32::from_le(transport.load32(EOF, Ordering::Acquire)) != u32::from(self.eof)
        {
            return Err(self.channel.fail(RunnelError::Broken, 0));
        }
        let tail = u64::from_le(transport.load64(TAIL, Ordering::Acquire));
        let outstanding = self.head.wrapping_sub(self.tail_shadow);
        if tail.wrapping_sub(self.tail_shadow) > outstanding
            || self.head.wrapping_sub(tail) > self.channel.capacity as u64
        {
            return Err(self.channel.fail(RunnelError::Broken, 0));
        }
        self.tail_shadow = tail;
        Ok(self.channel.capacity - self.head.wrapping_sub(tail) as usize)
    }

    fn write(&mut self, input: &[u8]) -> Result<usize, IoError> {
        self.channel.check()?;
        if input.is_empty() {
            return Ok(0);
        }
        if self.eof {
            return Err(self.channel.fail(RunnelError::Broken, 0));
        }
        let count = self.writable()?.min(input.len());
        if count != 0 {
            self.channel.copy_in(self.cursor, &input[..count]);
            self.cursor = (self.cursor + count) % self.channel.capacity;
            self.head = self.head.wrapping_add(count as u64);
            self.channel
                .transport
                .store64(HEAD, self.head.to_le(), Ordering::Release);
            self.channel.notify(count)?;
        }
        Ok(count)
    }

    fn finish(&mut self) -> Result<(), IoError> {
        self.channel.check()?;
        if self.eof {
            return Ok(());
        }
        self.writable()?;
        self.eof = true;
        self.channel
            .transport
            .store32(EOF, 1u32.to_le(), Ordering::Release);
        self.channel.notify(0)
    }

    fn write_all(&mut self, input: &[u8], deadline: Deadline) -> Result<(), IoError> {
        self.channel.check()?;
        let mut completed = 0;
        let result = (|| {
            while completed < input.len() {
                let count = self.write(&input[completed..])?;
                completed += count;
                if count == 0 {
                    self.channel.acknowledge()?;
                    if self.writable()? == 0 {
                        self.channel.wait(deadline)?;
                    }
                }
            }
            Ok(())
        })();
        result.map_err(|mut error: IoError| {
            error.completed += completed;
            error
        })
    }

    fn probe_ready(&mut self) -> Result<Option<ProducerReady>, IoError> {
        let writable = self.writable()?;
        if self.eof {
            return Ok((writable == self.channel.capacity).then_some(ProducerReady::EofConsumed));
        }
        if writable > 0 {
            return Ok(Some(ProducerReady::Writable { bytes: writable }));
        }
        Ok(None)
    }

    /// 唤醒后重查：先探条件，未满足则确认 DATA 后再探一次；
    /// 仍未满足返回 None（任务重新登记/重 arm 后停驻）。
    fn poll(&mut self, terminal: bool) -> Result<Option<ProducerReady>, IoError> {
        self.channel.check()?;
        if terminal {
            return Err(self.channel.fail(RunnelError::Closed, 0));
        }
        let started = u32::from_le(
            self.channel
                .transport
                .load32(CONSUMER_START, Ordering::Acquire),
        ) == 1;
        if started && !self.start_observed {
            self.start_observed = true;
            return Ok(Some(ProducerReady::Started));
        }
        if let Some(ready) = self.probe_ready()? {
            return Ok(Some(ready));
        }
        self.channel.acknowledge()?;
        let started = u32::from_le(
            self.channel
                .transport
                .load32(CONSUMER_START, Ordering::Acquire),
        ) == 1;
        if started && !self.start_observed {
            self.start_observed = true;
            return Ok(Some(ProducerReady::Started));
        }
        self.probe_ready()
    }
}

struct ConsumerCore<T> {
    channel: Channel<T>,
    tail: u64,
    head_shadow: u64,
    cursor: usize,
    eof_head: Option<u64>,
    start_observed: bool,
}

impl<T: Transport> ConsumerCore<T> {
    fn new(transport: T, creator: bool) -> Result<Self, InitFailure<T>> {
        let channel = Channel::new(transport, creator)?;
        let eof = u32::from_le(channel.transport.load32(EOF, Ordering::Acquire));
        let head = u64::from_le(channel.transport.load64(HEAD, Ordering::Acquire));
        if channel.transport.load64(TAIL, Ordering::Relaxed) != 0
            || eof > 1
            || head > channel.capacity as u64
        {
            return Err(InitFailure {
                owner: channel.transport,
                error: RunnelError::Broken,
            });
        }
        channel
            .transport
            .store32(CONSUMER_START, 1u32.to_le(), Ordering::Release);
        if let Err(error) = channel.transport.notify()
            && error != SystemCallError::ObjectNotAvailable
        {
            return Err(InitFailure {
                owner: channel.transport,
                error: RunnelError::Syscall(error),
            });
        }
        Ok(Self {
            channel,
            tail: 0,
            head_shadow: head,
            cursor: 0,
            eof_head: (eof == 1).then_some(head),
            start_observed: false,
        })
    }

    fn refresh(&mut self) -> Result<u64, IoError> {
        self.channel.check()?;
        let transport = &self.channel.transport;
        let eof = u32::from_le(transport.load32(EOF, Ordering::Acquire));
        let head = u64::from_le(transport.load64(HEAD, Ordering::Acquire));
        if eof > 1
            || (self.eof_head.is_some() && eof != 1)
            || u64::from_le(transport.load64(TAIL, Ordering::Relaxed)) != self.tail
        {
            return Err(self.channel.fail(RunnelError::Broken, 0));
        }
        let previous_used = self.head_shadow.wrapping_sub(self.tail);
        let free = self.channel.capacity as u64 - previous_used;
        if head.wrapping_sub(self.head_shadow) > free
            || head.wrapping_sub(self.tail) > self.channel.capacity as u64
            || self.eof_head.is_some_and(|final_head| head != final_head)
        {
            return Err(self.channel.fail(RunnelError::Broken, 0));
        }
        self.head_shadow = head;
        if eof == 1 {
            self.eof_head = Some(head);
        }
        Ok(head)
    }

    fn readable(&mut self) -> Result<usize, IoError> {
        Ok(self.refresh()?.wrapping_sub(self.tail) as usize)
    }
    fn eof_reached(&mut self) -> Result<bool, IoError> {
        self.refresh()?;
        Ok(self.eof_head == Some(self.tail))
    }

    fn read(&mut self, output: &mut [u8]) -> Result<usize, IoError> {
        self.channel.check()?;
        if output.is_empty() {
            return Ok(0);
        }
        let count = self.readable()?.min(output.len());
        if count != 0 {
            self.channel.copy_out(self.cursor, &mut output[..count]);
            self.cursor = (self.cursor + count) % self.channel.capacity;
            self.tail = self.tail.wrapping_add(count as u64);
            self.channel
                .transport
                .store64(TAIL, self.tail.to_le(), Ordering::Release);
            self.channel.notify(count)?;
        }
        Ok(count)
    }

    fn probe_ready(&mut self) -> Result<Option<ConsumerReady>, IoError> {
        // HEAD acquire 之后再看 Start；首批数据不能消耗早于它的过期 Start 快照。
        let readable = self.readable()?;
        let started = u32::from_le(
            self.channel
                .transport
                .load32(PRODUCER_START, Ordering::Acquire),
        ) == 1;
        let new_start = started && !self.start_observed;
        // Start 是独立的持久事实；空流 EOF 也不能绕过其 acquire 观察。
        self.start_observed |= started;
        if readable > 0 {
            return Ok(Some(ConsumerReady::Readable {
                bytes: readable,
                peer_started: new_start,
            }));
        }
        if self.eof_reached()? {
            return Ok(Some(ConsumerReady::EofDrained));
        }
        if new_start {
            return Ok(Some(ConsumerReady::Started));
        }
        Ok(None)
    }

    /// 唤醒后重查；Ack 之后重新采样 HEAD 和协议 Start。
    fn poll(&mut self, terminal: bool) -> Result<Option<ConsumerReady>, IoError> {
        self.channel.check()?;
        if terminal {
            return Err(self.channel.fail(RunnelError::Closed, 0));
        }
        if let Some(ready) = self.probe_ready()? {
            return Ok(Some(ready));
        }
        self.channel.acknowledge()?;
        self.probe_ready()
    }

    /// 对端协议 Start 尚未被本地推进观察。
    #[cfg(test)]
    fn includes_peer_started(&self) -> bool {
        !self.start_observed
    }

    fn read_exact_or_eof(
        &mut self,
        output: &mut [u8],
        deadline: Deadline,
    ) -> Result<usize, IoError> {
        self.channel.check()?;
        let mut completed = 0;
        let result = (|| {
            while completed < output.len() {
                let count = self.read(&mut output[completed..])?;
                completed += count;
                if completed == output.len() || self.eof_reached()? {
                    break;
                }
                if self.readable()? == 0 {
                    self.channel.acknowledge()?;
                    if self.readable()? == 0 && !self.eof_reached()? {
                        self.channel.wait(deadline)?;
                    }
                }
            }
            Ok(completed)
        })();
        result.map_err(|mut error: IoError| {
            error.completed += completed;
            error
        })
    }
}

#[cfg(target_arch = "riscv64")]
pub mod blocking {
    use super::MappedEndpoint;
    use super::*;
    use erhino_shared::{
        object::ObjectSignals,
        wait::{WaitReason, WaitResult},
    };
    use libkernel::mm::Placement;

    struct Guest {
        endpoint: Option<MappedEndpoint>,
        bytes: usize,
    }
    impl Guest {
        fn new(endpoint: MappedEndpoint) -> Self {
            let bytes = endpoint.geometry().bytes();
            Self {
                endpoint: Some(endpoint),
                bytes,
            }
        }
        fn into_endpoint(self) -> MappedEndpoint {
            self.endpoint
                .expect("initialization failure lost Endpoint owner")
        }
        fn endpoint(&self) -> &MappedEndpoint {
            self.endpoint
                .as_ref()
                .expect("closed channel accessed its mapping")
        }
    }
    impl Transport for Guest {
        fn bytes(&self) -> usize {
            self.bytes
        }
        fn load32(&self, offset: usize, order: Ordering) -> u32 {
            self.endpoint().memory().load_u32(offset, order)
        }
        fn load64(&self, offset: usize, order: Ordering) -> u64 {
            self.endpoint().memory().load_u64(offset, order)
        }
        fn store32(&self, offset: usize, value: u32, order: Ordering) {
            self.endpoint().memory().store_u32(offset, value, order);
        }
        fn store64(&self, offset: usize, value: u64, order: Ordering) {
            self.endpoint().memory().store_u64(offset, value, order);
        }
        fn read(&self, offset: usize, output: &mut [u8]) {
            self.endpoint().memory().read(offset, output);
        }
        fn write(&self, offset: usize, input: &[u8]) {
            self.endpoint().memory().write(offset, input);
        }
        fn notify(&self) -> Result<(), SystemCallError> {
            self.endpoint().events().notify()
        }
        fn acknowledge(&self) -> Result<(), SystemCallError> {
            self.endpoint().events().acknowledge_data()
        }
        fn wait(&self, deadline: Deadline) -> Result<Option<bool>, SystemCallError> {
            self.endpoint()
                .events()
                .wait_until(
                    ObjectSignals::DATA | ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED,
                    deadline,
                )
                .and_then(|result| match WaitReason::from_u32(result.reason) {
                    Some(WaitReason::Timeout) => Ok(None),
                    Some(WaitReason::Closed) => Ok(Some(true)),
                    Some(WaitReason::Signaled) => {
                        Ok(Some(result.observed.intersects(
                            ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED,
                        )))
                    }
                    None => Err(SystemCallError::InternalError),
                })
        }
        fn close(&mut self) -> Result<(), SystemCallError> {
            match self.endpoint.take() {
                Some(endpoint) => match endpoint.close() {
                    Ok(()) => Ok(()),
                    Err((owner, error)) => {
                        self.endpoint = Some(owner);
                        Err(error)
                    }
                },
                None => Ok(()),
            }
        }
    }

    /// 领域登记计划：承载 endpoint 观察项的 Copy 值，handle 不出域；
    /// cookie 由运行体在登记时注入重建。
    pub struct Producer {
        core: ProducerCore<Guest>,
        not_sync: core::marker::PhantomData<core::cell::Cell<()>>,
    }
    pub struct Consumer {
        core: ConsumerCore<Guest>,
        not_sync: core::marker::PhantomData<core::cell::Cell<()>>,
    }

    #[derive(Debug)]
    pub enum AttachFailure {
        Tunnel(libkernel::ipc::invitation::AttachFailure),
        Map(MapFailure),
        Protocol(InitFailure<MappedEndpoint>),
    }

    #[derive(Debug)]
    pub enum CreateFailure {
        Map {
            failure: MapFailure,
            invitation: libkernel::ipc::invitation::Invitation,
        },
        Tunnel(libkernel::ipc::invitation::CreateFailure),
        Protocol {
            endpoint: MappedEndpoint,
            invitation: libkernel::ipc::invitation::Invitation,
            error: RunnelError,
        },
    }

    fn producer(
        endpoint: MappedEndpoint,
        creator: bool,
    ) -> Result<Producer, InitFailure<MappedEndpoint>> {
        ProducerCore::new(Guest::new(endpoint), creator)
            .map(|core| Producer {
                core,
                not_sync: core::marker::PhantomData,
            })
            .map_err(|failure| InitFailure {
                owner: failure.owner.into_endpoint(),
                error: failure.error,
            })
    }
    fn consumer(
        endpoint: MappedEndpoint,
        creator: bool,
    ) -> Result<Consumer, InitFailure<MappedEndpoint>> {
        ConsumerCore::new(Guest::new(endpoint), creator)
            .map(|core| Consumer {
                core,
                not_sync: core::marker::PhantomData,
            })
            .map_err(|failure| InitFailure {
                owner: failure.owner.into_endpoint(),
                error: failure.error,
            })
    }

    impl Producer {
        pub fn create(
            pool: erhino_shared::object::Handle,
            bytes: usize,
            placement: Placement,
        ) -> Result<(Self, libkernel::ipc::invitation::Invitation), CreateFailure> {
            let (endpoint, invitation, bytes) =
                libkernel::ipc::invitation::Invitation::create(pool, bytes)
                    .map_err(CreateFailure::Tunnel)?;
            let endpoint = match MappedEndpoint::map(endpoint, bytes, placement) {
                Ok(endpoint) => endpoint,
                Err(failure) => {
                    return Err(CreateFailure::Map {
                        failure,
                        invitation,
                    });
                }
            };
            match producer(endpoint, true) {
                Ok(producer) => Ok((producer, invitation)),
                Err(failure) => Err(CreateFailure::Protocol {
                    endpoint: failure.owner,
                    invitation,
                    error: failure.error,
                }),
            }
        }
        pub fn attach(
            invitation: libkernel::ipc::invitation::Invitation,
            bytes: usize,
            placement: Placement,
        ) -> Result<Self, AttachFailure> {
            let endpoint = invitation.attach().map_err(AttachFailure::Tunnel)?;
            let endpoint =
                MappedEndpoint::map(endpoint, bytes, placement).map_err(AttachFailure::Map)?;
            producer(endpoint, false).map_err(AttachFailure::Protocol)
        }
        pub fn capacity(&self) -> usize {
            self.core.channel.capacity
        }
        /// 已 acquire 观察到对端协议 Start。
        pub fn peer_started(&self) -> bool {
            self.core.start_observed
        }
        pub fn writable(&mut self) -> Result<usize, IoError> {
            self.core.writable()
        }
        pub fn write(&mut self, input: &[u8]) -> Result<usize, IoError> {
            self.core.write(input)
        }
        pub fn write_all(&mut self, input: &[u8]) -> Result<(), IoError> {
            self.write_all_until(input, Deadline::INFINITE)
        }
        pub fn write_all_until(&mut self, input: &[u8], deadline: Deadline) -> Result<(), IoError> {
            self.core.write_all(input, deadline)
        }
        pub fn finish(&mut self) -> Result<(), IoError> {
            self.core.finish()
        }
        #[expect(
            clippy::result_large_err,
            reason = "关闭失败须返还完整映射和 Endpoint owner，清理路径不引入可失败装箱"
        )]
        pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
            match self.core.channel.close() {
                Ok(()) => Ok(()),
                Err(error) => Err((self, error)),
            }
        }

        /// 当前等待条件的登记计划；DATA 涵盖腾空与全部消费两种进展。
        pub fn wait_plan(&mut self) -> Result<libexecution::runtime::SourcePlan, IoError> {
            self.core.channel.check()?;
            let signals = ObjectSignals::DATA | ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED;
            let item = self
                .core
                .channel
                .transport
                .endpoint()
                .events()
                .wait_item(signals, 0);
            Ok(libexecution::runtime::SourcePlan::new(
                item.handle,
                item.signals,
            ))
        }

        pub fn terminal_wait_plan(&self) -> libexecution::runtime::SourcePlan {
            let item = self
                .core
                .channel
                .transport
                .endpoint()
                .events()
                .wait_item(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED, 0);
            libexecution::runtime::SourcePlan::new(item.handle, item.signals)
        }

        /// 唤醒后重查：观察信号合成类型化条件；终态经 IoError 报告。
        pub fn poll(&mut self, observed: ObjectSignals) -> Result<Option<ProducerReady>, IoError> {
            let terminal = observed.intersects(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED);
            self.core.poll(terminal)
        }
    }

    impl Consumer {
        pub fn create(
            pool: erhino_shared::object::Handle,
            bytes: usize,
            placement: Placement,
        ) -> Result<(Self, libkernel::ipc::invitation::Invitation), CreateFailure> {
            let (endpoint, invitation, bytes) =
                libkernel::ipc::invitation::Invitation::create(pool, bytes)
                    .map_err(CreateFailure::Tunnel)?;
            let endpoint = match MappedEndpoint::map(endpoint, bytes, placement) {
                Ok(endpoint) => endpoint,
                Err(failure) => {
                    return Err(CreateFailure::Map {
                        failure,
                        invitation,
                    });
                }
            };
            match consumer(endpoint, true) {
                Ok(consumer) => Ok((consumer, invitation)),
                Err(failure) => Err(CreateFailure::Protocol {
                    endpoint: failure.owner,
                    invitation,
                    error: failure.error,
                }),
            }
        }
        pub fn attach(
            invitation: libkernel::ipc::invitation::Invitation,
            bytes: usize,
            placement: Placement,
        ) -> Result<Self, AttachFailure> {
            let endpoint = invitation.attach().map_err(AttachFailure::Tunnel)?;
            let endpoint =
                MappedEndpoint::map(endpoint, bytes, placement).map_err(AttachFailure::Map)?;
            consumer(endpoint, false).map_err(AttachFailure::Protocol)
        }
        pub fn capacity(&self) -> usize {
            self.core.channel.capacity
        }
        /// 已经通过协议 Start 观察到对端就绪。
        pub fn peer_started(&self) -> bool {
            self.core.start_observed
        }
        pub fn readable(&mut self) -> Result<usize, IoError> {
            self.core.readable()
        }
        pub fn read(&mut self, output: &mut [u8]) -> Result<usize, IoError> {
            self.core.read(output)
        }
        pub fn read_exact_or_eof(&mut self, output: &mut [u8]) -> Result<usize, IoError> {
            self.read_exact_or_eof_until(output, Deadline::INFINITE)
        }
        pub fn read_exact_or_eof_until(
            &mut self,
            output: &mut [u8],
            deadline: Deadline,
        ) -> Result<usize, IoError> {
            self.core.read_exact_or_eof(output, deadline)
        }
        pub fn eof_reached(&mut self) -> Result<bool, IoError> {
            self.core.eof_reached()
        }
        #[expect(
            clippy::result_large_err,
            reason = "关闭失败须返还完整映射和 Endpoint owner，清理路径不引入可失败装箱"
        )]
        pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
            match self.core.channel.close() {
                Ok(()) => Ok(()),
                Err(error) => Err((self, error)),
            }
        }
        pub fn wait_peer_closed(&mut self, timeout: u64) -> Result<WaitResult, IoError> {
            self.core.channel.check()?;
            let result = self
                .core
                .channel
                .transport
                .endpoint()
                .events()
                .wait(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED, timeout)
                .map_err(|error| self.core.channel.fail(RunnelError::Syscall(error), 0))?;
            if result
                .observed
                .intersects(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED)
            {
                self.core.channel.terminal = Some(RunnelError::Closed);
            }
            Ok(result)
        }

        /// 当前登记只含 DATA 与终态；Start 由共享协议字段复检。
        pub fn wait_plan(&mut self) -> Result<libexecution::runtime::SourcePlan, IoError> {
            self.core.channel.check()?;
            let signals = ObjectSignals::DATA | ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED;
            let item = self
                .core
                .channel
                .transport
                .endpoint()
                .events()
                .wait_item(signals, 0);
            Ok(libexecution::runtime::SourcePlan::new(
                item.handle,
                item.signals,
            ))
        }

        pub fn terminal_wait_plan(&self) -> libexecution::runtime::SourcePlan {
            let item = self
                .core
                .channel
                .transport
                .endpoint()
                .events()
                .wait_item(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED, 0);
            libexecution::runtime::SourcePlan::new(item.handle, item.signals)
        }

        /// 唤醒后重查：观察信号合成类型化条件；终态经 IoError 报告。
        pub fn poll(&mut self, observed: ObjectSignals) -> Result<Option<ConsumerReady>, IoError> {
            let terminal = observed.intersects(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED);
            self.core.poll(terminal)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize},
        },
        vec::Vec,
    };

    struct Storage {
        magic: AtomicU32,
        version: AtomicU32,
        bytes: AtomicU64,
        capacity: AtomicU64,
        head: AtomicU64,
        tail: AtomicU64,
        eof: AtomicU32,
        producer_start: AtomicU32,
        consumer_start: AtomicU32,
        reserved: [AtomicU8; 76],
        data: Vec<AtomicU8>,
    }
    struct Host {
        storage: Arc<Storage>,
        fail_notify: Arc<AtomicBool>,
        wait_timeout: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
        notifications: Arc<AtomicUsize>,
        fail_close: bool,
        invited: bool,
        publish_on_head: Arc<AtomicBool>,
    }
    impl Host {
        fn pair(pages: usize) -> (Self, Self) {
            let bytes = pages * 4096;
            let storage = Arc::new(Storage {
                magic: AtomicU32::new(0),
                version: AtomicU32::new(0),
                bytes: AtomicU64::new(0),
                capacity: AtomicU64::new(0),
                head: AtomicU64::new(0),
                tail: AtomicU64::new(0),
                eof: AtomicU32::new(0),
                producer_start: AtomicU32::new(0),
                consumer_start: AtomicU32::new(0),
                reserved: core::array::from_fn(|_| AtomicU8::new(0)),
                data: (0..bytes - 128).map(|_| AtomicU8::new(0)).collect(),
            });
            let make = || Self {
                storage: storage.clone(),
                fail_notify: Arc::new(AtomicBool::new(false)),
                wait_timeout: Arc::new(AtomicBool::new(false)),
                closed: Arc::new(AtomicBool::new(false)),
                notifications: Arc::new(AtomicUsize::new(0)),
                fail_close: false,
                invited: false,
                publish_on_head: Arc::new(AtomicBool::new(false)),
            };
            (make(), make())
        }
        fn a32(&self, offset: usize) -> &AtomicU32 {
            match offset {
                0 => &self.storage.magic,
                4 => &self.storage.version,
                EOF => &self.storage.eof,
                PRODUCER_START => &self.storage.producer_start,
                CONSUMER_START => &self.storage.consumer_start,
                _ => panic!("invalid control offset"),
            }
        }
        fn a64(&self, offset: usize) -> &AtomicU64 {
            match offset {
                8 => &self.storage.bytes,
                0x10 => &self.storage.capacity,
                HEAD => &self.storage.head,
                TAIL => &self.storage.tail,
                _ => panic!("invalid control offset"),
            }
        }
        fn byte(&self, offset: usize) -> &AtomicU8 {
            if offset < 128 {
                &self.storage.reserved[offset - 0x34]
            } else {
                &self.storage.data[offset - 128]
            }
        }
    }
    impl Transport for Host {
        fn bytes(&self) -> usize {
            self.storage.data.len() + 128
        }
        fn load32(&self, offset: usize, order: Ordering) -> u32 {
            assert!(!self.closed.load(Ordering::Relaxed));
            self.a32(offset).load(order)
        }
        fn load64(&self, offset: usize, order: Ordering) -> u64 {
            assert!(!self.closed.load(Ordering::Relaxed));
            if offset == HEAD && self.publish_on_head.swap(false, Ordering::Relaxed) {
                self.storage.producer_start.store(1, Ordering::Release);
                self.storage.data[0].store(7, Ordering::Relaxed);
                self.storage.head.store(1, Ordering::Release);
            }
            self.a64(offset).load(order)
        }
        fn store32(&self, offset: usize, value: u32, order: Ordering) {
            self.a32(offset).store(value, order);
        }
        fn store64(&self, offset: usize, value: u64, order: Ordering) {
            self.a64(offset).store(value, order);
        }
        fn write(&self, offset: usize, input: &[u8]) {
            assert!(!self.closed.load(Ordering::Relaxed));
            for (i, b) in input.iter().enumerate() {
                self.byte(offset + i).store(*b, Ordering::Relaxed);
            }
        }
        fn read(&self, offset: usize, output: &mut [u8]) {
            assert!(!self.closed.load(Ordering::Relaxed));
            for (i, b) in output.iter_mut().enumerate() {
                *b = self.byte(offset + i).load(Ordering::Relaxed);
            }
        }
        fn notify(&self) -> Result<(), SystemCallError> {
            self.notifications.fetch_add(1, Ordering::Relaxed);
            if self.fail_notify.load(Ordering::Relaxed) {
                Err(SystemCallError::ObjectClosed)
            } else if self.invited {
                Err(SystemCallError::ObjectNotAvailable)
            } else {
                Ok(())
            }
        }
        fn acknowledge(&self) -> Result<(), SystemCallError> {
            Ok(())
        }
        fn wait(&self, _deadline: Deadline) -> Result<Option<bool>, SystemCallError> {
            std::thread::yield_now();
            Ok(if self.wait_timeout.load(Ordering::Relaxed) {
                None
            } else {
                Some(false)
            })
        }
        fn close(&mut self) -> Result<(), SystemCallError> {
            if self.fail_close {
                Err(SystemCallError::ObjectBusy)
            } else {
                self.closed.store(true, Ordering::Relaxed);
                Ok(())
            }
        }
    }

    fn pair(pages: usize) -> (ProducerCore<Host>, ConsumerCore<Host>) {
        let (p, c) = Host::pair(pages);
        (
            ProducerCore::new(p, true).unwrap(),
            ConsumerCore::new(c, false).unwrap(),
        )
    }

    fn consumer_creator_pair(pages: usize) -> (ProducerCore<Host>, ConsumerCore<Host>) {
        let (p, c) = Host::pair(pages);
        let consumer = ConsumerCore::new(c, true).unwrap();
        (ProducerCore::new(p, false).unwrap(), consumer)
    }
    #[test]
    fn empty_full_and_split_copies() {
        for pages in [1, 2, 3, 512] {
            let (mut p, mut c) = pair(pages);
            let cap = p.channel.capacity;
            assert_eq!(p.writable().unwrap(), cap);
            assert_eq!(c.readable().unwrap(), 0);
            let input: Vec<u8> = (0..cap).map(|i| (i % 251) as u8).collect();
            assert_eq!(p.write(&input).unwrap(), cap);
            assert_eq!(p.writable().unwrap(), 0);
            let mut output = vec![0; cap - 3];
            assert_eq!(c.read(&mut output).unwrap(), cap - 3);
            assert_eq!(output, input[..cap - 3]);
            let more = [11, 12, 13, 14, 15, 16, 17];
            assert_eq!(p.write(&more).unwrap(), 7);
            let mut output = [0; 10];
            assert_eq!(c.read(&mut output).unwrap(), 10);
            assert_eq!(&output[..3], &input[cap - 3..]);
            assert_eq!(&output[3..], &more);
        }
    }

    #[test]
    fn integer_wrap_preserves_physical_cursor() {
        for pages in [1, 2, 3, 7] {
            let (mut p, mut c) = pair(pages);
            let cap = p.channel.capacity;
            let count = u64::MAX - 11;
            p.head = count;
            p.tail_shadow = count;
            c.tail = count;
            c.head_shadow = count;
            let cursor = (count as u128 % cap as u128) as usize;
            p.cursor = cursor;
            c.cursor = cursor;
            p.channel.transport.store64(HEAD, count, Ordering::Release);
            c.channel.transport.store64(TAIL, count, Ordering::Release);
            let payload: Vec<u8> = (0..cap * 3 + 41).map(|i| (i % 251 + 1) as u8).collect();
            let mut result = Vec::new();
            let mut sent = 0;
            while sent < payload.len() {
                let n = p
                    .write(&payload[sent..(sent + 37).min(payload.len())])
                    .unwrap();
                sent += n;
                let mut output = [0; 29];
                let n = c.read(&mut output).unwrap();
                result.extend_from_slice(&output[..n]);
            }
            p.finish().unwrap();
            while !c.eof_reached().unwrap() {
                let mut output = [0; 29];
                let n = c.read(&mut output).unwrap();
                result.extend_from_slice(&output[..n]);
            }
            assert_eq!(result, payload);
        }
    }

    #[test]
    fn eof_is_final_and_cannot_recede() {
        let (mut p, mut c) = pair(3);
        p.write(b"hello").unwrap();
        p.finish().unwrap();
        assert!(!c.eof_reached().unwrap());
        let mut out = [0; 5];
        c.read(&mut out).unwrap();
        assert!(c.eof_reached().unwrap());
        p.channel.transport.store32(EOF, 0, Ordering::Release);
        assert_eq!(c.eof_reached().unwrap_err().error, RunnelError::Broken);
        assert_eq!(c.readable().unwrap_err().error, RunnelError::Broken);
    }

    #[test]
    fn notify_failure_reports_irrevocable_progress() {
        let (mut p, _c) = pair(1);
        p.channel
            .transport
            .fail_notify
            .store(true, Ordering::Relaxed);
        p.channel.transport.fail_close = true;
        let error = p.write_all(b"payload", Deadline::INFINITE).unwrap_err();
        assert_eq!(error.completed, 7);
        assert_eq!(error.error, RunnelError::Closed);
        assert!(!p.channel.transport.closed.load(Ordering::Relaxed));
        assert_eq!(p.channel.close(), Err(SystemCallError::ObjectBusy));
        assert_eq!(p.head, 7);
        assert_eq!(p.write(b"again").unwrap_err().completed, 0);
    }

    #[test]
    fn absolute_wait_timeout_preserves_progress_and_role() {
        let (mut p, mut c) = pair(1);
        let capacity = p.channel.capacity;
        let input = vec![7; capacity + 1];
        p.channel
            .transport
            .wait_timeout
            .store(true, Ordering::Relaxed);
        let error = p.write_all(&input, Deadline::at(123)).unwrap_err();
        assert_eq!(error.error, RunnelError::TimedOut);
        assert_eq!(error.completed, capacity);
        assert!(p.channel.terminal.is_none());
        assert!(!p.channel.transport.closed.load(Ordering::Relaxed));
        let mut first = [0; 1];
        assert_eq!(c.read(&mut first).unwrap(), 1);
        assert_eq!(p.write(&input[capacity..]).unwrap(), 1);

        let (mut p, mut c) = pair(1);
        p.write(&[3]).unwrap();
        c.channel
            .transport
            .wait_timeout
            .store(true, Ordering::Relaxed);
        let mut output = [0; 2];
        let error = c
            .read_exact_or_eof(&mut output, Deadline::at(123))
            .unwrap_err();
        assert_eq!(error.error, RunnelError::TimedOut);
        assert_eq!(error.completed, 1);
        assert_eq!(output[0], 3);
        assert!(c.channel.terminal.is_none());
        p.write(&[4]).unwrap();
        assert_eq!(c.read(&mut output[1..]).unwrap(), 1);
        assert_eq!(output, [3, 4]);
    }

    #[test]
    fn geometry_shadow_and_invalid_progress() {
        let (mut p, mut c) = pair(3);
        p.channel
            .transport
            .store64(0x10, u64::MAX, Ordering::Relaxed);
        p.write(b"ok").unwrap();
        let mut out = [0; 2];
        c.read(&mut out).unwrap();
        assert_eq!(&out, b"ok");
        p.channel.transport.store64(
            HEAD,
            p.head + p.channel.capacity as u64 + 1,
            Ordering::Release,
        );
        assert_eq!(c.readable().unwrap_err().error, RunnelError::Broken);
    }

    #[test]
    fn bad_format_and_role_reconstruction_are_rejected() {
        let (p, c) = Host::pair(1);
        let producer = ProducerCore::new(p, true).unwrap();
        producer.channel.transport.store32(4, 99, Ordering::Relaxed);
        assert!(matches!(
            ConsumerCore::new(c, false),
            Err(InitFailure {
                error: RunnelError::BadFormat,
                ..
            })
        ));
        let (p, c) = Host::pair(1);
        let mut producer = ProducerCore::new(p, true).unwrap();
        producer.write(b"x").unwrap();
        assert!(matches!(
            ProducerCore::new(c, false),
            Err(InitFailure {
                error: RunnelError::Broken,
                ..
            })
        ));
    }

    #[test]
    fn initialization_failure_returns_unclosed_transport() {
        let (p, c) = Host::pair(1);
        let producer = ProducerCore::new(p, true).unwrap();
        producer.channel.transport.store32(4, 99, Ordering::Relaxed);
        let failure = ConsumerCore::new(c, false)
            .err()
            .expect("bad header accepted");
        assert_eq!(failure.error, RunnelError::BadFormat);
        assert!(!failure.owner.closed.load(Ordering::Relaxed));
        let mut owner = failure.owner;
        owner.fail_close = true;
        assert_eq!(owner.close(), Err(SystemCallError::ObjectBusy));
        assert!(!owner.closed.load(Ordering::Relaxed));
        owner.fail_close = false;
        assert_eq!(owner.close(), Ok(()));
    }

    #[test]
    fn explicit_close_failure_retains_transport() {
        let (mut p, _c) = pair(1);
        p.channel.transport.fail_close = true;
        assert_eq!(p.channel.close(), Err(SystemCallError::ObjectBusy));
        assert!(!p.channel.transport.closed.load(Ordering::Relaxed));
        p.channel.transport.fail_close = false;
        assert_eq!(p.channel.close(), Ok(()));
    }

    #[test]
    fn reader_notification_failure_preserves_consumed_progress() {
        let (mut p, mut c) = pair(1);
        p.write(b"hello").unwrap();
        c.channel
            .transport
            .fail_notify
            .store(true, Ordering::Relaxed);
        let mut out = [0; 10];
        let error = c
            .read_exact_or_eof(&mut out, Deadline::INFINITE)
            .unwrap_err();
        assert_eq!(error.completed, 5);
        assert_eq!(&out[..5], b"hello");
        assert_eq!(c.tail, 5);
    }
    #[test]
    fn invalid_peer_tail_and_own_cursor_are_terminal() {
        let (mut p, _c) = pair(1);
        p.channel.transport.store64(TAIL, 1, Ordering::Release);
        assert_eq!(p.writable().unwrap_err().error, RunnelError::Broken);
        let (mut p, _c) = pair(1);
        p.channel.transport.store64(HEAD, 1, Ordering::Relaxed);
        assert_eq!(p.write(b"x").unwrap_err().error, RunnelError::Broken);
    }
    #[test]
    fn eof_cannot_advance_after_observation() {
        let (mut p, mut c) = pair(1);
        p.finish().unwrap();
        assert!(c.eof_reached().unwrap());
        p.channel.transport.store64(HEAD, 1, Ordering::Release);
        assert_eq!(c.readable().unwrap_err().error, RunnelError::Broken);
    }
    #[test]
    fn acknowledgement_requires_rechecking_published_state() {
        let (mut p, mut c) = pair(1);
        assert_eq!(c.readable().unwrap(), 0);
        p.write(b"before-ack").unwrap();
        c.channel.acknowledge().unwrap();
        assert_eq!(c.readable().unwrap(), 10);
        let mut out = [0; 10];
        c.read(&mut out).unwrap();
        c.channel.acknowledge().unwrap();
        p.write(b"after-ack").unwrap();
        assert_eq!(c.readable().unwrap(), 9);
        assert!(p.channel.transport.notifications.load(Ordering::Relaxed) >= 2);
    }
    #[test]
    fn invited_peer_acquires_already_published_data_and_eof() {
        let (p, c) = Host::pair(3);
        let mut producer = ProducerCore::new(p, true).unwrap();
        producer.channel.transport.invited = true;
        producer
            .write_all(b"published-before-attach", Deadline::INFINITE)
            .unwrap();
        producer.finish().unwrap();
        let mut consumer = ConsumerCore::new(c, false).unwrap();
        let mut output = [0; 64];
        let count = consumer
            .read_exact_or_eof(&mut output, Deadline::INFINITE)
            .unwrap();
        assert_eq!(&output[..count], b"published-before-attach");
        assert!(consumer.eof_reached().unwrap());
    }
    #[test]
    fn zero_length_operations_cannot_revive_broken_channel() {
        let (mut p, mut c) = pair(1);
        p.write_all(&[], Deadline::INFINITE).unwrap();
        assert_eq!(c.read_exact_or_eof(&mut [], Deadline::INFINITE).unwrap(), 0);
        p.channel
            .transport
            .store64(HEAD, u64::MAX, Ordering::Release);
        assert_eq!(c.readable().unwrap_err().error, RunnelError::Broken);
        assert_eq!(
            c.read_exact_or_eof(&mut [], Deadline::INFINITE)
                .unwrap_err()
                .error,
            RunnelError::Broken
        );
        assert_eq!(p.writable().unwrap_err().error, RunnelError::Broken);
        assert_eq!(
            p.write_all(&[], Deadline::INFINITE).unwrap_err().error,
            RunnelError::Broken
        );
    }
    #[test]
    fn concurrent_blocking_roles_and_eof() {
        for pages in [1, 3, 7] {
            let (mut p, mut c) = pair(pages);
            let total = p.channel.capacity * 17 + 71;
            let input: Vec<u8> = (0..total).map(|i| (i % 251) as u8).collect();
            let mut output = vec![0; total + 1];
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    p.write_all(&input, Deadline::INFINITE).unwrap();
                    p.finish().unwrap();
                });
                assert_eq!(
                    c.read_exact_or_eof(&mut output, Deadline::INFINITE)
                        .unwrap(),
                    total
                );
            });
            assert_eq!(output[..total], input);
        }
    }

    #[test]
    fn producer_poll_reports_writable_eof_consumed_and_pending() {
        let (mut p, mut c) = pair(1);
        // 成功初始化对端后协议 Start 的 acquire 观察只报告一次。
        assert_eq!(p.poll(false).unwrap(), Some(ProducerReady::Started));
        assert!(p.start_observed);
        assert_eq!(
            p.poll(false).unwrap(),
            Some(ProducerReady::Writable {
                bytes: p.channel.capacity
            })
        );
        // 写满后无进展：acknowledge 重查仍无。
        let fill = vec![1u8; p.channel.capacity];
        assert_eq!(p.write(&fill).unwrap(), p.channel.capacity);
        assert_eq!(p.poll(false).unwrap(), None);
        // 消费腾空 + EOF 发布：全部消费条件成立。
        let mut drain = vec![0u8; p.channel.capacity];
        assert_eq!(c.read(&mut drain).unwrap(), p.channel.capacity);
        p.finish().unwrap();
        assert_eq!(p.poll(false).unwrap(), Some(ProducerReady::EofConsumed));
        // 终态观察直接失败。
        assert_eq!(p.poll(true).unwrap_err().error, RunnelError::Closed);
    }

    #[test]
    fn consumer_poll_reports_attach_readable_and_drained() {
        let (mut p, mut c) = consumer_creator_pair(1);
        // 空流首次观察协议 Start 后记录本地推进状态。
        assert!(c.includes_peer_started());
        assert!(!c.start_observed);
        assert_eq!(c.poll(false).unwrap(), Some(ConsumerReady::Started));
        assert!(!c.includes_peer_started());
        assert!(c.start_observed);
        assert!(c.start_observed);
        assert_eq!(c.poll(false).unwrap(), None);
        // 有数据优先于一切：Readable 携带字节数。
        let payload = [7u8; 16];
        assert_eq!(p.write(&payload).unwrap(), 16);
        assert_eq!(
            c.poll(false).unwrap(),
            Some(ConsumerReady::Readable {
                bytes: 16,
                peer_started: false,
            })
        );
        // 读尽 + EOF：EofDrained 成立。
        let mut drain = [0u8; 16];
        assert_eq!(c.read(&mut drain).unwrap(), 16);
        p.finish().unwrap();
        assert_eq!(c.poll(false).unwrap(), Some(ConsumerReady::EofDrained));
    }

    #[test]
    fn consumer_observes_data_and_protocol_start_together() {
        let (mut p, mut c) = consumer_creator_pair(1);
        assert_eq!(p.write(&[7]).unwrap(), 1);
        assert_eq!(
            c.poll(false).unwrap(),
            Some(ConsumerReady::Readable {
                bytes: 1,
                peer_started: true
            })
        );
        assert!(c.start_observed);
        assert_eq!(
            c.poll(false).unwrap(),
            Some(ConsumerReady::Readable {
                bytes: 1,
                peer_started: false
            })
        );
        let mut data = [0; 1];
        assert_eq!(c.read(&mut data).unwrap(), 1);
        assert_eq!(data, [7]);
    }

    #[test]
    fn start_published_during_first_data_probe_is_reported() {
        let (_producer, transport) = Host::pair(1);
        let mut consumer = ConsumerCore::new(transport, true).unwrap();
        consumer
            .channel
            .transport
            .publish_on_head
            .store(true, Ordering::Relaxed);
        assert_eq!(
            consumer.poll(false).unwrap(),
            Some(ConsumerReady::Readable {
                bytes: 1,
                peer_started: true
            })
        );
        assert_eq!(
            consumer.poll(false).unwrap(),
            Some(ConsumerReady::Readable {
                bytes: 1,
                peer_started: false
            })
        );
    }

    #[test]
    fn peer_start_cannot_revive_terminal_producer() {
        let (mut producer, _consumer) = pair(1);
        producer
            .channel
            .transport
            .fail_notify
            .store(true, Ordering::Relaxed);
        assert_eq!(producer.write(&[1]).unwrap_err().error, RunnelError::Closed);
        assert_eq!(producer.poll(false).unwrap_err().error, RunnelError::Closed);
    }

    #[test]
    fn attach_hint_cannot_substitute_for_protocol_start() {
        let (p, c) = Host::pair(1);
        let mut producer = ProducerCore::new(p, true).unwrap();
        assert_eq!(
            producer.poll(false).unwrap(),
            Some(ProducerReady::Writable {
                bytes: producer.channel.capacity
            })
        );
        assert!(!producer.start_observed);
        let _consumer = ConsumerCore::new(c, false).unwrap();
        assert_eq!(producer.poll(false).unwrap(), Some(ProducerReady::Started));
    }

    #[test]
    fn empty_eof_preserves_observed_protocol_start() {
        let (p, c) = Host::pair(8);
        let mut consumer = ConsumerCore::new(c, true).unwrap();
        let mut producer = ProducerCore::new(p, false).unwrap();
        producer.finish().unwrap();
        assert_eq!(
            consumer.poll(false).unwrap(),
            Some(ConsumerReady::EofDrained)
        );
        assert!(
            consumer.start_observed,
            "empty EOF must retain the protocol Start fact"
        );
    }

    #[test]
    fn consumer_late_attach_event_survives_drained_data() {
        let (mut p, mut c) = consumer_creator_pair(1);
        assert_eq!(p.write(&[9]).unwrap(), 1);
        let mut data = [0; 1];
        assert_eq!(c.read(&mut data).unwrap(), 1);
        assert!(c.includes_peer_started());
        assert_eq!(c.poll(false).unwrap(), Some(ConsumerReady::Started));
        assert!(c.start_observed);
        assert!(!c.includes_peer_started());
    }
}
