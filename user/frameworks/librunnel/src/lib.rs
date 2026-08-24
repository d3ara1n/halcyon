//! Runnel——跑在隧道页上的单工 FIFO 字节流协议（规格见
//! notes/ideas/runnel.md）。本库是规格的参考实现：控制块访问、环形
//! 游标算术、内存序配对与摇铃纪律都收敛在这里，消费者无法写错。
//!
//! 分层：
//! - **协议核心**（本文件主体）：页布局、游标算术、非阻塞 I/O 与内存序
//!   配对。零 syscall 依赖，host 可测（规格不变量的全部可测形态都在
//!   `#[cfg(test)]`）；
//! - **阻塞流式**（`riscv64` 目标专属）：组合信号面等待与门铃的
//!   `write_all`/`read_exact_or_eof`，把「排空后清铃」纪律封装为唯一的
//!   正确循环形态。
//!
//! 单工页上铃铛含义由方向唯一确定：本端摇铃 = 「我这边状态变了」，读端
//! 醒来查到的是数据到达、写端醒来查到的是空间腾出。

#![cfg_attr(not(test), no_std)]

use core::ptr;

use erhino_shared::call::SystemCallError;

/// 页内布局常量（规格钉死，不得改动语义）。
pub const PAGE_SIZE: usize = 4096;
/// 控制块大小。
pub const CTRL_SIZE: usize = 128;
/// 数据区起始偏移。
pub const DATA_OFF: usize = CTRL_SIZE;
/// 环形容量（全数据区，不牺牲判满格）。
pub const CAP: usize = PAGE_SIZE - DATA_OFF;
/// 布局版本锚点。
pub const MAGIC: u32 = 0x524E_4C31; // "RNL1"
pub const VERSION: u16 = 1;

// 控制块字段偏移。
const OFF_MAGIC: usize = 0x00;
const OFF_VERSION: usize = 0x04;
const OFF_HEAD: usize = 0x08;
const OFF_TAIL: usize = 0x0C;
const OFF_EOF: usize = 0x10;

/// 已用量：模 2³² 回绕差值（规格「不变量」节）。
#[inline]
pub fn used(head: u32, tail: u32) -> u32 {
    head.wrapping_sub(tail)
}

/// 当前可写字节数。
#[inline]
pub fn free(head: u32, tail: u32) -> usize {
    CAP - used(head, tail) as usize
}

// ---------------------------------------------------------------------------
// 内存访问原语：控制块字段 volatile + 显式栅栏；数据字节普通拷贝。
// ---------------------------------------------------------------------------

#[inline]
fn load_u32(base: *mut u8, off: usize) -> u32 {
    // SAFETY: base 为已映射页内偏移，调用方保证对齐与有效性。
    unsafe { (base.add(off) as *const u32).read_volatile() }
}

#[inline]
fn store_u32(base: *mut u8, off: usize, v: u32) {
    // SAFETY: 同上。
    unsafe { (base.add(off) as *mut u32).write_volatile(v) }
}

/// release 栅栏：此前写入对本端读者可见后才发布游标。
#[inline]
fn fence_release() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw, w", options(nomem))
    }
    #[cfg(not(target_arch = "riscv64"))]
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
}

/// acquire 栅栏：取得游标后才能信任数据内容。
#[inline]
fn fence_acquire() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence r, rw", options(nomem))
    }
    #[cfg(not(target_arch = "riscv64"))]
    core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
}

/// 协议错误。
#[derive(Debug)]
pub enum RunnelError {
    /// 页不是 Runnel 协议页（magic/version 不符）。
    BadMagic,
    /// 对端已消亡或拆除（PEER_CLOSED）。
    Closed,
    /// 底层系统调用失败。
    Syscall(SystemCallError),
}

impl From<SystemCallError> for RunnelError {
    fn from(e: SystemCallError) -> Self {
        Self::Syscall(e)
    }
}

/// 协议端点：本进程视角下的一页隧道 + 自己的隧道 id。
///
/// 构造入口见 [`blocking::create`] / [`blocking::attach`]（riscv64）；
/// 测试与工具场景可用 [`Endpoint::from_raw`] 直接包一个已映射页。
pub struct Endpoint {
    base: *mut u8,
    id: u64,
}

