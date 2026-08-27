# Job 成员枚举与 capability 派生跨系统契约取证（Zircon / Windows / seL4 / Linux）

> 取证日期：2026-08-27。来源限制：只记录官方文档/API 参考及官方源码载明的事实；未核实的点显式标注，不推断，不代行本项目的设计决策。本文件为 `ref-` 对照资料，只记录成熟系统的实际契约与立场。源码引用固定于 Fuchsia commit [`7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a`](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/)（与 `ref-2026-08-task-termination-research.md` 同一快照）。设计决策见 [archived/todo-2026-08-27-job-management-design.md](archived/todo-2026-08-27-job-management-design.md)，已并入 [todo-2026-08-26-process-lifecycle.md](todo-2026-08-26-process-lifecycle.md)。

---

## 一、Zircon

### 1. `zx_object_get_child` 完整契约

官方定义：[zx_object_get_child](https://fuchsia.dev/reference/syscalls/object_get_child)（页面最后更新 2025-03-04）。

- **签名**：
  ```c
  zx_status_t zx_object_get_child(zx_handle_t handle, uint64_t koid,
                                  zx_rights_t rights, zx_handle_t* out);
  ```
- **koid 参数语义**：官方原文 "attempts to find a child of the object referred to by _handle_ which has the kernel object id specified by _koid_. If such an object exists, and the requested _rights_ are not greater than those provided by the _handle_ to the parent, a new handle to the specified child object is returned." 即**按 koid 精确匹配直接子对象**，找到即返回一个新 handle。
- **对 job handle 返回什么对象**：官方原文 "If the object is a _Process_, the _Threads_ it contains may be obtained by this call. If the object is a _Job_, its (immediate) child _Jobs_ and the _Processes_ it contains may be obtained by this call." 即对 job handle 调用，返回的是 **直接子 job 或直接 member process** 的 handle，不含孙级；对 process handle 返回 thread。
- **返回 handle 的 rights 规则**：既非继承也非固定铸造，而是**调用者指定、受 parent handle 权限约束**。官方原文 "the requested _rights_ are not greater than those provided by the _handle_ to the parent"；且 "_rights_ may be `ZX_RIGHT_SAME_RIGHTS` which will result in rights equivalent to the those on the _handle_"（这里的 handle 指 parent）。
- **rights 要求**：_handle_ 必须具有 `ZX_RIGHT_ENUMERATE`。
- **错误码集合**（官方 Errors 段）：`ZX_ERR_BAD_HANDLE`（handle 无效）；`ZX_ERR_WRONG_TYPE`（handle 不是 Process/Job/Resource）；`ZX_ERR_ACCESS_DENIED`（handle 缺 `ZX_RIGHT_ENUMERATE`，或 _rights_ 指定的权限不在 handle 上）；`ZX_ERR_NOT_FOUND`（handle 没有 koid 对应的子对象）；`ZX_ERR_NO_MEMORY`；`ZX_ERR_INVALID_ARGS`（_out_ 无效指针）。

### 2. koid 的稳定性

官方定义：[Zircon Kernel Concepts](https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts) 的 "Kernel Object IDs" 一节。

- **不复用**：官方原文 "Every object in the kernel has a 'kernel object id' or 'koid' for short. It is a 64 bit unsigned integer that can be used to identify the object and is **unique for the lifetime of the running system**. This means in particular that **koids are never reused**."
- **特殊值与位宽**：官方 `types.h` 定义 `ZX_KOID_INVALID`(0)、`ZX_KOID_KERNEL`(1)、`ZX_KOID_FIRST`(1024，"The first 1024 are reserved")，并注明内核生成的 koid 只用 63 位（最高位留给人工分配的 artificial koid）。[zircon/system/public/zircon/types.h](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/public/zircon/types.h)
- **单调性不保证**：官方原文 "**The sequence in which kernel generated koids are allocated is unspecified and subject to change.**" —— 官方只承诺「系统生命周期内唯一、不复用」，**不承诺单调递增**。
- **对「枚举→派生」的 TOCTOU 含义**：以下为由上述两条已载明契约推出的直接后果（推导，非原文引句）：koid 不复用 ⇒ 枚举得到的 koid 永远指向当初那个对象，不会因 PID/koid 复用而命中另一个对象；若该子对象已在两次调用之间消亡，`zx_object_get_child` 返回 `ZX_ERR_NOT_FOUND`（见第 1 条错误码），而不会错发给一个新对象。官方文档**没有**承诺"枚举结果在后续调用中必然仍存在"（即成员可消亡），也没有提供"原子枚举+派生"的单一原语——该点**未能核实存在**。

### 3. `ZX_INFO_JOB_CHILDREN` / `ZX_INFO_JOB_PROCESSES`

官方定义：[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)。

- **返回内容**：官方原文 "`ZX_INFO_JOB_CHILDREN`: Returns an array of `zx_koid_t`, one for each direct child Job of the provided Job handle."；"`ZX_INFO_JOB_PROCESSES`: Returns an array of `zx_koid_t`, one for each direct Process of the provided Job handle." 即返回**直接子 job / 直接子 process 的 koid 数组**（不含孙级、不含对象其余属性）。
- **buffer_size/actual/avail 三值语义**（官方通用段）：_buffer_ 为大小 `buffer_size` 的输出缓冲；_actual_ 返回**实际写入 buffer 的记录数**；_avail_ 返回**可读记录总数**。"If the buffer is insufficiently large, _avail_ will be larger than _actual_." 对固定记录数的 topic 缓冲不足返回 `ZX_ERR_BUFFER_TOO_SMALL`；对变长数组 topic（本两项属此类）则靠 `avail > actual` 指示截断。
- **扩容模式**：官方未为 job 枚举写专门章节；文档给出的通用契约是读 _avail_ 并按需扩容重试。**游标/分页**：syscall 签名无游标参数，官方**未提供** job 枚举的分页/游标机制（未能核实存在）。
- **多次调用间一致性承诺**：官方对这两个 topic **没有**一致快照或顺序承诺（未能核实）。对照证据：官方对 `ZX_INFO_PROCESS_THREADS` 明确警告 "Getting the list of threads is **inherently racy**... an external thread can create new threads."；而对 `ZX_INFO_HANDLE_TABLE` 明确承诺 "The kernel ensures that the handles returned are consistent." 本两项 job topic 均**未**得到这类承诺，且未声明数组顺序。
- **rights 要求**（官方 Rights 段）：`ZX_INFO_JOB_CHILDREN` 与 `ZX_INFO_JOB_PROCESSES` 均要求 handle 为 `ZX_OBJ_TYPE_JOB` 且具有 `ZX_RIGHT_ENUMERATE`；`ZX_INFO_JOB`（job 自身信息）要求 `ZX_RIGHT_INSPECT`。

### 4. job / process handle 的 rights 模型

- **`zx_job_create` 返回的 child job handle 的 rights**：syscall 页本身未载明，官方源码 `JobDispatcher::Create()` 在成功路径显式 `*rights = default_rights();`——即**新 handle 取得该对象类型的默认 rights，与 parent handle 的 rights 无关**。[job_dispatcher.cc@7dedc3f2](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/zircon/kernel/object/job_dispatcher.cc) 官方 `rights.h` 定义：
  ```c
  #define ZX_DEFAULT_JOB_RIGHTS   \
    (ZX_RIGHTS_BASIC | ZX_RIGHTS_IO | ZX_RIGHTS_PROPERTY | ZX_RIGHTS_POLICY | \
     ZX_RIGHT_ENUMERATE | ZX_RIGHT_DESTROY | ZX_RIGHT_SIGNAL | \
     ZX_RIGHT_MANAGE_JOB | ZX_RIGHT_MANAGE_PROCESS | ZX_RIGHT_MANAGE_THREAD)
  ```
  [rights.h](https://fuchsia.googlesource.com/fuchsia/+/main/zircon/system/public/zircon/rights.h)（`ZX_DEFAULT_PROCESS_RIGHTS` 类似，不含 `ZX_RIGHTS_POLICY`/`MANAGE_JOB`）。
- **文档不一致需注明**：concepts 页 [rights](https://fuchsia.dev/fuchsia-src/concepts/kernel/rights) 将 `ZX_RIGHT_MANAGE_JOB`/`MANAGE_PROCESS`/`MANAGE_THREAD` 标为 "**NOT YET IMPLEMENTED**"；但 syscall 参考 [zx_job_create](https://fuchsia.dev/reference/syscalls/job_create) 明确要求 `ZX_RIGHT_MANAGE_JOB`（错误 `ZX_ERR_ACCESS_DENIED` 当缺 `ZX_RIGHT_WRITE` 或 `ZX_RIGHT_MANAGE_JOB`），[zx_process_create](https://fuchsia.dev/reference/syscalls/process_create) 明确要求 `ZX_RIGHT_MANAGE_PROCESS`。两处官方文档互相矛盾，以 syscall 参考为准并按原文记录双方。
- **`zx_handle_duplicate` 的单调收窄规则**：[zx_handle_duplicate](https://fuchsia.dev/reference/syscalls/handle_duplicate) 官方原文 "creates a duplicate of _handle_, referring to the same underlying object, with new access rights _rights_"；"If different rights are desired they must be **strictly lesser** than of the source handle. It is possible to specify no rights by using `ZX_RIGHT_NONE`." 源 handle 需 `ZX_RIGHT_DUPLICATE`；请求的 rights 非源 handle rights 子集时返回 `ZX_ERR_INVALID_ARGS`。
- **handle transfer 时 rights 是否保留**：普通转移保留 rights；[zx_channel_write_etc](https://fuchsia.dev/reference/syscalls/channel_write_etc) 可在转移时收窄/移除——官方原文 "All source handles must have `ZX_RIGHT_TRANSFER`, but it can be removed in _rights_ so that it is not available to the message receiver"；并说明移除 `ZX_RIGHT_TRANSFER` 后 "the reader of the message will receive a handle that cannot be written to any other channel, but still can be using according to its rights and can be closed if not needed." 另 `zx_handle_duplicate` 页提示 "To remove `ZX_RIGHT_DUPLICATE` right when transferring through a channel, use `zx_channel_write_etc()`."

### 5. job 的子对象创建封口

- **官方 syscall 参考**：[zx_job_create](https://fuchsia.dev/reference/syscalls/job_create) 错误 `ZX_ERR_BAD_STATE: The parent job object is in the dead state.`；[zx_process_create](https://fuchsia.dev/reference/syscalls/process_create) 错误 `ZX_ERR_BAD_STATE: The job object is in the dead state.`（官方文档**只载明 dead state**，未提及 killing）。
- **源码层面**（[job_dispatcher.cc@7dedc3f2](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/zircon/kernel/object/job_dispatcher.cc)）：`AddChildProcess()`/`AddChildJob()` 均 `if (state_ != State::READY) return false;`；`JobDispatcher::Create` 中 `AddChildJob` 失败即 `return ZX_ERR_BAD_STATE`。job 状态机只有 `READY → KILLING → DEAD`（`Kill()` 中 `if (job->state_ != State::READY) return false; ... job->state_ = State::KILLING;`）。因此 **KILLING 状态下创建 child job/process 同样失败并返回 `ZX_ERR_BAD_STATE`**——这一条属源码确认，syscall 文档只载明 DEAD 情形。封口点是线性化的：`READY→KILLING` 转换与 `AddChild*` 在同一把 job 锁下，先加入者一定进入既有成员集合，后加入者失败。

### 6. `ZX_JOB_TERMINATED` 信号语义与等待方式

- **定义**：官方 [types.h](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/public/zircon/types.h) 中 `#define ZX_JOB_TERMINATED __ZX_OBJECT_SIGNALED`（bit 3，与 `ZX_TASK_TERMINATED` 同值）；另有 `ZX_JOB_NO_JOBS`(=SIGNAL_4)、`ZX_JOB_NO_PROCESSES`(=SIGNAL_5)；job 构造时初始置 `ZX_JOB_NO_PROCESSES | ZX_JOB_NO_JOBS | ZX_JOB_NO_CHILDREN`（[job_dispatcher.cc@7dedc3f2](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/zircon/kernel/object/job_dispatcher.cc) 构造器）。
- **语义**：Fuchsia 源码注释（fuchsia-zircon-types 绑定据此生成，[docs.rs](https://docs.rs/fuchsia-zircon-types/latest/fuchsia_zircon_types/constant.ZX_JOB_TERMINATED.html) 引用的 fuchsia 提交 [9c86250915cbe](https://fuchsia.googlesource.com/fuchsia/+/9c86250915cbe0b4d5a1a6443286df6c56221508/)）载明该信号表示 "a job has been killed and **all of its children have completed cleanup**"。源码确认发布时点：`FinishDeadTransitionUnlocked()` 中 job 状态置为 `DEAD` 时 `UpdateStateLocked(0u, ZX_JOB_TERMINATED)`；该函数仅在 `state_ == KILLING && jobs_.is_empty() && procs_.is_empty()`（`IsReadyForDeadTransitionLocked()`）且自底向上逐级离开父树后执行，故 **`ZX_JOB_TERMINATED` 晚于整棵既有子树的收束**（与 `ref-2026-08-task-termination-research.md` 第五节结论一致）。
- **与 `ZX_INFO_JOB.exited` 不同步**：源码 `GetInfo()` 中 `.exited = (state_ == DEAD) || (state_ == KILLING)`——kill 发起后 `exited` 立即为 true（官方 [zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info) 亦注明 "|exited| will immediately report that the job has exited following a |zx_task_kill|... but child jobs and processes may still be in the process of exiting"），早于 `ZX_JOB_TERMINATED`。即「终态可查询」与「teardown 完成」是不同时点。
- **等待方式**（三种，均为官方 syscall）：
  - [zx_object_wait_one](https://fuchsia.dev/reference/syscalls/object_wait_one)：阻塞，deadline 前信号 active 或 deadline 到期；返回 `ZX_OK` / `ZX_ERR_TIMED_OUT` / `ZX_ERR_CANCELED`（等待中句柄被关）。
  - [zx_object_wait_many](https://fuchsia.dev/reference/syscalls/object_wait_many)：多对象同时等待，每项需 `ZX_RIGHT_WAIT`，上限 `ZX_WAIT_MANY_MAX_ITEMS`(64)；`ZX_ERR_CANCELED` 时对应项的 `pending` 置 `ZX_SIGNAL_HANDLE_CLOSED` 位。
  - [zx_object_wait_async](https://fuchsia.dev/reference/syscalls/object_wait_async) + [zx_port_wait](https://fuchsia.dev/reference/syscalls/port_wait)：非阻塞订阅，packet 类型 `ZX_PKT_TYPE_SIGNAL_ONE`（`zx_packet_signal_t`，含 trigger/observed）；handle 需 `ZX_RIGHT_WAIT`、port 需 `ZX_RIGHT_WRITE`；handle 关闭终止关联的 wait 操作但已入队 packet 保留；`zx_port_cancel` 可按 key 撤销并清已入队 packet。

### 7. A 节未核实清单

- 官方未承诺 `ZX_INFO_JOB_CHILDREN/PROCESSES` 的一致快照或稳定顺序；无游标/分页机制。
- 官方不存在「枚举+派生」原子原语；两次调用之间子对象可能消亡（此时 get_child 返回 `NOT_FOUND`，koid 不复用保证不会错指新对象）。
- koid 单调递增**不保证**（官方明示分配顺序未规定且可变）。
- syscall 文档仅载明 dead state 下创建失败；KILLING 下同样失败（`BAD_STATE`）为源码级确认。

## 二、Windows Job Object

### 8. `QueryInformationJobObject` / `JobObjectProcessList`

- [QueryInformationJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject)（jobapi2.h）：_hJob_ 需 `JOB_OBJECT_QUERY` 访问权；_hJob_ 可为 NULL，此时查询调用进程关联的 job（嵌套时取 immediate job）。信息类 `JobObjectBasicProcessIdList`（值 3）。
- 返回结构 [JOBOBJECT_BASIC_PROCESS_ID_LIST](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_process_id_list)（winnt.h）：
  ```c
  typedef struct _JOBOBJECT_BASIC_PROCESS_ID_LIST {
    DWORD     NumberOfAssignedProcesses;
    DWORD     NumberOfProcessIdsInList;
    ULONG_PTR ProcessIdList[1];   // variable-length
  } JOBOBJECT_BASIC_PROCESS_ID_LIST;
  ```
  官方对字段的说明：`NumberOfAssignedProcesses` "The number of process identifiers to be stored in **ProcessIdList**"；`NumberOfProcessIdsInList` "The number of process identifiers returned in the **ProcessIdList** buffer. **If this number is less than NumberOfAssignedProcesses, increase the size of the buffer to accommodate the complete list.**"；`ProcessIdList[1]` "A variable-length array of process identifiers returned by this call. Array elements 0 through **NumberOfProcessIdsInList**–1 contain valid process identifiers."
- **返回形态**：一次调用填充一个**可变长 PID 数组**（头部 + 数组按需分配）；**检测截断的官方机制是计数比较**（`NumberOfProcessIdsInList < NumberOfAssignedProcesses` 即扩容重试）。官方文档**未要求** job 处于特定状态即可查询；`ERROR_MORE_DATA` 作为该调用的扩展错误码**未能从本次抓取的官方页面核实**（官方载明的是计数比较法）。
- **嵌套 job 的枚举范围**：结构页官方原文 "If the job is nested, the process identifier list consists of **all processes associated with the job and its child jobs**." —— 与 Zircon 的「直接 child」不同，Windows 枚举的是**本 job + 全部后代 job 的所有进程**。

### 9. `IsProcessInJob` / `AssignProcessToJobObject`（简要对照）

- [IsProcessInJob](https://learn.microsoft.com/en-us/windows/win32/api/jobapi/nf-jobapi-isprocessinjob)（jobapi.h）：_ProcessHandle_ 需 `PROCESS_QUERY_INFORMATION` 或 `PROCESS_QUERY_LIMITED_INFORMATION`（Windows Server 2003/XP 仅前者）；_JobHandle_ 非 NULL 时需 `JOB_OBJECT_QUERY`；_JobHandle_ 为 NULL 时判定进程是否处于任意 job 下。返回 _Result_ 指针指示是否在该 job 内。
- [AssignProcessToJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject)（jobapi2.h）：_hJob_ 需 `JOB_OBJECT_ASSIGN_PROCESS`；_hProcess_ 需 `PROCESS_SET_QUOTA` 与 `PROCESS_TERMINATE`。进程已关联某 job 时，新 job 必须为空或位于既有嵌套 job 层级内，且不得设置 UI limits（官方原文 "the job specified by _hJob_ must be empty or it must be in the hierarchy of nested jobs to which the process already belongs, and it cannot have UI limits set"）。

## 三、seL4

### 10. job/进程组对象与「按 ID 枚举派生」原语；CNode 派生的 rights 单调性

- **不存在 job/进程组对象**：[seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html) 按对象类型组织方法（`seL4_CNode`、`seL4_TCB`、`seL4_Untyped`、`seL4_IRQControl`、`seL4_SchedControl` 等），**没有 job、进程组或「按名字/ID 枚举派生 capability」的条目**；对 "job"/"process group" 在 API 参考全文检索无命中。seL4 教程 [Threads](https://docs.sel4.systems/Tutorials/threads) 及手册明确 TCB 是线程执行的基本抽象，seL4 "does not provide a high-level job abstraction"。seL4 无统一 process exit status / zombie / terminated 信号（与 `ref-2026-08-task-termination-research.md` 第三节一致）。
- **CNode 派生原语**（官方 [seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html) 与手册源码 [cspace.tex](https://github.com/seL4/seL4/blob/master/manual/parts/cspace.tex)）：
  - `seL4_CNode_Copy`："Copy a capability, **setting its access rights** whilst doing so" —— 新 capability 保持源 capability 的 **badge 与 guard 不变**。
  - `seL4_CNode_Mint`："Copy a capability, **setting its access rights and badge** whilst doing so" —— 可指定新 badge（32 位平台高 4 位忽略）与 rights。
  - 其余：`Move`、`Mutate`（move 并降 rights，不复制）、`Rotate`、`Delete`、`Revoke`（删除全部派生 child）、`SaveCaller`、`CancelBadgedSends`。
- **rights 单调性（官方原文）**：手册 "Capability Rights" 节——"When an object is first created, the initial capability that refers to it carries the maximum set of access rights. Other, less-powerful capabilities may be manufactured from this original capability, using methods such as `seL4_CNode_Mint` and `seL4_CNode_Mutate`. **If a greater set of rights than the source capability is specified for the destination capability in either of these invocations, the destination rights are silently downgraded to those of the source.**" 即派生 rights **不能增加，超限被静默降级到源水平**。
- **badge 变化规则**：手册 [ipc.tex](https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex) 载明，已带 badge 的 endpoint capability "cannot be unbadged, rebadged, or used to create child capabilities with different badges"；Mint 只能从 unbadged 端点制造 badged cap，或调整既有 cap 的 rights。Notification 的 badging 类似。badge 通过 Mint 施加、通过 Copy 保留。
- **派生树（CDT）**：Copy/Mint 产生的派生 capability 是原 capability 的 CDT child；`Revoke` 删除选定 capability 的所有派生 child；只有 original capability 支持派生（例外类型表见手册 "Capability Derivation" 节）。
- 说明：seL4 Reference Manual 的 PDF 本次未能直接解析，以上以 seL4 官方 GitHub 仓库中的 manual LaTeX 源（`manual/parts/cspace.tex`、`ipc.tex`、`objects.tex`）与 [docs.sel4.systems](https://docs.sel4.systems/) 为准。

## 四、Linux（仅竞态对照）

### 11. `pidfd_open` 的设计动机

- 官方 [pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)（man-pages 6.18）："obtain a file descriptor that refers to a task"；返回 FD 带 close-on-exec；`PIDFD_NONBLOCK`(5.10)、`PIDFD_THREAD`(6.9)；`ESRCH` 当 _pid_ 不存在。
- **官方竞态描述**（[pidfd_send_signal(2)](https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html) NOTES）："The **pidfd_send_signal()** system call allows the avoidance of **race conditions that occur when using traditional interfaces (such as kill(2)) to signal a process**. The problem is that the traditional interfaces specify the target process via a process ID (PID), with the result that the sender may accidentally send a signal to the wrong process if the originally intended target process has terminated and **its PID has been recycled for another process**. By contrast, a PID file descriptor is a **stable reference to a specific process**; if that process terminates, **pidfd_send_signal()** fails with the error **ESRCH**."
- **消除竞态的机制**：pidfd 持有对确切 task 的引用而非数字 PID；发送/等待均以引用为准，目标消亡后返回 `ESRCH`，不会命中被复用的新进程。pidfd_open(2) NOTES 进一步保证：子进程在 `pidfd_open()` 调用时已经终止，其 PID 也不会被回收，"the returned file descriptor will refer to the resulting zombie process"（该保证受 SIGCHLD 处置未被设为 SIG_IGN / 未设 SA_NOCLDWAIT / 未被提前 reap 约束；否则应改用 `clone(CLONE_PIDFD)`）。man 页同时指出替代法「打开 /proc/PID 目录」得到的是不可 poll、不可 `waitid()` 的引用。
- 对照含义（官方文字，不代行设计）：传统「readdir /proc + kill(PID)」的竞态根源是**数字 PID 复用**；稳定引用（FD/capability）把「目标身份」与「编号生命周期」解耦。这与 Zircon koid 不复用、seL4 capability 即引用的思路同属官方载明事实。

## 五、跨系统要点对照

| 维度 | Windows NT | Zircon | seL4 | Linux |
|---|---|---|---|---|
| 枚举面 | `QueryInformationJobObject(JobObjectProcessList)`：本 job + 后代 job 全部 PID 一次填充，计数比较法扩容 | `ZX_INFO_JOB_CHILDREN/PROCESSES`：仅直接 child 的 koid 数组，`actual`/`avail`，无快照承诺、无分页 | 无枚举面；无 job/进程组对象 | 数字 PID 面（/proc）+ 复用竞态（官方明文） |
| 按 ID 派生 handle/cap | — | `zx_object_get_child(koid, rights)`，rights 调用者指定且受 parent handle 约束；koid 不复用 | 无按 ID 派生；capability 经 CNode Copy/Mint/Move 派生，rights 单调不增（超限静默降级），badge 经 Mint 施加 | `pidfd_open(pid)` 一次性把数字 PID 转稳定引用 |
| 稳定引用 | 句柄（`JOB_OBJECT_QUERY` 等） | koid + handle（rights 随 handle） | capability（badge 携带身份信息） | pidfd |
| 封口 | — | `READY→KILLING→DEAD` 状态机 + 同锁 AddChild，`ZX_ERR_BAD_STATE` | capability 派生树 Revoke | — |
| 终态信号时点 | — | `ZX_JOB_TERMINATED` 晚于整棵子树收束；`exited` 早于该信号 | 无 | `EPOLLIN`(zombie)/`EPOLLHUP`(reap) |

## 六、来源清单

**Zircon**
- [zx_object_get_child](https://fuchsia.dev/reference/syscalls/object_get_child)
- [zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)
- [zx_job_create](https://fuchsia.dev/reference/syscalls/job_create)
- [zx_process_create](https://fuchsia.dev/reference/syscalls/process_create)
- [zx_handle_duplicate](https://fuchsia.dev/reference/syscalls/handle_duplicate)
- [zx_channel_write_etc](https://fuchsia.dev/reference/syscalls/channel_write_etc)
- [zx_object_wait_one](https://fuchsia.dev/reference/syscalls/object_wait_one)、[zx_object_wait_many](https://fuchsia.dev/reference/syscalls/object_wait_many)、[zx_object_wait_async](https://fuchsia.dev/reference/syscalls/object_wait_async)
- [Zircon Kernel Concepts（koid 段落）](https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts)
- [Rights（concepts）](https://fuchsia.dev/fuchsia-src/concepts/kernel/rights)
- [Zircon Signals（concepts）](https://fuchsia.dev/fuchsia-src/concepts/kernel/signals)
- [Job kernel object](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/job)、[Zircon Kernel objects（对象生命周期）](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects)
- 官方源码：[job_dispatcher.cc@7dedc3f2](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/zircon/kernel/object/job_dispatcher.cc)、[rights.h](https://fuchsia.googlesource.com/fuchsia/+/main/zircon/system/public/zircon/rights.h)、[types.h](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/public/zircon/types.h)、fuchsia 提交 [9c86250915cbe（ZX_JOB_TERMINATED 注释）](https://fuchsia.googlesource.com/fuchsia/+/9c86250915cbe0b4d5a1a6443286df6c56221508/)

**Windows**
- [QueryInformationJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject)
- [JOBOBJECT_BASIC_PROCESS_ID_LIST](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_process_id_list)
- [IsProcessInJob](https://learn.microsoft.com/en-us/windows/win32/api/jobapi/nf-jobapi-isprocessinjob)
- [AssignProcessToJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject)

**seL4**
- [seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html)
- 手册 LaTeX 源码：[cspace.tex](https://github.com/seL4/seL4/blob/master/manual/parts/cspace.tex)、[ipc.tex](https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex)、[objects.tex](https://github.com/seL4/seL4/blob/master/manual/parts/objects.tex)
- [Threads（seL4 docs 教程）](https://docs.sel4.systems/Tutorials/threads)

**Linux**
- [pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)
- [pidfd_send_signal(2)](https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html)

## 七、Gaps（未能核实/需注意）

1. `ZX_INFO_JOB_CHILDREN/PROCESSES` 的**一致快照/顺序承诺**：官方文档未作承诺（仅有 `PROCESS_THREADS` 的 racy 警告与 `HANDLE_TABLE` 的 consistency 承诺可对照）。
2. job 枚举的**游标/分页机制**：官方无此接口。
3. **"枚举+派生"原子原语**：官方 syscall 集不存在。
4. koid **单调递增**：官方明示不保证（分配顺序未规定且可变）。
5. `ZX_RIGHT_MANAGE_JOB/PROCESS/THREAD`：concepts rights 页标 "NOT YET IMPLEMENTED" 与 syscall 参考明确要求的矛盾，按原文双向记录。
6. Windows `QueryInformationJobObject` 的 **`ERROR_MORE_DATA`** 扩展错误码：未从本次抓取的官方页面核实，官方载明的检测法是计数比较。
7. seL4 Reference Manual 的 **PDF 未能直接解析**，正文以官方 GitHub 仓库 manual LaTeX 源 + docs.sel4.systems 为准（手册各版本 PDF 文字结论与之一致，但未逐行核对版本差异）。
8. `ZX_JOB_TERMINATED` 的语义注释引用自 Fuchsia 源码注释（经 docs.rs 绑定及 fuchsia 提交 9c86250915cbe 确认），未在单一"规范文件"中逐字定位其当前路径。
