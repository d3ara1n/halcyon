# IPC 对象实现

当前 IPC 由进程本地 HandleTable、内核对象、统一等待、消息控制面和 Tunnel/Runnel 数据面组成。共享 ABI 位于 `shared/src/{object,message,wait,startup,call}.rs`；用户封装位于 `user/rinlib/src/ipc/`。

## Handle 与对象

`os/handle_table` 是不依赖内核环境的 generation slot 表，负责 rights 裁剪、duplicate、原子 move、reservation/commit/rollback 和 generation 回绕退休。内核包装位于 `os/kernel/src/task/handle.rs`。`Process` 在 `os/kernel/src/task/proc.rs` 持有表；退出时逐项摘除，释放表锁后才执行 lifecycle callback。

`os/kernel/src/task/object.rs` 定义 `KernelObject`、`HandleRole` 和每对象 `ObjectWaitState`。当前对象类型为 Mailbox、Notification、Tunnel Endpoint 与 Tunnel Invitation；Handle role 含 MailboxOwner、MailboxSender、消费式 MailboxSenderOnce、Notification 双方与 Tunnel 双方。rights 与 role 同时验证；Endpoint、Invitation、一次性投递权和 owner 的关闭语义由对象实现，不由 HandleTable 猜测。消费式 role 的两个实例：Invitation 在 attach 时消费，MailboxSenderOnce 在首次成功 Send 时由 `os/kernel/src/task/mailbox.rs` 的 send 在同一表锁临界区内摘除（解析、入箱、消费原子化；该项同时作为 transit move 入箱时消费顺延到接收方）。`MailboxMakeSendOnce`（0x45）从具 DUPLICATE 权的 MailboxSender 派生，请求 rights 必须同时是源项 rights 与 role 允许集（WRITE|WAIT|TRANSFER）的子集，否则拒绝——与 HandleDuplicate 同判，不截剪也不放大。

当前装载器在进程 runnable 前安装启动 Mailbox owner，并在 `os/kernel/src/initfs.rs` 投递版本化 STARTUP 消息及 grants。rinlib 暂以 `StartupMailbox` 查询该 Handle；这是通用 startup-resource 枚举落地前的过渡实现，不是固定入口寄存器或最终资源发现接口。

## 等待与期限

`os/wait_context` 提供 `Installing → Armed → Finishing → Done` 状态机和单 outcome 仲裁。`os/kernel/src/task/wait.rs` 负责解析 Handle、保存对象强引用、安装订阅、跨对象逐项清理和写回 `WaitResult`。对象更新只改变非消费式电平并 offer 匹配 Context；同一 Context 只有一个完成者取得线程所有权。终态冻结住在 `ObjectWaitState::update`：CLOSED 置位后任何更新不再生效，「单向迁移、终态不可复活」由所有对象共用的这一结构保证，跨关闭窗口的事务收尾无需逐点防御。

WaitMany 是对象等待的唯一用户入口。Sleep 也构造 `WaitAction::Sleep` 的空对象 WaitPlan；`os/kernel/src/sched.rs` 的期限表持有同一个 WaitContext，到期以 `Deadline` outcome 竞争，不再使用独立等待代数。

## 消息与 Notification

`os/kernel/src/task/mailbox.rs` 实现唯一 receiver-owner、多 sender、16 条 FIFO、READABLE/WRITABLE/CLOSED 和 transit Handles。Send 在 `HandleTable → Mailbox` 锁序下先确认容量，再一次性摘除全部源 Handle 并发布消息。Receive 先预留目标 slots、以 token 独占队头，复制完整输出后提交；任一步失败都保留队头并回滚 reservation。Discard 关闭消息中的 transit Handles。邮箱电平由 `MailboxState::publish` 从状态派生：READABLE ⇔ 队列非空，WRITABLE ⇔ 占用（队列加在逯接收占位）低于 `MAILBOX_CAPACITY`，CLOSED 终态独占；所有迁移点调用同一发布函数，不做增量转移。Receive 与 Discard 的 syscall 尾部在表锁外调用 finish_waiters 唤醒等待容量的发送者。

