# 用户态分层与线程生命周期设计审阅

> 已归档；历史线程分层设计问题由运行时分层归档、对象链交付和后续统一整理承接，不再作为独立实施 Review 入口。

## 结论与范围

libkernel / libprocess / libthread / libruntime 的总体知识划分成立，设备描述、资源 authority、租约和功能服务分开也成立。当前最需要纠正的是启动资源准备边界，以及 PTR 的机制归属、执行活性和三种不同寿命的耦合。不能据目前材料宣告线程设计已闭合。

用户要求的 **Attach 提供资源，Start 发布状态** 尚未完整实现：Attach 已构造 Thread/UserContext/ThreadDeparture，但未取得调度准入；Start 仍选择执行域、分配交接 Vec、预留 Ready 容量。现状是“Start 内先准备、提交后不可失败”，并非“Start 仅发布”。这不是最近两次文档提交引入的运行期回归，也没有据此证明现有 Start 事务不安全；它是相对于此次明确设计目标的结构差距。

本报告拥有本组 findings 的修复与复核记录。涉及方案变更的建议尚未采纳，不自动覆盖现有 ideas 的已定方向；采纳后应在所属 idea 更新契约，在本报告记录处置，原专题计划只链接相应结论及施工依赖。没有修改 Rust、ABI、Cargo、构建配置或已定设计。

## Findings

优先级中的 P1 表示本轮线程迁移施工前须解决的契约问题，P2 表示应同步修正文档或结构；不把设计缺口等同于已发生的运行故障。

### D1 · P1 · Start 仍持有可失败的资源组装责任

**位置与事实**：

- [task](../../notes/ideas/task.md)「组装双通道」规定 Start 只发布满足 readiness 的环境；[bootstrap](../../notes/ideas/bootstrap.md)「Job 与 Process capability」仍把完整交接容量、执行域和 Ready 容量准备归入启动协议。
- `os/kernel/src/task/process.rs:883` 接受 `profile`；`:919` 才 resolve_domain；`:928` 创建并 reserve staged Vec；`:942` 为全部成员 reserve_ready。`:954` 越过 Job/lifecycle 提交门后，`:962` 才 bind_execution，随后消费 Builder、publish。
- `os/kernel/src/task/proc.rs:4471` Attach 构造 Thread 并登记 Staging；`sched.rs:118` 表明调度准入绑定目标域的实际类存储。Attach 目前不知道 Start 才收到的 profile，因而不能完整支付该准入。

**可达场景与影响**：所有 Attach 都成功之后，Start 仍可因交接 Vec 或 Ready 存储 OOM 失败；成功 Attach 的含义尚不包括“这个线程已具备发布所需的完整内核资源”。如果未来在 Start 补建 Attached Control、packet 或 owner 交接，会进一步扩大该边界。

**建议目标**：在 Attach 前明确不可变 execution requirement 与初始域；Attach 取得完整未发布线程及其全寿命调度准入，存入 Building owner。Start 不再接收新的资源描述，不分配执行结构，只检查状态、在途组装、Job 门和 readiness，关闭组装权并发布已有 owner。Start 仍允许因并发终止、Job seal、Building 操作在途或未准备好而拒绝，纯发布不等于无条件成功。

执行需求可以是 Create 的不可变配置，或独立 Building 绑定；**必须提前确定**是确定结论，具体 syscall 放置仍需选择。若保留“先建未知映像空壳”的需求，显式绑定更合适；若所有创建者已知映像需求，Create 参数可少一个操作。不要通过首次 Attach 隐式改变整个进程的执行需求。

**完成门**：Start 调用链不再分配交接/调度/观察结构；Attach/Spawn 的失败和 Building abandonment 完整释放预付 owner；bootstrap 复用同源发布机制；Start/Seal/Kill/Attach 竞态保留现有线性化与有界性。不能为消除 Vec 而引入无 owner 的跨 syscall 发布尾段。

**状态**：待设计裁决。

### D2 · P1 · Attached 的“执行前登记”与“入口采用”尚不相容

**位置与事实**：

