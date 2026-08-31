# 分批 deferred retire 未来审查

> 【未来审查计划】审查对象固定为提交 `722567344b636c149a61324bab63af95b17c0db1`（`feat(mm): 实现分批 deferred retire`）。只审该提交形成的显式 Retiring 阶段、固定容量 work debt、Remote final ack 后分批退休、metadata owner 接线、Tunnel/ProcessDrain 统一接管与 QEMU 超时校准；切片 6D 的 root/中间页表真实 funded 来源、切片 6E 的匿名 backing 全面资金化、公共 MemoryObject 与多页 Tunnel 不混入本结论。

## 对象概要

该提交为 `os/memory_space` 增加 `SynchronizedChange → RetiringChange → RetiredChange` 显式阶段。`begin_retire` 把 fragment 与 WritePermit 移入 `RetireBatch`，`finish_retire` 只接受已经排空的 batch；`BackingRetire::{Retain, Release}` 区分 Protect 的旧视图替换与 Unmap 的真实 backing 释放。内核 `RetiringSpaceChange` 保存 planner token、table retire cursor、匿名 backing cursor 与 object retire batch，每个 advance 只推进固定粒度，并在 AddressSpace 锁外析构真实 owner。

新增 `os/work_debt` 固定槽纯逻辑队列：Commit 前 Reserve，Remote 最后确认所在 hart 在 Retiring 发布时成为 owner，未完成项按 owner FIFO 重排，Finish 后递增代次再复用。内核以固定安全点总预算与单债务轮转预算推进；每 owner 原子 Pending 是无锁电平真值，IPI 只在预算耗尽仍有残债时提示，idle 双重检查禁止带债入睡。公开 Memory 与 Tunnel 在 Commit 前预留 MemoryChange、memory WaitContext、Remote completion 和 work-debt owner；Remote completion 只进入 Retiring 并发布 debt，最终批才完成 ledger、sink、mandatory operation、线程结果义务与 WaitContext。

Tunnel 显式 close 与 REAPABLE 后的 detached close 共用 `RetiringSpaceChange + LeaseRetire`。前者由 work debt 驱动，后者把同一状态固定保存在 Endpoint 中，由 ProcessDrain 每个 close work unit 推进一步；notice、object fragment 与 WritePermit 全部退休后才发布 CLOSED/PEER_CLOSED。提交同时把 QEMU route timeout 纪律改为按近期实测留宽裕余量，并重新校准 stress 路线；超时只负责收割异常停滞，不是固定性能上限。

## 审查重点

