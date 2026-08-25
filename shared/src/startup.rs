//! 版本化启动消息及其初始授权描述。

/// 内核/父进程投递的启动消息 kind。
pub const MESSAGE_KIND_STARTUP: u64 = 0x5354_4152_5455_5001;
pub const STARTUP_VERSION: u16 = 1;

/// 当前集成配置中授予 init 的 pm Mailbox sender。
pub const GRANT_PM_MAILBOX: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct StartupHeader {
    pub version: u16,
    pub kind: u16,
    pub grant_count: u32,
    pub reserved: [u64; 2],
}

impl StartupHeader {
    pub const fn new(grant_count: u32) -> Self {
        Self {
            version: STARTUP_VERSION,
            kind: 0,
            grant_count,
            reserved: [0; 2],
        }
    }
}

/// `handle_index` 指向同一消息 Receive 得到的 Handle 数组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct StartupGrant {
    pub tag: u64,
    pub handle_index: u32,
    pub reserved: u32,
}

impl StartupGrant {
    pub const fn new(tag: u64, handle_index: u32) -> Self {
        Self {
            tag,
            handle_index,
            reserved: 0,
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<StartupHeader>() == 24);
    assert!(core::mem::size_of::<StartupGrant>() == 16);
};