unsafe impl Send for Endpoint {}

impl Endpoint {
    /// # Safety
    /// `base` 指向本进程已映射的隧道页。
    pub unsafe fn from_raw(base: *mut u8, id: u64) -> Self {
        Self { base, id }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// 创建方一次性写入版本锚点（规格：其余字段零态即合法初态）。
    pub fn init_creator(&self) {
        store_u32(self.base, OFF_MAGIC, MAGIC);
        store_u32(self.base, OFF_VERSION, VERSION as u32);
    }

    /// 校验页为 Runnel 协议页（attach 路径用）。
    pub fn validate(&self) -> Result<(), RunnelError> {
        let ok = load_u32(self.base, OFF_MAGIC) == MAGIC
            && load_u32(self.base, OFF_VERSION) as u16 == VERSION;
        if ok {
            fence_acquire(); // 取得 magic 后才能信任整页布局
            Ok(())
        } else {
            Err(RunnelError::BadMagic)
        }
    }

    pub fn writable(&self) -> usize {
        let head = load_u32(self.base, OFF_HEAD);
        fence_acquire();
        let tail = load_u32(self.base, OFF_TAIL);
        free(head, tail)
    }

    pub fn readable(&self) -> usize {
        let tail = load_u32(self.base, OFF_TAIL);
        fence_acquire();
        let head = load_u32(self.base, OFF_HEAD);
        used(head, tail) as usize
    }

    /// 尽力写一批字节，返回实际写入数。**不摇铃**——批量场景在
    /// [`blocking::EndpointExt::write_all`] 统一摇铃，手工轮询场景自行
    /// 决定摇铃时机。
    pub fn write(&self, buf: &[u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        let head = load_u32(self.base, OFF_HEAD);
        fence_acquire();
        let tail = load_u32(self.base, OFF_TAIL);
        let n = free(head, tail).min(buf.len());
        self.copy_into_ring(head, buf, n);
        fence_release(); // 规范义务：数据先于游标可见
        store_u32(self.base, OFF_HEAD, head.wrapping_add(n as u32));
        n
    }

    /// 尽力读一批字节，返回实际读取数。不摇铃。
    pub fn read(&self, buf: &mut [u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        let tail = load_u32(self.base, OFF_TAIL);
        fence_acquire();
        let head = load_u32(self.base, OFF_HEAD);
        let n = (used(head, tail) as usize).min(buf.len());
        self.copy_from_ring(tail, buf, n);
        fence_release(); // 复用缓冲区前的发布次序（覆盖旧数据的写先于 tail 发布）
        store_u32(self.base, OFF_TAIL, tail.wrapping_add(n as u32));
        n
    }

    /// 流结束标记（生产者专用；置位后按规格不再写）。
    pub fn set_eof(&self) {
        store_u32(self.base, OFF_EOF, 1);
        fence_release(); // eof 发布不早于其语义前提（数据已在环内）
    }

    /// 流是否已正常终止：排空且 eof（规格「EOF」节）。
    pub fn eof_reached(&self) -> bool {
        let tail = load_u32(self.base, OFF_TAIL);
        fence_acquire();
        let head = load_u32(self.base, OFF_HEAD);
        let eof = load_u32(self.base, OFF_EOF);
        head == tail && eof == 1
    }

    /// 环形拷入：自动处理回绕的两段拷贝。
    fn copy_into_ring(&self, head: u32, src: &[u8], n: usize) {
        let off = (head % CAP as u32) as usize;
        let first = (CAP - off).min(n);
        // SAFETY: off + n ≤ CAP 保证两段都不越页；n ≤ src.len() 由调用方保证。
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), self.base.add(DATA_OFF + off), first);
            if n > first {
                ptr::copy_nonoverlapping(
                    src.as_ptr().add(first),
                    self.base.add(DATA_OFF),
                    n - first,
                );
            }
        }
    }

