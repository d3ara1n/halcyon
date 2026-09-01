# 中期设计审查报告：notes 全量与机制收敛

> 基线 `b6b05d1` 的 notes/ 快照；docs-only（切片 6D 收尾与 6E 由并行会话实施中，代码结论不作依据）。审查计划见 [`todo-2026-09-midterm-design-review.md`](todo-2026-09-midterm-design-review.md)，纪律见 [`REVIEW.md`](REVIEW.md)。每条 finding 给出证据与动作分类；triage 后行动项落地、本报告归档。

## 总体结论

方向机制整体自洽：收束分层公理覆盖全部现存对象类型且无特判残留；过渡形状（raw 路径、MetadataSponsor、单页 Tunnel、ASID=0）全部有登记的去向，未发现死胡同；唤醒所有权、单一归属、能力正交等横切不变量在文档间一致。**无范式级问题，无需推翻任何已确认方向。**

收敛机会集中在三处：①**事务协议形状缺少唯一拥有篇**——同一纪律在 5+ 篇独立重述，演进时需逐篇找齐（本报告最高价值项）；②**若干语义在多篇完整重述**（Bind 事务、execution gate 锁序、Tunnel drain 接管），违反「唯一拥有篇」纪律；③**impls/ipc.md 正在成为多机制堆积篇**，切片 7/8 将加剧，需在 MemoryObject 落地前做归属预规划。

另有过期残留与历史痕迹句式若干（立即修级）。

## Findings

### A. 立即修（文档级小改）

**A1 · impls/mm.md 内部矛盾（过期句）**
- 证据：「页表纯逻辑」节（L101）「内核当前以 `BorrowedRoot(FrameNumber)` 和 `Owned(FrameTracker)` 组成最窄过渡 token：root 在切片 6D 前仍由 `PoolBinding` 保活」；而「用户地址空间」节（L180、L189）「Bind 在锁外从绑定 Pool 取得 funded root，并将唯一 root owner 移交给 `TableTree`；`PoolBinding` 只表达绑定 authority……不与树重复持有 root」「root owner 已归 `TableTree` 持有」。
- 影响：同一篇对 root 所有权给出两个互斥答案；读者无法判断 PoolBinding 是否仍持有 root。
- 动作：删除/改写过期句。**与 6D 收口批次协调**（并行会话将同步 impls/mm.md，避免双写冲突；若主线收口文档同步已覆盖，本条消解）。

**A2 · 历史痕迹句式**
- 证据：impls/startup.md L26「**`parent_pid` 语义（A2 修复定死）**」——审查编号引用；impls/mm.md L189「旧 `AddressSpace.frames`、`alloc_map` 与 `DrainStage::Frames` 已删除」、L193「旧 `external_mappings`、按 VA 搜索、本地 `sfence.vma` 与 Drop 隐式解除已删除」——变更日志句式。
- 影响：违反「只呈现最终状态，不记录变更过程」；语义本身（现状描述）无恙。
- 动作：删除「（A2 修复定死）」标注与「旧 X 已删除」句（现状已由同段其余文字完整表达）。

**A3 · 「一页」表述易误读**
- 证据：impls/ipc.md L63「Connection 持一页共享帧」——ideas/tunnel.md 明确多页为设计形状，单页只是切片 8 前的实现现状。
- 动作：改写为「单页 backing（多页属切片 8）」类表述；可并入 6D/6E 收口的文档同步。

### B. 归属收敛（一轮文档修订完成）

