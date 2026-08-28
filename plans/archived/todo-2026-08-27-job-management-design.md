# Job 管理面设计确认（step 5 前置）

- 状态：**已拍板并归档（2026-08-27）**——决策已并入 `todo-2026-08-26-process-lifecycle.md` 第二批结构决策与「已确认的 Job ABI」；本文保留完整推导、被否选项与竞态闭合论证，供实施与 step 9 验证对照。
- 输入：
  - 计划基线：step 5 目标与已拍板决策 1–8（process-lifecycle 计划）；
  - 取证：[`ref-2026-08-27-job-enumerate-derive-research.md`](../ref-2026-08-27-job-enumerate-derive-research.md)（枚举/派生/ID 稳定性）+ [`ref-2026-08-task-termination-research.md`](../ref-2026-08-task-termination-research.md)（kill/seal/完成传播）；
  - 代码现状：`os/kernel/src/task/job.rs`——`sealed` 字段与 `ChildEntry::Job` 读取路径已预留；成员/子表为事务 marker + 强持条目（Reserved 对枚举不可见）；PID 分配器单调不复用；JobControl rights = CREATE|MANAGE|READ|WAIT|DUPLICATE|TRANSIT|GRANT；错误码 `BufferTooSmall` 已存在。

## 一、决策点

### D1 枚举形态：单调 ID 序游标分页

**问题**：管理者如何枚举 Job 的直接成员（child Jobs 与 member processes），同时满足完成标准「单次内核调用的枚举工作量有固定上界」。

**选项**：

| | a. 缓冲数组 + 截断计数（Ziron/Windows 式） | b. 单调 ID 序游标分页（推荐） |
|---|---|---|
| 形态 | 调用者给缓冲，内核一次填 `actual` 条，`avail` 提示总数，扩容重试 | `cursor` = 上批最后返回条目的 ID，内核按 ID 升序返回 ≤N 条 + `next_cursor` |
| 单批上界 | 受调用者缓冲限制（间接） | 内核常量封顶（直接） |
| 断点续扫 | 无——扩容即全量重扫 | 有——游标天然续扫 |
| 竞态语义 | 无快照承诺，遍历中成员增删不可判定（Zircon 官方明示 racy 无承诺） | 良定义：ID 单调不复用 ⇒ `id > cursor` 的存活成员必然在后续批出现；ID ≤ cursor 未返回者必然已移除 |
| 内核状态 | 无 per-caller 状态 | 无 per-caller 状态（游标在调用者手里） |

**推荐 b，独立理由**：PID 已是全局单调不复用分配器（代码现状），「ID 序游标」不引入任何内核侧枚举状态即获得断点续扫与良定义竞态语义；完成标准要求单批固定上界，b 直接封顶。Ziron 无游标是其宏内核可全量拷贝、且官方明示对 job 枚举无一致性承诺的惯性——照抄会继承一个我们没有承诺能力支撑的模糊语义。

**条目形态**：仅 `id: u64`（koid 数组风格）。状态/终因经派生后 `ProcessQuery`/`JobQuery` 查询；枚举本身只回答「谁是成员」。死成员自动从表消失，收敛性由「枚举至空」表达，不需要在条目里带状态。

**屏障语义（事务 marker）**：枚举按 ID 升序扫描，遇未决 Reserved 占位即终止本批——`next_cursor` 严格小于该占位 ID，占位不输出。占位窗口是单个 syscall 内的临界区序列，枚举者下批即越过。这闭合「跳过占位又推进游标越过它」的漏项窗口（详见第四节）。

### D2 派生原语：按 ID 单目标派生，rights 交集单调

**问题**：管理者如何从 JobControl 取得成员的 ProcessControl / child Job 的 JobControl。

**对照取证**：Ziron `zx_object_get_child(handle, koid, rights, out)`——按 koid 精确匹配**直接**子对象；rights 调用者指定且不得大于 parent handle；要求 ENUMERATE right；目标消亡返回 NOT_FOUND。koid 不复用保证枚举→派生之间永不错指新对象。官方不存在「原子枚举+派生」原语；竞态语义就是 NotFound 重试。seL4 无按 ID 派生面，capability 派生 rights 单调不增（超限静默降级）。Linux pidfd 是同型思路：稳定引用解耦目标身份与编号生命周期。

