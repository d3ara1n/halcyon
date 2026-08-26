//! initfs 装载：tar 就地遍历 → ELF 解析 → 组装启动授权 → 统一 launch。
//!
//! 本模块是启动资源交付的内核授权方（过渡）：按服务身份组装 opaque
//! payload 与 Handle grants。服务化阶段该策略整体迁往 init/pm，launch 机制
//! 原样复用（见 plans/todo-2026-08-process-startup-resources.md）。

use alloc::vec::Vec;
use erhino_shared::object::Rights;

use crate::{mm, sched, task, task::table};
use tar;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ServiceKind {
    Init,
    Pm,
    Other,
}

struct Pending {
    kind: ServiceKind,
    pid: erhino_shared::proc::Pid,
    spawned: task::proc::SpawnedProcess,
}

/// 装载 initfs（物理地址，长度）内 `bin/` 下的全部 ELF。
pub fn load(addr: usize, len: usize) {
    // SAFETY: initfs 区间位于直映射覆盖的 DRAM 内（帧池已剔除），只读访问。
    let data = unsafe { core::slice::from_raw_parts(mm::phys_to_virt(addr) as *const u8, len) };
    let mut pending = Vec::new();
    tar::walk(data, |entry| {
        if !entry.name.starts_with("bin/") || entry.name.ends_with('/') {
            return;
        }
        let Some(image) = elf::parse(entry.data).ok() else {
            warn!(InitFS, "{}: invalid ELF, skipped", entry.name);
            return;
        };
        let kind = match entry.name {
            "bin/srv_init" => ServiceKind::Init,
            "bin/srv_pm" => ServiceKind::Pm,
            _ => ServiceKind::Other,
        };
        if pending.try_reserve(1).is_err() {
            warn!(InitFS, "failed to retain {}: out of memory", entry.name);
            return;
        }
        let pid = table::alloc_pid();
        match task::spawn_from_elf(pid, 0, &image, entry.data) {
            Ok(spawned) => {
                pending.push(Pending { kind, pid, spawned });
                log!(Task, "loaded pid {} <- {}", pid, entry.name);
            }
            Err(e) => warn!(InitFS, "failed to load {}: {:?}", entry.name, e),
        }
    })
    .expect("malformed initfs archive");

    // pm 邮箱对（授权方惯例「服务出生自带邮箱」的组装点）：owner 授 pm，
    // sender 授 init。双方约定各自 StartupBlock Handle[0] 的语义。
    let (mut pm_owner, mut pm_sender) = pending
        .iter()
        .any(|item| item.kind == ServiceKind::Pm)
        .then(|| {
            let mailbox = task::mailbox::Mailbox::new();
            let object = task::mailbox::Mailbox::object_ref(&mailbox);
            let owner = task::handle::entry(
                object.clone(),
                task::object::HandleRole::MailboxOwner,
                Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            )
            .expect("mailbox owner entry rights");
            let sender = task::handle::entry(
                object,
                task::object::HandleRole::MailboxSender,
                Rights::WRITE
                    | Rights::WAIT
                    | Rights::TRANSIT
                    | Rights::GRANT
                    | Rights::DUPLICATE,
            )
            .expect("mailbox sender entry rights");
            (owner, sender)
        })
        .map_or((None, None), |(owner, sender)| (Some(owner), Some(sender)));

    let mut launched = 0;
    for item in pending {
        let mut handles = Vec::new();
        match item.kind {
            ServiceKind::Pm => {
                if let Some(owner) = pm_owner.take() {
                    handles.push(owner);
                }
            }
            ServiceKind::Init => {
                if let Some(sender) = pm_sender.take() {
                    handles.push(sender);
                }
            }
            ServiceKind::Other => {}
        }
        match task::launch(item.spawned, &[], handles) {
            Ok(thread) => {
                sched::enqueue(thread);
                launched += 1;
                log!(Task, "started pid {}", item.pid);
            }
            Err(e) => warn!(InitFS, "failed to launch pid {}: {:?}", item.pid, e),
        }
    }
    // 授权与接收方成对交付；接收方缺席时 transit 关闭未领侧，不泄漏。
    for entry in [pm_owner.take(), pm_sender.take()].into_iter().flatten() {
        task::handle::close_transit(entry);
    }
    log!(InitFS, "{} service(s) loaded", launched);
}
