# 不可信创建域的内核元数据预算

状态：Pool 双维账本、实际值/容量/scratch 付款和退款已交付；独立域硬容量隔离仍待条件触发。本篇拥有该后续的来源、准入和退款边界，维护执行由[维护计划](todo-2026-09-29-kernel-maintenance-execution.md)拥有。旧 MetadataSponsor/global Counter 候选和迁移过程见[历史快照](archived/ref-2026-10-27-metadata-plan-history.md)。

## 当前接力：硬隔离完成门

live metadata 槽是存活值/保留容量的计量单位，不等于独立堆字节、allocator 成功或跨域抗耗尽保证。外部 Weak 尾部和碎片仍占实际存储；当前实现见[mm](../notes/impls/mm.md#memorypool-额度与-metadata-准入)。

开放不可信创建域或承诺一域不能耗尽另一域的保证容量前，由资源/资金 owner 核实当前创建者、Pool 委派和 allocator 的实际成本，闭合独立域的硬隔离。不能用旧提交中“可信 init 独占创建”的快照判断今天仍未触发；CPU 份额和精确执行计量不由 metadata 额度替代。

## 目标与完成门

metadata、对象壳和操作/退休承载各有实际来源与付款；资源运输或原资助者退出不换账、不丢失已接受责任。普通创建可以在提交前拒绝，已接受的完成责任必须保留所需存储和执行机会。

硬隔离形式需从实际授权与资源模型确定，不冻结早期独立 KernelMemoryBudget 类型候选。自然顺序是核成本/来源图，明确域容量和安全储备，闭合派生/关闭/跨域存活与退款，再同步真实创建者和回收路径。完成需验证双域压力、跨域能力转移、源退出、最后退款和压力下必成退休；一域不能通过合法准入侵占另一域已保证的完成容量。

## 对象持有与释放链所需的账户形状（收敛结论，2026-10 与 LoneCrane 对抗 8 轮、advisor 一次）

Pool 是资金账户及维护公平域，ProcessCreate/JobCreate 显式消费所选 Pool 的 CREATE authority；Job 付款不隐式来自 parent Job 或调用进程。页与 metadata 分别划拨，实际 owner、charge、已接受 Work 与 child 保留来源至各自责任结束；Close Pool capability 只放弃外部授权。

维护欠账在子 Pool 结束时通过独立 debt transfer 并入父 Ledger，不复用 settle、不伪造 grant。公平范围是现存 Pool 域；新增域与层级调度的政策不由本案暗中扩张。父链归还按既有结构深度逐锁进行，物理页、metadata 和执行负债分别结算。

## 付款接入设计轮次：统一容量槽与双维派生

以下是对象链已交付的付款合同，证据见[对象链记录](archived/todo-2026-10-01-object-chain-kernel-integration.md)。旧 global/per-sponsor 多类 Counter 不作为目标限额依据。原“各类计数并入 Pool”收敛为统一 metadata 容量账本：各存储形状按其实际分配布局折算自然字宽槽，不再为每个角色复制一组限额、账户和 ABI 枚举。该槽是 live 值/保留容量的成本单位，不是对象身份数或可兑现的堆字节保证。

- Pool 同时拥有 page 和 metadata 两本同形守恒账本，`total=available+reserved+allocated+delegated`。纯 `memory_pool` 拥有通用额度算术及不可复制 token，内核区分页 charge 与 metadata charge。每本账本都有不可复用 owner key，诊断 PoolId 相同也不能跨维度提交。
- PoolDerive 固定宽请求显式给出两维额度和 child rights；任一维可以为零，整体为零拒绝。页和 metadata 同次预约、准备、输出、commit；第二维或后续准备失败撤回前面的预约。无隐式 metadata 默认额度或旧 pages-only 路径。
- root metadata 初始供给来自实际规划的 system heap 几何，扣除初始化时已占用和 root Pool/Account 的固定布局。账本不提供 allocator 成功承诺；所有实际存储仍须在提交前分配成功。后续域间硬容量/Weak 尾部/allocator 碎片隔离由本案完整完成门另行闭合。
- 独立对象按值与控制头布局计费，保留的数组/arena/表槽按容量布局计费，内嵌 Work/WaitNode/timer 包含在宿主布局中。对象退款随值销毁；Arc 外部 Weak 可能继续保留原分配块，不能宣称这时所有堆字节都已归还。存储容量 owner 不能按逻辑 `len` 提前退款。退款字段置于其覆盖的资源字段之后。
- 子 Pool 本体与独立 Arc<Account> 分别由父 metadata 付款。Account 自己持 charge，避免 Pool 析构后执行器仍持 Account 时提前退款；root 仅有 boot 固定供给特例。账本自身不持本 Pool 的 charge，避免环。
- Pool Close 只关外部授权；charge、资源及已接纳 Work 保留既定付款。传输不换账；新接收表容量与新 View/事务存储由产生这些责任的目标域付款。共享 backing 因外域操作新增的存储由引发增长的操作付款，原物理来源不变。
- 父链采用既有深度 32 的逐锁同步归还：每层固定结清页/metadata 委派与独立维护负债，不遍历子列表、不增生逐对象 Work。维护尾部析构成本仍属维护计量闭合项。
