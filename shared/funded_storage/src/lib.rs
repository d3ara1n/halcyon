#![no_std]
#![feature(allocator_ext, box_into_inner)]

//! 固定布局的已付款存储。付款来源由调用者选择；存储 owner 保留唯一 charge。
//! 不隐式增长或缩容；替换只搬移现有元素，并将旧空块返给调用者锁外释放。

extern crate alloc;

use alloc::boxed::Box;
use core::{
    alloc::Layout,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    ptr,
};

pub trait Funding {
    type Charge;
    type Error;
    fn charge(&self, layout: Layout) -> Result<Self::Charge, Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum StorageError<E> {
    Layout,
    Funding(E),
    Allocation,
}

/// data 的初始化前缀与整个分配由同一 owner 持有；字段顺序保证分配先于 charge 释放。
pub struct Buffer<T, C> {
    data: Option<Box<[MaybeUninit<T>]>>,
    len: usize,
    charge: Option<C>,
}

impl<T, C> Buffer<T, C> {
    pub const fn new() -> Self {
        Self {
            data: None,
            len: 0,
            charge: None,
        }
    }

    /// 额度与分配先于领域提交；零字节布局不占存储额度。
    pub fn try_empty<F: Funding<Charge = C>>(
        capacity: usize,
        payer: &F,
    ) -> Result<Self, StorageError<F::Error>> {
        let layout = Layout::array::<T>(capacity).map_err(|_| StorageError::Layout)?;
        let charge = if layout.size() == 0 {
            None
        } else {
            Some(payer.charge(layout).map_err(StorageError::Funding)?)
        };
        let data =
            Box::<[T]>::try_new_uninit_slice(capacity).map_err(|_| StorageError::Allocation)?;
        Ok(Self {
            data: Some(data),
            len: 0,
            charge,
        })
    }

