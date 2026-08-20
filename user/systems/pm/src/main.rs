#![no_std]

use rinlib::{sys_sleep, preclude::*};

fn main() {
    debug!("Hello, pm!");
    // sleep 异步通路验证：登记期限 → Waiting → timer 唤醒 → 继续。
    // 生效与否在内核回收行的「存活时长」观测（应 ≥ 40ms + 开销）。
    unsafe {
        sys_sleep(30).expect("sleep");
        sys_sleep(10).expect("sleep again");
    }
    debug!("awake after two sleeps");
}
