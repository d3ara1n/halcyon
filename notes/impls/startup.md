# 启动资源与用户态 launcher 实现

方向见 [`../ideas/bootstrap.md`](../ideas/bootstrap.md)、[`../ideas/object.md`](../ideas/object.md) 与 [`../ideas/task.md`](../ideas/task.md)。当前启动链是 BootPackage → 唯一 init → 用户态 launcher；内核不遍历归档、不识别服务名或服务拓扑。组装模型（线程是组装资源、双通道、生杀闭环）见 [`../ideas/task.md`](../ideas/task.md)「线程」；本篇记录机制落地。

## BootPackage v1

`shared/src/boot.rs` 定义 64 字节 little-endian envelope：magic、version、header_len、flags、total_len、initial ELF offset/length、payload offset/length和 reserved。validator 使用 checked arithmetic，要求 canonical offset、页对齐 payload、零 padding和窗口内完整几何；payload 可为空。

`tools/make-boot-package.py` 原子生成 `artifacts/boot-package.bin`。Just 构建把 `srv_init` 作为唯一 initial ELF，其余验证程序暂以确定序 ustar 组成 opaque payload。DTS `/chosen/boot-package` 只声明物理装载窗口；`board.rs` 验证窗口完整落在 DT memory 内。

`boot.rs` 在帧池注册前验证 envelope，并以实际 `total_len` 收窄保留区。内核只解析 initial ELF，不解释 payload。

## 出生块（Birth Block）线格式

出生块是**组装者与接收进程的用户约定数据**，内核不构造、不映射、不校验。`shared/src/startup.rs` 只保留线格式定义与构造/校验函数（用户态库工具），布局：

```text
[StartupBlockHeader (48 B)]
[Handle × handle_count]
[zero padding]
[opaque payload]
```

Header 保存 magic、version、块长、pid、parent_pid、Handle 数、payload offset/length 与 reserved。几何要求 `handles_end <= payload_off`，间隙全零。Handle 数组保存目标进程 HandleTable 的真实句柄值（由 ProcessGrant 输出），不从 index 推导 slot/generation。

**`parent_pid` 语义**：描述**目标进程**的创建关系，因此是**组装者自身的 pid**（= 目标的创建者，与内核 ProcessQuery 快照的 parent_pid 同一真值），不是组装者的父。组装者在构造出生块时必须传 `rinlib::env::pid()`——传 `env::parent_pid()` 会错位一代（init parent 0 spawn 服务时出生块里会是 0 而非 1）。当前无消费方（潜伏字段），但不能靠「没人读」放任语义错误。

接收进程以出生参数 arg1/arg2（块基址与字节长度）在首线程入口读取（rinlib `env::init` 沿用旧 a0/a1 契约）。init 的出生块 Handle 顺序固定为：Handle[0] root JobControl、Handle[1] primordial SystemReset、Handle[2] init ProcessControl、Handle[3] root MemoryPool；索引由 `shared::startup::initial` 定义。内部 PoolBinding 与 Handle[3] 指向同一个 root core，不复制额度。普通进程 Handle 数组完全来自组装者的 grants，slot 含义由具体启动协议定义，不属于通用线格式。

## 组装 ABI（Building 期外部通道）

`shared/src/proc.rs` 与 `shared/src/call.rs` 定义 fixed-width ABI，rinlib 封装位于 `user/rinlib/src/process.rs`：

- JobControl `CREATE`：JobCreate、ProcessCreate；
- JobControl `MANAGE`：JobSeal、JobDerive；`READ`：JobQuery、JobEnumerate；
- ProcessCreate：一次事务交付 page-resource-light 的 Unbound shell、affine ProcessBuilder 与稳定 ProcessControl；
- **ProcessBindMemory**：以 Pool GRANT authority 一次性把 shell 绑定为完整 AddressSpace；成功消费传入 Pool Handle，失败保留；
- ProcessMap/Write：只接受精确 Building 且已 Bound 的 process，完成匿名零页映射与 backing 回填；
- **ProcessGrant**：把 grants 从组装者表移入目标表，输出目标侧句柄值数组（组装者写入出生块）；
- **ProcessAttach**：向已 Bound 的 Building process 附入线程（`ThreadStartContext`：entry/sp/arg1/arg2），返回 tid；内核不分配用户栈与出生块，无观察壳；该现场类型也供 Running 期 ThreadSpawn 使用；
- **ProcessStart**：Bound readiness + 活体检查门（已附线程 ≥1）→ `Building → Running` → 一次冻结 execution binding → 预育线程整体入册；消费 builder。profile 为平铺参数。

