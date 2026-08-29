# 对象、Capability 与 Handle

对象是内核管理且可由进程引用的资源。用户态不直接持有对象身份，而是在自己的 HandleTable 中持有一项 **capability entry**；**Handle** 只是该 entry 的进程本地不透明名字。

## Handle 是本地名字

Handle 是固定宽度 `u64`：高 32 位为 generation，低 32 位为槽位，零值永远无效。槽位复用时 generation 必须改变；generation 回绕的槽位永久退休。因此旧值不能取得后来占据同一槽位的新对象。

Handle 不能序列化为跨进程凭据。把其数值写入文件、共享内存或普通 payload，不会让另一进程取得引用。

## Capability entry

每项 entry 由四个正交维度组成：

```text
object + lifecycle role + rights + immutable badge
```

- **object**：被引用的内核对象；
- **role**：owner、sender、invitation、endpoint 等对象关系与生命周期位置；
- **rights**：允许执行的操作；
- **badge**：对象类型可解释的不可变授权上下文，普通 entry 为零。

role 不能由 rights 伪造；badge 不改变对象身份或生命周期。duplicate、移动和 rights 裁剪都保持 object、role 与 badge，只能缩小 rights。

通用 rights：

- `READ`、`WRITE`：读取或修改对象内容；
- `WAIT`：观察对象电平并登记等待；
- `SIGNAL`：提交对象允许的状态；
- `DUPLICATE`：派生 rights 不超过原项的新 entry；
- `MANAGE`：执行对象定义的管理操作；
- `MAP`：建立对象允许的映射；
- `TRANSIT`：允许 entry 暂存于有缓冲消息，随后由接收方安装；
- `GRANT`：允许 Building 期的 ProcessGrant 等事务把 entry 直接安装到另一 HandleTable。

`TRANSIT` 与 `GRANT` 分离，因为两条运输的存储拓扑不同：消息会把 capability 暂存在内核对象图中，直接 grant 不进入对象容器。操作同时要求对象类型、role、对象状态和 rights 全部合法。

## 所有权与终态

对象在有效 Handle、消息 transit entry 或对象内部引用需要它时存活。关闭 Handle 只放弃该引用；对象的逻辑终态由 lifecycle role 决定，不等同于最后一个任意引用消失。

Mailbox 有唯一 receiver-owner。owner 不可复制，可持 `GRANT` 直接交付给 Building child，但不能持 `TRANSIT` 进入消息；sender 可复制、可按授权 TRANSIT/GRANT，并可携带 mailbox owner 铸造的 badge。owner 关闭或所在进程退出后 Mailbox 进入 `CLOSED`，清空队列及未接收 entry；残留 sender 只观察终态。

Notification 同样有唯一 owner 和可委托 signaler。owner 只直接 grant，不进入消息；signaler 可按授权 TRANSIT/GRANT。owner 关闭使 Notification 终态。

某些 role 是 affine 且消费式的：Mailbox send-once 在首次成功投递后消费，Tunnel invitation 在成功 attach 后消费。失败不消费。它们可以移动但不能复制。

Tunnel Endpoint 与进程 VM lease 绑定，既不能 TRANSIT，也不能 GRANT；跨进程建立对端使用 invitation。

所有关闭都是单向迁移。终态信号持续可见，对象不可复活，也不把旧资源重新解释为另一对象。

## 收束分层

关闭的收束工作量决定收束机制，只有两档：

- **同步 close**：工作量不超过容量常数的对象，关闭在单次调用内同步完成。其上界由 role 结构保证：owner 不可 TRANSIT，消息内不含容器角色，可转移的 role 关闭恒为叶子操作，唯一的容器收束（owner 清空队列）受对象容量上限约束。新增可转移 role 时维持这条推导即可，不需要重审全局枚举。
- **有界 drain**：收束工作量可能超过单次调用正常预算的对象（进程的 HandleTable 与地址空间），先发布可收束电平，由持管理 authority 的服务以硬预算分批驱动，进度保存在目标而非调用者。

分类判据是收束工作量是否超出单次调用预算，不是对象类型。容量可参数化的容器（如邮箱扩容）或引入级联关闭时，必须跨入 drain 档而不是抬高同步档的常量。

## 两种跨表交付

### 消息 transit

发送者提交 `HandleMove[]`。内核验证每项 `TRANSIT`、目标 rights 不放大且没有重复源项，随后原子摘除全部源 entry 并封入消息。失败保持源表不变。

接收方只有在输出缓冲与 HandleTable 都容纳完整消息时才安装全部 entry、写出完整结果并移除队头；失败不部分安装、不出队。Discard、Mailbox 关闭或进程退出会按 role 关闭未接收的 transit entry。

### 直接 grant

ProcessGrant 从组装者表中原子摘除持 `GRANT` 的 entry，直接安装到尚未 runnable 的目标表。它不经过对象队列，适合 affine owner、启动内存与设备资源。目标 rights 仍只能收窄；一次 grant 成功后所有权即归目标，不与后续线程附入或首次发布组成跨调用回滚事务。

两种运输都属于 capability move，但不能以一项 rights 冒充另一种存储拓扑。

## 身份、授权与平台根

PID 是内核分配的 provenance 与诊断身份，可用于审计、创建关系和故障定位，但不是 IPC 地址或 authority。`parent_pid` 只表示谁创建了进程，不产生管理、继承或回收权；管理权来自显式 Process capability。

普通对象的初始 capability 由创建者持有。MMIO、IRQ、DMA、物理资源、系统复位 authority 和 root Job 不能由用户凭空创建：内核依据可信平台事实铸造这些 primordial capabilities，并在 initial launch 中交付 init。内核是技术铸造者，init/resource manager 才是策略授予者；后续权利沿 capability 图传播，而不是沿 PID 树或进程权限等级传播。

系统不要求 uid/gid 或进程权限级别。若未来增加多用户，认证和 ACL 位于用户态 grant 铸造阶段；最终对象操作仍以 capability 为必要授权。

## StartupBlock

普通进程的 StartupBlock 是 launcher 与目标进程之间的用户态约定数据：

```text
Header(pid, parent_pid, geometry)
actual child-local Handles[]
opaque Payload[]
```

launcher 先以 ProcessGrant 取得目标表中的实际 Handle 数值，再构造 outer 与 payload、写入 Building 地址空间，并把完整块的地址和长度作为首个附入线程的出生参数。内核不解释普通 StartupBlock 的几何、Handle 数组或 Payload，也不为 Handle 赋予业务 tag；入口 `a0/a1` 只由用户态约定界定该块。

init bootstrap 是唯一内核内嵌组装特例：内核为 initial process 构造同形 outer，并把 BootPackage opaque payload 收编进 init 地址空间。该特例不形成普通进程可调用的映射或装载入口。

## 边界

对象模型只提供引用、role、rights、badge、运输和终态。服务协议如何解释 badge、认证请求、派生更窄 grant、执行配额或撤销，属于用户态协议。sender PID 可作 provenance，不能代替显式 capability。
