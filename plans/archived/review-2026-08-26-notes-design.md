# notes 设计审查

## 范围与判定原则

本审查覆盖 `notes/ideas/` 与 `notes/impls/`，目标是确认概念模型、系统边界与跨域契约，不以寻找普通代码缺陷为主。代码只用于区分“方向超前”与“实现记录陈旧”；硬件与 ABI 结论以 `references/INDEX.md` 所列固定规范为依据。

判定遵循项目既定方向：协作式微内核、内核短路径、长工作进入用户态服务、`shared/` 是内核与用户态 ABI、结构先完整而实现可从简。

## 总体结论

现有设计主干方向正确，尤其对象、Handle、消息事务、等待电平、Tunnel/Runnel 与执行环境已经形成较成熟的局部模型。主要缺陷不在单个机制，而在跨域组合后缺少闭合契约：平台根权利来源、进程管理与多线程终止、用户态服务 capability 的表示、正常完成与异常断开的区分，以及文档层级归属。

继续扩展设备、FAL、ThreadSpawn 或公开 ProcessCreate 前，应先完成上述骨架。

## 正确且应保留的设计

- 进程本地 Handle、generation 防陈旧、零值无效、回绕槽退休。
- Handle entry 中 object、lifecycle role 与 rights 正交；rights 只能单调收窄。
- 引用存活、对象逻辑终态和物理资源回收分离。
- Send/Receive 与 Handle move 采用全有或全无事务；失败不产生半安装。
- IPC 分为消息控制面、ObjectSignals/Notification 事件面、Tunnel 数据面。
- ObjectSignals 是非消费式电平，WaitMany 不清位；Notification 由 Take 显式消费。
- Tunnel Invitation/Endpoint 的线性参与方关系，无 PID 寻址、全局 ID 或永久 registry。
- 共享内存协议采用固定布局、单写者、Acquire/Release、shadow 校验、门铃仅为 hint、永久 Broken。
- bootstrap 与正式 hart 分离；HartId、HartSlot、拓扑、硬件 capability 和调度域不混用。
- 调度域—调度类—执行点三层结构与线程单一容器归属。
- FAL 私有命名空间、最长前缀路由、客户端符号链接展开与提供者直连。

## 已确认的架构决策

### 纯 capability 授权模型

系统是嵌入式单用户环境，不建立内核 uid/gid、用户组或进程权限等级。资源授权只来自 capability：

```text
Handle = 进程本地名字
Capability entry = Object + lifecycle role + rights + immutable badge
```

PID 只表示 provenance、诊断与创建关系，不授权操作。ELF ISA 要求称为执行需求，hart 匹配称为调度资格，Job 配额称为资源预算，均不得再使用“进程权限”等含混词汇。

内核依据可信平台事实铸造 root Job、MMIO、IRQ、DMA 等平台根 capability；init/resource manager 决定分发策略。内核是技术铸造者，不是策略授予者。因此最终 init 必须获得平台根 capability，不能在承担系统授权的同时以零 Handle 启动。

若未来增加多用户，由用户态认证服务在 grant 铸造阶段提供 principal/ACL；已铸造 capability 仍是操作热路径的授权真值，内核无需认识用户账户。

### `parent_pid` 的语义

保留 `parent_pid` 这一易理解名称，但它仅表示“由哪个进程创建”，用于诊断与关系展示：

- 不产生管理、继承或回收权；
- 不决定 rights 收窄或 Handle 转移；
- Process Controller capability 才表示管理权；
- Unix 式 wait/reap 或进程树政策由用户态 pm 协议维护。

### 通用 StartupBlock 外层

内核理解统一运输外层，payload 对内核完全不透明：

```text
StartupBlock
├── Header
│   ├── magic / version / total length
│   ├── pid / parent_pid
│   ├── handle_count
│   └── payload offset / length
├── Handles[]       内核实际安装的 child-local Handle 值
└── Payload[]       launcher 与 child 自行解释
```

普通服务可把 Payload 定义为 `LauncherParcel`；init 可使用 InitParcel 或 initfs；其他程序可采用不同格式。Handle 的业务 tag 与 payload 关联属于内层协议，以外层 Handle 数组索引表达，内核不解释。

ProcessStart 的提交顺序必须是：为 child 预留真实 Handle → 构造外层 → 只读映射 → 提交 Handle → 设置 a0/a1 → 首次发布 runnable。失败必须回滚全部临时值、映射、Handle 与进程可见性。不得再从数组下标推导固定 slot/generation。

### Handle 的两种运输拓扑

有缓冲消息会把 capability 暂存在内核对象图中；ProcessStart 的直接 grant 不进入对象容器。二者使用正交 rights：

- `TRANSIT`：允许 entry 进入 mailbox message，随后由接收方安装；
- `GRANT`：允许 ProcessStart 等直接跨 HandleTable 安装。

Send 只检查 `TRANSIT`，ProcessStart 只检查 `GRANT`，均不按对象类型特判。

唯一 owner 可持 `GRANT` 而不持 `TRANSIT`：launcher 可在 child 首次运行前预配置接收端，但 owner 不会进入有缓冲对象图。sender、signaler、send-once 和 invitation 可按授权获得 TRANSIT/GRANT；VM-bound Endpoint 两者均无。

分离的原因是防止 owner 被排入消息后形成不可达 capability 环。Mach/XNU 选择维护 in-space/in-transit destination chain，并在 Send 慢路径用全局链锁执行 O(chain length) circularity 检查；eRhino 选择显式区分运输拓扑，以保持内核 Send 路径固定且短。

### Badged mailbox sender

