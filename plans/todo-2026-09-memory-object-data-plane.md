# 多页 Tunnel 与 Runnel 数据面

> 当前可恢复，下一自然单元为切片 8 多页 Tunnel。方向由 `notes/ideas/{mm,object,task,bootstrap,tunnel,runnel,buffer-queue}.md` 拥有；现有实现见 `notes/impls/{mm,memory-object,tunnel,runnel}.md`。本计划只安排未完成的数据面能力与已登记的库存 selftest 来源收口，不再保留已完成地基的实施步骤。

## 当前推进门

地址空间事务、构造/Start/Ready、validated ELF 与 EXECUTE authority 的前置已完成；`228b6a5` 闭合最后的启动失败广播、nofd 锚点和 Tunnel 精确失败验证，A–E 全部报告已由独立 reviewer 确认归档。历史证据见 [`Review program 档案`](archived/todo-2026-09-review-program.md) 和 [`地址空间事务档案`](archived/todo-2026-09-memory-transaction-state-machine.md)。本计划可按切片 8→9 的依赖恢复，切片 10 保持独立 selftest 来源收口职责；本次归档不表示数据面能力已实施。

切片编号 8/9/10 保留作跨文档定位，不表示可以脱离前置开工，也不允许以“到切片 10 再清理”为理由在切片 8/9 留下旧接口。每个能力单元必须连同真实消费者、失败路径、测试和旧路径删除一起交付。

## 已有地基与证据入口

以下机制已经存在，不能因历史清单仍描述早期阶段就重新实施；相关 findings 已经闭合，复核追溯见 [`Review program 档案`](archived/todo-2026-09-review-program.md)。

| 已有机制 | 实现现状 | 首审证据 |
|---|---|---|
| 平台供给、系统储备、MemoryPool、funded broker | `notes/impls/mm.md` | [`B-1`](archived/review-2026-09-memory-supply-and-pool.md) |
| Unbound/Bound、PoolBinding、root/页表 owner、deferred retire | `notes/impls/{mm,startup,task}.md` | [`B-2`](archived/review-2026-09-process-bind-page-table-retire.md)、[`E-1`](archived/review-2026-09-system-audit-03-04.md) |
| 匿名与对象来源、公共 MemoryObject、ObjectView、WritePermit | `notes/impls/{mm,memory-object,tunnel}.md` | [`A`](archived/review-2026-09-memory-transaction-unification.md) |
| 单页 Tunnel/RNL1、IPC 与当前消费者 | `notes/impls/{tunnel,runnel,ipc}.md` | [`D-1`](archived/review-2026-09-mechanism-generalization.md)、[`E-2`](archived/review-2026-09-system-audit-05-07.md) |

历史设计与实施资料：[`IPC 数据面设计`](archived/todo-2026-09-ipc-data-plane-design.md)、[`MemoryObject 统一实施档案`](archived/todo-2026-09-memory-object-unification.md)、[`系统参照`](ref-2026-09-ipc-data-plane-systems.md)。历史提交与当时验证以报告、档案和 git history 为准，不在当前计划重复列出“下一步进入已完成切片”的指令。

## 目标与边界

- Tunnel backing 从单页扩展为固定长度、多 extent 的公共对象机制；create/attach 返回一致的规范化几何。
- Runnel 消费动态映射，以 RNL2 的固定 header、宽游标与动态 capacity 形成正式数据面；RNL1 直接删除。
- shared、内核、rinlib、librunnel、RPC/FAL 的实际相关消费者与验收同步迁移。
- 收口库存 selftest 的已登记 raw frame adapter，不把它视为合法生产来源。

不在本计划引入 KernelMemoryBudget 公共 ABI、resize、COW、pager、文件缓存、MemoryLease、BufferQueue、DMA/IOMMU、动态链接、Job 资源配额或普通 Pool revoke/reparent。未来能力继续由对应 ideas/计划拥有，不能写成当前 impl 事实。

## 共同行为与容量约束

- Job、MemoryPool、内核 metadata 和设备 authority 保持正交；数据 backing 由创建池支付，各端页表由所在进程绑定池支付。
- object backing 唯一持有固定长度的 extent 与 charge；view 只持引用、几何与许可，切割 view 不切数据 backing。可由普通 close 引出最后析构的对象维持硬容量上限。
- 每个新增或扩容 owner 在接线前给出 sponsor、global/local 上限、实际存储容量、退款终点与真实析构工作量。既有内部 MetadataSponsor 是过渡 admission，不代表 KernelMemoryBudget 或完整多租户隔离。
- Map、Unmap、Protect、Tunnel lease 建立/撤销都消费正式 AddressSpace 事务接口；不重新暴露页表步骤、裸 permit 或第二套 rollback。
- Commit 前完成 backing、table、许可、Handle、输出、Wait/Remote/work 容量；Commit 后只能消费预留责任，不扩容、不返回普通业务错误。
- 同步遵守 [`外部契约索引`](../references/CONTRACTS.md) 的 Supervisor Memory-Management Fence Instruction、RVWMO 与 Zifencei；ack 前不复用 frame、charge 或 writable permit。
- R/RW/RX 分别要求 `MAP|READ`、`MAP|READ|WRITE`、`MAP|READ|EXECUTE`，Seal 需要 `MANAGE`；EXECUTE ABI 与完整验证由 capability 计划作为前置交付，不在此重复设计。