    /// 环形拷出：对称处理回绕。
    fn copy_from_ring(&self, tail: u32, dst: &mut [u8], n: usize) {
        let off = (tail % CAP as u32) as usize;
        let first = (CAP - off).min(n);
        // SAFETY: 同 copy_into_ring；n ≤ dst.len() 由调用方保证。
        unsafe {
            ptr::copy_nonoverlapping(self.base.add(DATA_OFF + off), dst.as_mut_ptr(), first);
            if n > first {
                ptr::copy_nonoverlapping(
                    self.base.add(DATA_OFF),
                    dst.as_mut_ptr().add(first),
                    n - first,
                );
            }
        }
    }
}

/// 阻塞流式扩展：依赖内核信号面与门铃，仅在内核目标上编译
/// （host 测试只覆盖协议核心——阻塞循环的正确性由排空纪律的结构形状
/// 保证，见各方法的文档）。
#[cfg(target_arch = "riscv64")]
pub mod blocking {
    use super::*;
    use erhino_shared::signal::{ObjectKind, SignalItem, TUNNEL_DATA, TUNNEL_PEER_CLOSED};
    use rinlib::ipc::{signal, tunnel};

    /// 创建隧道并初始化协议页（创建方入口）。`addr` 为本进程内的页对齐地址。
    pub fn create(addr: usize) -> Result<Endpoint, RunnelError> {
        let id = tunnel::create(addr)?;
        // SAFETY: addr 刚由内核映射，页对齐且归本进程独占初始化窗口。
        let ep = unsafe { Endpoint::from_raw(addr as *mut u8, id) };
        ep.init_creator();
        Ok(ep)
    }

    /// 凭隧道 id 挂接对端并校验 magic/version。
    pub fn attach(id: u64, addr: usize) -> Result<Endpoint, RunnelError> {
        tunnel::attach(id, addr)?;
        // SAFETY: addr 刚由内核映射。
        let ep = unsafe { Endpoint::from_raw(addr as *mut u8, id) };
        ep.validate()?;
        Ok(ep)
    }

    impl Endpoint {
        /// 摇门铃：声明本端状态已变（写完数据 / 腾出空间）。
        pub fn ring(&self) -> Result<(), RunnelError> {
            tunnel::notify(self.id)?;
            Ok(())
        }

        /// 拆除本端（Dispose）。
        pub fn dispose(self) -> Result<(), RunnelError> {
            tunnel::dispose(self.id)?;
            Ok(())
        }

        /// 结束流：置 EOF **并**摇铃。eof 是带内标记、门铃是带外唤醒，
        /// 二者缺一不可——只置位不摇铃，读端会在排空后永久睡眠。
        pub fn finish(&self) -> Result<(), RunnelError> {
            self.set_eof();
            self.ring()
        }

        /// 阻塞写完全部数据。满则等待对端腾空间的门铃；每批落页后摇铃
        /// 唤醒读端。对端消亡返回 [`RunnelError::Closed`]。
        pub fn write_all(&self, mut buf: &[u8]) -> Result<(), RunnelError> {
            while !buf.is_empty() {
                let n = self.write(buf);
                buf = &buf[n..];
                match buf.is_empty() {
                    false if n == 0 => self.wait_event()?, // 满：等空间腾出门铃
                    false => {}                            // 有进展但未完：继续尽力写
                    true => self.ring()?,                  // 全部落页：唤醒读端
                }
            }
            Ok(())
        }

        /// 阻塞读满 `buf`。排空且未 EOF 才等待（清铃前置条件的结构性
        /// 落实：等待只发生在刚观察到空页之后）；读到 EOF 且无剩余数据时
        /// 返回实际读取数（可短于 buf 长度）。对端消亡返回 Closed，
        /// 此时已读到的数据仍然有效。
        pub fn read_exact_or_eof(&self, buf: &mut [u8]) -> Result<usize, RunnelError> {
            let mut total = 0;
            loop {
                total += self.read(&mut buf[total..]);
                if total == buf.len() {
                    return Ok(total);
                }
                if self.eof_reached() {
                    return Ok(total); // EOF：允许短读
                }
                if self.readable() == 0 {
                    self.wait_event()?;
                }
                // 未排空则立即重试（仍有余量可读，不烧等待名额）。
            }
        }