**设计**：单一原语 `JobDerive(job_control, kind, id, rights, out)`：

- `kind` 判别：0 = child Job（按 JobId 匹配 children 表）→ 新 JobControl；1 = member process（按 Pid 匹配 members 表）→ 新 ProcessControl；
- `rights ⊆ (源 JobControl handle rights ∩ 目标角色 allowed_rights)`，超集拒绝 `RightsDenied`——与 HandleDuplicate/JobCreate 现有「请求 ⊆ 源」完全同构，不学 seL4 静默降级（我们的错误面纪律是显式拒绝）；跨角色时 CREATE 等目标不具备的位自然被交集裁掉；
- 匹配失败（ID 非直接成员 / 目标已完成移表）→ `ObjectNotFound`。ID 单调不复用 ⇒ NotFound 只意味着「当初那个对象已完成」，永不是「错指了另一个」；
- 权限要求：MANAGE（见 D5）。

**ID 的角色边界（原则精确化）**：PID/JobId 都不构成全局操作入口——任何地方都没有「按 ID 操作对象」的路径。唯一的操作角色是在已持 JobControl 的**直接成员域内**作派生选择子：authority 完全来自 capability，ID 只在 capability 圈定的作用域内有效。这需要把 `ideas/task.md`「PID 仅用于诊断」的表述精确化为「PID 不构成全局操作入口；唯一操作角色是 JobControl 枚举域内的派生选择子」。对照：seL4 无 ID 面也没有内核强持容器，「capability 丢了就丢了」可以是其语义；我们的 Job 成员表是生命周期根，「成员存在但 authority 消散」是常态（control 消散、管理者重启），必须有再获取通道。

**为什么不做「枚举即派生」**（枚举直接批量输出 Handles）：批量 Handle 输出会占用调用者 HandleTable 槽位（枚举 N 个 = 消耗 N 槽），管理者观察拓扑也要消费 authority，违反观察/管理分离；且批间语义复杂（部分派生失败怎么回滚）。两步走（枚举 ID → 按需派生）让观察免费、管理显式，与 capability 模型的权利粒度一致。

### D3 Seal 语义与完成传播：seal 只封创建，完成 = 自身 sealed && empty

**问题**：JobSeal 的作用范围（是否向下传播）、Job 的完成（CLOSED 发布 + 从父表移除）条件、以及「祖先封口、后代完成」的传播机制。

**推荐结构**：

1. **seal 不向下传播**。`JobSeal(J)` 是 O(1)：置 sealed 位；若 J 已空立即完成。不扫成员表（宽度无界）。
2. **effective seal 上行检查只用于创建拒绝**。JobCreate/ProcessCreate/ProcessStart 的提交点沿 parent 弱引用链上行 ≤JOB_DEPTH_MAX(32) 步，任一祖先 sealed → `ObjectClosed`（对齐决策 6：不可逆关闭语义统一 ObjectClosed，不新增 InvalidState 类错误）。这一检查方向向上、成本 O(depth)，根封口无需递归遍历即覆盖全部后代的创建口。
3. **完成条件 = 自身 sealed && members 空 && children 空**。触发事件三处：seal 时已空；remove_member 后空；child Job 完成移除后空。每处事件单步判定，完成沿树自底向上逐步传播——任何单步有界。
4. **被「递归」覆盖的部分归用户态**。递归封口与递归 JobKill 同构：pm 组合「枚举 children → 对每个 child JobSeal → 递归；枚举 members → 派生 ProcessControl → ProcessKill → Drain」。这与「内核不递归遍历无界子树」的既有边界一致。
5. **root Job 特例**：static anchor 永持；完成时同样发布 CLOSED，但对象不从任何表移除、不释放（boot 生命周期）。

**被否选项**：

