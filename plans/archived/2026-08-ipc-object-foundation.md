# todo：IPC 对象 / Handle 重建

状态：**已完成**。本计划承接已完成的旧 IPC 三面审查，整体替换旧 ABI、PID 消息寻址、ObjectKind/id 等平行寻址、全局 tunnel id、阻塞 Receive 与隐式信号消费；不保留兼容入口。旧实现审查结论见 [review-2026-08-ipc.md](review-2026-08-ipc.md)。本任务以实现完成和既定测试通过为结束，不另建新实现 review；统一 review 留待后续规划节点。

契约基准：`notes/ideas/{object,wait,shared-memory,ipc,message,signal,tunnel,runnel,service}.md`。实现 RISC-V 内存序时须从 `references/INDEX.md` 的 `rvwmo.adoc` 引用对应条款。

## 目标与边界

- 任意跨进程资源只经本地 Handle 和裁剪 rights 引用；Handle move、Receive 与 attach 均为可回滚事务。
- 唯一阻塞入口是 WaitMany；Sleep 仅是使用同一 Context 与期限源的便利调用。
- 消息、对象状态、隧道三面职责分离；共享页按 Acquire/Release 正确运行于 RVWMO。
- 不引入通用 capability 撤销树、完整服务目录、多页共享对象或持久化共享内存格式。FAL、路径和挂载全在用户态服务。

## 冻结 ABI

所有 ABI 整数是固定宽度 little-endian，结构 `repr(C)` 且 8 字节对齐。调用者把所有 reserved 置零，内核验证其为零并写零；否则 `IllegalArgument`。`a7` 是调用号，`a0` 是返回错误码；以下 `a0–a5` 为输入，未列寄存器必须为零。所有用户指针输出在成功时才可解释。

```text
Handle         = u64    // 高 32 generation，低 32 slot；0 无效
Rights         = u64    // READ/WRITE/WAIT/SIGNAL/TRANSFER/DUPLICATE/MANAGE/MAP
ObjectSignals  = u64
WaitCookie     = u64
ProcessId      = u64    // 本次启动单调不复用；0 为内核身份

SendHeader { kind: u64, payload_len: u32, handle_count: u32, reserved: [u64; 5] }       // 56 B
MessageHeader { sender: ProcessId, kind: u64, payload_len: u32, handle_count: u32,
                reserved: [u64; 5] }                                                     // 64 B
HandleMove { handle: Handle, rights: Rights }                                             // 16 B
WaitItem { handle: Handle, signals: ObjectSignals, cookie: WaitCookie, reserved: u64 }  // 32 B
WaitResult { cookie: WaitCookie, observed: ObjectSignals, item_index: u32,
             reason: u32, reserved: u64 }                                                // 32 B
StartupHeader { version: u16, kind: u16, grant_count: u32, reserved: [u64; 2] }          // 24 B
```

`WaitResult.reason` 为 `SIGNALED`、`CLOSED` 或 `CANCELLED`。`READABLE=bit0`、`WRITABLE=bit1`、`DATA=bit2`、`PEER_CLOSED=bit62`、`CLOSED=bit63`；对象只能公开合法子集。首期上限为 payload 4096 B、转入 Handle 8、邮箱 16 条、WaitMany 64 项。

