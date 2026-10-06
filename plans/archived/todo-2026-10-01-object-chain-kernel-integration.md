# 对象持有与释放链：内核整体接入

状态：**本次约定范围已实施交付，消费者组合、有限资金/正常累计栈/完整M原子及结构检查已收口；尚未提交**。本篇是唯一工作记录，各轮次保留实施与取证过程，最终完成条件见篇末。用户已授权连续整链替换与膨胀回审；可靠物理执行计量为独立平台能力，精确全分配点注入与terminal诊断栈不属于本次承诺。

## 依据与范围

按需阅读：[系统总纲](../todo-2026-09-29-system-redesign.md)、[内存轮次 2–3](../todo-2026-10-01-memory-redesign.md)、[维护执行](../todo-2026-09-29-kernel-maintenance-execution.md)、[进程回收轮次 2](../todo-2026-09-29-process-automatic-reclamation.md)、[capability 轮次 4](../todo-2026-09-28-capability-last-reference.md)、[Pool 预算](../todo-2026-09-14-kernel-memory-budget.md)、[等待轮次 5](../todo-2026-10-02-wait-source.md)。详细目标契约归这些分案，代码事实归 impls；本篇只保留当前差距、必要实施决定、证据与接续。

交付范围仍为维护执行、VM、本地 CapTable 释放、角色即类型、Mailbox 有界关闭、私有 Body 退休、Job 自主完成、Pool 资金账户、Tunnel 普通 View 化及真实用户态消费者。TRANSFER capability 环回收不在本链；OnceSender 放弃不承诺穿透运输环。

中间态允许不可编译，开发检查不等于验收；不把将被整体删除的旧 proc/process 再修一遍，不补旧 Token/role/MAP/ObserverSink/Rearm adapter。整条责任链接通后统一编译、结构检查与验收；提交仍须单独授权。

## 收敛审视与本轮决定

此前已发现实际膨胀：连续几轮只增加基础设施与接缝，ProcessBody::try_new、ChangeWork::new、Views::commit_map 当时没有实际构造/映射调用者；为弥补准备 owner 未接入，又给每份 BackingUse 加 ReleaseWork，并给父记录加子完成/等待/唤醒，切分再预付新记录。这偏离内存轮次 2“事务推进其产生的退款，backing 负责最终残余”的契约。

本轮已收敛：

- 普通 BackingUse 只持 Arc/RangeUse，retire 产生值型 BackingRelease。ViewRetirement 内嵌游标，既有 ChangeWork/RetireWork 在自身预算中直接逐段退款；删除每 use Work、BackingRetirement done/waiter、父子 Blocked/唤醒接缝。
- BackingSplit 仅预取物理段节点，acquire/split 不再分配维护记录。唯一完整域 BackingOwner 放弃时，交给 backing 出生内嵌记录；公共 Close/出生失败仍自主收束。
- 删除未被真实 mapper 调用、依赖 use Drop 另起任务补偿的 PreparedObjectView/into_parts 与双层 prepare_view。Views 准备失败返原 use，PreparedView::cancel 取回 use，由统一操作 owner 回滚；未移交的 use/游标禁止静默 Drop。
- 保留 ledger/Views、写许可来源强根、独立 Change/整空间 Retire 与 Process→VM 完成观察：它们拥有不同事实或协议，不能为减少类型数而抹掉责任。
- 工作记录合并为这一条接续路线，不继续叠加相互矛盾的“下一步”。阶段中旧路径的删除属于该阶段收口，不一律拖到最后。

当前进程的 MemoryMap/MemoryUnmap/MemoryProtect 已切到 vm::operation，Operation 在首份资源前出生 ChangeWork，Drop 把完整准备资源交同一记录回滚；成功提交由既有线程续体等 DONE。旧 proc 中三入口、start_running_memory_change/map_anonymous_source/fund_and_complete_running 及 ChangeOutput 已删除，prepare_and_commit 也已删除。Process/Builder/bootstrap/Tunnel 与实际用户监督/Runtime 已接同链；这是消费者接线事实，系统交付仍须关闭付款、失败交错和平台组合门。

## 当前实现事实

| 区块 | 已有承载 | 未闭合接缝 |
|---|---|---|
| maintenance / timer | FIFO ticket 锁；Pool Account DRR；重复 activate/Idle/Done；最后 grant 结算且 Ready 空时 O(1) 摘环、锁外退根；侵入式 AVL timer/出生节点/epoch 提示 | 实际 ticks/量子成本定标、完整 metadata 付款 |
| capability / handle | 角色对象 Holders；非消费 Authorization/独立 use 与消费 claim/restore 分离；install 预约→锁外写回/必成出生→commit/Released | 全部 metadata permit；真实 Close/消费并发与组合验证 |
| Thread / process 子模块 | Shell=Control、Box<Body>、单原子 Frozen/ingress；成员/票据/回执；Waiting 持执行 owner；epoch/pending、一次复检、唯一 detach；Departure 强持 DONE 来源与付款 Pool | Process syscall/bootstrap 已切换；所有 metadata permit、资金尾段及组合验证 |
| 等待与角色来源 | Notification、Shell、ThreadControl、Job、Mailbox、ReplyObservation、Lifetime、MemoryObject；WaitOne；扁平持久 WaitSet，候选重读/队尾轮转/有界自摘；Runtime/Assembly/同步 Caller 与旧夹具迁移已接 | 完整到期/取消/退出组合、来源与节点实际付款 |
| Mailbox / Lifetime / Delivery | 普通 Send 先接受 LifetimeUse，Once/transit 独占 claim；成功入队后 attenuate，失败原权限返还；Receive 持消息事务，成功后出队不回退；authority/use 双计数；Delivery lastcap 退 use；关闭 Work 单项清队列 | 资金 permit、真实 IPC 与 FAL midflight 组合运行 |
| 纯 VM / 页表 | 分步 ledger/PTE、区间预约、发布 fault 分类、epoch/运输槽确认、端点摘表、SegmentMap 仿射切分与失败返 owner；OrdMap 无分配预留保证 | page_table 非生产旧 prepare API 仍待按消费者核实；完整预算和运行门 |
| 新 VM 实物 | 统一 Operation 已接公开三种 Memory syscall；预约/准备/原子 cookie/必成提交/同记录回滚；Install 按实际连续 PPN 分段；直接退款、内部表分批 fence/退款、最后边界复查；Views 保留 view 与 permit 的对象来源 | Builder/bootstrap/Tunnel Endpoint 已复用；实际运行与资金/成本门未闭合 |
| MemoryObject | 新 backing/同域 seal+EXECUTABLE；public owner 独占 root；公开 Map 在来源表项保护下接受 use/授权，Views 使 RO View 保留可重启写的来源 | 完整授权/Close/映射交错、实际付款与组合运行 |
| Job / Pool | JobCreate 显式 parent[CREATE]+Pool[CREATE]；逐层完成 Work；Pool 页/metadata 双维账本；Pool/独立 Account 各持父付款，子债务转父 | 其他对象与容量的实际 metadata 接线；完整资金/退出屏障 |
| 用户态 | ThreadStartTicket 消费语义、Control 等 DONE 后退栈；WorkQueue/Runtime/RPC 改侵入式 timer；实际 fs/pm/RootSupervisor Assembly、Caller、Tunnel/runnel 与 raw 夹具已迁移 | FAL midflight 偶发失败、完整路线/监督/资金屏障 |

当前七面 clippy、kernel check 与多次真实 core 路线已通过；既有 Sent Timeout 保留为其他组合风险。core 不替代完整退出/资金门，最新证据见末尾轮次。

## 必须保持的责任边界

- Frozen 先关入口并保存当时 active 快照；等外部 lease/ingress 后扫成员与 Ready，再等 Shell lease、释放 cap、激活 VM Retire。不能 activate 时重新读取已变化的 active 集合。
- WaitOne/来源提示只促使重读；Thread/退出执行 owner 唯一 detach，之后放执行 owner 再 request Departure。WaitSet 不能嵌套，无 WaitMany/Rearm；所有 admitted 发布走带门铃的唯一入口。
- CapTable 内只允许本篇授权接受轮次登记的短领域凭证取得；入队/提交/输出/发布/释放仍在表锁外。已完成出生不能因 shut 撤预约而反向撤销，统一发最后持有事件。线程 Pending 票据与真实消费操作同一 claim/restore 规则。
- Lifetime 的 authority/use 与普通 Arc 分离；Once Sent/Abandoned 在 core 锁内确定，Reply/Lifetime 发布和门铃锁外执行。Receive 独立持已摘出的消息责任，Receiver Close 只关准入与收束队列；不另造“Close 等 Receive”的 Blocked 分支。
- MemoryObject Close 不新增 CLOSED 电平，保留 EXECUTABLE 观察语义；root 关闭不撤已有 View。普通 View 的 use/许可必须在来源 capability 尚受表锁保护时取得，不能用存储 Arc 代替授权。
- Views 只持实物/use/写许可来源，ledger 拥有几何权限与预约。预约覆盖所有可能切界点，索引安装先于有效 PTE；相邻事务已经物化端点时归还未用准备，包括另一侧已被退休/替换。
- 数据游标完成退款后进入空表收束；ledger.complete 仅在原子末项摘表时释放预约，原 Work 继续持表帧直到额外 fence/退款，外部 DONE 晚于这些责任。这样避免等相邻事务未来无限制造空表；新映射获准前，旧空分支已摘或由相邻真实映射持有。整空间仍须索引空、根表/资金归还，Process 才能释放 Body。付款 Pool 覆盖记录最后 grant；原子退款成本仍须计入预算。

本轮实际操作契约：Map 的 guard 是字节而非页；cookie 发布只写 payload 前 56 字节再 atomic u64 release store，之后无可失败步骤或异步用户写回。结果通过普通等待续体观察内部 DONE，返回 a1=0。准备失败由已出生 ChangeWork 自动回滚，VM access 覆盖该回滚，不依赖调用者继续调用。

两索引切界在同一 VM 临界区协调，先实物后 ledger，后续失败只允许实物更细；对象来源同时由 View/permit 份数保留，避免 RO 降权后失去后续 RW 授权来源。空表分为内部预算扫描与最终两端原子复查，忽略自己的不透明 ChangeKey、保护其他预约；Rollback 也在撤预约的同一锁内复查端点。摘出的每批表留到运输最终 ack/槽归还后退款；缺低层表仍继续查上层。运输收件链已改 FIFO，防止持续新批次插队。

本轮 Process 接线决定与静态复核：

- Shell 在 Job 骨架公开前已有 construction lease；Body 出生任何失败均 Frozen，未发布的空根页可以局部退款。Job 预约回滚同时检查 sealed/空完成，不留下无唤醒空 Job。
- Builder 出生时内嵌启动记录和 DONE 来源。Job→Shell 提交 Running 后关闭 Building VM 准入；释放短 Body 借用后发布记录，等旧 VM lease/回滚，再开 entry。首次 Ready 前冻结槽终点/tid 上界，分批发布不误启动 Running 期新 Pending 票据。Frozen 唤醒 startup waiter，已接管责任不随 syscall 消散。
- 初始进程使用一个有真实职责的 Bootstrap Work，保存 ELF/出生块游标和 boot-held owner；不自行泵整个映射/退休。每次只交一个 Map 或 Write 批，使用普通 DONE 来源、内嵌节点与维护 WakeToken。payload 按 PROCESS_MAP_MAX_BYTES 分片，未移交尾段仍由 Loader 持有；prefix 字节借用清除后才退库存。runtime_gate Ready 现在表示调度环境与 Bootstrap Work 已准入；异步装载错误走 runtime_stop，全 hart 停驻，不试图回退启动 gate。现有 boot::load 注入仍覆盖 Preparing 失败，Ready 后装载 OOM/停驻须补组合证据。
- Create ABI a0=Job/a1=Pool/a2=rights/a3=output，两份来源保留。用户 loader、srv_init 所有创建/容量用例与 hammer 协议已同步；创建命令现在传 Job 和 Pool 两份 authority。旧 BindMemory 用例改为出生付款/拒绝无 CREATE/完成后退款，旧容量验收的 metadata 条件仍未接回，不能报告通过。
- 正常最后成员回执与最后预留回滚，在同一 Shell gate 检查 Running 且计数皆零并冻结 Completed；Kill/fault 竞争保留先冻结终因。VM 封口/回收发布复用锁外 finish_freeze，不把执行 owner 与结果义务混为空间可退休证明。

本轮 Tunnel/旧链删除决定与复核：

- Connection root、双侧弱关系和 Endpoint WaitQueue 共用 CONNECTION 锁。Create 显式 Pool CREATE，只公开授权；Attach 只产一个 Endpoint，rights 是 Invitation 与 Endpoint 上限的交集。Notify/Ack/Close 同域仲裁，双 Closed 锁外放 root，已发布 View 继续持 use，来源 observation loan 不保 root。源码 reviewer 未报告可证明新 bug；不替代跨进程实验。
- 源 Map 统一接受 MemoryObject/Endpoint，来源 claim 内取得 use，Endpoint 无 EXECUTE。删旧 proc/lease/BoundAddressSpace、全局 remote/deferred/work_ledger、ObjectWaitState、notify_work、lifecycle/request/retirement 与已失配内核夹具，os/wait_context、os/work_debt 从 workspace/dependency/source 删除。局部 fence/LocalEpoch 留在 vm::remote，与内嵌运输共一个 owner。
- 当前异步完成均不保留用户输出指针，ThreadResultObligation 没有真实消费者，已删计数/park/唤醒；VM lease/运行 active/BackingUse 的独立屏障保留。删 ObjectBacking、unused PreparedView 辅助与空消费 wrapper，不重新建设失败补偿层。
- Process sponsor 的 17 类计数与 PoolCore global 4096/local 1024 过渡额度已删除。页/metadata 分别从父供给划拨；metadata 统一 live 布局槽，Pool/Account 各自收费。Region/transaction 实际数组付款由末尾 OrdMap/Backing/VM 接线承接；仍未付款的 planner/操作 scratch、树节点与独立对象继续按同一责任链接线，不能用容量常量冒充资金保证。旧容量验收不恢复。
- libkernel Endpoint/Invitation 只持 capability；librunnel MappedEndpoint 持独立 MappedRegion。Create Map 失败保留 cap/Invitation，Attach Map 失败保留新 Endpoint，协议失败保留完整映射 owner。公共 unsafe SharedMemory 构造列完整对齐/RW RAM/寿命前提；safe wrapper 提供证明。部分 close 失败返回值仅供清理，不继续 I/O。
- RNL 头版本改为 3，0x2c/0x30 为双方 Start。映射/校验后 release Start，再 Notify；poll acquire 检查，Ack 后再复检，不以 PEER_ATTACHED 当协议 ready。reviewer 找到 Start 与首数据交错吞事件、Started 快路绕过终态，已修为 HEAD 后观察 Start与终态优先；增加控制交错测试。RISC-V SharedMemory::new 的可见性错误已修。
- srv_fs 获第五个出生 Pool CREATE grant，libfs 用 offer.tunnel_bytes 映射；Runnel 初始化失败 owner、init/pm/FAL 字段与监督清理已同步。libexecution 使用持久来源/Assembly，raw Tunnel geometry/race、等待与 DONE 夹具已迁移，旧 one-shot/rearm/arm_generation、Drain、role/MAP 消费者已删。这一用户闭包接线项已关闭，剩余付款与组合风险按当前表承接。

## 唯一接续路线

### 用户 Runtime：本轮收敛设计

当前授权要求施工中持续回审机制膨胀：责任链接通、失败路径或新消费者暴露新状态时检查 owner、重复真值、补偿层与旧路径；结论和删除条件记在本篇，不把每次检查变成新的工作包。

1. libexecution 的 Assembly 是实际扁平集合 owner，独占 Receive 缓冲、Wait 与 Close；独立 source 模块拥有 SourceOps/实际集合状态/Registration，Runtime 只消费登记借用及自己任务/来源。依赖为 Assembly→Runtime/source、Runtime→source，无反向构造或 facade。SourceOps 是既有内核/host 可移植 seam，合并关闭契约，不创建每域 wrapper/facade。Runtime 不 Receive、不导出 WaitSet 来源、不 Close 共享集合。
2. NEXT_TASK 已进程内单调不复用，继续用 task ID 作为每任务 cookie 命名空间；同一集合的 member token 不复用。SourceId 保存 token/task，Remove 先验所属任务，再验本地条目；未知/已删条目不调用共享 Remove。Receive 路由同时检查 cookie/token，直接装配来源可保留 cookie 0。不增加运行体 domain counter、bit 分区或额外 registration epoch。
3. 来源成员持久，删 armed/generation/Rearm 及其失败事件、每运行体接收缓存/source_ready/notified、嵌套 wait_item 和独立 run。每来源仍有一个预付 pending 提示与任务输入 FIFO，重复记录合并电平/关闭提示，不改变已有 FIFO 位置。来源注销失败仍由原 SourceEntry/额度/期限重试，Finalizing 等成功注销才退任务。Gate 收尾从实际输入状态查询，不再缓存 has_pending。
4. Assembly 每轮仅接收一个有界批次，再让各 Runtime 推进期限/来源退休/任务退休/任务；持续电平不能耗尽执行份额。只有全部本地工作不可运行时，按最早绝对期限 Wait 真实共享集合；短批次不证明集合空。单 Runtime 的 Assembly.run 检查真实绑定且无其他 Runtime，不能消费别域消息。
5. 正常 Runtime.close 证明任务/Gate/来源空后释放借用；Assembly.close 在无借用时消费集合，失败保留 owner。异常非空 Runtime Drop 或接收 owner 先消散使整个装配失败，禁止继续提供服务；逐项解除自己来源，最终集合/进程边界接管。任务业务失败仍由原运行体 Hold/退休，不触发该异常路径。选择整装配 fail-closed，避免为误用新增孤儿来源队列/第二注销 actor；该失效语义需 host 测试，服务装配按已有致命错误收束。
6. RootSupervisor 直接持一个 Assembly，保留五槽业务推进和各自 world/任务类型；来源上限组合含仍待注销成员。删 idle 集合、六目标数组、嵌套等待宏与按集合 cookie notified。pm/fs/pump/outbox 的单运行体装配使用同一实际 owner。源码 scope、失败 owner 与预算准备先闭合，再迁移正式消费者，不补旧 ABI adapter。

局部审查后的收敛修正：持续新 task/token 准入能让原退休游标永不回绕，容量不能证明旧记录活性。WorkQueue 以原任务槽链指针维护 Finalizing FIFO，每次头项同时判定“还有来源→返回一份待注销 token”“无来源/回执→退休”“待回执→回到尾部”；Runtime 将该动作与独立来源重试 timer 公平交替。删除任务/来源两套退休扫描及退休瞬间的 Retiring 状态，不增加每任务分配或孤儿 actor。成功 advance 进入 Gate 前及 registered/refused/unregistered 回调后同步任务期限，过期输入不能来自已取消期限；failed 装配在 spawn/shutdown 回调前即拒绝。SourceEntry.pending 只持合并电平，身份从当前来源表构造；SourceId/当前表双重 owner 校验的第二分支收为不变量断言，删 drop-only wrapper 与必成 adopt Result。

**内核 Process/Builder/bootstrap/Tunnel、单/多 Runtime Assembly、同步 Caller 与监督 DONE 已接同链，raw 夹具已迁移。** 本节保留已实施路线与契约，不再作为未完成任务清单；当前付款与验证由末尾轮次承接。

1. **当前进程统一 VM 操作已落代码。** 首份资源前出生 Operation/ChangeWork，预约后准备并必成提交，失败由原记录回滚；三个公开入口共用匿名/MemoryObject backing、PTE 分段、直接退款与末项摘表。prepare_and_commit 和旧公开入口/专用后半程已删。本项随第 2 项补齐运行前提与组合验证；不再回头另造准备 wrapper。
2. **Process/Builder/bootstrap 入口已落代码，资金门未完成。** Create 同时使用 Job/Pool CREATE，不消费源；失败冻结 Shell。Map/Write/Grant/BindExecution/ThreadCreate/Start/Query/Kill 与 Terminate/Abort 已用新类型。Start 等 VM 操作/回滚，分批发布冻结批次；bootstrap 用同一完成来源串接装载。已删旧 proc::Process、同步 ELF loader、旧 process.rs 控制壳/Drain/BindMemory/死亡快照和 boot 单独 Job 提交口。旧 proc VM 已随 Tunnel 删除；metadata permit 与 Pool/Body 实际退款屏障仍须闭合。
3. **Tunnel 普通 View 内核已落代码。** Connection root 与 Endpoint authority/普通 View 独立；Create/Attach 无映射，Close 无撤映射，PEER_ATTACHED 非共享区 ready。libkernel/runnel 的新组合 owner 与 Start 已落；服务/raw 夹具的完成门随第 5 项验证，不再回来为旧 lease 造 wrapper。
4. **真实用户态等待与组合。** 同步 libkernel/libprocess/librunnel、srv_init/srv_pm/srv_fs 及测试；libexecution 装配层唯一拥有顶层扁平 WaitSet，各 Runtime 登记真实来源，以 cookie 区分命名空间、成员身份路由任务，退出 Remove 本域、丢迟到身份，不另建 SourceOps 装配层。删除 Collector/drain_to_completion、one-shot/rearm、旧 request/wait_context/notify_work/work_debt 与过时夹具；统一编译与验收。

步骤是编辑依赖顺序，不各自宣称系统交付。对象 root 与 View 关闭交错、准备 OOM、部分 unmap、seal、Frozen 等待事务、跨 hart 门铃与运输槽归还都在整链责任内。

## 未闭合责任及删除条件

owner 均为本记录；当前闭包内必须完成，不以延期归档绕过正确性。

| 现状/位置 | 替代与触发/顺序 | 关闭标准 |
|---|---|---|
| Pool/Account 之外对象、节点、容器保留容量尚未收费 | 在新双维账户上按实际 owner 接出生/构造/退出，删除旧局部计数 | 强 Pool 存活不能冒充付款；live 值/容量释放后退款，DONE 屏障完整；外部 Weak 尾部不冒充字节保证 |
| STEP_UNITS/qM/qU/qT 与 cost=1、Pool 父链原子退款成本未定标 | 在既定执行模型内证明/计费，不因此增加执行器 | 实测 ticks、单步上界、公平性与退出成本验证通过 |
| 新结构安装/取消/迟到提示、输出撤销与资金失败交错的部分窗口未直接验证 | 按实际等待/消息/VM/退款 owner 复用现有设施补有区分力的证据 | 不丢失已提交责任、不复用旧 epoch、不提前 DONE/退费，不复刻已删除自检 |
| stress/release/其他平台/启动失败组合及最终结构检查未闭合 | 资金消费者接齐后选择正式聚合路线，按 REVIEW 收口 | 当前能力/边界、失败与退休链、旧路径删除、文档和必要组合证据满足交付条件 |

同步 MemoryObject 创建仍受 ABI 页数和 funding extent 上限保护，限制准备/清零成本；不再用同步 Close 作为限额理由。具体定标与 metadata 缺口仍在本表，不另开平行计划。

## 开发验证与既有审查处置

上一轮整仓 `just check` 曾停于旧 wait_context 的 5 个 TimerToken 导入错误，退出 101，未进入 kernel 类型检查：`artifacts/vm-operation-check-001.log`、本轮 `artifacts/process-builder-check-001.log`。本轮 `just clippy` 的 shared-host 通过，os-host 在旧 wait_context 本体/测试失败（TimerToken/try_register），退出 101：`artifacts/process-builder-clippy-001.log`，完整分段日志已另存 `artifacts/process-builder-clippy-{shared,os-host}-001.log`，rustfmt/diff 检查通过。libprocess host 既有尝试停于 libkernel 旧 ABI，`artifacts/libprocess-context-host-test.log`；不代表其测试失败或通过。当时没有新内核类型或 QEMU 证据；本轮内核证据见下方增量，仍无新 QEMU 证据，以下都是局部开发证据。

| 范围 | 最近保留证据 |
|---|---|
| 公开 VM 操作接线 | `vm-operation-host-003.log`：capability 19、memory_space 40、page_table 38，共 97 项；`vm-operation-clippy-001.log`：三包 host all-targets clippy，退出 0。新增用例覆盖末项检查仅忽略自身且完成前范围仍被预约。`vm-operation-host-002.log` 因 ChangeKey 尚未公开而退出 101，已修正可见性后复跑 |
| 本轮收敛 | `backing-convergence-host-002.log`：capability 19、memory_space 39（含新增 root/View 交错退款用例）；`backing-convergence-clippy-001.log`：两包 host all-targets clippy；退出均为 0。首轮测试字段名错误留存于 `backing-convergence-host-001.log`（101），已修正后复跑 |
| capability / memory_space | `memory-object-foundation-host-001.log`：19 / 38 项 host；纯层计数不覆盖 kernel glue |
| ordered_table | `vm-view-index-host-001.log`、`vm-view-index-clippy-001.log`：6 项与 host all-targets clippy |
| memory_space | `vm-backing-host-002.log`、`vm-view-backing-host-003.log`、`vm-view-backing-clippy-003.log`：OOM 后旧 reserve 切分、范围 owner、planner 与 clippy |
| maintenance / timer | `wait-foundation-host-002.log`：maintenance 19、当时 capability 18；`timer-intrusive-host-002.log`：3 项 |
| ABI / ready_queue | `wait-level-abi-host-002.log`：28 项；`ready-queue-host-test.log`：9 项及 1 文档测试 |
| 更早底层检查 | funded_frame 12 项、page_table tree 32 项曾通过；不是当前 kernel 整体验收 |

既有局部 findings 及处置保留，内核项仍须实际组合验证：