`os/kernel/src/task/notification.rs` 实现 OR 累积的 pending bits。对象 READABLE 只表示 pending 非零；`NotificationTake` 是唯一消费入口，普通 WaitMany 不清位。

## Tunnel 与 Runnel

`os/kernel/src/task/tunnel.rs` 不含全局 registry。Connection 独占一页 `FrameTracker`，两侧状态为 Alive、Invited 或 Closed；Endpoint 持本进程 VA lease，Invitation 是一次性可转移 Handle。Attach 在 `HandleTable → Connection → AddressSpace` 锁序下预留输出、验证邀请、建立映射、消费 Invitation 并安装 Endpoint。Handle 关闭和进程退出走同一 callback；本端解除映射，幸存端得到 PEER_CLOSED，最后一个对象引用释放共享帧。

`user/frameworks/librunnel/src/lib.rs` 把页暴露为互斥的 Producer/Consumer 角色视图。magic、head、tail 与 eof 都通过对齐 `AtomicU32` 访问：初始化最后 release 发布 `RNL1`，attach 首先 acquire magic；数据写后 release head、读者 acquire head，腾空后 release tail、写者 acquire tail；EOF 先 acquire eof，再以后一笔 acquire head 判断排空。该次序对应 RISC-V 规范 `references/normative/riscv-isa-v20250508/src/rvwmo.adoc` 的 “Preserved Program Order / Explicit Synchronization” acquire、release 规则，而非依赖 volatile 或偶然指令顺序。

双方保存对端游标 shadow，并在寻址前验证推进量与 `used <= CAP`。违反角色字段、游标边界、EOF 或版本契约后视图永久 Broken。阻塞封装采用“检查 → AcknowledgeData → 重查 → WaitMany”闭环，并在写入、腾空和 EOF 后通知对端。

## 事务相位模板

所有安装 Handle、写用户输出的 syscall 遵循同一相位序：锁 HandleTable → 构造与校验 entries（全部可失败步骤先于预留）→ 预留（可回滚）→ 锁 AddressSpace、`check_range`/映射（失败整体回滚）→ 写输出（预校验后视为不可失败，expect）→ 提交预留。`MailboxCreate`、`NotificationCreate`、`HandleDuplicate`、`MailboxMakeSendOnce`、`TunnelCreate` 与 `TunnelAttach` 均按此模板实现；用户输出的写回一律先显式预校验、后不可失败写入，不依赖写失败再回滚。

## 验证入口

- `os/handle_table` host 单测：generation、rights、move 和 reservation；
- `os/wait_context` host 并发单测：安装窗口、arm 竞态和唯一赢家；
- `librunnel` host 单测：回绕、满空、EOF、非法游标 Broken，以及双线程多轮回绕；
- `user/systems/init` 集成负载：消息/Handle move/Notification、满箱不 move、send-once（派生、用后即摘、经消息转移后由接收方一次性使用、满箱失败不消费、once 同时作目标与 transit move、从 once 派生被拒）、WRITABLE 电平快路径、128 轮控制面事务、64 轮 Tunnel 生命周期、Invitation 跨进程 attach 和 8192 字节 Runnel；
- 与 pm 协作的跨进程流控唤醒：pm 填满目标邮箱、确认满箱错误后阻塞在 WRITABLE 上，init 腾出容量唤醒，逐条校验 15 条填充与被唤醒后补发的末尾消息；pm 侧内联检测虚假唤醒（醒来后再撞满箱置 spin 位），init 末尾校验 spin 为空，证明唤醒只能由腾位引起；
- QEMU `virt` 四核与 `sifive_u` 四个可运行 hart 覆盖对象等待、timer、进程退出和帧回收。
