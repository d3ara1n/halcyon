//! 生命周期竞态矩阵线协议（init ↔ test_hammer，step 9 验证负载）。
//!
//! 单一二进制多角色：payload 字定模式，handle 经 startup grants 按槽位
//! 交付。指令与 handle 经 Mailbox 运行时投递，发令枪是每锤一把的
//! notification（READABLE 电平，锤 wait 通过后立即 take 清位转脉冲，
//! 多轮复用同一把枪）。
//!
//! 剧本逻辑与全部断言归 init；锤只保证「醒后即打」的窗口密度，回执
//! 只报告 syscall 结果，不判定。

use erhino_shared::object::Rights;

/// payload word[0]：负载模式。
pub const MODE_HAMMER: u64 = 1;
/// payload word[0]：竞态靶模式。
pub const MODE_TARGET: u64 = 2;

/// TARGET payload word[1]：等枪后 sys_exit(code) 正常退出。
pub const TARGET_SUICIDE: u64 = 1;
/// TARGET payload word[1]：等枪后解引用空指针（LoadAccess fault）。
pub const TARGET_FAULT: u64 = 2;
/// TARGET payload word[1]：等枪后高频 sys_sleep(1) 循环。
pub const TARGET_PARK: u64 = 3;
/// TARGET payload word[1]：持续执行公开 Map/Protect/Unmap，供异 hart kill 竞速。
pub const TARGET_MEMORY_CHURN: u64 = 4;
/// TARGET payload word[1]：建立 guarded mapping 后访问 guard，验证用户 fault。
pub const TARGET_GUARD_FAULT: u64 = 5;

/// HAMMER grants 槽位。
pub const HAMMER_CMD: usize = 0;
pub const HAMMER_REPORT: usize = 1;
pub const HAMMER_GUN: usize = 2;
/// TARGET grants 槽位。
pub const TARGET_GUN: usize = 0;

/// 消息 kind：指令（Mailbox → 锤）。
pub const MSG_CMD: u64 = 1;
/// 消息 kind：回执（锤 → report mailbox）。
pub const MSG_REPORT: u64 = 2;

/// 指令动作（payload word[0]，见 [`Cmd`]）。除 START（消费 builder）
/// 与 CREATE（保留待编排方收束）外，动作完成即 close 携带的 handle。
pub const ACTION_EXIT: u64 = 0;
pub const ACTION_KILL: u64 = 1;
pub const ACTION_START: u64 = 2;
pub const ACTION_CREATE: u64 = 3;
pub const ACTION_CREATE_ABANDON: u64 = 4;
pub const ACTION_SEAL: u64 = 5;
pub const ACTION_DRAIN: u64 = 6;
pub const ACTION_CLOSE: u64 = 7;
pub const ACTION_ENUMERATE: u64 = 9;

/// 锤侧 ProcessCreate 请求的 control rights（READ/WAIT/MANAGE——
/// 编排方兜底收束的派生基准）。
pub const HAMMER_CONTROL_RIGHTS: Rights =
    Rights::from_raw(Rights::READ.raw() | Rights::WAIT.raw() | Rights::MANAGE.raw());

/// 指令 payload：5×u64 小端。moves 槽位由动作决定（KILL/DRAIN/CLOSE
/// 用 moves[0]=control，START 用 moves[0]=builder，CREATE/CREATE_ABANDON
/// 用 moves[0]=job，SEAL/ENUMERATE 用 moves[0]=job control）。
#[derive(Debug, Clone, Copy)]
pub struct Cmd {
    pub action: u64,
    /// KILL 的 exit code。
    pub code: u64,
    /// START 的入口地址。
    pub entry: u64,
    /// START 的栈顶。
    pub sp: u64,
    /// 执行前延迟（毫秒）：锤等枪后先 sys_sleep 再执行，时序变体用
    /// （对侧先行窗口）；0 = 醒后即打。
    pub aux: u64,
}