| 调用号 | 调用 | `a0–a5` |
|---|---|---|
| `0x18` | `StartupMailbox` | reserved、reserved、reserved、reserved、reserved、reserved；临时返回 bootstrap owner Handle |
| `0x25` | `Sleep` | 期限、reserved、reserved、reserved、reserved、reserved |
| `0x30` | `HandleClose` | handle、reserved、reserved、reserved、reserved、reserved |
| `0x31` | `HandleDuplicate` | source、rights、结果 Handle 指针、reserved、reserved、reserved |
| `0x32` | `WaitMany` | `WaitItem` 指针、项数、`WaitResult` 指针、reserved、reserved、reserved |
| `0x33` | `NotificationCreate` | owner rights、signaler rights、`HandlePair` 输出、reserved、reserved、reserved |
| `0x34` | `NotificationSignal` | signaler、bits、reserved、reserved、reserved、reserved |
| `0x35` | `NotificationTake` | notification、mask、taken-bits 输出、reserved、reserved、reserved |
| `0x40` | `MailboxCreate` | owner rights、sender rights、`HandlePair` 输出、reserved、reserved、reserved |
| `0x41` | `Send` | mailbox、`SendHeader` 指针、payload 指针、`HandleMove` 指针、move 数、payload 长度 |
| `0x42` | `Peek` | mailbox、`MessageHeader` 输出、reserved、reserved、reserved、reserved |
| `0x43` | `Receive` | mailbox、`MessageHeader` 输出、payload 输出、payload 容量、Handle 输出、Handle 容量 |
| `0x44` | `Discard` | mailbox、reserved、reserved、reserved、reserved、reserved |
| `0x60` | `TunnelCreate` | VA、Endpoint 输出、invitation 输出、reserved、reserved、reserved |
| `0x61` | `TunnelAttach` | invitation、VA、Endpoint 输出、reserved、reserved、reserved |
| `0x63` | `TunnelNotify` | Endpoint、reserved、reserved、reserved、reserved、reserved |
| `0x64` | `TunnelAcknowledgeData` | Endpoint、reserved、reserved、reserved、reserved、reserved |

本阶段的内核 launch primitive 在新进程 runnable 前直接安装 bootstrap receiver，并投递版本化 `STARTUP` 消息；入口保持 `a0=pid`、`a1=parent_pid`，rinlib 通过临时 `StartupMailbox` 查询已安装的 owner Handle。该 syscall 只服务本次迁移，不是最终对象模型：后续必须以通用的 args/startup-resource 枚举取代，使 Mailbox 与其他 tagged grants 一样成为可选、主动探索的附属资源，不占据固定入口寄存器或特殊槽位。直接安装是建立根 Handle 图的唯一例外，普通发送永不接受 PID。公开 `ProcessCreate`/`ProcessStart` 留到 init/进程服务里程碑定稿，但必须复用同一资源授予和 `STARTUP` 契约，不能恢复 PID 消息寻址。

新增错误：`ObjectBusy`、`BufferTooSmall`、`ObjectClosed`、`RightsDenied`、`WrongObjectType`、`StaleHandle`。保留 `MailboxFull` 与 `ObjectNotAvailable` 的非阻塞语义。`SendHeader.payload_len` 必须等于 `a5`，`handle_count` 必须等于 `a4`；输入 signals 含对象不支持的位、rights 含角色不允许的位或任何 reserved 非零均返回 `IllegalArgument`。

## 结构边界

| 边界 | 职责 | 位置 |
|---|---|---|
| shared ABI | Handle、rights、signals、消息、等待、调用号和错误 | `shared/src/{object,message,wait,call}.rs` |
| 内核对象 | 类型、终态、强引用、lifecycle role | `os/kernel/src/task/object.rs` |
| Handle 表 | generation、永久退休、授权、预留、move/duplicate/drain | `os/kernel/src/task/handle.rs` |
| 等待 | WaitContext、订阅、期限、取消、完成仲裁 | `os/kernel/src/task/wait.rs` |
| 邮箱 | FIFO、transit Handle、READABLE、接收事务 | `os/kernel/src/task/mailbox.rs` |
| 隧道 | Connection、Endpoint、Invitation、VM reservation、帧 | `os/kernel/src/task/tunnel.rs` |
| syscall | ABI 拷贝、空间锁、异步出口与分发 | `os/kernel/src/syscall.rs` |
| 用户态 | 类型化 Handle、RAII、消息/等待/隧道封装 | `user/rinlib/src/ipc/` |
| Runnel | 布局、原子访问、Broken、阻塞循环 | `user/frameworks/librunnel/src/lib.rs` |

## Handle、邮箱与消息事务

Handle 槽位含 generation；查找同时验证 generation、类型、role 和 rights。每个 syscall 在解析时取得对象强引用，并把本次已验证的授权保留到操作完成，不能在随后复查可变 Handle 槽位。generation 将回绕的槽位永久退休。Handle 从表中移除后先释放表锁，再执行 role 的关闭动作，禁止对象回调反向进入 HandleTable。

