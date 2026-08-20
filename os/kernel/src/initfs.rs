//! initfs 装载：tar 就地遍历 → ELF 解析 → 进程创建 → 入队。

use crate::{mm, sched, task, task::table};
use tar;

/// 装载 initfs（物理地址，长度）内 `bin/` 下的全部 ELF。
pub fn load(addr: usize, len: usize) {
    // SAFETY: initfs 区间位于直映射覆盖的 DRAM 内（帧池已剔除），只读访问。
    let data = unsafe { core::slice::from_raw_parts(mm::phys_to_virt(addr) as *const u8, len) };
    let mut spawned = 0;
    tar::walk(data, |entry| {
        if !entry.name.starts_with("bin/") || entry.name.ends_with('/') {
            return;
        }
        let Some(image) = elf::parse(entry.data).ok() else {
            crate::println!("[InitFS  ] {} 非法 ELF，跳过", entry.name);
            return;
        };
        let pid = table::alloc_pid();
        match task::spawn_from_elf(pid, 0, &image, entry.data) {
            Ok(thread) => {
                sched::enqueue(thread);
                spawned += 1;
                crate::println!("[Task    ] pid {} <- {}", pid, entry.name);
            }
            Err(e) => crate::println!("[InitFS  ] {} 装载失败: {:?}", entry.name, e),
        }
    })
    .expect("initfs 归档格式错误");
    crate::println!("[InitFS  ] {} 个服务已装载", spawned);
}
