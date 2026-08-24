//! init：消息/信号通路的集成验证负载。自发自收一条消息（同步路径），
//! 再阻塞等待一条异步到达的消息（Receive 阻塞 + 移交唤醒路径），随后
//! 自我提交 TERMINATE 请求并经 SignalWait 消费——三面机制一次跑通。

#![no_std]

use rinlib::{
    env,
    ipc::{
        message::{peek, receive, send, wait_message},
        signal,
    },
    preclude::*,
    shared::signal::{ObjectKind, SignalItem, TERMINATE},
};

fn main() {
    debug!("Hello, init!");
    let me = env::pid();

    // 同步自发自收：Send 即完成，Peek 立即可见。
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

    // 异步通路：先阻塞等待，由 pm（pid 4）投递一条消息唤醒——
    // 若 pm 先发则走队列快路径，若 init 先阻塞则走到达移交路径。
    match wait_message() {
        Ok((digest, payload)) => debug!("waited: sender={}, kind={}, payload={:?}", digest.sender, digest.kind, payload),
        Err(e) => debug!("wait_message failed: {:?}", e),
    }

    // 事件面：自我提交终止请求，SignalWait 消费（命中即清）。
    let items = [SignalItem {
        kind: ObjectKind::SelfProcess as u64,
        id: 0,
        interest: TERMINATE,
    }];
    if let Err(_) = signal::send(me, TERMINATE) {
        debug!("signal send failed");
    }
    match signal::wait(&items) {
        Ok((_, bits)) => debug!("terminate requested: bits={:#x}", bits),
        Err(e) => debug!("signal wait failed: {:?}", e),
    }
}
