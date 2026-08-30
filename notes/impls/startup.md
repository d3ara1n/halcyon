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

**`parent_pid` 语义（A2 修复定死）**：描述**目标进程**的创建关系，因此是**组装者自身的 pid**（= 目标的创建者，与内核 ProcessQuery 快照的 parent_pid 同一真值），不是组装者的父。组装者在构造出生块时必须传 `rinlib::env::pid()`——传 `env::parent_pid()` 会错位一代（init parent 0 spawn 服务时出生块里会是 0 而非 1）。当前无消费方（潜伏字段），但不能靠「没人读」放任语义错误。

接收进程以出生参数 arg1/arg2（块基址与字节长度）在首线程入口读取（rinlib `env::init` 沿用旧 a0/a1 契约）。init 的出生块 Handle 顺序固定为：Handle[0] root JobControl、Handle[1] primordial SystemReset、Handle[2] init ProcessControl；索引由 `shared::startup::initial` 定义。普通进程 Handle 数组完全来自组装者的 grants，slot 含义由具体启动协议定义，不属于通用线格式。

## 组装 ABI（Building 期外部通道）

`shared/src/proc.rs` 与 `shared/src/call.rs` 定义 fixed-width ABI，rinlib 封装位于 `user/rinlib/src/process.rs`：

- JobControl `CREATE`：JobCreate、ProcessCreate；
- JobControl `MANAGE`：JobSeal、JobDerive；`READ`：JobQuery、JobEnumerate；
- ProcessCreate：一次事务交付 affine ProcessBuilder 与稳定 ProcessControl；
- ProcessMap/Write：Building process 的匿名零页映射与 backing 回填；
- **ProcessGrant**：把 grants 从组装者表移入目标表，输出目标侧句柄值数组（组装者写入出生块）；
- **ProcessAttach**：向 Building process 附入线程（`ThreadStartContext`：entry/sp/arg1/arg2），返回 tid；内核不分配用户栈与出生块，无观察壳；该现场类型也供 Running 期 ThreadSpawn 使用；
- **ProcessStart**：活体检查门（已附线程 ≥1）→ `Building → Running` → 一次冻结 execution binding → 预育线程整体入册；消费 builder。profile 为平铺参数。

ProcessBuilder 不可 duplicate，最后一个 builder 关闭触发 Building abandonment（预育线程与已装句柄随收束消解）。ProcessMap 最终权限拒绝 W+X；ProcessWrite 不要求最终 PTE 可写。

### 三 op 事务

**ProcessGrant**：BuildingLease 登记准入 → 调用者表 `pin_transfer`，把 builder 作为受保护 authority、grants 作为待搬移集合统一验证（MANAGE/GRANT、rights 子集、去重、拒绝 builder 自授予）→ 目标表 reserve 槽位与 moved 缓冲预留 → 输出目标句柄值（copy_to_user，失败 rollback + unpin）→ `commit_pinned_transfer` 只提取 grants 并原样恢复 builder → 目标表 commit。成功即一次独立完成的所有权转移；后续组装失败不会把 handles 还给调用者，libprocess 以 `SpawnFailure.grants = Consumed` 明示。单批上界由 shared `PROCESS_MAX_GRANTS` 唯一定义；组装侧超限在建进程前报 `TooManyGrants`。rinlib 安全封装要求 grants/output 等长，裸 syscall 不暴露第二个长度。

**ProcessAttach**：拷入 `ThreadStartContext` → BuildingLease 登记准入 → `Process::attach_thread` 完成出生现场校验（entry 可执行、sp 可写且 16 对齐）并调用 `lifecycle::attach_member(闭包)`：lifecycle 锁内检查 Terminating/表长上限（`PROCESS_MAX_THREADS` 1024）、try_reserve 容量、分配 tid、构造 Thread、插入 Staging；失败零副作用（Context/Closed/Limit/Oom）。bootstrap 也复用该内部出生路径。Staging 强引用与 `Thread.process` 构成的环仅存在于 Building 期，终止游标 `take_first_staging` 打破。

