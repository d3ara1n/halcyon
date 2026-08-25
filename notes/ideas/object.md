# 对象与 Handle

对象是内核管理且可被进程引用的资源：邮箱、通知、隧道端点以及未来的内存、设备和服务登记项都遵循同一模型。对象的身份、存活和权限由内核维持；用户态只持有进程本地的 **Handle**。

## Handle 是唯一的可操作引用

Handle 是固定宽度、不透明的 `u64` 值：高 32 位为 generation，低 32 位为槽位；值零永远无效。槽位复用时 generation 必须改变，因此旧值不能取得新对象；generation 回绕的槽位永久退休，不能再次分配。

每一项 Handle 同时关联对象、rights 与其类型定义的 lifecycle role。role 表达接收所有者、端点 lease 等对象关系，rights 只表达可做的操作；两者彼此独立，不能互相伪造或替代。每项操作同时要求 Handle 指向正确对象、对象仍接受该操作、role 合法且 rights 覆盖操作。rights 是可裁剪集合：

- `READ`、`WRITE`：读取或修改对象内容；
- `WAIT`：观察对象状态并登记等待；
- `SIGNAL`：提交对象允许持有者提交的状态；
- `TRANSFER`：把此 Handle 移入消息；
- `DUPLICATE`：派生 rights 不超过原项的新 Handle；
- `MANAGE`：执行对象定义的管理或关闭操作；
- `MAP`：建立对象允许的映射。

派生或转移只能保留或缩小 rights，不能放大；没有 `TRANSFER` 的项不能经消息外泄。对象类型可定义更细操作，但不得绕过 Handle、role 与 rights 检查。

## 所有权与终态

对象在有效 Handle、消息中转项或对象内部引用需要它时存活。关闭一个 Handle 只放弃该引用；对象的逻辑关闭由 lifecycle role 决定，不等同于最后一个任意 Handle 消失。

邮箱由唯一 receiver-owner 维持开放。`MailboxCreate` 向创建进程交付该不可复制、不可转移的 owner Handle，以及可复制、可转移的 sender Handle；进程内各线程共享 owner 的接收能力。owner 关闭或其进程退出后邮箱进入 `CLOSED`，清除队列及其中未接收的转入 Handle，残留 sender 只观察关闭。sender 还可派生一次性投递权：承载一条消息后由内核摘除，经消息转移后由接收方继续一次性使用。Notification 同样有唯一且不可复制、不可转移的 owner，以及可按授权复制或转移的 signaler；owner 关闭使它终态。

某些 role 是消费式的：执行其定义操作后终态，失败不消费。隧道的 invitation 在 attach 时消费，邮箱的一次性投递权在首次成功投递时消费；两者是同一条 role 维度规则的两个实例，不依赖任何 rights 位表达生命周期。

隧道的端点 lease 和 invitation 则由 Connection 的参与方关系定义。所有关闭都是单向迁移；终态信号持续可见，已关闭对象不可复活，也不把资源重新解释为另一对象。

进程退出时，内核先取消未完成等待和执行，再 drain Handle 表、待接收消息与本地对象关系。drain 与正常关闭使用相同释放语义：端点关闭通知对端，消息中的转入 Handle 被关闭，最后引用释放对象资源。已送入其他进程邮箱的 Handle 属于该消息，不随发送方退出回滚。

## Handle 转移

消息是 Handle 的唯一跨进程转移通道。发送者选择待转移项，内核校验 `TRANSFER`、保留对象引用、撤销发送方项并把裁剪后的项封入消息；投递失败时发送方项保持原状。消息持有这些引用，绝不暴露临时全局名。

接收方只有在输出缓冲和 Handle 表都容纳完整消息时才能接收。内核安装全部转入 Handle、写出完整消息并移除队头；失败不出队、不部分安装。调用期间输出用户区由调用者独占，失败输出不可解释，不得被当作部分交付使用。

## 身份、寻址与启动授权

PID 是内核赋予进程的身份和管理对象，可用于审计、父子关系、回收和诊断，但不是普通 IPC 地址或 bearer token。消息 envelope 的 sender 由内核填写，不自动提供回复能力；回复方必须取得明确交付的邮箱 sender Handle。普通 `Send` 的目标始终是授权的邮箱 Handle；服务发现返回受 rights 约束的服务 Handle，不返回 PID。

启动授权方建立初始 Handle 图。`ProcessCreate`/`ProcessStart` 是统一事务：新进程在可运行前取得一组按类型和标签描述的 startup resources，运行时像探索参数一样主动枚举所需 grants。Mailbox 只是可选资源之一，不占据固定入口寄存器、固定 Handle 数值或特殊对象槽位；获得 receiver 的进程可以从版本化 `STARTUP` 消息继续取得动态 grants。直接安装初始资源是建立根图的唯一例外，不构成 PID 寻址接口。

当前内核装载者仅以同一内部 launch primitive 暂代授权方：sender 为内核身份零，并按集成配置向 `srv_init` 交付 grants。未来 init 读取 initfs 配置、以公开 `ProcessCreate` 创建服务并交付同类资源；届时删除内核授权策略，不改变对象 ABI，也绝不保留 `Send(pid)` 后门。

## 边界

Handle 不是可序列化的全局标识，不能写进共享内存、文件或裸消息负载后在另一进程直接使用。跨进程交付必须使用消息 Handle move。对象模型提供引用、rights、lifecycle role 和终态；服务协议如何认证调用者、如何解释 kind 与负载，仍属于上层。