- [task](../../notes/ideas/task.md)「线程」与「用户态线程资源收束」、[bootstrap](../../notes/ideas/bootstrap.md)「启动映像的寿命」已承认 Attached 由构建者准备、子入口采用。
- [用户内存 owner 计划](../todo-2026-09-14-user-memory-owner-lifecycle.md)「ProcessThreadRegistry 设计裁决」「生命周期与关闭顺序」仍要求 Attached/Spawned 都在进入执行前登记同一 ThreadRecord，且 control、栈、packet、环境全部稳定。
- [framework](../../notes/ideas/framework.md)「启动上下文与入口方案」在子入口运行后才初始化 allocator/PTR。
- 当前 `process.rs:733` Attach 只返回 Tid；`proc.rs:4847` 以 `None` 创建 departure control。构建期栈经 libprocess 的 Building Map 供给，现有出生块没有可直接采用的完整栈 reservation owner。

**可达场景与影响**：Rust `_start` 要运行才能建立本进程 PTR，却被要求在运行前已经进入该 PTR。父进程也不能替尚未运行的子 runtime 构造任意 Rust F/T/E 对象并安全转交其内部指针。仅将 control 数值和栈顶写入 payload，不足以证明映射独占权、guard 范围和采用唯一性。

**建议目标**：把交接分为两段：Building 期交付可验证、版本化的启动资源描述和 child-local Control；子进程 runtime 在自身稳定存储中创建并一次采用 entry record，然后才进入应用级线程 wrapper。控制/映射描述属于受控启动协议，不要求跨地址空间运输 Rust ThreadRecord。统一的是采用后的清理协议，出生事务不必相同。

交接必须同时覆盖：未 Start 就放弃、Start 后入口未采用即失败、采用部分失败、采用成功后入口线程先于其它线程结束。Control 可以复用普通 Grant 交付，或 Attach 原子安装到子表；目前没有足够理由断言某一种 ABI 唯一最优。

**容易漏掉的实现约束**：当前 `lifecycle.rs:708` 的 Staging 终止路径摘成员后直接交 `process.rs:1027` drop，并不经过 `ThreadDeparture::finalize` 发布 DONE。现在 Attached 没有 Control，因此不能把它直接称为已有 bug；一旦给 Attach 增加可观察 Control，必须同步定义从未运行线程的完成发布，否则等待者可永久等不到 DONE。登记已获胜、终止后才完成 Attach 的直接退休路径同样须覆盖。

**完成门**：一份明确的 descriptor/Control/映射 authority 协议，列出父侧、子入口和 ProcessDrain 的唯一责任及所有失败时序；删除其余“Attached 已在执行前进入子 PTR”的冲突叙述。

**状态**：已有 TODO，但关键时序尚未闭合。

### D3 · P1 · PTR 的推进者与最后执行者尚未闭合

**位置与事实**：[task](../../notes/ideas/task.md)「Join、期限和清理」承诺 Thread Drop 只放弃观察、记录由 PTR 保留；[framework](../../notes/ideas/framework.md)「allocator、终止与平台边界」规定 main 返回只结束当前线程；[用户内存 owner 计划](../todo-2026-09-14-user-memory-owner-lifecycle.md)「ThreadDriver 与关闭」仍把实际执行形态、poll 时机、唤醒和最终关闭列为 TODO。

**可达场景与影响**：

1. A 创建 B 后丢弃观察权，A 长期运行纯计算或等待业务，B 结束。仅保存 record 不能证明有人执行 cleanup；如果只在 spawn/join/应用显式 poll 时推进，责任虽未丢失，长期资源回收仍无活性保证。
2. 若增加普通用户态专职 driver，它也属于内核线程成员；等待 PTR 全空会等待自身 DONE，直接停止则须交代它的栈、record 和观察资源由谁完成。
3. main 返回错误码而 worker/driver 最后以 0 离场时，当前 `lifecycle.rs:677` 由最后成员的 code 冻结进程终因；driver 会影响可观察退出结果。不能把它当成无语义的执行细节。

**建议目标**：先固定运行政策，再落类型。若要在应用不合作时仍回收已结束线程，需要获得调度机会的独立用户执行上下文或等价的已证明执行保证；一般库函数内的轮询不足以提供该承诺。若选择 cooperative 模式，必须明确其进展条件并收窄承诺。

专职 driver 的一种可行终局是：应用线程准入关闭并离场、普通记录收束后，最终执行者以显式终局交接结束；它自身无法在 DONE 后继续执行的少量栈/record 由 ProcessDrain 收束。这符合已有最终进程资源边界，但应明确例外的成立理由、剩余资源及退出码政策，不伪称所有 record 都经过用户态 Retired。其它执行方案可能成立，目前不能证明唯一最优。

