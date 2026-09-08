# MemoryObject 实现

本文是公共 `MemoryObject` 对象适配层的实现归属点。backing、ObjectView、WritePermit、地址空间事务与可执行发布状态机的内存不变量由 [`mm.md`](mm.md) 唯一拥有；Handle、运输和 WaitMany 的通用机制由 [`ipc.md`](ipc.md) 拥有。

## 实现形态

公共 MemoryObject 已接入独立 Handle、系统调用 ABI 与 rinlib affine owner，与 Tunnel 共用同一个对象 core。

`memory_space` crate 提供 `ObjectId`、`ObjectViewAuthorization`、`WritePermit`、`MemoryObjectState`、`ExecutableState` 与 `SealOutcome`。它不访问页表、物理帧、HandleTable、hart 或用户指针。

内核侧的对象 core 是 `os/kernel/src/task/memory_object.rs` 的 `MemoryObjectCore`，持 `ObjectBacking`、`MemoryObjectState`、对象自身的等待面与 metadata owner（sponsor 强引用 + `ObjectBackingPermit`）。对象身份从内核对象身份序列（`object::try_mint_koid`）铸造，与 `MemoryObjectState` 保管——身份、长度与可执行状态同属对象的逻辑状态，单一真值点避免失步。等待面按信号归属分层：`EXECUTABLE` 是对象自身的电平，与状态机共用同一把对象锁（`MEMORY_OBJECT` 秩）；Tunnel 的 `DATA`/`PEER_CLOSED`/`CLOSED` 属于 Endpoint 关系状态，由 Endpoint 各自拥有。

`ObjectBacking`（`os/kernel/src/frame.rs`）是固定长度、不可分解的对象数据 backing，堆化持有多 extent 的单 extent owner 列表（`Vec<FundedExtent>`），不内联定长 funding 容器——定长容器只适合一次性事务结果，嵌进常驻对象会使每个对象无论实际 extent 数都占满整份槽位且构造路径逐层复制本体。它不暴露 split/merge：对象 backing 由创建者绑定池一次付清，view 的切割、降权与解除不切数据 backing。`ObjectBacking::project(offset, length, &mut spans)` 把逻辑页区间投影为有界物理 span 序列，追加写入调用方缓冲，单页退化为长度为一；这是对象 view 建立 translation 的唯一几何展开入口。

## 公共 ABI

三个系统调用接入对象生命周期：

- `MemoryObjectCreate(0x55)`：从当前进程绑定池取得固定长度 backing（长度按页取整后冻结，受 `MEMORY_OBJECT_MAX_PAGES` 硬上限约束），返回完整 rights 的 Handle。rights 上限是 `MAP | READ | WRITE | WAIT | MANAGE | DUPLICATE | TRANSIT | GRANT | EXECUTE`；MemoryObject 没有 owner role，全部 Handle 是同一 capability，只以 rights 分权。
- `MemoryObjectQuery(0x56)`：`READ` 权下读固定宽快照（identity/bytes/write_views/state）。identity 只作诊断。
- `MemoryObjectSeal(0x57)`：`MANAGE` 权下单向请求可执行发布。幂等，不阻塞也不登记等待者；完成经 WaitMany 观察 `EXECUTABLE` 电平（`1 << 5`），任意数量等待者复用通用等待面。

`MemoryMap` 的 `source` 字段声明 backing 来源：零为匿名页，否则为具 `MAP` 的对象 Handle。view 权限与 Handle rights 正交——只读 view 要求 `MAP|READ`，可写 view 追加 `WRITE`，读执行 view 追加 `EXECUTE`。进程自有 view 归 AddressSpace authority，可由普通 `MemoryUnmap` 撤销；Handle 关闭或转移不撤销已建立的 view（view 强引用独立保活对象），地址空间只能按内存模型的所有权规则解除自己的 view。

## 对象状态与授权基元

`MemoryObjectState` 以固定 `ObjectId` 标识对象，并保存可执行发布状态与在途可写 view 数量。状态单向经过 `Mutable → Sealing → Executable`：Mutable 可以授权符合最大权限的 view 并取得 `WritePermit`；Sealing 拒绝新的写许可；最后一个 permit 取消或退役后进入 Executable。