/// 回执 payload 头：3×u64 小端 + 动作特定变长尾（ENUMERATE 的尾即
/// 成员 ID 序列，count 由尾长给出）。
#[derive(Debug, Clone, Copy)]
pub struct Report {
    /// 0 成功，否则 SystemCallError 判别值（未知值不降级解释）。
    pub status: i64,
    /// CREATE*/DRAIN：work_done；KILL*：0。
    pub aux0: u64,
    /// DRAIN：ProcessDrainStatus 判别值。
    pub aux1: u64,
}

/// 指令编码为消息 payload。
pub fn encode_cmd(cmd: &Cmd) -> alloc::vec::Vec<u8> {
    let mut payload = alloc::vec::Vec::new();
    for word in [cmd.action, cmd.code, cmd.entry, cmd.sp, cmd.aux] {
        payload.extend_from_slice(&word.to_le_bytes());
    }
    payload
}

/// 指令解码：长度不足返回 None（协议违约，调用方按 FAILED 记）。
pub fn decode_cmd(payload: &[u8]) -> Option<Cmd> {
    decode_words::<5>(payload).map(|words| Cmd {
        action: words[0],
        code: words[1],
        entry: words[2],
        sp: words[3],
        aux: words[4],
    })
}

/// 回执头编码。
pub fn encode_report(report: &Report, tail: &[u64]) -> alloc::vec::Vec<u8> {
    let mut payload = alloc::vec::Vec::new();
    for word in [report.status as u64, report.aux0, report.aux1] {
        payload.extend_from_slice(&word.to_le_bytes());
    }
    for word in tail {
        payload.extend_from_slice(&word.to_le_bytes());
    }
    payload
}

/// 回执解码：头 + 变长尾（ENUMERATE 的 count + 成员 ID）。
pub fn decode_report(payload: &[u8]) -> Option<(Report, alloc::vec::Vec<u64>)> {
    let words = decode_words_var(payload)?;
    if words.len() < 3 {
        return None;
    }
    let report = Report {
        status: words[0] as i64,
        aux0: words[1],
        aux1: words[2],
    };
    Some((report, alloc::vec::Vec::from(&words[3..])))
}

/// 负载 payload 字编码（模式 + TARGET 角色）。
pub fn encode_hammer_payload(words: &[u64]) -> alloc::vec::Vec<u8> {
    let mut payload = alloc::vec::Vec::new();
    for word in words {
        payload.extend_from_slice(&word.to_le_bytes());
    }
    payload
}

/// 负载 payload 字解码（模式 + TARGET 角色）；长度非 8 倍数或超短
/// 返回 None。
pub fn decode_payload(payload: &[u8]) -> Option<alloc::vec::Vec<u64>> {
    decode_words_var(payload)
}

fn decode_words<const N: usize>(payload: &[u8]) -> Option<[u64; N]> {
    if payload.len() < N * 8 {
        return None;
    }
    let mut words = [0u64; N];
    for (index, word) in words.iter_mut().enumerate() {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&payload[index * 8..index * 8 + 8]);
        *word = u64::from_le_bytes(bytes);
    }
    Some(words)
}

/// 负载 payload 字编码（模式 + TARGET 角色）。
pub fn encode_payload(words: &[u64]) -> alloc::vec::Vec<u8> {
    let mut payload = alloc::vec::Vec::new();
    for word in words {
        payload.extend_from_slice(&word.to_le_bytes());
    }
    payload
}

fn decode_words_var(payload: &[u8]) -> Option<alloc::vec::Vec<u64>> {
    if payload.len() % 8 != 0 {
        return None;
    }
    let mut words = alloc::vec::Vec::new();
    words.try_reserve_exact(payload.len() / 8).ok()?;
    for chunk in payload.chunks_exact(8) {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(chunk);
        words.push(u64::from_le_bytes(bytes));
    }
    Some(words)
}