**完成门**：driver 启动前的容量与 authority、独立唤醒、跨线程提交、运行期失效处理、seal/close 竞态、最后执行者和退出码各有确定时序；main/worker 持续运行且不再调用线程 API 时的 cleanup 进展有证据。

**状态**：已有 TODO，是架构前置，不是单纯施工接线。

### D4 · P1 · 结果寿命未闭合，完整 record 类型擦除方案冻结过早

**位置与事实**：[分层计划](../todo-2026-09-runtime-kernel-library-separation.md)「libthread 接口稿」明确 ResultReceiver 的寿命和签名待定；[用户内存 owner 计划](../todo-2026-09-14-user-memory-owner-lifecycle.md)「稳定 record 的类型擦除与安全边界」却把 `Box<CleanupRecord<F,T,E>>`、vtable 析构 T 和资源/结果都结算后才退款作为已冻结方案。

**可达场景与影响**：线程 DONE 后，使用者很久不领取 T。若 T 必须留在整个 record 内，RegistrySlot 会受未领取结果而非运行/待清理线程控制；若清理线程资源时直接释放 record，receiver 的存储寿命又不成立。“返回 stale”不能替代对已有 T 的析构和独占领取证明。结果可以先领取、资源 cleanup 尚未完成的反向交错同样存在。

**建议目标**：按独立寿命划开三项责任：

| 责任 | 保活至何时 | 推荐提供者 |
|---|---|---|
| 栈、环境、内核 Control 引用与 cleanup | DONE 且 cleanup 完成 | libthread registry record |
| DONE 观察 | 最后观察者消散 | 稳定 ThreadControl 的共享用户 owner |
| T / outcome 与一次领取 | 已领取或最后领取权消散并完成析构 | 独立结果存储与唯一领取权 |

具体可以共用一次分配或拆分分配，但不能让共享布局变成错误的共享寿命。结果存储的 Charge 随其实际占用保留，不能随线程槽退款；线程槽也不应无独立理由被一个长期保存的观察 handle 或未领取 T 占住。

**完成门**：明确 DONE、结果发布、结果领取、receiver Drop、record cleanup 五者的允许交错；证明 T 恰好一次移动/析构及初始化内存序。再决定 vtable 和分配布局。

**状态**：已知未决项；“类型擦除与 SAFETY 已冻结”的表述须收窄。

### D5 · P1 · 单步 cleanup 的有界性不能覆盖任意 Rust 析构

**位置与事实**：[task](../../notes/ideas/task.md)「Join、期限和清理」规定一次推进一个有界动作；[用户内存 owner 计划](../todo-2026-09-14-user-memory-owner-lifecycle.md) 的 vtable 同时承担 T 的 Drop、环境退休和映射/Control 清理。现有契约只要求 F/T/环境状态为 Send，不能由此推出析构有界或非阻塞。

**可达场景与影响**：无人领取的 T 的 Drop 等待另一个线程的结果、获取用户锁或执行长循环；单一 driver 在这个析构上停驻，则其它 record 不能取得 cleanup/结果交付。若该结果交付又依赖同一 driver，可形成等待环。Unmap/Close 本身也可能停驻，但其内核退休完成与任意应用析构的进展前提不同。

**建议目标**：区分机械 cleanup step 与任意用户代码执行。结果/closure 析构优先随独立结果或准备 owner 的消费在明确的用户执行上下文执行，不把它隐入共享 registry 锁或承诺公平的控制循环。环境 retire 的调用前提、是否可停驻及其对 driver 的影响必须明示。若最终仍由专职 driver 承担任意析构，就要接受并说明该线程可被业务代码阻塞，不能同时承诺所有记录公平推进。

本 finding 不要求内核抢占，也不要求再造执行器；它要求准确界定用户态执行政策。计一次 step 不等于动作耗时有界。

**完成门**：每条执行任意 F/T/E 代码的路径都说明执行者、锁状态、可等待对象和失败政策；清理其它记录所需的进展不依赖一个可能反向等待它们的析构。

**状态**：本轮新增发现，待裁决。

### D6 · P2 · PTR 的知识 owner 与实例 owner 混在一起，可收敛掉登记桥接

**位置与事实**：[task](../../notes/ideas/task.md)「线程环境与结果交接」规定 libthread 定义 record/state machine、抽象登记端口，具体 PTR 由 libruntime 持有；[用户内存 owner 计划](../todo-2026-09-14-user-memory-owner-lifecycle.md) 再定义 runtime 侧的槽、计数、ready 扫描、claim、预算和 seal。record 的状态合法性与这些 registry 规则共同构成线程生命周期。

