//! Runnel：共享 Tunnel 页上的单工 SPSC 字节流。
//! 控制字段使用原子 Acquire/Release；角色视图、对端游标校验与 Broken
//! 状态都封装在本库，调用方不能直接改写协议字段。

#![cfg_attr(not(test), no_std)]

use core::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use erhino_shared::{call::SystemCallError, object::Handle};

pub const PAGE_SIZE: usize = 4096;
pub const CTRL_SIZE: usize = 128;
pub const DATA_OFF: usize = CTRL_SIZE;
pub const CAP: usize = PAGE_SIZE - DATA_OFF;
/// little-endian 字节序列 `RNL1`。
pub const MAGIC: u32 = 0x314C_4E52;
pub const VERSION: u32 = 1;

const OFF_MAGIC: usize = 0x00;
const OFF_VERSION: usize = 0x04;
const OFF_HEAD: usize = 0x08;
const OFF_TAIL: usize = 0x0C;
const OFF_EOF: usize = 0x10;

#[inline]
pub fn used(head: u32, tail: u32) -> u32 {
    head.wrapping_sub(tail)
}

#[inline]
pub fn free(head: u32, tail: u32) -> usize {
    CAP - used(head, tail) as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnelError {
    BadMagic,
    Broken,
    Closed,
    Syscall(SystemCallError),
}

impl From<SystemCallError> for RunnelError {
    fn from(error: SystemCallError) -> Self {
        Self::Syscall(error)
    }
}

struct RawEndpoint {
    base: *mut u8,
    handle: Handle,
    broken: bool,
}

unsafe impl Send for RawEndpoint {}

impl RawEndpoint {
    unsafe fn creator(base: *mut u8, handle: Handle) -> Self {
        let mut endpoint = Self { base, handle, broken: false };
        endpoint.initialize();
        endpoint
    }

    unsafe fn attached(base: *mut u8, handle: Handle) -> Result<Self, RunnelError> {
        let endpoint = Self { base, handle, broken: false };
        if endpoint.atomic(OFF_MAGIC).load(Ordering::Acquire).to_le() != MAGIC
            || endpoint.atomic(OFF_VERSION).load(Ordering::Relaxed).to_le() != VERSION
        {
            return Err(RunnelError::BadMagic);
        }
        Ok(endpoint)
    }

    fn initialize(&mut self) {
        // 创建方独占尚未发布的零态页；控制区全部清零后用 release magic
        // 发布。attach 的 acquire magic 取得此前全部初始化写。
        unsafe { ptr::write_bytes(self.base, 0, CTRL_SIZE) };
        self.atomic(OFF_VERSION).store(VERSION.to_le(), Ordering::Relaxed);
        self.atomic(OFF_HEAD).store(0, Ordering::Relaxed);
        self.atomic(OFF_TAIL).store(0, Ordering::Relaxed);
        self.atomic(OFF_EOF).store(0, Ordering::Relaxed);
        self.atomic(OFF_MAGIC).store(MAGIC.to_le(), Ordering::Release);
    }

    fn atomic(&self, offset: usize) -> &AtomicU32 {
        debug_assert_eq!((self.base as usize + offset) % core::mem::align_of::<AtomicU32>(), 0);
        // SAFETY: Tunnel 映射覆盖整页，控制字段天然对齐且其全部并发访问
        // 都通过 AtomicU32；对象生命周期保证映射在视图存活期有效。
        unsafe { &*self.base.add(offset).cast::<AtomicU32>() }
    }

    fn ensure_usable(&self) -> Result<(), RunnelError> {
        if self.broken {
            Err(RunnelError::Broken)
        } else {
            Ok(())
        }
    }

    fn fail<T>(&mut self) -> Result<T, RunnelError> {
        self.broken = true;
        Err(RunnelError::Broken)
    }

    fn handle(&self) -> Handle {
        self.handle
    }

    fn copy_into_ring(&self, head: u32, source: &[u8], count: usize) {
        let offset = (head % CAP as u32) as usize;
        let first = (CAP - offset).min(count);
        // SAFETY: 已验证 used<=CAP，count<=free 且 count<=source.len()。
        unsafe {
            ptr::copy_nonoverlapping(source.as_ptr(), self.base.add(DATA_OFF + offset), first);
            if count > first {
                ptr::copy_nonoverlapping(
                    source.as_ptr().add(first),
                    self.base.add(DATA_OFF),
                    count - first,
                );
            }
        }
    }

    fn copy_from_ring(&self, tail: u32, output: &mut [u8], count: usize) {
        let offset = (tail % CAP as u32) as usize;
        let first = (CAP - offset).min(count);
        // SAFETY: 已验证 used<=CAP，count<=used 且 count<=output.len()。
        unsafe {
            ptr::copy_nonoverlapping(self.base.add(DATA_OFF + offset), output.as_mut_ptr(), first);
            if count > first {
                ptr::copy_nonoverlapping(
                    self.base.add(DATA_OFF),
                    output.as_mut_ptr().add(first),
                    count - first,
                );
            }
        }
    }
}

/// 唯一写 head/eof、只读 tail 的生产者视图。
pub struct Producer {
    raw: RawEndpoint,
    head: u32,
    tail_shadow: u32,
    eof: bool,
}

impl Producer {
    /// # Safety
    /// `base` 是刚由 TunnelCreate 映射、尚未发布的完整页。
    pub unsafe fn from_creator(base: *mut u8, handle: Handle) -> Self {
        Self {
            raw: unsafe { RawEndpoint::creator(base, handle) },
            head: 0,
            tail_shadow: 0,
            eof: false,
        }
    }

    /// # Safety
    /// `base` 是刚由 TunnelAttach 映射的完整页。
    pub unsafe fn from_attached(base: *mut u8, handle: Handle) -> Result<Self, RunnelError> {
        let mut raw = unsafe { RawEndpoint::attached(base, handle)? };
        let head = raw.atomic(OFF_HEAD).load(Ordering::Relaxed).to_le();
        let tail = raw.atomic(OFF_TAIL).load(Ordering::Acquire).to_le();
        let eof = raw.atomic(OFF_EOF).load(Ordering::Acquire).to_le();
        if used(head, tail) as usize > CAP || eof > 1 {
            return raw.fail();
        }
        Ok(Self { raw, head, tail_shadow: tail, eof: eof == 1 })
    }

    pub fn handle(&self) -> Handle {
        self.raw.handle()
    }

    fn refresh_tail(&mut self) -> Result<u32, RunnelError> {
        self.raw.ensure_usable()?;
        if self.raw.atomic(OFF_HEAD).load(Ordering::Relaxed).to_le() != self.head
            || self.raw.atomic(OFF_EOF).load(Ordering::Acquire).to_le() != u32::from(self.eof)
        {
            return self.raw.fail();
        }
        let tail = self.raw.atomic(OFF_TAIL).load(Ordering::Acquire).to_le();
        let outstanding = used(self.head, self.tail_shadow);
        let advanced = tail.wrapping_sub(self.tail_shadow);
        if advanced > outstanding || used(self.head, tail) as usize > CAP {
            return self.raw.fail();
        }
        self.tail_shadow = tail;
        Ok(tail)
    }

    pub fn writable(&mut self) -> Result<usize, RunnelError> {
        let tail = self.refresh_tail()?;
        Ok(free(self.head, tail))
    }

    pub fn write(&mut self, input: &[u8]) -> Result<usize, RunnelError> {
        if input.is_empty() {
            self.raw.ensure_usable()?;
            return Ok(0);
        }
        if self.eof {
            return self.raw.fail();
        }
        let tail = self.refresh_tail()?;
        let count = free(self.head, tail).min(input.len());
        self.raw.copy_into_ring(self.head, input, count);
        self.head = self.head.wrapping_add(count as u32);
        // 数据普通写先于 release head；消费者 acquire head 后才读数据。
        self.raw
            .atomic(OFF_HEAD)
            .store(self.head.to_le(), Ordering::Release);
        Ok(count)
    }

    pub fn set_eof(&mut self) -> Result<(), RunnelError> {
        self.raw.ensure_usable()?;
        if self.eof {
            return Ok(());
        }
        self.refresh_tail()?;
        self.eof = true;
        self.raw.atomic(OFF_EOF).store(1u32.to_le(), Ordering::Release);
        Ok(())
    }
}

/// 唯一写 tail、只读 head/eof 的消费者视图。
pub struct Consumer {
    raw: RawEndpoint,
    tail: u32,
    head_shadow: u32,
    eof_head: Option<u32>,
}

impl Consumer {
    /// # Safety
    /// `base` 是刚由 TunnelCreate 映射、尚未发布的完整页。
    pub unsafe fn from_creator(base: *mut u8, handle: Handle) -> Self {
        Self {
            raw: unsafe { RawEndpoint::creator(base, handle) },
            tail: 0,
            head_shadow: 0,
            eof_head: None,
        }
    }

    /// # Safety
    /// `base` 是刚由 TunnelAttach 映射的完整页。
    pub unsafe fn from_attached(base: *mut u8, handle: Handle) -> Result<Self, RunnelError> {
        let mut raw = unsafe { RawEndpoint::attached(base, handle)? };
        let tail = raw.atomic(OFF_TAIL).load(Ordering::Relaxed).to_le();
        let eof = raw.atomic(OFF_EOF).load(Ordering::Acquire).to_le();
        let head = raw.atomic(OFF_HEAD).load(Ordering::Acquire).to_le();
        if used(head, tail) as usize > CAP || eof > 1 {
            return raw.fail();
        }
        Ok(Self {
            raw,
            tail,
            head_shadow: head,
            eof_head: (eof == 1).then_some(head),
        })
    }

    pub fn handle(&self) -> Handle {
        self.raw.handle()
    }

    fn accept_head(&mut self, head: u32) -> Result<u32, RunnelError> {
        if self.raw.atomic(OFF_TAIL).load(Ordering::Relaxed).to_le() != self.tail {
            return self.raw.fail();
        }
        let capacity = CAP as u32 - used(self.head_shadow, self.tail);
        let advanced = head.wrapping_sub(self.head_shadow);
        if advanced > capacity || used(head, self.tail) as usize > CAP {
            return self.raw.fail();
        }
        self.head_shadow = head;
        Ok(head)
    }

    fn refresh_head(&mut self) -> Result<(u32, bool), RunnelError> {
        self.raw.ensure_usable()?;
        let eof = self.raw.atomic(OFF_EOF).load(Ordering::Acquire).to_le();
        if eof > 1 {
            return self.raw.fail();
        }
        // eof 必须先于这一次 head 取得；观察到 EOF 后冻结最终 head。
        let head = self.raw.atomic(OFF_HEAD).load(Ordering::Acquire).to_le();
        let head = self.accept_head(head)?;
        if eof == 1 {
            match self.eof_head {
                Some(final_head) if final_head != head => return self.raw.fail(),
                None => self.eof_head = Some(head),
                _ => {}
            }
        }
        Ok((head, eof == 1))
    }

    pub fn readable(&mut self) -> Result<usize, RunnelError> {
        let (head, _) = self.refresh_head()?;
        Ok(used(head, self.tail) as usize)
    }

    pub fn read(&mut self, output: &mut [u8]) -> Result<usize, RunnelError> {
        if output.is_empty() {
            self.raw.ensure_usable()?;
            return Ok(0);
        }
        let (head, _) = self.refresh_head()?;
        let count = (used(head, self.tail) as usize).min(output.len());
        self.raw.copy_from_ring(self.tail, output, count);
        self.tail = self.tail.wrapping_add(count as u32);
        // 数据读取先于 release tail；生产者 acquire tail 后才覆写空间。
        self.raw
            .atomic(OFF_TAIL)
            .store(self.tail.to_le(), Ordering::Release);
        Ok(count)
    }

    /// 先 acquire eof，再 acquire head；观察到 EOF 后以冻结的最终 head 判排空。
    pub fn eof_reached(&mut self) -> Result<bool, RunnelError> {
        let (head, eof) = self.refresh_head()?;
        Ok(eof && head == self.tail)
    }
}

#[cfg(target_arch = "riscv64")]
pub mod blocking {
    use super::*;
    use erhino_shared::{
        object::ObjectSignals,
        wait::WaitItem,
    };
    use rinlib::ipc::{object, tunnel, wait};

    pub fn create_consumer(addr: usize) -> Result<(Consumer, Handle), RunnelError> {
        let pair = tunnel::create(addr)?;
        // SAFETY: TunnelCreate 刚建立完整映射，尚未向 peer 发布 Invitation。
        let consumer = unsafe { Consumer::from_creator(addr as *mut u8, pair.owner) };
        Ok((consumer, pair.peer))
    }

    pub fn create_producer(addr: usize) -> Result<(Producer, Handle), RunnelError> {
        let pair = tunnel::create(addr)?;
        // SAFETY: 同 create_consumer。
        let producer = unsafe { Producer::from_creator(addr as *mut u8, pair.owner) };
        Ok((producer, pair.peer))
    }

    pub fn attach_producer(invitation: Handle, addr: usize) -> Result<Producer, RunnelError> {
        let handle = tunnel::attach(invitation, addr)?;
        // SAFETY: TunnelAttach 刚建立完整映射。
        match unsafe { Producer::from_attached(addr as *mut u8, handle) } {
            Ok(producer) => Ok(producer),
            Err(error) => {
                let _ = object::close(handle);
                Err(error)
            }
        }
    }

    pub fn attach_consumer(invitation: Handle, addr: usize) -> Result<Consumer, RunnelError> {
        let handle = tunnel::attach(invitation, addr)?;
        // SAFETY: TunnelAttach 刚建立完整映射。
        match unsafe { Consumer::from_attached(addr as *mut u8, handle) } {
            Ok(consumer) => Ok(consumer),
            Err(error) => {
                let _ = object::close(handle);
                Err(error)
            }
        }
    }

    fn ring(handle: Handle) -> Result<(), RunnelError> {
        tunnel::notify(handle).map_err(|error| match error {
            SystemCallError::ObjectClosed => RunnelError::Closed,
            _ => RunnelError::Syscall(error),
        })
    }

    fn acknowledge(handle: Handle) -> Result<(), RunnelError> {
        tunnel::acknowledge_data(handle)?;
        Ok(())
    }

    fn wait_event(handle: Handle) -> Result<(), RunnelError> {
        let result = wait::wait_many(&[WaitItem::new(
            handle,
            ObjectSignals::DATA | ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED,
            0,
        )])?;
        if result
            .observed
            .intersects(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED)
        {
            Err(RunnelError::Closed)
        } else {
            Ok(())
        }
    }

    impl Producer {
        pub fn close(self) -> Result<(), RunnelError> {
            object::close(self.handle())?;
            Ok(())
        }

        pub fn write_all(&mut self, mut input: &[u8]) -> Result<(), RunnelError> {
            while !input.is_empty() {
                let count = self.write(input)?;
                if count != 0 {
                    input = &input[count..];
                    ring(self.handle())?;
                    continue;
                }
                acknowledge(self.handle())?;
                if self.writable()? == 0 {
                    wait_event(self.handle())?;
                }
            }
            Ok(())
        }

        pub fn finish(&mut self) -> Result<(), RunnelError> {
            self.set_eof()?;
            ring(self.handle())
        }
    }

    impl Consumer {
        pub fn close(self) -> Result<(), RunnelError> {
            object::close(self.handle())?;
            Ok(())
        }

        pub fn read_exact_or_eof(&mut self, output: &mut [u8]) -> Result<usize, RunnelError> {
            let mut total = 0;
            loop {
                let count = self.read(&mut output[total..])?;
                total += count;
                if count != 0 {
                    ring(self.handle())?;
                }
                if total == output.len() || self.eof_reached()? {
                    return Ok(total);
                }
                if self.readable()? == 0 {
                    acknowledge(self.handle())?;
                    if self.readable()? == 0 && !self.eof_reached()? {
                        wait_event(self.handle())?;
                    }
                }
            }
        }
    }
}

const _: () = {
    assert!(CTRL_SIZE >= OFF_EOF + core::mem::size_of::<u32>());
    assert!(CAP > 0);
    assert!(OFF_MAGIC % core::mem::align_of::<AtomicU32>() == 0);
    assert!(OFF_VERSION % core::mem::align_of::<AtomicU32>() == 0);
    assert!(OFF_HEAD % core::mem::align_of::<AtomicU32>() == 0);
    assert!(OFF_TAIL % core::mem::align_of::<AtomicU32>() == 0);
    assert!(OFF_EOF % core::mem::align_of::<AtomicU32>() == 0);
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{boxed::Box, vec};

    #[repr(align(4096))]
    struct Page([u8; PAGE_SIZE]);

    struct Pair {
        _page: Box<Page>,
        producer: Producer,
        consumer: Consumer,
    }

    impl Pair {
        fn new() -> Self {
            let mut page = Box::new(Page([0u8; PAGE_SIZE]));
            let base = page.0.as_mut_ptr();
            // SAFETY: page 覆盖完整布局且由 Pair 保持存活。
            let consumer = unsafe { Consumer::from_creator(base, Handle::from_raw(1)) };
            let producer = unsafe { Producer::from_attached(base, Handle::from_raw(2)) }.unwrap();
            assert_eq!(&page.0[..4], b"RNL1");
            Self { _page: page, producer, consumer }
        }
    }

    #[test]
    fn cursor_wraps_at_u32_boundary() {
        let tail = u32::MAX - 10;
        let head = u32::MAX - 2;
        assert_eq!(used(head, tail), 8);
        assert_eq!(used(2, tail), 13);
    }

    #[test]
    fn empty_and_full_boundaries() {
        let mut pair = Pair::new();
        assert_eq!(pair.producer.writable().unwrap(), CAP);
        assert_eq!(pair.consumer.readable().unwrap(), 0);
        let all = [0xABu8; CAP];
        assert_eq!(pair.producer.write(&all).unwrap(), CAP);
        assert_eq!(pair.producer.writable().unwrap(), 0);
        assert_eq!(pair.consumer.readable().unwrap(), CAP);
        let mut output = vec![0u8; CAP];
        assert_eq!(pair.consumer.read(&mut output).unwrap(), CAP);
        assert!(output.iter().all(|&byte| byte == 0xAB));
    }

    #[test]
    fn wrap_around_split_copy() {
        let mut pair = Pair::new();
        let warm = [0u8; CAP - 3];
        assert_eq!(pair.producer.write(&warm).unwrap(), CAP - 3);
        let mut drain = [0u8; CAP - 3];
        assert_eq!(pair.consumer.read(&mut drain).unwrap(), CAP - 3);
        let payload = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(pair.producer.write(&payload).unwrap(), payload.len());
        let mut output = [0u8; 10];
        assert_eq!(pair.consumer.read(&mut output).unwrap(), output.len());
        assert_eq!(output, payload);
    }

    #[test]
    fn eof_is_visible_only_after_drain() {
        let mut pair = Pair::new();
        assert_eq!(pair.producer.write(b"hi").unwrap(), 2);
        pair.producer.set_eof().unwrap();
        assert!(!pair.consumer.eof_reached().unwrap());
        let mut output = [0u8; 4];
        assert_eq!(pair.consumer.read(&mut output).unwrap(), 2);
        assert!(pair.consumer.eof_reached().unwrap());
    }

    #[test]
    fn head_cannot_advance_after_eof_publication() {
        let mut pair = Pair::new();
        pair.producer.set_eof().unwrap();
        assert!(pair.consumer.eof_reached().unwrap());
        pair.consumer
            .raw
            .atomic(OFF_HEAD)
            .store(1u32.to_le(), Ordering::Release);
        assert_eq!(pair.consumer.eof_reached(), Err(RunnelError::Broken));
    }

    #[test]
    fn impossible_peer_cursor_breaks_endpoint_permanently() {
        let mut pair = Pair::new();
        pair.consumer
            .raw
            .atomic(OFF_HEAD)
            .store((CAP as u32 + 1).to_le(), Ordering::Release);
        assert_eq!(pair.consumer.readable(), Err(RunnelError::Broken));
        assert_eq!(pair.consumer.readable(), Err(RunnelError::Broken));
    }

    #[test]
    fn zero_length_io_is_noop() {
        let mut pair = Pair::new();
        assert_eq!(pair.producer.write(&[]).unwrap(), 0);
        assert_eq!(pair.consumer.read(&mut []).unwrap(), 0);
    }

    #[test]
    fn concurrent_roles_survive_many_wraps() {
        const TOTAL: usize = CAP * 257 + 113;
        let mut page = Box::new(Page([0u8; PAGE_SIZE]));
        let base = page.0.as_mut_ptr();
        // SAFETY: page 在 scoped threads 完成前保持存活。
        let mut consumer = unsafe { Consumer::from_creator(base, Handle::from_raw(1)) };
        let mut producer = unsafe { Producer::from_attached(base, Handle::from_raw(2)) }.unwrap();
        let expected: Vec<u8> = (0..TOTAL).map(|index| (index % 251 + 1) as u8).collect();
        let mut output = vec![0u8; TOTAL];
        std::thread::scope(|scope| {
            let input = &expected;
            let writer = scope.spawn(move || {
                let mut offset = 0;
                while offset < input.len() {
                    let count = producer.write(&input[offset..]).unwrap();
                    if count == 0 {
                        std::thread::yield_now();
                    } else {
                        offset += count;
                    }
                }
                producer.set_eof().unwrap();
            });

            let mut offset = 0;
            while offset < output.len() {
                let count = consumer.read(&mut output[offset..]).unwrap();
                if count == 0 {
                    if consumer.eof_reached().unwrap() {
                        break;
                    }
                    std::thread::yield_now();
                } else {
                    offset += count;
                }
            }
            writer.join().unwrap();
            assert_eq!(offset, TOTAL);
            assert!(consumer.eof_reached().unwrap());
        });
        assert_eq!(output, expected);
    }
}