- Pending 票据过期 panic：按 slot/generation/tid 幂等拒绝；票据出生移到表锁外；跨域入队漏门铃收敛为唯一 enqueue；DONE 来源改由 Departure 强持，不依赖弱 Control 壳。
- 来源预算漏 timer 存储：预算/charge 共用字节真值；最后活动空 Account 强环不退：最后 grant 结算 O(1) 摘环、锁外退根；Departure/VM Work 独立持 Pool 覆盖结算尾段。
- Mailbox Send 参数次序/MessageHeader/PID 错误、锁内 Reply 门铃、Receive reserve 错误仍持表锁：已逐项修正；删除不可达且有唤醒窗口的 Close 等 Receive 分支。cap attenuate 增失败保权限/成功移交不伪零回归。
- OrdMap 连续预留过量、部分扩容 OOM 后 cut 再分配：修正 additional 计算/free-list 一致性，reserve_recycled 复用回收槽；真实故障中间态回归已过。
- 相邻 View 端点另一侧退休/替换：先识别现有起点/终点，再检查是否需要分裂；无内核运行证据。
- MemoryObject 未消费授权的移动/Drop 错误曾修正；相关独立 PreparedObjectView 当前已随收敛删除，防止将过时中间结构继续当下一步。
- 来源/WaitSet、Mailbox、View 与旧父子退休曾局部静态审查；本次另以既定内存设计进行架构收敛，不能把旧审查结论等同于当前最终实现验收。
- 收敛后独立局部审查覆盖 root/View 交错、public create 各失败出口/lastcap、prepare_map/cancel 返回 use、直接游标完成点、切分与退款锁序、Pool 结算尾段，未发现可证明的新 bug。源码检查确认已删 ReleaseWork/BackingRetirement/PreparedObjectView；RetireWork::wait_done 仍是必要的 Process→VM 屏障。rustfmt/diff 检查通过；没有运行 kernel 类型检查、并发 OOM 或 QEMU，因此不宣称本轮已经完成系统接入。
- 公开 VM 操作局部 reviewer 报告：PublishCursor 错误模块路径、u64 UserContext 写入 usize、ledger 先切界造成粗 View、自己的预约挡住空表且无重试。前两项及 syscall 参数宽度已修；切界已改实物先行；空表补齐内部预算扫描和 complete/rollback 同锁末项复查，并以原记录二次 fence/退款。修复后按 map→unmap→大页复用、相邻事务互跳、准备取消逐点静态复核；额外发现并修正最低层不存在时遗漏上层、远端收件 LIFO 饥饿。尚无 kernel 类型/QEMU 验证，不能以纯层测试证明这些组合。

- 本轮 Process/Builder/bootstrap 局部审查：末成员退出未启动回收、Thread Control 权限常量私有、boot payload 整段超过单事务限额，均已按上述链修复；另外自行修正启动批次纳入新 Pending、出生者 BodyLease 未释放便发布 loader，以及 bootstrap 表锁内错误释放 cap。复核方式为源码责任链追踪和 rustfmt/diff；内核依赖阻塞，未有新内核类型或 QEMU 证据。

本轮验证增量：`tunnel-kernel-check-003.log` 的 just check 退出 0；`tunnel-clippy-003.log` 的 shared-host/os-host/kernel-target 全通过，user-host 停在 libexecution 旧 ABI 以及已修的局部 clippy 警告。`tunnel-runnel-host-004.log` 23 项全部通过；此前 001 仅因 mock 未增加 Start 存储失败，002/003 的 21 项通过尚未覆盖 reviewer 新交错，不用旧成功替代最终 23 项。`tunnel-user-build-003.log` 越过 libkernel 后停在 libexecution/libthread；libthread WaitOne/票据可变借用已据诊断修正，目标复验留后续。最新 targeted host clippy、静态 diff/格式证据见下面接力补记。

本轮 Runtime 接力补记：

- 用户要求持续检查机制膨胀已立即生效并写入本篇。结构审阅覆盖了 owner、普通输入/退休两个责任域、来源身份、成功/失败 Gate、期限、共享集合关闭与异常 Drop；不是按状态数最小化。已删 armed/generation/Rearm/SourceEvent.error、每 Runtime records/source_ready/notified/wait_item/run、SourceSet 分离 trait、Gate.has_pending、冗余 pending 身份/drop-only wrapper/必成 adopt Result、两套退休 key 扫描与瞬时 Retiring。保留真实 source/输入额度、唯一 pin timer、业务 Hold 和公平队列。
- advisor 给出的异常 Drop 孤儿问题采用整装配 fail-closed：正常失败仍由原 Runtime 退休，异常 nonempty Drop 不再继续服务。cookie 复用现有全局 taskId，SourceId 带 task/token；没有新的 domain counter 或位宽分区。Assembly 同步组合与 Runtime 构造的初版依赖环已拆为 source 提供 Registration、Assembly 组合、Runtime 消费。没有每域 SourceOps facade。
- 局部 reviewer 指出退休游标可能被持续新准入饿死、成功 Gate/回调取消期限仍可能超时、failed 装配的 spawn/shutdown 漏检；均已修并增加独立契约用例。最终 `runtime-assembly-host-012.log` 29 项通过，含多 Runtime 一次接收分流、重复提示合并、迟到 cookie、异常 Drop、取消期限、新完成不能插队旧退休、持续 ready 保留任务量子；`runtime-assembly-clippy-003.log` host all-targets 退出 0。结构缩小后 Runtime.close 已不触发 result_large_err，旧豁免已删。
- 正式消费者：Outbox/Dispatcher/FAL retirement 的旧 rearm 标志开始删除；Root source 生命周期仍由原表持有。Pump 构造/失败 carrier 已改实际 Assembly，budget 只在接收 owner 计 receive buffer；先清空 Runtime 后关闭集合。Raw-before-assembly 与 Ready-assembly 的失败用所有权枚举持有同一集合，没有孤儿 actor。**尚未目标组合验证**，不能登记这些 consumer 完成。SDK role→kind 的相关消费者/fixture同步，libthread Start 大错误保留 prepaid owner 的返回契约。
- 最新 `just check`：`runtime-assembly-kernel-check-001.log` 退出 0。完整 `just clippy`：`runtime-assembly-full-clippy-003.log` 的 shared-host/os-host/kernel-target/user-host 全通过，user-target 停于下述旧 ABI；完整分段日志另存见当前 artifacts。最新 `just build_user`：`runtime-assembly-user-005.log` 退出 101，libexecution target 越过，主要剩同步 RPC/FAL Caller 的 WaitItem/Cancelled/item_index，ProcessDrain supervise/management，正式 Runtime factory/run 等 consumer 接缝。Runnel 的目标 Cancelled 分支已按新 reason 删，但删除后未重跑完整目标构建；无 QEMU。
- 上述正式接线已由本轮推进：Caller 持久成员、监督 DONE/Verify/Close、RootSupervisor 单 Assembly、fs 双运行体共用 Assembly、pm/Pump 实际接收 owner 均已落代码；旧断点不再是当前下一步。最新验证和唯一接力如下。

## 完成门与后续

### 本轮真实消费者契约修正

- ProcessShell 的 TERMINATED 业务终态用共享 DONE 位表达，屏障仍为已提交回收与 Body/VM 退款完成；死后 Control 仍提供稳定查询，来源不因业务终态置通用 closed。Job 保留所属分案的 CLOSED 完成契约；ThreadControl/Process DONE 与通用来源关闭不混成 alias。修正此前消费者按 DONE、内核按 CLOSED 的接缝，同步公开掩码与发布。
- MailboxMakeSendOnce 返回 sender/ReplyObservation 两项；封装必须接收完整 HandlePair，并以 typed ReplyPermit 交付两个 owner。Request 持观察权、Packet 运输一次性回复权；Unsent 撤回、成功与 Sent abort/Drop 都收束观察权。Caller 把 ReplyObservation 的 ABANDONED（当前 PEER_CLOSED 位）作为真实来源，不能丢掉它或只等待 ReplyPort。
- Dispatcher 同步接观察壳，但不追加第三个每调用来源：原 Writable 槽在实际 unregistered 回执后转为 Abandoned，Closed 槽保留；binding role 是槽阶段唯一真值，来源身份逐条校验，观察权随 PendingCall 到两个来源收束后释放。Request 删除 reply_attached 重复 bool，由仍持观察权表示 Unsent 已准备回复。
- NotificationSignaler 仍不可等待。FAL watch 的客户端退出观察需要正式只读来源，新增 NotificationObserve：从 Owner READ 或 Signaler SIGNAL 授权派生 NotificationObservation，只观察同一 core 的 CLOSED，无业务 authority、关闭不影响 Owner；复用同一个核心 WaitQueue/Work，不另建 Lifetime/终态计数或第二扫描器。该接线属于当前 watch 失败/退出责任闭包，创建/运输/校验与 watcher owner 同步迁移，旧 Signaler WAIT/CLOSED 路径删除。
- 纯电平的空候选批不证明真实来源未就绪。最终超时裁决直接复核 Notification/服务/取消/回复源；陈旧 READABLE 后 take 的 ObjectNotAvailable 是重试。Unsent 的取消在再次投递前检查，Sent 回复接受后结束；完整批次按阶段裁决，不能让持续 WRITABLE 压过取消。整个调用使用同一个绝对 Deadline。

### 本轮开发证据与结构检查

- `consumer-done-kernel-002.log`：`just check` exit 0。`consumer-done-clippy-003.log` 的 shared-host/os-host/kernel-target/user-host 通过；完整 user-target exit 101，当前保留分段为 `consumer-done-clippy-user-target-003.log`，停在 test_hammer 等旧 Tunnel/WaitMany 接口。
- `consumer-services-check-003.log`：fs/pm 目标检查 exit 0；`consumer-services-clippy-003.log`：libkernel/libprocess/librpc/libfal/libfs/librunnel 与 fs/pm 目标 Clippy `-D warnings` exit 0。本轮支持角色 Close 失败原样返还 owner 的 Clippy 豁免各有原因，不通过失败路径装箱增加分配；不等同于整仓 lint。
- `consumer-done-host-001.log`：现有 libfal 39、libprocess 4、librpc 9，共 52 个 host testcase 通过；不覆盖只在 RISC-V 编译的 CallWait/Subscription 或新 NotificationObserve 的真实 syscall。没有新增与实现互抄的模拟消费者。
- 最新 `consumer-done-user-009.log`：整仓用户构建 exit 101，fs/pm/公共库已越过，剩 srv_init 旧 WaitMany/Drain、旧监督进度断言、旧 Runtime fixture、旧 nested/Rearm 测试、raw Tunnel；test_fal 剩单来源 WaitItem/item_index。该轮后只修正 init 局部 root_memory_pool 变量遮蔽调用，未重复整仓构建。MakeSendOnce 所有实际 raw 输出点已接完整 Pair，并显式释放不需要的观察项；typed Permit 的调用点和 Subscription 可变等待同步。
- 主代理交付前检查覆盖公开掩码/输出宽度、两项 capability 失败回滚、Request/Unsent/Sent/Drop 与 ReplyCleanup、Dispatcher 来源身份和槽转换、Notification 观察无业务 authority/单扫描器、FAL watch 的注销后释放，以及正式装配单接收/关闭责任。删除重复 reply_attached、旧 rearm/error 标志与 init 伪 work_done 日志；runtime 实现篇删除重复/过时执行器真值，链接其 framework owner。rustfmt 与 diff 空白检查通过。以上为当前代码的局部结构检查，不是完整责任链交付或固定提交独立 Review。

### 唯一接力与未覆盖责任

1. 同一消费者闭包继续迁移现有 init/race/public_ipc/time_checks、test_fal/test_hammer 的实际接口：单来源改 WaitOne，多来源复用既有 WaitSet；nested/arm_generation/Rearm 用例按新公开契约替换，不补旧 facade。直接 Drain 剧本改等待 DONE/Query 核验，旧 drain_work/work_done 断言删除。init 的独立 Outbox fixture 使用 Assembly，并清除 SourceEvent.error/Requests.rearm。raw Tunnel 用真实 MemoryObject/Map/Attach/Endpoint 与映射 owner，按现有场景保留失败/退出覆盖；不另造运行体或并行测试计划。
2. 然后重跑完整用户构建与 `just clippy`，接已有 QEMU 路线覆盖真实 Process DONE/Job CLOSED、ReplyObservation 放弃、取消与到期交错、NotificationObserver 权限/关闭及多 Runtime 组合；只读类型与纯 host 证据不能替代这些运行门。原内核自检替换、metadata permits/Pool 父链成本与资金屏障仍由本篇第 5 项承接，NotificationObservation 的实际额度也包含其中。
3. 任务仍未满足整链完成条件，不归档、不登记系统能力交付。上述残留的 owner 是本对象链迁移，触发为当前真实消费者接线，保留至现有消费者和验收按新接口闭合；删除标准为旧 ABI/路径全删、失败 owner/单接收责任完整、完整 target/host/必要 QEMU 通过。后续从这些断点继续，不重复已通过的局部检查。

### 旧验收消费者清理与真实启动组合轮次

前节接力第 1 项的源码迁移已完成，第 2 项的构建/lint 已越过；当前下一步由本节接力覆盖，历史开发日志保留原失败事实。

- init/race/public_ipc/time_checks、test_fal/test_hammer 的单来源使用 WaitOne，多来源用现有 WaitSet。旧 WaitItem/WaitMany、Drain 调用、REAPABLE 公共位与 arm_generation/Rearm 均已删除；原集合场景改检查持久电平、消费后的陈旧提示淘汰、Remove 旧 token 与 nested WrongObjectType，不补兼容 facade。非空集合 Kill 改观察自动退休 DONE/Dead/Killed 与退款；最后 Control 消散的派生用例保留尚在组装的 Builder，避免依赖已完成进程长期保留 record。
- raw Tunnel 使用现有 Invitation/Endpoint/MappedEndpoint。原跨线程 Close、Close/Attach 竞争、固定 VA 复用、双方通告与混合尺寸访问保留；Attach 的 bytes 经本夹具 payload 显式传递。thread suite 的来源 MemoryPool 经现有 Grant 交付，失败依 GrantOutcome 关闭保留项。test_hammer 直接依赖 librunnel 的映射 owner，没有新增运行体、adapter 或库组件。
- Outbox 独立 fixture 使用 Assembly + Registration，Runtime 先关、Assembly 后关。management 的两个既有 host 集成夹具改 SourceOps/Assembly 与真实查询计数，保留来源退休先于 Query、关闭失败接续、逐步 Job 页游标/退出资源返还及无提示时实际 Ready 在到期赢得裁决的独立契约；不照抄实现预期。
- 同步取消夹具第一次预置取消以证明 Unsent；第二次清空该电平、用真实 CallOperation.advance 确认 Sent 后再发布取消，避免以预取消的空 mailbox 偶然进入 Sent。后者已编译，最终长路线见下面边界，尚未稳定运行到该处。

真实执行暴露并已修正以下责任/身份接缝：

1. 固定 64 extent funding 值在 memory_map/initial_map 的大请求栈重复传播。Backing::fund 统一匿名、MemoryObject、bootstrap 的 funding 准备，不改 quota/释放 owner，不提高限额。旧 ELF 单帧 `0x46e0/0x3c80` 超过来自 guard 的 `0x3000`；收敛后审计最大为 Backing::fund `0x2970`。此值是单帧证据，不是累计调用链栈峰值证明。
2. 异步等待完成只写 a0/a1，未推进 sepc，导致相同 ecall 用返回值 0 重执行。GDB 002/004/005/006 证明用户原 MemoryMap 参数正确、保存正确、续体完成后清零并回到旧 PC。set_result 在 detach/取走唯一 Entry 后推进 4；重停驻和退出不推进，审查确认只修改归属当前执行 owner 的 frame。
3. uaccess 经校验与空间锁稳定映射后，用受保护 SUM 下 RV64 LB/SB 整数地址循环，不为合法 VA 0 构造 non-null Rust 引用。空复制不访问，cookie 发布以 RVWMO FENCE RW,W + 对齐 SD 保持 release；没有增加软件遍历复制路径。
4. Building ThreadCreate 的 Control 实际属于 caller 表；launcher 现在先以 Capability 接管，再 Grant 到 child，写真实 child-local 输出到出生块。失败保留/关闭原 caller owner，成功才交出；启动失败仍由 Builder/DONE/Control 清理接续。Startup host 用例同步版本 2、有效 generation 与页对齐几何，仍只验证父/孙身份。
5. RootSupervisor 集合容量改 u32 表示上界（由 query/member ABI 决定），不新增数量政策。bootstrap root Job 使用 Job-owned JOB_RIGHTS，不借 ProcessControl 掩码；Process 监督请求去掉不支持的 TRANSIT。Mailbox 唯一 Receiver 与 core 共用 Header/KOID，保留各角色 Holders，删除无独立领域含义的 Receiver header，使 Sender.related_id 对应 Receiver。
6. Runnel Consumer 先保存实际 acquire 观察到的 Start，再裁决数据/EOF。原先空流 EOF 提前返回，丢掉 start_observed，导致合法 pre-Start EOF 的 Start 持续 Busy 后 ServiceClosed。复用 Host pair 增加独立协议用例，24 项通过，真实 pre-Start EOF 场景也已通过。

### 用户验收迁移轮次的验证与历史断点

- `consumer-acceptance-user-003.log` 默认用户构建 exit 0；各次 virt 同步构建也通过。`consumer-acceptance-check-002.log` 的 just check exit 0。最终 `consumer-acceptance-clippy-014.log` 七面全部通过（含 user-stress/user-fp）；完成后不重复。
- `consumer-acceptance-host-012.log`：libprocess management 13 项、execution_recovery 4 项、observation_runtime 4 项，共 21 项通过。`consumer-acceptance-runnel-host-002.log`：24 项通过。目标源码类型检查与 host 测试覆盖的边界分开，不把它们称为平台交付。
- 定向 reviewer sub-32 源码/ELF 审查未发现可证实的安全、并发或 owner finding，确认等待 PC、SUM 字节循环、funding 失败释放、Control Grant 与 Receiver header 去重。未补 OOM 注入、并发退出实测；后续 uaccess release cookie 汇编由主代理核对，不能把定向结论扩大到全系统。
- QEMU 开发证据已越过启动、Clock、公共映射、Pool/Job 授权、派生付款与失败回滚、真实 fs/pm/FAL 装配、discovery、属性/流/Move/Delegate/Open、pre-Start EOF。`virt-015`（150 秒）进一步到 resource quota 恢复、Watch、affine Take、provider shutdown 和 Open terminal，停于已修正的取消夹具旧断言。
- **组合尚不稳定且未通过**：`virt-013/016` 在独立 midflight cancellation watcher 的 Lookup 偶发 Sent Timeout；016 的永久诊断明确 `Transport(CallError { phase: Sent, cause: Timeout, owner: ReplyCleanup… })`，父方随后 phase deadline。`virt-014/015` 可以越过这处，不能据一次越过关闭该缺口。最新 `consumer-acceptance-virt-016.log`、`failed-acceptance-20261004-145825-23205.log` 是冻结参考；均 exit 124。stress/release/sifive_u 与后续监督/资金屏障运行尚未覆盖。所有有界/调试 QEMU 及 launcher 已退出，无保留服务。
- 该轮断点由后续授权接受与验收清理承接：已有 FAL midflight watcher/Copy 场景曾出现 Sent Timeout，必须按真实来源/续体/业务责任核对，不能提高 deadline 或补运行体掩盖。后续 core 已多次完成，其他路线组合风险继续保留。metadata 付款与新结构失败/退出验证由本篇承接，不复建已删除的内核自检设施，不把 FAL 全栈现有结构当作目标范本。
- 本轮最终结构检查核对公开 ABI/掩码、父/子句柄身份、失败与退出 owner、资金准备的实际释放、无新 Work/嵌套等待、代码中的旧路径清除、static impls 和当前导航；rustfmt/diff 空白检查完成。Review 固定提交登记仍待代码交付与独立 commit 授权，本轮未提交。

### 授权接受轮次：非消费操作与消费 claim 分离

真实 watcher 重现新增 `Unsent/System(ObjectBusy)`，开始时剩余约 109 秒。原因不是邮箱容量：普通 Send 在判断 Sender/Once 前调用 Invocation::claim，claim 独占表项，阻挡同槽 Query/Wait 来源登记。当前 FAL Grant.clone 只共享同一 Sender owner，正常并发被无关的所有权移交状态串行化。定向 advisor 的一次调查确认应纠正内核机制；仅 SDK Busy/yield、强制 Duplicate 或共享 Sender 互斥均不能闭合查询/观察/发送责任，也不作为目标。

目标接受契约：

- 非消费授权在 HANDLE_TABLE 锁下验证 live handle/rights，取得只读身份/rights/观察存储快照；需要领域责任的操作在同一锁域内执行短、无用户访问的准备，取得 owned use/派生产物，随后释放表锁进入业务仲裁。明确允许 HANDLE_TABLE(100)→MEMORY_OBJECT(250)/BACKING(350) 的 Map use 准入，以及 →OBJECT_WAIT(500) 的 Lifetime 接受/派生；不泛化为任意业务动作，不反向进入表，不执行 install/用户访问/维护发布或释放。当前 prepare 回调取得责任后直接成功返回，禁止在表锁内 Drop 部分成功的 use/派生产物。准备失败原 capability 不变；不存在裸 Arc 跨锁后才接用量的空窗。capability 所属计划的旧“任何领域锁都禁止”草图已同步修正。
- Authorization 不拥有或复制 Cap，不计入 capability 数量，也不安装新表项；BodyLease 只覆盖调用者已接管执行/输出责任。普通 Sender Send 在该域取得 LifetimeUse，成功随 Message/Delivery 移交，失败归还；MakeOnce 在该域取得独立 LifetimeOwner。MemoryMap 在该域取得授权/BackingUse。纯 Query/WAIT 的 Arc 只保存观察存储，无业务 authority。
- table Close/shut 先使槽不可用，则不能再接受操作；准备先完成，则已有 owned 责任不被随后 Close 或 move 追溯撤销。业务 core 的 Receiver shut/对象关闭/容量仍可独立拒绝尚未提交的操作。表 shut 拒绝新的 get/claim，原 ticket 的 restore/commit 仍按既有规则消费。
- Once Send、Invitation Attach、Builder Start、启动票据与附件/Grant 的真实移交保留 exclusive Invocation/ClaimTicket；这些真正 Claimed 的槽仍可 Busy，禁止给预约补影子授权。普通 Receiver 的 Peek/Receive 只读/非消费授权不 claim；Receive 事务排他由 Mailbox core 所有，domain Busy 保留。
- 同轮核对并接通全部 Invocation 与跨锁 resolve/get：普通 Mailbox、Mint/MakeOnce、对象 Query/观察、Pool/Job 创建、Process Query/Kill与非消费 Builder 操作、MemoryObject/Endpoint控制与Map、Notification Observe。来源接受和使用的两侧同步，删除旧非消费 claim，SDK 不新增笼统 Busy 重试。验证以同槽 Send/Query/WAIT、最后 Close 与已接受 use、真实消费 claim 排他/恢复三个独立契约为先，再回原 FAL 场景；此前 Sent Timeout 不因本发现自动关闭。

本轮实施与证据：

- `Authorization::{accept,accept_with}` 已接全部非消费 Invocation 入口，只余 Once Send、附带 transit、Grant 来源、Builder Start 与 Invitation Attach 的五处 claim。非消费 Builder 操作仍持目标 Building BodyLease，Shell.begin_running 在同一 gate 复核 ingress/reserved/member；VM 异步 Map 则由 VM access 屏障约束入口，未新增 Builder 互斥层。Receive 最后 Close 可立即关 core；已摘出的消息由事务独占，失败时仅在仍开放的 core 回插，关闭时锁外释放，不造第二清理队列。
- `authorization-check-001.log`：just check exit 0。`authorization-clippy-002.log`：七个分面全通过。`authorization-cap-host-003.log`：20 项 host 通过，新增 shut 后未 drain 的 Occupied 槽拒绝 get/claim；旧消费 claim/restore、撤返还、收窄与 zero policy 覆盖保留。001/002 的失败分别是旧 StaleHandle 预期与最后一处预期未同步，现已按表关闭契约更新，不删除失败记录。
- 现有公共 IPC fixture 增加同一无 DUPLICATE Sender 的两线程 Send/Query/WAIT 登记、满箱 backpressure、完整消息与 Lifetime 收束回归，在正式服务启动前运行；无新运行体/测试服务。`authorization-virt-001.log`、`failed-acceptance-20261004-162323-36610.log`：回归通过，FAL midflight/Copy 取消与 provider 关闭均越过，最后停在旧 WaitSet 空候选批预期（Ok([]) 误当 ObjectNotAvailable），exit 124。该预期已修，不能把首轮局部成功写成完整路线通过。
- `authorization-virt-002.log`：再次越过同槽回归与 FAL midflight，WaitSet 持久/陈旧/非空退休/nested 拒绝通过，随后停在 threaded_receive 将“发送 Sender 身份”和“附带 OnceSender 身份”混成一项的旧断言，exit 124。修正为身份独立，Delivery 对应 header.sender_context_id；没有改内核身份语义。共享 Sender 用例也核验 query 的实际 Sender id 与每条消息 header/Delivery 对应。Notification 夹具去掉 Signaler 不支持的 WAIT。QEMU runner 的已有 panic 拒绝项“Panicking in”同步列入终态收割，避免已确认用户 panic 等满硬超时；并将共享 Sender 回归登记为正式公共锚点。
- 定向 reviewer sub-35 源码审查未发现可证实代码缺陷，覆盖接受/Close、领域凭证取得与错误析构、Once/附件失败、Receive shut、Map root/use、Builder ingress/VM 屏障和单向锁序；指出的旧 Receive/mapper 文档已修。主代理同时核对上述 gate 与事务 owner，impls 的 IPC/MemoryObject/MM/task 已同步。此结论只覆盖本轮源码交错，不能替代整链运行/资金门。
- 结构收敛：没有隐式 DUP、影子 capability、SDK Busy/yield、Sender 锁、每调用新 Work 或新退款 owner；Authorization 是非消费接受元数据，独立责任仍归既有 LifetimeUse/BackingUse/ReceiveTxn。完整 metadata、新结构内部交错和各平台/退出矩阵仍未闭合。

原验收覆盖核对与当前验证断点：

