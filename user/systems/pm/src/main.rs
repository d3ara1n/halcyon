//! pm：sleep 异步通路与消息投递的集成验证负载。两次睡眠后向 init
//! （pid 3，initfs 装载顺序固定）发一条消息：init 若已阻塞在邮箱上
//! 则走到达移交唤醒路径，否则消息入队走快路径——两种时序都必须收敛。

#![no_std]

use rinlib::{ipc::message::send, preclude::*, sys_sleep};

/// init 的 pid（initfs 服务装载顺序：drv/fs/init/pm）。
const INIT_PID: u32 = 3;

fn main() {
    debug!("Hello, pm!");
    // sleep 异步通路验证：登记期限 → Waiting → timer 唤醒 → 继续。
    // 生效与否在内核回收行的「存活时长」观测（应 ≥ 40ms + 开销）。
    unsafe {
        sys_sleep(30).expect("sleep");
        sys_sleep(10).expect("sleep again");
    }
    debug!("awake after two sleeps");
    match send(INIT_PID, 514, &[1u8, 9u8, 1u8, 9u8]) {
        Ok(()) => debug!("pinged init"),
        Err(e) => debug!("send to init failed: {:?}", e),
    }
}