    pub fn capacity(&self) -> usize {
        self.data.as_ref().map_or(0, |data| data.len())
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 显式锁外准备追加容量。付款失败保留原块和值；成功后原块释放并退款。
    /// 两份块同时存活的峰值都须获准，不能拿旧块退款预支新分配。
    pub fn try_reserve<F: Funding<Charge = C>>(
        &mut self,
        additional: usize,
        payer: &F,
    ) -> Result<(), StorageError<F::Error>> {
        let needed = self
            .len
            .checked_add(additional)
            .ok_or(StorageError::Layout)?;
        if needed <= self.capacity() {
            return Ok(());
        }
        let capacity = self.capacity().checked_mul(2).unwrap_or(needed).max(needed);
        self.prepare_capacity(capacity, payer)
    }

    /// 显式锁外准备精确追加容量；已有足够容量时不分配或换付款人。
    pub fn try_reserve_exact<F: Funding<Charge = C>>(
        &mut self,
        additional: usize,
        payer: &F,
    ) -> Result<(), StorageError<F::Error>> {
        let needed = self
            .len
            .checked_add(additional)
            .ok_or(StorageError::Layout)?;
        if needed <= self.capacity() {
            return Ok(());
        }
        self.prepare_capacity(needed, payer)
    }

    fn prepare_capacity<F: Funding<Charge = C>>(
        &mut self,
        capacity: usize,
        payer: &F,
    ) -> Result<(), StorageError<F::Error>> {
        let mut prepared = Self::try_empty(capacity, payer)?;
        // 独占借用下 needed > 原 capacity >= len，新块容量在进入此方法前已证明。
        self.replace_storage(&mut prepared)
            .expect("prepared storage cannot hold initialized prefix");
        drop(prepared);
        Ok(())
    }

    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.len == self.capacity() {
            return Err(value);
        }
        self.data.as_mut().expect("nonempty capacity lost storage")[self.len].write(value);
        self.len += 1;
        Ok(())
    }
    /// 提交路径只能消费已经预付的容量。
    pub fn push(&mut self, value: T) {
        assert!(
            self.len < self.capacity(),
            "storage capacity was not prepared"
        );
        // 已复核容量；此处不构造丢弃 value 的错误分支。
        self.data.as_mut().expect("nonempty capacity lost storage")[self.len].write(value);
        self.len += 1;
    }
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: 原初始化前缀的最后一项只移动一次，缩短后的前缀不再包含它。
        Some(unsafe {
            self.data.as_mut().expect("initialized prefix lost storage")[self.len]
                .assume_init_read()
        })
    }
    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "storage removal index out of range");
        let data = self
            .data
            .as_mut()
            .expect("initialized prefix lost storage")
            .as_mut_ptr()
            .cast::<T>();
        // SAFETY: index..len 均已初始化；read 移出目标，重叠 copy 左移其余值。
        // 旧末尾的副本不再属于缩短后的初始化前缀，因此不会被重复析构。
        let value = unsafe {
            let value = data.add(index).read();
            ptr::copy(data.add(index + 1), data.add(index), self.len - index - 1);
            value
        };
        self.len -= 1;
        value
    }
    pub fn clear(&mut self) {
        if self.len == 0 || !core::mem::needs_drop::<T>() {
            self.len = 0;
            return;
        }
        let len = core::mem::replace(&mut self.len, 0);
        let data = self
            .data
            .as_mut()
            .expect("initialized prefix lost storage")
            .as_mut_ptr()
            .cast::<T>();
        // SAFETY: 仅析构唯一初始化前缀；标准 slice drop glue 在元素 panic 时也清理其余项。
        // len 已清零，存储块最终析构不会重复 drop 值。
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(data, len));
        }
    }

    /// 与逐值 mem::forget 等价：丢弃初始化前缀，不运行值析构。
    /// 调用者须已按领域真值解脱各值的资源责任；分配和费用仍由本 Buffer
    /// 持有并正常释放。用于已逐项清空的保留槽，避免最终再次遍历容量。
    pub fn forget_values(&mut self) {
        self.len = 0;
    }

    /// 只析构多余初始化项，实际保留容量及付款持续到换块或 Drop。
    pub fn truncate(&mut self, len: usize) {
        while self.len > len {
            drop(self.pop());
        }
    }

    /// prepared 必须为空；不足时双方原样保留。成功后 prepared 持旧空块。
    /// 不分配、退费或析构 T，付款可以来自不同 Pool。
    pub fn replace_storage(&mut self, prepared: &mut Self) -> Result<(), InsufficientCapacity> {
        assert!(prepared.is_empty(), "prepared storage contains values");
        if prepared.capacity() < self.len {
            return Err(InsufficientCapacity);
        }
        if self.len != 0 {
            let from = self
                .data
                .as_mut()
                .expect("initialized prefix lost storage")
                .as_mut_ptr();
            let to = prepared
                .data
                .as_mut()
                .expect("prepared capacity lost storage")
                .as_mut_ptr();
            // SAFETY: 非零大小时独立分配不重叠；零大小不访问字节且指针仍满足对齐。
            // 目标容量已校验。搬移位型后只目标拥有值，
            // 源 len 清零；MaybeUninit 不会析构遗留副本。普通借用阻止外部持有元素引用。
            unsafe {
                ptr::copy_nonoverlapping(from, to, self.len);
            }
        }
        prepared.len = core::mem::replace(&mut self.len, 0);
        core::mem::swap(self, prepared);
        Ok(())
    }
}

impl<T, C> Default for Buffer<T, C> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: core::fmt::Debug, C> core::fmt::Debug for Buffer<T, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
impl<T, C> Deref for Buffer<T, C> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        if self.len == 0 {
            return &[];
        }
        let data = self
            .data
            .as_ref()
            .expect("initialized prefix lost storage")
            .as_ptr()
            .cast::<T>();
        // SAFETY: 仅暴露初始化前缀；借用存储 owner 保证生命周期与分配一致。
        unsafe { core::slice::from_raw_parts(data, self.len) }
    }
}
impl<T, C> DerefMut for Buffer<T, C> {
    fn deref_mut(&mut self) -> &mut [T] {
        if self.len == 0 {
            return &mut [];
        }
        let data = self
            .data
            .as_mut()
            .expect("initialized prefix lost storage")
            .as_mut_ptr()
            .cast::<T>();
        // SAFETY: 独占 owner 借用，只暴露其初始化前缀；没有隐式 realloc 入口。
        unsafe { core::slice::from_raw_parts_mut(data, self.len) }
    }
}
impl<T, C> Drop for Buffer<T, C> {
    fn drop(&mut self) {
        self.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsufficientCapacity;

pub struct IntoIter<T, C> {
    data: Option<Box<[MaybeUninit<T>]>>,
    front: usize,
    back: usize,
    _charge: Option<C>,
}
impl<T, C> IntoIterator for Buffer<T, C> {
    type Item = T;
    type IntoIter = IntoIter<T, C>;
    fn into_iter(mut self) -> Self::IntoIter {
        IntoIter {
            data: self.data.take(),
            front: 0,
            back: core::mem::replace(&mut self.len, 0),
            _charge: self.charge.take(),
        }
    }
}
impl<T, C> Iterator for IntoIter<T, C> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        let index = self.front;
        self.front += 1;
        // SAFETY: front..back 是唯一初始化区间，移出后先推进 front，避免重复析构。
        Some(unsafe {
            self.data.as_mut().expect("iterator lost storage")[index].assume_init_read()
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.back - self.front;
        (len, Some(len))
    }
}
impl<T, C> DoubleEndedIterator for IntoIter<T, C> {
    fn next_back(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        // SAFETY: 原 front..back 区间最后一项已初始化，只移动一次。
        Some(unsafe {
            self.data.as_mut().expect("iterator lost storage")[self.back].assume_init_read()
        })
    }
}
impl<T, C> ExactSizeIterator for IntoIter<T, C> {}
impl<T, C> Drop for IntoIter<T, C> {
    fn drop(&mut self) {
        let len = self.back - self.front;
        if len == 0 {
            return;
        }
        let data = self
            .data
            .as_mut()
            .expect("iterator lost storage")
            .as_mut_ptr()
            .cast::<T>();
        // SAFETY: front..back 仅包含未移出的初始化项，借用排他；分配在值 drop 后释放。
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(data.add(self.front), len));
        }
    }
}