1. 重做 planner 类型状态证明：`begin_retire` 必须只接受 Synchronized token，事务记录同步进入 Retiring；`finish_retire` 必须拒绝仍含 fragment/permit 的 batch，Retired/Complete 不得提前删除 ledger transaction。
2. 逐类核对 `BackingRetire`：Unmap、drain 与真正移除 mapping 的路径必须 Release；Protect 或仍由 live ledger 完整覆盖的旧视图必须 Retain。错误分类不得造成 backing 早退、泄漏或重复释放。
3. 复核 Commit 前准入闭包：MemoryChange、WaitContext、Remote completion、work slot、Remote Call slots、completion Arc 与 Tunnel sink 必须全部在 Commit 前取得；任一 metadata/OOM/Busy/stale failure 必须经 RAII 完整回滚，Commit 后不得取得 heap、Pool、WaitContext 或 Remote 槽。
4. 审查 `os/work_debt` 的固定容量与 affine token：Reserve/Cancel/Publish/Take/Requeue/Finish 的 phase、owner、next、generation 必须闭合；错误 token 不得篡改队列，代次耗尽只能永久退休槽，不能 ABA 复用。
5. 复核 work-debt 容量与 metadata admission 的对应关系：全局 work slots 不得小于可 Commit 的 MemoryChange 存量；预留顺序和失败退款不能形成一类 permit 已提交、另一类槽不可得的死锁或容量旁路。
6. 审查 owner hart 发布与内存序：Remote final ack 所在 hart 必须成为唯一推进 owner；队列 Publish 必须先于 Pending Release，安全点/idle 的 Acquire 观察后必须能取得同一 work。Pending 计数、队列 phase 与 owner FIFO 不得出现假零、欠计数或跨 hart take。
7. 逐入口核对安全点：trap entry/exit、scheduler loop 与 Requeue/Killed/Park 收束必须先 drain Remote Call，再推进本 hart work debt；常态无 Pending 时不能争全局债务锁，Pending 存在时 idle 不能 WFI。重复、合并或失败门铃不得改变工作真值。
8. 复核固定预算和公平性：单安全点总步骤、单债务 turn 与 FIFO requeue 必须共同保证每步有静态上界、长债不独占、短债不饿死；预算耗尽只重新提示 owner，不得在不可中断内核路径 drain-until-empty。
9. 追踪 Remote completion Arc 生命周期：Remote Call 最后确认必须消费一个稳定强引用并把它移入 work debt；slot Finish、completion publish、debt Finish 的顺序不得丢失 completion、重复 final ack 或在 `complete()` 返回后留下无主 mandatory operation。
10. 逐项审查 `PublishedTableChanges`：unused outcome、retired branch owner 与 outcome 容器本身必须按固定步骤退休；任何 `TableFrameToken` 都只能在 AddressSpace/Remote/work-debt 锁外析构，Remote ack 前不得退款。
11. 复核匿名 backing cursor：`release_one` 每次最多切下一个 extent owner，跨 extent 的 offset/remaining 必须单调且页数守恒；owner 在 AddressSpace 锁外释放。`binary_search_by_key` 当前依赖 `table_transaction_active` 覆盖 mint→Commit 的单调 push，6D/6E 拆闸前必须改为有序插入或显式索引。
12. 追踪三类 metadata owner 的真实寿命：MemoryChange 与 Remote completion permit 随 completion，MemoryWait permit 随 WaitContext；发起线程退出、进程终止、结果放弃或 Control 壳延寿均不得提前退款、重复退款或扩容 sponsor/global 上限。
13. 重建最终完成顺序：全部 table/backing/object/permit owner 锁外退休后，必须依次 FinishRetire/Complete ledger、sink finish、`complete_mandatory`/REAPABLE、释放 ThreadResultObligation、完成 WaitContext。原发起线程消散或 termination 先到不得改变该顺序。
14. 审查 `LeaseRetire` 的 exactly-once 条件：只接受对应 lease 的唯一 object fragment 与唯一 WritePermit；peer notice 必须 Commit 后安装、最终消费一次；fragment/permit 缺失、重复、错误 object/region 或重复 finish 必须 fail closed。
15. 复核 detached close 的持久化与引用环：Endpoint → DetachedLeaseRetire → LeaseRetire → Endpoint 的临时强环必须由 pending_close 持续重入并在完成分支 take 后打破；任何新增放弃 entry 路径都必须先拆环。特别防止 if-let scrutinee 延长 Spinlock guard 后在未完成分支重取同一锁的自死锁回归。
16. 核对 ProcessDrain 预算：每次 detached close advance 只计一个诚实 work unit，Busy/未完成必须原样回存 entry，完成后才能继续 HandleTable/AddressSpace drain；REAPABLE 的 mandatory 屏障必须排除公开 work-debt 事务与 drain ledger 并发。
17. 复核 Lock Ladder：`MEMORY_COMPLETION < WORK_DEBT < REMOTE_CALL` 的秩位置、AddressSpace→completion/work 的实际调用、owner 锁外 Drop、Tunnel Connection/Endpoint/Wait 锁组合均不得反向嵌套或持容器锁调用外部 sink。
18. 审查验证与基础设施事实：planner/work-debt host 测试必须真实覆盖 live-owner Finish 拒绝、Retain/Release、全局容量回滚、owner FIFO、最小预算多批、公平性、缺失/重复门铃与代次复用；QEMU core/stress 必须覆盖 memory-vs-kill、无 runnable、Tunnel exit、ProcessDrain 最小预算与完整竞态矩阵。route timeout 只按 workload 实测留余量，不得再次写成架构上限，也不得用放宽超时掩盖缺失业务/reset 锚点。
19. 对照 `notes/ideas/mm.md`、`notes/impls/mm.md`、主计划与 `plans/TOOLING-PITFALLS.md`，确认方向、实现和验证纪律一致；6D/6E 尚未完成的 funded table/backing 来源只回到主计划，不用其否定本提交的 retire 协议，也不把 transitional raw owner 描述为最终机制。

## 基线证据

- `cd os && cargo test -p work_debt -p memory_space --target aarch64-apple-darwin`
- `cd os && cargo test --release -p work_debt -p memory_space --target aarch64-apple-darwin`
- `cd os && cargo clippy -p work_debt -p memory_space --all-targets --target aarch64-apple-darwin -- -D warnings`
- `just check`
- `just virt`
- `just virt-stress`

提交收口时上述验证通过：memory_space planner 19 项与 work_debt 5 项 host debug/release 测试全绿，两个改动 crate 的 clippy 无警告；默认节流的 virt core 与 stress 均完成业务收束和显式 reset，stress 命中最小预算 Drain、Tunnel exit 16 轮、同 AddressSpace 多 hart shootdown、thread storm 与完整 16/16 竞态矩阵。内核全 workspace clippy 仍受未由该提交引入的既有 lint 基线阻断，不列作本提交通过证据。

## 完成标准

所有发现按严重度给出文件/符号证据、可达交错、锁上下文、owner 方程或预算计数，并明确影响的是 planner 阶段、work-debt phase/Pending、Remote ack、table/backing/metadata owner、Tunnel notice、mandatory/result/WaitContext 还是 ProcessDrain。任何 Commit 后分配或可恢复失败、Pending 假零、无主债务、预算内无界扫描、ack 前退款、容器锁内 owner 析构、ledger 提前 Complete、mandatory/result/WaitContext 乱序、detached 引用环泄漏、自锁或同步 retire 旁路均为阻断项。修复后重跑相关 host debug/release、clippy、`just check`、默认 core 与 stress；6D/6E 的未实现能力只回到主计划既有真值点，不复制新的 TODO。
