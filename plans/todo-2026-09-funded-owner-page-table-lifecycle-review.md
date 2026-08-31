# 资金化 owner 与页表生命周期未来审查

> 【未来审查计划】审查对象固定为提交 `c522e50afb02feafaba4070effce4279397bbc6a`（`feat(mm): 建立资金化 owner 与页表生命周期协议`）。只审该提交形成的资金化 owner 守恒分解、切片 6 metadata admission、owner-aware 页表事务、空表剪枝、表 owner 跨 Remote ack 保活与可恢复 drain；分批 deferred retire、root/中间表真实 funded 来源、匿名 backing 全面资金化、公共 MemoryObject 与多页 Tunnel 不混入本结论。

## 对象概要

该提交完成主计划切片 6A/6B。`os/funded_frame` 以 `QuotaCredit`、`PhysicalClaim::split_at`、借用式 `Funded::split_off` 与 `merge_from` 建立物理 extent 和同源额度不可拆散的守恒变换；内核 `MemoryCharge` 与 `ClaimedUserExtent` 接回具体 Pool/FramePool owner。`metadata_admission` 增加原子批量预留和精确退款，内核为 Region、planner transaction、backing slice、MemoryChange、memory WaitContext 与 Remote completion 固定 global/sponsor 上限及真实 permit owner。

`os/page_table` 把启动期 `EagerFrameMemory` 与运行期 `TableFrameMemory + TableFrameOwner` 分开。运行树保存 root/branch affine owner ledger；`TranslationPreflight → prepare → publish` 分离结构计算、owner 供给与不可失败发布。单项发布严格绑定当前树代次，同事务多项发布只允许同代次 Map 批次；unused/retired owner 显式返回。Unmap 自底向上剪除新空 owned 分支，shared root 不进入 Map/Protect/Unmap/drain 的 owned 子树语义；层级无关 `DrainCursor` 逐步交出 owner，`TableTree::Drop` 只接受已 drain 的常数终态。

内核以 `BorrowedRoot(FrameNumber) | Owned(FrameTracker)` 作为切片 6D 前的最窄过渡 token。Running 与 Tunnel 通过 `table_transaction_active` 独占 Prepared 到 Commit/rollback 的树代次；Publish outcome 随 ledger 事务跨 Synchronize/Remote ack 保活，unused 与 retired table owner 在 AddressSpace 锁外析构。`BoundAddressSpace` 改为独立 `Box`，避免 owner-aware 树扩大 Process 构造栈帧；root funded owner 仍由 `PoolBinding` 唯一持有，树只借用 root 几何。

## 审查重点