`authorization-virt-003.log` / `failed-acceptance-20261004-163018-45954.log` 已跑完 core、服务/Job监督、自动回收、Pool 收束、辅助 capability 清理和显式 Shutdown；runner exit 1 是七个原自检/旧接口锚点缺失。`authorization-virt-004.log` / `failed-acceptance-20261004-164058-48563.log` 在新增 Tunnel owner 失败/退款用例后也完整 Shutdown，新增用例通过，仍由同一七锚点拒绝。两次真实完整业务结束均是运行证据，均不记作正式验收通过。最初已有 Sent Timeout 仍保留为多路线风险，不能仅以连续 core 越过关闭。

新增 Tunnel 用例使用实际消费者与独立四页子 Pool：0 长度拒绝、5 页超额拒绝后 allocated=0；普通匿名 View 占据 FixedEmpty 地址，Map 失败返回原 Endpoint，再解占并重试；creator Close 后 Attach 返回 ObjectClosed 且返原 Invitation；两端授权关闭后普通 View 仍可访问、子 Pool 仍 allocated=1；Unmap 后在有限期限内检查自主退休退款至 0。没有 fault 注入服务、额外 syscall、显式回收泵或全局 Pool 近似基线。`authorization-clippy-004.log` 与最终 `authorization-clippy-006.log` 七面通过；003/005 分别为夹具误用未公开 raw call 路径和私有 management 模块的编译失败，已改用公开 typed Pool/collect_remaining API，保留失败日志。

主代理核对原 `HEAD:task/{wait_set/selftest,wait/selftest,mailbox/selftest,tunnel/selftest}` 的责任与当前消费者，explorer sub-36 补充原 case 路径。原测试的 ArmCycle/rearm/retirement actor、Drain、四类旧全局债务与 PA projection API 已被目标删除，不复建这些旧机制。下表记录旧覆盖与新结构风险的归属，不保留对应旧锚点，也不要求逐项复刻旧用例：

| 领域 | 当前已覆盖 | 新结构仍需核对的独立风险 / 处置 |
|---|---|---|
| 持久 WaitSet | 真实持久电平/陈旧提示/非空退出退休/嵌套拒绝；signal host 三项插入边界/删游标/瞬态与关闭 | 迟到 epoch、Add 安装与 Remove/Close 交错、候选在途后 owner 自摘/迟到拒绝，以及新节点实际付款/构造失败退款；不迁移已删除的 rearm/actor |
| 等待续体 | 真实多来源等待/调用者 Kill；maintenance host 的 Running/Parked/迟到 ticket | 安装中到期与取消、旧 epoch 拒绝、完成凭证归还前不复用、输出失败和释放链完整退款；旧 Native Drain 压力移除，不造替代 Drain |
| 消息事务 | 同槽 Sender、多 Receiver 消息/transit/Delivery、背压、退出退款 | Receive 输出撤销及同时关闭时的事务消息 owner/唤醒；新的 metadata 实际付款与精确归还 |
| 定时等待 | 真实过期/绝对期限/优先级/已提交结果；timer_queue host 取消/到期摘链 | 安装窗口立即到期、已挂 timer 随进程退出取消、WaitEntry/TimerDelivery 存储及付款退款；不恢复 Token 外观或测试运行体 |
| 监督预算 | 非法政策拒绝；合法有限 DONE 等待耗尽，Timeout/WaitDone、queries=0、保留原 collector/authority；Kill 后 replenish 同机器至 DONE，virt-005 已通过 | 本项实际消费者门已成立；旧 Drain 次数/工作预算删除 |
| Tunnel 失败 owner | 新隔离 Pool 的 quota/Map/Attach 失败 owner、独立 View 与退款已真实通过 | 输出撤销导致目标进程 fault 的失败 owner、新对象/表页/节点实际 funding 失败；不照搬旧五类 Sponsor/全局债务限额 |
| backing 几何与实际付款 | Running 六长度、双端 VA、65536 Stream；SegmentMap backing/碎片供给 host | 碎片化实际 backing 的逐页 PTE/撤销/资金守恒证据；不恢复被删的 projection API |

新结构风险由本篇继续承接，按当前契约与现有证据去重，不以旧用例数量制造施工任务。旧 `deferred_work/selftest.rs` inventory 已无模块/消费者，已直接删除。主代理补查确认 stress 的 24 轮 Close/Attach 实际由已迁入新 Endpoint/View 的 Hammer 调用，保留此真实门；init 的生命周期用例是另一覆盖面。尚未运行的 stress/fp/mode 保持未覆盖。

最终 `authorization-virt-005.log` / `failed-acceptance-20261004-164716-50536.log`：共享 Sender 身份/并发回归、Tunnel owner/退款和合法监督预算耗尽均通过，FAL midflight 与 core 全业务第三次完整显式 Shutdown；当时 runner 因六个失效旧锚点 exit 1。历史退出码保持原样，这不是六个旧测试必须重建的证据；判定规则由下一轮清理。授权分离本轮源码/host/真实消费者门已成立，系统交付门仍未成立。

收尾：删除无引用 inventory 后 `authorization-check-002.log` just check exit 0；`git diff --check` 与 runner shell syntax 通过。五次受控 QEMU 与诊断启动器均已结束，无残留 QEMU。未提交、未归档、未登记系统能力完成；COMPASS 继续保留本篇入口。

### 验收清理轮次

用户明确确认：删除失效机制、夹具和门，不为新旧一起生效建立补丁或兼容路径。六个原内核自检锚点所依附的源码/协议已删除，直接从 runner required 移除；上表的新结构风险继续按契约和实际证据归属，本轮不将它们扩成六套旧测试的替代工程。funded frame/Clock 仍在真实启动路径，自检保留；公共时间、Sender/Receiver/退出、监督、Tunnel/Stream/FAL、拓扑与 reset 锚点保留。

清理范围与事实：

- `tools/qemu-acceptance.sh` 只要求当前设施。stress 删 `drain minimum-budget`，要求实际非空 WaitSet 自动退休、control-plane 压力和 init Tunnel lifecycle；Hammer 已迁移到当前 Endpoint/View 的 24 轮 Close/Attach 仍为真实消费者门，保留，不因名称来自历史就删除。先前 explorer 将其误判不存在，主代理以源码调用链纠正。
- 旧 `virt-capacity` 的前提是 BindMemory/Bound 模型与全局固定 32 个地址空间准入，要求第 33 个 ReachLimit；当前已删除这些机制和限额。直接删除 Justfile 路线/超时/负载选择、srv_init Cargo feature、三个顺序/并发/混合夹具及专属常量/分支，约 240 行夹具减少，不建立新 quota/adapter 维持旧结果。原固定提交 Review 仍按其固定对象审查，历史证据不改写为当前事实。
- impls 删除无消费者旧 selftest/运行日志清单，以及仍声称 Drain/WaitMany/Tunnel 未迁完或不能组合构建的陈述；同步当前 Connection 的 BackingOwner 与普通 View 的独立 BackingUse、现有验证设施、BUILD 手册与 KNOWN_ISSUES。历史过程和验证缺口只在本篇，当前位置不保留过时“下一步”。
- `acceptance-cleanup-clippy-001.log`：七面 clippy exit 0，core/stress/fp 的源码同步通过。`bash -n tools/qemu-acceptance.sh`、`just --list` 和 `git diff --check` 通过，现行源码/工具/impls 不再引用删除的容量 feature/recipe/超时或六个旧 required。
- `acceptance-cleanup-virt-001.log`、完整 `qemu-acceptance-20261004-170623-53036.log` 及同名 meta：50% 节流、debug core、硬上限 150 秒，**just virt exit 0，当前 core 正式路线通过**。真实业务、服务/Job 监督、退款、辅助 capability 关闭与显式 Shutdown 完整收束；日志/ELF SHA256/运行身份保留。此前六锚点导致的失败记录保持原退出码，不通过日志回放冒充新运行。没有改变业务 deadline 或失败拒绝规则。

该清理轮次的接续由下节 metadata 账户实施承接；core 通过不关闭此前 Sent Timeout 的多路线风险，也不替代系统交付条件。未提交、未归档。

历史 Tunnel 开发日志索引（已由 Runtime/消费者接线承接）：`tunnel-libraries-clippy-001.log`、`tunnel-libkernel-host-001.log`、`tunnel-runnel-host-004.log`、`tunnel-user-build-004.log` 及 `tunnel-clippy-{kernel,os-host,user-host}-003.log` 保留当时成功/失败事实；不作为当前施工断点。

### metadata 账户与派生接入轮次

