# review：Bootstrap / 用户态 launcher 机制审查

按用户裁定，本次为**机制层 review**：对象是 notes/ideas 与 notes/impls 的方向性文档，代码仅作参照验证文档声称的机制形状；不做逐行代码审查。原十切片代码审查计划见 [`todo-2026-08-26-bootstrap-launcher-review.md`](../todo-2026-08-26-bootstrap-launcher-review.md)，其结论边界不受本篇替代。

## 审查输入

| 层 | 文档 |
|---|---|
| 方向 | `notes/ideas/bootstrap.md`（主）、`notes/ideas/{object,task,service}.md` |
| 实现 | `notes/impls/startup.md`（主）、`notes/impls/{task,ipc}.md` |
| 参照 | `os/kernel/src/sched.rs`、`os/kernel/src/task/table.rs`、`shared/src/boot.rs`、`shared/src/proc.rs` |

## 逐主题结论

### 1. 两层格式与 opaque payload —— 成立

内核可见面收敛为 fixed envelope + initial ELF，payload 对内核完全 opaque（不解释归档/路径/服务拓扑），换 initfs 协议不动 BootPackage 与内核。这是 Zircon bootdata/userboot 与 seL4 BootInfo/root task 的同款责任切分，且独立理由充分：内核只保留不可替代的 bootstrap 机制，后续装载遵守同一 capability 模型。envelope 版本严格钉死（`version != CURRENT` 即拒绝），packer 与内核同仓同步演进，无需兼容策略——与「ABI 不冻结、两侧同步改」戒律一致。

### 2. 唯一 init 与 authority root —— 成立

内核启动五步只到「首次发布 init runnable」，不遍历 bin/、不组装服务拓扑；init 的全部 authority 来自显式 Handle（root JobControl + primordial capabilities），PID 1 仅 provenance。「init 是临时授权根、退出即消散未交付权、不级联不重启」与 object.md 的「权利沿 capability 图传播、不沿 PID 树传播」自洽。若 init 半途 fault，系统降级续存是文档明示的可接受结果，配置责任在用户态侧——边界诚实。

### 3. borrowed payload 映射 —— 机制成立，留一条生命周期建议

initfs 以 borrowed `U|R` 页直接映入 init、prefix 复制后回投、物理保留按实际 total_len 收窄（非 DT 最大窗口）——时间线清晰，所有权不混淆。但见 F1。

### 4. StartupBlock outer 与真实 Handle —— 成立

outer 只含几何与实际 child-local Handle 值（来自 reservation，不从 slot/generation 推导），不赋予业务 tag；bootstrap padded prefix 与普通紧凑块共用同一 validator。与 ideas/object.md「StartupBlock」节逐条对应，无单边漂移。

### 5. Job / ProcessBuilder / ProcessControl capability 边界 —— 成立

affine builder（禁 DUPLICATE、close 回收半成品）、CREATE 权限门控、control rights 不放大、关闭 control 不杀进程、Dead 后只留轻量终态壳——与 ideas/{object,task}.md 的 role/rights 模型一致；创建关系（parent_pid）与管理权分离贯彻到位。生命周期四态中 Terminating 缺席属已声明的分阶（绑定 ThreadSpawn 屏障），非缺口。

### 6. ProcessStart 原子事务与 marker 发布 —— 成立，附一处演进注记

预构造→双 marker 预留→同一锁下复检并 extract grants→零分配提交区的结构，使「失败全回滚、成功线性化」可论证；`has_ready` 忽略 marker + commit 后才发 IPI + idle 双重检查，静默判定闭合。但 marker 的 reserve/commit/rollback 落在公平类的自由函数上而非 `SchedClass` trait 契约内，见 F2。

### 7. 用户态 ELF 装载分层 —— 成立

os/elf 纯逻辑共用、libprocess 承担 program header/BSS/页权限/栈规划、内核只验范围与 W^X——实现集中化不产生 authority；「多数服务无 spawn 权、走 pm 协议」的方向已在 ideas/bootstrap 明示。ld-erhino/MemoryObject 留白位置正确。

### 8. 指令流同步 —— 成立

新 dispatch 前本地 `fence.i`、Resume 热路径省略的前提（Running 无 Building 写入口）成立且与多线程无关——Building 写只在 Start 前存在，Start 后同进程加线程也不破坏该前提。写入经队列锁 Release/Acquire 先于目标 hart 取指，本地 fence.i 即满足 Zifencei 语义，不需要 remote fence。

### 9. 文档间一致性 —— 无矛盾

ideas↔impls 抽查：owner GRANT/TRANSIT 不对称、Tunnel Endpoint 绑 lease、HandleTable 线性扫描债务声明、「Building 不进进程表」与「marker 不可见预留」表述相容、KNOWN_ISSUES 写回窗口与 impls/task 单线程前提互指——两侧同步维护良好。

## Findings

### F1（已实施）：payload 页映入 init 时即收编为 owned backing，随地址空间销毁归还

原实现将 payload 页由启动保留洞持有到系统结束；init 死后无人可再映入，属纯死重。裁定：payload 页在 `map_bootstrap_block` 映入时即移交为 init 地址空间的 owned backing（帧池启动保留洞从未入空闲链，Drop 经 dealloc 首次归还，无双重释放），全程无 pid 特判。`just virt` 验证：quiescent 时空闲帧 256904 → 260721（+3817 页 ≈ initfs payload 全量归还）。方向已入 notes/ideas/bootstrap.md 与 notes/impls/startup.md。

### F2（注记）：ready-marker 预留目前硬连单一公平类，域路由是既定演进点

通俗版：ProcessStart 发布子线程时，「先占位、后替换」的预留机制写死在当前唯一的具体调度类（Fair）的自由函数里，而不是调度类的通用接口（`SchedClass` trait 只有 enqueue/pick/has_ready）。现在只有一个域所以没问题；将来接 D64（异构大核域）时，子进程要按执行需求路由到兼容域，届时必须决定：预留语义上收为所有调度类都必须实现的接口，还是由域层提供统一的预留通道。这是调度域扩展工作的设计点，此处登记防误用。

### F3（已实施）：Pid 由 u32 拓宽为 u64

u32 的历史原因是计划把 pid+parent_pid（或 pid+tid 的 uni_tid 方案）拼进一个 u64。该方案未落地，属纯债务，已直接换 u64：shared/src/proc.rs（`Pid = u64`，uni_tid 注释删除）、object.rs（`ProcessId` 统一为 `proc::Pid`）、StartupBlockHeader（40→48 字节，pid/parent_pid 各 u64）、ProcessCreateResult（reserved 提升为 u64 消除隐式 padding）、kernel mailbox 去 cast、rinlib env 原子量改 AtomicU64、libprocess Spawned.pid 改 u64。三侧 host 测试与 `just virt` 全绿。

### F4（已回答）：Job「预算」在代码中亦无含义

核查 `os/kernel/src/task/job.rs`：结构只有 header、dead_code 标注的 parent 引用与等待状态，无任何预算字段或语义——「预算」仅存在于 ideas/task.md 的方向性表述。维持文档级 finding：pm 接管管理权前需在 ideas 层补契约或删词。

## 对后续计划的约束

todo-2026-08-26-bootstrap-launcher-review.md 的十切片代码审查保留但降级为机会型任务（有空就做），不再阻塞 process-lifecycle 计划；其结论边界以代码证据为准，本篇不替代。
