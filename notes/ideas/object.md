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
- `EXECUTE`：把已发布的对象内容作为指令建立执行 view；它不蕴含读取、映射或修改权；
- `WAIT`：观察对象电平并登记等待；
- `SIGNAL`：提交对象允许的状态；
- `DUPLICATE`：派生 rights 不超过原项的新 entry；
- `MANAGE`：执行对象定义的管理操作；
- `MAP`：建立对象允许的映射；
- `TRANSIT`：允许 entry 暂存于有缓冲消息，随后由接收方安装；
- `GRANT`：允许 Building 期事务把 entry 直接移交到目标进程资源图，既包括安装到另一 HandleTable，也包括对象类型定义的不可转移内部 binding；
- `CREATE`：在对象定义的创建域内构造新对象或派生独立资源。

`TRANSIT` 与 `GRANT` 分离，因为两条运输的存储拓扑不同：消息会把 capability 暂存在内核对象图中，直接 grant 不进入对象容器。GRANT 到内部 binding 必须是对象类型明确允许的一次性消费，不能由 rights 自行伪造 role 或改变其它对象。操作同时要求对象类型、role、对象状态和 rights 全部合法；duplicate 与同对象运输只能缩小 rights，创建新对象时的结果 authority 还受创建来源定义的派生上限约束。

## 所有权与终态

对象在有效 Handle、消息 transit entry 或对象内部引用需要它时存活。关闭 Handle 只放弃该引用；对象的逻辑终态由 lifecycle role 决定，不等同于最后一个任意引用消失。

MemoryObject 的 Handle 只授权建立新映射或管理 backing；映射本身作为对象内部引用独立保活 backing。关闭、转移或裁剪 Handle 不撤销已经建立的映射，地址空间只能按[内存模型](mm.md)的所有权规则解除自己的 view。`MAP` 与 `READ`、`WRITE`、`EXECUTE` 正交：只读 view 要求 MAP|READ，可写 view 要求 MAP|READ|WRITE，执行 view 要求 MAP|READ|EXECUTE；任何组合还必须通过对象状态与 W^X 校验。

MemoryObject 的可执行发布状态、WritePermit 与 mapping retire 由[内存模型](mm.md)共同拥有。`SealExecutable` 要求对象定义的管理 authority，并与新 WritePermit 在对象锁上线性化；对象进入 Sealing 后拒绝新写入口，最后一个 retiring writable view 收到全部地址翻译确认后才释放 permit 并推进 Executable。Handle 关闭或 seal 发起线程消散都不能绕过该计数、撤销已发布 seal 或让 backing 在 stale writable translation 仍可能存在时析构。

## MemoryPool 与 MemoryObject interface

MemoryPool 是 page-backed storage 的预算 capability：core 持有固定页额度及其守恒状态，不持有进程、地址空间、child 或活对象列表。root pool 由内核按可信用户物理供给铸造并交给 init；`Derive` 从父池原子转移非零固定额度形成 child core，不能复制额度。普通 Handle 的 duplicate、TRANSIT 与 GRANT 只共享或移动对同一 core 的 authority，不改变容量；Pool 本身没有 owner role、关闭状态、等待电平或按 ID 操作入口。

Pool authority 按操作正交：`READ` 允许固定宽 Query，`CREATE` 允许 Derive，`GRANT` 允许 Building 期消费为目标进程的内部 PoolBinding；`DUPLICATE`、`TRANSIT` 与 `GRANT` 仍分别控制对应传播路径。Derive 产生新对象，但其初始 rights 不得突破来源 entry 的派生上限；资源管理者由此可以交付只能绑定的 leaf pool、允许继续分池的管理 pool，或只读观察 capability，而不产生 ambient 资源权。Pool identity 与 parent identity 只作诊断，不能枚举、寻址或授权。

ProcessCreate 只创建 Building 空壳，不消费 MemoryPool，也不创建页表根。持 ProcessBuilder 的组装者通过 `ProcessBindMemory` 原子消费具 GRANT authority 的 Pool Handle，建立不可转移的 PoolBinding，并把稳定 AddressSpace 从 Unbound 一次转为 Bound。失败保持 Handle 与空壳不变；成功后绑定只授予目标内部 frame-backed 分配，不自动给目标安装 Pool Handle。目标若还需 Query、Derive 或转授，必须另经普通 capability grant 明确取得 authority。

