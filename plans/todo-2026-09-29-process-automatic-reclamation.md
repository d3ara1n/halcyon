# 进程终止后的自动资源回收

状态：**本链自动回收及后续Job管理权/后代封口修复已交付**。首次实施/组合验证由[对象链档案](archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有，后续修复、C1接通及实际证据见[审查修复档案](archived/review-2026-10-07-object-chain.md#最终复核与交付)。本案属于[全项目重构](todo-2026-09-29-system-redesign.md)，拥有进程终止至资源收束的责任，不拥有各对象清理算法。现状盘点及“推荐目标状态与owner”为历史；以最终轮次、下方修正后的Job合同和[task实现](../notes/impls/task.md)分别核目标与现状，不重复启动旧设计门。不可达capability环收集、设备/DMA退休不是当前私有资源回收承诺；新增能力由对应owner另行闭合。

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

## 设计轮次 2 收敛结论（2026-10，advisor 一次 + LoneCrane 对抗 8 轮）

闭合“剩余设计门”1、4，及 3/6 中与本案相接部分；资金账户见[预算分案账户形状](todo-2026-09-14-kernel-memory-budget.md#对象持有与释放链所需的账户形状收敛结论2026-10-与-lonecrane-对抗-8-轮advisor-一次)。

### 载体：Shell 即 Control

- `ProcessShell` 就是 ProcessControl 引用的 KernelObject，只含回收 WorkRecord、唯一终态真值、Job 成员身份、gate 与 `body: Option<Box<ProcessBody>>`。删除 `revive_control` 与终态快照复制（其无并发 drain 证明在自主回收下失效），不存在空 Control 槽与重铸窗口。不使用 Weak 指向 Shell。
- 与权限模型“ProcessControl 归零可重新铸造”一致：Shell 自身不因最后 capability 消失而销毁或终止，重新铸造只为同一 Shell 发新 capability，不新建壳；铸造资格仍由对象操作决定。
- `ProcessBody`：线程表、CapTable、`Arc<AddressSpace>`、私有 permit。外部不得强持 Body；线程/Builder/syscall 经 Shell gate 取**短** lease 访问，不跨等待或用户返回；ProcessWait/Query 只访问 Shell。
- Shell metadata 由创建 Pool 资助，最后 strong drop 退槽；Body 的 charge 由 Shell 留持，drop Box 后退。Job 成员强持 Shell 至摘除。

### 出生

- 骨架前失败：只做有界局部回滚，删 Job 占位并 try_commit，无遗留责任。
- 骨架建立后：构造者已持构造 lease，预约在 Job 锁内转为持 Shell 的成员；此后构造失败走统一 Frozen；Frozen 后只按已准入规则完成/回滚。Job.Done 覆盖已建骨架的创建。
- 施工接线中将构造 lease 明确落在 Shell gate 的 `construction_pending`：Job 成员提交与 Body 安装之间仍保留该 lease；Frozen 拒绝后续安装，构造者锁外回滚 Box 并解除 lease，回收 Work 在来源锁复检后方可 Settle，不能将暂时空的 Body 当作已退款。
- Start 不持 gate 等 Builder lease：显式排除/转移自身准入，锁外停驻；Running 提交后的线程发布链由启动记录持有，Kill 与未入队线程交接有唯一完成者。

### 阶段补充

- Frozen 关准入后，先等短 Body lease 归零，再 ReleaseHandles/RetireSpace；异步事务获准即接管自身 VM lease/结果 owner；已有 lease 的释放路径不重入 gate。
- 退出主动有界地从 Created/Ready/Waiting/调度容器摘线程以破 Shell→Body→Thread→Shell 环（保留 lifecycle.rs:20–24 责任，承载改为回收记录），不等 Arc 计数。
- Thread/departure/结果记录的私有存储纳入 Settle 实际退休；独立 owner 合法存活者在实施时逐项列明。`ThreadDeparture` 的 `Weak<Process>` 改为已登记回执短期强持 Shell，完成时有界解除。
- Settle 屏障 = Body 已拆且私有退款实际入账；Shell 不在清单内，消除“记录等自身载体退款”的自等待。

### Shell 与 VM 进入门锁序（本链接入收敛）

- Shell gate 沿用 `LIFECYCLE` 秩，只仲裁 Body/运行准入、Frozen 和 lease 计数；VM gate 为 `ADDRESS_SPACE` 秩，只仲裁入口开放、active 和曾接纳 hart。两锁不嵌套，不在 Shell gate 内取 VM gate。
- dispatch 先在 Shell gate 取得运行准入并放锁，再在 VM gate 检查入口、同步当前 epoch 并登记 active；入口已关则回 Shell gate 归还准入，不能进入用户态。
- 终止先在 Shell gate 冻结并放锁，再在 VM gate 关闭入口并取 active 快照，原样移交出生内嵌的 `RetireWork`；VM gate 是进入与封口的胜负点。此前登记的 hart 由离场清除或快照对应的 fence 覆盖，此后试图登记的 dispatcher 必被拒。离场先切内核 root、发布访问，再在 VM gate 注销 active，最后在 Shell gate 归还准入并按需唤醒回收 Work。若确需同时持锁，只允许 VM(300) → LIFECYCLE(600)，但本协议无需嵌套。
- `RetireWork` 只在入口已封、active 归零、在途事务及访问租约清空后激活；对封口时保存的 active 快照一次全量 fence 并收齐 ack/归还运输槽，才开始摘 View/use 与页表并退款。不能在激活时重读 active 代替原快照，也不能在 drain 期间把屏障推给 `RetireSink`。

### 成员回执与 Body 唯一所有权（2026-10 施工收敛）

- `Box<ProcessBody>` 一直留在 Shell gate 下，直到已登记成员回执 `member_count=0`、外部短 Body lease、回执短访问与运行准入全为零，且独占退休已完成，才 `take_body`、锁外 drop。ThreadDeparture 不拥有独立成员表，也不跨等待长期持 Body lease。
- Shell gate 的 `member_count` 是未结算 ThreadDeparture 的唯一真值；成员出生/登记在 gate 内增加，ThreadDeparture 摘成员后在 gate 内递减，任一归零转换取出预登记的回收唤醒票据并在锁外 wake。Body.members 的物理槽占用（含预留）仅供容量和稳定身份管理，不作为回收完成判断。
- Frozen 关闭外部 syscall/Builder 的**新** Body 准入；已登记回执每步可取一份短 receipt access，修改 Body.members 后立即归还，不重新申请已封闭的外部权限。Shell gate 与成员表锁允许 Shell(600) → Members(610) 的出生原子登记，VM(300) 与 Shell(600) 仍不嵌套。
- Frozen 后有界摘 Created/Ready/Waiting/调度容器责任；回收 Work 等外部短 lease、运行准入与 `member_count` 归零（arm 后在来源锁内复检），再置独占退休标志，逐步 ReleaseHandles/RetireSpace；此间 Box 留在 Shell，只有回收 Work 可短取字段。退休结束、回执短访问已退出后取 Body 并锁外析构，确认私有退款才发布 Dead。
- ThreadDeparture 完成时先摘 Body 成员，归还 receipt access，再在 Shell gate 减 `member_count` 并锁外 wake；最后解除它对 Shell 的强持。最后 drop 不展开 Shell 的回收工作。
- **Ready 发布闸门修正**：Frozen → WaitIngress → SweepMembers/Ready → WaitShell。原“先扫再等 external”会漏掉持有 Frozen 前短 lease、尚未入队的 Start。Shell 上一个原子 gate word 同时承载 Frozen bit 与 external/Ready 发布在途计数；Start 的 external lease 覆盖 Created→Ready 和实际入队，Waiting 唤醒在成员锁内以 CAS(`!Frozen`, +1) 登记发布或见 Frozen 后直接交回收，实际入队后才减计数。登记用 Acquire、入队后的递减用 Release、Frozen 置位用 AcqRel、WaitIngress 读零用 Acquire；减到零且已 Frozen 者在锁外 wake 已 arm 并复检的回收 Work。入队所需 Ready admission 必须在 CAS 前预留，登记后不得失败或等待；每线程同一时刻最多一份发布，溢出视结构错误。
- **出生预约修正**：ThreadCreate 在准备期经 Job 链与 Shell 门受理成员预约，构造、用户输出失败则由仿射预约回滚。输出完成后已预付的成员提交在表锁外必成，Frozen/Seal 不取消先受理的预约；Body external lease 贯穿提交/票据 arm，使 WaitIngress 在预约结算后才扫描。表 shut 后的 capability 公开返回最后持有事件走正常放弃，不增加 Building 无票据的反向结算路径，具体安装预约由 capability 分案拥有。
- 仅在成员状态锁内检查 Frozen 并置 Ready、锁外随后入队仍有窗口；共享 ingress 字覆盖此窗口。被 Frozen 拒绝的 Waiting 唤醒在同一成员锁内转待摘/请求离场回执，不能入队；SweepMembers 持相同锁逐槽摘，赢者唯一。已经入队被调度运行的线程由运行准入和离场回执归 WaitShell，不要求 SweepReady 再摘。

### Job 完成资金与权限（2026-10 施工收敛）

- JobCreate 接受 `parent JobControl[CREATE]` 与明确的 `Pool[CREATE]`；Job 本体 metadata 和出生即内嵌的完成 Work 从该 Pool 准入，WorkRecord 关联其维护 Account，Job 在完成前持续强持该 Pool。root Job 由 boot 的 root Pool 出资；ProcessCreate 仍独立要求自己的 Pool capability，Job 不提供隐式资金。
- Seal 只切断创建/启动准入并发布有界 Job Work；Process Dead 摘成员与 child Job 完成后各自尝试提交 Job 完成，完成判定和唯一发布责任同 JobInner 锁内仲裁，锁外 Work 发布 CLOSED 并逐层摘父表。创建 Pool 与父 Job Pool 不要求同源，关 Pool capability 不撤销已准入责任。

### 终态与发布顺序

lifecycle 锁内取不可变终因（pid、parent_pid、类型化 cause+detail；exit code 若新增须定义适用终因）→ 放锁 → 写 Shell 终态 slot（同时发布公开电平，通知扫描交 Work）→ 单 Job 锁摘成员 → 锁外 publish Job Work。

### Job（闭合门 4）

- 首次 effective seal 是不可逆准入：Job Work 在 Seal 时 publish，先以有界游标把 sealed 物化到子节点（父锁只护 sealed 置位与单步游标/单项摘取，不一次扫全表），再 Blocked 等成员/子表清空（登记+复检，变化 wake），未满足绝不 Done。
- 所有使表变空的入口（Settle摘成员、空Job Seal、member/child reservation回滚、child完成）调用Job锁内同一完成判定，谓词为effective_sealed、members/children均空且closing_children为0；两表包括Reserved，计数覆盖已经摘父表但尚未发布CLOSED的child。首次满足转Dead，Waiting凭证带出门外wake，Sealing已有稳定执行根。
- 父 Seal 自动收束已空子树（与 Create/Start 祖先判定同一事实）；已物化 sealed 的父 commit ChildReservation 时子出生即 sealed 并 publish；尚未被传播到的父由未越过它的游标覆盖。
- 公开终态由Work发布：先从parent.children摘完整节点并增closing_children，门外退款及发布child CLOSED，再回父锁减计数并判断父完成，锁外wake父Work；根无父表。CLOSED保证父表已无本Job，父不得越过在途child先完成。该顺序取代早期“先CLOSED再摘父表”；删除CompletionCursor和seal内同步finish_completion。
- root/child统一Remintable，关闭最后capability只放弃本地授权；父MANAGE仍可对尚在直接成员表中的child重新派生。显式Seal、父表/root anchor及有界Work决定Job业务寿命，不能用Terminal封闭位代替这一责任链。

### 交付边界

本链交付本地 CapTable 释放、已触发关闭对象的有界收束与私有 ProcessBody 退休；队列中 capability 环所涉资源不冒称“本进程私有资源均退款”，见 [capability TRANSFER 门](todo-2026-09-28-capability-last-reference.md)。
