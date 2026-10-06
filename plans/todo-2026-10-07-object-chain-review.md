# 对象链交付批次：独立 Review 与机制收敛

状态：**待执行**。用户明确决定本次只准备切面完整的审查任务并提交，下次再直接并行委派多个 reviewer。本次未启动这些 reviewer，不含独立审查结论。形成报告后，修复/复核唯一入口转到 `review-2026-10-07-object-chain.md`，本计划登记报告并归档。

## 固定审查目标

审查目标为**首次加入本计划的对象链交付提交**，该提交的树是唯一目标，不能用下次会话的 HEAD 或工作树代替。这样本计划与代码同次提交即可固定目标，无须把本提交自身哈希写进自身内容。

```sh
git log --diff-filter=A -1 --format=%H -- plans/todo-2026-10-07-object-chain-review.md
```

下次主代理先取得上述完整哈希，记入审查报告及每份委派 context；使用独立只读 checkout/worktree 固定该树，保留其他任务工作树改动。审查准备及运行均按 [REVIEW](REVIEW.md) 与 AGENTS。

- 内核拼装前比较基线：`253c04acc25799dd8f930ed883f1e760131924bc`。
- 中途施工检查点：`4b6e4c14e1af7ae94cc1f5a56f46771f8e8c6f21`，当时不可编译，不是本批次完整比较起点。
- 提交树全貌是事实；基线→目标 diff 用于定位变化，不能只读 diff 或只读已有检查点之后的变化。目标提交应包含新增生产文件、纯逻辑依赖及消费者，不漏 untracked 转新增内容。
- 实施/验证：[交付档案](archived/todo-2026-10-01-object-chain-kernel-integration.md#最终结构检查完成条件与后续触发)。设计分案仅用于契约与取舍；`notes/impls/` 用于现状导航，与代码冲突时记录文档 finding。未审定 ideas 不作为目标真值。

本批次范围：角色 capability/授权与消费、Process Shell/Body/Builder/Thread/Job 自动回收、来源/消息/独立值/容器/scratch 实际付款、VM/backing/View/普通 Tunnel、持久等待/Runtime/Assembly/Caller、真实服务/监督，以及采样费用与有限服务机会。

TRANSFER 环收集、可靠物理执行计量/精确 CPU 份额/WCET、terminal 诊断栈及全分配点注入没有承诺。FAL 的本次授权/运输/装配接缝纳入审查，FAL 全面重设计仍为独立任务。上述非目标不能用来忽略本次保证所必需的失败、取消、退款、活性与隔离。

## 下次直接执行的方式

1. 固定目标提交，读本计划、交付档案的最终契约/完成条件、KNOWN_ISSUES、REVIEW；记录实际可用日志和构建环境。
2. 给 R1–R5 各委派一个 `reviewer`，使用干净上下文，`inheritConversation: false`；cwd 指向固定 checkout。五个切面可以并行，没有先审谁才能开始另一个的依赖。
3. 每份委派复制“共同任务”及对应切面全文，将目标完整提交哈希、基线、checkout 路径置于 context；files 传该切面的入口源码/impl/契约。不可只说“审上面的改动”而不提供这些材料。
4. 主代理在等待期间亲读下表跨切面接缝，核对 ABI/失败移交与知识归属，不把主体判断全部交给子代理。每个 reviewer 可向 explorer/researcher 获取有限事实，不再派 reviewer 确认另一个 reviewer。
5. 收齐结果后主代理核实具体 source/可达时序，合并重复 findings，保留互相矛盾的证据及更正。报告成为唯一修复入口后，COMPASS 转向报告、本计划归档；不要建五份各自可更新的修复 todo。

### 所有 reviewer 的共同任务（可直接复制）

> 只读审查给定固定提交的目标切面，覆盖正确性、代码质量与结构收敛。实际代码优先于通过的测试、交付宣告、impls 和证明工具数字。核对正常/失败/取消/退出/迟到/最后引用、authority/owner、付款/退款、跨 hart 与锁序、unsafe/别名/Pin、单步工作上界及实际消费者。检查是否能合并机制、删重复真值/状态/阶段、减少分配和间接层，同时保留长期责任边界与演进能力。现有消费者/兼容不是保留错误结构的理由；只有一个实现或类型较多也不单独构成删除理由。不得加 facade/回调隐藏反向依赖。只报告，不改实现、ideas 或计划，不运行整套验收；具体 finding 需要时按 BUILD-AND-TEST 用最小现有 host/testcase 获取证据。硬件/ABI 结论按 references/CONTRACTS 固定规范核实，外部对照须查官方资料，记忆不作证据。输出按本计划“统一输出”格式，覆盖不够必须明说。

## 五个审查切面

### R1：付款、仿射存储与共享容器

**目标**：实际付款单位是否有唯一表示和 owner；容量/业务值、供给/准入、父债务是否守恒，是否能减少存储 wrapper 与预约对象而不绕过锁阶/失败边界。

入口文件：

- `shared/funded_storage/src/lib.rs`、`shared/ordered_table/src/lib.rs`、`shared/timer_queue/src/lib.rs`。
- `os/memory_pool/src/lib.rs`、`os/ready_queue/src/lib.rs`、`os/funded_frame/src/lib.rs`。
- `os/kernel/src/task/resources.rs`、`task/memory_pool.rs`、`frame.rs`、`heap.rs`。
- 付款消费者取 `task/handle.rs`、`process/members.rs`、`vm/views.rs`、`vm/operation.rs` 的准备/增长/回滚调用，不重新审其业务状态机。
- 说明/契约：`notes/impls/{internals,mm,task}.md`，`plans/todo-2026-09-14-kernel-memory-budget.md`，`user/libraries/README.md`；host tests 取 funded_storage/ordered_table、memory_pool、ready_queue 及 memory_space/payment。

重点：两维准入与实际 allocator 失败；Prepared/Owned/Token/MetadataCharge 的移动、split/merge、0 维 token；MaybeUninit 的初始化前缀/增长/clear/Drop；真实保留 capacity 退款；AVL 两子节点删除的同一费用 owner；侵入节点 Pin 与所有权；门外分配/释放；父深/费用与 Account 最后引用接缝。容器取得短付款许可不应隐藏反向领域依赖。

**收敛问题**：funded box/buffer、容器 Funding/PreparedEntry 与内核 metadata wrapper 是否重复承担一份 ownership？cap/ready/member/VM 容量增长的反复模式中哪些是共享存储机制、哪些是独立业务门？提案须给实际可删层/字段、退款点与锁阶变化，不能仅提“统一 RAII”。

边界：Account 队列/DRR 轮转归 R4；付款发生后的 VM 事务资源归 R2；capability 的消费语义归 R3。R1 负责共享原语错误，消费者违反原语合同归对应切面并互引。

### R2：VM、backing/View、页表事务与普通 Tunnel 映射

**目标**：准备→发布→必成提交/回滚→运输/fence→退款/DONE 的责任链，以及 ledger/实物索引、许可/root/use 的独立事实是否必要。

入口文件：

- `os/kernel/src/vm/{operation,change,backing,views,space,table,remote,retire,completion}.rs`。
- `os/memory_space/src/{space,backing,object,ordmap}.rs`、`os/page_table/src/lib.rs`。
- `os/kernel/src/task/{memory_object,tunnel}.rs`，`user/libraries/libkernel/src/{mm,memory_object,shared_memory}.rs`、`ipc/tunnel.rs`。
- ABI：`shared/erhino_shared/src/{memory_object,call,object}.rs`；同时按实际引用追内存/Tunnel ABI 文件。
- 说明/契约：`notes/impls/{mm,memory-object,tunnel}.md`，`plans/todo-2026-10-01-memory-redesign.md`，交付档案的 Map cookie、View 切界与最终表帧收束条款；host tests 取 memory_space/page_table。

重点：公开 Map 的字节单位/guard/cookie 发布；源表保护内的 use/写许可取得；Close/seal/RO→RW 与源权限；不相交事务切同 view；付款准备与容量复检；退休片段精确性；相邻预约边界及内部表摘除；ack/槽归还/fence 后页表/物理资金退款；构造失败、输出失败、caller 退出后稳定 Work；whole State 最后析构和 Completion/access 屏障；Connection/Endpoint/Invitation 与普通 View 的两个独立 owner。

**收敛问题**：Operation/Preparation/阶段 scratch/ChangeWork 的多次封装和状态是否共同描述同一事实？ViewRetirement/BackingRelease、ledger/View 切界是否存在可删分支或重复预约？transport 与退休遍历能否减少重复游标而保持批次/fence身份？不以重新引入每 use 子 Work 或隐式映射恢复旧结构。

边界：容器和账本付款原语归 R1；Process 冻结/出生/Body gate 归 R3；runnel 页内协议/用户映射收束归 R5；Work 调度保证归 R4。

### R3：capability、对象角色、Process 与 IPC 责任

**目标**：授权、消费、公开出生、进程收束与最后 authority/引用是否分别建模且闭合；角色拆分与壳/体/票据/回执是否重复持有状态。

入口文件：

- `os/capability/src/{lib,table,signal}.rs`、`os/kernel/src/task/{cap,handle,object,resources}.rs`。
- `os/kernel/src/task/process.rs`、`task/process/{shell,body,builder,thread,ticket,members,reclaim,departure}.rs`、`task/{thread,job}.rs`。
- `task/{mailbox,lifetime,delivery,notification,system_reset}.rs`，`os/kernel/src/boot/bootstrap.rs`。
- ABI 与封装：`shared/erhino_shared/src/{object,proc,message,call}.rs`、`user/libraries/libkernel/src/{process,thread,ipc/capability,ipc/message,ipc/notification}.rs`、`libthread/src/lib.rs`。
- 说明/契约：`notes/impls/{task,ipc,startup}.md`，capability/自动回收定稿；capability 现有 tests、test_hammer 的创建/Close/退出/IPC 用例。

重点：Authorization 非消费与 claim/restore 的线性化；Once/transit 与普通运输衰减；MANAGE/CREATE/映射权限；handle.install output/出生/Released；Receive 摘出后 owner 与 Receiver Close；Lifetime authority/use 与存储 Arc；Frozen 保存 active 快照、construction/ingress/access/membership gate；Ready/Waiting 执行 owner；Builder Prepared/Pending/Started/Aborted 与子线程容器根移交；Body/VM 完成后 Shell/Job/Thread DONE；last-live metadata 退款与观察壳仍保活的区别；所有真实 bootstrap 失败路径。

**收敛问题**：角色对象各自的 header/Holders/来源实现是否有真正可统一的机制？Shell/Reclaim/Builder 的阶段、计数器、WorkReceipt、ConstructionLease、BodyLease 能否消除双重状态或中间层？业务 CLOSED 与资源完成 DONE 不得为合并而混淆；普通 authorization 不得通过复制消费 authority 简化。

边界：R3 不重新审 AVL/paid allocation 内部（R1）或 PTE/fence（R2）；Thread 等待注册/epoch 归 R5，执行 owner/Departure 移交仍归 R3；U/M 执行费用归 R4。

### R4：维护执行、调度、时间、U/M 费用与原子上界

**目标**：费用与机会权分别守恒，粗 TIME 的 0 样本仍能有限轮转；最后析构/父归账完整计费；结构界和栈证明前提符合真实可达代码。

入口文件：

- `os/maintenance/src/{account,record,rotor}.rs` 及 host tests。
- `os/kernel/src/{maintenance,sched,clock,main,trap,rt,uaccess,sync}.rs`，`boot.rs`，`os/kernel/src/vm/remote.rs` 的维护入口。
- `os/ready_queue/src/lib.rs`，按调用追 sched_domain/runtime_gate/平台 timer，不整包重审其他硬件机制。
- 说明/契约：`notes/impls/{internals,execution-context,time}.md`，维护执行定稿与交付档案“有限服务机会”“最终累计栈与M原子结构收口”；外部 time/cycle/SBI 契约定位 CONTRACTS。

重点：双环/started/在途 gate/等在途封轮，信用/负债与机会 K=q、丢正保债；真实 C=0 不造金融成本；pick/step/finish/drop(root)/Account父归账/settlement 的计费区间；锁等待和 IRQ；强引用环/摘环；Candidate U 的绝对期限、timer装填费用、连续 M 期限复检；Trap/SUM/用户 Frame 完成推进唯一性；门铃失败后 Ready 活性；每实际 Work 的循环/锁内高阶操作与准备分配资源界。

**收敛问题**：Account/Ledger/Rotor/Resume 是否重复仲裁、信用或状态？是否能合并状态/访问入口而保留 Smp 在途与父财务 owner？timer、U scope 与 M step 的记账是否有重复 wrapper/取时或不必要政策特判？区分有成本依据的结构复杂度与施工补丁，不重开已确认的“有限机会+真实采样费用”保证，除非反例推翻它。

累计栈现有本地 artifact 为 `artifacts/maintenance-stack-bound-final.py/.json`、`maintenance-dynamic-sites-004.py/.json`、固定 ELF SHA256 `294a36aa292db1d99e0bdee2060a01f5f07499ecd76ffb9780cc1e092f466082`，历史结果 boot120000B/secondary119216B。核对强根/空字段/回执层次/动态方法集/正常格式化前提，不将工具算出数字当正确性 oracle；缺本地产物注明证据不可复核，不给新 ELF 套旧数字，也不启动无界全平台 WCET 研究。

边界：业务状态机由 R2/R3/R5 审；R4 负责它们对执行原子/费用/上下文的承诺与生产调用的计量覆盖。

### R5：持久等待、Runtime/Assembly/Caller 与真实用户装配

**目标**：来源安装/复检/提示/候选/注销/迟到的责任及用户业务续体、失败 owner、监督是否闭合；新执行/等待层是否能进一步合并。

入口文件：

- `os/kernel/src/task/{wait,wait_set,wait_source}.rs`、`thread.rs` 的 epoch/等待出口，以及 `shared/timer_queue/src/lib.rs` 的消费者。
- `shared/erhino_shared/src/{wait,wait_set}.rs`，`user/libraries/libkernel/src/ipc/{wait,wait_set}.rs`。
- `user/libraries/libexecution/src/{source,assembly,runtime,work_queue,wake}.rs`，`librpc/src/{caller,dispatcher,exchange,outbox}.rs`，`libprocess/src/{supervise,observation,management,context,race}.rs`。
- `librunnel/src/{lib,mapping}.rs`，`libkernel/src/ipc/{tunnel,invitation}.rs`，`libbudget/src/lib.rs`、`libruntime/src/rt.rs`，libservice/FAL/fs 的相关授权/stream/注册接缝。
- 真实消费者：`user/services/srv_init/src/{main,supervisor,building,public_ipc,race,time_checks}.rs`、`srv_fs/src/{main,server}.rs` 及 server 子模块、`srv_pm/src/main.rs`，`user/tests/{test_hammer,test_fal,test_target}`。
- 说明/契约：`notes/impls/{runtime,framework,call,rpc,runnel,fal,task,time}.md`，`user/libraries/README.md`，统一等待源轮次5；libprocess 现有 host测试及真实消费者场景。

重点：扁平 WaitSet Add 准备/挂链/输出失败、Remove/Close 与 Receive 提示交错；hint 只促重读；Thread epoch/pending 与安装/取消/detach；SharedSet 单接收/关闭 owner、cookie/token/迟到代际；Runtime 发布失败保留原 owner、Finalizing FIFO/来源回执收束；CallWait prepare/send/sent/reply 注册切换与 timeout；Runnel Start/acquire/普通 View/partial failure/peer close；Job DONE监督而非Drain循环；多运行体真实装配与退出最后清理。

**收敛问题**：SourceOps/SourceSet/Task/Runtime/Assembly、CallWait/ReplyObservation、同步/异步多来源是否有重复状态或双层退休/监督？服务场景的宏/容器/策略 wrapper 是否隐藏责任或只适配施工期？提案必须接真实 fs/pm/init 消费者，保留一个接收 owner、晚通知与失败 owner，不回退嵌套 WaitSet、临时 sender锁或 Busy/yield adapter。

边界：capability 授权/消费和进程 owner 归 R3；共享侵入容器错误归 R1；内核/用户计费政策归 R4；内核映射/backing owner 归 R2。FAL 只查本次接缝，不将全面重设计当作必需整改。

## 跨切面接缝：主代理负责组合

| 接缝 | 主审切面 | 组合必须核实的交接 |
|---|---|---|
| MetadataCharge/实际存储→业务准备 | R1 → R2/R3/R5 | 实际付款根、成功后旧capacity退款、供给失败原owner、锁阶与门外释放 |
| Operation/VM access→Builder/Body退休 | R2 ↔ R3 | caller消散后的稳定Work、root/use/许可、Completion/access/ingress与DONE先后 |
| Work/Source→账户/执行器 | R4 ↔ R2/R3 | Ready/Blocked根、queued后最后释放竞态、finish父归账、零样本机会与有界原子 |
| Thread/WaitSet→Runtime/Caller | R3 ↔ R5 | 执行 owner、epoch复检与detach、token/cookie代际、迟到回执、取消后的退款 |
| Tunnel→普通View→Runnel/服务 | R2 ↔ R5 | Invitation claim失败、Connection root与View use、协议Start、partial progress与退出 |
| cap/IPC→FAL/监督 | R3 ↔ R5 | 非消费Authorization/消费claim、输出失败、消息authority/use、Job完成与实际owner |
| ABI→封装→真实消费者 | 主代理组合 | 调用号/角色/掩码/字节单位/输出发布与所有消费者，无旧路径或重复真值 |

切面按责任分工，不以目录禁止追调用者。发现跨边界问题时报告完整时序并写主owner/协作切面；主代理合并成一条，不让两边各自认为对方会审。

## 统一输出与收敛判断

每个 reviewer 返回以下四部分：

1. **实际覆盖**：亲读的机制/消费者、运行的验证、未覆盖原因。不声称“全切面无问题”掩盖只读局部。
2. **缺陷 findings**：建议标识 `R<切面>-B<n>`，严重度 P0/P1/P2，精确 file:line、触发/并发时序、违反的独立合同、影响、建议修复及最小验证。无证据风险或风格不列bug。
3. **结构收敛建议**：标识 `R<切面>-S<n>`，给现有重复/缠绕的代码证据、替代owner/状态/接口、可删内容、保留的演进能力、消费者同步范围及验证门；列收益/代价/前置、是否影响已确认语义。不只建议重命名、少写代码或“抽公共函数”。指出已经合理、应保持独立的边界及原因。
4. **文档/证据差异与组合问题**：代码/impls/交付记录不一致、适用验证缺口、需要主代理组合核实的跨切面问题。把本次正确性欠账与范围外独立能力分开。

主代理最终报告分别列缺陷与收敛建议，逐条记录核实/更正、推荐取舍、修复责任与待用户决定项。保留原始发现；不把建议自动算成应实施任务，不把已交付宣告阻止推翻错误结构。涉及新关键语义或方向的选择回设计；通常内部简化在获准修复范围内自行决定。

## 验证证据与限制

目标批次最后执行代码的本地证据：`service-opportunity-check-002`、`service-opportunity-acceptance-001`、`service-opportunity-hetero-001` exit0；29项维护host、10项存储拒绝、47项付款前缀及对应lint。acceptance覆盖七面lint、stress16/16、release/sifive_u/nofd、Ready前panic/alloc/fatal。完整命令/日志/窗口由交付档案拥有。

这些结果是已有行为证据，不自动证明未知交错或结构合理。精确kernel Arc OOM、逐输出fault和部分安装窗口仅源码论证；terminal日志不在正常累计栈结论内。日志/ELF通常为本地ignored artifact，未来clone缺失时明确说明，不伪称能复核所有历史证据；按具体风险复用现有host/testcase，构建统一走just、host显式target，不新建测试服务/运行体，不机械重跑整套路线。

## 接力与完成门

当前未开始独立审查、没有 findings、没有新生产改动。下次从固定目标提交开始并行R1–R5，主代理执行组合审阅。完成审查要求五份有边界的输出、跨切面owner闭合、原始发现核实与统一报告；实际修复不由“审查结束”冒充完成。

报告开始拥有 findings 后，将本篇归档并把COMPASS唯一入口切到报告。同组问题的修复、验证和复核只在报告更新；独立延期项须有owner/触发/完成标准。只有全部处理/复核或合法独立延期后才归档报告，不逐条重新首审，不为每次Git提交生成新review计划。