角色允许的最大 rights 固定如下；创建、复制和转移请求只能取其子集：

| role | 最大 rights |
|---|---|
| Mailbox owner | `READ | WAIT | MANAGE` |
| Mailbox sender | `WRITE | WAIT | TRANSFER | DUPLICATE` |
| Notification owner | `READ | WAIT | MANAGE` |
| Notification signaler | `SIGNAL | WAIT | TRANSFER | DUPLICATE` |
| Tunnel Endpoint | `WAIT | SIGNAL | MANAGE` |
| Tunnel invitation | `MAP | TRANSFER` |

Endpoint、invitation、Mailbox owner 与 Notification owner 均不可 duplicate；Endpoint 与 owner 不可 transfer。`HandleClose` 是释放这些 lease 的唯一通用入口，不另设 TunnelClose。

固定锁序为 **HandleTable → Mailbox**。Send 先在 space 锁保护下复制并交叉校验 `SendHeader`、payload、`HandleMove[]`；拒绝重复源项及 rights 放大。随后同时持有发送方 HandleTable 与目标 Mailbox，在不会再失败的临界区确认容量、移除全部源项并发布完整消息。目标 sender Handle 即使也在 move 列表中，已解析的目标引用仍使本次 Send 有效。不得同时锁两个进程 HandleTable。

Receive 先按固定顺序锁接收方 HandleTable 再锁 Mailbox，检查队头、一次性预留所需 slots 并把队头标为 `receiving(token)`；竞争 Receive 或 Discard 返回 `ObjectBusy`，不得越过保留队头。释放两锁后，在接收方 space 锁保护下校验并复制完整输出。随后再次按 HandleTable → Mailbox 提交全部安装并删除队头，或 rollback slots 与 token；RAII reservation 保证所有提前退出都回滚。失败输出不可解释且队头保留。执行 syscall 的线程退出临界区前，进程回收不得拆其地址空间或 HandleTable。

## WaitContext

syscall 一次复制和验证最多 64 个 WaitItem，取得每项对象强引用及 `WAIT` 授权并读取初始信号；若初始检查已有命中，取最小 index 同步返回。否则形成 park intent。线程 handoff 以 `Running → ParkIntent → Publishing(Context) → Waiting/Completed` 单向迁移；kill/exit 可在 ParkIntent 前抢先标记 `Abandoned`，也可在 Context 建立后竞争其 outcome。调度器只有在 CAS 取得 Publishing 所有权且线程已离开执行点后才能创建 `Installing` Context。

Installing 期间外部发布者只能竞争写单一 outcome，不能触达线程、清理订阅或继续登记。安装者在每次登记前后检查 outcome；一旦已有结果即停止新增，并作为 Installing 唯一清理者撤销已登记项。全部登记与重查完成后，arm 要么由安装者交付已有 outcome，要么转为 Armed 并把清理权交给未来赢家。Armed 后的状态、关闭、期限、取消与退出以 outcome CAS 竞争；跨对象并发结果由先成功者决定，同一对象一次更新命中多项则取最小 index。

对象状态更新先在线性化点改变电平，再向全部匹配订阅 offer；电平条件不能只唤醒一个观察者。对象订阅数有明确额度，安装超额时整次 WaitMany 回滚并返回 `ReachLimit`，从而给协作式投递路径提供边界。

完成者先取走线程所有权，再释放触发对象锁，逐对象、一次一锁地摘订阅，绝不同时持两个对象锁；随后写普通结果并入队恰一次。普通取消返回 `CANCELLED`；期限是内部完成原因，Sleep 把期限完成翻译为成功；kill/exit 的 `Abandoned` 只清理、不返回用户态。

## 隧道与 Runnel

