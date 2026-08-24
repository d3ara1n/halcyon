# 罗盘

> 跨会话导航：方向、位置、戒律。只存上下文不排任务——走法由目标与架构自然序决定，每次收口时维护。

## 方向

构建 notes/ 所描述的系统：RISC-V 微内核 eRhino（内核 erhino_kernel，用户态 rinlib）。方向性结论（细节见 notes/ 对应篇）：

- **微内核 ↔ 协作式互为因果**：长工作一律在用户态服务，内核路径恒短，内核态不可打断是推论不是选项（impls/internals.md「内核中断模型」）。
- **调度 = 域—类—执行点三层**：异构 hart 即多域；策略在类内整体可替换；扩展是横向加项不是改结构（ideas/task.md、impls/task.md「调度」）。
- **单一归属不变量**：线程任意时刻恰处于一个容器——类队列 / hart current / 无容器（impls/task.md）。
- **唤醒所有权**：timer = 自己的确定期限，IPI = 他方请求，无主唤醒不存在（impls/internals.md「唤醒所有权」）。
- **异步 syscall = 内核请求 + wake**：内核永不等待，阻塞表达为 Waiting，完成即唤醒（ideas/call.md）。
- **ABI 演进两侧同步**：shared/ 不冻结，内核与 rinlib 一起改。
- **框架先行、实现从简**：整体系统设计为先，搭框架再填充——结构一次到位，实现按需求从简；将来换复杂实现不动结构（调度域/类即范例）。

## 位置

- 已完成：boot/高半区启动协议、帧池（os/frame_pool）、堆、Sv39 页表（os/page_table，区域切段，host 测试）、板级解析（os/dtb）、任务模型（trap 路径与 trap 锚、域—类调度、进程/线程、initfs/ELF 装载 os/tar + os/elf、syscall 面 Debug/Exit/Extend/Sleep、进程回收、timer/IPI 通路）、执行环境重构（a9a65cb：ISA 契约面/ELF 执行需求判定/Base64-D64 分档/CSR 三边界所有权）、IPC 前地基工程（hart 身份统一、锁内存序、所有权单向化、通用等待模型 wait_gen 代数仲裁 + 循环后发布、uaccess 集中化、Extend 字节 sbrk 语义；见 plans/archive/2026-09-pre-ipc-groundwork.md）、IPC 契约与消息/信号语义注入（2026-08：契约拍板见 plans/ref-2026-08-ipc-contract-research.md 与 notes/ideas/{message,signal}.md——邮箱有界 FIFO 永不阻塞满箱 MailboxFull、对象切面信号状态 = 粘滞余量 + 移交队列、Receive 阻塞形态经 KernelRequest 同构接入、内核托管位 NONEMPTY 与消费式进程信号位的清除策略二分、SignalWait 多源等待代数仲裁去重；内核 task/ipc.rs + rinlib wait_message/peek/receive/discard/signal 封装）、隧道/Runnel 数据面（2026-08：机制契约与协议规格见 notes/ideas/{tunnel,runnel}.md——登记制随机 id、帧生命周期 = 双端全亡、门铃制内核不解析协议、环形字节流单写者原则、acquire/release 内存序为规范义务、清铃前置 = 排空；内核 task/tunnel.rs 四调用 + rand 模块（Zkr seed 主路 / 启动时刻混合兑底）+ librunnel 协议库（host 单测覆盖规格不变量，阻塞层封装排空纪律））。
- 已验证：四服务在 virt 4 核、virt 1 核、sifive_u 5 核下完成用户态 main（fs 因 FAL 桩语义化后 panic 退出回收，属预期，见 KNOWN_ISSUES）；消息/信号通路集成负载：init 自发自收（同步）、pm→init 跨进程投递（阻塞移交/队列快路径两时序均收敛）、TERMINATE 经 SignalWait 消费；隧道/Runnel 集成负载：跨进程字节流 8192 字节（超容量分批 + 回绕）pattern 校验、PEER_CLOSED 终态、帧归还，virt（Zkr 真熵主路）与 sifive_u（混合兑底）双平台全过。旧内核单核挂起、SMP 别名 UB、进程不回收三大死穴均已在增量证伪中。virt 30+ 轮压测全过；sifive_u 野跳转已归因修复（free_subtree 巨型栈帧越界踩踏相邻 hart 栈，非时序问题；调查方法论见 plans/DEBUG-PLAYBOOK.md）。
- 自然序往后：FAL/FS（fs 集成负载复活是验收线，流通道即 Runnel 隧道）→ 服务化（pm 接管 spawn；用户态多线程设计含 rt 监听线程与退出语义，notes/ideas/signal.md 分发层节已有草案）→ 设备/中断接入 → 异构（效能核多域、实时核 AMP）。
- 对照负载：user/ 四服务；fs 的 FAL 依赖面是下一里程碑的集成验收场景。
- 旧内核是行为对照系统：plans/ref-2026-08-legacy-kernel-design.md（按需参考）、review-2026-08-mm-map-bug.md 与 review-2026-07-code-review.md（教训档案）。

## 戒律

- 内核态路径保持短；出现「必须内核抢占」的需求 = 工作放错了地方，修方向不修模型。
- 公平性靠数据结构性质（FIFO 等），不靠记账字段——旧内核死因是记账字段无写入点。
- 用户可触发的 fault 一律杀进程绝不 panic 内核；syscall 未知号返回错误。
- 全局状态按三层纪律（hart 私有走 tp / 对象走锁 / 全局 OnceLock+Spinlock），禁 static mut。
- 用户内存访问：SUM 直访 + translate 前置校验，不软件遍历页表拷贝。
- 框架先行、实现从简：结构一次到位，实现按需求渐进替换（如调度域/类）。
- 共享 ABI 改动内核与用户态两侧同步，不留单边。
- 文档即决策：方向性结论进 notes/，本文件只导航；每轮收口于「决策入档、代码全绿已提交、下一步自然序明确」。