## 切片 8：多页 Tunnel 的完整调用链

Connection 继续使用正式 MemoryObject backing core，仅将固定一页扩展为调用方指定的有界长度。TunnelCreate 接受长度并返回 Endpoint、Invitation 与规范化映射几何；Attach 从 Connection 取得长度，完整准备多页 view，成功才消费 Invitation。

本单元同时迁移 shared/kernel/rinlib 与现有 Tunnel 消费者。最终接口可以处理一页的退化情况，但不得保留一套旧 ABI 或单页 adapter。RNL2 的格式替换由下一单元承担，切片 8 交付前必须明确既有消费者可完整支持的几何，不假装当前 RNL1 已支持任意动态 ring。

- create/attach/close 直接消费已收口的 MemoryChange 协议，不手写新失败矩阵。
- close retire 校验完整 lease range 与连续对象 offset，不依赖单 fragment/单 permit。
- Endpoint 不可 TRANSIT/GRANT，Invitation 维持 affine consume-on-success；Handle close、detached drain 与 peer 状态使用同一生命周期真值。
- 验证一页、多页、多物理 extent、容量边界、VA 冲突、Attach 失败不消费、双端跨 hart close 与进程 drain 接管。
- 每类失败均检查 Pool/frame/PTE/Handle/permit 守恒，Commit 后 allocator 禁用仍可完成；本单元删除全部被替代的单页几何假设与调用入口。

## 切片 9：RNL2 与真实消费者

librunnel 从 Tunnel 映射几何构造动态 slice，按 `notes/ideas/runnel.md` 的 128 B RNL2 header、`u64` 游标、动态 capacity、几何 shadow 与 Acquire/Release/EOF/Broken/门铃协议实现。

这是格式和消费者的同一次纵向迁移：librunnel、rinlib 相关入口、RPC/FAL/服务实际调用点与测试一起切换，直接删除 RNL1，不留双版本分支。

FAL acceptance 至少使用一条大于单页的数据流验证跨进程 Open 基础；相关 FAL 前置由其所属计划负责，不由 ring 实现暗自补出服务发现机制。现有 IPC 压力覆盖不同页数、物理多 extent、游标回绕模型、create/Attach/close/kill 竞态，确认容量/对齐/恶意几何输入在边界明确拒绝。

## 切片 10：库存 selftest 来源收口

这是一个有明确删除门的专项，不是集中清理其它切片残留的阶段。

| 项目 | 当前与目标 |
|---|---|
| 现状 | `os/kernel/src/frame.rs` 的 user-inventory selftest 仍使用 raw `alloc_user_order` / `alloc_user_largest` 入口；生产 backing/页表应经 funded owner |
| 目标 | 库存模型测试验证纯 claim/return；内核接线自检经正式 funded owner 验证真实来源与退款，不保留可被生产误用的 raw adapter |
| 前置/删除触发 | 事务/owner 结构收口后，确定库存测试与资金化接线测试的职责边界即可实施，不必等 RNL2 完成 |
| 验证 | host 覆盖库存 split/coalesce/重复归还；内核自检覆盖 Pool/frame 守恒；全仓确认生产与自检没有旧 raw 入口 |
| 自然顺序 | 冻结测试边界 → 同时迁移 selftest 与删除 API → host/真实启动验证 → 删除本项记录并更新 impls |

其它尚未落地能力留在对应计划，不为其保留无人消费的 adapter、备用 owner 或版本分支。

## 验证与完成标准

每个切片完成时执行相关 host debug/release、shared ABI 与 `just check`，连同本单元残留审计和 notes 更新交付；不是先改一个 crate、留适配层，再让调用者以后迁移。

- 涉及启动/IPC 后跑 `just virt`；涉及调用/寄存器边界补 `just virt-release`；涉及 Remote/drain/竞态跑 `just virt-stress`。
- 阶段收尾执行 `just acceptance`（debug stress + release core + sifive_u core）；涉及域契约另跑 `virt-hetero` / `virt-nofd`。
- 失败注入覆盖分配、输出、Attach、close/kill 和最小预算 drain；保持完整日志，按 workload/业务/reset 锚点判定。
- 静止点能证明 platform/system/user supply、Pool 与 FramePool 守恒；没有 Job 资源第二真值或生产 frame 来源旁路。
- MemoryObject、多页 Tunnel、RNL2 的 ABI、实现、实际消费者与文档一致；scope 内普通/lease mapping、seal、close、drain 全部保持 owner/permit 责任。
- 每单元删除旧 API 与过渡逻辑，提交后生成真实提交范围的未来 Review 入口；COMPASS 只导航下一能力，不重复历史实施步骤。