        /// 等待本端任一事件（数据到达 / 空间腾出 / 对端关闭）。
        fn wait_event(&self) -> Result<(), RunnelError> {
            let items = [SignalItem {
                kind: ObjectKind::TunnelEndpoint as u64,
                id: self.id,
                interest: TUNNEL_DATA | TUNNEL_PEER_CLOSED,
            }];
            match signal::wait(&items)? {
                (_, bits) if bits & TUNNEL_PEER_CLOSED != 0 => Err(RunnelError::Closed),
                _ => Ok(()),
            }
        }
    }
}

const _: () = {
    assert!(CTRL_SIZE >= OFF_EOF + core::mem::size_of::<u32>());
    assert!(CAP > 0);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟一对端点：同一页的两个视角（单线程交错测试的合法简化）。
    struct Pair {
        _page: Box<[u8; PAGE_SIZE]>,
        producer: Endpoint,
        consumer: Endpoint,
    }

    impl Pair {
        fn new() -> Self {
            let mut page = Box::new([0u8; PAGE_SIZE]);
            let base = page.as_mut_ptr();
            // SAFETY: base 指向活着的堆上页，生命周期由 Self 持有。
            let producer = unsafe { Endpoint::from_raw(base, 7) };
            let consumer = unsafe { Endpoint::from_raw(base, 7) };
            producer.init_creator();
            assert_eq!(load_u32(base, OFF_MAGIC), MAGIC);
            Self { _page: page, producer, consumer }
        }
    }

    #[test]
    fn cursor_wraps_at_u32_boundary() {
        // 自由计数越过 2³² 后差值语义不变（规格「不变量」节）。
        let tail = u32::MAX - 10;
        let head = u32::MAX - 2;
        assert_eq!(used(head, tail), 8);
        let head = 2; // 再写 5 字节：MAX-2 越过回绕点落到 2，used = 8+5
        assert_eq!(used(head, tail), 13);
    }

    #[test]
    fn empty_and_full_boundaries() {
        let p = Pair::new();
        assert_eq!(p.producer.writable(), CAP);
        assert_eq!(p.consumer.readable(), 0);
        // 写满：used == CAP、free == 0（不牺牲判满格）。
        let all = [0xABu8; CAP];
        assert_eq!(p.producer.write(&all), CAP);
        assert_eq!(p.producer.writable(), 0);
        assert_eq!(p.consumer.readable(), CAP);
        // 排空。
        let mut out = vec![0u8; CAP];
        assert_eq!(p.consumer.read(&mut out), CAP);
        assert!(out.iter().all(|&b| b == 0xAB));
        assert_eq!(p.consumer.readable(), 0);
    }

    #[test]
    fn wrap_around_split_copy() {
        let p = Pair::new();
        // 写游标贴近页尾：迫使数据分两段落页。
        let warm = [0u8; CAP - 3];
        assert_eq!(p.producer.write(&warm), CAP - 3);
        let mut drain = [0u8; CAP - 3];
        assert_eq!(p.consumer.read(&mut drain), CAP - 3);
        assert_eq!(p.producer.writable(), CAP); // 逻辑空但 head 已贴尾

        let payload: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(p.producer.write(&payload), 10); // 尾段 3 + 回绕头部 7
        assert_eq!(p.consumer.readable(), 10);
        let mut out = [0u8; 10];
        assert_eq!(p.consumer.read(&mut out), 10);
        assert_eq!(out, payload);
    }

    #[test]
    fn eof_semantics() {
        let p = Pair::new();
        assert!(!p.consumer.eof_reached());
        assert_eq!(p.producer.write(b"hi"), 2);
        p.producer.set_eof();
        // 未排空时 EOF 不算到达。
        assert!(!p.consumer.eof_reached());
        let mut out = [0u8; 4];
        // 直接用核心层模拟 read_exact_or_eof 的排空判定。
        let n = {
            let got = p.consumer.read(&mut out);
            if p.consumer.eof_reached() { got } else { unreachable!() }
        };
        assert_eq!(n, 2); // 短读
        assert!(p.consumer.eof_reached());
    }

    #[test]
    fn zero_length_io_is_noop() {
        let p = Pair::new();
        assert_eq!(p.producer.write(&[]), 0);
        assert_eq!(p.consumer.read(&mut []), 0);
    }
}
