//! init：消息 + 信号 + 隧道/Runnel 全通路的集成验证负载。
//!
//! 剧本：
//! 1. 同步自发自收一条消息（邮箱快路径）；
//! 2. 创建 Runnel 隧道，把 id 经消息发给 pm（pid 4），随后阻塞读
//!    8192 字节（跨回绕、走到达移交唤醒）并校验数据；
//! 3. 等 pm 退出后的 PEER_CLOSED 终态位，Dispose 本端（帧归还）。

#![no_std]

use rinlib::{
    env,
    ipc::{
        message::{peek, receive, send},
        signal,
    },
    preclude::*,
    shared::signal::{ObjectKind, SignalItem, TUNNEL_PEER_CLOSED},
};
use librunnel::blocking;

/// pm 的 pid（initfs 服务装载顺序：drv/fs/init/pm）。
const PM_PID: u32 = 4;
/// 隧道页在本进程的映射地址（VA 分配器落地前由调用方自报）。
const TUNNEL_VA: usize = 0x4000_0000;
/// 验证数据量：超过环形容量（3968），强制写端分批与回绕。
const STREAM_LEN: usize = 8192;

fn main() {
    debug!("Hello, init!");
    let me = env::pid();

    // —— 同步自发自收：Send 即完成，Peek 立即可见 ——
    match send(me, 114, &[5u8, 1u8, 4u8]) {
        Ok(()) => match peek() {
            Ok(digest) => {
                debug!("digest: kind={}, len={}", digest.kind, digest.payload_length);
                let mut buf = [0u8; 16];
                match receive(&mut buf) {
                    Ok(n) => debug!("payload: {:?}", &buf[..n]),
                    Err(e) => debug!("receive failed: {:?}", e),
                }
            }
            Err(e) => debug!("peek failed: {:?}", e),
        },
        Err(e) => debug!("send failed: {:?}", e),
    }

    // —— 数据面：建隧道 → id 经消息面转交 → 阻塞读流 ——
    let tunnel = match blocking::create(TUNNEL_VA) {
        Ok(t) => t,
        Err(e) => {
            debug!("tunnel create failed: {:?}", e);
            return;
        }
    };
    debug!("tunnel created id={:#x}", tunnel.id());
    let id_bytes = tunnel.id().to_le_bytes();
    if let Err(e) = send(PM_PID, 514, &id_bytes) {
        debug!("send tunnel id failed: {:?}", e);
        return;
    }

    let mut buf = [0u8; STREAM_LEN];
    match tunnel.read_exact_or_eof(&mut buf) {
        Ok(n) => {
            let ok = buf.iter().enumerate().all(|(i, &b)| b == (i % 251 + 1) as u8);
            debug!("stream received {} bytes, pattern {}", n, if ok { "ok" } else { "MISMATCH" });
        }
        Err(e) => debug!("stream read failed: {:?}", e),
    }

    // —— 事件面：等 pm 退出后的 PEER_CLOSED 终态位 ——
    let items = [SignalItem {
        kind: ObjectKind::TunnelEndpoint as u64,
        id: tunnel.id(),
        interest: TUNNEL_PEER_CLOSED,
    }];
    match signal::wait(&items) {
        Ok((_, bits)) => debug!("peer closed observed: bits={:#x}", bits),
        Err(e) => debug!("peer-closed wait failed: {:?}", e),
    }
    let _ = tunnel.dispose();
}