用户进一步确认：每轮都检查是否在给重构前模块补代码；旧模块可暂时损坏、逐步删除，不为中间通过建立 adapter。本轮在目标 Pool 责任链实施，不修改用户旧 FAL/Sponsor 结构。预算设计闭合写入[资金计划的本轮设计](../todo-2026-09-14-kernel-memory-budget.md#付款接入设计轮次统一容量槽与双维派生)，统一槽取代旧多类计数，不并存旧额度；库的薄页接口也没有留下 pages-only 派生。

范围/消费者/责任：

- 纯 `os/memory_pool::LedgerState` 改为单位无关账本；同一 Pool 的 pages/metadata 各有独立 OwnerKey，诊断 ID 相同不能交换 token。零单维 ledger 与 delegation 允许，charge 仍拒绝零；Pool 双维整体零拒绝。将额度加总与单位选择留在唯一内核 owner，不复制另一套 metadata 算术。
- 内核 Pool 同时预约两维，第二维失败立即撤第一维；PreparedChildOwner 持两份 prepared 责任到失败/commit。构造 Account/Pool、收费、caps buffer、输出及表关闭失败都由现有 RAII/表预约收束。输出成功后两维必成 commit，shut 撤公开再正常 release；不中间拆交付。父链逐锁深度最多 ABI 32，归还两维并转维护债务，不新增 Work。
- `resources::MetadataCharge` 强持付款 Pool 与独占 allocated credit。Arc 值按控制头/T/padding 的布局折自然字宽槽；root 源自实际 heap 供给扣启动占用与固定 root Pool/Account。child Pool 和独立 Account 各持父付款，账户字段覆盖独立寿命，关闭父句柄不撤 child 或 charge。内嵌 Work 不重复收费，外部 Weak 保留分配块的尾部不被宣称已退字节。原 global/local PoolCore Counter、resources init/峰值数据、kernel 对 metadata_admission 的依赖直接删除；用户态仍用的纯计数库没有为本次内核接线补改。
- shared ABI `MemoryPoolBudget {pages,metadata}`、32 字节 `MemoryPoolDeriveRequest`、104 字节 `MemoryPoolSnapshot` 同步 syscall/libkernel/实际 init 派生者。derive 为 parent/request_ptr/output_ptr，query 两维守恒。root 诊断输出实际 metadata 账本，不能用 root allocated=0 推断其余未收费对象都已退款。
- `test_pool_budget` 使用独立测试域，不以全局 Root 做精确退款基线。实际验证整体零拒绝、metadata 第二维失败撤页预约、无隐式赠送、metadata-only 子树、父句柄先关后孙域仍可查询、最后两维及父付存储完全退款。测试不复制 Kernel 类型成本表；新增收益是捕获之前 local Counter 可每次派生重置额度的错误，以及两维失败/尾部 owner 断链。原页-only Tunnel 夹具显式 metadata=0；后续对象收费接通时其真实付款消费者按新需求迁移，不给旧 ABI 默认额度。

证据与检查：

- `metadata-ledger-check-002.log` just check exit 0；`metadata-ledger-clippy-003.log` 七面通过。001 check 是新入口误用不存在的 uaccess helper，已改为现有 VM 锁下受保护整数读，保留失败日志。
- `metadata-ledger-host-002.log` 16 项 pure ledger 通过，其中零维拓扑和同 ID 跨维 token 拒绝独立于实现流水；`metadata-ledger-abi-host-001.log` 3 项布局/双维守恒相关用例通过。
- `metadata-ledger-virt-001.log` 与 `qemu-acceptance-20261004-175950-58048.log/meta`：50% 节流 debug core，真实预算用例通过，完整 core 显式 Shutdown，exit 0；新 Pool 门登记 required。root reset `total=available=2096693,reserved=allocated=delegated=0` 是本轮被收费的 Pool/Account 责任守恒证据，不冒充全系统 metadata 资金门。此轮不重复运行已过路线。
- 定向 reviewer sub-39 未发现独立前置的透支、重复/漏退款、发布窗口 panic、引用环或必要膨胀，亲核 dual prepare/Drop/commit、Account 维护环摘除后锁外释放、父链及所有 ABI 消费者。主代理核对并将测试基线隔离；该审查只覆盖本轮基础。真实 Arc OOM/输出撤销和 Account 晚于 Pool 的实际执行窗口未注入，后者随真实 maintenance 消费者组合补证据。
- 核实 kernel workspace 使用的 Rust 源 `alloc/src/sync.rs::ArcInner` 为 repr(C) 的 strong/weak 两个自然字及 T，与当前 charge 布局政策一致；这是当前工具链布局取证，不扩展为 allocator 碎片/外部 Weak 的字节保证。最终 diff/shell syntax 通过，49 个当前 common required 均在新运行日志，无残留 QEMU。未提交、未登记系统交付；COMPASS 保留本篇入口。

该账户基础轮次的未闭合责任（由后续保留存储轮次继续承接）：

1. CapTable、Members、Ready、OrdMap/Backing双数组与VM ledger固定/保留容量已由后段接线；Views/Job/WaitSet节点由末尾轮次接入。后续只读实际分配盘点确认planner固定workspace与内核page_table单步自身无动态scratch，未付款的是Operation/账本调用侧容器，owner为对应操作。按实际capacity/块占用付款、门外准备/释放；不以len或逻辑clear提前退款。当前顺序由下方唯一下一步承接。
2. Shell/Body/Builder/Thread/Control、Mailbox/Message/Delivery/Lifetime、Notification/WaitSet/WaitMember、MemoryObject/Connection/Endpoint/Invitation 的独立值仍未收费；按真实存活根逐项接 charge，内嵌字段包含于宿主。旧 `_pool` 是保持来源/维护资格，不自动成为外域增长或新对象的付款授权。须从实际调用者默认 Pool／明确 CREATE 源／已确认目标域授权确定 payer，传输不重计费，新 View/事务及接收表容量由产生责任的域付款；共享 backing 外域增长由触发操作付款。
3. 以上是同一资金切面的自然依赖，不以每个字段拆阶段，不新开平行 todo。每批接相关消费者/错误/取消/关闭/退休，删除过渡计数；不补坏旧模块。新增消费者用已存在服务/runtime，资金不足必须可恢复地收束。
4. 本链闭合实际页/live metadata 槽付款、提交前存储准备、完整维护计量及平台/退出验证。完整堆字节硬隔离、外部 Weak 尾部/allocator 碎片隔离属于独立预算案的域隔离完成门；按该案第 27 节已确认最小账户形状，不自动升级为当前对象链前置。现行普通 heap OOM、live 值/容量槽语义明确。基础双维账户成立不代表整个对象链交付，不归档、不登记完成。

当前剩余规模估计：约 6–8 个主要闭合单元，按责任链而非文件/字段计数。容量 owner 接线约 1–2 个；进程/线程/Job 与 IPC/等待/内存对象付款约 2 个；维护计量/父链尾部成本约 1 个；跨机制资金失败/取消/退出证据约 1–2 个；正式组合与交付结构检查约 1 个。它们仍是本链同一施工闭包，不为计数新开阶段或计划。已完成的旧 VM/ABI 删除、Runtime 装配、结果义务删除和失效自检清理不重复计入；锁优化、FAL 全栈重设计、设备体系和独立堆字节硬隔离不自动扩成本链任务。估计可能因真实缺陷调整，调整须先核对本链交付契约与重复真值，不以新增基础设施或恢复旧门扩大工作。

### 保留存储付款接线轮次

用户确认继续闭合当前内容。目标是实际存储容量的唯一付款 owner，移除容器隐式 heap 扩容；不以增长后补费或旧无收费 API 维持新旧并存。主代理核对 CapTable/Members/Ready、OrdMap/SegmentMap、Views 的源码；advisor sub-40 只咨询这一次容量接口/锁序，决策由本记录承载。

- 共用薄原语 `shared/funded_storage` 拥有固定布局分配及唯一凭证，Funding 只按 Layout 取得 charge。固定容量未初始化 slice 解决 Vec 实际 capacity 超请求的问题；元素初始化前缀由存储 owner 拥有，不暴露能隐式 realloc/shrink/into_vec 的接口。Box 节点也持完整分配 owner，存储实际销毁后退 charge。全仓目录/package/default target/crate 统一 funded_storage，无默认 `C=()` 内核旁路。该原语已实现，CapTable/Members 为正式消费者；OwnedBox 是下一批树节点的既定分配契约，目前仅 host 测试，尚不能算 Views/Job 节点付款接通。
- 增长准备一份已付款空块；同锁复核当前 len/全部 reserved/本次需求和领域状态，搬移保持原槽编号，换下旧空块由同一种 owner 带出所有高锁再销毁。共用原语不增加 Prepared/Retired 状态机，组合准备属于各容器。新块全额由触发增长的已授权域付款，旧块保持原付款直到释放，不合并跨 Pool credit；新旧同时存在的峰值必须可负担。
- Funding/allocator 必须在相关高锁之外。HANDLE_TABLE100允许有限短 Pool230 边；Members610 外层 Shell600、VM300/Views325/Backing350/Ready 高锁均不能收费或退费。仅退出最内层锁不足，旧块和未用候选须返最外层 Operation/构造 owner。函数接 mutable 准备存储，落败保留 owner，禁止 `?` 在高锁内析构退款对象；无需新退款 Work。
- CapTable 身份/代次、Members 的 Shell 门与 reserved_members、Ready 的 outstanding、OrdMap 的双数组+reserved、VM 区间+输出 lease 分别有唯一 owner，不能以 metadata 余额取代这些准入事实。Ready 的全局调度域保留存储由该域的系统付款源承担，客户端支付私有线程/过程资源；共享队列不能把某调用者预算永久钉为系统缓存 owner。
- OrdMap 只有 nodes/free 两块都预备成功才一起发布；free 与 arena共同保证预约，remove/unreserve只返逻辑槽，不退容量。整体搬移成本由实际 retained arena 上界计量，AVL树高不覆盖搬移；若不适合一次内核推进则按该容器独立存储界限分块，不另建通用分块框架。
- OrderedTable 摘除交完整节点 owner，不能锁内解构 Box 仅返V；Views 重复 source/unused boundary返外层直到VM锁退出。Map准备可借身份但不代表接受，最后必须在表内重验同代 handle/rights/目标，安装容量并取得 use；所有旧/未用准备回外层再释放，Close/use线性化不变。
- 本批不引入自动 shrink。真实保留容量继续收费；显式缩容/退休先保护未消费预约与 handle 代次，再换块/清内容，锁外释放。Backing/VM DONE如承诺包括私有容量退款，须清预约并摘空存储，不能等观察Arc归零假装已退。

自然迁移闭包：薄存储及 CapTable/Members/Ready 准入；OrdMap/SegmentMap+Backing最终授权接受；VM ledger/planner scratch+Views节点与Operation回滚；实际退休资金屏障和针对性验证。共用原语新增测试只捕获独立不变量（固定容量、付费先于分配、值/块析构后退、失败原值/原块保留、跨payer更替和双块失败）；内核复用正式消费者和锁阶检查，不新增故障服务或测试运行体。

当前实施与责任检查：

- Buffer/IntoIter 独占初始化前缀或剩余连续区间，remove/move不复制有效 owner，Drop 交标准 slice drop glue，字段先释放分配后退款。OwnedBox::try_new两种失败返原 T，into_inner释放原 Box 后才退 charge。ZST 零字节布局免存储付款，正常值析构保留。薄接口只有一个 Funding trait 和两类实际 owner，没有 allocator 框架、反向回调、每次分配 Work。
- `CapTable<A,C>` 的 slots、安装 handles/releases、extract返回块已用 paid Buffer；所有 Kernel表创建/reserve（通用install、Grant、Receive、bootstrap、Tunnel Attach）显式传目标表所属进程的 Pool。第二份scratch或后续增长失败撤逻辑预约，已实际保留的增长容量继续收费；closed commit不分配并锁外投递事件。返回 handles/iterator保持原付款，clear/remove不退容量。旧 Default 64k 无资源/ABI依据，改Handle u32上界；实际规模还由不可增发的metadata Root供给及峰值决定。
- 主代理核对Body构造失败、install_body落败、take_body和DropBody：所有新表存储销毁都在Shell gate之外，退款完成才Settle/DONE。CapTable低锁100允许短Pool230边；释放原T/业务最后事件仍在表锁外。当前caps输入Vec、IPC claims/payload与其他prepare scratch未收费，由后续对象/事务切面承接，既有CapTable新块不丢owner，不能把通用表scratch完成冒充所有临时存储都已收费。
- `Members` slot数组已付款。首次在Job链→Shell→Members门内仅尝试复用；缺容量后退出所有门，准备严格更大块，回门复核再搬移，旧/未用块由Job调用者锁外释放。竞争失败不向用户新增临时Busy：再次失败必须证明其他方已装更大容量，重试次数由容量几何增长/自然字宽上界约束。tid与reserved_members只在成功准入时推进，已有MemberReservation/Seal/Frozen规则保留。主代理亲核外层祖先锁、早退和Drop顺序；未新增成员互斥或清理owner。
- Ready、OrdMap/SegmentMap/Backing 与 VM ledger 容量已由后段接入；未收费树节点、操作scratch及独立对象继续后续切面。不加空payer/default()维持内核中间API。

证据：

- `funded-storage-host-002.log` 7项；001为Box方法调用选择了MaybeUninit::write而非Box::write的类型错误，已明确Box::write，保留失败日志。无系统依赖安装。
- `storage-cap-host-002.log` 22项capability通过，新增资金失败撤逻辑预约但保留真实容量、安装scratch全寿命收费。`storage-cap-check-002.log` exit0；001只指出bootstrap/Tunnel两个实际reserve尚未传payer，接入目标表付款后通过，没有加旧参数重载。
- `storage-members-clippy-001.log` 七面通过；此前CapTable的check/clippy不证明后来Members的退款，但本轮源码检查和实际路线涵盖其核心创建/退出。
- `storage-cap-members-virt-001.log` / `qemu-acceptance-20261004-220913-69141.log/meta`：50%节流debug core，真实创建、等待、竞争、FAL、退出/退款完整收束与explicit Shutdown，exit0。root reset metadata `total=2096693,available=2096021,allocated=672,reserved=delegated=0` 是仍Running init的真实保留表/成员容量，不能当全部资源退款或缺陷。私有消费者退款和表层零业务cap清理已越过；OOM/高争用窗口尚未注入。
- 定向reviewer sub-41只读审了薄unsafe storage与CapTable切面，未发现可证实内存安全/锁阶/重复退款/owner丢失或当前切面必须补齐的隐藏债务；证据覆盖MaybeUninit/ZST、部分iterator、Box失败、闭表commit、Body锁外退款。确认表扫描/搬移及最终T析构仍O(真实保留容量)，不能用16槽清扫预算声称整个析构的时长。后来Members由主代理单独核对，没有将此前review结论冒充它的独立审查。

CapTable/Members段的未覆盖与接续：Ready已由下一段完成；双数组OrdMap/SegmentMap、Backing授权窗口、VM/Views/操作scratch与退休资金屏障继续按同一闭包接线。实际allocator OOM、析构panic、Members增长碰撞的定向注入和新的收尾成本计量尚未覆盖；可观察原语已host验证，组合继续复用真实消费者，不为这些窗口造服务。全系统metadata收费与平台矩阵未闭合，本篇保持施工，未提交、未归档。

Ready 接入设计：

- 调度域及 Capacity 协调 Arc 在 root 初始化前由 boot 构造、冻结，实际占用已从 root 初始 heap 供给扣除。它仍是 Admission 唯一身份/原子计数，不把存储 charge 塞进 Admission；远端归还保持无分配、无队列锁/Pool锁。系统域在存活期保留 Ready 容量，由系统 root Pool 显式支付运行时新块；不占客户端 Pool 的长期额度。
- `ReadyQueue<T,C>` 使用已付款固定环，固定 `Buffer<Option<Admitted<T>>,C>` 槽在锁外初始化None，环 head/len 为逻辑内容。准备块与旧空块是同一 ReadyStorage owner；发布只搬FIFO owner、保留同源Capacity，不析构T或credit，不更换在途Admission。
- `reserve(count,&mut prepared)` 在类锁内复核 outstanding+count，候选不足返回CapacityChanged且不新增责任；成功保证整个batch可随时回Ready。候选在锁外取 `2*capacity+count`，再次落败意味着实际outstanding已超过所采样容量的两倍，容量不可缩减，重试有自然字宽上界；取消只能降低outstanding。
- constructor/准备的OOM/metadata拒绝/溢出保持现有队列/owner原样；所有旧/未用块在类锁外drop。首次Bootstrap/ThreadCreate实际消费者同步传播QuotaExceeded/OOM，不以无付款重载维持旧reserve。FIFO enqueue/pick/轮转/scan单项不分配，原allocator探针继续作为独立门。

Ready 实施与证据：

- `ReadyQueue<T,C>` / `ReadyStorage<T,C>` 已使用固定 paid 环，删除原 VecDeque 的隐式reserve路径；Admission/Admitted未泛化C、不Clone、不增第二计数。固定Capacity Arc不包含charge，内核boot已供给，外部最后credit仅原子退位置与普通Arc寿命动作；不能将其当存储付款。
- FairClass先以空候选复用，类锁内读outstanding及growth目标；失败后退出类锁释放旧/未用块，再明确从`MemoryPool::system_pool()`取系统财务源、付款/分配/初始化。成功return也按嵌套作用域先退出ready guard再析构prepared。所有Bootstrap/ThreadCreate调用处在Job/VM/execution/requirement锁之外，MemberReservation和BodyLease保留业务责任而不持锁；接SystemCallError，不吞Quota为OOM。
- 主代理核对ring wrapping、scan轮转、reserve快照只被并发减少、搬移不drop/copy权、BOOT顺序/root成本扣除与所有实际Drop点。字段`Buffer<Option<Admitted>>`物理容量保留到整块替换/销毁，任何取消/离场不退存储账。增长后一个客户端退出不会把系统容量扔给已结束Pool。
- `ready-storage-check-001.log` exit0；`ready-storage-host-002.log` 12项加1编译失败doc通过。保留原8k状态转换、并发atomic退准入、foreign queue、scan和allocator拒绝探针；新直接验证未备好不分配/发布、额度拒绝先于allocator、换块旧付款持续到实际释放、在途credit跨换块、环wrap/full scan后FIFO。host-001只因foreign queue未显式C和新增测试atomic旧方法deprecated失败，已同步测试类型及当前方法，无内核默认C旁路。
- `ready-storage-clippy-001.log` 七面通过；`ready-storage-virt-001.log` / `qemu-acceptance-20261004-225841-73127.log/meta`：50%节流debug core，正式全业务/线程/退出/显式Shutdown，exit0。root reset `total=2096688,available=2095995,allocated=693,reserved=delegated=0` 包含仍活init私有存储与system域保留环，不能因outstanding归还宣称storage退费。seed与上轮差异来自BOOT真实存储形状占用变化，不硬编码旧总额。
- 定向reviewer sub-42未发现本Ready切面的内存安全、并发计数、FIFO、付款owner或锁序缺陷；独立核对Root时序和候选`2c+n`重试证明，以及外层调用者不持高锁。该证据为源码审查；真实并发换环的精确窗口、内核payer成功后的allocator OOM尚未注入，host已有OOM/状态转换门；结构搬移/初始化成本仍属计量阶段，不补虚假常数。

付款切面接线事实：独立对象/Work/来源/消息出生与退出付款、私有退款屏障及源码复核已完成。Operation/账本实际 scratch 的paid header/数组、小owner阶段传递、供给许可借caller数组、所有废弃/失败/最终容量门外释放已闭合，旧聚合真值/平行batch/Vec已删。各切面的host、栈审计、core/stress及平台证据保留在对应轮次；当前收口与尚未闭合的门只由篇末登记。事务槽满走ObjectBusy背压，不增容量或runtime补丁。planner固定workspace/page_table单步、funded_frame固定本体不列虚构动态费任务；旧FAL admission覆盖留其重设计接通。任务未提交、未归档，正常路线不替代剩余完整资金门。

OrdMap/Backing/VM 接线决定：

- OrdMap强制charge泛型，nodes/free两个固定buffer由一个OrdMapStorage成对准备；不能先publish nodes再尝试free，也不保留旧partial growth测试钩子/API。reserve只提交现成容量与逻辑credit，交换下的两块由外层在所有VM/Object/Backing锁外析构；logical remove/unreserve不缩容量。整库及MemorySpace/SegmentMap真实消费者同步，不给内核默认 `C=()`。
- Slot逻辑可用量与在途reserve为唯一真值。准备容量≥nodes.len+max(当前reserved+本次n-free.len,0)。首次空候选复用；落败后强制准备2*当前capacity+n，不能因重采样瞬间空出槽再次取空候选。完整候选再次不足证明容量已越过两倍采样值；容量不缩减、最终空Backing永久关准入，重试有自然字宽/索引上界。失败不接受新credit。Backing最终段与预约均空后摘容量锁外释放，不以外部Arc归零推迟退款。
- Map先借同代handle的存储身份，取得待准备Backing，但不接受use。Caller资金在锁外准备双数组；表内最终重验仍是同代/rights/相同core与可用root，内层仅swap+reserve+acquire，不收费、不析构候选。候选不足不产生业务use，退出表再重备。成功或错误的旧/未用存储都由原Operation外层持有，Close/use线性化不变。
- VM ledger的region arena和transaction保留容量亦使用实际payer准备。逻辑max_regions不能假装涵盖替换期间旧/新节点同时存在的峰值；以真实reserved及materialize需求准备，最后同VM门复核。需要的全部计划scratch纳入已出生Operation，失败统一回滚，VM DONE只在私有账本容量已锁外释放后发布。
- Views/Job等树节点按已定OwnedBox owner进入构造/摘除，禁止锁内解构仅返V并退费；取出的重复来源、unused boundary和旧节点回外层。如果所在路径还无法遵守此合同则保持未迁移/损坏记录，不能套旧无费包装使检查通过。

OrdMap/Backing/VM 本轮实施与交付前检查：

- `OrdMap<K,V,C>` nodes/free 已改固定 Buffer，成对 prepare/install，删除 Vec 隐式增长/partial-growth钩子。SegmentMap<C>/SegmentStorage 与 MemorySpace<C>/RegionStorage 同步真实消费者；保留插入、预约、切段与错误 owner 契约。NodeReserve 溢出归 NodeLimit/ReachLimit，allocator/付款失败由锁外 StorageError 单独传播，不把额度拒绝伪装OOM。无默认C内核旁路。
- 初始Backing索引由创建Pool付款，共享增长由触发Operation/调用者Pool付款。Map先借同代存储身份，首次最终表内尝试复用；需要容量才退出表付款/分配，回表核验同代handle/rights/同一对象后swap与接受use。候选不足只返None，不产生业务use或外部临时Busy；旧/未用块由Operation在锁外释放，submit在维护发布前销毁这些准备容量。
- Backing普通释放/root Work及BackingSplit返预约都在段与预约全空时摘 metadata 存储，最外层锁外退款；终态关新准入，观察Arc继续存在不留私有容量。MemorySpace regions双数组和固定transaction Buffer出生付款，替换峰值按真实nodes/reserved/free准备；ReserveFailure保留原plan/permits用于同操作重试。AnySpace整空间drain后锁外销毁ledger容量，早于VM/Process私有完成屏障。
- Unmap/Protect边界准备移到VM门外，最终同门复核容量、预约及两端是否已有匹配候选；物化顺序仍实物→ledger。ViewBoundary保留Option节点与split，commit仅消费需要的节点；未用节点/credit在Operation锁外清理。这个接线为后续Paid OrderedTable节点移交清除锁内析构路径，尚不冒充节点已收费。
- sub-44首审发现P1：准备时无View而丢弃None，竞争Map随后跨过端点，最后门只复核Some；Protect失败留下细ledger/粗View，退出会panic。已改`boundaries_ready`在同VM门检查两个实际端点，包括无候选；欠准备保持ledger原样。已成端点/实物消失不消费旧候选。主代理沿原反例复核：新Map跨点→检查拒绝→无ledger切界；空→仍空时允许进入并由正常几何验证拒绝。没有制造测试服务或把core当该精确交错已注入。
- sub-44确认首审版本的双数组原子发布、锁外owner、最终表内授权和root/View退款无可证实新bug。之后新增容量内部重采样由主代理核对：首次复用、失败后全额增长，不在高锁分配/析构；Backing退休终态防止容量重开，VM AccessLease保留ledger。首审结论不冒充后来重试代码的独立审查。
- 数组换块/最终初始化前缀析构仍O(实际保留容量)，没有把AVL高度或STEP_UNITS冒充完整搬移/析构预算；正式计量/必要分块仍是本链维护计量门。planner scratch、Views/Job树节点和独立对象付款未迁移；当前新接口已接消费者且源码可检查，未给旧FAL/坏旧模块补adapter。

本轮验证证据与边界：

- `object-paid-check-004.log`：just check exit0；`object-paid-clippy-004.log`：七面clippy通过。clippy-002保留测试辅助大Err报警，按正式ReserveFailure仿射owner契约在该辅助处标明allow，没有为 lint 给失败owner增加Box分配；003/004通过。
- `memory-space-paid-host-005.log`：43项通过（10 unit +9 backing +24 planner），保留原几何/阶段/错误owner/根与View释放门。新增独立合同用例覆盖第二buffer付款拒绝退首charge、逻辑清空不退容量/旧块交换后直到外层释放才退款、竞争credit归还后强制增长候选不退回空候选；不复制Kernel成本表。003测试辅助误对&mut参数再次&mut的失败保留，004/005修正后通过。
- `object-paid-virt-002.log`与`qemu-acceptance-20261005-005225-82034.log/meta`：50%节流debug core，正常真实Map/Unmap/Protect、Tunnel、Pool、Job/Seal、FAL、监督/退出/退款与explicit Shutdown完成，exit0，进程组已收束。Tunnel创建测试域从实际父metadata供给显式划给子域一半，留另一半给调用者View/操作；同步等待页及metadata allocated归零，不复制类型成本或给旧ABI赠送默认quota。
- reset `total=2096688,available=2031950,allocated=64738,reserved=delegated=0` 包含仍Running init的已付款ledger/表/成员存储与system Ready环；不能因其他域已收束将其报成全系统零资源。ELF审计最大frame0x2b20未越现行guard审计门，heap无source failure。virt-001也是core通过，但只保留外层完整命令输出/摘要；随后改变内部增长重试并执行002，002保留完整原始runtime日志。
- 精确None→新View切界竞态、真实并发双数组增长失败窗口、allocator付款成功后的OOM未定向注入；这些不由普通core通过替代。Views/Job节点/操作scratch、独立对象付款、维护计量与完整平台/退出组合按上面的唯一下一步继续，未提交、未归档。impls与KNOWN_ISSUES已同步；COMPASS仍指向本篇当前施工，无过时下一步。



全链实际消费者和资金屏障接通、旧路径删除；`just check`、`just clippy` 与构建手册选定 QEMU 组合通过（退出/Kill/构造失败、Map/Unmap/Protect 并发、seal、整空间退休及用户态扁平等待）。按 REVIEW 做最终结构检查，回填 impls/KNOWN_ISSUES/COMPASS，再申请提交；代码提交后登记独立 Review。未满足前不归档、不宣称能力完成。

本链之后才进入[锁优化与无锁化评估](../todo-2026-10-01-lock-performance.md)。

树节点付款接线（当前轮次）：

目标与已定取舍：共享 OrderedTable 强制存储 charge 泛型，AVL 链使用 OwnedBox<Node,C>；prepare 显式 Funding，remove/pop 返回同一个完整 PreparedEntry owner，不在树锁内释放节点或只解构返V。旋转/查找/插入/摘除不收费、不退款。Kernel表全部显式 MetadataCharge，付款在所有 Job/VM/Views/WaitSet 门外；未用候选跟原Operation带出外层门再释放。
Job成员/子节点由拥有表的Job Pool付款，锁外备无身份占位，在祖先链门内核seal/深度/容量、取不可复用ID与token、改候选key/value并发布；退款在回滚/完成摘除的锁外，完成责任发布之前。祖先Arc链与守卫数组也按实际固定/所需容量付款；守卫先释放全部锁，随后才退数组charge。JobSeal后最后child占位失败回滚同成员回滚一样检查完成，避免留下无人推进的封口Job。
Views/ViewBoundary/ObjectSource节点由触发Operation的Pool付款。PreparedView.node为Option，commit借mut候选只移动已用节点，重复来源候选仍由Preparation持有，VM锁外清理；hold_writes先复用已有来源，缺失则锁外准备再重验。WaitSet Registration AVL节点与稳定Pinned Member两份分配分别由集合Pool付款，挂链前全部备齐，提交失败/Remove/Close摘出完整owner到集合锁外detach与退款。
sub-46基于用户budget/allocator源码的一次咨询后采用用户显式C=()与AllocatorBacked：libbudget拥有该策略，表示使用可执行程序选择的heap allocator且本层不追加逐分配软额度；不宣称Kernel metadata已付款。既有Task/InputBytes/FAL/Service数量及混合布局Charge仍由业务V持有，不改账户/单位/退款owner；其布局型计算同步新allocation_bytes，Runtime Source输入费覆盖节点+timer。新增依赖仅用户消费者→libbudget→funded_storage，不依赖libruntime、不选择allocator；RPC同步依赖此政策owner。shared不提供默认C或无payer旧重载，Kernel类型不接受unit-charge策略。
全部实际消费者同步新泛型/payer/StorageError及完整摘除owner，用户无锁边界根据业务用into_value/into_parts解构。FAL仅同步共享契约和实际布局，不为其坏结构补兼容adapter、不宣告重设计或严格逐heap字节收费。用户逐分配软存储owner若要成立，须先在本篇核定扩展闭包，按各领域拆混合账并补执行/RPC装配政策，不把尚未立案的分案当作已承接，不隐藏为本轮Kernel资金门。
验证门：既有AVL/用户预算测试；新增节点拒付返输入、双子树摘除保持付款、owner跨表不再扣费/精确一次退款和pin地址/值先毁后退款。just check/七面clippy、真实core与非空WaitSet退出组合；精确并发窗口/allocator OOM注入未覆盖则如实记录。planner/Operation及page_table scratch和独立对象出生付款仍未接，按原对象链资金门继续，不以节点完成关闭整链。

树节点轮次中发现并闭合的heap供给前置：core的srv_fs启动在ProcessWrite返回OOM，失败证据`failed-acceptance-20261005-013811-87124.log`与诊断`failed-acceptance-20261005-014603-88256.log`。后者显示1MiB请求、16 tickets全消费但堆仍空闲15,200,992bytes；不是metadata额度拒绝。代码尺寸增长让ELF写入触及已有MAX_USER_ACCESS整块暂存上界，暴露每个1MiB ticket单独claim的旧arena边界。
依据Cargo.lock钉住talc5.0.4的`src/base/mod.rs`：claim 569–601 明确相邻heap不合并，755–827规定extend使用本实例当前end并独占旧end..新end。SystemSource现在只保存最近成功claim/extend的返回end和已转交ticket映射末端；下一预清零ticket紧邻时extend该arena，不相邻则独立claim。Source不resize/truncate、HEAP私有，end只在同allocator锁下改。只O(1)消费一个ticket/判断邻接，不扫描、不额外清零、不进入FramePool，维持HEAP→SYSTEM_SUPPLY单向边；固定16MiB供给及1MiB票据几何不变。预算许可仍不保证allocator成功，不能合并物理空洞。该修正使当前真实消费者可运行，属于本闭包必要前置；未改ProcessWrite复制/失败/请求长度语义。临时syscall OOM诊断在问题核实后删除。

树节点交付前结构检查与开发证据：
- sub-45完成真实13处消费者盘点；sub-46仅咨询用户付款边界，一次结论由主代理据代码采用。sub-47定向首审未发现可证实的新bug，确认双孩子删除返原owner、后继保留其owner、AVL不退款、Job/Views/WaitSet锁外候选/摘除释放及pin构造成立；首审之后新增heap供给修正与验收probe调整由主代理按实际契约复核，不能冒充子审覆盖。
- `tree-paid-host-001.log`：funded_storage 8项（1 unit +7 integration，含 paid Pin）与8 AVL用例通过。新增节点用例捕获付款拒绝误消费/发布、双子树摘除提前退费/跨表重复扣费；pin用例以!Unpin地址和析构顺序捕获移动目标、detach前退费。`tree-paid-user-host-001.log`：100项通过（budget6/execution29/fal39/rpc9/service17），预算身份/失败/任务退款等既有契约复用。
- `tree-paid-check-002.log`与`tree-paid-clippy-002.log`：heap修正后check/七面lint通过。早期user消费者与generic lint失败完整保留dev-001/002/003日志，按真正新接口同步，未补旧API重载。Kernel所有OrderedTable/PreparedEntry均MetadataCharge，无AllocatorBacked或default C旁路。
- `tree-paid-virt-003.log`与`qemu-acceptance-20261005-020346-89402.log/meta`：50% debug core正常Shutdown，source_failures0、claimed5MiB、reset allocated536612，完整页/metadata退款与Job/VM/WaitSet/FAL路径走通；固定5MiB是本运行实际ticket消耗，不是放宽堆限额。ELF审计max frame0x2e00，未调guard。剩余Running init与开放services Job账使root metadata allocated65166，不报全系统零资源。
- 并行001 core/stress因共享最终ELF/initfs产物主动撤销，仅顺序路线可用；记录`tree-paid-parallel-cancel.txt`。temporary syscall OOM探针已删除。后续stress失败详见下一段，不登记节点完整组合门已通过。

stress admission实际故障与当前修正：
- `failed-acceptance-20261005-020840-91404.log`（50% stress420s）在request140发送背压后response missing；未到hammer矩阵，不能称矩阵超时或Kernel deadlock。init失败后稳定supervisor清理服务但不提交Shutdown，工具最终124。
- 主代理亲自核对Send只是同步入队/期限裁决，10秒send_deadline并不等待，raw FAL业务deadline为INFINITE。前述口头“背压等待10秒”解释已纠正，没有改成At(0)（那会被check_delivery拒绝）。服务Ingress/dispatcher拒绝回复另有5秒绝对政策。
- probe保留全部回复邮箱为满，却额外每8条sleep100ms及发送后sleep2s，构造/固定等待/逐个清空都在消耗拒绝回复5秒期限；此前无真实错误/index可分DeadlineExpired与PeerClosed。只去掉人为pacing/settling，保留现有Registry QueryName真实Quota回执作阶段边界，然后立即清空全部reply filler；缺回复日志补index/error。150数量、send10s/receive30s/服务拒绝5s/路线420s均未增加，FAL服务政策不改。
- `tree-paid-virt-stress-003`去掉pacing/settle后在request35背压，但Registry不是Quota，exit124（failed-acceptance-20261005-022800-95607.log）：只制造输入队列暂时满，未建立132任务饱和。004恢复原pacing、仅去settle仍在request140后index0/DeadlineExpired，exit124（failed-acceptance-20261005-023340-97226.log）；实验未解释普通请求0的无限回复为何缺失。两个实验失败保留，最终恢复原pacing/settle，只保留index/error诊断。不把未经证实的节奏修改作为修复。
- sub-48只读取证：同admission response missing在20260924-094900已出现，同点至少8次通过，属于既有间歇组合问题而非可据单次证明节点回归；与core midflight watcher字面Sent Timeout为不同阶段，不合并证据。普通Lookup响应deadline来自header=INFINITE；另有Ingress/Dispatcher拒绝5s，不能拿后者解释第0条普通回复。request send是即时入队而非等10秒。当前失败无panic/OOM/明确Kernel额度拒绝；provider部署为root Pool授权复制，TASK_LIMIT132/SOURCE_LIMIT284，init reply mailbox每个16 filler。
- 最终源码已恢复probe原节奏、150数量及原全部期限，保留一处index/error诊断。`tree-paid-clippy-003.log`恢复后七面通过；不因恢复失败实验而再跑无新行为的长stress。Kernel heap修正/节点代码与已通过core-003一致；stress组合门仍失败，不登记本轮完整交付或归档。
- 历史断点（已由后轮Outbox预算1修复及旧probe删除承接）：当时在004同代reply0上沿Outbox/回复owner/WRITABLE/WaitSet注册/Runtime来源取证，不新造服务或消费者。原stress整组收束与退款仍是组合验证门；后续按源码反例修复新责任链，旧FAL无契约政策probe已删除并登记覆盖缺口，不再重复此断点或补adapter。
- 该轮尚缺实际调用侧Operation/账本scratch与独立对象出生付款，继续由本链承接；planner/page_table自身动态分配假设已由后段实际盘点纠正。接通顺序为已准备元数据→同领域门复检/只移动→锁外旧块/失败资源收束→完成屏障，再计量O(capacity)数据搬移/析构。全资金门和内部失败/退出窗口仍未闭合，impls/KNOWN_ISSUES及当前接力以后段为准。

Outbox预算1活性修正：
- 主代理亲自核对Outbox::drive/attempt_send/ensure_registration及Runtime::turn/advance_one。发送前唯一协调登记时来源仍CLOSED-only；单次发送MailboxFull把Ready→WaitingWritable后若本轮预算1用完，旧代码直接Parked，requested/removing/result全无，没有声明Remove，WRITABLE未登记。无限期限没有其他输入能让它再推进；量子最后只剩1是正常Runtime合同，不能提高Outbox最小预算规避。
- 固定流程改为每个有界发送尝试后再协调一次当前所需来源。掩码失配在本轮声明Remove并置removing，Gate接收责任且保持Runnable；unregistered后同记录登记WRITABLE|CLOSED，初始候选与后续电平共同唤醒发送。意图队列满则operation_retry保证Runnable，原来源与reply owner继续持有；成功/错误则按原declare_remove退休，不重复注册，不增加预算/期限或polling。
- 没有加入临时Outbox日志/新测试传输/伪造SourceId或cap：静态一步反例已经确定。复用原stress实际满邮箱→释放槽→回复验证跨Runtime预算和来源交接，host复用现有outbound/预算用例；修改在同一唯一对象链记录，后续记录真实验证结果。
- `outbox-progress-stress-001`仍使用原pacing/settle，普通回复0..122已完成，失败移至123/DeadlineExpired（failed-acceptance-20261005-133016-21228.log）。预算1缺来源问题不再吞第0条无限期限回复；132任务饱和后的拒绝Outbox另有5秒。保留原pacing证明task而非入口暂满，以Registry实际Quota回执代替之后固定2秒sleep；150数量、send10s/receive30s/拒绝5s/路线420s不变。
- `outbox-progress-stress-002`通过admission并越过provider正常退出退款、FAL业务、控制128事务，停在Tunnel生命周期旧断言（failed-acceptance-20261005-133933-24190.log）。Invitation拥有Attach后Endpoint的权限模板（含WAIT），本身allowed_signals=None；WaitOne先查WAIT后查对象是否可观察，正确返WrongObjectType。唯一工作记录和tunnel impl已确认不等待、不复制及Attach权限取交集，不能删WAIT破坏新Endpoint的权限或改内核检查顺序迎合旧断言。
- raw fixture改验证不支持来源的类型，仍借同一未消费Invitation，无重复owner；后续邀请放弃、创建端关闭、Attach返失败owner等原职责保留。无新消费者/兼容入口。下一stress验证原合同，完成必须16/16矩阵、退出退款、explicit Shutdown；若再有失败按真实点继续。

Control/affine Builder运输设计闭合与矩阵迁移：
- 一次advisor咨询以计划/代码确认：旧Control无TRANSIT、Builder仅MANAGE没有独立禁运政策依据，是可委托authority接线缺漏；历史启动契约及当前ThreadStartTicket都支持运输且不可复制。主代理依据绑定对象不扩大业务authority、已有消息事务责任链选择接通；不以既有fixture本身证明应增权，不改成测试专用同域thread/creator代Start绕开机制。
- Shell CONTROL_RIGHTS上限增加TRANSIT；ProcessCreate仍由调用者显式请求，无默认给普通SUPERVISOR/DERIVED_CONTROL增权。affineBuilder在所属module唯一RIGHTS=MANAGE|GRANT|TRANSIT，Create mint与max一致，无DUPLICATE/READ/WRITE/WAIT；标准StartupContext已允许ProcessBuilder，接受MANAGE可Start；运输按来源TRANSIT/目标子集，接收仅MANAGE。JobDerive保持源Job权限和类型上限双约束。
- Message成功仅移动原cap，失败Invocation原预约恢复；返还资格被Close撤销才公共释放。Sender/Receive回滚、discard/receiver关闭/进程退出沿原Cap::release最后owner，Pending Builder放弃为Aborted；成功Start已提交Work不因源表/接收者消散而丢失。运输会扩大可显式委托范围，记录为对外能力接线，不伪装纯内部修改。
- 原案完整不可达环收集仍非目标：Builder放入其目标Body持Receiver的队列或直接Grant回目标表可能自持，不能承诺无监督环自动释放；由capability TRANSFER门承接，同本篇既有非目标，不为当前开放跨进程正常/失败/退出路径增隐藏限制。ThreadStartTicket已有Kernel GRANT/TRANSIT但StartupContext白名单尚拒绝；若承诺标准环境交付Ticket必须同步消费者，接通条件/验证由运行时分层及本对象链后续出生责任范围核定，当前不冒充其消费者已完成。
- 矩阵靶采用RACE_CONTROL_RIGHTS=SUPERVISOR|TRANSIT，worker自控仍SUPERVISOR；building factory由调用者明确传Controlrights，普通core传旧SUPERVISOR。Builder指令移交MANAGE，终态Query移交READ；Job/Create/Seal传既有合法目标权。每个worker记录实际send成功的report_pending，失败不等虚构回执；不因INVALID假cap卡住至外层420s。
- DONE后Job已移除成员；Create-即弃报告真实Pid后，枚举只要求当前严格升序、身份来自已创建PID/存活hammer，不要8个历史墓碑。尚可派生者观察Aborted/DONE，ObjectNotFound仅在已成功创建的同域PID上按DONE后摘成员成立。最后Control移交后同样可已退休，不强求再派生；KILL回执保证实际提交，未退休者检查Killed/DONE。Builder放弃的旧Abandoned断言同步Aborted。
- 不把原TERM_OBSERVE的两个Query伪作旧drain事务互斥，DONE 后两次只读Query均须成功且返回合法终态。源码仍有少数fixture duplicate失败以INVALID传播，但send失败会被pending guard立即反馈；若发送成功后worker退出则需要真实报告/worker完成并等失败收束，精确缺口在本篇，不抬deadline掩盖。
- sub-51定向Review未发现新内核运输越权/业务owner/lastcap破坏，发现fixture P1/P2：Builder send失败原件仍在init且&&跳过Kill，再无限observe_done；gone oracle误用ObjectNotAvailable而Kernel JobDerive返ObjectNotFound；接收失败清pending允许旧回执错配。已分别声明两次发送结果，失败close仍在init的Builder/dup Control且保留原Control做Kill，再观察DONE；命令状态Idle/Pending/Failed，接收失败隔离锤，退出监督Kill，不复用旧回执；known同域PID只接受ObjectNotFound。DONE后双只读Query两者须成功，去旧drain互斥假设。
- `authority-transport-stress-001`旧oracle构建进入矩阵并通过前9项（kill/exit/thread/fault/start/park/memory）；420s在Thread suite的Tunnel close/Attach 24/24完成后截止（failed-acceptance-20261005-144626-42215.log），classification failure-after-progress。该日志不能证明Kernel deadlock；下一实际步骤是join_publication/registry reclaim/raw thread storm，添加原test_hammer实际边界与32轮join progress定位，不新造验证运行体、不抬预算。
- Review修正后源码的seven-face lint已通过authority-transport-clippy-001；启动下一序贯stress验证，不把旧oracle运行当新源码完整组合证据。

U/M 轮转实际计时与 registry 活性接线：
- `authority-transport-stress-002/003`（分别 failed-acceptance-20261005-150338-56027.log、151839-69758.log）均在 join32完成后的 registry-reclaim-start 截止420s，003的filled8/probe标记确实已编入ELF而未出现。未抬期限。全速只读GDB诊断 registry-readonly-diag-001.snapshot 在同边界取得四hart现场：均为sched/remote/返回路径，无该快照可证实的等锁环；诊断后本进程组已清理，暂停过的运行不计验收。
- 主代理源码反例：旧service仅charge(M)，从未charge(U)，Rotor起始U正余额在持续Ready时永久选U。旧service还会执行M后直接pick U，Advance也pick U，未按仲裁控制执行。registry中独立hold worker持续yield而保持Ready；后续Map的已提交Work等待M，构成真实活性缺漏。只在无Ready窗口推进M会让早期业务偶然通过，不能作为活性保证。
- 按维护分案既有统一实际tick合同接线：per-hart Execution唯一持Rotor和可空U起点；dispatch开区间，trap入口扣用户执行，syscall/fault业务开闭区间，紧急remote/timer和调度仲裁排除；Resume开新区间但不补量子，U耗尽改Requeue。Switch返回/VM与成员处置尾段亦归U，由run最终关区间；Frozen入口提前Killed允许空区间，不伪造新信用。新dispatch timer取线程量子/类别余额/最早来源期限最小值，Resume不重装线程量子。
- service返回已有Choice，只有U才pick，M每批后回安全点，Advance不睡且不pick，Idle进入既有双检。M批按真实ticks测量全部领取/推进/结算/摘环/锁外释放；领域Work::step只返Outcome，删除15处伪1/16成本及无人消费的grant参数。每步仍是一份既有领域有界原子，实际超过DRR grant或M预算全额记负债，不拿节点数量当ticks。qU/qM/账户量子数值未增加，完整原子上界与政策定标仍在本链。
- Account负责维护Ledger且强持父Account；最后Work/Record在锁外析构后才settle，子Account最后Drop把剩余负债移交父账本。原MemoryPool::Drop提前absorb已删：资金Pool可在最终Work析构时先消散，不能要求此时维护grant已经settle；账户独立寿命持有计时/欠账，避免最后Pool毁于in_flight=1而panic或丢尾债。finish后的新激活/并发领取仍按各自grant结算，无新状态或临时工作层。
- 开发检查 rotor-progress-check-001与七面rotor-progress-clippy-001通过；maintenance既有19项host通过（rotor-progress-host-001.log，显式aarch64-apple-darwin）。原stress420s/50%真实消费者已启动，结果待记录；sub-52只读定向首审正在进行。未新增harness/服务/伪造owner或compat入口，未提交、未归档。
- rotor-progress-stress-001 / failed-acceptance-20261005-155147-86308.log：新计时原路线420s退出124，停在旧admission入口背压request61，Registry并非Quota。故障阶段早于registry，不能报本轮registry已通过；init失败后稳定监督仍回收全部服务，没有panic/heap source failure。
- 旧stress_fal2_admission已删除函数与唯一调用（srv_init）；工具没有要求其单独锚点，无兼容锚点留存。源码先遇MailboxFull就停止发请求，再断言132任务饱和，但同步Send只证明入口队列满；固定每8条sleep100ms也不提供真实task饱和事实。150满回复邮箱与5秒拒绝政策的组合亦无统一有效期保证；继续调节节奏会为矫正态旧FAL维持无契约fixture。用户已明确授权删除旧验证与不补旧模块，按此范围清理，不改FAL服务、额度或期限。此前probe找到的Outbox预算1缺来源反例独立成立且修复保留。
- 覆盖缺口唯一登记：新FAL服务重设计后需提供可观察的准入饱和/拒绝/恢复契约，并以真实owner与来源推进验证，不能拿入口MailboxFull或sleep替代业务task状态；owner为FAL/服务重设计，触发为该分案详细契约及实际消费者接通，届时替换本条并运行同域正常/拒绝/取消/退出退款组合。当前内核对象链不承诺旧FAL在任意发送节奏下产生Registry Quota，host既有业务预算用例不冒充这条跨组件覆盖。
- 删除probe后rotor-progress-clippy-002七面通过；rotor-progress-stress-002沿原420s/50%重启，结果待记录。
- sub-52首次定向审查：两项P2已修正并由主代理按原finding复核。①真实成本终点需包含结算等锁，但clock失败不能在MAINTENANCE ticket锁内park；锁内只取Result，失败先drop守卫再request/check停止。②trap入口已Frozen早退也在紧急remote之后开启U区间，使汇编页表归一/Running离场/等待意图清理/reap尾段计入U，不随Frozen交错改变计量。此前“早Frozen空区间”描述作历史，最终行为以此修正为准。审查确认并发finish/再领取、Account晚于Pool与父链欠账安排未发现可证实错误；不作为完整竞态验证。
- 主代理收敛额外发现：旧pick按2*ring_len+1扫描并宣称两圈可领取，但真实时间负债可超过两量子，推导不成立，临界区亦随账户数增长。改为一次仅访问一个账户、先旋转游标，Skip或在途空账户返回安全点。Ready电平仍真，service下一次正常仲裁，不进入idle；无新状态/重试层，队列临界区O(1)。sub-52结果形成时尚未含此修改，主代理对旋转/Skip/Ready/settle自行复核，不另启动重复首审。
- 最终账户计量边界登记：grant涵盖step/finish/记录析构及结算等锁，之后的settle/摘环/最后Account析构、parent欠账移交/metadata退款时间由外层M批全额承担，尚未计入该grant的DRR成本；不能宣称账户全生命周期精确计量。owner为本链维护执行切面，触发为完整成本/原子上界闭合，须明确共享仲裁成本与应归父付款账户的销毁成本并验证尾部归账/无锁停驻；不引入为计时而嵌套的临时Work或改变金融退款责任。此项与原政策定标/δ上界一起闭合后方可交付完整维护能力。
- rotor-progress-stress-002 / failed-acceptance-20261005-155738-88331.log：旧停点越过，registry填满63worker（entry占1槽），join探针、真实Prepared drop退款和连续3次再准入通过，raw-thread-storm及thread-memory-suite完成。随后Tunnel退出真实消费者在Attach遇RightsDenied并panic，路线最终420s退出124；不报整体验收通过。此运行使用审查修正前计时/两圈pick，只作开发因果证据。
- Tunnel退出fixture迁移缺口：spawn_tunnel_exit_target把Invitation只授READ，真实Attach claim需要MANAGE，派生Endpoint的共享读写View还需要READ|WRITE。HandleGrant现明确授READ|WRITE|MANAGE，仍不授DUPLICATE/TRANSIT/GRANT等业务外权限；邀请仍由Spawn/目标唯一owner负责，内核权限检查和普通View路径不增加兼容特例。原wait Invite WrongObjectType及Control/Builder运输修复仍保留。
- 审查修正、单账户visit与Tunnel真实grant后rotor-progress-check-002、七面rotor-progress-clippy-003通过，原条件stress003重启；结果待记录。

release Job 完成边界收敛：
- rotor-platform-virt-release-001 / failed-acceptance-20261005-160731-96457.log 实际业务/服务/root辅助清理与显式shutdown完成，但唯一 FAILED 为job seal completion（jid5, Dead, root children left1），脚本exit1。未判为编译、超时或内核panic，平台序列已按首个失败停止，其他路线尚未执行。
- 当前测试seal后以At0探测CLOSED却把wait Result::Ok当完成，再立即要求父表无child；这不等同真实电平等待。源码另一方面把PublishClosed与UnlinkParent拆两M步，正常CLOSED观察亦可能领先父表删除。已确认Job完成使用CLOSED（等待分案的正式ABI），不改成Process DONE或保留两种信号。
- 决定在现有唯一Job Work一次有界原子中：只移除一个父表节点并锁外释放→发布本Job CLOSED→若本次摘除令parent complete_if_ready成立则锁外发布parent责任→Done。删除CompletionPhase及独立phase锁；JobInner::dead的单次转换是唯一责任出生门，WorkRecord拥有排队/运行/Done真值。根Job无父表仍走自身发布。树高由既有direct-entry上界限定，不扫成员、不沿祖先递归推进，实际时间进入DRR/M计量。
- 消费者fixture改为既有5秒正绝对期限等待并确认CLOSED电平；不是新增polling/放宽原正常业务期限，纠正旧“空JobSeal同步完成”的假设。Job Snapshot Dead仍描述本Job已sealed且两表空；CLOSED新增实际父表移除屏障，消费者不以单独Dead快照证明父枚举收束。正常/最后cap/重复Seal/父子完成仍由同一owner路径接通；不增加旧Job接口兼容。
- Job完成单步实现与shared ABI/libkernel文档同步后，rotor-progress-check-003通过（删除phase后残留unused ranks告警已清理），rotor-progress-clippy-004曾被该unused import拒绝，修正后rotor-progress-clippy-005七面通过；git diff --check通过。未忽略-D warnings或保留旧phase接口。
- 主代理Job切面结构复核：complete_if_ready在唯一JobInner锁里dead false→true只产生一次责任；移除父节点只在父门内，完整node退款在门外，self始终由Running Work强持；若父也完成，当前Work本地强持父直到self CLOSED后发布父记录，Seal/rollback/remove的竞争者看到dead不再重复发布，责任不会因cap消散丢失。每层一次工作不递归推进，重复Seal保持幂等；snapshot Dead与source CLOSED各自语义在shared和封装已明确。无新ABI布局/call号/旁路授权。
- 新版本rotor-platforms-002：virt-release、sifive_u、virt-nofd均exit0并满足core完整锚点（日志各为rotor-platform-<route>-002.log）；sifive_u以完整core+明确reset NotSupported终态收割判定，符合既有平台边界。virt-hetero默认40s退出124，failed-acceptance-20261005-161643-3163.log截止仍推进FAL正常业务，Base64域{0}、D64域{1,2,3}与FP验收已观察，没有业务失败/panic。不能由超时宣称内核死锁；序列首失败后停止，boot-failure/stress尚未执行。
- 异构域临时开发诊断仅覆写VIRT_HETERO_TIMEOUT=120，THROTTLE仍50，源码/业务期限未改；rotor-platforms-003顺序复用hetero、boot-failure、stress真实路线，外层510s保障正常420s stress期限。先取得完整耗时与收束证据，必要时再按BUILD-AND-TEST校准默认路线，不以放宽期限宣称原40s默认通过。

本轮最终验证与交付前结构检查：
- `rotor-platforms-003.status`：hetero exit0（原40s预算截断后，临时120s路线实际约51.9s完成，来源为run_id开始时间至最后QEMU日志写入；含全部core锚点及明确reset）、boot-failure exit0（panic/alloc/fatal三种注入均4hart Gate Failed/Parked）、virt-stress exit0（最终Job实现下16/16竞态、registry63worker、Prepared退款/再准入、Tunnel24轮/跨进程退出、全部服务监督和explicit shutdown）。日志为`rotor-platform-<route>-003.log`，最终stress原始`qemu-acceptance-20261005-161940-7251.log`。
- release/sifive_u/nofd最终Job版本证据在`rotor-platform-<route>-002.log`，均exit0；启动失败检测未触及运行中U/M故障窗口，三例不能替代clock epoch失败/在途grant的动态注入。19项host只验证纯逻辑Ledger/Rotor/Record，正常真实平台运行不能替代内部安装/取消/输出撤销/资金失败交错。
- hetero旧默认40s已被实测52s正常负载证伪（单hart Base64 core仍推进，无业务失败）。依BUILD手册校准Justfile独立默认至90s，与普通virt core默认同档；保留独立覆盖入口，不改kernel时钟/业务deadline/额度/调度归属。`rotor-hetero-timeout-dryrun-001.log`解析出正式recipe90s；已通过的120s诊断实际小于新默认，不再重复正常路线，明确未额外运行新默认90s。
- 主代理按REVIEW核对本轮已接切面：U/M同量纲及唯一起点、Choice仲裁/Skip与Ready活性、Resume/早Frozen/非Resume尾账、锁外clock停止、最后Work/Account/父账本顺序、Job单次责任与父摘除/本层CLOSED/父发布顺序、Invitation消费授权与实际映射读写、shared/封装/消费者和impls同步、旧phase/伪成本/probe删除。该检查完成；Account最终DRR尾账及完整原子上界依登记仍未闭合，不把本轮切面通过登记为整项能力完成。
- 子审两P2已按原证据由主代理修复/复核；当前没有为本轮已接契约保留已知正确性欠账。源码/文档/日志各按当前事实，维护分案旧“未接内核”接力已改为对象链唯一入口，KNOWN_ISSUES保持矫正区并明确本轮已过平台。COMPASS原入口仍准确，无需重复进度。所有本轮QEMU/GDB/临时launcher已经退出并清理；未提交、未push、未归档。

付费计划 scratch 收敛轮次（继续当前唯一闭包）：
- 当前Code事实：MemorySpace 的 validate/reserve 在 ADDRESS_SPACE 内构造多份裸Vec；reserve成功会析构验证存储，rollback/complete也消费并析构阶段载荷。仅将Vec换Buffer<C>会使MetadataCharge在VM300内退Pool230，不能作为接线方式。目标prepare/付款门外、同门复检容量/状态后只移动，所有废弃/失败/最终storage必须归门外owner。Operation本身仍是唯一失败/回滚记录，不另造wrapper Work。
- 先消去逻辑冗余分配：covered_indices的key Vec改为已校验Coverage只读视图（首/末key、数目、借同OrdMap），后续借该视图迭代；key是VA起点，不当密集index区间。范围覆盖、authority、guard判定仍在原唯一检查路径，持同一账本借用保证随后迭代不变。normalize_templates改为原数组sort+读写游标相邻归并+truncate，不再复制临时Vec；RegionTemplate为Copy，顺序/归并predicate及几何结果不变。
- 付费Storage<C>贯穿Validated/Prepared/Committed/Retiring的具体接口正在根据实际所有权收敛；advisor单次咨询仅辅助新paid scratch API决策，不重新讨论已有Funding泛型方案。验证/物化所需上界按本次目标region/结果lease投影与guard推导，避免每次预付整个4096 ledger容量或新建异构Arc arena/allocator。返回/错误/rollback/complete各阶段的锁外释放必须在同一设计闭合后实施，旧裸Vec API直接替换，不为中间态补兼容。
- advisor sub-53 单次咨询已收集，主代理选择分阶段按实际需求准备的单一ChangeStorage<C>。独立Validation/Materialization释放wrapper不采用；前期数组的真实容量默认保留至操作收束并持续付款，未来仅在实际占用证据需要时增加明确锁外释放出口。C只传播到实际owning token/错误owner；MemoryObjectState仍只拥有业务计数，许可输出按方法泛型借调用者已付Buffer，不给状态加金融泛型。
- 确认目标类型：ChangeStorage<C>独占validation keys/templates/retire templates/translations/lease segments/requirements及物化Region/RetiringFragment/supplied/retiring WritePermit数组；ValidatedChange<C>、PreparedChange<C>、CommittedChange<C>、RetiringChange<C>沿阶段移动同一storage，Range/lease/growth等计划事实仅保留一份。UserWriteLease变借用投影，不再独立own段数组。validation容量描述不等于预约，validate最终同门复检不足返CapacityChanged且不取storage；门外按不足字段单调补齐。reserve物化容量先在门外按已验证长度补齐，最后门内复检snapshot/arena/permit后无可失败分配。ReserveFailure<C>原样返validated与供给许可；commit/synchronize只移动owner，rollback/complete返回storage而不门内析构。
- 删除面与依据：普通Unmap/Protect直接用remove作为snapshot，不另clone；no-op Protect保留独立身份快照，Map lease以已有segments内RegionKey/geometry/RW投影复检，删平行lease keys。object_region_deltas/retiring_object_capacity为旧对象批退休表示（仅剩host统计），实际Views/Backing按fragment与permit责任退休，不维护第二套聚合真值，本切面删类型/统计/接口及仅测试旧表示的断言，保留实际几何/offset/许可计数验证。不因单独“无即时消费者”删除抽象，依据是责任owner已整体替换且表示重复。
- 退休收口：memory_space RetireBatch合并进RetiringChange，synchronize只有一个owner出口，RetiringChange给逐fragment/permit的固定单元接口；内核固定表帧RetireBatch保留其独立fence/K依据。rollback返回Storage保留replacement中每份写许可，锁外每步只消费一个replacement或supplied许可，不在VM里重组数组或一口气取消。许可总Buffer预付Σcount，每个对象直接在对象门内追加，unindexed仅标识该对象与后缀起点，hold_writes失败先取消这段再处理已有索引段，删per-object Vec搬运。
- 几何容量：Map模板数为usable1+实际guard，输出lease实际l段；Unmap实际n remove/n退休模板，区间外残片a+b≤2；Protect changed最多n+a+b+2模板/n+2 remove（两个相邻吸收上界），退休n、translation真实q≤n；no-op只需n身份快照。物化Region/retire片段按normalize后长度，退休许可按带permit实际remove数；请求/端点物化后同门重新计算复检，不预付全部4096限额。所有key mint、arena reservation、Object许可计数修改均在足容量证明之后，cookie之后只移动/填已付槽。
- Operation是失败根：所有候选storage、validated与reserved阶段owner保留在Preparation直到移交，不能把局部paid owner留在已发布Rollback之外并让DONE先于局部析构。成功submit在work.start之前mem::take并锁外释放整份已空Preparation（逻辑empty不等于容量0）；失败则原Record有界接管。access lease与DONE晚于所属scratch真实释放，独立Work/Source出生费及Account尾账仍按原剩余门承接。
- 已实施的独立接线：funded_storage显式try_reserve/try_reserve_exact由调用者给Funding、同时准入旧/新块峰值，复用已有replace_storage不加unsafe；失败原样保留，clear/truncate不退款。Kernel游标/表帧reserve/boundaries采用Buffer<_,MetadataCharge>，Metadata来自Operation选定Pool，物理表帧仍由VM account按既定来源付款，两者不混同。page_table分步仅从调用者Iterator取得FrameOwner，不依赖Vec或金融类型，无自分配或逆领域回调。现有host page_table消费者同步标准pop迭代器，未保留旧容器特殊接口。
- 开发证据：memory_space收敛后43项（unit10/backing9/planner24）host通过；funded_storage9项（unit1/integration8）通过，新增1项独立峰值付款拒绝/保留数组费用契约用例复用既有Payer；page_table38项（1+5+32）通过；scratch-kernel-check-001通过。所有host显式aarch64-apple-darwin。当前账本paid token替换未完成，不能把上述视为scratch全闭合验收，不为中间态建兼容或临时服务。
- 主代理完成账本token、Object输出与Kernel接口直接替换；裸Vec、归一化副本、覆盖key副本、lease keys、普通remove平行snapshot、ObjectRegionDelta/retiring_object_capacity、账本RetireBatch均已删除。page_table host签名与真实Kernel同步。元数据错误由StorageError/选定Funding直接传递，不再混入纯ChangeError/ ObjectError的AllocationFailed；Object输出不足为CapacityChanged，所有计数前置检查后才填许可。
- sub-54首次定向审查P2（原始finding保留）：返回Storage合法复用时旧供给许可空Buffer容量在副作用后触发assert，Rollback保留旧退休描述还会重复生成retire fragment。主代理以更少owner修复：reserve只借用调用者的paid permit Buffer，失败保持许可与存储在原caller、成功只移动许可到Region；ReserveFailure只返Validated，删除Storage供给Buffer和错误里重复所有权。prepare/validate复用时清普通回滚退休描述，Region许可必须先逐单位取消，完整Storage仍统一释放。新增两条真实pure planner回归：complete后同一Storage/供给Buffer连续创建两个writable view，实际许可数2并逐退归0；Rollback的Unmap描述复用后只产生1份对应真实region的退休片段。主代理按原证据复核通过，不另开重复review。
- Financial host新2项（payment.rs）只复用正式Funding与阶段API：以被动C::Drop标记证明validate/reserve/commit-step/rollback/complete在账本门内不分配/退款且转换前后live费用不变，caller门外逐许可/region收束并Drop后归还；partial Funding拒绝后caller仍持已付prefix、无事务/region出生，显式释放后回基线。不是模拟VM/PageTable/hart的新harness，不把host门标记替代Kernel真实锁序审阅或目标运行。已修改既有hostconsumer直接用borrow reserve/单退休token；删除旧统计断言，几何/offset/许可等原24项保留。
- 进一步实际分配盘点确认funded_frame::Funded本体固定数组，不需为其制造heap付款任务；Kernel Backing.from_funded原extent Vec是真实caller scratch，改为按frames.extent_count锁外预付Buffer<FundedExtent,MetadataCharge>。Frame.into_extents只借预付输出，抽取不分配；分解拒绝返回错误，已抽出与剩余physical/credit各由output/inner归还，不panic。真实frame/selftest（funded-frame/页面清零/physical credit消费者）同步新接口，先释放数组再核对物理baseline，不恢复已删除的旧capability/wait/drain自检。
- 栈上界开发失败保留：scratch-route-virt-001在启动QEMU前audit失败，10个debug函数超过0x3000 guard（Work::step0x8300、Arc构造0x5c50等），无运行证据。内联paid Storage含C导致token/Phase复制放大，不能放宽guard或关audit。主代理改为单一已付款OwnedBox<StorageData<C>,C>，空candidate不分配，低门外prepare才付存储头和数组；所有阶段只移动小owner。没有异构arena、Arc共享载荷或新的Work层。
- 首次box后scratch-box-kernel-001还剩2超限（0x35c0/0x31b0）。同时发现Prep的candidate/validated/reservation实际互斥却并排占位；合为PreparationPlan::Candidate/Validated/Reserved，空默认也是Candidate，删除两Option及并存组合。既有类型状态getter/take组织该唯一owner，原Record Rollback直接接管当前变体。scratch-box-kernel-002构建/audit通过，最大单函数0x2d70<0x3000；硬件guard、Formal stack、审计阈值均未改，无临时“热函数豁免”。此状态不能凭阶段结束单独宣布任务完成。
- 容量同时精简：requirements按实际可能的object writable模板上界准备（匿名/只读/no-op为0，Map最多1，Unmap至多两writable外片，Protect为对象目标片+实际writable外片，邻吸收本就禁止writable），不拿全部guard/template数代替。每块按真实保留capacity持续付款，header亦有实际布局charge；View/region事实与snapshot/lease复检仍各唯一。
- 开发证据：scratch-ledger-host-006及scratch-box-host-002均47项（unit10/backing9/payment2/planner26）通过，显式host target；scratch-ledger-clippy-002七面通过（boxing前），scratch-box-check-002通过；boxed最末版lint与真实路线将继续。旧stage自引用“materialize returns affine supplied storage”描述被本段borrow模型替换，当前不会让局部unused paid owner落在异步失败根之外。
- boxed最新scratch-box-clippy-001七面通过；scratch-routes-002的virt/core完整锚点exit0。但stress420s最终exit124，15/16（失败thread-memory-suite）；同址63worker填满、join probe、Prepared drop/三次再准入都通过，随后libthread cleanup完整性检查收到Unmap(ReachLimit)并中止PID57。原始failed-acceptance-20261005-182705-21670.log；其余平台顺序首失败后未执行，不能沿用上轮证据替代当前版本。
- 未直接增metadata/transaction额度或改runtime重试。scratch-routes-003是同源唯一定位运行，临时锁外打印ReserveFailure种类，证明多次TransactionLimit而非元数据或物理资金失败；registry63退出并发reap已接受/等待的transaction超过当前16个在途槽。日志artifacts/.qemu-acceptance-20261005-183653-25466.log；确定种类后中止本诊断进程组，状态-15，不计默认420s验收，所有私有QEMU/timeout/节流子进程收束。临时诊断log语句已删除。
- 错误分类机制修正：TransactionLimit仅表示当前in-flight transaction槽忙，随既有M责任Complete释放，VM mapper映为既有ObjectBusy；PageLimit/RegionLimit/KeyExhausted仍ReachLimit、真实Funding拒绝保持原明确错误。已有libthread只对ObjectBusy保留claim/stack并恢复Live，在既有yield/reap路径重试；不改consumer、容量、deadline或新增等待/退款机制。Provider的pure ChangeError种类不变，Kernel统一ABI分类拥有边界。错误发生在Cookie/Commit前，Scratch/许可证owner仍由原Operation稳定Rollback，既有BackPressure不是完整性损坏。
- scratch-admission-check-001、scratch-admission-clippy-001七面通过；scratch-routes-004用正式默认条件顺序执行stress（420s）、sifive_u、release、nofd、hetero（90s）、boot-failure，结果待记录。当前编译/host/栈审计/首个core通过仍标开发证据，待真实stress与低内存/platform收口；没有新兼容接口、旧FAL/old kernel gate补丁或预算回退。

actual scratch 切面收口、验证与下一步：
- 最终正式运行：scratch-routes-004.status 全exit0，virt-stress完整16/16 + registry63回收/再准入/Prepared退款 + passive/forced/CallerDeadline/Tunnel/监督shutdown组合；原始qemu-acceptance-20261005-184425-29142.log。sifive_u低内存/core及reset NotSupported明确终态（184622-33043），release（184713-34869）、nofd（184727-35598）、hetero正式90s默认（184813-37378）均完整core exit0；日志各scratch-route-<route>-004.log。boot-failure panic/alloc/fatal均四hart Gate Failed/Parked。此前最末boxed core已于scratch-route-virt-002/qemu-acceptance-20261005-182617-19911.log exit0；改动错误分类由最终stress/各core真实路径覆盖。
- 栈/构建：debug kernel ELF audit最大0x2d70<0x3000（Work::step），release最大0x1610；LP64 soft-float ABI及FP局限目标段检查均通过。只调整所有权表示/互斥载荷布局，guard/stack/阈值与formal平台规则未改。scratch-admission-check-001、七面scratch-admission-clippy-001通过；47 memory_space host（10+9+2+26）以及同cut funded_storage9、page_table38通过，所有host显式aarch64-apple-darwin。whitespace scratch-diff-check-001通过。检查通过后未重复正常路线。
- 主代理交付前REVIEW（本切面）完成：实际动态分配逐项核对，planner/页表单步/funded-frame固定本体免动态费有代码依据；header/各数组/游标/boundary/储备/extent无裸Vec或未付容量，财务值随实际存储而非len持有。准备门外/最后同门复检及容量变化重试、Arena最後可失败步骤与身份/许可副作用顺序、错误owner原样保留、unindexed后缀先取消、cookie后只填已付槽、Commit/分步账本/fence/精准View退片段/最后边界同门复查、complete带出头及final table ack/DONE排序均复核。正常、输出失败、取消/退出/退许可、源Close后已接受use与Pool关闭强持付款链使用原Record，无新增actor/嵌套Work。纯逻辑型与Kernel消费者、host/Frame实际消费者均同步，不保留旧API/default C或single-side facade。
- 结构减少：Coverage只读视图取代索引Vec、模板原地合并、remove/lease segments单一快照；旧ObjectRegionDelta/对象计数聚合与第二账本RetireBatch删除；调用者供给数组单owner、不按对象重组/分配；同一paid存储头在阶段间只移动小值，Prep互斥变体取代并排三载荷，提交前统一释放剩余已空Prep。付费头不是新领域对象，不新增公开capability/调用号/ABI布局。
- 限制保留：真实路线只验证已有可观察正常/竞态/退出组合，不宣称内部任意Funding失败交错或跨hart安装窗口全覆盖。新host finance标记仅证明纯API不在consumer领域门内触发付款/退款；Kernel实际锁序仍以源码审阅和目标路线为证据。旧未收费的独立对象、Work/Source/消息及Account最终尾账不是本cut付款完成事实，仍在同一闭包，最终资金屏障尚未宣告。47host/16场景不能替代下一切面的真实出生费/内部失败/维护边界证明。
- sub-54原P2修复与两回归复核已闭合；boxing/Prep布局复核由主代理承担，不再开重复首审。impls(mm/MemoryObject)、KNOWN_ISSUES与本篇当前接力已回填；COMPASS原唯一入口仍准确。所有本轮QEMU/GDB/临时launcher退出，未提交/未push/未归档。剩余按当前343行两大责任切面继续，不为旧模块补兼容；延期项（FAL policy probe/已有Sent Timeout组合风险）原唯一记录与触发条件不扩散。

独立出生付款轮次（当前设计与施工接力）：
- 基本费用合同沿既有MetadataCharge::arc<T>/layout：自然字宽live-layout槽，Arc控制头+值对齐/padding+内嵌Work/WaitQueue/Frame字段只计一次；独立堆数组/Box/Arc逐真实分配持各自费用。退款晚于全部值/字段析构，field放宿主末尾并强持真实payer；外部Weak的Arc尾存储不构成硬heap隔离承诺，不能把逻辑len/业务CLOSED/DONE直接当全存储析构。Pool/Account父付款、buffer/node/header费用已接，不重复。
- 金融来源：有显式CREATE Pool的出生使用该明确Pool，隐式派生、Observe/Once、消息Send用执行者Body funding_pool；内部持久Shell/Body/Thread/VM/Source/Work用该构造选定Pool，队列容量归Receiver对象Pool，消息payload/运输scratch归发送者本次Pool。payer与capability身份/使用者分别建模，运输不改原payer，已接受责任持payer直到真实释放。跨组件隐式生育若现来源不明确须在本篇闭合，不根据旧未付款字段臆断必须兼容。
- Flat Core/WaitSet/SignalSource/Notification/Lifetime/MemoryObject/Delivery等Arc生前锁外取得MetadataCharge::arc::<Self>再构造，末字段charge承担refund；仅为保活的_pool由charge取代，真正存储政策入口沿真实payer。构造失败沿原局部owner/Record收束，不能在高于Pool的Object/VM/Views/Set门内销毁金融owner。Handle门100低于Pool230：已消费的cap输入scratch在commit迭代器末释放；固定批次用栈数组，其余payer由当前Caller Body或新安装Delivery持续强持，不触发父付款链最终析构，只有明确有界的当前费用归还。Authorization.accept_with只做短领域凭证，原cap保活Object clone；Source/WaitSet拆node、Entry.detach、Cap释放仍走原锁外出口。
- 真正Message费用与队列：Mailbox固定capacity FIFO不留会隐式扩容的VecDeque，使用已付固定Buffer与领域内head/len；Message本体内嵌槽，不另造Arc/对象费。payload、handles与发送/接收/安装scratch必须有真实Buffer owner，先付款再读取/claim/预留，错口径/Quota失败完整归还，成功运输原owner，Receiver consume/discard/shut锁外释放，接收发布失败原message仍持责任返队列。handle install原Vec签名直改所有真实消费者，Host/ABI布局不分叉。
- Shell/Builder/Control/Ticket/Thread/Departure、VM/Views/Backing/Retire/Change等独立live存储按实际Arc布局付款；BodyStorage为OwnedBox、销毁后退Box费用。UserContext/FpState/Frame为Thread内嵌字段，随其布局计一次，不制造不存在的独立Box任务。末live在哪个锁内消散逐点复核，必要时返回完整owner到锁外，不在高门加charge.Drop或新退休actor。bootstrap/root沿root财务来源，已有父支出的Pool/Account不重扣。
- private Work退款随birth费用闭合，持久公开观察壳继续last-live收费。sub-56纠正计时事实：advance_one移动work到队列/停驻根/retired，drop(retired)在结算取时前；Account和摘环/债务移交尾部计量仍未闭合。新Completion末字段先销毁私有字段/退宿主费，再返回access与通知来源，不保活被等待Work、不用strong_count。RetireWork激活时由Body交出执行根，复用Shell既有SignalSource内部完成槽：仅标internal_done/锁外返唯一WakeToken，不提前公开DONE、不另出生可能晚退的私有Source；父随后DropBody/发布Process DONE。独立公开Source可由真实观察loan付费保留。Thread末字段先退费用才请求Departure；Departure末字段先退费、Thread DONE，再减少父member_count。Ticket Prepared/Pending只持Weak，真实Created成员拥有执行容器，外域票据不能阻止私有退款。Builder Start将原已接受ingress从短BodyLease转为无Body指针的WorkReceipt；末字段退宿主费→归还回执→私有启动DONE，Frozen WaitIngress不能越过尚活启动宿主。
- sub-55构造/金融来源盘点已收集。上述Arc宿主与Body Box、Mailbox固定已付环、Message payload/handles/输入/claims/rights scratch、ProcessWrite/Grant与WaitSet receive scratch已接；成功运输原payer，接收Delivery槽发送时付款，恢复消息不扩容。MetadataCharge.payer提供真实付款Pool，单/双cap批次改栈数组，Handle install直接使用ExactSizeIterator，不建旧Vec adapter。Thread动态1/2 cap输入使用显式paid Buffer。
- 主代理末根审计与sub-57首审：P2原始finding为VmAccessLease先减access计数/唤醒、字段VM Arc随后才析构，可能使Process DONE领先私有VmShell费。已改同VM门内在非零租约/Body与退休owner保活的证明下先交回本强根，再发布计数归零；父同门复检，解锁前owner稳定。RunPermit同类窗口亦改为VM强根先释放再减少Shell.running。一个raw指针临时借门有明确SAFETY，解锁后不再访问；没有Weak升级后的悬挂强根或快照计数判断。主代理按原交错复核；BuilderCompletion/WorkReceipt补充由主代理自行审阅（首审完成后），不重复新开reviewer。
- 既有srv_init Pool预算用例追加真实metadata拒绝：page-only子Pool页足够/meta=0，创建非空Tunnel必须QuotaExceeded，恢复typed owner后同Snapshot证明没有页/metadata残留；预期来自独立非空存储必须付款契约，不复制Kernel layout或制造临时consumer。现有Tunnel失败owner/最后View退款真实用例同时核对metadata归零。精确内部allocator OOM/取消/迟到窗口仍按原唯一缺口登记，不能以此正常用例全覆盖。
- 开发证据：birth-flat-check-001、birth-check-004/005通过；birth-clippy-001七面与birth-virt-001 debug core exit0。003只因Buffer非自动借用Iterator编译失败，改显式iter/iter_mut后004通过，不加包装实现。前述clippy/core运行早于最后Builder回执/VM根顺序及metadata拒绝用例，最终just acceptance已启动pid49807，artifacts/birth-acceptance-001.{log,status}；结果待记录，当前不能报最终组合通过。账户尾部DRR、父链完整析构上界、政策定标与更深资金失败组合仍在本链，出生费验收后继续。

设计轮次：Account最后计量与完整M原子（承接出生费后，当前已确认内部实现选择）
- 出生费最终证据：just check artifacts/birth-check-final-006.log exit0；最终just acceptance artifacts/birth-acceptance-final-002.{log,status} exit0，七面lint、stress16/16、release/sifive_u/nofd和panic/alloc/fatal全通过。003异构在当前消费者修正后exit0（最后DebugPrint付款之前，后续组合按新源码验证）。新增DebugPrint临时读取Buffer由执行者付款，动态运行scratch没有免费Vec残留；impls已回填task/mm/ipc/memory-object/tunnel/call，未提交。
- 异构原失败/诊断保留：birth-hetero-001/diag-002 exit124，failed-acceptance-20261005-214637-58413及20261005-215209-60078；诊断证实Status(Busy)，不是Quota/OOM/Sent Timeout。主代理亲读并收集sub-58链路：Outbox入箱Sent后还待runtime unregistered才finalize，server.rs:2829 commit_take才清take_reserved，backend.rs:1456 Read快照在该窗Busy。只改test_fal消费者在原120s绝对期限内重试该已支持Busy，仍断言空Blob/0cap、其他错误失败，删除临时debug，不改FAL服务/库、不延长期限、不删真实用例。此证据关闭本次即时Read假设缺口；旧Sent Timeout/FAL重设计债务仍原位。
- sub-59单次Account计量咨询已收集，采纳Account Dtor主动收束：从metadata的payer取唯一parent Account（构造接口不允许错配两个parent），Dtor先take父owner保活，再测自身metadata完整释放/付款Pool收束/债务移交/等锁。M执行区间在QUEUE同域直接charge父Ledger，不补quantum、伪造grant或挂环；U dispatch中的非M销毁归当前U区间，紧急服务/共享仲裁仍沿显式排除边界，不把非M时间另注入M账本，旧M债仍按父链移交。子in-flight必须0是结构断言。末尾解锁后释放父owner，各祖先逐层独立计量，root永久anchor为最终债汇。
- 单独hart-local类别只读事实及累计已归父cleanup ticks，与Execution分离，避免service持&mut Execution时Dtor再次取同一&mut别名。M grant取得始末快照，实际Work成本=完整elapsed-该区间已单独归账cleanup；两区间可加守恒，U/M类别用原完整elapsed不减。父并发in-flight的direct charge/child absorb同QUEUE线性化；非M费用不再借M信用，选定线程后的U区间须覆盖续体/失败/reap及最终owner释放。不新增清理actor、payer栈、strong_count判断或永久保活死Account。时钟失败仍完成已持owner退款/原欠账移交，锁外请求既有全局停止，不锁内park、不假零成本继续公平服务。
- Account metadata/付款Pool收束归父；其后父析构归更上父。pick/债务skip/环仲裁、结束采样后的固定计账/解锁、Account T析构后Arc控制块deallocation属于显式共享尾部：Work区间内仍被外grant包含、区间外由M整个service包含，不声称Dtor能测自身返回后free。完整一次原子需要乘实际最多析构账户/Pool链数量，不能只计一次free。
- 公平同步owner：默认Kernel Spinlock及Talc RankedRaw入口统一既有FIFO RawTicketLock，删除无公平CAS实现/平行TicketSpinlock别名；链式锁阶与IRQ保护保持。RankedRaw也明确获取前关本地IRQ、持有者存前状态、解锁后恢复，使每hart每锁至多一个票成立；Try仅空队列发号，serving Acquire后预留，修正现Ticket try_lock的Relaxed serving不能获取前owner Release写入。等待至多H-1个owner临界区，完整界仍纳入临界区内部更高门与数据操作，不能把FIFO名字当全界。
- 私有保留数组：funded_storage Buffer提供显式forget_values（mem::forget初始化值语义，仍保留真实allocation+charge owner）；CapTable occupied=0证明每槽无authority，Members count=0证明entry=None时在Dtor丢初始化前缀，随后bulkfree/退款，避免Body末步重新扫已逐项收束的容量。不作unsafe跳过活数据、不改变非空通用CapTable原析构行为，dropcap最后事件仍原责任链。目标是依据领域zero-live真值，不能按业务CLOSED就丢存储。
- sub-60/talc与sub-61/真实Work盘点均已收集；主代理自己读Sync/Account/Ledger/Buffer/CapTable/Members/OrdMap并承担判断，具体证据及原子收束见下段。每tick政策旧值已发现跨平台频率依赖，改100µs账户、U10ms/M2ms的显式时间政策并由Clock Geometry唯一ceil换算，量子不是Work成本上限。Pool MAX_DEPTH32实际拒绝检查/最大深度与M祖先释放已开发验证，累计栈界和每原子最大链数仍须纳入正式结构账。新的每visit旋转反例阻塞真实CPU公平宣告，sub-63单次新决策咨询正在进行；不以已有计量/平台通过替代该闭合。

Account原子与开发证据续接：
- sub-62首次代码审查原P2：选定线程后的wait::resume/detach及准入失败reap在begin_user之前，account父链释放既非M且没有U区间，实际漏计。已在pick成功后用UserDispatchScope承担唯一最终关闭责任；trap仍按既有暂停/恢复路径处理紧急服务，不持Execution引用。续体/准入后先扣该U段，再按原U余额剩余值arm timer，正常/重新Park/失败continue/离场及Rust局部owner析构均先于scope关闭。主代理依原finding复核，未开重复review。
- sub-62第二P2为深链fixture用空Budget期待DepthLimit，但derive入口空Budget非法；已改非零metadata=1，并比较前后完整leaf Snapshot。prepare子Pool时Quota拒绝也核对完整parent Snapshot，metadata预算逐1下降至失败/成功，使用已部署真实Pool/Tunnel/Backing消费者，不复制私有价格或增加harness。最大Pool深度32通过；保留payer与更上观察owner的运行只证明至观察锚点的已关闭祖先M释放，不能宣称实测所有32层最后owner递归。
- 原子容器末步已补OrdMap：K限定Copy，remove在len归零前take每个V；空map Drop和take_empty_storage忘初始化节点前缀、两数组实际allocation/付款照常释放。Buffer.clear对needs_drop=false的纯记录显式清len，不逐值glue；有析构类型仍标准slice drop。ChangeStorage十份数组值均为无Drop的纯记录/affine逻辑凭证，region/permit责任由既有逐步业务接口归还，不能把无需Rust Drop当许可业务已取消。CapTable occupied含Claimed、Members count0含entry=None，与上述零live证明共同消除末Body/VM/Backing的高水位扫描。
- 固定外部free证据：Cargo.lock talc5.0.4 checksum40a5888b3189a89207eea4190e74387b5db3150d213f84186cac637dcd55259d，对应官方release ad7bae44cec09e810d531b558499bef47ca4d17b、[固定源码](https://docs.rs/crate/talc/5.0.4/source/src/base/mod.rs)。GlobalAlloc::dealloc→Talc.deallocate至多两邻居unlink及一gap link，Node为侵入式常数操作；DefaultBinning尺寸分箱为leading_zeros算术，SystemSource TRACK_HEAP_END=false，不扫arena/bin、不调用acquire/resize。kernel只走core Global，不走allocator-api2的零尺寸分支；实际std源码记录rustc1.100.0-nightly c54751567b19c4ceb08b0412d83529c2568cba8b(2026-08-22)/LLVM23.1.0，Arc弱尾dealloc不冒充值费用已refund后的heap硬隔离。
- FIFO只能给H-1先行owner数，不给未审数据操作的时间界。Talc allocate full_search_bin仍会扫bin内gap；物理系统heap几何frame::heap_capacity_bytes来自16个1MiB票、每gap至少32bytes，可给资源量界G/32，不能以free常数界冒充alloc常数界。Bootstrap仍可能在M准备allocation，正式完整账必须单列其CHUNK及该分配界；正常退休路径主要消费预付储存。现锁阶为MAINTENANCE660/HEAP900/SYSTEM_SUPPLY925。
- 补盘点到启动剩余实际birth：SystemReset改显式root Pool try_new、先付Arc live-layout费；Bootstrap私有Arc末字段收费；Loader的PAGE startup prefix容量及固定initial Cap向量在构造前按实际输入Layout准入，Fee跟真实owner生命周期、block Fee随Loader，Vec实际容量不增长，cap Fee在commit消费后收束。Boot输入只由正式启动ABI编码，不增公开格式、服务或派生权限。account-bootstrap-check-001和account-policy-check-001编译通过，后者源码包含该补漏；最终组合尚未运行。
- 开发检查account-atomic-check-002/003 exit0；account-atomic-host-os-003共90项（capability22/maintenance21/memory_space47）及account-atomic-host-storage-002共9项显式aarch64 host通过。直接成本/父inflight新增2条Ledger守恒用例；Profile/dispatcher/birth/NS policy后续只改Kernel/正式消费者，不能把早期host结果说成全部新Kernel行为验证。
- account-atomic-virt-002退出124，保留failed-acceptance-20261005-235010-80595.log：仅fixture空Budget断言，未到真实链，不能报Kernel反例；修正后003、完整U scope/static名字诊断后的004 core均exit0（默认50%）。004各hart最大grant2630930/2663610/2637330/2632590ticks，分别Retire/Change/WaitSet，Account cleanup最大3080。
- 计时定标使用BUILD-AND-TEST指定全速专项：account-atomic-fullspeed-005 THROTTLE100、真实core exit0（含Boot补漏与ns policy），最大atomic190280/grant184820/cleanup3200ticks，最长均Bootstrap；10MHz下约19.028ms/18.482ms/0.320ms。50%由tools/qemu-throttle.sh每0.5s STOP/CONT造成，观测不得作正常性能或WCET证据。Profile只读compiler静态type_name引用，AtomicPtr发布static对象地址、不分配、不保活执行宿主，Exit诊断不增加运行期逐步log。
- 硬件/栈证据开发中：当前正式debug ELF STACK_SIZE=0x40000、STACK_GUARD=0x3000，最大单帧约0x2e60；单帧通过不能替代Account/Pool祖先总链及M调用前缀累计栈界。保留原guard/stack与Elf audit，不设新豁免。
- 新设计阻塞：当前pick在每次visit开头轮转，Ledger正credit不补q，负/零才补。持续Ready A每原子1tick、B100ticks且q=maxGrant100时，两者每圈各一次，实际服务1:100；A保留正credit而B反复补q。按真实CPU等权DRR目标不能接受这个每步RR语义。sub-63对单safePoint一账户一原子、SMP已在途/新激活/父direct charge/终止给一次闭合建议，主代理随后决定并直接替换目标路径；不是重问sub-59的尾账。当前所有源码/证据保持施工状态、不归档、不提交，旧FAL policy probe/既有Sent Timeout债务原位保留。

### 设计轮次：真实 tick DRR 的双环消费（2026-10-06）

sub-63 单次咨询的完整初稿已从活动日志收集（结束输出只含后续锁阶更正）；主代理核对Ledger与kernel后采纳双环轮次。此轮只修改DRR消费，不重开Account尾账的既定归属。当前能力是同轮信用守恒的调度机制；CPU服务偏差证明需要完整原子δ与计量进展前提，不能用正路线或观测最大值替代。

- Queue拥有两个侵入式Ring(cursor,len)与current索引，Account仅一组强next/弱prev和所属ring；不新增节点/heap/单调epoch或grant代次。current为空才O(1)切两个环的角色；本轮的所有在途都保留成员，故角色切换严格晚于旧grant结清。
- Ledger拥有i128余额、in_flight和started。begin_round在无在途且未started时只补一次q；visit只从正信用预扣，绝不再补q；退款归本轮。round_complete须started、余额非正、in_flight=0。finish_round清started，保债；真正idle清正信用/started保债。原允许第二grant提前补量子的测试和接口已删除。
- 环外新激活进入next，任一环上的再激活只追加Ready；当前成员集合不会因新账户扩大。pick每次至多一账户，旋转本轮cursor，started=false才begin，预扣cap=ceil(q/HART_NUM_LIMIT)，目前链接/源码上限H=8，为同账户多hart留下正信用；q不足时并行度自然下降，不造零授予。
- 无Ready但有在途、或信用全被在途预扣时留current并返回安全点。账户有Ready且round_complete时finish_round并移next。finish仍先重新链接/保Blocked强根、门外释放记录，再在原账本settle；只有settle最后看到无Ready/无在途才idle摘环，或看到完成轮次才移next。Blocked不要求责任DONE才退执行环，后续wake按新激活进next，避免下一轮依赖被本轮阻塞。
- ring_remove是唯一结构摘链接原语，要求无在途、按Account所属ring改正确cursor/len；idle/endRound由caller按业务条件决定。移环后新环已持强根，旧next/cursor强根仍通过Pick.retired/settle removed带出QUEUE后释放，最后Account不能带任何环身份。Frame中返回固定两个Arc，不建退款receipt或临时执行体。
- 父direct charge/子债继承同QUEUE保持原义：current减少未预扣信用，next保留负债且不重开原轮，off-ring仅入账不挂空账户。父无限收到外来债时，不承诺其自有Work与无此负担账户得到相同CPU；公平成本必须包含负担，MAX_DEPTH32不限制累计注入量。
- M service每sched安全点只做一次advance_one及完整raw计时，余额留在Rotor；不再时间while调用多原子。已有U/紧急检查路径复用，领域Work不加测试专用调用。轮末可能暂时只有其他hart的在途，next有Work却等待；这个明确的利用率代价是严格累计份额公平的前提，不宣称占满全部空闲hart。

守恒/范围：令D为余额，R为所有未结grant预扣和，K为begin_round次数，C为已结实际成本加独立归账及继承负担，则持续active区间D+R=D(0)+Kq-C；只有begin改变K，退款不改变K。无外来负担、同一起点、持续Ready的N账户完成r轮后R=0、rq≤C_i≤rq+Hδ，轮内宽界|C_i-C_j|≤q+2Hδ，差不随轮数线性增长。真实费用0不强收1；若在某平台短原子总被量化为0，计量进展不能由这个守恒式推出，zero_grants诊断及领域持续进展审计是尚未关闭的门，不能宣称连续时间无量化误差公平。

i128不饱和：每hart至多一在途M原子，物理time counter剩余epoch小于2^64，H=8；不重叠归账加共享原边界后实际账户成本总量<2^67，债务只移动不复制；每轮开始前余额非正/无旧在途，正余额及预扣和受单个u64 q约束，idle丢正。父输入只减少余额。Rotor同一hart两类支出<2^64，正余额≤该类q。故i128 checked add/sub失败为上述结构前提违反，kernel外没有将任意用户数额当CPU支出的入口；不把saturating的截断结果继续当守恒。库的in_flight checked界由kernel H=8保证。

实施/验证：kernel两环、Ledger started接口及Rotor i128已接；drr-round-check-001 target exit0、drr-round-host-001显式aarch64 host22项通过，包含1:100原子成本每轮等服务、同轮并发与迟到退款、跨轮越额、direct/继承与在途顺序交换、idle丢正和u64::MAX真实成本不经i64截断。drr-round-fullspeed-001 core exit0。sub-64首次审查已收集：新双环、迟到退款、摘环强根和i128范围未发现可证实缺陷；发现下述Bootstrap宿主保活环，由主代理修复并沿原反例复核。该结论不替代尚未关闭的计量进展/完整原子与累计栈门。

### Bootstrap 完成责任与原子取证

- sub-64原P1：Bootstrap永久强持Builder，同时等Builder私有完成来源；Builder的DONE由last-live footer在退宿主费/WorkReceipt后发布。这能使已经Started的初始进程继续运行，但Bootstrap与Builder回执不能收束，退出可能卡在WaitIngress。现改Loader持`Option<Arc<ProcessBuilder>>`，Start时take、提交、在私有锁外立即drop；已接受启动由Builder队列根接管，Bootstrap只持独立完成来源。错误先freeze Shell并释放Loader再走runtime_stop；不提前DONE，不增完成actor或兼容路径。
- 主代理复核原交错：Start提交后无Bootstrap→Builder强边；队列结束才释放Builder，footer按退费→回执→完成来源顺序唤醒Bootstrap；Bootstrap detach后终结，自身费用最后释放。原错误与修复保留，不用成功启动用户现场代替footer证据。
- `bootstrap-footer-fullspeed-002.log` exit0，完整客体日志`qemu-acceptance-20261006-151402-21442.log`有initial process started、最大Pool链退款与正常Shutdown。为避免Close借用恰好保留最后payer，使旧深链用例误把U析构当M析构，正式Pool用例建立普通Endpoint View，关闭全部公开authority后核验payer仍占账，再unmap并等最终退款；M cleanup_total=2110 ticks。用例只承诺释放至保留的观察锚点，不冒充实测32层全部最后引用。
- Bootstrap MapImage/Stack/Payload的单片由公开最大请求256页改用VM既有`STEP_UNITS=16`页；WriteImage/WriteBlock仍由uaccess字节上界约束。公开请求ABI不变。分片增加View/region保留存储，真实metadata同步付款；没有重新引入旧Loader或免费scratch。
- `atomic-bounds-check-001`、`atomic-bounds-clippy-001` exit0。全速`atomic-bounds-fullspeed-001` exit0，完整日志`qemu-acceptance-20261006-152533-25659.log`：atomic_max183750、grant_max177420、cleanup_max3030、cleanup_total2210、zero_grants0，virt10MHz。16页后最长Bootstrap观测仍约17.7ms，不能宣称分片已解决全部最大成本。新增静态phase名字只为定位实际prepare/map/write阶段，采样在QUEUE外、无分配；50%节流最大值包含STOP/CONT，不能用于WCET。

### 最终收口概览

本节汇总交付基线，最终完成条件与后续触发由篇末拥有。已经接通的机制不再列为待施工，已过且执行代码未变化的门不重复运行；旧轮次中的“正在运行/下一步”只说明当时过程。

1. **当前组合证据。** 用户审定的两级有限服务机会、绝对线程/类别期限、timer装填费用与连续M的期限复检已落最终源码。`service-opportunity-check-002` exit0，`service-opportunity-host-001`显式aarch64维护host29项通过；`service-opportunity-acceptance-001` exit0（七面clippy、debug stress16/16、release core、sifive_u core、nofd与Ready前panic/alloc/fatal），`service-opportunity-hetero-001` exit0，成功客体日志保留。旧atomic-drr/bootstrap-copy日志仍是其各版证据，不代替本轮；最末“本轮接线、首审与验证”拥有具体路径、原finding及复核。
2. **累计析构栈。** 正常/可恢复M门已关闭。固定debug ELF的6672节点、41个间接jalr site及35个函数内jr表已闭合；计入金融父链、AVL/stdlib深度、实际空字段glue、回执宿主层次及正式入口后，boot为120000B、secondary为119216B，均小于既有262144B正式栈。篇末最终累计栈轮次保存源码条件、首审finding与复核；133584B及中间候选不再作为当前接力。该结论不覆盖terminal诊断，也不承诺平台WCET。
3. **有限服务机会与费用边界。** 本次结构性无饥饿与真实采样费用方案已接、首审finding已修、正式组合通过。Ledger/Rotor/Resume的K=q、零费不虚扣、封轮丢正保债/等在途，绝对线程期限、U timer准备费用、连续M期限复检均闭合；机会强制轮不声明原CPU份额偏差界。可靠执行计量独立，不阻本链；不再反复研究计数器或追加执行后台。完整M原子/累计栈、资金矩阵及最终结构检查均已组合收口。
4. **完整M原子结构账。** 下表的15个实际Work、固定Talc/资源几何、锁阶及最后root释放已与最终累计CFI闭包组合审定；Placeholder不列生产消费者。单次推进界以STEP_UNITS、SV39层数、实际paid capacity、AVL高度、H≤8、父深D≤32与堆gap数G/32表达，容器空前缀和完成回执不存在隐藏全表业务析构或嵌套Work.step。M机会覆盖pick/step/finish/最终root/父归账/settlement全程；量子及采样最大值均不冒充该结构界或WCET。不追加预算阈值、执行器或平台计量前置。
5. **资金失败与最终结构检查。** Buffer准入/allocator拒绝10项、MemorySpace付款前缀拒绝47项及各自clippy已过；四条实际链加其余七类birth矩阵已审定付款、失败、唯一owner与门外收束，真实消费者组合和最终REVIEW完成。精确kernel Arc OOM与部分内部交错保留源码证据边界；不补全分配点注入、旧FAL或新自检。FAL历史风险仍由原独立记录拥有；本次执行代码未变，仅清理四处WaitMany/WaitContext旧注释，未提交。

正常Account父链栈取证（不是全栈WCET或异常路径证明）：

| 项目 | 固定事实与边界 |
|---|---|
| 产物 | `artifacts/account-chain-stack-snapshot.elf`，2026-10-06 14:11 debug构建；STACK_SIZE=0x40000，STACK_GUARD=0x3000。后续重编译地址变化不得交叉引用；帧值只归此快照。 |
| 真递归 | Account::drop末尾调用mem::drop(parent)，返回后有epilogue，非尾跳；每层Account帧288及Arc/drop glue共176，累计464 bytes。Account链与Pool派生一一对应，ABI MAX_DEPTH=32。 |
| 临时Pool跨度 | 本层释放MetadataCharge的正常峰值2720 bytes，在递归parent之前完全弹栈。父Account本地强根、祖父Pool的charge强根防止Pool字段析构嵌套下一Account/Pool祖先，故不乘32层。 |
| 链本体累计 | 外层调用前缀之外，`(D-1)*464+288+2720`；D≤32时≤17392 bytes（约17KiB）。这是链本体上界，不能用它省略M入口、vtable领域glue或高阶调用前缀。 |
| 已知M前缀 | service208、advance_one624；idle摘环从Option tuple glue进入Account，WorkRetire经Arc<dyn Work>的80/80/48-byte glue及具体类型vtable。具体宿主字段按Rust顺序串行析构，多个payer链不同时叠栈。 |
| Job/Process边 | Job.parent和Shell.job都是Weak，不沿32层Job父关系递归析构。Job完成由逐Work发布/摘表推进；Shell/Departure的付款根随各自footer释放。Job深度仍影响领域向上循环的工作量，不能因为非递归而从时间账删除。 |
| 尚未关闭 | 以上vtable入口可到多个Work宿主，需把实际业务前缀及其内部最大动态分配/锁/退款链统一核对；sub-67对此续取证900s超时，未生成可用清单，不能认定该缺口关闭，也不把其正在反汇编的片段当证明。不重做全仓callgraph，后续由主代理先固定Bootstrap Map准备、Mailbox release、来源Thread hint这三个正常入口逐条合成，再补其余动态目标。不声称整个256KiB栈已凭这张表证明。 |

#### 实际M单步工作量核对

主代理亲读当前Work及其直接高阶调用；“一个节点”不等于一条指令，下表把嵌套成本与根释放单独列出。15个生产实现由源码`impl Work for`穷举，Placeholder只在空侵入next的胖指针初始化中存在，不会入队。记录不增加重复预算常量。

| 实际Work | 单步与高阶成本 | 进展/收束边界 |
|---|---|---|
| ProcessBuilder | 启动/回滚每步读取一个Members槽；启动最多转移一个Thread到Ready入口，其他阶段为单次VM gate/来源发布。 | Created扫描有tid_ceiling快照，队列完成后Builder last-live先退费/回执再发DONE；Bootstrap不再持Builder强根。 |
| ProcessShell | Cap排空每步最多16槽但只release一个cap；Members、Ready、publishing每步一个槽。Body最后释放前CapTable occupied=0、member_count=0且VM space已取走。 | 槽代数及退出屏障由CapsClosed→Unbound→WaitMembers/Ingress→TakeVm→WaitVm→DropBody单调推进；残留在途owner不被跳过。 |
| ThreadDeparture | 一次Members.depart摘成员槽，再退BodyLease；footer退本体费、发布Thread DONE、member_departed。 | 没有遍历全Members；completed防重复，Shell负责最后成员屏障。 |
| Job | 父AVL表删除本Job一个节点，父门外退款；发布自身CLOSED、至多激活一个完成父Job。 | Job父Weak，不递归step祖先；树高由direct entries≤65536界定，祖先责任另排Work。 |
| MailboxCore | close cleanup与Source扫描交替；每cleanup取一条Message，锁外最多8附带cap+1 Delivery authority，payload≤4096，queue容量16。 | release不能递归执行新Work；lastcap可发布新维护根，每份退款/Pool链仍算当前完整原子。 |
| WaitSet | close cleanup与Source扫描交替；cleanup删除一个AVL Member，锁外detach一个WaitNode与凭证退款。 | closed阻止新Member，AVL容量来自u32 ABI limit及实际付款。没有同一步清全树；其他hint仍按单候选计。 |
| SignalSource、NotificationCore、Lifetime、MemoryObjectCore、Endpoint（五个） | 持所属域锁调用一次Source.step，只检查一个候选或一次轮末状态，再锁外send一份Delivery。 | 稳定来源可结束扫描；持续pending/新增节点可重复Ready，故这里不能提供跨轮有限Ready次数。三个实际HintTarget为WaitSet、Thread、Bootstrap：候选入队/单成员hint转移/单wake，不调用下一Work.step。 |
| Backing | SegmentMap.release_step一次，至多摘一segment并锁外drop FundedExtent；root完成后取已空存储。 | segment索引/双数组容量实际付款；释放游标减少持有片段。归还extent是一次库存/额度操作，非逐页软件copy。 |
| ChangeWork | 发布每步ledger≤16节点及PTE≤16槽；取消/retire≤16业务单元；table refund≤16帧（实际batch容量4）。Fence是提交并Blocked，等待远端ack后再释放。 | SV39 LEVELS=3；每个单元的AVL/View/Segment与付款退款成本仍另计，不能把16当作完整成本。锁外10个scratch数组bulkfree不遍历业务元素，affine许可已由取消/退休入口收束。 |
| RetireWork | 正常退出每步冻结/发布状态或清至多16域单元；table批次固定4、整VM末尾空数组直接free。 | body gate冻结新VM使用，等entrants/changes与TLB ack；执行等待不自旋推进整个VM。每次release_fragment_step包含一次BackingRelease.step，未增每use子Work。 |
| Bootstrap | 每Map/Write只取同一VM STEP_UNITS×PAGE=64KiB分片，并保留uaccess字节校验；Thread阶段准备一个首线程及对应Ready容量。 | 初始化Building内的Map准备仍含MemorySpace布局、View/plan容量、page/metadata准入与heap分配；限字节不能省略这些高阶成本。错误先freeze/drop Loader，致命停机不纳入正常服务进展。 |

共同正常高阶边界：

- AVL树高由容量与平衡不变量界定；OrdMap/SegmentMap的slot与free-index数组由实际paid capacity界定，grow竞争仅在观察到更大容量时继续，次数受字宽限制。维护域内仅Bootstrap需要真实prepare/grow；其他业务准备在提交之前完成，不能把用户提交scratch再免费搬进Work。
- Talc 5.0.4 free固定最多两邻居unlink及一个gap link，无bin/arena扫描；allocation是独立的heap gap搜索，实际system heap G≤16MiB、minimum chunk32，候选上界G/32。最后退款包括fee-slot、Pool与Account父链D≤32；一条Message可有9个authority根，依序释放，不同时压9条父链栈。不能把free固定操作数拿来证明allocate或全对象析构。
- IRQ保存的FIFO锁至多排在其余H−1个hart的已发行票之后，H≤8；临界段内部工作量与单向锁阶仍是等待界的前提。Work、摘环旧强根、业务废弃容量及footer释放均在QUEUE外；QUEUE只做侵入队列/环、Record/ledger有限状态操作。
- 完整原子还包含intrusive publish/finish、Blocked root释放、dyn Work/glue/field析构、Account直接父归账和最终queue settlement；M Rotor raw区间覆盖全程。账户净区间扣除已单独归父的cleanup，禁止双记。这些成本以真实输入容量/固定深度表达；数值政策q不是其上界。

本表与篇末最终累计栈轮次共同关闭本次正常/可恢复M原子结构门。有限服务机会独立处理持续0计量Ready的轮转：每轮费用或机会先耗尽者拒绝下一领取，已领取的一个有界原子可以完成；期间对候选U的绝对期限按安全点复检。这里不从粗TIME推导物理CPU份额或毫秒WCET，不重开已独立登记的可靠执行计量前置。

#### 执行计量前置：保留目标，不以调度补丁替代缺失平台合同

sub-66咨询及研究取证已收集；以下保留可靠执行计量能力的独立前提、owner与接通条件。用户已审定篇末有限服务机会方案，物理CPU占用份额能力不阻本链的结构活性交付；不另建平行todo或尚无合格后端的meter facade。现有双环/Record/父尾账继续承担采样守恒，费用进展不足时由机会门保证换出。

三项需分别成立的目标：

1. **守恒。** 账户预扣、原轮退款、越额欠账和父归账满足`D+R=D₀+Kq−C`，C为同一计量源/单位的归属费用；不得忽略在途、双记cleanup或向其他账户转嫁间隔。
2. **执行进展。** 一个持续获得本身执行且持续Ready的账户，有限推进后归属C必须正增长；如要求结构性访问次数界，平台与执行契约须给出N、b>0，使至多N个该账户的合法短段费用之和≥b。参数来自计数器可见更新/执行语义，不是人为保底费用。
3. **代表性。** 归属单位须表示所承诺的执行占用（包括等锁/访存停顿），声明允许误差、平台暂停/虚拟化与SMT边界。仅有C递增的INSTRET不能代表该成本；仅有恒频TIME也不能消除短段量化丢失。

固定取证与实际版本：

- ISA v20250508 [`counters.adoc:119`](../../references/normative/riscv-isa-v20250508/src/counters.adoc)定义TIME墙钟；142–165允许可见更新低于名义tick，平台可另给最小更新频率。virt 10MHz/sifive_u 1MHz DTS只提供TIME频率，没有逐短段进展界。
- 同篇61–117的CYCLE依实现/执行环境，属于core，未保证跨hart共同恒频；167起的INSTRET属于退休指令数。privileged `machine.adoc`的mcounteren与mcountinhibit分别控制访问与递增，能读CSR不证明执行时未冻结。
- 实际安装QEMU为11.1.1，客体日志OpenSBI为1.8.1；索引中11.1.0/1.9不是本次运行版本。官方固定[QEMU v11.1.1 csr.c](https://github.com/qemu/qemu/blob/v11.1.1/target/riscv/tcg/csr.c#L1278)默认无过滤、无icount时CYCLE/INSTRET取cpu_get_host_ticks；当前Justfile未开icount。官方固定[OpenSBI v1.8.1 sbi_hart.c](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/sbi/sbi_hart.c#L32)配置默认访问/运行，但不提供频率、可见更新或短段进展保证。因此简单把now_ticks改rdcycle不能关闭前提，当前QEMU默认路线上TIME区间还包含宿主暂停，不能称为精确物理CPU运行时间。

目标接线与拒绝的替代：

- 优先使用平台已确认的执行计数器原生单位。每段起止在同一hart相减，不要求各hart绝对起点同步；所有hart差值必须具有共同固定频率，或平台必须提供可靠分段换算/DVFS契约。Ledger直接累计原生单位，量子在唯一初始化入口换算；不能逐短段先取整为TIME tick再重复丢失费用。
- Work与Account cleanup同步使用同一源，延续父费用去重和raw U/M scope；TIME仍拥有期限/timer。初始化确认权限、计数运行、频率、回绕与可见进展；短时自校准/启动读数样本只能验证配置，不证明长期合同。无需每账户小数余量、跨安全点强根或新清理Work。
- 不保留跨账户上次时间戳直接相减：会把U/其他账户/紧急服务间隔错扣。拆掉间隔再累加零差值也无法恢复信息。只有拥有平台更新上界且连续有用工作区间具有结构界时，批量合并才是可实施优化；不以“直到计数变化”为退出条件。
- 不暗加每轮访问门：允许正信用账户先移next、其他账户继续补q可以保证访问活性，但已改变严格累计CPU份额关系；保留原轮补信用限制则零计量反例仍在。若未来明确选择这类较弱合同，必须作为改变已确认公平前提交用户裁决，不能称为当前目标闭合。

能力owner为内核平台执行计量/维护入口；接通触发为选定平台能提供上述三项独立证据。完成标准是固定平台/固件/模拟器版本的合同、权限/单位初始化及跨hart/嵌套计量消费者同步，随后才运行相应算法/零计量/平台组合验证。计费改用原生非TIME单位时须同步类别预算到timer期限的换算，不能只替换读CSR函数。当前默认QEMU提供责任链/状态运行证据，此独立能力未接通、不阻本链；不得因此恢复旧DRR或添加伪费用。

本轮局部结构复核：Bootstrap写入复用唯一CHUNK并保留uaccess校验，未改变ABI/授权/owner；新增Buffer allocation failure与扩展现有MemorySpace付款前缀测试只观察独立余额/数据契约，无测试运行体/全局allocator替换。target/七面lint/core、两个受影响host及各自all-targets lint已过；rustfmt --check和git diff --check通过，受控QEMU/just子进程均结束。该检查覆盖本轮变化，不代替整个对象链的最终REVIEW与尚未闭合完成门。未提交、未归档。

栈前缀接力补充：sub-67超时后主代理自行按具体函数提取，不另派全仓调用图。旧快照原始启动调用边保存在`artifacts/bootstrap-prefix-stack-001.txt`，随后固定**当前最后写入分片源码**的`artifacts/maintenance-current-stack-snapshot.elf`，SHA256=`3ee903dd0899d8e123d26a976f5195bc4f708ba983e5144a4d9feabda2cd776d`。`maintenance-function-probe-001.py`只提取固定ELF函数入口及直接/间接调用边，当前28个函数原始输出在`maintenance-current-function-frames-001.txt`；不构建生产机制，不判定整栈安全。

| 当前快照入口/分支 | 已核实帧（bytes） |
|---|---|
| service→advance_one | 208→624 |
| Bootstrap.step→advance→initial_map | 1648→3888→5200；连同上述M入口共11568，尚未加此下更深callee。 |
| initial_map调用Backing.fund | fund为`496+(0x2000+1920)=10608`；此分支前缀共22176，不与finish分支叠加。 |
| initial_map调用finish | finish为`496+(0x2000−1760)=6928`；此前缀共18496。finish之后prepare/View/存储/域glue仍待按实际分支合成。 |
| ChangeWork.step | `496+(0x3000−1184)=11600`，不能只识别直接addi而遗漏寄存器sub sp。 |
| 其他Work.step | Retire960、Builder560、Shell160、Departure160、Job48、Mailbox1632、WaitSet592、Backing1152；五种Source Work均144。Placeholder16只作占位。 |
| Account/Pool Drop | 当前入口仍分别288/1824；祖先链的层间glue与先弹临时Pool再递归结论归前述固定证据，不能把单帧直接当链体。 |

下一项栈工作只沿这些明确地址续核Boot正常Map prepare/fund/finish及其最后根，再核Mailbox release、Thread hint和各Work最终glue；当前表是帧与实际边事实，尚不是完整stack-bound证明。失败/panic格式化分支未混进正常路径最大值，亦未据此宣布其栈已审。执行计量前置仍阻塞公平交付，剩余资金/栈门在同一记录继续。

### 设计收口轮次：有限服务机会与费用预算（用户已审定，施工中）

用户要求征询advisor或CalmStoat后，已收集sub-68交付边界咨询及CalmStoat只读意见，主代理亲读维护原定稿、Ledger、pick/settle与Rotor。两者一致认为：不应把无限精度物理CPU时间、所有平台WCET证明升级成本链硬门；当前零采样饥饿必须在本链解决，不能仅记平台债务。CPU费用份额并非全部凭空追加——账户DRR及含误差E的类别份额原已定稿——因此改变保证提交用户审定。用户现已明确选择推荐方案并授权继续实施；本节取代前述将计量平台前置列成本链硬交付门的选择，取证继续有效，可靠执行计量保留为独立能力。

**已确认：本次承诺结构性无饥饿及真实采样费用约束，精确CPU份额由独立平台合同提供。** 统一“费用额度 + 有限服务机会”于现有Account/两环和per-hart Rotor，不新增执行者、队列、兼容模式或空meter facade。可靠执行计量不再阻该能力交付，但已知全零费用反例必须由本次仲裁修复。

有限机会依据与正常费用行为：

- 某级每轮费用量子为q，最小正表示单位为u，最多机会数`K=ceil(q/u)`；当前整数tick账本u=1，即K=q。K来自已有量子/费用表示，不是另设8/16之类无理由阈值，也不把每次机会虚扣为一tick。
- 每个计数机会若至少产生一正单位费用，q预算最迟在K次结束时已耗尽；机会门不会提前改变该情形的费用轮转。A=1/B=100/q=100原反例仍按费用服务，不能采用每账户固定一原子或任意小K而称保持同样CPU份额。该条件指每个计数机会，包括尝试后未执行的零费仲裁，不只是已出现的正费用样本。
- 计数机会和费用归属须一一对应：每个机会都有其实际采样费用（可为0），空pick/gate/债仲裁不得计次后漏掉该归属段。U因紧急服务暂停/恢复计时可按原合法短段合计同一次继续机会的费用，但不为计时API调用再发机会或重复计次。无活动段的重复end_user、未选定类别的共享仲裁既不收费也不计该类机会。选定类后的实际选择/准备/收束段须由同一类scope涵盖，包括失败的选择尝试；计数与费用的边界必须在接线中同步明确。已有授予允许完成本次有界段，机会归零只拒绝下一次授予，不在当前准备中途作废已经授予的执行。
- 费用未耗尽而K耗尽时强制让出：丢弃正的未用执行信用、保留负债。丢弃的是本轮机会权，不是Pool金融费用；真实C仍原样结算，0仍是0。连续强制轮没有原CPU份额偏差界，不用“计量可用”笼统掩盖这一限制。原采样信用式须显式计入丢弃量F：`D+R=D₀+Nq−C−F`；Pool/metadata守恒不变。

两级具体闭包：

| 位置 | 机会与结束条件 | 所有权/并发要求 |
|---|---|---|
| Account | 每次实际领取Work grant消费一次机会；无信用或无机会均不发下一grant。信用耗尽或K用尽，且全部in-flight结清，才能封轮移next；真正无Ready/在途仍按idle收束。 | 保留预扣、原轮退款、父归账与强根门外释放；机会耗尽不得越过在途gate提前补下一q。新激活仍进next，当前环有限快照不被新准入扩长。同账户并行额度保留现有cap=ceil(q/H)，不改为固定一原子。 |
| M类别 | 每次已选中的维护推进/仲裁尝试消费机会，包括pick为空、欠账/在途仲裁。 | 不能仅数成功Work.step，否则零费空仲裁仍可阻U；退場完整raw区间照常记M费用。 |
| U类别 | 每次调度执行尝试及可调度返回/Resume继续机会共用同一额度，须覆盖pick失败和零费短调用。 | 同一继续机会只能消费一次，不能在准备、暂停计时、恢复计时、最终析构重复扣；Resume不补q/K。既有timer截断纯用户执行，紧急服务沿原边界；费用采样分段不决定是否重发机会。 |
| U/M切类 | 费用或机会任一耗尽，竞争时转另一类；补其q并重置该级机会。无另一类时仍借用空闲能力。 | 保留负债、丢正不积攒；机会不是新费用或另一套调度器。无工作的类/未成功执行的尝试不能令额度永久保持正而阻另一类。 |

条件与诚实能力边界：hart持续推进、单原子/在途grant最终返回、timer/remote紧急服务满足已有准入约束；有限既有费用债不会造成永久跳过，持续外来父债注入不承诺该账户自有Work最低份额。结构活性不依赖TIME短段跨tick；数值延迟、真实CPU份额与计量误差仍需要平台证据。这不是恢复旧每Work RR：费用仍在两级仲裁中控制可领取与越额偿债，机会门提供不依赖计量的终止条件。

本轮有限完成门：

1. 同步接Account与U/M/Resume，不保留两套策略；host验证恒零、混合零/正费用、1:100正费用原反例、在途迟到退款、持续新准入、强制轮丢正/负债保留、U/M零费用和未成功选择/无用户线程。预期来自上述机会/费用合同，不复制实现为oracle。
2. 沿已固定15个实际Work合成正常/可恢复失败的原子、锁链和累计栈界，致命路径有明确边界；不扩为全仓形式化证明或数值WCET。
3. 将剩余资金失败/取消/输出失败/迟到通知收为有限责任矩阵，逐项给源码论证或既有设施证据，完成必要组合及最终REVIEW。已有FAL范围不扩大，独立执行计量能力只保留owner/触发/完成条件，不在本链建设新后端。

备选保留原CPU份额交付门、等待合格执行计量平台未被选择。K派生、并发布局和接线细节由主代理在已审定范围内决定。新方案施工中，不以设计审定替代实际接线/验证。

实施接线补充：Ledger每轮重置q次机会，每份Grant消耗一次，finish_round丢正保债且仅在in-flight=0结束；Rotor选Run及同轮Resume各领取一次机会，计费函数只记真实费用，不把暂停/恢复采样再计机会。U scope移至选定类别后、domain.pick之前，pick失败只结束scope并返回循环，不在活动U计时内idle。当前U的准备/用户/syscall/退出费用仍沿同一机会归属，多个合法采样段可合计；无活动段end不产生费用或机会。Resume在费用/机会任一耗尽后Requeue，否则消费同轮下一机会，并重装timer。重装保持本次dispatch的绝对线程期限，不以Resume重新发线程量子；per-hart期限只在sched拥有，进入用户现场前建立、Switch后清除，class剩余额度与已有最早timer仍取min。共享选类/紧急服务排除边界不变。

#### 本轮接线、首审与验证

- 已接Ledger机会及Rotor选Run/Resume机会，不增队列/执行者；次数消耗与真实费用独立，所有正费用原反例仍按费用提前耗尽。机会归零的已授予段仍可完成，下次授予才拒绝；Account须等R=0才能移next。off-ring新激活进入next，当前有限paid账户快照不能被新准入扩长；持续Ready只延长自身Ready链，不能重置本轮机会。机会强制封轮丢未用正信用，负债和迟到退款保持原账。
- `service-opportunity-host-001`显式aarch64维护host29项全部通过；7个新契约用例覆盖两个恒零账户每轮可终结、不虚扣费、强制轮迟到退款/越额债、混合0/正费用、恒零U/M、Resume多段采样不重复机会、落败选择不阻另一类、无另一类时借用且不积攒正信用；原1:100与实际tick权重测试保留且通过。新激活不扩大当前环是实际ring_insert/pick/settle源码不变量，未造平行内核环模拟器。
- 初轮target `service-opportunity-check-001`、七面`service-opportunity-clippy-001`、全速core `service-opportunity-fullspeed-001` exit0，完整客体`qemu-acceptance-20261006-171758-80397.log`。这些是下述两项修正之前的开发证据，不冒充最终源码验收。
- sub-69只读首次审查发现P2：首次arm与Resume rearm处于end_user/begin_user之间，漏计timer锁等待/时钟/SBI准备。主代理沿两入口复核并修正：先打开U段再arm，`user_deadline`直接取活动段起点+Rotor原余额，避免从装填时刻重新延长额度；Resume只打开一次U段，Switch单独打开离场尾段，机会不随采样增加。线程绝对期限仍只在dispatch建立、Switch清除。原finding保留，不重复首审或以正常测试替代费用归属论证。
- 主代理发现同一活性闭包的期限窗口：未来Timer入队仅arm硬件，内核SIE=0且连续M时STIP不进入trap；原loop只看软件pending，期限无法变成Ready。现每个调度安全点复用earliest_future单堆顶复检到期电平，随后既有16项service_expired；不要求先idle/进入U，不增加timer actor、循环或限额。Timer锁只peek，Clock/后续hint在对应锁外，紧急服务仍不归U/M。源码证明关闭发现路径，尚未新增独立强制连续M的运行体；正式core/stress的真实等待消费者提供组合证据。
- 最后源码`service-opportunity-check-002` exit0；`service-opportunity-acceptance-001`聚合exit0（七面lint、stress16/16、release、sifive_u、nofd、Ready前panic/alloc/fatal），完整客体日志保留。`service-opportunity-hetero-001` exit0，完整客体`qemu-acceptance-20261006-173445-96027.log`，零grant样本0但不以此替代host恒零契约。host逻辑源码自29项通过后未改，不重复运行；Kernel timer/U费用修正由target与正式消费者验证。

本轮交付前局部结构复核：Scope在U类选择后、线程局部owner之前建立，失败continue和所有Switch先释放局部owner再结U尾费；idle仅在未持U scope的仲裁Idle路径。首次arm与Resume已打开U段，活动段起点+余额的类别期限只读取原账，不改变信用/次数；原线程绝对期限在Switch清除，重复Resume不重发。Ledger全部在途结清前不封轮；Rotor计费与授予分开，采样分段不消费机会、强制轮不积攒正信用，负债/i128界保持。M空仲裁的全程raw费用与机会都覆盖，期限poll/到期hint仍是锁外有界紧急服务。对照sub-69原P2沿两条实际入口复核完成，未再次启动reviewer。没有新unsafe、容器/allocator、测试运行体、兼容出口或用户ABI；源文件格式/差异检查和受控进程清理完成。该切面已收口，整体对象链仍待完整M原子/累计栈、有限资金失败责任矩阵与最终REVIEW，不提交、不归档。

### 完整M原子/累计栈收口：固定CFI、首审与有限补边

主代理固定最终源码的`artifacts/maintenance-closure-stack-snapshot.elf`，真实SHA256=`294a36aa292db1d99e0bdee2060a01f5f07499ecd76ffb9780cc1e092f466082`（以文件重算，纠正sub-71输入中的误抄尾部）；STACK_SIZE=0x40000、guard=0x3000。rustc为1.100.0-nightly，commit c54751567b19c4ceb08b0412d83529c2568cba8b；当前CFI`maintenance-closure-cfi-001.txt`约7.45MB，全部CFA为sp，无动态frame-pointer解释缺口。

不同于只数函数入口addi，CFI覆盖寄存器式sub sp及全函数最大CFA偏移。`maintenance-stack-cache-001.py`/`...graph-001.json`最初漏掉W/w弱函数；主代理检查未知直接目标发现memcpy/memmove/memset/__udivti3四类，已在002脚本/缓存包含弱函数（13869函数、13863 CFI），不得只把“已缓存调用均有CFI”当覆盖全部直接调用。无CFI的`__rust_no_alloc_shim_is_unstable_v2`已反汇编核实仅ret，可记0帧。源码/固定LLVM产物未经修改，不为审计新造运行体。

sub-71为本次累计栈候选的独立首次审查，结果：

| 项目 | 已审定/尚未审定的事实 |
|---|---|
| 金融链 | Account先取父账户强根再退款metadata、结账后才递归drop(parent)，阻止临时Pool释放再次进入父Account。Pool字段inner→maintenance→metadata，custom Drop的临时退款帧先返回再进父字段。Account跨层驻栈464B，Pool字段跨层224B，D≤32；外部临时峰2496≤留出的2720。可使用金融包络31904B，依据两种阶段和深度，而非只说“21函数求和”。 |
| AVL | 系统heap≤16MiB，当前kernel节点最少32B，N≤524288。高度27最低514228节点，高度28最低832039，所以h≤27，含空基例≤28次递归。remove1776B、take_min512B分别×28是保守组合。 |
| 标准库排序 | 固定本地rust-src确认quicksort初值limit=2*ilog2(N)，每次递归前减1、右侧循环，0时heapsort；pivot每层/8，64以下停止。用非ZST元素N≤2^24的宽资源界，49/9层成立。帧及直接调用仍按固定CFI。 |
| 正式入口 | hart_formal_entry288、boot bring_up_runtime784、sched.run1136；assembly先重设formal sp，boot外前缀2208B，secondary1424B。不把用户保存寄存器区再加为同时驻栈的Rust调用链。 |
| 候选P1：业务SCC | 001脚本将含Slot/Job/Node的378函数业务SCC整体当单次，依据不足。空Members/Views/Job及摘出Node的断边各自正确，但不能覆盖Transport.wake、自强持Record或其他Work析构循环。必须剪具体不可能字段边，而非按出现某类型名消整个SCC。 |
| 候选P2：动态调用 | 应逐site/method/receiver来源補边。Authority仅holders、cap.deliver仅on_last_capability、Delivery仅hint；SchedDomain辅助函数不能被子串误认成所有SchedClass方法。WakeToken目标只Bootstrap/ChangeWork/RetireWork/Builder/Shell五类，不能扩成15个Work全体。jr本地匹配跳转表不是callee，须与尾调用区分。 |
| 候选P1：格式化 | 001未决fmt分量权重0、七个动态点无后继，因此134608B候选不是完整上界。正常维护日志sink是Console，参数为固定Arguments/基础值，按实际formatter/sink闭包；索引失败terminal不能混作正常递归或默认为0。 |

主代理002候选进一步收窄调度/Authority方法，剔除闭包误入trait方法、补正常fmt参数/Write sink、切RecordInner与Transport最终空字段边，保留Null Option胶水保守跨度。金融macro使Account/Pool资金父链独立，不把所有业务生命周期乘32。002当前数值133584B（不含正式外前缀）**仍非可交付上界**：业务SCC仍352函数、15728B；动态检查仍按函数而非site完成，部分fmt合法嵌套规则为候选。工具的“unresolved=0”不能关闭这些语义未审定项。001/002脚本与候选JSON仅是取证工具/输入，不进入内核。

当前栈断点精确限定：

1. 逐字段证明并剪RecordInner.next/root、Account.head/ring_next、Transport.wake及空容器元素的不可能最终Drop边；WorkRecord运行/排队自强根、Transport self WakeToken及最后ack take在maintenance.rs/remote.rs已有真实来源。其他Vm/Shell waiter、Bootstrap自身token按实际owner继续核对，不给整个业务SCC单次豁免。
2. 用具体间接site和receiver种类补Work/Hint/Observation/SchedClass/KernelObject方法；WakeToken析构目标限制五类，不能用“该函数已添加一些边”替代覆盖所有site。直接weak内存/算术leaf已补，继续确认直接边无遗漏。
3. 正常日志Arguments嵌套、Formatter sink及参数闭包完整后，重新算CFI累计界+入口前缀并逐条复核；不可恢复fatal路径另有边界，不追全平台数值WCET。上述三个缺口消除后才关闭累计栈门，不再重复金融/AVL/排序/入口取证或新建全仓调用图。

### 有限资金失败责任矩阵：四条实际链

sub-70提供事实定位，主代理亲读create/thread、handle、mailbox receive、wait安装/detach、成员epoch以及Operation/drop/finish直接路径，校正VM cookie分支与实际task文件路径。以下是当前源码论证与既有行为证据的边界，精确kernel Arc OOM/安装交错未被注入，不把正常QEMU组合写成资金失败测试。

| 实际链/失败窗 | 状态与唯一收束owner | 退款与证据 |
|---|---|---|
| Process/Builder出生 | task/process.rs construct先Job Reserved，再Shell commit；Shell出生失败rollback Job，Body/space/handles/OwnedBox/install_body失败freeze_and_seal，construction guard结束交接。create的Builder/cap/输出失败同样由Shell自动回收。 | 尚未安装的Body为空，本地Drop有界；已进入Job的Shell由reclaim稳定持有。MetadataCharge/Buffer/PreparedEntry保留实际payer。源码+真实创建退出组合；非精确Arc OOM注入。 |
| Thread出生/输出 | task/thread.rs create_in在MemberReservation之后预备ReadyBatch/Control/Thread/Departure/ticket/caps；发布前错误由reservation Drop rollback Reserved槽。handle.install write成功才commit Created并arm ticket。 | ThreadRelease.requested=false、Departure.completed=false时只退宿主费，不depart未登记成员、不发提前DONE；已commit Created后的cap撤销按ticket/Control真实退出路径。主代理核thread/departure/shell guard，源码证据。 |
| cap安装/claim撤销 | handle.rs install：body不存在/reserve失败先release caps，write失败rollback预约后release，publish后commit遭shut则返Released events，不能推翻已提交责任。Invocation Drop通过ClaimTicket restore；shut返最后释放事件。 | capability action delivery在表锁外；纯预约存储的financial退款允许HANDLE_TABLE100→MEMORY_POOL230→MAINTENANCE660/HEAP900单向锁序。部分付款/基础allocator失败由既有纯库测试覆盖，kernel安装窗是源码证据。 |
| Mailbox Send失败/成功界 | 输入/Delivery birth失败由本调用临时Buffer/claims回退；逐move失败、closed/full/deadline入箱失败恢复每份transfer并释放Delivery。enqueue成功后commit_transfer，Once由锁内Pending→Sent线性化。 | Message拥有payload、最多8附带cap及Delivery；未送出的授权回caller，已Sent责任不因caller消失退回。已有capability/资金host及真实IPC组合；具体窗口为源码论证。 |
| Mailbox Receive输出 | begin_receive先持唯一receiving token；cap预约/用户输出失败撤槽并rollback message，未shut push_front，已shut锁外release。复制成功后commit_receive必成，cap表shut只撤公开资格，释放events。 | 原消息/费用完整由receiving或queue持有，无截断退款；release_all/message.release不持Mailbox/Cap表业务锁。源码与实际消费者组合，未强制输出fault窗口。 |
| WaitSet Add/Remove | Member OwnedBox+PreparedEntry先付WaitSet payer，稳定pin后attach；closed/limit失败门外drop(prepared)唯一detach，remove同一路径；Add输出失败remove(token)。 | Node/source强根存活到detach，目标仅Weak；Member/树节点退款在集合锁外。基础funded_storage/ordered_table host为存储证据，具体attach窗口为源码论证。 |
| Thread Wait安装/退出/迟到 | wait.rs begin_wait仍是Running执行owner，提示只设pending；Entry存source后attach并登记epoch timer，然后复检/park。epoch溢出无分配，requeue或reap。正常/冻结共用timer cancel_if(epoch)→source detach→source loan drop→end_wait。 | Thread出生已付内嵌节点，无新增安装分配；其他hart不能取走仍Running的安装owner。旧epoch hint被members.hint拒绝，detach完成前node不释放。源码+真实等待/退出组合，非安装窗口注入。 |
| VM prepare失败/取消 | Operation持所有已付scratch、plan、View/use、permit、frames；任意准备失败Drop转ChangeWork.Rollback并publish，责任不依赖caller继续调用。View.prepare失败归还usage到prep；阶段cancel/fence/refund终结后释放私有费用/回执。 | memory_space付款前缀47项及Buffer allocation failure10项提供基础行为证据；真实map/unmap/退出由stress/platform组合，部分内部cancel是源码论证。 |
| VM输出/cookie | finish：payload deliver失败或cookie publish失败的`?`都退出至Operation::drop回滚；后者defer_output_fault，trap锁外freeze caller。**仅cookie成功后**boundary/View/ledger commit与submit必成。 | 保留准备退款owner，不把失败cookie误写成已commit；cookie成功后由ChangeWork强根承担退休/资金屏障。uaccess保护路径/源码及正常组合，精确fault窗未注入。 |

上述矩阵将原“继续查内部窗口”收敛为有限真实分支，禁止为填证据补旧FAL或自检服务。当前已确认owner/退费路径，但整体资金门仍需对照已有跨组件完成条件，明确范围外对象出生链的既有依据和剩余真实组合，不因这张表单独宣布全系统完成。对应验证可采用源码审定，若必要行为风险不能由现有设施覆盖则精确登记，不要求全分配点故障注入。

## 最终累计栈与M原子结构收口

固定产物仍为`artifacts/maintenance-closure-stack-snapshot.elf`，SHA256 `294a36aa292db1d99e0bdee2060a01f5f07499ecd76ffb9780cc1e092f466082`。最终输入为`maintenance-closure-graph-002.json`、`maintenance-dynamic-sites-004.json`，计算脚本与结果为`maintenance-stack-bound-final.py/.json`；没有重编译、增大栈/guard或改变生产机制。

- Source/回执的真实向下宿主关系为Change→Builder→Shell、Retire→Shell；普通SignalSource私有完成waiter仅Shell，VM的start waiter为Builder、quiescence waiter为Shell。`wake`解队列锁后另一hart可能把目标推进Done，合法ticket仍保留最后释放分支；不以“刚入队”剪析构。WorkReceipt/ConstructionLease、member_departed借用Arc、Bootstrap Delivery.target及三处publish的原调用者强根，只覆盖各自内部参数减引用，外层最后字段释放仍计算。
- Whole Change/Retire State只在未发布Idle或队列完成Done后析构；只克隆其空Phase分支，局部有载荷Phase与tables仍展开。Shell退休来源在wait_done后take，final/birth为None。Record/Transport自根、Account已take的parent、Thread定时payload、Member/Table的空字段各保留固定CFI glue；旧粗`+128`已删除。VmAccessLease唯一两项flatten正常循环耗尽，只剪耗尽iterator载荷，真实回调未删。
- 41个间接jalr逐指令地址/槽位/方法登记，Work、HintTarget、ObservationSource、SchedClass及KernelObject的方法不互串；动态析构与WakeToken五种真实arm provenance分上下文。35个jr均为目标落在所属函数内的枚举/ordering表，不作未登记尾调用。
- 正常Console格式化为外层Arguments/str、内层成功pid/payload及primitive；Ready门铃IPI可恢复失败的`u64::LowerHex`也在闭包。Bootstrap Err的Debug构造site `0xffffffc08030cd28`与terminal日志site `0xffffffc08030cd60`单独排除，同函数正常成功日志与Ready警告仍保留，先行freeze/退款仍保守展开。

sub-75对011首审给出两条finding：P1为遗漏Ready门铃失败的LowerHex真实formatter；P2为Bootstrap terminal日志在正常图中套用正常目标集。主代理按原具体site修复并重扫，没有重开reviewer确认旧结论。新增LowerHex使节点6671→6672，最大值不变；未决递归/间接site/CFI全部为空。最终脚本遇ELF身份不符、未决SCC、缺site、缺CFI或正式栈超限以非零退出且不签发bounds。

| 正式入口 | M闭包峰值 | 入口存活前缀 | 累计 | 既有正式栈 |
|---|---:|---:|---:|---:|
| boot | 117792B | 2208B | **120000B** | 262144B |
| secondary | 117792B | 1424B | **119216B** | 262144B |

保守峰值路径为service→advance_one→Change rollback→Views取消→AVL remove/take_min/rebalance→对象/backing退款→金融父链包络。AVL remove与take_min各自保守深度28同时计，金融仍采用已审`32×912+2720=31904B`；没有拿独立Account链17KiB省掉领域前缀，也没有以SCC函数帧求和冒充递归深度。该数字归上述固定ELF及本次正常/可恢复合同，terminal日志、异常trap和任意新二进制需各自证据。

完整M原子共同账同时包含一个队列/账户访问、Work有限业务单元、来源候选/HintTarget、至多一条消息的9 authority串行最后根、真实容量AVL/数组高阶调用、Bootstrap准备的资源量界、FIFO更高门与Account父链、finish后强根/费用结算。Source持续pending只增加下一次Ready，不在同一原子循环业务；回执允许同步最后析构但不调用下一Work.step。页表SV39层数3、STEP_UNITS=16、固定4帧batch、D≤32、H≤8、G≤16MiB与minimum chunk32均沿代码/硬件/ABI真值引用，不复制新的运行期常量。固定ticket源O(1)、Talc free常数邻居操作与allocate最多G/32 gap分别计界；锁等待仍以更高临界段有限和单向锁阶为前提。这关闭结构性推进与正常累计栈门，费用观测和政策量子不提供平台WCET。

## 其余出生链资金矩阵与组合审定

sub-76只读取到的七条链由主代理对照公开点与真实消费者补齐；以下沿用同一个handle.install/claim矩阵，不为每个对象复制安装状态机。SignalSource是宿主私有来源，不是独立可发行cap对象。

| 真实出生链 | 付款/准备与公开前失败 | 副作用与唯一收束owner | 现有组合依据 |
|---|---|---|---|
| Job | job.rs:256/275的child与SignalSource由所选Pool付款，父表PreparedEntry/Reserved由父Job payer；gate/深度/ID/metadata/Arc/mint/install任一失败。 | JobId/koid只单调消耗；未公开Reserved由rollback_child摘除，可能激活已关闭父Job。child出生值及节点各退实际payer；发布后父成员表/root anchor持本体直到Work完成，关最后JobControl不冒充整Job完成。 | test_hammer::create及真实RootSupervisor/父CLOSED组合；root()的boot expect属于供给基线。 |
| 派生Pool/Account | memory_pool.rs:246–298/363–387以parent做delegation双维预约及两份metadata，prepared子Pool先不可见；quota/depth、metadata、两Arc、mint/install均可拒绝。 | PreparedChildOwner:697–713撤销两账本，值/metadata正常Drop；无Source/Wait语义。公开commit后最后引用由Pool.Drop:460–529退parent页/metadata，Account祖先归账按有限父链。 | 既有memory_pool host守恒/边界与boot self_test，test_hammer真实child Pool消费者；不是精确kernel Arc OOM注入。 |
| Notification pair/observe | notification.rs:295–349的Core/Owner/Signaler及249–292的Observation都由caller funding_pool付款；每份metadata/header/Arc/mint/install可失败。 | 无公开authority前Owner/Signaler/Core本地退款。signaler mint失败release已mint owner，owner.on_last触发core.shut，内嵌Work稳定持有未公开Core直到空扫描完成；费用不因调用返回丢失。已公开最后读owner同路径CLOSED，Signaler/Observation仅共同保活。 | srv_init main.rs:362/377/859/865及FAL provider/client真实notification路径；正式core/stress/platform运行过这些消费者。 |
| Sender Lifetime | mailbox.rs:770–816 mint_sender授权MANAGE后，由LifetimeOwner::new(:786)及Sender出生；payer=caller funding_pool。metadata/ID/Arc/SenderCount/mint/install可拒绝。 | Lifetime出生authorities=1，未公开Owner Drop或最后authority各只release一次，双零直接不可重开CLOSED。SenderCount与Lifetime owner独立对称收束；来源强根与费用由Lifetime持到完成。该链归Mailbox Sender铸造，非Receive提交窗。 | libkernel/libfal真实sender身份与Lifetime观察组合；沿既有Mailbox/cap基础测试及正式路线。 |
| MemoryObject | memory_object.rs:52–85以所选Pool fund pages/固定extents，再付Core；317–349包装公开对象再付metadata/header/Arc/mint/install。 | fund错误在backing.rs:203–218映射quota→Pool错误，页/extent上限→ReachLimit，供给失败→OutOfMemory。未公开BackingOwner单一root退款；lastcap取走root owner，已有view use继续保backing直到退休，Core最后引用退宿主费。 | srv_init main.rs:2712/2778/2829分别真实create/map、seal和executable流程；Tunnel也复用同Core，现有正式core/stress/platform组合。 |
| Connection/Endpoint/Invitation | tunnel.rs:349–422的Core/Connection/首Endpoint/Invitation由request.pool CREATE付款，attach:425–498的第二Endpoint由attach funding_pool付款；每份metadata/header/Arc/mint/install/claim/state检查可失败。 | Connection.state.owner持BackingOwner。未公开首Endpoint live=false，mint invitation失败release不会误关闭真实侧；公开后sides/lastcap收束，双Closed取root锁外drop。attach拒绝由Invocation.restore回Invitation，预约rollback且释放新Endpoint，消费commit后责任不回caller。 | test_hammer concurrent_tunnel_close、tunnel_close_attach、tunnel_exit_target及librunnel真实映射owner组合。 |
| 宿主私有SignalSource | wait_source.rs:169–186仅metadata/Arc两拒绝点，宿主Pool实际付款；Job/Thread/Shell/Builder/Mailbox/Change各有唯一字段根。 | 无独立mint/KernelObject。WorkRecord::new(maintenance.rs:60–71)仅Idle/null next/root，无分配且不挂账户；只有publish才link。宿主出生失败本地refund，已经publish的CLOSED扫描/回执由Queue/Blocked强根完成。 | Job/Thread/Builder/Change DONE或CLOSED观察、自动回收及正式组合；不为来源造独立测试运行体。 |

本轮没有增加生产代码或测试设施。主代理补核了Notification真实部署、MemoryObject直接创建消费者、Lifetime属于mint_sender、fund错误映射及Record出生不挂链，消除sub-76五个事实未知项。精确Arc OOM、每个用户输出fault、attach安装交错仍是源码审定而不是注入行为证据；基础实际capacity付款前缀/allocator拒绝由47+10项host契约测试覆盖，跨组件成功/关闭/取消/退出由已过core、stress、release/sifive_u/nofd/hetero组合覆盖。当前有限矩阵不要求全分配点注入，也未将旧FAL改造作为完成前置。

资金共同守恒：出生值、capacity、View/计划scratch及Source/Work各有真实付款根；发布前拒绝由唯一准备值退款，发布后责任转稳定Work/队列/宿主，caller/cap消散不丢失。私有Work的last-live footer先退宿主费、access/ingress回执再发完成DONE；普通业务CLOSED按对象电平合同发布，不宣称仍由观察/引用保活的对象metadata已经退款。新独立对象没有借库容器本体费代替业务值出生费，没有把并发转移阶段计作退款。由七链与前述安装/IPC/等待/VM矩阵、既有守恒行为证据和真实消费者组合，本次有限资金门关闭。

## 最终结构检查、完成条件与后续触发

按REVIEW手册以最终代码/impls核对本次交付，不重跑未改变源码的已过检查，也不把独立提交后Review当作本次检查。前序定向finding的处理证据仍留原轮次：VM/View/backing退休与permit、侵入链接、来源安装/epoch/detach、输入Authorization/消费claim、输出cookie、Runtime多来源/Caller、Source last-live、metadata容量/值出生、Account归账与U scope/timer等均有具体owner与清理入口；新增累计工具的两条finding已经按site修复并复算。记录保留初始finding而不以新审查覆盖历史。

| 完成条件 | 本次结论与证据 |
|---|---|
| 1 承诺与边界 | 本篇范围+有限服务机会轮次为最终保证：结构性无饥饿、真实采样费用守恒、真实责任/资金owner、正常/可恢复M单步结构与固定ELF累计栈。TRANSFER环收集、全平台精确CPU份额/WCET、terminal诊断栈、全分配点注入均不承诺。 |
| 2 前置与消费者 | 共享ABI/内核与libkernel、Process/Builder/bootstrap、Mailbox/Notification/Lifetime、普通MemoryObject/Tunnel/View、Runtime/Assembly/Caller、fs/pm/RootSupervisor及正式hammer均接当前契约；没有空机制冒充系统消费。 |
| 3 正常/失败/退出与收束owner | 当前两份有限矩阵穷举已有创建、付款、输出、运输、等待、取消/迟到、lastcap与退出分支；准备值或已提交稳定Work是唯一owner。Body/VM gate、Source detach、私有last-live与DONE明确，Timeout失败沿原续体/绝对期限收束；无用户ProcessDrain活性依赖。 |
| 4 旧路径与重复真值 | 旧proc/request/wait_context、WaitMany、嵌套WaitSet、旧actor/Drain自检、Tunnel特有映射/许可退休、MetadataSponsor/多类计数与临时容量layout旧路径已删除。时间几何、派生K、资源深度/容量、ABI与来源电平各有唯一owner；没有meter facade、新调度器或财务最低伪费用。 |
| 5 验证与结构 | 最后执行代码的service-opportunity-check-002、acceptance-001（七面lint、stress16/16、release/sifive_u/nofd/Ready前失败组合）及hetero-001 exit0；维护host29项、存储拒绝10项、付款前缀47项已过。最终累计工具6672节点/41jalr/35jr、零未决，boot120000B/secondary119216B；impls旧迁移描述与四处源码旧等待注释已纠正。本轮只改文档/注释和审计工具，不重复构建。 |
| 6 延期不破坏当前保证 | 精确物理执行计量依独立平台合同；FAL历史midflight-copy flake在原独立记录调查，不扩大对象链范围。精确Arc OOM/逐输出fault/安装交错与terminal栈为未覆盖验证，并非保留错误实现；没有本次闭包内未完成接线/临时consumer/过渡owner。 |

后续责任只保留：

- **可靠执行计量平台**：owner为clock/架构平台，触发为具备可归属kernel CPU时间、hart间换算及计量误差合同；完成需等费用正样本/多hart/强制轮份额证据后再声明物理CPU份额。当前费用/机会政策不依赖该后端，未接空接口。详情仍由本篇“执行计量前置”拥有。
- **超出本次的故障注入/terminal栈**：触发为实际失败、ABI/付款/输出/安装责任变更或将terminal恢复列入系统保证；复用当前正式消费者和独立库契约，注明源码/注入证据差异，不造自检服务。当前owner/费用/取消正确性已有源码审定。
- **独立代码Review**：当前未提交。用户授权提交后固定本次提交或序列，按REVIEW登记独立review任务；提交不自动授权push。独立审查重点保留metadata birth/容量、unsafe存储/侵入链接、跨hart完成/接线、U/M计费机会、真实消费者和有限证明前提；不得以只审单一Diff取代整链责任范围。

本次约定范围已交付。源码、impls及本记录各自回填；COMPASS移向接续专题，矫正区不再将已替换的对象/等待结构当作施工中。当前工作树保留待用户授权的提交，后续不重开已审定计量选择或复制第二份对象链计划。
