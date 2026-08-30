# MemoryPool、MemoryObject 与多页 IPC 数据面

> 【当前实施计划】架构决策见 [`archived/todo-2026-09-ipc-data-plane-design.md`](archived/todo-2026-09-ipc-data-plane-design.md)，方向契约由 `notes/ideas/{task,mm,object,tunnel,runnel,buffer-queue}.md` 拥有。开始点 `adef9a7`；先完成 MemoryPool 前置，再公开 MemoryObject 和多页 Tunnel。BufferQueue 本计划只冻结方向，不在没有真实驱动消费者时实现。

## 目标与边界

本计划一次消灭四套潜在重复：用户匿名 backing、页表帧、Tunnel backing 与公共 MemoryObject 都通过一个可收费的帧取得 seam；普通与 object-owned mapping 都通过一个 AddressSpace/MemoryChange seam；Tunnel 不自建物理连续分配；Runnel 不承担记录队列。

首版范围：

- 页额度型 MemoryPool、root/child 派生、固定宽查询与进程资源绑定；
- AddressSpace 根/中间页表、匿名 backing、bootstrap payload、Tunnel/MemoryObject backing 的 charge 守恒；
- 固定长度 eager MemoryObject 的创建、Query、对象子范围映射与 `SealExecutable`；
- 多页 ObjectView、TunnelCreate/Attach 几何和 Runnel v2；
- shared、内核、rinlib、libprocess、librunnel 与 acceptance 同步迁移。

不在本计划实现：resize、COW、pager、文件缓存、原始 frame capability、通用 revoke、BufferQueue 代码、DMA pin/IOMMU/cache maintenance、动态链接、Job 聚合配额或内核 heap 通用收费。

## 共同不变量

- Pool capacity 只能经 derive 转移，duplicate/TRANSIT/GRANT 不增加额度；任一页 charge 恰属于一个 Pool。
- 物理 extent 与 charge 正交但同生灭：取得物理帧失败必须回滚 charge，归还 backing 必须同时归还二者，任何失败点总量守恒。
- ProcessCreate 在 AddressSpace 取 root 表帧前原子消费 Pool Handle 建立绑定；失败不消费，成功后绑定不可转移。
- object backing 逻辑连续、物理可多 extent；页数和 extent 数都有硬上限，普通 close 不扫描 view 或对象图。
- 普通 object mapping 与 Tunnel lease mapping 共享 planner、PTE reservation、epoch 和 retire；调用者不操作 extent 投影。
- MemoryObject Handle 消散不解除 view；Tunnel Endpoint close 只撤销本端完整 lease；stale translation 确认前不归还帧或 charge。
- Runnel 只解释自己的共享字节；任何页内字段都不是 authority 或资源回收凭据。

## 切片 1：MemoryPool 纯逻辑与对象壳

新增 host 可测的 `memory_pool` 纯逻辑 crate，定义固定容量、可用额度、派生深度、charge reserve/commit/rollback/return 与父池归还状态机。内核 adapter 持锁和对象引用，纯逻辑层不分配帧、不访问 HandleTable。

MemoryPool kernel object 提供普通可复制/运输的 pool role；`Derive` 从父池原子转移页额度，`Query` 返回总量、可用量、在途 charge 和层级。root pool 依据可信启动内存与系统储备建立并交付 init。确定并共享以下硬上限：pool 深度、单次 derive 页数、每对象页数和每 backing extent 数。

验证：host debug/release 覆盖零/溢出、父子守恒、并发 reserve、物理失败 rollback、Handle 先关/charge 后归、深度上限和最后引用归父。

## 切片 2：ProcessCreate 绑定与帧支付 seam

ProcessCreate 增加 MemoryPool 输入，预留输出、Job 成员、进程 core、pool binding 与 AddressSpace root 全部成功后才消费 pool entry；任一失败保持父池、HandleTable 与 Job 成员零变化。bootstrap 为 init 建立同形 primordial binding。

把 `TableMem` 改为从目标 AddressSpace 的 pool binding 取得单页 charge；root、中间表、mega split 与 drain/Drop 均携带并归还原 charge。把 `OwnedBacking::allocate` 改为先预留整笔 charge，再用 `alloc_largest` 取得有 extent 上限的 backing；部分 Unmap 只切 extent 几何，charge 随仍存 backing 精确切分或按页归还。bootstrap payload 取得与保留页匹配的 primordial charge。