**B1 · 事务协议形状缺唯一拥有篇（最高价值）**
- 证据：「提交前完成全部可失败预留，提交后不分配、不回滚」「失败零副作用」的事务纪律在 ideas/kernel.md「有界路径」、ideas/mm.md「MemoryChange 事务」、ideas/object.md「收束分层」、ideas/call.md「System Call」、ideas/tunnel.md「建立与邀请」（「提交前任何失败完全回滚，提交后不再有可失败步骤」）等至少 5 篇独立完整重述；impls 侧 task.md「reserve/commit/rollback 协议」四要素亦只覆盖 marker 事务三处。代码层共享骨架已由主线承接（AddressSpace/MemoryChange seam，6D/6E 在做），文档层却无一处声明「这是所有对象事务的公共形状」。
- 影响：任何形状级演进（如挂起项「Unmap 调用者唤醒点前移」改变完成点语义）需逐篇找齐；新切片（7–9 的 MemoryObject create/seal、多页 Tunnel）将继续复制叙述。
- 动作：ideas/kernel.md「有界路径」升格为「事务协议形状」唯一拥有节，显式列出形状要素（Validate 只形成计划不占资源 → 提交前全预留含 metadata/输出/远端槽 → 唯一不可逆线性化点 → Commit 后零分配零可恢复失败 → 失败零副作用 → 超预算完成义务转 work debt），并声明各对象事务遵循此形状、细节归各自拥有篇；各篇重述压缩为引用。**不重开「要不要抽象代码骨架」**（由 mechanism-generalization 代码审查与主线承接）。

**B2 · Bind 事务语义三处完整重述**
- 证据：「失败保持 Handle 与空壳不变；成功后绑定不可转移」在 ideas/mm.md（AddressSpace 节，最完整：lease 登记、pin、锁外准备、发布复检、终止接管）、ideas/object.md（「ProcessCreate 只创建 Building 空壳……失败保持 Handle 与空壳不变；成功后绑定不可转移」）、ideas/task.md（「绑定成功后不可替换、运输或按映射重新选择费用来源」）三处独立成文。
- 动作：确立 mm.md 为 Bind 事务语义唯一拥有篇；object.md 保留 capability 消费视角一句 + 引用；task.md 保留组装通道视角 + 引用。

**B3 · execution gate 锁序两处重述**
- 证据：ideas/mm.md「Commit 的嵌套顺序固定为 AddressSpace commit lock 在外、Process execution gate 在内。终止路径只在 execution gate 内发布准入截止……」与 ideas/task.md「Running→Terminating 的准入截止与 AddressSpace Commit 共享 Process execution gate。Commit 按 AddressSpace lock 在外、execution gate 在内的顺序取得两者……」——同一锁序与胜负点事实完整重述两遍。
- 动作：execution gate 概念与锁序方向归 task.md 拥有（impls 侧 Lock Ladder 亦在 task.md），mm.md 引用。

**B4 · Tunnel drain 接管细节三处散布**
- 证据：「ProcessDrain 的 detached close 把 `RetiringSpaceChange + LeaseRetire` 固定保存在 Endpoint 中逐批推进、冲突留 pending_close」在 impls/mm.md（用户地址空间节 Tunnel 条目）、impls/ipc.md（close callbacks 条目）、impls/task.md（退出收束节）三处各述一遍。
- 动作：收敛到一处拥有（建议 ipc.md 的 Tunnel 段），其余引用。

**B5 · Pool 负面能力清单双份**
- 证据：「无 child 枚举/关闭电平/等待电平/reparent/通用 revoke」在 ideas/mm.md（「Pool 不提供 child 枚举、关闭状态、等待电平、reparent 或通用 revoke，因此普通 close 不扫描对象图」）与 ideas/object.md（「Pool 本身没有 owner role、关闭状态、等待电平或按 ID 操作入口」）双份。
- 动作：负面清单归 mm.md（资源面）；object.md 保留 rights/role 视角的差异句。

**B6 · 用户态库分工与索引微瑕**
- 证据：framework.md 列 librpc/libsrv/libfal/libfs/libdrv，bootstrap.md 列 libelf/libprocess/ld-erhino——「装载链库 vs 运行期协议库」的分工事实上成立但未成文；README 索引「内核边界与协作式执行」与「内核内部锁、中断、唤醒与停机」两行同指 kernel/internals。
- 动作：framework.md 或 README 一句话成文分工；索引两行合并。

### C. 切片 7 前决策