**影响**：如果“libruntime 持有”进一步变成“libruntime 实现上述通用规则”，相同线程状态会跨两个库协商。没有 Cargo 循环也会产生生命周期知识的反向耦合，替换 runtime 必须重做 registry 安全证明。

**建议目标**：libthread 同时拥有 ThreadRecord 和 Registry 的机制实现；libruntime 建立、持有其实例，提供环境实现、容量配置和 driver 装配。runtime 可以选择已有 executor 或其它执行上下文，但不定义 record/claim/槽复用算法。仅为了访问另一个库里的具体 PTR 而存在的抽象登记端口可删除；若未来确有多种独立 registry 存储契约，再按实际变化保留窄接口。

同理，公开观察面可直接共享已有 ThreadControl owner。它提供持久 DONE，record 清理无需让正常观察者 stale。`Arc<ThreadControl owner>` 可共享一个内核 entry，最后引用才关闭，既不复制关闭权，也不保活用户栈。除非另外确认主动撤销观察权的需求，否则不必向公开 Thread 引入 slot/generation/失效协议；内部可复用槽的代次仍按内部 ABA 风险决定。

**完成门**：唯一生命周期算法在 libthread，runtime 只有组合；公开观察是否需要失效有独立需求和契约。该项是结构建议，尚不能认定当前仅文档的模块描述已造成实际反向依赖。

**状态**：建议待采纳。

### D7 · P2 · 设计完成状态与实际阻塞相互冲突

**位置与事实**：

- [用户内存 owner 计划](../todo-2026-09-14-user-memory-owner-lifecycle.md)「设计完成门」保留 driver、Attach、关闭与终局的施工前设计门；「前置、触发与顺序」又称“本轮设计已完成”；「本轮设计产物与剩余门」称剩余工作属于代码实施与验证。
- [分层计划](../todo-2026-09-runtime-kernel-library-separation.md)「Kernel 与 shared 的施工边界」承认 Attach 交接可能改变 ABI，却又总结 kernel 不是独立硬前置、可建立在现有 ABI 上。现有 ABI 可供搬迁部分原语，不足以证明完整 Attached/PTR 能力可成立。
- 接力仍把 `bb38eee` 和“新文件未跟踪、未提交”写成当前工作树事实，与本次两个固定提交后的基线不符。

**影响**：后续会话可能把架构问题当成已授权的实现细节，先做布局和桥接，再反向补 ABI/退出政策。当前设计已决定的 main-return 语义也不同于 `user/rinlib/src/rt.rs:93` 的 ProcessExit 实现，须作为行为迁移明确验证，不能归为单纯改名。

**建议目标**：统一为“方向确定，D1—D5 所列关键协议未闭合，不进入完整线程迁移施工”；修正当前基线，历史工作树描述明确标为历史。采用建议后以本报告记录裁决，原计划不维护第二份 findings。

**状态**：待文档修正。

## 已有实现遗留与证据边界

以下不是这两个文档提交新引入的代码缺陷；新设计部分已意识到它们，但尚未实现修复：

| 遗留 | 当前位置与影响 | 处置要求 |
|---|---|---|
| safe thread::exit 绕过结果写入 | `user/rinlib/src/thread.rs:308` 是安全函数；`:148` 才写 T；join/Drop 经 `:158` / `:165` 无条件 assume_init，安全 closure 可以直接调用 exit | 高优先级既有安全问题；必须禁止安全逃生并建立结果发布状态，不能以 DONE 代替 T 初始化证明 |
| JoinHandle Drop 同步等待 | 同文件 `:284` Drop 经 `:254` 等待 DONE、关闭、解除栈；若当前线程持有子线程完成所需资源，可阻塞或死锁 | 迁移后明确 Drop 只放弃观察的语义，以及实际 PTR 推进保证 |
| 栈清理失败只记录 abandoned | 同文件 `:107`，非 Busy 错误和 sleep 错误丢失用户态继续处理入口，映射留至 ProcessDrain | 保留运行期 cleanup owner 与失败状态 |
| 入口寄存器准备不完整 | `user/rinlib/linker.ld:2` 为 ENTRY(main)，无 `__global_pointer$`；`thread.rs:137` 直接进入普通 extern C 函数；内核 `proc.rs:4857` 零初始化 gp/tp | 设计的 `_start` 和工作线程汇编入口方向正确；需要固定 psABI 及实际链接/反汇编证据，当前无新镜像证据 |
| 启动映像与 grants 采用 | 分层计划已登记普通 launcher 的出生块 RW、env 暴露 raw 初始 handles 与来源/role 证明分离 | 按已设计的只读进程寿命映像、唯一 StartupSeed 采用落实，不与 Attached 可回收栈混成同一 owner |
| main 返回语义 | `rt.rs:93` 当前转码后 ProcessExit；目标改为 ThreadExit | 明确这是可观察行为变化，覆盖 worker 继续运行、main 错误码、最后 driver 等场景 |
| “进程的死 = 末线程离场”表述 | task 该句与 Building/Running/Terminating/Dead 四态容易混淆；`lifecycle.rs:677` 末线程只是进入 Terminating | 写成“正常执行寿命结束/进入 Terminating”；Dead/CLOSED 仍以资源 Drain 完成为准 |