内核静态页表、frame metadata、talc 基础储备、启动过渡和 selftest 继续走系统库存；代码中每个 `frame::alloc_*` 调用点必须被分类注释或迁移，不能留下未归属的用户请求路径。

验证：ProcessCreate 失败原子、root/table/backing 页数守恒、部分 Unmap charge 精确退款、AddressSpace drain、TableTree Drop 兜底、bootstrap payload 首次入池，以及内存不足时 charge rollback。

## 切片 3：公共 MemoryObject 与统一对象映射

增加 MemoryObject kind/role、固定宽 Create/Query/Seal ABI 和 rinlib affine wrapper。创建从进程绑定池取得固定长度、多 extent、零态 backing；Query 报告规范化长度与 Mutable/Sealing/Executable 状态。普通 Handle 允许按 rights duplicate、TRANSIT、GRANT；view 独立保活对象。

扩展 Running `MemoryMap` 与 Building `ProcessMap` 的来源意图，使 Anonymous 与 MemoryObject 只在 authority/backing reserve 上不同，共用 placement、guard、结果承诺、PTE 和 shootdown。对象 offset/length 必须页对齐且在范围内；MAP/READ/WRITE 与状态共同限制 R/RW/RX。公开 mapping 归 Process owner，可精确 Unmap/Protect；Tunnel lease owner 仍不可被普通调用解除。

把当前单 PA、单 `PreparedTranslation` 的 kernel adapter 改为按 ObjectBacking extent 投影生成有界 translation 集；planner 已有的 `BackingView::Object { object, offset }` 是唯一 ledger 表达，不增加第二张对象映射表。

验证：多 extent object 在不同 VA/权限映入多个进程、Handle 先关仍可访问、部分 object Unmap offset 保持、rights 拒绝、seal 与 writable permit/remote retire 竞态、对象最终帧与 charge 守恒。

## 切片 4：多页 Tunnel

Connection 改持与 MemoryObject 共用的 ObjectBacking core 和创建池 charge，删除单 `pa/frame` 字段。TunnelCreate 接受长度并返回 Endpoint、Invitation 与规范化映射几何；Attach 从 Connection 读取长度，完整预留多页 view，成功才消费 Invitation。两端页表成本各由所在进程绑定池支付。

create/attach/close 的 prepared transaction 必须对全部 translations、write permits、handle reservations、WaitContext 与 Remote Call 槽先 reserve；Commit 后无普通失败。close retire 验证完整 lease range 和连续对象 offset，不再断言单 fragment/单 permit。

验证：最小一页、跨多个物理 extent、容量/extent 上限、VA 冲突、Attach 失败不消费、双端跨 hart close、进程 drain 接管、peer 状态和 pool/frame/PTE 总量守恒。

## 切片 5：Runnel v2 与真实消费者

librunnel 改为从 Tunnel 映射几何构造动态 slice；按 `notes/ideas/runnel.md` 实现 128 B RNL2 header、`u64` 游标、动态 capacity、几何 shadow 与既有 Acquire/Release/EOF/Broken/门铃闭环。RNL1 直接删除，不保留双版本分支。

FAL acceptance 至少用一条大于单页的数据流验证跨进程 Open 基础；现有 carryover IPC 压力线扩展为不同页数、物理多 extent、游标回绕模拟、创建/Attach/close/kill 竞态。该负载证明实现，不作为多页设计的合法性门槛。

## 切片 6：收尾

同步 `notes/impls/{mm,ipc,call,startup,task}.md` 与 FAL 实现现状；任何未落地的 BufferQueue、pager、文件缓存和设备能力只留在 ideas/后续计划，不写成 impl 事实。

每批执行相应 host debug/release 和 `just check`；阶段收尾执行 `just virt`、`just virt-release`、`just virt-hetero`、`just virt-nofd`、`just sifive_u`，并以多轮资源守恒锚点判定。最后生成对应提交范围的未来 review 计划。

## 完成标准

- Job 代码与方向均不含资源配额第二真值；
- 所有用户可放大的帧分配都能定位到唯一 Pool charge 或明确系统储备；
- 公共 MemoryObject、多页 Tunnel 与 Runnel v2 两侧 ABI/实现/文档一致；
- 普通 mapping、lease mapping、seal、close、drain 与跨 hart retire 在失败和竞态下 frame/charge/PTE/Handle/permit 守恒；
- 全验证矩阵 debug/release 通过，COMPASS 下一自然序转入 FAL 剩余面。