TunnelCreate 建立 A-alive/B-invitation Connection 和 object-owned VM reservation。invitation drop 通知 A；A 先 close 令 invitation `CLOSED`，attach 失败；attach 先赢后 B-alive，A close 通知 B。Attach 先预留 Handle slot、VA 和输出；以 Connection 锁线性化 consume/close，失败全 rollback，提交后无失败步骤。本端 lease close 撤销本端映射，双方 closed 才还帧。DATA 只能由专用 `TunnelAcknowledgeData` 确认。

Runnel 创建者写零保留区及初始字段后，最后以 release `MAGIC.store(0x314C4E52)` 发布；attach 先 acquire MAGIC。同版本读取者忽略保留内容。EOF 检查先 acquire eof，若为一再 acquire head 并判空。双方以 shadow cursor 验证对端推进量不超过此前可合法推进范围、`used <= CAP` 与每次复制 `n <= CAP`。会阻塞的封装在写入、腾空或 EOF 正进展后于等待或返回前通知对端；纯轮询模式可省略门铃。实现与审查逐条引用 RVWMO 规范。

## 纵向迁移与验收

每一阶段必须形成可构建的纵向切换：shared、kernel、rinlib 和受影响服务在同一变更删除旧入口，而非长期双栈。

1. **内部地基**：先落地 shared 新类型、Object/HandleTable、WaitContext 与事务守卫，但不暴露第二套可用用户 ABI；旧纵向路径在此期间保持可构建。
2. **控制面纵向切换**：在一个可构建变更中同时切换 shared 调用号、Mailbox/Notification/Wait、bootstrap launch、syscall、rinlib 和服务；随后删除 PID Send、ObjectKind/id Wait、阻塞 Receive、隐式邮箱与旧 signal ABI。
3. **数据面纵向切换**：在一个可构建变更中同时切换 Connection/Endpoint/Invitation、object-owned VM reservation、rinlib tunnel、librunnel 与服务；随后删除 tunnel registry/id 和旧 Runnel 访问层。
4. **验证与收口**：迁移 Sleep 的内部完成源，补齐压力测试与实现记录，清除旧 ABI、测试和文档残留。未来 init 接管启动时删除 kernel launch policy；通用 startup-resource 枚举替换临时 `StartupMailbox`，但不改变 Handle、消息或对象 ABI。

| 类别 | 验收 |
|---|---|
| ABI | 尺寸、对齐、reserved、字节序、调用号和 `a0–a5` 边界 |
| Handle | generation 防陈旧、回绕退休、rights/role、duplicate/move rollback、drain |
| 消息 | 满箱不 move、长度交叉校验、Receive Busy/BufferTooSmall、不出队、输出区并发 |
| 等待 | 三窗口、Installing outcome、arm、重复项、关闭、取消/期限/exit 竞态、线程不滞留 |
| 隧道 | 单次 attach、attach/close 竞态、reservation、双端关闭、帧守恒、PEER_CLOSED |
| Runnel | 初始化锚、回绕、满空、EOF acquire 顺序、Broken、双线程和双 hart 压测 |
| 启动 | loader `STARTUP`、grants rights、未知服务拒绝、临时查询不占入口寄存器；通用资源枚举列为后续任务 |
| 集成 | 消息与隧道风暴、QEMU 多核重复运行、RVWMO 条款审查记录 |

完成标准：仓库仅暴露新 Handle/对象 ABI；所有对象关闭完成相关 WaitMany；消息、Receive、attach 的事务由单元与竞态测试覆盖；对象、Handle 与帧计数在压力前后守恒；Runnel host/SMP 与 RISC-V 集成验证通过；ideas、impls、封装与服务无旧契约。

验证结果：HandleTable 10 项、WaitContext 4 项、Runnel 8 项 host 测试通过；`virt` 四核压力负载重复通过（最终构建 3/3）；`sifive_u` 四个可运行 hart 在 2 秒运行窗口内完成同一负载并进入 quiescent（SRST 不可用按平台约定由 timeout 收束）。集成负载覆盖 128 轮消息/Handle/Notification 事务、满箱不 move、64 轮 Tunnel invitation/关闭生命周期、跨进程 attach、8192 字节回绕流和 Sleep deadline。`StartupMailbox` 的通用 startup-resource 枚举替换明确留作后续工作。
