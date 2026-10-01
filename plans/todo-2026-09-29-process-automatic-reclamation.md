# 进程终止后的自动资源回收

状态：**回收结构设计轮次 1 已定稿（见“回收结构目标设计”），各 owner 剩余门未闭合，禁止直接施工**。本案属于[全项目重构](todo-2026-09-29-system-redesign.md)，拥有进程终止至资源收束的责任，不拥有各对象的清理算法。下文“推荐目标状态与 owner”起为前期推导，与定稿节冲突时以定稿节为准。

## 目标与现状

正常末线程退出、主动终止、fault、Kill、Building 放弃和未发布构造失败，均由内核自动推进本进程资源收束。用户态无需存在、存活或再次发起回收 syscall。监督服务只决定终止政策、观察结果和组织后续业务。

当前 `task/process.rs::run_termination_todo` 自动推进至 REAPABLE；`task/proc.rs::drain_batch` 的 HandleTable 和 AddressSpace 中段由用户 ProcessDrain 驱动，到 PublishDead 才发布 finalization 强根。`libprocess::supervise::Collector` 因此维护 Drain/Verify/Close 与 Busy 重试；`libexecution` 承载这些任务。它们是本案明确删除的旧责任，不是新方案前置。

ProcessControl 的查询/终态观察可独立存活；关闭 Control 不等于终止目标。共享对象和已移交到其他进程的能力不随本进程终止被撤销。资源来源及退款不由执行回收的 hart 或最后一个监督者决定。

## 回收结构目标设计（设计轮次 1，已定稿）

