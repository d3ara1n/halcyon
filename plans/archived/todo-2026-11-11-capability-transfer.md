# 统一 TRANSFER 与 Receiver 转交

状态：本闭包代码与实际消费者已接通，host/静态/ELF 检查及交付前结构检查完成；按仓库流程归档，未提交、未运行 QEMU。实现结果、日志和证据边界见文末「实施交付与验证」。pm 本体及代理出生的正式消费者仍由[服务装配计划](../todo-2026-10-07-service-assembly.md)接续，不把本案的机制交付写成 pm 系统能力已完成。

## 已确认目标与边界

- 合并 GRANT / TRANSIT 为 TRANSFER，统一表达允许转交该份 capability；目标权利仍不得超出来源。是否可复制继续由 DUPLICATE 与对象性质决定，移动唯一 Receiver 不等于复制 Receiver。
- Receiver 应能通过正常能力转交到中间进程，再进入新进程，满足请求方 → pm → 目标的代理出生场景。不能仅改权限名字却仍保留等价的“双运输权限”限制，也不采用 pm 身份白名单或读取第三方能力表绕过转交。
- 消息缓冲与直接跨表安装的内部流程可以不同；Receiver 的持有、转交及可能形成的环由实际机制处理，而非通过禁止正常中转回避能力需求。
- 具体机制由下文 D1 确定，须处理自己的接收权进入自己队列、多个队列互相持有接收权等情况，明确允许的移动、失败结果及释放责任。不能简单开放 TRANSIT 而保留无法解释的永久滞留。
- 不预先承诺全对象图垃圾回收；所选方案涉及的环与当前合法转交闭包必须成立。既有更广不可达环的边界仍见[重构总纲](../todo-2026-09-29-system-redesign.md#不可达能力环的范围边界)，不能以该边界掩盖本案承诺的缺口。

## 接手基线与历史更正

核查基线为 master / d0a6fd8；已有服务装配及 IF2 文档改动保留。接手时源码为两个独立位：shared/erhino_shared/src/object.rs 的 TRANSIT(bit 4)、GRANT(bit 8)。mailbox::send 检 TRANSIT，process::grant 检 GRANT；MailboxReceiver 的 allowed_rights 含 GRANT、不含 TRANSIT。

原拆分来自 29c6519（2026-08-26），目的是避免 Receiver 在有缓冲消息中形成不可达持有环，保持 Send 无对象图遍历；见[原 ABI 计划](todo-2026-08-26-abi-foundation.md#handle-运输-rights)。该理由说明当时的取舍，不构成当前必须保留双位的约束。

[capability 重构档案](todo-2026-09-28-capability-last-reference.md)曾把合并 TRANSFER 及移除双位记为已完成，但与本轮接手基线源码不符；git blame 将基线中的两位追溯至原拆分提交，2aee028 的共享定义也仍是双位。本案承接这一文档/实现失配，档案保持原样，不将其完成宣告当成实现证据。现有正常运输机制不因此整体作废，缺失的是已记完成的合并及本轮 Receiver 中转能力。

## 设计阶段的实施门（已按文末结果执行）

1. 审计现有能力持有、消息接收/丢弃、Receiver 最后释放、Building 安装及进程退出链；核定 Receiver 转交方案，不另造 pm 专用旁路。外部参照按固定版本取证。
2. 闭合唯一持有的交接点、入队/接收与关闭竞争、失败释放及环处理；内核各次推进有结构性上界，需要后续维护的已提交责任由稳定内核 owner 持有，不依赖调用者继续调用。
3. 方案确定后，同一闭包迁移共享 rights、内核检查/允许矩阵、libkernel、实际用户态消费者及文档；删除旧双位与过渡兼容写法，ABI 不冻结。pm 只引用本案结果，不重复定义运输机制。

## 验证与交付边界

设计阶段尚未执行产品检查或运行验证；实施结果以文末为准。设计需覆盖唯一 Receiver 的正常两段移动、权限不增大、失败时唯一持有不丢失，以及自队列/跨队列环、接收或丢弃、任一持有进程退出的处理。优先复用 host 机制验证；实际 pm 接入由服务装配计划接续，正式运行由用户另选范围，不建 guest harness。

本案完成要求：统一权限已实际接通，Receiver 正常中转与所承诺的失败/释放路径成立，旧双位删除、消费者同步、必要验证与结构检查完成，impls 按代码回填。本段保留最初方向登记时的完成门；实际交付以文末逐项核对为准。

## 本轮调查接力（2026-11-11）

用户确认先由一个 worker 收集足够信息，再回主会话讨论。已委派 Subagent Pool 任务 `sub-1`，只读审计实际持有链、历史依据及固定版本外部机制，比较 Receiver 转交与环处理候选；不修改文件、代码或 ABI，不提交、不运行构建/测试/QEMU、不继续委派。报告返回任务结果，由主代理核实关键证据后与用户讨论，不将 worker 建议当作已批准方案。调查 `sub-1` 已完成并返回报告；主代理已复核下节关键源码依据。取证足以区分主要方向，但尚未确定最终机制，不进入代码实施。

### 调查结论与主会话复核

- 现有普通移动、提交前失败返还、Receiver 最后释放后关闭及分步排空机制已经存在；本案不能笼统重做这些责任。消息中的 Cap 是实际 authority，普通对象 Arc、观察来源和已接受 use 各有不同含义，不能用 Arc 计数直接判定能力可达性。
- Receiver 直接入队会产生 `Q_A → message → Receiver_B → Q_B → message → Receiver_A → Q_A`；接收权仍未归零，现有只在 closed 后运行的排空不会自行启动。
- 当前代码已可静态构造间接链：先把 Q 的 Receiver Grant 给 Building 进程 P，再把 P 的 Builder 发入 Q，形成 `Q → message → Builder → P/Body/CapTable → Receiver_Q → Q`。主会话核实了 process/builder.rs 的 RIGHTS、State.shell 和 on_last_capability，process.rs::grant，process/shell.rs::complete_if_empty（仅 Running 空成员自动冻结），mailbox.rs::enqueue 及 MailboxCore::step。该链不会仅因原组装者退出自动终结；外部有权监督者可能 Kill，不等于已有自动收束。此为静态推导，未运行复现，不宣称系统中任何权力都无法打断。
- 因此不能把所有能力表视为可主动推进的根，也不能只检查 Receiver 是否被直接排进某个邮箱，就声称解决含 Receiver 的所有责任环。是否及如何纳入 Builder/Building 等边须在方案中明确，尚未据此决定全对象图 GC。

固定版本外部证据：

| 方向 | 证据与结果 | 本项目适用边界 |
|---|---|---|
| 转交前检查 | [XNU xnu-10063.121.3 ipc_port.c](https://github.com/apple-oss-distributions/xnu/blob/xnu-10063.121.3/osfmk/ipc/ipc_port.c) 的接收权 destination chain；[ipc_kmsg.c](https://github.com/apple-oss-distributions/xnu/blob/xnu-10063.121.3/osfmk/ipc/ipc_kmsg.c) 标记 CIRCULAR 后销毁整条消息并返回 MACH_MSG_SUCCESS | 主会话已直接复核 ipc_kmsg.c 2966–2971 及 copyin 检测；不照搬成功丢弃语义，也不把任意长链加锁当成有界推进 |
| 局部在途图回收 | [Linux v6.12 net/unix/garbage.c](https://github.com/torvalds/linux/blob/v6.12/net/unix/garbage.c) 登记 SCM_RIGHTS 在途边并检查闭合分量 | 主会话核实 __unix_gc 持 unix_gc_lock 调用整图遍历；摘队列后锁外释放可参考，复制 fd、file_count 和整图锁内算法不能直接移植 |
| 会合交付 | worker 核查 [seL4 13.0.0 endpoint.c](https://github.com/seL4/seL4/blob/13.0.0/src/object/endpoint.c)、[thread.c](https://github.com/seL4/seL4/blob/13.0.0/src/kernel/thread.c)：会合时安装能力，无同类独立缓冲 capability 容器 | 会改变当前入队即提交、发送者退出不撤销的语义，不能伪装成内部重构；主会话本轮未重复逐行核查 |

worker 的 Zircon 固定源码取证只支持 endpoint 转移/并发 owner 检查，不足以证明任意环回收；L4Re/QNX 只作初筛，不作为算法依据。完整原报告保留在本会话 Subagent Pool `sub-1` 结果中；本节承载后续设计必要的事实和证据边界。

### 调查后初步建议（已由 D1 设计与同组审查接续）

主会话建议先保留现有缓冲消息外部语义，在真实持有/推进依赖模型上比较“提交前防环”和“提交后有界回收”，并用直接双邮箱环与 Builder/Building 间接链同时检验方案。不能把循环改成每步固定条数就宣称并发一致性及最终完成已经证明，也不能照搬 XNU 的成功丢弃。会合路径只有明确改变消息语义时才继续，不作为默认迁移。本轮没有选择最终算法、扩大能力保证或授权实施。

## 设计轮次 D1：仿射容器托管森林

2026-11-11 用户授权继续设计，并允许与另一助手讨论。主代理独占本文编辑；CoolHawk 只读独立推演与反例审查，不改代码或文档。本节初稿经下文同组审查、用户发布边界确认后收敛为当前目标；实施状态由文末接力拥有，不把设计闭合当成代码交付。保持消息入队即提交、未提交失败返还；不建设 pm 专用中转协议。

### D1.1 问题模型与边界

要维护的不是所有 Arc 的可达性，而是“容器中能力的推进依赖于哪一份唯一能力”。定义一个内部托管域 `CustodyDomain`：

| 域 | 被托管的内容 | 谁使其能够被取出/运行或放弃 | 图中定位 |
|---|---|---|---|
| MailboxCore Q | 队列及已暂摘、尚可回插的 Message capabilities | 唯一 Receiver | Receiver 所在域为 Q 的父域 |
| Building Process P | 其能力表及尚未完成的出生 | 唯一 Pending Builder | Builder 所在域为 P 的父域 |
| 已发布执行的 Process P | 其能力表 | 已发布成员的执行承诺；最后执行成员退出时原子冻结 | 独立根；未发布 Created 不保活，外部 ticket 不使其复活 |
| 已关闭 Mailbox / 已冻结 Process | 尚待释放的旧内容 | 已提交内核排空/回收 | 独立维护根，禁止新增相应业务准入 |
| 内核未公开构造暂持 | 尚未安装的唯一能力 | 当前有界构造调用 | 私有暂持根，失败沿原 owner 放弃 |

一份 Receiver 或尚有效的 Builder 对应一个域；该份 capability 存在于仍受控的 D 时，记 `parent(controlled_domain)=D`；D 为独立根时可省略该边。已指向后来转为根的 D 的子边允许保留至自然移动/释放，不扫描重写全表。移动的是唯一 capability，不增减其 authority 计数。每个受控域最多一个父域，因此相关关系可保持为森林，不需要一般对象图 SCC。

`Q→P→Q` 同时覆盖“R 授给 Building P、B 排入 Q”的间接反例。ProcessStart 提交或进程冻结时只切断 P 的父边，整棵下属树随 P 成为独立根，无需扫描表内所有能力。Mailbox 最后 Receiver 释放时同理切断父边，已有清队列 Work 接管。

图不判断普通程序会不会主动 Receive/Close、不检测运行中进程间业务死锁，也不承诺所有存储引用环自动回收。Job/ProcessControl 的可重新派生或 Kill 权不是另一份 Builder；不能以监督者可能介入为由允许违反本图不变量的存放。内核观察来源和临时 ObjectRef 不算 Receiver authority。

类型核查边界：NotificationOwner 不容纳 capabilities，WaitSet 成员持 ObservationSource 而非 Cap。旧 ThreadStartTicket 虽仅持 Weak<Thread>，仍通过 Created 成员影响进程完成，故不能据 Weak 排除依赖；同组审查发现的反例已由用户确认的发布存活边界解决：目标中未发布线程及 ticket 不保活 Running 进程，因此不作为新的托管门。MemoryObject/Pool、资源对象、Delivery、Lifetime、Sender/Once/Observer、Tunnel 邀请等沿各自 owner 终结，不以普通强引用虚构新的接收权。ProcessControl 虽能保活 Shell，但不控制 Pending Builder 的放弃；其存储引用与托管门分开。新增能容纳 Cap 或控制容器推进的类型，必须审计对应关系，不允许默认为普通对象而漏边。

### D1.2 检测结构：确定性动态森林

不用任意长度祖先链遍历，也不把有界算法偷偷换成摊还界。采用 Euler-tour forest，以确定性 AVL 序列树保存每个连通分量的环形欧拉序列：

- 每个域有一个自标记 token；每条父边用两个方向 token。域出生时预付自己的标记及父边 token 存储；根域的父边 token 闲置。
- 节点字段为平衡树子/父指针、height、subtree size 与 token 所属域；逻辑 `parent` 及对应边 token 是本模块维护的权威关系，AVL 序列只是同锁更新的连通索引，不由业务层另存一份父链。
- `connected(a,b)` 比较平衡树根；`cut(child)` 删除其两方向边 token 并拆接序列；`link(root,parent)` 旋转欧拉序列后连接两棵树。每次只用常数个 AVL split/join 操作。
- 所有旋转、split/join 原地重用已预付 token，不在提交锁内分配，不遍历被移动的子树。实现必须采用最坏对数时间的 join/split，不能用随机 treap 或摊还 splay 替代。
- 数据结构知识由内核侧纯逻辑组件 `os/capability` 的 forest 模块拥有；内核 `task` 集成域、锁、付款和真实 Cap 位置。纯逻辑模块不认识 Process、Mailbox、Arc 或内核回调。

算法依据：[MIT 6.851 2012 Lecture 20 §3](https://courses.csail.mit.edu/6.851/spring12/scribe/L20.pdf) 说明 Euler-tour 的 cut/link/connectivity 归约为常数次平衡树 split/join；[Blelloch/Ferizovic/Sun, SPAA 2016, Just Join for Parallel Ordered Sets](https://www.cs.cmu.edu/~guyb/papers/BFS16.pdf)，DOI `10.1145/2935764.2935768` 的 AVL JOIN、SPLIT、JOIN2 及相应界用于底层序列树。以上证明数据结构复杂度，不替代本项目生命周期和并发证明。

工作界来自表示与既有 ABI：设活跃 tour token 数为 N，批量移动门控能力数为 k，则事务包括回滚为 `O(k log(N+1))`。k 受 MESSAGE_HANDLE_MAX / PROCESS_MAX_GRANTS 约束（当前 8 / 64）；N 为单个连通分量的 token 数，节点 subtree size 使用 checked usize（不设全局节点注册计数）；AVL 最大高度由位宽及最小节点递推推导，不新增可配置的 Receiver 链深限额。固定 scratch 按该高度与既有批量上限推导；不根据进程数分配递归栈，也不拿平均耗时证明不可抢占安全。实际指针操作上界和布局在实现时按唯一纯逻辑算法核定，性能尚无测量证据。

### D1.3 批量移动与失败原子性

在真正提交前，从所移动 Cap 中取得域身份；进入 forest 锁后才读取是否仍受控及旧父边，已 Start/冻结的域不重新建立控制关系。Cap 本身仍由原 Invocation/Message/准备对象唯一持有，图操作不复制 Cap。

1. 所有输入、rights、容量、实际元数据存储与目标槽先准备。
2. 取得实际目标业务锁，再取得全局 forest 锁。
3. 切断本批各受控域的旧父边，保存小型回滚记录。
4. 逐项检查新父域：若目标是独立根则只 cut、不 link；否则相同节点或仍同一连通分量即形成环，只有不连通时才 link。
5. 任一失败，逆序移除本批已加边并恢复全部旧边；不裁权、不入队、不安装 Cap，锁外由既有 Invocation 完成返还/释放。
6. 全部检查成功才裁权、安装或入队。forest 更新与真实能力提交对并发观察者属于同一临界区，不在检查后放锁再无条件提交。
7. 锁外清除源表返还预约、释放失败事件或发布来源活动，不从 forest 锁内回调对象。

批量先 cut 全部旧边再 link，避免把本批已经要移走的旧关系误判成环；此时后续只有加边，因此一旦出现连通闭环，整个最终图必非法。回滚不依赖再分配或任意重试。来源槽被 Close 撤销时不复活原句柄，继续沿既有 restore-or-release 合同。

计划新增明确 `CyclicTransfer` 错误表示本次存放会形成托管环；建议占用当前空闲错误码 `0x3e`，实际实施须再次核对共享定义。这不是缺少权限，不返回 RightsDenied；也不采用 XNU 的销毁消息并返回成功。旧 ABI 中本就可构造的 Builder 环也由同一规则拒绝，不能留一个旁路。

### D1.4 claim、接收暂摘与输出

- claim 不把域立即变成独立根：保留来源位置依赖至最终提交，现有 BodyLease 稳定来源表/域；否则一次失败 restore 可能在其他 hart 修改后重新闭环。源码中的 BodyLease 不跨等待，本候选不引入新的跨等待 Invocation。
- Receive begin 暂摘消息也保留其原队列依赖，core Arc 与接收 token 稳定旧父域。成功安装到执行进程表时再移动，失败回插不需另一次可失败的图准入；closed 后失败则移除边并释放。
- 在途 Receiver 与此前已经接受的 READ 操作可能并存。保留暂摘依赖是为保证原回滚位置合法，不把临时借用伪造为可永久依赖的根。已接受接收可能使一次潜在环短暂可被人为解开，但本案不以这种偶然并发放宽托管拓扑。
- Receive 的目标是调用中的执行进程根；成功安装时只 cut、不 link，成功写回后的安装仍可保持无失败拓扑步骤。
- ProcessGrant 输出只是目标预留槽的数值，不是能力发布。保留现有预留及有界 uaccess；在写回成功后、实际安装前做最终森林检查。若此时发现环，则撤销槽预留、返原 Cap，返回 CyclicTransfer；失败时输出缓冲无有效结果。现有逐项写回已允许出错时出现部分输出，ABI 未承诺失败不触缓冲。需删除当前“输出完成后必成”的内部注释，明确真正 commit 在最后检查之后；不在图锁中访问用户内存，不增加跨 uaccess 的图预约。

### D1.5 锁序、线性化与释放

使用一个全局 forest 同步域：它拥有真实的跨进程/跨邮箱托管关系，不是业务政策或所有 I/O 的总锁。只有含门控域的移动、域 root/retire 变化需要取得它，纯值消息和普通叶能力不必进入树。

锁序目标为已有业务锁 → FOREST →（释放 FOREST 后才发布后续维护）。FOREST rank 位于已用领域锁之后、MAINTENANCE 之前，由 `sync::ranks` 唯一定义；不得持 FOREST 再获取 CapTable、Mailbox、Shell、VM、Pool、堆锁或用户内存。rank 不能与现有 REMOTE_CALL 冲突。

| 路径 | 业务与图的线性化位置 |
|---|---|
| Send | Mailbox 状态锁内核关闭/容量/期限；随后 forest 批量核定，成功才裁权/入队；Once 提交仍与入队同一业务临界区 |
| ProcessGrant | 目标 CapTable 提交锁内再取 forest，检查后安装；表已 shut 的拒绝/释放沿原事务处理 |
| Receive 成功 | 目标表安装与 forest 移动同临界区，后续结束 receiving；不反取 source mailbox 锁 |
| ProcessStart | Job/Shell 启动提交边界把 Process 域切为根；启动 Work 与执行责任已接管，再消费 Builder |
| Kill/ThreadExit 归零导致结束 | Building 被冻结时将受控标记置 false 并 cut；Running 本来即根，冻结不增加图操作。能力表以后逐项释放 |
| Receiver 最后释放 | core 锁内关闭准入并切为维护根，再发布排空 Work |
| Discard/退出清表 | 取出有限数量 Cap，保持来源域 pin；在锁外公共释放路径拆边/触发最后释放，不持 forest 递归释放别的对象 |

forest 锁只做有界索引与字段更新。使用现有 FIFO ticket 锁，单次临界区最坏界由上节推导；没有以版本变化无界重试的路径。全局争用是否值得进一步分片必须由实际测量决定，不提前引入多图并锁协议。

### D1.6 节点存储与生命周期（同组复核后简化）

固定 token 与域状态由稳定 core/Shell 的原 Pool 按实际布局预付。森林没有全局对象强引用表，也不增加节点注册/注销状态或全局节点计数：新域天然是独立单点；所有 incident edge 切除后，不会再从其他节点的索引到达它。树指针只作索引，不通过 Arc 父链建立新的存储持有环。

两条存储不变量必须由接线保证：①最终存储析构前该域为单点，Drop 不取图锁，最多做 debug 断言；②cap 最后释放及其同步 cut 必须发生在来源域仍有真实 pin 时。现有 Invocation 的 BodyLease、Receive/Discard/rollback/drain 所持 core Arc，以及进程 WaitIngress→逐项 ReleaseHandles→DropBody 可提供此责任。`DetachedMessage { source: Arc<MailboxCore>, message }` 可将队列外持有类型化，但不需要额外 detached 计数、退图 Work 或唤醒协议。此前主代理考虑的该计数及显式退图设计撤回。

Receive 安装时的 cut 与目标表 commit 必须同锁嵌套完成；不能先公开新 cap 再迟到地无条件 cut，否则另一线程可能已经将它转移到 Q2，迟到 cut 将误删新边。以后任何不得不延迟的拆边必须核原父身份，不能根据旧快照删当前边。公共 on_last_capability 可无条件切除自己的旧父边，因为最后唯一 gate authority 已消失；cap::deliver 保持在来源 pin 消散之前同步执行，不把该 cut 延迟到来源存储可能已释放之后。

Process 已 Start 或已冻结后，其域的受控标记在 forest 同步域置为 false 并切父边；旧 Builder 仍可作为无效能力被移动，但不得重新挂父边。子边可保留至真实 Cap 移走或释放，不扫描全表。Running 表作为目标时不新增父依赖；来源及目标业务状态决定收件/安装是否仍准入，图不替代那些检查。Mailbox closed 后的排空仍按原 Work 进行；只要上述同步 cut/pin 成立，Work 在 receiving 尚存时 Idle 不会造成新的索引退休活性缺口。

### D1.7 ABI 与消费者迁移

- `Rights::TRANSFER` 使用旧 TRANSIT 位，删除两个旧名字及 GRANT 位；旧位从 KNOWN mask 移除，不保留别名或兼容解析。各对象原 TRANSIT/GRANT 的可转交意图统一到同一位，Receiver、NotificationOwner、WaitSet 的移动不赋予 DUPLICATE。
- 同步普通 Send、ProcessGrant、CapTable 测试/旧运输辅助、Packet 预检、启动 grant、对象允许矩阵、formation 权利名字和所有实际协议输出上限；共享 ABI 不冻结。不能把两个旧位都写成 TRANSFER 后保留重复配置或失效检查。
- 当前检索已见 os/handle_table 仍是被跟踪的 workspace 成员，不按旧档案“已删除”省略其实际检查/测试；本案只同步与权限迁移有关的消费者，不顺带展开无关 crate 清理。
- 不新增 pm 专用 syscall，不把来源路径/包格式/动态统计/驱动归属塞进本案。pm 通过正常 Message 收到真实能力后再 ProcessGrant，仍由内核逐跳保证权利不扩大。

### D1.8 候选比较与验证门

本方案相较局部 GC 的优势：拒绝非法存放发生于当前事务，不新增后台可达性扫描、灰白标记/写屏障或事后成功消息终结语义；已有关闭后的维护继续负责释放。代价是每个门控域固定图存储、一份跨域索引锁以及确定性动态树的实现/证明成本。相比线性链检查，避免未知长度临界区；相比会合，保持缓冲消息的现有外部含义。尚不据此宣称性能优于其他方案。

纯逻辑验证门：ETT 与独立朴素森林模型的连接/断开/连通对照；每步校验 AVL 平衡/父指针/size；批量先切再连的成功及失败回滚与最终图无环判定一致；覆盖超过任何旧任意链深阈值的链，不新增同义人工上限。内核接线验证覆盖 Send/Grant 两个方向的成环、并发候选提交、claim 撤销返还、Receive 回插/关闭、Start/Kill root 转换、节点退图后残存观察引用。测试以 host 可验证机制为边界，不建设客体 harness；真实使用和运行范围仍由用户决定。

本节已由 CoolHawk 同组核对并按下文发布边界收敛。新环拒绝结果及线程发布计数的外部后果在代码实施前明确展示；目前未改 ABI、产品代码或 impls。

### D1 同组审查：Created 线程暴露的根语义选择

CoolHawk 已返回可达反例，主会话认可其推翻“弱引用就没有推进依赖”的论证。具体证据：`task/thread.rs` 非 Building 创建会 commit Created 成员并 arm ticket；`process/shell.rs::commit_member` 对 Created 也增加 member_count；`complete_if_empty` 只在 Running 且 member_count/reserved_members 全零时终止；`process/ticket.rs` 的 Pending ticket 最后释放才 abandon_created，且 ticket 可运输、可由外部进程 Start。

- C1：P 唯一已启动线程 A 创建 B，ticket T 留在 P 自己的能力表；A 退出。P 仍有 Created 成员 B，当前保持 Running，没有线程能取 T；须外部 Kill 才收束。它根本不经过能力转交。
- C2：A 先把 T 发入 Q，R_Q 留在 P 表，再退出。T 的释放依赖 Q 排空，Q 关闭依赖 P 表释放 R_Q，P 完成又依赖 Created B 离场。
- 多 ticket 时，任意一张可由外部 Start 就能恢复执行（OR 入口），但最后一张被关闭也可能移除最后外部推进来源。此依赖可因线程退出/Close 形成，不只在可拒绝的 Send/Grant 提交时形成。
- 因此若本案要自动收束这些情况，单父森林及只在转交前拒环都不足够。不得默默禁止 ticket TRANSFER，也不得为了保住算法默认更改线程生命期。

以下是当时提交用户选择的生命周期候选；已由下一节用户确认的发布边界取代，不作为当前未决项：

| 选择 | 可观察行为 | 与 D1 的关系 |
|---|---|---|
| P1 保持当前合同 | 所有已发布进程仍作为托管根；仅剩 Created 成员时可继续存在，由持票者或显式 Kill 处理；C1/C2 不承诺自动收束 | D1 只保证 Receiver/Builder 门控存放无环；需明确把此类运行期停滞列为非目标，不能称所有含 Receiver 的责任环均已解决 |
| P2 最后已启动成员离场即结束进程 | 内核自动放弃其余 Created 成员；外部稍后 Start 的 ticket 不再能维持/恢复进程 | 森林根有更强执行依据，但改变已有跨进程启动挂起线程能力，须用户确认 |
| P3 保留外部启动机会，自动收束失去外部推进入口的部分 | 外部仍有可用 ticket 时保留；只靠内部相互持有时可被内核自动终止 | 需要多入口可达性及增量回收；也改变当前 C1 永久 Running 的可观察行为，不能称语义完全不变；具体是否分层或统一图由后续设计比较，不预先添加两套机制 |

当时主会话与 CoolHawk 均未代用户选择。CoolHawk 最初倾向 P1；用户随后明确选择发布计数模型，P1/P3 不再是当前方向。图生命周期简化已按 D1.6 回填；Grant 是否独占 claim Builder 暂不采用，以免改变同一 Builder 的并发操作能力。

辅助核查与证据边界：CoolHawk 未发现 VM quiescence 的 active/access_leases 等待 Cap 被释放或消息被接收，相关 holder 为已执行 hart 与已提交 Change/forced-MMIO Work；未重新证明全部远程 fence 与硬件退休终止性。主会话做了无文件写入的有限纯逻辑推演：1–5 个域共 1441 个森林、250908 次同目标批量移动，154165 次成环拒绝，最终图准入和精确回滚与朴素判环零差异。这只验证森林批量规则，不验证 ETT 实现、并发、内核接线，更不能覆盖/消除 C1/C2。未构建、未跑 host 产品测试或 QEMU。

此断点已由下节用户确认关闭；按发布成员模型补齐森林根的前置。产品代码施工仍未开始；当前设计与同组结构核对已按文末收口。

## 已确认的线程发布与进程执行寿命（D1 前置，同轮实施）

用户在交互问题中最初选择 P1，随后提出“创建/组装与发布分开，未发布线程只是资源”，并在进一步确认中明确采用发布边界：最后一个已发布线程退出后，即使外部仍持未发布线程的启动票据，也不得复活进程。该明确决定替换初选 P1；不建设 P3 可达性 GC。永久 Sleep/Wait 仍是已发布执行器，继续保活；未发布 Created、创建预约、外部 ticket/Control 引用都不构成执行保活。这里不按 Ready 队列是否为空判生死。

### 计数与单一真值

ShellState 新增 `published_members`（内部名字），在 Shell lifecycle 锁下拥有：仅 Running 状态表示尚未撤销的线程执行承诺。现有 `member_count`、`reserved_members`、running/ingress/receipt 等计数分别继续负责资源、操作与实际 hart 收束，不混成一个数。

| 转换 | 发布计数与行为 |
|---|---|
| 初始 Building | published_members 为 0，由 Builder 或已提交内核构造 owner 持有出生责任；没有执行器不意味着此时自动结束 |
| ProcessStart 提交 | `begin_running` 在同一 Shell 门内把状态改为 Running，并将 published_members 设为 expected_members（必须 ≥1）；初始所有成员的发布责任由 Builder Work 一次承诺 |
| Builder 分步兑现 Ready | 只兑现先前承诺，不再逐项增加 published_members；保留首个 Ready 前的 start_snapshot/tid_ceiling，避免 Running 新建成员混入初始批 |
| 运行期 ThreadStart | Shell→Members 锁序下确认 Running，Created→Ready 成功时 +1；随后预付 ReadyBatch 的提交必成，不在 +1 后增加可失败步骤 |
| ThreadExit 提交 | 当前执行 owner 在 syscall 的 ThreadExit 分支，经同一 Shell→Members 接口把当前成员置 Exiting，并在 Running 下 -1；变为 0 时同锁 freeze_locked(Completed)，锁外 finish_freeze，然后返回 Killed |
| Kill/fault/Terminate/Abort/正常最后退出冻结 | 在 freeze_locked 内将 published_members 规范为 0，与 Terminating/首次终因/FROZEN 同一线性化；之后不再逐成员减发布计数，保留既有终因 |
| Created 放弃/冻结扫描、Departure 退款 | 只处理资源离场，不修改发布计数；最后回执仍决定 Body/VM/费用何时真正退休和 DONE 何时发布 |

`ThreadExit` 是当前唯一未预先冻结的已发布执行结束入口。`sched::reap` 不设第二个递减入口，只消费仿射 AdmittedThread、取消/拆等待并交 Departure；若尚 Running，断言该成员已由执行结束接口置为 Exiting。以后新增单线程强制结束等能力，必须接入同一个执行结束合同，不在调度队列 Drop 或延迟退款处补减。

最后 ThreadExit 与外部 ThreadStart 共用 Shell 门：Start 先提交则计数增加、进程继续；退出先归零则永久冻结，迟到 Start 拒绝。退出提交在请求/等待 detach、Thread/Departure 存储退款之前，不留下以清理延迟重新开启执行的窗口。原资源回执仍等待真实释放，因此“执行终结已定”不冒充“资源已退/DONE 已发布”。

删除 `complete_if_empty` 及其在 member_departed、rollback_member 中的调用；这两处以后只归还资源回执和唤醒收束，不再决定 Running→Terminating。`begin_running(expected==0)` 保留现有 ObjectNotAvailable，不引入零线程成功发布。

### 与森林根的关系

Running 必须有至少一个已发布执行承诺，包括 Builder 尚在分步兑现的初始批；否则已由同一门进入 Terminating。故 Running 表或已冻结、已有内核回收 owner 的表都能作为独立根。ThreadStartTicket 只代表尚未发布的资源操作资格，不再保活进程，也无需成为森林的新门。C1/C2 中最后已发布线程退出立即冻结，既有回收释放 Receiver、关闭队列并释放 ticket，不再依赖 ticket 先归零。

这不是把全部用户态等待依赖纳入内核：已发布线程永久 Sleep/Wait 或应用自身协议死锁仍由用户态管理；本轮只纠正“资源成员”和“执行器”的生命周期边界。线程结果仍由用户态线程库拥有，内核不保存或搬运用户函数结果。

### 已核实的执行 owner 与锁序依据

- ready_queue::Admitted 私有拥有值及 Admission，不可 Clone；生产 ReadyQueue 的 pick/scan/replace 只搬移，不静默丢执行 owner；替换后旧存储为空。通用 ready_queue 不引入内核进程回调。
- sched::run、requeue、Wait install/resume、request::settle、Frozen Ready/Waiting 扫描的生产路径均移交队列/等待或 reap。成员表与类队列正常销毁仍要求无剩余执行 owner。
- Thread::hint 先结束 Members 操作并 drop BodyLease 再 reap；来源 Delivery.send 在来源锁外调用 HintTarget。正常 ThreadExit 在 syscall 中结束执行有 ProcessTerminate 的既有冻结路径作为接线参照，不从高阶锁内回取 Shell。
- 初始 Builder 无 ThreadStartTicket；首个 Ready 前已固定初始批。Running 时只有执行线程能调用 self ThreadCreate，所以初始 snapshot 不会混入运行期创建；实现期用断言/针对性验证核该不变量，不在锁内额外扫描整批作为生产控制流。

## D1 设计收口与实施接力

CoolHawk 在发布边界确认后完成同组最终结构核对，认可 Receiver/Builder 托管森林在本范围内闭合，未留下需要再交用户选择的架构阻塞。此为设计协作检查，不冒充提交后独立 Review。主会话已核线程初始零成员拒绝、Admitted owner/来源提示的锁外移交，并纠正过早加入的退图计数/维护协议。

当前采用的目标：统一 TRANSFER；正常 Receiver 中转不受 pm 身份特例限制；用确定性有界动态森林在 Send/Grant 提交前拒绝 Receiver/Builder 托管环；先按已确认的发布边界建立进程根；不引入全对象 GC，不改变入队即提交的消息合同。CyclicTransfer 失败返还及失败输出无有效结果须在实施前交付摘要中明确呈现。

设计确认时的自然施工顺序（下列步骤现已执行，同一责任闭包，不要求中间步骤独立交付）：

1. 发布存活边界及其完整退出/冻结/资源回执接线；保留 ProcessStart 初始批、现有 DONE 和回收阶段的责任。
2. 能力纯逻辑层的确定性 forest、原子批量 cut/link/rollback 与派生工作界；核心域/门、短锁接线及来源 pin。
3. Send、Receive、ProcessGrant、Start/Kill/Close/丢弃/回收全部位置变化接入同一规则；统一权利、共享错误和所有实际消费者，同闭包删除旧双位与旧计数判定。
4. 按影响选择 host/check/clippy，核对森林独立模型、线程发布计数竞争、正常两段Receiver移动及失败保持；impls 随代码回填。QEMU 仍须用户选范围，不建专用客体设施。

实施时的核定项不是新的架构待决：FOREST 使用唯一秩表中高于 Shell/Members、低于 MAINTENANCE 且不与 REMOTE_CALL 冲突的位置；当前候选 655，开始施工时复核所有新增调用边。纯逻辑算法必须给出实际最坏高度、固定 scratch 及批量指针操作上界，不能换成摊还实现。错误码 0x3e 开工时再次核定未占用。任何发现推翻工作上界、生命周期或根模型时，回到本篇修订，而不是为通过测试加旁路。

交付前设计检查：角色/权限/付款分离、唯一 Cap 持有、临时位置回滚、关闭/退出、锁序、有界工作、跨组件同步与旧路径删除已逐项核对；当前只有目标设计，未声称产品机制已接通。ideas 仅同步本轮明确的线稿关系，impls 保留当前代码事实。未执行产品构建、host 测试或 QEMU，未提交；本轮的有限模型推演边界见上文。设计/代码交付提交后按 REVIEW 登记固定提交的可选独立审查。

本轮文档验证：工作树内 9 份变更/新增 Markdown 的改动链接检查覆盖 22 个本地路径及 7 个标题锚点，代码围栏与 git diff --check 通过；不包含外部网站可用性验收。以上统计包含本会话此前的 pm 设计修订，未把它们作为外部改动覆盖或提交。

## 已授权实施交接（2026-11-11）

用户要求将实施交给新关联的 Panda 助手（用户称 SilverPanda，关联资源显示 SwiftPanda，资源 `6shlPRvFvwS5M6Om`）。该助手负责本闭包的实施、验证与唯一工作记录协调；主会话交接后不并行修改其代码或本记录。派发基线 `master / d0a6fd8046cd9080edcd3d3aefcdf67105108e90`；当前 8 份 tracked Markdown 改动及本篇 untracked 计划均属本会话前序设计，全部保留，不回滚，不当成已提交基线。

授权范围：上文发布存活边界、确定性托管森林、统一 TRANSFER/共享错误及实际消费者迁移，必要验证和 impls/导航/矫正区同步。不包含 pm 本体、驱动/用户服务创建归属的新功能、全对象 GC、提交、push 或 QEMU。实施者按 AGENTS 完整流程接手；host 测试显式指定 host target，构建走 just，长命令保存完整日志。

设计本身不替代代码取证。实现中若发现根模型、工作界、锁序或生命周期前提不成立，先在本篇记录并回主会话讨论，不以 adapter、任意链深限额或悄悄改变已确认语义绕过。内部结构与布局按既定契约自主落实，无需逐项重复授权。结束时报告实际改动、验证日志、未覆盖运行边界和剩余责任，不把阶段检查称为完整交付。

### D1 既定设计文档补漏（2026-11-11）

实施收口的文档核对发现，前序 D1 仅精确同步了 object/task，`notes/ideas/ipc.md:15` 与 `message.md:3/7/37` 仍保留双位运输及 Receiver 不得进消息的旧关系。短暂回到 D1 的线稿补漏整理：只将这两篇相应运输关系对齐已确认的 TRANSFER/唯一 Receiver 中转，不新增设计决定、接口细节或失败状态机，不由代码反推目标。总纲审定范围同步覆盖上述精确段落；其他未审定线稿仍受原矫正区规则约束。随后继续原实施收口。

## 实施交付与验证（2026-11-11）

### 实际实现与责任收口

接手再次实查为 `master / d0a6fd8046cd9080edcd3d3aefcdf67105108e90`，保留前序 8 份 tracked Markdown 和本计划。当前交付尚未提交，不伪称 HEAD 已包含本轮实现。主实施者统一协调三个互不重叠切面：纯 forest、发布执行寿命、ABI/用户态消费者；集成及所有文档由主实施者收口。

- `os/capability/src/forest.rs` 拥有真实 ETT/AVL 和批量 cut/link/失败恢复；kernel `task/custody.rs` 只准备门控 core/Shell pin、持唯一 FOREST 锁调用它并完成真实提交，不复制算法。Domain/DomainId 为非拥有索引，parent/controlled 唯一真值在纯层；无全局注册计数、对象强根表、detached 计数或退图 Work。
- MailboxCore/ProcessShell 按实际宿主布局付费并内嵌 Domain。Send 在 MAILBOX→FOREST 内完成准入、裁权、入队和 Once 仲裁；ProcessGrant 保留输出后最终检查，失败返回 CyclicTransfer 且原 Cap/rights 回交 Invocation；Receive 在目标表→FOREST 内 cut 与安装，无迟到拆边窗口。
- claim/暂摘仍保留原边；Invocation BodyLease、Receive/Discard/排空的 core Arc、回收 ReleaseHandles→DropBody 顺序保证父存储在同步最后事件/cut 前不消散。ProcessStart、首次冻结和 Receiver shut 同业务门内 make_root；旧子边自然消散，不扫描整表。
- 表安装补 `validate_reserved/commit_reserved` 借用预约与外层精确迭代器。首项前校验全批，实际槽公开无分配、无缓冲析构；旧 commit 复用同一核心。预约/迭代器/事件 owner 及退款都在 FOREST 外；目标 shut 保留原“消费后释放、不公开新槽”的结果，不误改为源能力复活。
- Shell 的 published_members 只在 begin_running 一次初值、运行期 Start 成功、ThreadExit 执行结束和 freeze 清零处写入。最后 Exit 同门冻结，reap 不补扣，资源回执仍等待真实释放；complete_if_empty 及资源退款保活判定已删除。
- TRANSFER 使用 bit 4；旧双名、旧 bit 8 的 KNOWN 接受及旧组态名字均删除。共享 CyclicTransfer 为 0x3e，核对了手写/派生数值转换。所有内核允许矩阵、bootstrap、handle_table、libkernel Packet、formation/system.toml、实际库/服务/驱动同步；唯一 Receiver/Owner/WaitSet 未因可转交而获得 DUPLICATE。
- 当前 impls 回填源码状态。D1 线稿补漏仅覆盖上节精确句段，未扩大系统设计；两项已修复的专属矫正区删除，全项目重审/未审定线稿等既有矫正范围保留。

### 同组实现检查与修复

1. 主实施者在集成前发现旧 `CapTable::commit` 会在公开时析构预付 releases/输入容器；以借用安装核心消除图锁内 Pool/heap 反取，而非缩短图锁制造检查/安装窗口。4 项新 table host 用例验证成功时付款 owner 不消散、全批预约失效先失败、shut/数量错误不消费以及空批次。
2. 主实施者发现永久根可保留子边的恢复反例：旧 `3→2`、2 已 make_root，前项 `3→4` 成功、后项 `4→4` 成环；普通 link 的 Inactive 拒绝不能用于原边恢复。已在纯层 batch 内以私有拓扑核心精确恢复本次保存边，普通 link 继续拒绝 Inactive；不改 controlled、不公开恢复旁路、不扫描子树。该真实 API 反例及 64 项晚期失败均通过。
3. `sub-5` 对 kernel 集成、Shell/成员/执行 owner、来源 pin、安装与退款边界作只读结构核对；`sub-6` 对最终 ETT 切分、AVL JOIN/SPLIT 最坏界、checked 容量/高度、unsafe/析构及真实 batch 薄接线作独立角度核对，均无可行动 findings。此为本次交付前结构检查，不冒充固定提交的提交后 Review。最后 Grant 清理进一步区分已安装与 shut：成功直接销毁预约，shut 才 rollback，不依赖对已安装槽 rollback 的空操作。
4. 最终简化核对：没有增加第二份发布计数、一般对象 GC、回滚 Drop 协议、额外退休计数或 pm 特例。保留一份 parent/controlled、一份批量算法及一份槽安装核心；授权、执行责任、存储 pin 与费用各有独立 owner。

### 产品验证与日志

所有 host 路线经 `just test-host` 显式指定 `aarch64-apple-darwin`，不继承 RISC-V target。实际工具链 `rustc 1.101.0-nightly (19833af72 2026-10-20)`、LLVM 23.1.4。

| 最终检查 | 退出码 / 日志与边界 |
|---|---|
| `just check kernel` | 0；`artifacts/checks/20261111T080117.967627Z-60962/kernel.log` |
| `just clippy kernel` | 0；`artifacts/checks/20261111T080118.784387Z-60997/kernel.log`；无 Rust 警告 |
| `just check capability` / `just clippy capability` | 0 / 0；`artifacts/checks/20261111T075430.760360Z-59824/capability.log`、`artifacts/checks/20261111T075643.169808Z-60258/capability.log` |
| `just test-host capability` | 0；`artifacts/checks/20261111T080119.390254Z-61035/capability.log`，34 项；含 8 项 forest 及 4 项借用安装新增契约用例 |
| 共享、formation、handle_table 与真实用户态消费者 | 最终均 0；完整逐项命令、日志和首错处置见 `artifacts/checks/transfer-abi/evidence.txt`。包含 `check user`、`clippy user`、erhino_shared/formation/handle_table clippy 和 host，以及 libkernel/libprocess(management)/libservice/libfs/libdevice/liboal/libpci/libdriver/librpc/libhid/libusb/srv_fs(lib) host |
| `just build_kernel`（debug / virt，仅构建） | 0；`artifacts/checks/transfer-kernel/build-kernel-final.log`；LP64 soft-float、FP 局限区及单帧 guard 审计通过；链接器报告 LOAD RWX 警告，未把它隐去或称成 Rust 检查失败 |
| 最终格式、旧名/重复权利与文档链接 | 0；`artifacts/checks/transfer-kernel/final-structure.log`，87 份 Rust 格式、43 个变更本地路径及 14 个标题锚点、代码围栏及 diff 检查通过；不验证外部网站可用性 |

forest host 直接调用产品 API，对 1–5 域共 1441 个森林检查独立朴素父数组模型及每步 AVL 父指针/height/size/有向 tour；对 1–4 域 9852 次 batch 比较最终准入及精确回滚；覆盖 16384 域长链、64 项晚期失败、永久根保留旧子边、空批次和 checked 算术边界。没有新增逻辑深度限额或 guest harness。host 模型不冒充真实内核并发交错或跨进程运行。

保留的开发失败：发布切面初次 kernel check 为 101（`20261111T074717.309293Z-57778/kernel.log`），属于共享 ABI 已迁移、kernel 尚未统一及 custody 未接通的中间态，最终组合检查已过；forest 早期缺测试文件/测试 helper 未用参数已修正。ABI 切面 libfs/formation 中间 host 失败、user-host clippy 的 host/bin 选面失败及替代证据详 `transfer-abi/evidence.txt`，未靠旧名 alias、错误预期、工具改造或 guest 特例使其通过。上述日志均保留，不将“局部未见新诊断”当成通过证据。最终格式首查还发现 3 处 rights 合并后的换行差异（`artifacts/checks/transfer-kernel/structure-initial.log`），仅 rustfmt 后复查通过并重新生成最终 ELF，未修改行为或重复运行无关测试。

### 工作界、实际布局与 ELF 栈证据

- RV64 的 AVL 最高可表示高度 H=91，来自最小节点递推，不是配置。每域三 token 的纯布局，经最终 debug ELF 离线 GDB 查询为 Domain=136B、DomainId=8B：`artifacts/checks/transfer-kernel/domain-layout-final.log`，退出 0；没有连接 target。
- split/join 固定 scratch 数组共 1547B，64 项事务记录 1024B。源码给出单个 link/cut 的宽松 token 访问证书 `8192*(H+1)`，失败批次每项至多两次 cut、两次 link 加固定扫描；这些是最坏结构界，不是性能测量。
- 最终 ELF SHA256：`8a4e32a739079b2111d4d6ffa0081960587e6b7e69491427d2dbce845b87a01b`。固定反汇编逐函数 sp 扫描在 `artifacts/checks/transfer-kernel/frames-final.json`：forest transfer64=2064B、join=1104B、split=1200B，Grant=5760B、finish_receive=3568B，custody wrapper 约 1.1KiB。链接常量 STACK_SIZE=262144B、EMERGENCY_SIZE=4096B、STACK_GUARD=12288B；全 ELF 最大单帧为 bootstrap 的 0x2c20，构建审计通过。
- 单帧扫描不是全调用图累计栈证明；这里记录无递归算法的固定 scratch 及实际编译帧，不借旧固定 ELF 证书证明当前版本，也不扩展到全内核新的累计机器证明或运行延迟承诺。

### 完成门与剩余边界

| 门 | 核对 |
|---|---|
| 目标/非目标清楚 | 统一转交、仿射门控无环与发布执行寿命已接通；无全对象 GC、业务死锁检测、pm 新功能 |
| 必要前置及本闭包消费者 | 发布根前置、Send/Receive/Grant/Start/Kill/Close/丢弃/回收、ABI两侧与正式组态均同步；旧双位和资源回执保活路径删除 |
| 正常/失败/取消/退出/超时/退款 owner | 入队即提交，未提交失败保原能力；原 Deadline 和 Once/Delivery/Lifetime 责任不变；来源 pin 内同步 cut，既有维护收束已提交关闭；费用随真实存储 |
| 验证/文档/结构 | 上述 host、静态、构建 ELF 和两角度结构检查完成，impls 如实回填；未把测试或构建称作正式运行 |
| 延期不破坏当前合同 | 没有暂留本闭包旧路径、adapter 或未闭合正确性责任；以下是明确非目标与证据边界，不是偷偷保留当前错误 |

本案不启动 QEMU，因而没有真实“请求方→pm→目标”运行、实际硬件、精确跨 hart 故障交错或延迟证据。当前机制允许正常两段 Receiver 移动，pm 接受真实能力并代理出生、返回对象权利和驱动/服务创建归属的产品设计仍由[服务装配计划](../todo-2026-10-07-service-assembly.md#当前接力)拥有；接通时复用本契约，不重造运输旁路。更广不可达能力环与空槽性能等既有条件后续继续由[重构总纲](../todo-2026-09-29-system-redesign.md#不可达能力环的范围边界)拥有，不扩大本案自动退款保证。

本闭包完成归档；后续可接 pm 设计/正式消费者，但不因此自动授权运行或新增功能。提交、push、QEMU 均未执行。若获得提交授权，提交后再按 REVIEW 登记固定提交的可选独立 Review，本次未预造虚构提交号。

### 提交前整理（2026-11-11）

用户在上述交付后授权整理代码、更新文档并提交；不含 push 或 QEMU。最终复核聚焦纯层批量恢复、Gates 的存储 pin、Grant/Receive 借用安装及锁外析构、发布成员计数与 reap，不另增结构或改变产品代码。已有同组结构检查与产品验证继续有效，本轮不是固定提交的独立 Review。

文档整理补明 ProcessGrant 写回后仍可因成环失败、输出无效及原能力返还；服务装配计划清除“所有领域必须经 pm”、图标/路径属于首轮及“先实施已完成 IF2”的残留措辞。普通应用 pm 范围、驱动/用户服务创建归属未决和 TRANSFER 已交付前置分别保留，不扩张功能承诺。

`artifacts/checks/transfer-kernel/precommit-structure.log` 退出 0：54 个变更/新增本地链接、21 个标题锚点、diff 空白及旧权利符号检查通过。整理只改说明与接力，不重复已通过的产品检查。提交后的实际固定基线与可选审查入口由登记记录承载，避免在本提交内预填自身哈希。
