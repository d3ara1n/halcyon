# MemoryObject 实现

本文拥有公共 MemoryObject 对象与来源适配。分段 backing、View 索引、WritePermit 的内存不变量及地址空间退休组合由 [mm.md](mm.md) 拥有，capability 与纯电平等待由 [ipc.md](ipc.md) 拥有。

## 存储与业务持有

[`MemoryObjectCore`](../../os/kernel/src/task/memory_object.rs) 持 Backing 存储 Arc、MemoryObjectState、WaitQueue 与付款 Pool；状态机和来源链共用 MEMORY_OBJECT 锁。对象身份由全局 KOID 序列铸造，ObjectId、固定长度、seal 状态与写许可数都在 MemoryObjectState。public MemoryObject 有自己的 capability 身份，HandleQuery 的 related_object_id 指向 core 的 ObjectId。

core 构造从付款 Pool 取得并清零 funded frames，交给新的分段 Backing，返回 core Arc 与唯一完整范围 root BackingUse。public MemoryObject 独占 root use；最后 capability 在自身 LEAF 锁下取出 use，锁外关闭 backing 的新 acquire 准入并移交退休。已接受 View 各有独立 use，不因源 Handle 关闭而撤销。普通 Arc、观察 loan 和写许可来源强根只保活状态，不能代替物理 use；因此 core 存活不必使已无 use 的物理页继续占用。

BackingUse、区间切分、物理投影及逐段退款详见 [mm.md 的分段 backing](mm.md#分段-backing-与逐段退款)。对象 core 不再使用旧 ObjectBacking 的不可分解 Vec owner。

## 公共调用与观察

- MemoryObjectCreate 从当前进程的 funding Pool 取得 backing，按页取整固定长度；当前同步资金准备/清零仍受 ABI 页数上限与 funding extent 上限约束。Close 的工作量由预付退休记录界定，页数上限不再以同步析构为理由。
- MemoryObjectQuery 在 READ authority 接管期间读取固定宽快照，identity 仅作诊断；write_permits 包含 CPU View 与 DMA 写者的独立许可。
- MemoryObjectSeal 在 MANAGE authority 接管期间发起单向 seal，不等待；任意观察者通过 WaitOne 或显式扁平 WaitSet 观察 EXECUTABLE。

Create/Query/Seal 使用 Body/AnySpace/uaccess/CapTable。Query/Seal 经 handle::Authorization 接受非消费授权并持调用者 BodyLease；core 快照与 seal 在表锁外，不抽取 capability，也不通过临时 Arc 延长 backing 的 root use。Create 的 caps/output 失败由 handle::install 的统一回滚接管。

rights 上限为 READ/WRITE/WAIT/MANAGE/DUPLICATE/TRANSIT/GRANT/EXECUTE，没有旧 MAP 位或 role。映射来源要求 READ，可写追加 WRITE，可执行追加 EXECUTE；当前进程 mapper、Builder/bootstrap 与 Endpoint 普通 View 共用 vm::operation/AnySpace/Views 链，来源接管按各自 authority 和 rights 校验。

EXECUTABLE 是唯一允许电平，普通 Close 不新增 CLOSED 信号。seal 或最后写许可退出引发状态转换时，在同一对象锁内更新来源电平，解锁后激活出生 Work；Work 每步检查一个观察节点、锁外提示目标，不写等待结果、不代摘节点。没有等待者也完成 seal，重复 seal 在 Executable 上成功且不重新打开写授权。

## View 准备与写许可

MemoryObjectState 单向 Mutable → Sealing → Executable。Mutable 允许新的符合权限的写者；Sealing 拒绝新写授权，但允许既有 writable CPU 区间切分/缩小所需的 replacement permits。最后 permit 取消或在所属领域同步后退休，才完成 Executable 转换。CPU View 仍按 TLB 合同收束，DMA 写者须满足受信设备完成合同；CPU 同步不能替 DMA 背书。任意状态拒绝超过最大 view 权限的授权。

core 提供 ObjectViewAuthorization/WritePermit 的领域操作；当前没有独立的 PreparedObjectView 或裸 into_parts 交接层。mapper 在首份资源前出生统一 Operation/ChangeWork，借同代 handle 的存储身份到锁外，用触发调用者的 Pool 准备 Backing 双数组；这一步不接受 use。回来源表内重新核验 handle/rights/同一对象，再在 HANDLE_TABLE → MEMORY_OBJECT/BACKING 短边取得授权与独立 BackingUse，内层不收费、不析构候选、不发布维护任务。纯容量竞争退出表重采样；Close 先发生则最终接受失败，use先接受则不被随后 Close 撤销。旧/未用块由 Operation 带到所有高锁外释放；失败资源交原记录回滚，不通过 use Drop 另起任务。

许可方法借caller预付的Buffer直接追加，在对象门内先检查状态、总数和剩余槽后才修改计数，不分配临时Vec。提交前用Views::hold_writes固定来源强根；准备失败以object+输出后缀先取消未挂索引的许可，挂入后经Views归还。Reserve借同一供给数组，把许可逐份移交ledger Region；失效准备通过冻结来源归还，不能回查可能变化的 live view。退休时事务/空间 Work 经 Views 直接完成本批 backing 退款，再在 VIEWS/VM/父 Work 锁外调用 retire_write。全对象 permit 数与本空间持有来源数的分工、精确 fragment 游标见 [mm.md](mm.md#实物-view-索引与退休组合)。

## 消费者边界

`begin_pin` 只接受页对齐、checked 的 MemoryObject 区间，经 BackingAcquisition 保持分步取得所需的原 root use；具体内存责任见 [分段 backing](mm.md#分段-backing-与逐段退款)。`reserve_dma_write` 在原 MEMORY_OBJECT 门调用 `reserve_writes(1)`，取得与 CPU View 独立的 affine WritePermit，不伪造 replacement 或 VM 记录。设备只读方向不取得写许可；设备写方向在 Sealing/Executable 被拒绝。DMA owner 保持实际 core 与 permit，可信退休后归还，取消未暴露准备也通过原取消入口归还。地址暴露、静态保留及权限由 [设备实现](device.md#dma-稳定-pin-与地址交付)拥有。

core/Create/Query/Seal 与当前进程 mapper 使用同一 backing/View 链。Tunnel Connection 在两侧都关闭前持 BackingOwner，Endpoint 的普通 View 各自持独立 BackingUse；两端授权关闭不撤销已接受 View。MemoryObjectCore 与连接的普通 Arc 只保留存储，不承担 backing 的业务寿命。

MemoryObjectCore与公开MemoryObject分别先取得真实Arc布局费用；core内嵌WaitQueue/Work只计一次，Backing及其动态数组另持费用。费用末字段强持创建Pool，字段/存储loan最后释放后退款；业务root use关闭不提前退仍活的core/观察存储。同步创建成本定标、内部失败交错与金融拒绝/allocator OOM的运行覆盖由[对象链工作记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)承接。libkernel typed owner与消费者使用独立capability/权限、普通View和扁平等待来源。

## 验证入口

`os/memory_space/tests/planner.rs` 提供对象授权、permit 在同步后归还、Sealing 中切分/降权、只读重新加写拒绝、rollback、幂等 seal 与对象 offset 的 host 契约用例。`os/capability` 的 host 用例覆盖 claim/restore、撤销返还资格、权限衰减和最终持有事件。这些检查不覆盖 kernel core/Invocation 的实际锁序、硬件页表、跨进程 view 或对象等待组合；对应证据与接力只记在工作记录。
