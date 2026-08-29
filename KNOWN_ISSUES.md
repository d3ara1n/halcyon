# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## 帧池 free-list 无界扫描（非 Drain 路径）

`FramePool::dealloc` 的插入位定位是 O(region_count) 地址序单链扫描
（碎片化程度决定）。ProcessDrain 已全链走 `dealloc_bounded`（可恢复
游标 + 硬预算），但**常规运行路径**的 `FrameTracker::Drop`（对象关闭、
页表构造失败回滚、boot 前半区回收等）仍调用普通 `dealloc`——单次
成本无上界（与全局碎片数成正比）。这是既存的全局短路径缺口，不属于
进程生命周期里程碑的收束契约面。

触发条件：帧池 allocator 演进（buddy / 地址分桶多链）或实测碎片化导致
可观察延迟时，把普通路径统一迁到有界归还（游标化）或重构底层数据
结构。
触发条件：帧池 allocator 演进（buddy / 地址分桶多链）或实测碎片化导致
可观察延迟时，把普通路径统一迁到有界归还（游标化）或重构底层数据
结构。

## 提前 quiescent 停机（负载存活时误判静默）

竞态矩阵 kill-vs-exit 期间系统在负载仍存活时判定 quiescent 并 SRST 停机：
正常终态 virt 帧数 248843，异常轮 234158（差 14685 帧 = 双锤 + 靶未回收）。
virt 上表现为锚点缺失的 acceptance 失败（QEMU 退出）；sifive_u 上 SRST 不可用
→ `hart::park()` → QEMU 永不退出——**与真挂死同形**，可能就是批一 review §B
那个「无法定性」的挂死真身。

频率极低（首次发现 8 轮 1 次；收口验证 26 轮未复现），非稳定复现。
非批一引入（基线同样复现）。调查计划见 `plans/todo-2026-08-29-early-quiescent-shutdown.md`；
失败日志由 `tools/qemu-acceptance.sh` 自动保留（`artifacts/failed-acceptance-*`），
首份现场在 `artifacts/evidence-2026-08-29-early-quiescent.log`。

触发条件：quiescent 谓词对「存在 Waiting 线程但队列无 runnable」的误判窗口；
若确认即修复，否则留作已知限制。
## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