1. 逐分支验证 `Funded::split_off` 的双账本守恒：跨 extent 边界必须同步切割物理 owner 与同源 credit；零、尾端、越界和 credit split 失败必须保持原 owner 完整，不能产生重叠 extent、空 credit 或页数错配。
2. 复核 `Funded::merge_from` 的失败原子性：wrong-owner、页数溢出和固定 extent 容量不足必须在改写任一方前失败；成功后 receiver 几何/credit 同步增长，donor 成为合法空 owner，双方最终析构恰退款一次。重点检查 `Option<C>` 只表达已转移终态，不形成可丢失 credit 的安全旁路。
3. 追踪内核 `MemoryCharge`、`ClaimedUserExtent` 与 generic traits 的契约对应：owner identity、页数、split/merge 错误和析构顺序必须与 `memory_pool::AllocatedCredit`、FramePool geometry 一致；不得暴露可把物理与额度分别取走的公共 API。
4. 复核批量 metadata admission 的原子性：global/local 任一侧不足时已取得部分必须完整回滚；`Permit::units`、批量精确退款和 sponsor 强引用寿命不得扩容、提前退款或重复退款。
5. 对照准入表逐类追踪真实 owner：Region/planner storage 由 Bound AddressSpace 的 `AddressSpacePermit` 一次预付；backing slice、MemoryChange、memory WaitContext、Remote completion 的 permit 必须分别随对应对象延寿。确认 global 131072 Region / 128 transaction 与每 AddressSpace 4096 / 4 的“最多 32 个完整 planner AddressSpace”推导一致。
6. 审查 Eager/运行期页表 seam 分离：启动页表只能经 `EagerFrameMemory` 即时取得静态表号；运行树只能接收调用方已取得且清零的 affine owner。PTE 中的 `FrameNumber` 只能是几何投影，owner ledger 必须是 root/每张 owned branch 的唯一所有权真值。
7. 重做 Map/Unmap/Protect preflight 证明：需求计数必须精确去重缺失路径和 mega split；资源不足、非法 flags、冲突、NotMapped/ProtectionMismatch 与 owner ledger reserve 失败均须在任何 PTE 修改前返回，并把全部 supplied owner 交还失败值。
8. 复核树代次与 stale Prepared 契约：`prepare` 必须按当前结构复检；单项 `publish` 只接受当前代次；`publish_batch` 仅允许同代次 Map 且 outcome 容器已预留，前项只能减少后项需求。Map-vs-pruning-Unmap、双 Unmap 和 shared-root attach 后的旧 Prepared 不得进入 Commit 后 panic、分配或缺 owner 路径。
9. 逐入口核对内核 `table_transaction_active`：公开 Map/Unmap/Protect、Tunnel Map/Unmap 的成功 prepare 必须置位，所有 prepare 后失败、rollback、stale execution、正常 Commit 与 detached close 都必须恰清一次；第二笔事务返回 Busy 不能损失 ledger permit、WritePermit、backing 或 table owner。Building 即时路径不得越过仍活跃的 Running/Tunnel Prepared。
10. 审查 shared root 隔离：attach 必须与 owned/有效槽冲突检查和 generation 增长原子；Map/Protect preflight 与 Publish 防线不得遍历或改写外部子树；Unmap 和 drain 必须跳过 shared 槽，`finish_drain` 不得要求外部 owner。
11. 逐层验证 Unmap 空表剪枝：只在 owned child 真正变空时清 branch PTE、owned root 位和 owner ledger；摘除 owner 必须进入 `PublishOutcome.retired`，在 Remote ack 前持续存活。部分 Unmap、mega split、跨 child 边界与 shared root 相邻槽均不得早退或漏退表页。
12. 追踪 `PublishedTableChanges` 全路径：Running Map/Unmap/Protect、Tunnel Create/Attach/Close 与 detached close 的 unused/retired owner 必须跨 Publish→Synchronize→Retire 保活，并在 AddressSpace guard 释放后析构；completion 槽不得在安装、Remote 发布或 ack 竞态中丢失 outcome。Backing 的分批 retire 属于 6C，不用其尚未实现否定本提交的表 owner ack 边界。
13. 复核 drain cursor 的层级无关性和恢复语义：每步只做固定 work unit，`max_work=1` 可遍历最大深度/宽度树并让每个 owner 恰返回一次；中断后 cursor 必须稳定恢复。`finish_drain` 只在 owned 槽和 ledger 全空时交出 root，`TableTree::Drop` 不递归扫描或静默兜底。
14. 审查 `Box<BoundAddressSpace>`：分配失败必须自然回滚 ledger、binding/root charge 与 AddressSpace permit；Unbound→Bound 仍保持一次性发布。重算 Process/Bound 构造栈与 ELF audit，确认最大帧 `0x2390`、`0x2800` audit 上限和 `0x3000` guard 真值链未被破坏。
15. 核对过渡边界：root owner 仍唯一位于 `PoolBinding`，中间表仍由集中 raw adapter 提供；该特例只能持续到 6D。审查不得以尚未接入 funded table source 否定 6B owner 协议，也不得把 `BorrowedRoot` 或 raw token 泛化为长期所有权模型。
16. 对照 `notes/ideas/{kernel,mm}.md`、`notes/impls/mm.md` 与主计划，确认特权 work debt、锁内 preflight/锁外 funding、Remote ack 后 Retiring、6C/6D/6E 边界表述一致，没有把同步 backing retire 或 raw table supply 误写成最终机制。

## 基线证据

- `cd os && cargo test -p funded_frame -p memory_pool -p metadata_admission -p page_table --target aarch64-apple-darwin`
- `cd os && cargo test --release -p funded_frame -p memory_pool -p metadata_admission -p page_table --target aarch64-apple-darwin`
- `cd os && cargo clippy -p funded_frame -p metadata_admission -p page_table --all-targets --target aarch64-apple-darwin -- -D warnings`
- `just check`
- `just virt`

提交收口时上述相关验证通过：funded_frame 11 项、MemoryPool 14 项、metadata admission 8 项、page_table tree 30 项、eager 5 项与独立 drain 1 项 host debug/release 测试全绿，相关三个改动 crate 的 clippy 无警告。virt core 完成公共内存映射、ProcessBindMemory、Tunnel、进程监督与显式 reset 全部锚点；Kernel ELF audit 最大帧为 `0x2390`。`os/memory_pool` 独立 clippy 仍有未由本提交修改的 `result_large_err` 基线告警，不列作本提交通过证据。

## 完成标准

所有发现按严重度给出文件/符号证据、可达交错、锁上下文或 owner 方程，并明确影响的是 Pool/FramePool 双账本、metadata admission、PTE/owner 同生灭、Remote ack 保活、drain 有界性还是栈契约。任何物理与额度拆散、批量准入扩容、Commit 后分配/可恢复失败、stale Prepared 发布、shared 子树改写、ack 前表页退款、owner 重建/双重归还、未 drain 递归 Drop、AddressSpace 逆序析构或单帧越过 guard 均为阻断项。修复后重跑相关 host debug/release、clippy、`just check` 与至少 virt core；6C–6E 的未实现能力只回到主计划既有真值点，不复制新的 TODO。
