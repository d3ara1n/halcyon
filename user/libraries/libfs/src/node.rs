//! 本案文件 backing 的四种节点；OAL 不解释这些种类。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum NodeKind {
    Directory = 1,
    Property = 2,
    Stream = 3,
    SymbolicLink = 4,
}
impl NodeKind {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Directory),
            2 => Some(Self::Property),
            3 => Some(Self::Stream),
            4 => Some(Self::SymbolicLink),
            _ => None,
        }
    }
    pub const fn max_info_len(self) -> usize {
        match self {
            Self::Directory | Self::SymbolicLink => 28,
            Self::Property => 88,
            Self::Stream => 80,
        }
    }
}