线程级结果义务与进程级 mandatory operation **不应为了减字段合并**：`thread.rs:163` 先确认结果义务归零、摘成员再 DONE；进程 REAPABLE 另外等待 active、Building 操作与 mandatory 操作。二者保护的资源与完成时刻不同。同样，ResultPublished、KernelDone 与栈 Unmap 完成也不能合成一个 done 位。

## 推荐的机制收敛

以下是可讨论的目标，不是已实施方案：

| 切面 | 保留 | 可删减的责任/机制 |
|---|---|---|
| Process | Job 归属、AddressSpace、HandleTable、Building gate、终止/Drain、不可变执行需求 | Start 中的资源发现、调度准入分配和临时线程组装 |
| 内核线程出生 | 一个共享的完整线程准备机制，Attach/Spawn 分别提供 Building/self authority 和不同提交协议 | 重复的出生资源准备；不为统一而合并两种 authority/线性化语义 |
| 发布 | 已准入 owner 从 Staging 交给 scheduler；有界、不可失败的尾段 | 发布时重新分配、重建 Control、重新选择执行 profile |
| libthread | 线程环境协议、Registry/record、唯一 cleanup claim、结果交接 | runtime 专属登记协议、重复槽/状态规则 |
| libruntime | 程序入口、allocator、panic/退出政策、Registry 实例和 driver 装配 | 通用线程状态机与记录算法 |
| 观察 | 稳定 ThreadControl 的共享 owner | 无独立撤销需求的公开 slot/generation/stale 协议 |
| 返回值 | 独立一次领取与析构责任 | 让结果领取者保活整个线程 cleanup record |

Start 的批量发布可以消费 Attach 已形成的 owner 集合。消除临时 Vec 可以用预备集合移交或同源稳定存储，但仍必须证明发布责任不被终止游标抢走、过程有界、未发布资源可收束；不能仅因想少一个容器就破坏已修复的竞争协议。现有上界与容量成本也须按真实资源模型沿用或调整，不创造随意常量。

无法宣称存在覆盖全部性能、内存、实时性和运行政策的唯一全局最优解。可以确定的结构改进是：准备与发布分开、同一知识只有一个提供者、不同寿命独立保活，以及明确实际执行者。是否专职 driver、执行需求放 Create 还是 Bind、结果是否共用分配，都须在这些约束内再比较成本。

## 设备设计部分

本轮通读 device 的领域、依赖、状态、恢复和 DMA 边界，未发现需要推翻其总体切面的确定矛盾。值得保留的设计是：描述不授权、占用不等于服务 Ready、关闭 Handle 不等于撤销、进程 Dead 不证明 DMA 静默、跨资源协调留在用户态、无法证明安全则隔离而不退款。

设备记录明确标为 D0/D1 组织层稿，D2—D6 未闭合，因此不将所有未设计 API 列成缺陷。继续设计必须验证：控制 authority 对派生副本/映射的完整撤销、管理者死亡后可达的上级接管、跨域预留取消、IRQ 代次与解绑、DMA backing 脱离进程寿命及硬件静默证明。这些已有唯一记录，见 [设备整体计划](../todo-2026-09-device-system-design.md) 的关键未决项。

设备模块是逻辑责任，不必各变成 crate/进程；当前 device 已明确这一点。后续不要为了对应图中的每个框创建服务，也不要以合并状态机为由消除硬件、进程、注册三个不同完成事实。

## 处置顺序与验证

