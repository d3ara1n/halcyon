//! 内核验收 task 的私有出生帧与报告格式；由 init 编码，`test_kernel` 解释。
//!
//! 命名出生契约（`libprocess::birth`）接通前的临时格式，删除条件见组态计划债务表。
#![no_std]

pub mod birth {
    //! 出生帧：部署模式、进程角色、输入槽号与映像有效长度。

    /// 帧长：0–3 模式位，4 角色，5–16 输入槽号，17–23 保留为零，24–55 四个映像长度。
    pub const FRAME_BYTES: usize = 56;
    const SLOTS: core::ops::Range<usize> = 5..5 + Input::COUNT;
    const RESERVED: core::ops::Range<usize> = 5 + Input::COUNT..24;
    const LENGTHS: usize = 24;
    /// 该输入缺省时的槽号。
    pub const ABSENT: u8 = u8::MAX;

    /// 部署模式位，由 init 按所选验收部署设置。
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Mode(pub u32);
    impl Mode {
        pub const STRESS: u32 = 1;
        pub const MEMORY: u32 = 1 << 1;
        pub const MMIO_PUBLICATION: u32 = 1 << 2;
        pub const MMIO_FORCED: u32 = 1 << 3;
        pub const DMA_RACES: u32 = 1 << 4;
        pub const DMA_SHARED: u32 = 1 << 5;
        pub const DMA_QUOTA: u32 = 1 << 6;
        pub const DMA_INSTALL_FAILURE: u32 = 1 << 7;
        pub const IRQ_EXIT: u32 = 1 << 8;
        const KNOWN: u32 = (1 << 9) - 1;
        pub fn has(self, flag: u32) -> bool {
            self.0 & flag != 0
        }
        pub fn stress(self) -> bool {
            self.has(Self::STRESS)
        }
        /// memory 部署复用 stress 的线程套件。
        pub fn memory(self) -> bool {
            self.has(Self::MEMORY)
        }
    }

    /// 进程角色：init 启动的主验收进程，或主进程为跨 reset 保留 fixture 自启动的子进程。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    #[repr(u8)]
    pub enum Role {
        Main = 0,
        RetainExposed = 1,
        RetainFault = 2,
    }
    impl Role {
        fn decode(byte: u8) -> Option<Self> {
            Some(match byte {
                0 => Self::Main,
                1 => Self::RetainExposed,
                2 => Self::RetainFault,
                _ => return None,
            })
        }
    }

    /// 输入角色；值即出生帧槽号表中的下标。
    #[derive(Clone, Copy, PartialEq, Eq)]
    #[repr(usize)]
    pub enum Input {
        Pool,
        Job,
        Reset,
        Pm,
        Target,
        Hammer,
        Mmio,
        Irq,
        Dma,
        Retention,
        SelfImage,
        Report,
    }
    impl Input {
        pub const COUNT: usize = 12;
        /// 映像输入在长度表中的下标。
        fn length_index(self) -> Option<usize> {
            Some(match self {
                Self::Pm => 0,
                Self::Target => 1,
                Self::Hammer => 2,
                Self::SelfImage => 3,
                _ => return None,
            })
        }
    }

    pub struct Frame {
        pub mode: Mode,
        pub role: Role,
        slots: [u8; Input::COUNT],
        lengths: [u64; 4],
    }
    impl Frame {
        pub fn new(mode: Mode, role: Role) -> Self {
            Self {
                mode,
                role,
                slots: [ABSENT; Input::COUNT],
                lengths: [0; 4],
            }
        }
        pub fn set_slot(&mut self, input: Input, slot: u8) {
            self.slots[input as usize] = slot;
        }
        pub fn slot(&self, input: Input) -> Option<u8> {
            let slot = self.slots[input as usize];
            (slot != ABSENT).then_some(slot)
        }
        /// 映像输入的有效长度；非映像输入恒为零。
        pub fn image_length(&self, input: Input) -> u64 {
            input.length_index().map_or(0, |index| self.lengths[index])
        }
        pub fn set_image_length(&mut self, input: Input, length: u64) {
            let index = input
                .length_index()
                .expect("only image inputs carry a length");
            self.lengths[index] = length;
        }
        pub fn encode(&self) -> [u8; FRAME_BYTES] {
            let mut bytes = [0; FRAME_BYTES];
            bytes[..4].copy_from_slice(&self.mode.0.to_le_bytes());
            bytes[4] = self.role as u8;
            bytes[SLOTS].copy_from_slice(&self.slots);
            for (index, length) in self.lengths.iter().enumerate() {
                let start = LENGTHS + index * 8;
                bytes[start..start + 8].copy_from_slice(&length.to_le_bytes());
            }
            bytes
        }
        pub fn decode(bytes: &[u8]) -> Option<Self> {
            let bytes: &[u8; FRAME_BYTES] = bytes.try_into().ok()?;
            let mode = Mode(u32::from_le_bytes(bytes[..4].try_into().ok()?));
            if mode.0 & !Mode::KNOWN != 0
                || (mode.memory() && !mode.stress())
                || bytes[RESERVED].iter().any(|&byte| byte != 0)
            {
                return None;
            }
            Some(Self {
                mode,
                role: Role::decode(bytes[4])?,
                slots: bytes[SLOTS].try_into().ok()?,
                lengths: core::array::from_fn(|index| {
                    let start = LENGTHS + index * 8;
                    u64::from_le_bytes(
                        bytes[start..start + 8]
                            .try_into()
                            .expect("fixed frame length checked"),
                    )
                }),
            })
        }
    }
}

pub mod report {
    //! 主验收进程经私有 sender 向 init 报告 held Pool 的最终快照。
    use erhino_shared::memory_pool::MemoryPoolSnapshot;

    pub const KIND: u64 = u64::from_le_bytes(*b"KERNPOOL");
    pub const BYTES: usize = 104;

    /// 快照的全部字段逐字编码；init 以字节相等判断 held Pool 在报告后未变。
    pub fn encode_pool(pool: MemoryPoolSnapshot) -> [u8; BYTES] {
        let words = [
            pool.identity,
            pool.parent_identity,
            pool.total,
            pool.available,
            pool.reserved,
            pool.allocated,
            pool.delegated,
            u64::from(pool.depth),
            pool.metadata.total,
            pool.metadata.available,
            pool.metadata.reserved,
            pool.metadata.allocated,
            pool.metadata.delegated,
        ];
        let mut bytes = [0; BYTES];
        for (index, word) in words.into_iter().enumerate() {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }
}