经 advisor 审查一次、与 DeepAsh 一轮收敛确定，执行承载为[维护执行定稿](todo-2026-09-29-kernel-maintenance-execution.md#执行机制目标设计设计轮次-1已定稿)。标“用户确认”为用户明确决定，其余为收敛结论。

### 现状事实（当前分支核对）

- 唯一首因已在 `task/lifecycle.rs::request_termination` 与末线程 `thread_departed` 线性化，可沿用；锁外副作用经 `task/process.rs::run_termination_todo` 只推进到 REAPABLE。
- 三个执行根：ProcessDrain 驱动的 managed 批（`task/request.rs` DrainRequest/DrainBatchPermit）、未发布回滚（`task/proc.rs` UnpublishedBound）、PublishDead 后的 finalization debt；均汇入 `drain_batch`。
- 已提交 Map 的完成由 remote 回调独立发布推进（`task/proc.rs` advance_retire），不依赖进程 drain；现状不存在结果义务自等待环。
- `drain_inner` 注释称 Handle 先清 object-owned region，但 Ledger 段本身仍摘 Object view；末尾 views 空断言是 ledger/view 一致性不变量，不证明 Handle 先清。

### 记录与 owner

- 进程、地址空间、Job 各一条独立记录，均出生内嵌；进程记录只发布子责任并等待，不代执行其清理。
- 一条进程回收记录取代 managed/unpublished/finalization 三根。出生先建立含记录的空骨架再接纳资源；构造失败、Building 放弃、Kill、fault、正常退出都交给同一记录，**统一从 Frozen 起步**，空阶段自然完成，不跳阶段。出生前失败只允许有界局部回滚。
- 已提交 Map/Unmap、准备回滚、结果记录/输出租约由操作记录自持进度、依赖与空间安全根，独立推进，绝不反向等待进程或地址空间 Done。

### 阶段

| 阶段 | 完成事实 | 停驻依赖 |
|---|---|---|
| Frozen | 终止提交动作：冻结首因，原子关闭新执行、新成员与新资源操作准入；此前获准操作按提交/拒绝/转交规则收束 | 无 |
| Quiesce | 不再有用户执行点或借其执行现场的同步内核路径访问；Ready/Created/Waiting 不可复活。不以混入结果义务的成员计数判断 | 各 hart 离场确认 |
| ReleaseHandles | 表已 shut，条目与预约已全部摘出并锁外移交；**不等待 LastCapability 引出的对象 Done** | 无 |
| RetireSpace | 地址空间记录 Done | 空间记录 |
| Settle | 见终态屏障与发布顺序 | 本进程私有退款结算事实 |

ReleaseHandles 与 RetireSpace 无固有先后，实现按串行推进；反例（Handle 等对象、对象等 view、view 等排在后面的 Space）证明清表阶段不能等对象退休。

### 地址空间退休

- 入口先关闭自身 mutation gate，等已受理事务与访问租约归零后才拆；active hart=0 不足（反例：外部 Builder 修改 Building 进程、他 hart completion 仍操作 ledger）。本轮选“既有事务自主完成，退休等待租约归零”，不做逐事务吸收。
- 局部安全顺序必须保留：冻结修改 → 撤销访问并完成翻译确认（可用经 VM 证明的整空间退出屏障替代逐 view shootdown）→ 归还写使用许可、释放 view 与页表 → 最后有效使用消失后物理释放并退款。先退写许可会使 seal 宣称可执行而旧 hart 仍可写；先退 backing 会让旧翻译写入复用页。
- view 不持 capability，但须持写使用/封存状态的稳定归还目标，不能只剩裸帧引用。Space.Done 只证明映射、页表与租约责任结束，不证明 backing 已全部退款。

### 终态、发布顺序与观察（用户确认）

- **终态屏障**：执行不可恢复 ∧ 表责任交接完 ∧ Space.Done ∧ 本进程私有 metadata、页表、事务残留已实际入账退款（入队不等于退款）。外域合法持有的共享 backing/capability/use 与观察壳按独立 owner 存活，不阻塞 Done。
- **发布顺序**：先冻结进程终态快照，再解除 Job 成员，再触发 Job 传播；观察者不会先见 Job.Done 后见成员未终态。Job 完成由 Job 自己的记录推进（空 Job 的 Seal 也可触发完成），不依附某进程。
- **ThreadControl.Done 不降级**：线程执行离场与 join 完成屏障分开；异步操作仍会写回退出线程的栈/结果页时，保留独立访问租约至最后访问。
- ProcessControl 保留 Weak 管理入口与持久终态壳，不保活进程本体；删除 `drain_owner` 与 REAPABLE。

### 资金归属

退款按准入时确定的资金账户入账，不以成员 Job 身份作付款来源。账户由在途工作与未退资金强持，独立于 Job 终态（反例：共享 backing 活过 Job）；维护 DRR 的账户即此资金账户。

### 剩余设计门与 owner

1. **进程（本案）**：出生准入、Start/终止仲裁、终态快照字段与发布；Settle 精确的退款类别清单。
2. **VM 分案**：结果义务与访问租约、TLB 整空间屏障、view/backing 写许可归还、退款依赖无环。
3. **capability 拼装草图**：表 shut 与锁外释放不等待。
4. **Job**：摘成员与完成可见顺序的实现与锁序。
5. **资金账户（预算分案）**：来源、余额与欠账寿命、最后退款。
6. **维护（维护分案）**：上述依赖的登记、唤醒与强根交接核验。

施工按责任链方式：本链就绪后与 capability、维护 crate 一次接入，删除三执行根、ProcessDrain ABI 与用户侧 Collector/drain 循环。

## 推荐目标状态与 owner

对外保留“尚未运行、正在运行、正在终止、收束完成”的概念；具体 ABI 名称与字段在共享 ABI 清单中冻结。请求终止受理不等于完成。完成观察只需一个具有明确定义的终局屏障，不再暴露必须继续操作的 REAPABLE 阶段。

内部由一个进程回收 owner 持有以下阶段的进度；它们是依赖顺序，不是用户需要驱动的事务协议：

1. **冻结并离场**：冻结终因、关闭新操作准入，停止 Created/Ready/Waiting，向正在执行的 hart 提交离场责任。已获提交资格的操作按其契约完成或交由同一 owner 接管。
2. **停止访问**：确认没有该进程的用户执行点能继续运行，所有不能越过的已提交操作和地址翻译义务达到对应安全边界。
3. **释放持有**：有界摘出 HandleTable entry，交给公共释放入口；需要后续推进的对象责任已经独立转交，不能由调用线程的本地变量保管。
4. **退休地址空间**：整个地址空间进入专用的独占退休流程，处理本进程映射、页表及真实 backing 归属；避免逐 region 重演 Running Unmap。与阶段 3 的依赖按实际对象图定义，不硬编码为每个对象都必须物理销毁后才能继续。
5. **完成进程收束**：本进程执行、表、地址空间和必须覆盖的内部责任已结束，从 Job 成员关系中解除并形成持久终态。跨祖先的 Job 完成传播由各自稳定责任继续推进，不等待用户代码运行。

终态屏障覆盖“该进程自身的资源责任已归还或合法转交”，不承诺销毁其他域仍持有的共享对象，也不等待所有控制句柄关闭。只因观察者持有而存在的终态壳按自己的资源来源存活。

## 出生、活性与终局并发

- 进程创建成功前取得稳定回收记录和实际承载；构造中途失败由准备 owner 进入相同的必要子集，不再依赖另一个管理进程补清理。
- 一旦终止决定不可逆，回收 owner 保持可达。未到停止访问条件时按明确依赖停驻，条件完成后由内核唤醒；没有用户线程时仍由[维护执行](todo-2026-09-29-kernel-maintenance-execution.md)推进。
- 正常退出、Kill、fault 等在同一终止 gate 仲裁唯一首因；迟到请求不重置进度，不创建第二份回收 owner。
- 与创建、Start、能力转移、内存修改的胜负点必须逐项列出；终止只能拒绝尚未获提交资格的操作，已提交结果不能丢失或伪装成失败。
- 不在进程或 HandleTable 锁内调用可跨对象展开的释放；摘取与锁外执行按公共释放契约交接。
- 关闭进程正在使用的观察集合、释放订阅、等待内部工作完成不能形成回收 owner 的自等待环。

## ABI 与消费者迁移

明确删除：ProcessDrain 调用号、结果/状态结构、max_work 参数、用户侧循环推进 wrapper、DrainRequest/DrainBatchPermit 等仅为外部批次存在的执行和仲裁结构。内部稳定游标保留其必要用途，不因删除公共 Drain 而改回无界遍历。

REAPABLE 公共电平随驱动协议移除；如存在独立诊断需求，另以只读事实表达且不得要求执行动作。ProcessQuery/终态等待继续提供终因与收束完成；位号、名称与布局直接同步实际消费者，不保留旧 ABI alias。

| 消费者 | 目标变化 |
|---|---|
| `shared/erhino_shared`、内核 syscall 分发 | 删除 Drain ABI；定义唯一公开终态完成条件 |
| `libkernel`、`libprocess` | 终止请求、查询和等待；删除 drive-to-completion 与 Busy 重试机器 |
| `libprocess::supervise`、`srv_init` | 监督任务变为观察与政策，保留真正的超时升级/重启决策，删除内核回收工作编排 |
| `libthread`、`libruntime` | 进程终局直接依赖内核自动回收；线程结果与本进程活着时的用户堆/栈管理仍按其独立契约重审 |
| 服务/FAL/验收进程 | 停止发送 Drain；失败路径不因监督者不存在而留下必须代跑的内核事务 |

Job 的创建/封口/递归终止政策在全项目任务模型中可以重审。本案只要求已决定终止的每个进程自行收束；不把现有 JobSeal/分页枚举自动冻结为未来唯一形状。

## 依赖与施工门

依赖 [capability 释放](todo-2026-09-28-capability-last-reference.md) 和[维护执行](todo-2026-09-29-kernel-maintenance-execution.md)，并需映射重构给出完整地址空间退休的原语。施工前必须进一步固定：

- Process core、Control 与回收记录的具体持有图及准入来源；消除现有 managed/unpublished/finalization 多驱动后唯一执行根的位置。
- 各阶段依赖列表、锁序、允许并行的操作、最终终态与 Job 成员摘除的线性化关系。
- “合法转交”包含哪些内部资源；哪些退款必须先于进程终态，哪些属于独立共享/观察对象而合法延续。
- 地址空间整体退休与 per-object Close 的相互约束，不能发生一方等待另一方尚未推进的循环。
- ABI 与消费者符号级清单，删除全部以持续 Drain 为目的的状态、计时器和错误分支。

## 验证与完成条件

- 终止后监督者关闭所有控制能力或自身先死亡，目标仍最终释放本进程资源。
- 自然退出、fault、Kill、Building 放弃与构造失败走同一拥有链；末线程与并发 Kill 只冻结一次终因。
- 目标包含排队消息、在途能力、WaitSet 注册、共享映射及进行中的内存修改时，全部责任被正确转交；别的进程仍持有的对象不受误收。
- 对外终态屏障与本进程退款、地址空间失效以及 Job 成员完成一致；Control 壳持续查询不保活资源体。
- 无 runnable 用户线程、多个大进程同时退出及远端确认延迟时仍保持活性与公平。
- 源码/ABI/用户封装/测试中无 ProcessDrain 驱动残留；impls 按最终实现回填，不预先宣称完成。

依项目手册完成规定检查与必要 QEMU 组合。当前只形成设计记录，无实现验证。
