//! 门控容器的真实位置索引。森林不拥有业务对象；短事务的 Gates 只保活被移的门。
//!
//! 来源由 Invocation 的 BodyLease 或暂摘消息的 core Arc 保活。父边不是存储引用，
//! 所以最后 authority 的同步关闭/cut 必须先于来源 pin 消散。锁序为业务锁 → FOREST；
//! FOREST 内只操作预付 token、固定回滚数组及无析构的 capability 槽移动。

use alloc::sync::Arc;
use capability::forest::{self, Domain, DomainId};
use erhino_shared::{call::SystemCallError, proc::PROCESS_MAX_GRANTS};

use super::{
    cap::Cap,
    mailbox::{MailboxCore, MailboxReceiver},
    process::{builder::ProcessBuilder, shell::ProcessShell},
};
use crate::sync::{Spinlock, ranks};

static FOREST: Spinlock<()> = Spinlock::new(ranks::CUSTODY_FOREST, ());

// gate 只在准备阶段取得，绝不在图锁内取 Builder/业务锁或销毁 Arc。
enum Gate {
    Mailbox(Arc<MailboxCore>),
    Process(Arc<ProcessShell>),
}
impl Gate {
    fn domain(&self) -> &Domain {
        match self {
            Self::Mailbox(core) => &core.custody,
            Self::Process(shell) => &shell.custody,
        }
    }
}

pub(crate) struct Gates {
    pins: [Option<Gate>; PROCESS_MAX_GRANTS],
    len: usize,
}

impl Gates {
    /// 不复制 authority；Arc 只覆盖本次移动涉及的稳定域地址。
    pub(crate) fn prepare<'a>(caps: impl IntoIterator<Item = &'a Cap>) -> Self {
        let mut gates = Self {
            pins: core::array::from_fn(|_| None),
            len: 0,
        };
        for cap in caps {
            let object = &cap.object().0;
            let gate = if let Some(receiver) = object.as_any().downcast_ref::<MailboxReceiver>() {
                Some(Gate::Mailbox(receiver.custody_core()))
            } else if let Some(builder) = object.as_any().downcast_ref::<ProcessBuilder>() {
                builder.custody_shell().map(Gate::Process)
            } else {
                None
            };
            if let Some(gate) = gate {
                // 输入数量分别由 Message/ProcessGrant 的共享 ABI 上限限制。
                assert!(
                    gates.len < gates.pins.len(),
                    "custody gate batch exceeds ABI bound"
                );
                gates.pins[gates.len] = Some(gate);
                gates.len += 1;
            }
        }
        gates
    }

    /// 在真实目标业务锁中调用；commit 只能做已预付的无失败/无析构公开。
    /// 每个成功/失败事务至多 O(k log(N+1))，无分配、无重试、无用户内存访问。
    pub(crate) fn transfer<T>(
        &self,
        target: Option<&Domain>,
        commit: impl FnOnce() -> T,
    ) -> Result<T, SystemCallError> {
        if self.len == 0 {
            return Ok(commit());
        }
        let _forest = FOREST.lock();
        // SAFETY: pins 与目标业务 owner 保持 Domain 地址稳定；所有森林访问由 FOREST 串行。
        unsafe {
            let target = target.map(|domain| DomainId::new(domain));
            let mut children = [None; PROCESS_MAX_GRANTS];
            for (id, gate) in children
                .iter_mut()
                .zip(self.pins[..self.len].iter().flatten())
            {
                *id = Some(DomainId::new(gate.domain()));
            }
            // 仿射门控能力使 child 唯一；来源 BodyLease/core Arc 保证旧父 pin。
            // 批量 cut/link/恢复由纯层唯一实现，失败返回时原图已完整恢复。
            forest::transfer::<PROCESS_MAX_GRANTS>(&children[..self.len], target).map_err(
                |error| match error {
                    forest::LinkError::Cycle => SystemCallError::CyclicTransfer,
                    forest::LinkError::Overflow => SystemCallError::ReachLimit,
                    // children 唯一且纯层先切全批，controlled 与目标也在同锁读取；
                    // 其余错误是接线不变量损坏，不伪装成用户的容量或权利错误。
                    forest::LinkError::NotRoot | forest::LinkError::Inactive => {
                        unreachable!("validated custody batch violated forest preconditions")
                    }
                },
            )?;
            Ok(commit())
        }
    }
}

/// 关闭/发布是不可逆根转换；调用者仍持域的唯一生命周期业务锁。
pub(crate) fn make_root(domain: &Domain) {
    let _forest = FOREST.lock();
    // SAFETY: 业务 owner 保证稳定地址与存活，FOREST 是域字段/索引的唯一同步域。
    unsafe {
        forest::make_root(DomainId::new(domain));
    }
}