**ProcessStart**：BuildingLease 登记准入 → 解析 profile/解析兼容域 → 按预育数预留提取缓冲 → `pin_consume` 独占 builder → 在一次 Ready 队列锁内预留完整批次 → Job 链锁内上行检查 seal + `begin_running(expected, out_staged)`（活体门、`Building → Running`、Staging→Ready 与强引用交出原子完成）→ 提交区一次冻结 execution binding、消费 builder、批量 commit Ready。并发 Attach 使 expected 失配时返回 ObjectBusy；所有提交前失败均 unpin/rollback，Start 可重试。BuildingLease 的 Drop 统一配平 Building 操作计数，成功由 `commit_running` 消费登记，不存在手工 enter/leave 分支遗漏。

## 唯一 init bootstrap（内核内嵌同构序列）

`os/kernel/src/boot.rs` 以与用户态组装者同构的 op 序列构造 init（bootstrap 特例：进程未启动、无用户代码可执行）：

1. 解析 initial ELF，创建 pid 1 的 AddressSpace（含 init 栈映射——内核供栈仅此一处 bootstrap 例外）与 root Job 成员 core；
2. 创建 root JobControl、primordial SystemReset 与 init ProcessControl，预留 Handle 槽并安装；SystemReset 初始 rights 为 `MANAGE | DUPLICATE | TRANSIT | GRANT`；
3. 以真实句柄值构造出生块 prefix（用户态线格式），payload 以 BootPackage 保留帧映射为只读，映入即收编为 init 地址空间 owned backing；
4. 经 `Process::attach_thread` 附入首线程，冻结 execution binding，`begin_running` 入册并发布 Ready。

initial ELF 与 prefix 完成后，package 前缀页回投帧池；payload backing 随 init 地址空间回收。内核没有 pid 特判的保留洞。

## 用户态公共 loader

`os/elf` 是 bootstrap 与用户态共用的纯逻辑 parser。`user/frameworks/libprocess` 验证 entry、segment overlap、文件边界和页级 W^X，合并连续同权限页，分块 ProcessMap/ProcessWrite；组装序列为 Grant → 自构造出生块 → Write 写入映像顶之上的页对齐区 → Attach → Start。SpawnRequest 的 control rights 必须含 MANAGE，使任一步失败都能统一调用 rinlib `abandon_to_completion` 执行 builder close → ProcessDrain → control close；Grant 已提交时，`SpawnFailure.grants` 返回 Consumed，否则返回 Retained，清理链自身的异常由 `cleanup_error` 单独保留。loader 不产生 authority，调用者必须显式持 JobControl。

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

acceptance 收容一次性 IPC、FAL、Job 与竞态验证负载，结束后整域 job_kill。init 在全部服务监督与资源收束锚点成立后，先以错误对象和裁剪掉 `MANAGE` 的 SystemReset 副本验证 capability 负路径，再直接提交 `Shutdown + Requested`。平台拒绝时记录明确错误并常驻管理端点，保持 root supervisor 存活；当前不经独立电源服务转发。

## 验证

- shared host：BootPackage/出生块 canonical geometry、零 padding 与空 payload；
- handle_table host：consume/transfer 两类 pin、builder 保护、自授予/重复拒绝、rights 回滚、reservation 与 TRANSIT/GRANT；
- libprocess host：entry、segment overlap 与页级 W^X；
- QEMU acceptance：`virt`、`virt-release`、hetero、nofd、`sifive_u` 均要求最小预算 Drain、竞态矩阵 12/12、分阶段服务监督、委托域终态、reset authority 负路径与内核接受 reset 锚点。virt 必须再由 QEMU 正常退出证明后端成功；`sifive_u` 无 shutdown 设备，内核返回失败后由验收脚本按明确终态锚点主动收割（`ACCEPTANCE_TIMEOUT` 仅作挂死兜底）。