- *内核递归 seal/kill（Ziron 式 `Kill` 递归子树）*：向下递归成本与子树规模成正比，无界内核路径，直接违反完成标准与协作式戒律。
- *完成条件用 effective_sealed（祖先封口即可完成）*：看似更自动，实则断裂——祖先 seal 时「已空但自身未 seal 的后代」需要被触达才能完成，而向下游历无界；协作式内核无后台线程可异步推进，挂到「下一次触达」则无观察者时永不完成。否。相比之下，「完成 = 自身 sealed」把传播责任明确交给用户态逐层 seal，任何卡住的情形都能由 D4 的派生兜底救回（枚举可见 → 派生 JobControl → 显式 seal → 完成）。
- *seal 时扫描 immediate children*：单次 O(width)，宽度无界，违反固定上界。否。

**语义代价（明示）**：单独 seal(J) 而不逐层 seal 子孙时，J 的 CLOSED 会被未 seal 的 child Job 卡住——这是调用者政策不完整（只封了创建口、没收束子域），不是内核缺陷；递归 JobKill 的正确组合本来就逐层 seal。上溯兜底链：任何 authority 消散的空壳 Job 都能从 root 出发「枚举 + 派生 + seal」收敛，最终管理根是持 root JobControl 的 init；init 失效是既有原则定义的系统级管理根失败。

### D4 JobId 引入与派生兜底

**JobId**：对齐 Pid 的 `JobId(u64)` 全局单调不复用分配器；root Job 恒为 1（首个创建）。与 Pid 分立空间——两者计数语义与错误面（WrongObjectType）独立，不共享。

**派生兜底（决策 2 的遗留）**：「无人收束的进程」的 Job 派生兜底由 D2 直接覆盖，无需专门机制：进程自 Building 提交点入成员表、Dead 发布点移除，因此 REAPABLE 但 control 全消散的进程**必然仍在成员表内**——枚举可见 → JobDerive 派生持 MANAGE 的 ProcessControl（从成员表强持的 core 上铸造，与原 control 是否消散无关）→ Drain 至 Complete。同构覆盖 Job 侧：authority 消散的空壳 child Job 可枚举 + 派生 JobControl + 显式 seal → 完成。派生后的 Drain 进度保存在目标进程而非调用者（既有 drain 契约），管理者重启后接管语义自然成立。

### D5 枚举/派生的 rights 位：READ + MANAGE 复用（推荐）

**问题**：枚举与派生各要求什么 right；是否新增 ENUMERATE 位（Ziron 有专门位）。

- a. **复用现有位（推荐）**：JobSeal 要 MANAGE；JobQuery/JobEnumerate 要 READ；JobDerive 要 MANAGE。枚举 = 观察拓扑信息 → READ 语义自然；派生 = 铸造 authority → MANAGE 语义自然。
- b. 新增 `ENUMERATE` 位：粒度对齐 Ziron，但当前 KNOWN 位 10 位刚满，为一个语义已能由 READ 表达的操作扩位是位空间浪费；且我们的观察面（Query）本来就持 READ，枚举与 Query 同属观察，分位反而制造「能查状态不能查成员」的人为割裂。

### D6 JobQuery 最小面

对齐 ProcessSnapshot 风格的 fixed-width 快照：

```text
JobSnapshot（40 字节，8 字节对齐）
  jid: u64            // 本 Job；root 的 parent_jid = 0 表示无父
  parent_jid: u64
  state: u32          // 判别值固定：Open=0, Sealed=1, Dead=2
  live_processes: u32 // 近似计数：当前直接成员进程数（不含事务占位）
  live_children: u32  // 近似计数：当前 child Job 数
  reserved: u32       // 必须为零
  reserved2: u64      // 必须为零
```

- 计数是**非精确近似值**（读取时点的瞬时值，无一致性承诺）——正确性路径永远走 CLOSED 等待与枚举收敛，计数只供显示与启发式决策；此点写入 ABI 注释，不构成协议依据。
- Dead 后查询返回冻结快照（Job 无收束工作，完成即 Dead，观察壳由存活 JobControl 保活——与 Process dead shell 同构）。
- 未知 state 判别值由用户态拒绝（对齐 Process ABI 纪律）。