**C1 · MemoryObject 的 impl 归属与 ipc.md 膨胀预规划**
- 证据：impls/ipc.md 单篇已覆盖 HandleTable、lifecycle roles、Mailbox、Notification、WaitContext+TimerQueue、close callbacks、Tunnel——README 索引中 7 个 idea 主题共享它；切片 7 的 MemoryObject（对象面+backing 面）与切片 8 的多页扩展若无预规划将继续堆积。
- 动作：切片 7 开工前决定 MemoryObject impl 归属（mm.md / ipc.md / 新篇），并顺带拆分决策（候选：Tunnel+MemoryObject 对象面独立成篇；WaitContext/TimerQueue 归属维持）。此为切片 7 文档前置，非当前错误。

### D. 观察项（不立案）

- **D1 · permit 分类枚举增长**：按对象类型逐类登记是既有纪律（计划「过渡期 metadata admission 门」），MetadataSponsor 已共享 `ProcessResources` 结构位，前向收敛结构成立；KernelMemoryBudget 未设计，不宜过早固定对应关系。切片 7–9 新增 permit 类时照常登记即可。
- **D2 · 类型状态词汇不统一**（Validated/Prepared、Installing/Armed、Reserved/Pending 等）：各 crate 阶段语义有实差，统一词汇表收益低于约束成本，维持。

### E. 驳回说明（查证后维持现状）

- **E1 · 数据搬运八篇不合并**：message/ipc/tunnel/shared-memory/runnel/buffer-queue/wait/signal 各有唯一所有权与独立引用者（shared-memory 是两协议共同引用的公共契约；wait 是全对象通用面），合并制造大杂烩。
- **E2 · ObjectSignals 与 Notification 不合并**：电平观察 vs 显式消费的语义与所有权不同，归属已清晰（wait.md / signal.md）。
- **E3 · Tunnel 与 MemoryObject 外部 interface 不合并**：tunnel.md「两者只共享内部实现，不合并外部 interface」已有独立理由（两套帧所有权 vs 消失的参与方关系/门铃复杂度）。
- **E4 · retire 路径无需再收敛**：6C 已把终止接管与 ProcessDrain 统一进同一状态机，无同步扫描旁路。
- **E5 · 异步写回双模式维持**：Map 的 UserWriteLease（创建进程资源且可能 park）与 WaitMany 的结果页复检（普通观察写回）差异已有判据成文（call.md），无需统一。

## 动作汇总

| 级别 | 条目 | 建议去向 |
|---|---|---|
| 立即修 | A1 A3 | 并入 6D/6E 收口的文档同步（与并行会话协调，主线收口时复核消解） |
| 立即修 | A2 | triage 后直接修（3 处小改） |
| 归属收敛 | B1–B6 | 一轮独立文档修订（可一次提交收口） |
| 切片 7 前决策 | C1 | 写入 memory-object 主线计划作为切片 7 文档前置 |
| 观察/驳回 | D1–D2、E1–E5 | 无动作 |

## Triage 记录（2026-09-01）

用户确认全部按推荐执行；并行实施会话已取消，无双写冲突。落地情况：

- A1、A2、A3 直接修复（impls/mm.md 三处、impls/startup.md 一处、impls/ipc.md 一处）；
- B1 事务协议形状已升格入 ideas/kernel.md「有界路径」并声明唯一拥有；B2 Bind 事务归 ideas/mm.md（object.md/task.md 压缩为引用）；B3 execution gate 锁序归 ideas/task.md（ideas/mm.md 压缩为引用）；B4 Tunnel drain 接管细节归 impls/mm.md（impls/ipc.md 压缩为引用）；B5 Pool 负面清单归 ideas/mm.md（object.md 压缩）；B6 库分工成文（framework.md）+ README 索引合并；
- C1 已写入主线计划切片 7 开工前置；
- D、E 无动作。

审查与行动全部收口，本报告与 [`todo-2026-09-midterm-design-review.md`](todo-2026-09-midterm-design-review.md) 一并归档。
