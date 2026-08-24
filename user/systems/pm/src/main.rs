//! pm：sleep 异步通路 + 消息接收 + Runnel 数据面的集成验证负载。
//!
//! 剧本：两次睡眠（timer 通路观测）→ 阻塞等 init 的隧道 id 消息 →
//! attach → 写入 8192 字节校验模式（跨回绕、逐批摇铃）→ EOF+摇铃 →
//! 退出（触发对端 PEER_CLOSED）。

#![no_std]

use rinlib::{
    ipc::{
        message::wait_message,
    },
    preclude::*,
    sys_sleep,
};
use librunnel::blocking;

/// 隧道页映射地址：与 init 约定的一致（各自进程空间内的同一常量）。
const TUNNEL_VA: usize = 0x4000_0000;
const STREAM_LEN: usize = 8192;

fn main() {
    debug!("Hello, pm!");
    // sleep 异步通路验证：登记期限 → Waiting → timer 唤醒 → 继续。
    unsafe {
        sys_sleep(30).expect("sleep");
        sys_sleep(10).expect("sleep again");
    }
    debug!("awake after two sleeps");

    // 阻塞等 init 的隧道 id（消息到达 → 移交唤醒）。
    let (digest, payload) = match wait_message() {
        Ok(r) => r,
        Err(e) => {
            debug!("wait_message failed: {:?}", e);
            return;
        }
    };
    if digest.payload_length != 8 {
        debug!("unexpected message kind {}", digest.kind);
        return;
    }
    let mut id_bytes = [0u8; 8];
    id_bytes.copy_from_slice(&payload[..8]);
    let id = u64::from_le_bytes(id_bytes);

    let tunnel = match blocking::attach(id, TUNNEL_VA) {
        Ok(t) => t,
        Err(e) => {
            debug!("tunnel attach failed: {:?}", e);
            return;
        }
    };
    debug!("tunnel attached id={:#x}", id);

    // 校验模式写入：i%251+1，跨回绕分批，每批落页即摇铃。
    let mut sent = 0usize;
    let mut chunk = [0u8; 512];
    while sent < STREAM_LEN {
        let n = (STREAM_LEN - sent).min(chunk.len());
        for (i, b) in chunk.iter_mut().enumerate().take(n) {
            *b = ((sent + i) % 251 + 1) as u8;
        }
        if let Err(e) = tunnel.write_all(&chunk[..n]) {
            debug!("stream write failed at {}: {:?}", sent, e);
            return;
        }
        sent += n;
    }
    if let Err(e) = tunnel.finish() {
        debug!("finish failed: {:?}", e);
        return;
    }
    debug!("stream written {} bytes", sent);
}
