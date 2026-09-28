//! 外部共享映射的 RV64 访问边界；不建立指向不可信字节的普通 Rust 引用。

use core::marker::PhantomData;
use core::sync::atomic::Ordering;

#[derive(Clone, Copy)]
pub struct SharedMemory<'a> {
    base: usize,
    bytes: usize,
    lifetime: PhantomData<&'a ()>,
}

impl<'a> SharedMemory<'a> {
    /// # Safety
    /// 完整 RW 映射须在 'a 内存活；访问后端仅对正常一致性 RAM 成立。
    pub(crate) unsafe fn new(base: usize, bytes: usize) -> Self {
        Self {
            base,
            bytes,
            lifetime: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.bytes
    }
    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }

    fn address(&self, offset: usize, bytes: usize) -> usize {
        assert!(
            offset <= self.bytes && bytes <= self.bytes - offset,
            "shared memory access out of bounds"
        );
        self.base + offset
    }

    pub fn load_u32(&self, offset: usize, order: Ordering) -> u32 {
        let address = self.address(offset, 4);
        assert_eq!(address % 4, 0, "shared control field is misaligned");
        assert!(
            matches!(order, Ordering::Relaxed | Ordering::Acquire),
            "unsupported shared load ordering"
        );
        #[cfg(target_arch = "riscv64")]
        {
            let value: usize;
            // SAFETY: 有效完整映射、自然对齐；默认 asm 内存副作用阻止编译器跨越访问。
            unsafe {
                core::arch::asm!("lwu {value}, 0({address})", value = out(reg) value, address = in(reg) address);
            }
            if order == Ordering::Acquire {
                acquire();
            }
            value as u32
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            // SAFETY: host 模型只对同宽原子存储建立视图，不模拟外部非原子并发。
            unsafe { core::sync::atomic::AtomicU32::from_ptr(address as *mut u32).load(order) }
        }
    }

    pub fn load_u64(&self, offset: usize, order: Ordering) -> u64 {
        let address = self.address(offset, 8);
        assert_eq!(address % 8, 0, "shared control field is misaligned");
        assert!(
            matches!(order, Ordering::Relaxed | Ordering::Acquire),
            "unsupported shared load ordering"
        );
        #[cfg(target_arch = "riscv64")]
        {
            let value: u64;
            // SAFETY: 同 load_u32，自然对齐的 RV64 ld 为单次控制字段采样。
            unsafe {
                core::arch::asm!("ld {value}, 0({address})", value = out(reg) value, address = in(reg) address);
            }
            if order == Ordering::Acquire {
                acquire();
            }
            value
        }
        #[cfg(not(target_arch = "riscv64"))]
        unsafe {
            core::sync::atomic::AtomicU64::from_ptr(address as *mut u64).load(order)
        }
    }

    pub fn store_u32(&self, offset: usize, value: u32, order: Ordering) {
        let address = self.address(offset, 4);
        assert_eq!(address % 4, 0, "shared control field is misaligned");
        assert!(
            matches!(order, Ordering::Relaxed | Ordering::Release),
            "unsupported shared store ordering"
        );
        #[cfg(target_arch = "riscv64")]
        {
            if order == Ordering::Release {
                release();
            }
            // SAFETY: 同 load_u32；写入不可信区的值使用全部位型有效的整数。
            unsafe {
                core::arch::asm!("sw {value}, 0({address})", value = in(reg) value as usize, address = in(reg) address);
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        unsafe {
            core::sync::atomic::AtomicU32::from_ptr(address as *mut u32).store(value, order);
        }
    }

    pub fn store_u64(&self, offset: usize, value: u64, order: Ordering) {
        let address = self.address(offset, 8);
        assert_eq!(address % 8, 0, "shared control field is misaligned");
        assert!(
            matches!(order, Ordering::Relaxed | Ordering::Release),
            "unsupported shared store ordering"
        );
        #[cfg(target_arch = "riscv64")]
        {
            if order == Ordering::Release {
                release();
            }
            // SAFETY: 同 store_u32，访问宽度由 RV64 sd 明确指定。
            unsafe {
                core::arch::asm!("sd {value}, 0({address})", value = in(reg) value, address = in(reg) address);
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        unsafe {
            core::sync::atomic::AtomicU64::from_ptr(address as *mut u64).store(value, order);
        }
    }

    pub fn write(&self, offset: usize, input: &[u8]) {
        let address = self.address(offset, input.len());
        for (index, value) in input.iter().copied().enumerate() {
            #[cfg(target_arch = "riscv64")]
            // SAFETY: 地址在已验证范围内；本地输入与外部映射之间没有 Rust 共享引用。
            unsafe {
                core::arch::asm!("sb {value}, 0({address})", value = in(reg) value as usize, address = in(reg) address + index);
            }
            #[cfg(not(target_arch = "riscv64"))]
            unsafe {
                core::sync::atomic::AtomicU8::from_ptr((address + index) as *mut u8)
                    .store(value, Ordering::Relaxed);
            }
        }
    }

    pub fn read(&self, offset: usize, output: &mut [u8]) {
        let address = self.address(offset, output.len());
        for (index, value) in output.iter_mut().enumerate() {
            #[cfg(target_arch = "riscv64")]
            {
                let byte: usize;
                // SAFETY: 同 write；任意采样字节都可表示为 u8。
                unsafe {
                    core::arch::asm!("lbu {value}, 0({address})", value = out(reg) byte, address = in(reg) address + index);
                }
                *value = byte as u8;
            }
            #[cfg(not(target_arch = "riscv64"))]
            unsafe {
                *value = core::sync::atomic::AtomicU8::from_ptr((address + index) as *mut u8)
                    .load(Ordering::Relaxed);
            }
        }
    }
}

#[cfg(target_arch = "riscv64")]
fn acquire() {
    // SAFETY: 正常 RAM 的 acquire 排序；不得声明 nomem/pure。
    unsafe {
        core::arch::asm!("fence r, rw");
    }
}
#[cfg(target_arch = "riscv64")]
fn release() {
    // SAFETY: 发布前全部读写先于发布写。
    unsafe {
        core::arch::asm!("fence rw, w");
    }
}