/// 固定节点的完整分配 owner；解构为 T 前先释放原 Box，再退 charge。
pub struct OwnedBox<T, C> {
    data: Box<T>,
    charge: Option<C>,
}
impl<T, C> OwnedBox<T, C> {
    /// 地址由固定Box拥有，pin后不可通过安全接口移动目标值。
    pub fn into_pin(self) -> core::pin::Pin<Self> {
        // SAFETY: 移动owner不移动Box内T；Pin只经Deref借T，!Unpin目标不能安全取回owner。
        unsafe { core::pin::Pin::new_unchecked(self) }
    }
    pub fn try_new<F: Funding<Charge = C>>(
        value: T,
        payer: &F,
    ) -> Result<Self, (StorageError<F::Error>, T)> {
        let charge = if core::mem::size_of::<T>() == 0 {
            None
        } else {
            match payer.charge(Layout::new::<T>()) {
                Ok(charge) => Some(charge),
                Err(error) => return Err((StorageError::Funding(error), value)),
            }
        };
        let data = match Box::<T>::try_new_uninit() {
            Ok(data) => Box::write(data, value),
            Err(_) => return Err((StorageError::Allocation, value)),
        };
        Ok(Self { data, charge })
    }
    pub fn into_inner(self) -> T {
        let Self { data, charge } = self;
        let value = Box::into_inner(data);
        drop(charge);
        value
    }
}
impl<T, C> Deref for OwnedBox<T, C> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}
impl<T, C> DerefMut for OwnedBox<T, C> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use core::{cell::Cell, marker::PhantomPinned};

    struct Stable {
        address: Cell<usize>,
        order: Rc<Cell<usize>>,
        _pin: PhantomPinned,
    }
    impl Drop for Stable {
        fn drop(&mut self) {
            assert_eq!(self.address.get(), self as *const Self as usize);
            assert_eq!(self.order.get(), 0);
            self.order.set(1);
        }
    }
    struct Charge(Rc<Cell<usize>>);
    impl Drop for Charge {
        fn drop(&mut self) {
            assert_eq!(self.0.get(), 1);
            self.0.set(2);
        }
    }
    struct Payer(Rc<Cell<usize>>);
    impl Funding for Payer {
        type Charge = Charge;
        type Error = ();
        fn charge(&self, _: Layout) -> Result<Charge, ()> {
            Ok(Charge(self.0.clone()))
        }
    }

    #[test]
    fn moving_pinned_owner_keeps_target_address_and_drops_value_before_refund() {
        let order = Rc::new(Cell::new(0));
        let pinned = OwnedBox::try_new(
            Stable {
                address: Cell::new(0),
                order: order.clone(),
                _pin: PhantomPinned,
            },
            &Payer(order.clone()),
        )
        .unwrap_or_else(|_| panic!("paid pin allocation failed"))
        .into_pin();
        let value = pinned.as_ref().get_ref();
        value.address.set(value as *const Stable as usize);
        let mut holders = [Some(pinned)];
        let moved = holders[0].take().expect("pinned owner disappeared");
        let value = moved.as_ref().get_ref();
        assert_eq!(value.address.get(), value as *const Stable as usize);
        drop(moved);
        assert_eq!(order.get(), 2);
    }
}