seal 不保存等待者：状态机只报告「本次是否发生 Executable 转换」，由调用方（对象 core）据此发布 `EXECUTABLE` 电平并完成通用等待者。发起 seal 的线程消散不影响已发布的状态转换；重复 seal 在 Executable 上幂等成功。

`ObjectViewAuthorization` 是在对象状态锁内取得的只读授权快照，不持物理 backing；它同时携带对象的固定长度与 `view_pages(offset, bytes, page_size)` 几何判定，view 越界以对象自身长度为准，调用方不另传一份可能与对象不符的长度。实际含写权限的 view 另持不可复制 `WritePermit`。取消尚未提交的 permit 与同步后的 retire 使用不同入口，均验证 permit 属于同一 `ObjectId`；对象锁只保护状态与计数，permit 在进入 AddressSpace 事务前移出对象锁，退役也在地址空间锁外完成。

## WritePermit 所有权与锁序

permit 的真值链：对象状态机铸造 → AddressSpace 事务持有（reserved/published/retiring 三阶段）→ 同步确认后经 `view_core(object)` 归还对象状态机。`MemoryRetireSink` 只推进对象侧生命周期通知，不持有 permit。对象状态锁秩（250）低于 AddressSpace（300），因此：

- 预取：view 所有权（`PreparedObjectView`）在 AddressSpace 锁外构造，身份随之取定，Commit 路径不回取对象锁；
- 两段式 Unmap/Protect：Validate 在 AddressSpace 锁内定几何并报告 permit 多重集（含 W 的 object view 被部分撤销或降权时，存活片段是新铸造区域、各需一枚新 permit），permit 在 AddressSpace 锁外向对象取得后重入 Reserve；
- 归还：retire 批次先在锁内取得对象 core 的强引用，解锁后再归还 permit。

地址空间每对象持一枚 view owner（强引用 + `ObjectViewPermit`），保存在 fallible AVL 中。`region_count` 按 ledger Commit 的 `ObjectRegionDelta` 更新，Retire 不回扫 live ledger；同一批次每对象只保留一个 `RetiringObjectView`，强持 core 到该批 permit 归还完成，计数为零时再交出 live view owner。具体容量与退役步骤见 [`mm.md`](mm.md)。

## Tunnel 内部复用

Tunnel 的 `Connection` 持 `Arc<MemoryObjectCore>`，两侧 RW view 经 `authorize_write_view` 与写许可线性化于对象锁；对象 backing 由创建进程绑定的 MemoryPool 支付。Tunnel view 是 object-owned lease（authority 归对象，只能经对象关闭撤销），与进程自有 view 走同一事务核、同一 permit 归还路径，只在 `MapAuthority` 维度不同。

## 用户态 owner

rinlib 的 `MemoryObject`/`MemoryPool` typed owner 只接纳当前进程已安装的对应 leaf role，安全创建/派生保持唯一 Handle 所有权；该 role 的内核 close 不含 Tunnel 异步路径。显式 `close(self)` 与 Drop 因而共用不可失败 leaf-close 边界，不再伪造一个可重试错误分支；close 错误只表示 unsafe `from_handle` 契约或内核 HandleTable 不变量破坏。

## 验证入口

- host：`os/memory_space/tests/planner.rs` 覆盖对象授权、WritePermit、seal 状态推进、对象 offset、permit mismatch 与逐项 retire。
- `srv_init` core 验收（`test_memory_mapping` 尾段）：创建 → 快照 → 同对象 RW/RO 双 view → Handle 先关仍可访问 → 部分撤销 → Pool charge 守恒。
- `srv_init` capability 矩阵覆盖 Mutable 状态拒绝 RX、缺 `EXECUTE` 的派生 Handle 返回 `RightsDenied`、原 Handle 关闭后具 `MAP|READ|EXECUTE` 的派生 Handle 仍可完成 RX Map/Unmap，并通过 Seal 后 Query 状态与 RX Map 检查可执行状态；该用例没有直接 WaitMany(`EXECUTABLE`) 断言。
- 验证边界：当前公共 MemoryObject guest 用例在同一进程内执行；跨进程 view 与 Seal/WaitMany 的直接组合证据仍不足，不等同于现有对象不支持 capability 转移。A 报告保留该验证限制；多页 Tunnel/RNL2 的未来能力由数据面计划拥有。