### D7 F4 预算去留（需拍板）

F4 挂账原话：「代码无预算语义，『预算』仅存在于 ideas/task.md 方向性表述；pm 接管管理权前在 ideas 层补契约或删词」。触发点（pm 接管）就在 step 6，必须现在定。

- a. **补最小契约（推荐）**：ideas/task.md 保留「资源预算是 Job 的方向性职责」表述，改述为一句话契约——「Job 预算 = 域内成员资源总量的上限记账；粒度与接入时点待资源记账需求出现后另行设计，当前系统无预算机制，Job ABI 不含预算面」。理由：Job 作为资源域是成熟系统的立身职责（Ziron job / Windows Job Object 的核心功能即资源限制），完全删词丢失方向；ideas 层允许领先于实现（AGENTS：代码落后于设计不是文档错）；但不给无需求的机制设计 ABI。
- b. 删词：Job 定义收缩为「创建域 + 管理域」。更符合「无需求不设计」，但丢弃一个方向性锚点，将来重引入时缺乏概念连续性。

## 二、拟定的 Job 管理 ABI（草案，拍板后转正式）

```text
JobSeal(job_control) -> ()
    要求 MANAGE；幂等（重复 seal 成功，不改变既有状态）；
    sealed 后该 Job 及全部后代的创建/启动口经上行检查永久关闭。

JobQuery(job_control, out: *JobSnapshot) -> ()
    要求 READ。

JobEnumerate(job_control, kind, cursor, buf: *u64, buf_len) -> JobEnumerateResult
    要求 READ；kind：0 = child Jobs（JobId 序），1 = member processes（Pid 序）；
    cursor = 上批 next_cursor（首批传 0）；按 ID 升序返回 ≤min(buf_len,
    JOB_ENUMERATE_MAX) 个 ID；遇未决事务占位终止本批（next_cursor 停在其前）；
    buf_len 超过 JOB_ENUMERATE_MAX 按 MAX 截断（BufferTooSmall 不用于此——
    截断由 more 表达）。

JobEnumerateResult（16 字节，8 字节对齐）
  next_cursor: u64   // 本批最后返回条目的 ID；无返回时等于入参 cursor
  actual: u32        // 本批实际写入 buf 的条目数
  more: u32          // 0/1：表内是否仍存在 ID > next_cursor 的（可见或占位）条目
  // 契约：more=1 ⇒ actual ≥ 1（more=1 且 actual=0 为内核违约，用户态拒绝）

JobDerive(job_control, kind, id, rights, out: *Handle) -> ()
    要求 MANAGE；kind 同上；rights ⊆ (源 handle rights ∩ 目标角色
    allowed_rights)，超集 RightsDenied；目标不在直接成员表内 ObjectNotFound。
```

- 调用号（接 Process 控制段 0x1x）：`JobSeal = 0x19`、`JobQuery = 0x1a`、`JobEnumerate = 0x1b`、`JobDerive = 0x1c`。
- `JOB_ENUMERATE_MAX`：实施定量级取 64–256（单批 ≤2KB 用户缓冲），编译期常量进 shared。
- Job 的 `ObjectSignals` 维持仅 CLOSED：Job 无收束工作，Sealed 本身可通过 JobQuery 观察，不需要独立电平；完成即 CLOSED，等待 CLOSED 即「直接成员全部完成」屏障。

## 三、数据结构与锁序要求（实施约束）