1. 裁决 D1 的 execution/Attach/Start 边界，与 D2 的启动交接一起同步 shared、内核、libkernel、launcher 和 bootstrap 方案。
2. 裁决 D3 的 driver/终局及 D4—D5 的结果与析构执行位置，再冻结具体 registry 类型；D6 可在此消除跨库桥接和观察失效机制。
3. 更新所属 ideas、计划基线与施工门，逐项复核本报告。未闭合前不因 crate 搬迁局部编译成功宣告能力完成。
4. 实施验收按现有设施验证 Start 无准备分配、Attach/Start/Kill/Seal 交错、未启动 Control 完成、子线程先于 spawn 返回结束、观察者 Drop 后回收、延迟领取结果、main 提前返回与最终执行者等不同失败机制。

本次为文档设计与源码静态审阅，未修改代码，未运行编译、host tests 或 QEMU，也没有以已有测试通过代替上述新设计证据。报告与 COMPASS 共 59 个本地链接存在性检查通过，两个文件无行尾空白；`git diff --check` 通过。只新增本报告并登记 COMPASS 导航，未提交。

## 外部实现参照与适用边界

本轮 researcher 辅助核对以下官方资料；它们用于检验方案空间，不作为直接照搬接口或宣称最优的依据。

- **seL4 16.0.0**，固定提交 `6e7c3b733d296cfd88d5fbf635c96e447a882374`：[Threads 手册源码](https://github.com/seL4/seL4/blob/6e7c3b733d296cfd88d5fbf635c96e447a882374/manual/parts/threads.tex)「Thread Creation / Thread Deactivation」、[TCB 接口](https://github.com/seL4/seL4/blob/6e7c3b733d296cfd88d5fbf635c96e447a882374/libsel4/include/interfaces/object-api.xml)。新线程初始 inactive，先设置 CSpace/VSpace/IPC buffer、写寄存器；WriteRegisters 可以选择同时 resume，也可单独 TCB_Resume。MCS 运行还要求 scheduling context。该事实说明配置与激活可以分离，不证明 Halcyon 必须照搬 TCB API 或把 CPU 预约并入地址空间。
- **musl v1.2.5**：[pthread_create.c](https://git.musl-libc.org/cgit/musl/tree/src/thread/pthread_create.c?h=v1.2.5)、[pthread_join.c](https://git.musl-libc.org/cgit/musl/tree/src/thread/pthread_join.c?h=v1.2.5)、[pthread_detach.c](https://git.musl-libc.org/cgit/musl/tree/src/thread/pthread_detach.c?h=v1.2.5)。库分配栈的 joinable 线程退出后保留映射和返回值，joiner 最终读取并解除；detached 路径通过不再触栈的退出汇编处理自身映射。进程线程链表不等于集中 reaper。其 Linux 退出协议、TLS/栈布局和 POSIX join/detach 契约不同，不能用于论证 Halcyon 可以直接安全 unmap 当前栈；也不能据此否定已选的 PTR owner。
- Zircon syscall 在线文档只核实了接口概貌，未取得固定 release/commit 的源码证据，本报告不以其具体 Start 实现作裁决依据。

上述参照不解决 D3—D5：Halcyon 仍须依据自身 DONE/Map 义务、可停驻 Unmap、Rust 结果析构和运行活性要求确定执行政策。

## 处置（2026-09-26，用户已裁决）

| 编号 | 处置 | 落点 |
|---|---|---|
| D1 | 采纳：执行需求由 ProcessBindExecution 预先固定；ThreadCreate 准备全部资源，Start 只发布 | task、bootstrap、owner 计划 |
| D2 | 采纳：Building ThreadCreate 安装 child-local Control，`EntryThreadDescriptor` once 采用 | bootstrap、task |
| D3 | 采纳：无专职 driver，协作式回收，承诺容量不被已死线程占用 | task「离场与 cleanup」 |
| D4 | 采纳：拆为 ThreadRecord / 共享 ThreadControl / ResultCell 三个 owner，删除可失效观察壳 | task、framework |
| D5 | 采纳：F/T/enter/leave 只在用户线程执行，cleanup 只做机械动作 | task「结果发布」 |
| D6 | 采纳：Registry 本体归 libthread，删除登记端口 | framework、task |
| D7 | 已修正：owner 计划改为"方向已定、待施工"并给出施工顺序与验证门 | owner 计划、分层计划 |

另经裁决：删除全部线程与进程退出码，进程终局改为结构化 `TerminationCause`；`main` 返回 Ok 只结束入口线程。施工尚未开始，报告在实施验收后关闭。
