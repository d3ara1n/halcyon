# IPC 对象实现

当前 IPC 由进程本地 HandleTable、内核对象、统一等待、Mailbox 控制面和 Tunnel/Runnel 数据面组成。共享 ABI 位于 `shared/src/{object,message,wait,startup,call}.rs`，用户封装位于 `user/rinlib/src/ipc/`。

## Handle entry 与运输

`os/handle_table` 是纯逻辑 generation slot 表。entry 保存 object、role、rights 与不可变 badge；duplicate、rights 裁剪、TRANSIT 和 GRANT 均保持 badge。表提供两条原子摘除：

- `extract_moves` 要求 TRANSIT，供 Mailbox message；
- `extract_grants` 要求 GRANT，供 ProcessStart 的直接跨 HandleTable 安装。

`os/kernel/src/task/handle.rs` 负责对象 allowed-rights 校验与表锁外生命周期回调。Mailbox/Notification owner 最大 rights 含 GRANT、不含 TRANSIT/DUPLICATE；sender、signaler、send-once 与 invitation 按 role 支持 TRANSIT/GRANT；Tunnel Endpoint 与 VM lease 绑定，两者均无。

ProcessStart 在调用者 HandleTable 锁下复检全部 source，然后一次性 extract；此前已预留 child slots 与 StartupBlock 实际 Handle。失败保持每个 source 原值，成功后 entries 直接 commit 到 child，不经过消息对象或 transit buffer。

空槽查找当前仍为线性扫描；真实大 Handle 负载出现前收敛为空闲链。

## Mailbox 与 badge

`os/kernel/src/task/mailbox.rs` 实现唯一 owner、多 sender、16 条 FIFO、READABLE/WRITABLE/CLOSED 和 transit entries。

`MailboxCreate` 原子返回 owner 与 badge-0 sender。`MailboxMintSender` 要求 owner MANAGE，创建同对象、MailboxSender role、调用者指定 immutable badge 和收窄 rights。`MailboxMakeSendOnce` 保持源 sender badge。

Send 在 `HandleTable → Mailbox` 锁序下读取目标 entry 的 role、object 与 badge，以当前 pid 和目标 badge 构造 `MessageHeader`。target 为 send-once 且同一 Handle 出现在 moves 中时，在入队和摘除前返回 IllegalArgument；其余成功 send-once 投递后源项必须仍在表内并被消费。

Receive 预留目标 slots、以 token 独占队头、完整写回后 commit；失败 rollback 且不出队。Discard 和 owner close 在对象锁外逐项执行 transit close。

事务窗口内 READABLE 仍采用乐观电平；多线程接入后最多产生一次可恢复 ObjectBusy 虚假唤醒，语义不丢事件。

## Notification

`os/kernel/src/task/notification.rs` 实现 OR pending bits。ObjectSignals::READABLE 只表示 pending 非零，`NotificationTake` 是唯一消费入口。owner 可直接 GRANT、不可 TRANSIT；signaler 可按 rights 委托。

## 等待与期限

`os/wait_context` 提供 `Installing → Armed → Finishing → Done` 与单 outcome 仲裁。`os/kernel/src/task/wait.rs` 解析 Handle、保存对象引用、安装/清理订阅并写回结果。WaitMany 与 Sleep 共用 WaitContext 和期限表。

当前 ABI 使用相对毫秒 deadline，0 为无限；显式取消分支尚未接入。绝对单调 deadline 与正式取消语义仍在设计审查延期项中。

## Tunnel 与 Runnel

`os/kernel/src/task/tunnel.rs` 不含全局 registry。Connection 持共享帧与双方状态；Endpoint 持本进程 VA lease，Invitation 是 affine capability。Invitation 支持 TRANSIT/GRANT，Endpoint 两者均不支持。Attach 在 HandleTable→Connection→AddressSpace 锁序下原子消费 invitation 并安装 Endpoint。

`user/frameworks/librunnel` 使用对齐 AtomicU32、Release/Acquire、shadow 游标验证与检查—确认门铃—重查—WaitMany 闭环。格式错误和对端异常关闭永久进入 Broken。

## StartupBlock

启动资源交付见 [`startup.md`](startup.md)。outer 直接保存 child reservation 产生的实际 Handle；固定 slot/generation 与 shared tag descriptor 已删除。

## 验证入口

- `os/handle_table` host 测试：generation、reservation、rights、badge、TRANSIT/GRANT；
- `os/wait_context` host 并发测试：安装窗口与唯一赢家；
- `shared` host 测试：StartupBlock v2 构造与损坏拒绝；
- `librunnel` host 测试：回绕、满空、EOF 与非法游标；
- init 集成负载：badge-0/minted/duplicate/TRANSIT/send-once badge，mint 权限与错误对象拒绝，owner GRANT/TRANSIT 边界，send-once alias、满箱失败、WRITABLE、Handle move、Notification 与 Tunnel；
- QEMU virt 四核：四服务验收、进程退出、Handle drain、帧回收与静默停机。
