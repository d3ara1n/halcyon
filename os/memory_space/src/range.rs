use core::cmp::{max, min};

pub const PAGE_SIZE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeError {
    Empty,
    Overflow,
    Unaligned,
}

/// 通用半开字节区间；可用于固定宽用户结果槽等非页对齐范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressRange {
    start: usize,
    end: usize,
}

impl AddressRange {
    pub fn new(start: usize, bytes: usize) -> Result<Self, RangeError> {
        if bytes == 0 {
            return Err(RangeError::Empty);
        }
        let end = start.checked_add(bytes).ok_or(RangeError::Overflow)?;
        Ok(Self { start, end })
    }

    pub fn from_bounds(start: usize, end: usize) -> Result<Self, RangeError> {
        if start >= end {
            return Err(RangeError::Empty);
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> usize {
        self.start
    }

    pub const fn end(self) -> usize {
        self.end
    }

    pub const fn bytes(self) -> usize {
        self.end - self.start
    }

    pub const fn contains_address(self, address: usize) -> bool {
        self.start <= address && address < self.end
    }

    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub const fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub const fn adjacent(self, other: Self) -> bool {
        self.end == other.start || other.end == self.start
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let start = max(self.start, other.start);
        let end = min(self.end, other.end);
        (start < end).then_some(Self { start, end })
    }
}

/// 页对齐半开字节区间。构造完成后可安全换算页数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange(AddressRange);

impl PageRange {
    pub fn new(start: usize, bytes: usize) -> Result<Self, RangeError> {
        if !start.is_multiple_of(PAGE_SIZE) || !bytes.is_multiple_of(PAGE_SIZE) {
            return Err(RangeError::Unaligned);
        }
        Ok(Self(AddressRange::new(start, bytes)?))
    }

    pub fn rounded(start: usize, bytes: usize) -> Result<Self, RangeError> {
        if !start.is_multiple_of(PAGE_SIZE) {
            return Err(RangeError::Unaligned);
        }
        let rounded = bytes
            .checked_add(PAGE_SIZE - 1)
            .ok_or(RangeError::Overflow)?
            / PAGE_SIZE
            * PAGE_SIZE;
        Self::new(start, rounded)
    }

    pub fn from_bounds(start: usize, end: usize) -> Result<Self, RangeError> {
        if !start.is_multiple_of(PAGE_SIZE) || !end.is_multiple_of(PAGE_SIZE) {
            return Err(RangeError::Unaligned);
        }
        Ok(Self(AddressRange::from_bounds(start, end)?))
    }

    pub const fn address_range(self) -> AddressRange {
        self.0
    }

    pub const fn start(self) -> usize {
        self.0.start()
    }

    pub const fn end(self) -> usize {
        self.0.end()
    }

    pub const fn bytes(self) -> usize {
        self.0.bytes()
    }

    pub const fn pages(self) -> usize {
        self.bytes() / PAGE_SIZE
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0.contains(other.0)
    }

    pub const fn overlaps(self, other: Self) -> bool {
        self.0.overlaps(other.0)
    }

    pub const fn adjacent(self, other: Self) -> bool {
        self.0.adjacent(other.0)
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        self.0.intersection(other.0).map(Self)
    }
}