ProcessBuilder 不可 duplicate，最后一个 builder 关闭触发 Building abandonment（预育线程与已装句柄随收束消解）。ProcessMap 最终权限拒绝 W+X；ProcessWrite 不要求最终 PTE 可写。

### 四 op 事务

**ProcessBindMemory**：精确 BuildingLease 登记 → 同一 shell 的原子 Bind reservation → 调用者表 `pin_transfer`，把 builder 作为受保护 authority、Pool entry 作为 consume-on-success authority，统一拒绝 alias/错误 role/rights/pin 冲突 → 锁外准备 AddressSpace metadata permit、单页 funded root、kernel shared root 与 ledger → HandleTable→AddressSpace 双锁内复检 Unbound、发布 Bound 并逻辑摘出 Pool entry → 锁外完成 close 尾段。重复串行 Bind 返回 ObjectNotAvailable，并发 Bind 返回 ObjectBusy；提交前任意失败恢复 pin 并保留 Pool Handle。

**ProcessGrant**：BuildingLease 登记准入 → 调用者表 `pin_transfer`，把 builder 作为受保护 authority、grants 作为待搬移集合统一验证（MANAGE/GRANT、rights 子集、去重、拒绝 builder 自授予）→ 目标表 reserve 槽位与 moved 缓冲预留 → 输出目标句柄值（copy_to_user，失败 rollback + unpin）→ `commit_pinned_transfer` 只提取 grants 并原样恢复 builder → 目标表 commit。成功即一次独立完成的所有权转移；后续组装失败不会把 handles 还给调用者，libprocess 以 `SpawnFailure.grants = Consumed` 明示。单批上界由 shared `PROCESS_MAX_GRANTS` 唯一定义；组装侧超限在建进程前报 `TooManyGrants`。rinlib 安全封装要求 grants/output 等长，裸 syscall 不暴露第二个长度。

**ProcessAttach**：拷入 `ThreadStartContext` → 精确 BuildingLease 登记 → `Process::attach_thread_registered` 校验出生现场（entry 可执行、sp 可写且 16 对齐）→ `lifecycle::attach_registered_member(闭包)` 在 lifecycle 锁内构造线程并决定归宿。若此时仍为 Building，检查表长上限（`PROCESS_MAX_THREADS` 1024）、try_reserve 容量、分配 tid 并插入 Staging；若终止在 lease 登记后截止，登记资格不被撤销，调用仍成功取得 tid，但新线程不入容器而作为终止接管资源在 lifecycle 锁外析构。Context/Limit/Oom 失败均无成员副作用；终止若先于 lease 则入口直接返回 ObjectClosed。bootstrap 在无并发条件下复用 `Process::attach_thread → attach_member` 的精确 Building 路径。Staging 强引用与 `Thread.process` 构成的环仅存在于 Building 期，终止游标 `take_first_staging` 打破。

**ProcessStart**：精确 BuildingLease 登记与 Bound readiness → 解析 profile/解析兼容域 → 按预育数预留提取缓冲 → `pin_consume` 独占 builder → 在一次 Ready 队列锁内预留完整批次 → Job 链锁内上行检查 seal + `begin_running(expected, out_staged)`（要求 `building_ops == 1`，活体门、`Building → Running`、Staging→Ready 与强引用交出原子完成）→ 提交区一次冻结 execution binding、消费 builder、批量 commit Ready。并发 Bind/Map/Write/Grant/Attach 仍持登记时返回 ObjectBusy，不会被 Start 越过；并发 Attach 使 expected 失配同样返回 ObjectBusy。所有提交前失败均 unpin/rollback，Start 可重试。BuildingLease 的 Drop 统一配平 Building 操作计数，成功由 `commit_running` 消费登记，不存在手工 enter/leave 分支遗漏。

## 唯一 init bootstrap（内核内嵌同构序列）

`os/kernel/src/boot.rs` 以与用户态组装者同构的 op 序列构造 init（bootstrap 特例：进程未启动、无用户代码可执行）：