Mailbox owner 可铸造带不可变 `u64 badge` 的 sender。badge 属于 capability entry，duplicate、TRANSIT、GRANT 与 send-once 派生都保持原 badge。初始普通 sender 使用 badge 0。

接收侧 MessageHeader 由内核填写：

```text
sender_pid    provenance / 审计，不是授权
sender_badge  目标 sender capability 的授权上下文
```

服务端以 badge 索引用户态 GrantState。badge 数值本身不是 bearer token；authority 来自不可伪造的 sender Handle。该机制统一承载 DirectoryGrant、服务 session、设备 lease 等用户态对象引用。

### DirectoryGrant

“Directory Handle”统一称为 `DirectoryGrant`。它不是内核 Directory 对象，而是用户态 FAL capability：允许持有者从 provider 内某个根节点开始，以给定操作上限解析相对路径。

推荐表示为 badged mailbox sender：

```text
badge -> { provider root node, FAL rights ceiling, lifecycle/revocation state }
```

进程 namespace 保存 `prefix -> DirectoryGrant`。路径只是 grant 内的名字，不携带 authority；`..` 钳在 grant 根。子树或更窄 rights 由 provider 执行 DeriveGrant 并铸造新 badge。

内核 Handle rights 只控制 Send/Wait/Duplicate/TRANSIT/GRANT；Traverse、Enumerate、Create、Remove、Read、Write 等属于 FAL grant rights，不进入内核。

### launcher 预配置服务接收端

默认组装方式是 launcher 创建 mailbox、保留 sender，并通过 ProcessStart 的直接 GRANT 把 affine owner 安装进 Building child。child 首次运行即可 Receive。服务仍可选择启动后自行创建并发布 mailbox，但不强制引入 activation handshake。

### send-once

send-once 表示最多一次成功投递。若同一 send-once 同时作为 Send target 与 transit move，第一次投递后 capability 会继续存在于消息中，实际产生两次成功投递，违反授权语义。Send 必须在任何入队或摘除前整体拒绝 target/transit alias，失败不消费。

### FAL watch

当前“provider 把 Notification signaler 给客户端，客户端 Wait/Take，多个客户端共享广播”不成立：signaler 是提交侧，共享 Take 是竞争消费而非广播。

方向改为每订阅者一份 Notification：客户端保留 owner/read 侧，把 signaler TRANSIT 给 provider；关闭 owner 即取消订阅。真正共享广播若需要，应使用版本状态或消息流。

## 仍需设计或延期的问题

### 进程管理与多线程终止

需补 Job/Process/Thread 的完整模型：Building/Running/Terminating/Dead、Controller/Observer capability、线程成员关系、Ready/Waiting/Running 撤销、active-hart 集合、地址空间失效与回收屏障。

ThreadSpawn 前必须完成。RISC-V 特权规范明确 SFENCE.VMA 只影响本 hart；同一地址空间并发运行时，即使 ASID 恒 0，unmap/protection change 也需要远端失效与确认。

### 资源预算

邮箱容量只解决局部背压，不能防止大量对象、排队消息、Handle、Tunnel 和等待订阅耗尽内核资源。需引入分层 Job/process 预算、费用归属和 capability 移动时的记账规则。

### RPC deadline/cancel

关闭 ReplyPort 只表示调用方停止等待，不能撤销已执行请求。超时结果必须定义为“是否执行未知”；重试依赖协议幂等或去重。内核 ABI 宜采用绝对单调 deadline，用户库可接收相对 duration。Cancel 是用户态协作协议，不应伪装成内核强取消。

### Runnel 正常结束

EOF 是页内正常终态，但随后 Endpoint close 会令对端观察 PEER_CLOSED 并进入 Broken。需选择消费者完成确认、上层控制面确认，或明确某些已完成状态下的关闭属于正常收尾。

### 服务目录生命周期

需定义 Absent → Starting → Ready(instance, protocol, endpoint) → Draining → Absent；service record 必须原子发布。客户端取得旧 CLOSED endpoint 后可重新发现，但是否重试取决于业务幂等语义。

### 静默停机政策

当前自动静默关机语义明确，但需决定它是生产系统政策还是集成验证政策。若保留，所有有效 timer、设备源、remote request 与外部唤醒源必须进入静默谓词。

### Tunnel extent

方向层需统一“一页机制”与“可变连续区间”的表述。Runnel v1 可保持单页；未来 Tunnel extent 应优先保证虚拟连续，不把协议扩展绑定到物理连续大块分配。

## 文档结构问题

- 协作式内核、执行环境、调度域、内存模式等方向性结论目前主要藏在 impls，应补 idea 层唯一归属。
- ideas 中的“当前实现”“尚未接管”“目前未实现”等施工状态应移至 impls/plans。
- impls 中的历史选型、施工顺序和未实施方向应移至 review/todo/ideas。
- wait/signal、fs/fal 重复定义同一不变量，已出现 Deadline 术语漂移。
- `notes/README.md` 的 RPC 索引重复且主题映射不准确。
- `ideas/device.md` 与 capability/FAL/Notification 方向冲突，需整体重写。
- `ideas/ecs.md` 仍是应用构想，应降到 robotics/application 子域或 plans，避免与内核核心架构同级。

## 建议后续顺序

1. 完成 StartupBlock、TRANSIT/GRANT、badged sender、send-once 的 ABI 基座。
2. 真实集成验证后一次性修订 object/message/startup/task 等 ideas 与对应 impls。
3. 落地 DirectoryGrant、watch 与服务目录状态机。
4. 设计并实现 Job/Process 生命周期与平台根 capability。
5. 在 ThreadSpawn、设备对象和跨进程 FAL 前分别复审剩余专题。