Derive 是不可撤销 grant：parent 只以 ParentCredit 为 child 容量提供来源，不保留 child 列表或撤销入口；child、进程绑定和 backing 自然消散后额度才沿有界父链归还。强制回收需要另一种从创建时就带成员登记、撤销准入和有界 drain 的资源对象，不能给普通 Pool 补一个会追踪全系统 view 的 revoke。关闭或转移 Pool Handle 不撤销既有 binding、backing 或 descendant authority。

MemoryObject 的公共 interface 只包含固定长度创建、固定宽 Query、通过内存映射 interface 建立对象子范围 view，以及单向 `SealExecutable`；作为可等待对象公开 `EXECUTABLE` 电平位，等待复用 WaitMany 通用面与 WAIT right，语义由[内存模型](mm.md)拥有。首版没有 resize、COW、pager、对象内分配器、原始 frame capability 或通用 revoke；更大的逻辑对象和受限子范围授权由多个对象及用户态协议组合。创建从当前进程绑定池取得 affine charge 和实际 backing，charge 随对象而不是创建进程、Job 或 Handle 位置存活。对象 backing 唯一持有 charge；ObjectView 只强持对象与自己的 permit，不复制 backing 所有权。

KernelMemoryBudget 与 MemoryPool 是两种正交 capability：前者支付内核 metadata 与对象壳，后者支付页后备资源。ProcessResources 可以同时持两种不可转移 binding，用户态资源管理器按政策组合交付；Job 不因此变成资源套餐或统计真值。在 KernelMemoryBudget 公开前，进程的内部 MetadataSponsor 只提供固定 permits；MemoryObject 等可脱离创建进程存活的对象取得 permit 后强持 sponsor 到自身析构，不能在进程 Dead 时提前退款或转嫁到持有 Handle 的进程。

Mailbox 有唯一 receiver-owner。owner 不可复制，可持 `GRANT` 直接交付给 Building child，但不能持 `TRANSIT` 进入消息；sender 可复制、可按授权 TRANSIT/GRANT，并可携带 mailbox owner 铸造的 badge。owner 关闭或所在进程退出后 Mailbox 进入 `CLOSED`，清空队列及未接收 entry；残留 sender 只观察终态。

Notification 同样有唯一 owner 和可委托 signaler。owner 只直接 grant，不进入消息；signaler 可按授权 TRANSIT/GRANT。owner 关闭使 Notification 终态。

某些 role 是 affine 且消费式的：Mailbox send-once 在首次成功投递后消费，Tunnel invitation 在成功 attach 后消费。失败不消费。它们可以移动但不能复制。

Tunnel Endpoint 与进程地址空间 lease 绑定，既不能 TRANSIT，也不能 GRANT；跨进程建立对端使用 invitation。

所有关闭都是单向迁移。终态信号持续可见，对象不可复活，也不把旧资源重新解释为另一对象。

## 收束分层

关闭的本地工作量决定对象采用两种收束机制；是否需要等待另一个 hart 的完成确认是正交维度：

- **有界 close**：对象本地关闭工作不超过容量常数。owner 不可 TRANSIT，消息内不含容器 role，可转移 role 的关闭恒为叶子操作，唯一的容器收束受对象容量上限约束。若关闭同时撤销 object-owned mapping，Handle 层必须在消费 entry 前预留地址空间事务；提交后 mapping retire 可以通过 WaitContext 异步完成，但对象不在 close callback 中自旋或保存无界工作。
- **有界 drain**：收束总量可能超过单次调用正常预算的对象（进程的 HandleTable 与地址空间），先发布可收束电平，由持管理 authority 的服务以硬预算分批驱动，进度保存在目标而非调用者。Drain 摘下的 entry 若触发尚不能提交的地址空间事务，作为可恢复 pending close 保留，不能丢弃 lease 或绕过完成确认。

分类判据是本地收束总量是否超出单次预算，不是对象类型；异步确认不把固定容量对象变成无界容器。容量可参数化的容器或引入级联关闭时，必须跨入 drain 档，不能抬高有界 close 的常数。

MemoryObject 只有在 backing 受硬容量上限约束时才属于有界 close；其普通可转移 Handle 的最终消散最多释放该容量内的固定工作。若对象容量不再有硬上限，它必须先进入有管理 authority、持续电平和可恢复进度的 drain 档，不能沿用普通引用计数析构。具体 backing、mapping retire 与完成边界由[内存模型](mm.md)拥有。

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
