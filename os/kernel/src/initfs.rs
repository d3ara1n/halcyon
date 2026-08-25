//! initfs 装载：tar 就地遍历 → ELF 解析 → 建立启动授权 → 统一入队。

use alloc::vec::Vec;
use erhino_shared::startup::{
    GRANT_PM_MAILBOX, MESSAGE_KIND_STARTUP, StartupGrant, StartupHeader,
};

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

    let mut pm_sender = pending
        .iter_mut()
        .find(|item| item.kind == ServiceKind::Pm)
        .and_then(|item| item.spawned.sender_grant.take());

    let mut spawned_count = 0;
    for mut item in pending {
        let mut handles = Vec::new();
        let mut grants = Vec::new();
        if item.kind == ServiceKind::Init {
            if let Some(sender) = pm_sender.take() {
                handles.push(sender);
                grants.push(StartupGrant::new(GRANT_PM_MAILBOX, 0));
            }
        }
        if let Some(unused) = item.spawned.sender_grant.take() {
            task::handle::close_transit(unused);
        }
        let payload = startup_payload(&grants);
        item.spawned.bootstrap_mailbox.enqueue_startup(
            MESSAGE_KIND_STARTUP,
            payload,
            handles,
        );
        sched::enqueue(item.spawned.thread);
        spawned_count += 1;
        log!(Task, "started pid {}", item.pid);
    }
    if let Some(unclaimed) = pm_sender {
        task::handle::close_transit(unclaimed);
    }
    log!(InitFS, "{} service(s) loaded", spawned_count);
}

fn startup_payload(grants: &[StartupGrant]) -> Vec<u8> {
    let header = StartupHeader::new(grants.len() as u32);
    let total = core::mem::size_of::<StartupHeader>()
        + grants.len() * core::mem::size_of::<StartupGrant>();
    let mut payload = Vec::new();
    payload.try_reserve_exact(total).expect("startup payload allocation failed");
    append_value(&mut payload, &header);
    for grant in grants {
        append_value(&mut payload, grant);
    }
    payload
}

fn append_value<T: Copy>(output: &mut Vec<u8>, value: &T) {
    // SAFETY: Startup ABI structs are fully initialized and contain no padding.
    let bytes = unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    };
    output.extend_from_slice(bytes);
}