1. 验证 BootPackage 并以不可伪造的 `BootHeldExtent` 按 payload_off 切分物理 owner，消费 supply seed 铸造唯一 root Pool；
2. 创建 pid 1 的 Unbound shell，调用与 syscall 共用的 Bind helper 安装 root-funded PoolBinding，再装载 initial ELF 与 init 栈；
3. 创建 root JobControl、primordial SystemReset、init ProcessControl 与指向同一 root core 的 MemoryPool 管理 Handle，预留四个 Handle 槽并安装；
4. 以真实句柄值构造出生块 prefix；payload owner 先与 root charge 合成 `BootFundedExtent`，prefix 在发布前直接回填，随后以 owner 借用投影完成可失败映射并无分配地安装本体，形成不可公开 Unmap 的只读 lease backing，不经历回库存再取得或无 owner 映射窗口；
5. 经 `Process::attach_thread` 附入首线程，冻结 execution binding，`begin_running` 以唯一 bootstrap Building lease 入册并发布 Ready。

initial ELF 与 prefix 完成后，package 前缀 owner 首次回投帧池；payload backing 与 root PoolBinding 在 init AddressSpace 有界收束时于锁外同步归还物理 extent 与 charge。内核没有 pid 特判的保留洞。

## 用户态公共 loader

`os/elf` 是 bootstrap 与用户态共用的纯逻辑 parser。`user/frameworks/libprocess` 验证 entry、segment overlap、文件边界和页级 W^X，合并连续同权限页；SpawnRequest 显式携带来源 MemoryPool，loader 复制 GRANT-only authority并依次驱动 Create → BindMemory → 分块 ProcessMap/ProcessWrite → Grant → 自构造出生块 → Write 写入映像顶之上的页对齐区 → Attach → Start。SpawnRequest 的 control rights 必须含 MANAGE，使任一步失败都能统一调用 rinlib `abandon_to_completion` 执行 builder close → ProcessDrain → control close；Grant 已提交时，`SpawnFailure.grants` 返回 Consumed，否则返回 Retained，清理链自身的异常由 `cleanup_error` 单独保留。loader 不产生资源或创建 authority，调用者必须显式持 JobControl 与 MemoryPool。

## init/pm 当前政策

`user/services/srv_init` 把 opaque payload 当作私有 ustar，建立：

```text
root
├─ init
└─ services
   ├─ pm_domain
   └─ acceptance
```

所有常规服务是 services 的直接成员。init 保留每个 ProcessControl，按 REAPABLE|CLOSED → ProcessDrain → Query 收束。pm 经出生块 grants 获得 Handle[0] mailbox owner 和 Handle[1] pm_domain JobControl；后者 rights 为 `MANAGE | READ | WAIT`，不含 CREATE。init 保留 pm_domain control 作为兜底。pm 对委托域执行枚举→派生→kill→drain→seal。

acceptance Job 收容一次性 IPC、Job 与可选竞态负载，结束后整域 job_kill。`srv_init` 默认编译为 core workload，只运行确定性内存、IPC、Tunnel、Job 与监督契约；`acceptance-stress` feature 在同一用户态编排器中追加 control/Tunnel 重复压力、`max_work=1` Drain 和完整 16/16 竞态矩阵。profile 只改变 initfs 是否携带 `test_hammer` 及 init 的剧本分支，内核、StartupBlock 与 syscall ABI 均不感知。init 在全部服务监督与资源收束锚点成立后，先以错误对象和裁剪掉 `MANAGE` 的 SystemReset 副本验证 capability 负路径，再直接提交 `Shutdown + Requested`。平台拒绝时记录明确错误并常驻管理端点，保持 root supervisor 存活；当前不经独立电源服务转发。

## 验证

- shared host：BootPackage/出生块 canonical geometry、零 padding 与空 payload；
- handle_table host：consume/transfer 两类 pin、builder 保护、自授予/重复拒绝、rights 回滚、reservation 与 TRANSIT/GRANT；
- libprocess host：entry、segment overlap 与页级 W^X；
- QEMU acceptance：`virt` 是 debug core 快线，`virt-stress` 承担最小预算 Drain、重复压力与 16/16 竞态矩阵，`virt-release` 以 core 覆盖优化代码生成和 trap 寄存器保持，hetero/nofd 与 `sifive_u` 只叠加各自平台/调度域差异；`acceptance` 聚合阶段收尾所需的 stress、release 与 sifive_u。每种 wrapper 都校验 `acceptance workload: core|stress`，避免构建 feature 与所需锚点错配。virt 必须由 QEMU 正常退出证明 shutdown 后端成功；`sifive_u` 在内核明确返回 reset 失败后主动收割。