1. **成员/子表改按 ID 有序映射**（`BTreeMap<Pid, MemberEntry>` / `BTreeMap<JobId, ChildEntry>`）：枚举单批 `range((cursor,..]).take(N)` 为 O(log n + N)，固定上界达成。现 `Vec + swap_remove` 退役——swap_remove 破坏序且移除点 O(1) 的收益不值得枚举的 O(width) 扫描；BTreeMap 移除 O(log n)，成本有界。
2. **ID 分配必须在 owner Job 的 JobState 锁内与占位插入同一临界区**：否则多核下「hart A 分配 P、hart B 分配 Q > P 并先入表、A 才入表」乱序会让游标越过 Q 后漏掉 P。锁内分配保证表内 ID 序 = 分配序。现有 `alloc_pid` 在锁外（process 创建路径），实施时挪入。
3. **创建检查 + 提交原子化**：JobCreate/ProcessCreate 的「上行检查祖先 seal + 占位提交」须在先父后子的 JobState 链锁（≤32 把，短临界区）内完成；JobSeal 只持自身单锁。两方向在 owner Job 锁上互斥，形成与 Ziron「AddChild 与 Kill 同锁」等价的线性化——先到者定胜负。ProcessStart 的封口检查在 job 链锁内做（链锁在 lifecycle 锁之外先取，不违反「lifecycle 锁内不出游」契约；即锁序规范为 **Job 链锁（先父后子）→ lifecycle 锁 → 其他对象锁**）。
4. **完成传播延迟触发**：事件点在子 Job 锁内发现「空 + sealed」后，先放子锁再取父锁执行移除与父级再判定（避免子→父嵌套与全局锁序冲突）。延迟窗口内不可能出现新成员（sealed ⇒ 创建口已封），判定幂等，安全。
5. **游标不漏论证**：next_cursor 永远等于「本批最后**返回**条目的 ID」，不超过表内已见最大 ID；锁内分配保证任何后续新占位的 ID 必然大于当时表内一切既有 ID > next_cursor——后续批必然覆盖。已移除（Dead/完成）条目不返回，是收敛方向。

## 四、竞态闭合清单（供 step 9 验证矩阵）

- 枚举 vs 并发 Create（占位屏障）／并发 Dead 移除（消失即收敛）；
- 派生 vs 目标 Dead/完成（NotFound 干净失败，ID 不复用永不错指）；
- seal vs 并发 Create/Start 提交（链锁线性化，先到者定胜负）；
- 完成传播 vs 并发 seal（延迟触发 + 幂等判定）；
- 多核乱序分配窗口（锁内分配约束消除）；
- 递归 JobKill 组合的收敛性（逐层 seal 先行 ⇒ 各层成员集单调收缩）；
- 派生兜底链：control 消散进程的枚举可见性（Building 入表 / Dead 移表的窗口两端）。

## 五、与 step 6–10 的衔接

- **step 6（pm 监督）**：本设计即 pm 的机制面——libprocess 提供 JobKill 组合的公共实现（逐层 seal → 枚举 → 派生 kill → drain → 等 CLOSED），init/pm 消费，服务不自写遍历。F4 预算决策（D7）同步落 ideas。
- **step 7（ThreadSpawn 屏障）**：JobDerive/JobEnumerate 的用户写回与既有 syscall 写回同属 KNOWN_ISSUES「写回 panic 面」——单线程下无虞，多线程前随统一修复（锁内复检），不单独处理。
- **step 8（D64 eligibility）**：无耦合。
- **step 9（验证矩阵）**：第四节清单并入。
- **step 10（文档）**：`notes/impls/task.md` 补 Job 管理面实现记录；`notes/ideas/task.md` 按 D3/D7 精确化 seal 传播与预算表述。

## 六、待拍板清单

| # | 决策 | 推荐 |
|---|---|---|
| D1 | 枚举形态：游标分页 vs 缓冲数组 | 游标分页 |
| D2 | 派生原语：按 ID 单目标 + rights 交集单调 | 如文 |
| D3 | seal 只封创建（上行检查）；完成 = 自身 sealed && empty；递归归用户态 | 如文 |
| D4 | 引入 JobId；派生兜底由 JobDerive 天然覆盖 | 如文 |
| D5 | 枚举/派生 rights：READ+MANAGE 复用 vs 新增 ENUMERATE 位 | 复用 |
| D6 | JobQuery 最小面（40B 快照，计数为非精确近似值） | 如文 |
| D7 | F4 预算：补一句话契约 vs 删词 | 补契约 |
