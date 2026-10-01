# 不可信创建域的内核元数据预算

> 状态：资源预算分案，纳入[全项目重构](todo-2026-09-29-system-redesign.md)重新设计，未实施。当前 MetadataSponsor 是全局政策下的有界准入，不等于不同授权域之间的完整资源隔离。本案拥有来源、准入与退款；内核自主推进已提交工作由[维护执行分案](todo-2026-09-29-kernel-maintenance-execution.md)拥有，不再以 PM 持续调用 ProcessDrain 为前提。

## 目标与边界

metadata、对象壳及操作/退休承载必须具有明确的实际来源、准入与退款关系，不能与 page-backed storage 或 Job 管理关系混为一份真值。独立 KernelMemoryBudget capability 是已有候选形式，需在全项目重审中重新论证，不因写入 ideas 而冻结。对象跨进程或跨 Job 运输后保留真实来源，最后实际责任消散才退款。

准入应在承诺对象或操作前发生；接受后不得因调用者不继续运行、关闭源预算句柄或全局压力而丢失必成资源。长期预算不能靠 PID、当前持有者或 Job 位置重记账，也不能把普通 MemoryPool 改成带全系统成员列表的可撤销池。

## 前置、触发与自然顺序

硬触发：开放不可信 Process/对象创建域、委派独立 metadata 预算，或真实服务要求一域不能耗尽其他域的公共元数据容量。必须在开放前完成；不能用对象数量上限或自动进程回收代替隔离。按 `be6dae3` 核对，当前 `srv_pm` 持有的 `pm_domain` JobControl 只有 MANAGE|READ|WAIT，没有 CREATE；ProcessCreate 仍由可信 init 执行，故现有委托管理域并未触发不可信创建域前置。普通 Running 进程仍可按其 sponsor 限额创建被授权的对象/映射，未来一旦承诺域间额度保证，本项立即成为前置。全局 region slot 的 32 倍数值与 heap 压力观测另由[架构审计 A2/M3/M5](todo-2026-09-14-architecture-audit-findings.md)校准；独立预算不能取代字节容量证明。

与 capability、自动回收、UnitOfWork 和映射方案共同闭合实际成本图及生命周期。已有可信创建基线能否暂时保留全局政策，由当前交付承诺决定，不能沿用“FAL 完成后再处理”的旧排期。已接受清理的存储承载与执行机会需要保障；普通对象创建的隔离配额、CPU 调度份额和每步工作上界分别建模。普通进程即使不持管理其他进程的 authority，也能创建获授权的对象，因此限制终止权限不能代替分配准入。

1. 从固定规范与多种官方系统参照取证，列出对象、Handle、等待注册、通知/继续执行/退休槽和页表 metadata 的实际成本与来源图。
2. 明确预算 capability 的派生、绑定、关闭、对象转移与真实退款契约；确定全局安全储备与域配额的关系，以及哪些完成容量必须独立保留。共同核对预算资助者、Process/Job 回收 authority 和上级接管链；额度退出后仍有来源不等于仍有运行中的管理者，实际执行成本也不能用 metadata 或 max_work 冒充 CPU 计费。
3. 同次设计内核、shared ABI、libkernel、进程资源绑定与真实资源管理者的授权链；创建、失败、取消、退出和最后来源消散共同闭合，不预设旧绑定与类型名称必须保留。
4. 迁移当前 MetadataSponsor/global admission 政策，删除过渡绑定与重复真值；保留仍有独立正确性依据的结构上限。
5. 以两个独立域的压力、跨域能力转移、源退出、最后退款和压力下退休进行验证，再开放不可信创建域。

## 完成门

每个元数据和必成执行责任有可验证的预算来源与寿命；一域无法通过合法创建请求占用另一域已保证的完成容量；capability 运输不丢失或放大额度。共享 ABI、所有真实创建者和退出路径共同迁移，适当 host/clippy/QEMU 组合通过。持久契约更新 notes，完成后归档本计划。

## 对象持有与释放链所需的账户形状（收敛结论，2026-10 与 LoneCrane 对抗 8 轮、advisor 一次）

本节只定本链接入所需的最小形状，不闭合本案的域隔离完成门。

- **Pool 即资金账户与维护公平域**：`MemoryPool = {页 credit, metadata 槽计数, maintenance Ledger}`。只有受权 Pool 派生（消耗父 Pool 页额度与 metadata）创建新份额；ProcessCreate 与资源准入只引用既有 Pool，不新建账户。Pool 自身 metadata 由父 Pool 资助，root 静态，无 Account→Pool→Permit→Account 环。
- **metadata 授权**：派生时页与 metadata 预算分别划拨；ProcessCreate 需 Pool capability 的 `CREATE` right（Pool 上 CREATE 解释为“创建由本 Pool 付款的资源”，含派生子 Pool），Shell/Body 账户在创建时确定，`bind_memory` 不再改来源。不新增 FUND 位：当前只有 Pool 一个付款对象，“可付款不可派生”也无真实委托用例；触发条件见 [capability 标准 rights](todo-2026-09-28-capability-last-reference.md#标准-rights当前目标集合非永久封闭)。原 MetadataSponsor 各类计数并入 Pool，process-core permit 删除。
- **metadata 计量**：按活对象槽额度计量（字段 Drop 退槽），不宣称字节先归还后退款。
- **退款语义**：每份资源 owner 物理释放后消费自己唯一的 charge；账户 Arc 消散不是退款证明。
- **Pool 寿命**：Close Pool capability 只撤外部授权；charge、permit、已接纳 Work、child 均强持 Pool；销毁条件为无 owner 强持（不是 `in_flight=0`）。维护队列未 Done 强持 Work，finish 先 settle 再锁外释放 Work/账户根；Pool 自身收尾 Work 记父账户；Ready 链由全局维护根摘除。
- **欠账政策**：欠账不因销毁丢失——子 Pool 销毁时剩余负 deficit 以独立 debt transfer 并入父 Ledger（MAINTENANCE 锁下一次完成，不调用 `settle`、不动 `in_flight`、不伪造 grant）。公平只在现存 Pool 域之间成立；接受派生新域可绕开父域欠账，不引入层级调度。
- **无欠账**仅指物理与 metadata 不透支；维护 deficit 允许为负。
- `MemoryPool::drop` 向父链退款的深度上界（JOB/Pool 深度 32）计入原子成本，或逐级转交记录，施工时择一并写明。
