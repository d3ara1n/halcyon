//! target：kill 监督靶子——长寿命 sleep 循环，被外部 ProcessKill 终止
//! 是其唯一正常出口（验证 Waiting 取消与幂等竞争，见 init 剧本）。

#![no_std]

use rinlib::{preclude::*, sys_sleep};

fn main() {
    debug!("target alive");
    loop {
        // SAFETY: 值参数；本进程只经 kill 退出。
        unsafe { sys_sleep(1000).expect("target sleep") };
    }
}
