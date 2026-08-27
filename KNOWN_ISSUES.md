# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## release 构建不收束（steady-state 后无 quiescent 停机）

release 构建从未纳入验证线；首次观测（2026-08-28，生命周期 step 7
收口时）发现 `MODE=release` 下 virt 4 核全负载走到 `init: steady-state
supervision` 后不再前进：无 `[Sched] system quiescent` 停机、QEMU 不
退出（debug 模式同负载稳定收束，virt ×5 / sifive_u 全绿）。干净
HEAD（b5c2bfe）同样复现，非 step 7 引入。

伴随现象：收束路径与 debug 分叉——pm 委托域走了
`pm delegated domain not collected by pm; init collecting` 兜底分支
（debug 下为 `pm delegated domain confirmed Dead`），最终拓扑
`job root (jid 1, Open, members 0, children 0)`（debug 下 root 仍含
init 进程与 services 子 Job）。提示是仅在 release 时序下暴露的竞态或
内存序问题，而非纯停机谓词缺陷。is_quiescent 三条件（全员 idle /
就绪队列空 / 各 hart 期限表空）中何者不满足尚未定位。

触发条件：排入 release 调查计划时定位（候选轴：收束竞态内存序、
pm 收集超时路径、期限表残留、idle 掩码竞争）；release 验证线接入前
必须解决。

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

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
