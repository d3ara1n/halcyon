# 进程/Job 终止、终态查询与等待语义取证（Zircon / POSIX/Linux / seL4）

> 取证日期：2026-08-26。来源限制：只记录官方文档/API 参考及指定官方源码载明的事实；未核实的点显式标注，不推断。本文件为 `ref-` 对照资料，只记录成熟系统的实际契约与立场，**不代行本项目的设计决策**。Windows NT 侧取证另见 [`ref-2026-08-windows-nt-semantics.md`](ref-2026-08-windows-nt-semantics.md)，本文对照表中 NT 列引用自该文件。

---

## 一、Zircon

### 1. `zx_task_kill` 的作用与返回语义

`zx_task_kill()` 的官方定义是："This asynchronously kills the given process or job and its children recursively, until the entire task tree rooted at handle is dead." 该调用接受 process 或 job handle，并递归终止目标 task tree；**不支持杀死 thread**。调用要求 handle 具有 `ZX_RIGHT_DESTROY`。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

因此，`zx_task_kill()` 是**异步发起终止**，不是在返回前确认整个目标树已经停止。函数返回 `ZX_OK` 只表示终止操作成功发起；官方明确要求通过 `ZX_TASK_TERMINATED` 等待 "procedure completes"，并指出观察到该信号时目标及其所有子对象才处于 dead state。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

当目标是 process 或 job 时，杀死范围包括目标及其后代。目标是 job 时，成功调用后该 job 不能再用于创建新 process。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

当进程使用 `zx_task_kill()` 杀死自身时，系统调用不返回；文档原文为："If a process uses this syscall to kill itself, this syscall does not return." [zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

官方 syscall reference 没有规定对已终止 process/job 重复调用 `zx_task_kill()` 必须返回哪一个具体错误码。因此，**重复 kill 及 kill 已终止对象的稳定返回值未能从限定官方来源核实**。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

杀死自己所在的 job 时，官方文档只明确说明该 job 及其子树异步终止，并未单独规定调用线程何时停止或调用是否返回。若调用者属于该 job，不能把"函数返回"解释为调用者继续运行的保证；该点的专门语义**未能从官方来源核实**。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

### 2. 终止观察、信号与等待

`ZX_TASK_TERMINATED` 是用于观察 task 完成终止的信号。`zx_task_kill()` 文档明确说明：当该信号观察到时，目标 task 及其所有 children 被认为处于 dead state，此后大多数操作不再成功。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

`zx_object_wait_one()` 在指定信号已经处于 active 状态，或在 deadline 前被观察到时返回 `ZX_OK`；deadline 到期返回 `ZX_ERR_TIMED_OUT`。等待句柄被关闭或失效时，返回 `ZX_ERR_CANCELED`。[zx_object_wait_one](https://fuchsia.dev/reference/syscalls/object_wait_one)

`zx_object_wait_async()` 本身是非阻塞调用；它为对象注册异步观察，当指定信号变为 active 时向 port 排入 packet，再由 `zx_port_wait()` 取出。对象句柄关闭时，关联的异步等待操作终止，但已经排入 port 的 packet 不会被移除。[zx_object_wait_async](https://fuchsia.dev/reference/syscalls/object_wait_async)

因此，process、job、thread 的终止观察都采用 Zircon 的对象信号与 wait 机制；对 process/job 使用 `ZX_TASK_TERMINATED` 可等待终止完成，thread 的普通对象状态也可通过 wait 机制观察。`zx_task_kill()` 本身不接受 thread handle。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)；[zx_object_wait_one](https://fuchsia.dev/reference/syscalls/object_wait_one)

Zircon 文档将 task 的终止与对象生命周期分开：handle 关闭会终止与其关联的异步 wait，但限定来源没有规定"进程终止后仍有句柄时对象是否必然保活"的完整对象生命周期条款。该点在本报告的限定证据范围内**未能完整核实**。[zx_object_wait_async](https://fuchsia.dev/reference/syscalls/object_wait_async)

### 3. 终态查询

`zx_object_get_info()` 的 `ZX_INFO_PROCESS` 返回：

```c
typedef struct zx_info_process {
    int64_t return_code;
    zx_instant_mono_t start_time;
    uint32_t flags;
} zx_info_process_t;
```

其中 `return_code` 仅在 `ZX_INFO_PROCESS_FLAG_EXITED` 置位时有效；`start_time` 仅在 `ZX_INFO_PROCESS_FLAG_STARTED` 置位时有效。[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)

**关键时点分离**：在 `zx_task_kill()` 之后，`ZX_INFO_PROCESS_FLAG_EXITED` 会立即报告为已设置，但 child threads 可能仍在退出。这说明"终态可查询"与"所有线程 teardown 已完成"是两个不同时点。[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)

`ZX_INFO_JOB` 返回：

```c
typedef struct zx_info_job {
    int64_t return_code;
    bool exited;
    bool kill_on_oom;
    bool debugger_attached;
} zx_info_job_t;
```

job 的 `return_code` 仅在 `exited` 为 true 时有效；文档明确说明，**kill job 是 job 退出的唯一方式**。调用 `zx_task_kill()` 后，`exited` 会立即为 true，但 child jobs/processes 仍可能处于退出过程中。[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)

`ZX_INFO_THREAD` 返回 thread 的当前状态、异常等待 channel 类型及 CPU affinity mask；这些字段目前主要用于信息和调试。[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)

通过 `zx_task_kill()` 杀死 process 或 job 时，`return_code` 为 `ZX_TASK_RETCODE_SYSCALL_KILL`。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

正常退出由 `zx_process_exit(retcode)` 提供 return code；该调用结束当前 process、不返回且不能失败，给定 code 可通过 `ZX_INFO_PROCESS` 查询。[zx_process_exit](https://fuchsia.dev/reference/syscalls/process_exit)

`zx_thread_exit()` 只结束当前调用 thread；它是 `[[noreturn]]` syscall，不返回且不能失败。官方文档没有在该页规定最后一个 thread 退出时 process return code 的全部来源规则。[zx_thread_exit](https://fuchsia.dev/reference/syscalls/thread_exit)

异常处理文档及 Zircon 定义包含 `ZX_TASK_RETCODE_EXCEPTION_KILL` 等异常终止编码；异常处理耗尽后，process 可因 exception kill 终止。因而 Zircon 的 return code 能区分 syscall kill 与 exception kill，但限定来源没有找到一份完整、稳定列举"正常退出、所有 fault 类型、所有 kill 来源"编码的单一 ABI 表，不能据此补全未列出的编码。[异常处理文档](https://fuchsia.googlesource.com/fuchsia/+/17dcb7cb44eb9e559aa1a79d4def4003812ca447/docs/concepts/kernel/exceptions.md)；[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)

### 4. Job 语义

Zircon job 用于组织、控制和限制 process；job 可拥有 child jobs 与 member processes，job 形成层级树，每个 process 属于一个 job。[Jobs](https://fuchsia.dev/fuchsia-src/concepts/process/jobs)

`zx_task_kill()` 对 job 的传播范围是以该 job 为根的 task tree，包括其子 jobs 及其中的 processes。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

`zx_job_set_policy()` 设置 job policy。新的 effective policy 会由 parent effective policy 与当前 policy 组合，并应用于以后创建的 child process 或 child job；`ZX_POL_ACTION_KILL` 表示 policy violation 时终止违规 process。[zx_job_set_policy](https://fuchsia.dev/reference/syscalls/job_set_policy)

在限定的 Fuchsia syscall reference、官方 concepts 文档和官方源码检索范围内，**未能核实名为 `JOB_POL_KILL_ON_CLOSE` 的 Zircon 公共 job policy**，也未找到"关闭最后一个 job handle 即杀死成员"的官方定义。可核实的相关机制是 `ZX_PROP_JOB_KILL_ON_OOM`/`kill_on_oom`，它针对 OOM，而不是 handle close。[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)；[zx_job_set_policy](https://fuchsia.dev/reference/syscalls/job_set_policy)

`ZX_JOB_NO_JOBS` 表示 job 当前没有 child jobs；官方 job 对象资料同时描述了 job 层级及 child-job 状态信号。该信号表达成员状态，不等同于 job 对象已经销毁。[Job kernel object](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/job)

官方资料表明 job 可无限嵌套；但本次限定来源没有找到足够明确的、可逐字引用的"无限深度"规范句子。因此，"无限嵌套"作为无实现上限的强断言**未能核实**；可以确认的是 Zircon job 是可嵌套的层级容器。[Jobs](https://fuchsia.dev/fuchsia-src/concepts/process/jobs)

### 5. 线程级终止与 pending 操作

`zx_task_kill()` 明确不支持 thread handle；Zircon 公开的 `zx_thread_exit()` 是当前 thread 自身退出，而非外部强制终止任意 thread 的 syscall。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)；[zx_thread_exit](https://fuchsia.dev/reference/syscalls/thread_exit)

因此，限定官方 API reference 不能支持"杀死单个线程后同一 process 的其他线程必然继续运行"或"必然导致整个 process 退出"等更具体结论。该行为**未能从限定来源核实**。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

`zx_channel_call()` 在等待 reply 时，peer 关闭可返回 `ZX_ERR_PEER_CLOSED`；调用 handle 被关闭或失效时可返回 `ZX_ERR_CANCELED`。但官方没有把 process/job kill 与每一种 pending channel call 的返回状态作一一对应规定。[zx_channel_call](https://fuchsia.dev/reference/syscalls/channel_call)

`zx_port_wait()` 等待 port packet，packet 到达时返回 `ZX_OK`，deadline 到期且没有 packet 时返回 `ZX_ERR_TIMED_OUT`。官方 syscall reference 没有规定 process/job teardown 对 pending port wait 的统一专门返回码。[zx_port_wait](https://fuchsia.dev/reference/syscalls/port_wait)

### 6. 资源 teardown 与共享对象

`zx_task_kill()` 文档只定义终止传播、异步完成点、return code 和后续操作限制，没有给出"先关闭 handle table、再释放 VMO、再处理跨进程对象"的公开 teardown 顺序。[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

`ZX_INFO_PROCESS_HANDLE_STATS` 可查询 process 当前持有的各类 handle 数量；官方资料也说明 process 可能仍持有对象的 live reference，即使已经没有对应 handle，例如运行中的 thread。[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)

对于跨进程共享的 kernel objects，限定来源没有规定"拥有该对象的 process 被 kill 后对象必然销毁"或"共享对象必然失效"。可以确认的是 handle 的关闭会影响 handle 及关联的异步 wait；具体共享对象的生命周期由该对象及其余引用决定，不能从 process kill 页面推导统一规则。[zx_object_wait_async](https://fuchsia.dev/reference/syscalls/object_wait_async)

### 7. Fault 与 debugger

Zircon exception handling 会在线程 fault 时暂停执行并生成 exception；异常可通过 thread、process 或 job exception channel 交给 handler。handler 可检查异常，关闭 exception handle 后由系统继续处理或转交后续 handler。[Exception handling](https://fuchsia.dev/fuchsia-src/concepts/kernel/exceptions)

若异常没有被处理，官方资料定义了 exception kill 路径及相应 task return code；这与显式 `zx_task_kill()` 使用的 `ZX_TASK_RETCODE_SYSCALL_KILL` 相区分。[异常处理文档](https://fuchsia.googlesource.com/fuchsia/+/17dcb7cb44eb9e559aa1a79d4def4003812ca447/docs/concepts/kernel/exceptions.md)；[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

`ZX_INFO_JOB` 的 `debugger_attached` 字段表示 job 是否附加 debugger。`zx_task_kill()` 页面没有说明 debugger 是否能阻止或改变显式 kill 的最终结果；该点**未能核实**。[zx_object_get_info](https://fuchsia.dev/reference/syscalls/object_get_info)；[zx_task_kill](https://fuchsia.dev/reference/syscalls/task_kill)

## 二、POSIX / Linux 对照

POSIX 的 wait status 应通过 `<sys/wait.h>` 宏解释。`WIFEXITED(status)` 表示正常退出，`WEXITSTATUS(status)` 返回传给 `_exit()`/`exit()` 或从 `main()` 返回值的低 8 位；`WIFSIGNALED(status)` 表示因未捕获 signal 终止，`WTERMSIG(status)` 返回终止 signal 编号。[Open Group wait](https://pubs.opengroup.org/onlinepubs/9799919799/functions/wait.html)

Linux 传统 wait status 在低 16 位编码：正常退出码位于 bits 8–15，终止 signal 位于低位，`0x80` 表示产生 core dump；这是 Linux 实现细节，POSIX 应用不应直接依赖位布局，而应使用上述宏。[Linux wait(2)](https://man7.org/linux/man-pages/man2/wait.2.html)

子进程终止后，Linux 会保留包含 PID、终止状态和资源使用信息的最小 zombie 记录；父进程执行 wait 后，内核才释放与该 zombie 关联的资源。[Linux wait(2)](https://man7.org/linux/man-pages/man2/wait.2.html)

`waitpid()` 在已有可报告状态时立即返回，否则阻塞；指定 `WNOHANG` 时，没有状态可报告则立即返回 0。`waitid()` 支持 `WNOHANG`，并可用 `WNOWAIT` 查看状态而不消费它。[Linux wait(2)](https://man7.org/linux/man-pages/man2/wait.2.html)；[waitid(2)](https://man7.org/linux/man-pages/man2/waitid.2.html)

`SIGKILL` 不能被捕获、阻塞或忽略；Linux `kill(2)` 成功只表示 signal 已按规则发送，不表示目标在该调用返回前已经完成退出。[Linux kill(2)](https://man7.org/linux/man-pages/man2/kill.2.html)；[Linux signal(7)](https://man7.org/linux/man-pages/man7/signal.7.html)

Linux 的 `exit_group()` 终止调用线程所属 process 中的全部线程，并以指定 status 结束该线程组；它与只结束调用线程的 `_exit()` 语义不同。[Linux exit_group(2)](https://man7.org/linux/man-pages/man2/exit_group.2.html)

## 三、seL4 对照

seL4 没有 Zircon/Windows 意义上的 process 或 job 对象；基本调度对象是 TCB，资源和控制权通过 capability 传递。TCB capability 可用于调用目标线程的 TCB 操作。[seL4 Reference Manual](https://sel4.systems/Info/Docs/seL4-manual-latest.pdf)；[seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html)

`seL4_TCB_Suspend` 挂起由目标 TCB capability 指定的线程；`seL4_TCB_Resume` 恢复线程。两者都是针对 TCB 的同步 capability invocation，不是递归终止 process/job 的操作。[seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html)

seL4 的 capability 删除操作是 `seL4_CNode_Delete`；它删除指定 CNode slot 中的 capability。`seL4_CNode_Revoke` 删除由指定 capability 派生出的 child capabilities。[seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html)

删除一个 TCB capability 与终止一个 Zircon process object 不是同一抽象：capability 删除撤销了该引用，但限定的官方 API 文档没有支持"删除 TCB capability 即自动 reset 并回收该 TCB 所有相关资源"的统一表述。[seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html)

在限定的官方 Manual/API 页面中，未能核实一个公开的、与 Zircon `zx_task_kill()` 对应的 TCB `Reset` 调用，也未找到统一的 process exit status、zombie 对象或 `ZX_TASK_TERMINATED` 式终止信号。上述内容不应从 capability deletion 推断。[seL4 Reference Manual](https://sel4.systems/Info/Docs/seL4-manual-latest.pdf)；[seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html)

## 四、关键分歧点对照

NT 列引用自 [`ref-2026-08-windows-nt-semantics.md`](ref-2026-08-windows-nt-semantics.md)。

| 分歧点 | Windows NT | Zircon | POSIX/Linux | seL4 |
|---|---|---|---|---|
| kill 返回语义 | `TerminateProcess` 对他进程异步（发起即返回）；自杀不返回；确认靠 `WaitForSingleObject` | `zx_task_kill()` 异步发起；`ZX_OK` 不等于目标树已完成死亡，需等待 `ZX_TASK_TERMINATED` | `kill()` 成功表示 signal 发送；不表示目标已完成退出 | 无统一 process/job kill 原语；TCB suspend/resume 是线程控制操作 |
| 终态查询与编码 | `GetExitCodeProcess` 未终止时输出哨兵 `STILL_ACTIVE`(259)（与用户退出码冲突的著名陷阱）；退出码 32 位自由值，不区分死因 | `ZX_INFO_PROCESS`/`ZX_INFO_JOB` 提供 fixed-width `return_code`(i64) 与 exited flags；syscall kill / exception kill 用保留编码区分 | wait status 经宏解释；Linux 布局为退出码 bits 8–15 + signal 低位 + core 位 | 无统一 process exit-status ABI |
| EXITED 可查询时点 | 对象 signaled 在终止完成时 | kill 后 `EXITED` flag 立即置位，**线程可能仍在退出**——终态可查询与 teardown 完成分离 | zombie 可 wait 时资源已收（reaped 前 PID/状态保留） | — |
| kill-on-close 机制位置 | job limit flags（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，句柄属性） | **无此公开 policy**（可核实的仅 `kill_on_oom`，针对 OOM） | 无对应机制 | 无对应 policy；capability 删除表达权利撤销 |
| 死进程对象保活 | 句柄保活；最后句柄关闭才销毁对象；终止后句柄仍可查询退出码/退出时间 | 终止后仍可经信号与查询观察终态；完整保活条款未核实 | zombie 保留最小终态，父进程 wait/reap 后释放 | 无进程尸体对象；生命周期由 capability 与 TCB/VSpace 对象管理 |
| 等待机制 | `WaitForSingleObject`（`SYNCHRONIZE` 权限）；进程对象终止时 signaled | 对象 signal（`ZX_TASK_TERMINATED`）+ `zx_object_wait_one()` / `wait_async`+port | `waitpid()`/`waitid()`；阻塞、`WNOHANG`、`WNOWAIT` | 无统一 process wait；由 notification/IPC 组合 |
| 与父子关系的耦合 | 终止进程不终止子进程；无 reaping 义务（对象生命周期由句柄计数决定） | 同左（task tree 显式 kill 才传播） | **强耦合**：zombie 等父 reaping；SIGCHLD 通知父 | — |

## 五、Zircon 源码级终止路径

### 1. 固定版本与证据边界

dispatcher 主路径固定于 Fuchsia commit [`7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a`](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/)：

- [`process_dispatcher.cc`](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/zircon/kernel/object/process_dispatcher.cc)
- [`job_dispatcher.cc`](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/zircon/kernel/object/job_dispatcher.cc)
- [`thread_dispatcher.cc`](https://fuchsia.googlesource.com/fuchsia/+/7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a/zircon/kernel/object/thread_dispatcher.cc)

该快照未能定位底层 `Thread::Kill()` 的完整函数体。逐调度状态的底层分支另以较早 commit [`9f04bc5272b7`](https://fuchsia.googlesource.com/fuchsia/+/9f04bc5272b7/zircon/kernel/kernel/thread.cc) 为证；这两部分不得混称为同一快照，也不能据旧函数体断言当前底层实现逐行不变。

### 2. Process 终止仲裁与新线程封口

`ProcessDispatcher::Kill(retcode)` 在 process 锁内检查状态。DEAD 直接返回；首次从非 DYING 状态进入终止时写入 `retcode_`，随后在线程列表非空时转换为 DYING、为空时直接转换为 DEAD。`SetStateLocked(DYING)` 调用 `KillAllThreadsLocked()`，后到的 kill/exit 不覆盖已经记录的 return code。

`ProcessDispatcher::AddInitializedThread()` 使用同一把 process 锁：首线程只可加入 INITIAL process，后续线程只可加入 RUNNING process；DYING/DEAD 不接受新线程。因此「RUNNING → DYING」与「加入新线程」在同一状态锁下形成明确先后，不存在 kill 已封口后又成功加入的线程。

### 3. 各执行状态的线程如何离场

`KillAllThreadsLocked()` 遍历 process 的线程集合调用 `ThreadDispatcher::Kill()`。dispatcher 对 RUNNING/SUSPENDED 线程调用底层 kill 并把 dispatcher 状态置为 DYING；DYING/DEAD 不重复处理。

commit `9f04bc5272b7` 的底层 `Thread::Kill()` 在全局 thread lock 下设置 kill signal，其推进方式按状态分化：

| 线程状态 | 源码行为 |
|---|---|
| 当前 CPU 正在运行 | 只置 kill signal，由当前线程在检查点自行退出 |
| 其他 CPU 正在运行 | 置 signal，并以 `mp_reschedule(...)` 请求目标 CPU 尽快进入调度检查点 |
| READY | 保持 runnable，下次运行时观察 kill signal |
| BLOCKED / BLOCKED_READ_LOCK | 可中断等待被唤醒并返回 killed 状态 |
| SLEEPING | 可中断 sleep 被解除 |
| SUSPENDED | 重新 unblock，使其恢复后处理 kill |
| INITIAL | 不在发送方远程销毁，留待后续启动/退出路径处理 |
| DEATH | 已死亡，不再处理 |

kill signal 最终由目标线程在安全点消费并调用当前线程退出路径。源码展示的是「置请求 → reschedule/解除等待 → 目标执行点自行退出」，不是发送 CPU 直接析构远端线程。

### 4. 最后线程、地址空间与终止信号

`ThreadDispatcher::ExitingCurrent()` 先把线程置为 DYING，完成退出调试通知与统计，再置为 DEAD、发布 `ZX_THREAD_TERMINATED`，释放底层 core thread 和 exceptionate，最后调用 `ProcessDispatcher::RemoveThread()`。线程先成为 DEAD，之后才从 process 成员集合移除。

`ProcessDispatcher::RemoveThread()` 在 process 锁内删除成员；列表变空时将 process 置为 DEAD，解锁后调用 `FinishDeadTransition()`。其可核实顺序为：

```text
最后线程 DEAD 并移出成员集合
→ process DEAD
→ 关闭 process/debug exceptionate
→ Destroy unified/restricted address space
→ 释放 shared process state
→ 发布 ZX_TASK_TERMINATED
→ 从父 Job 移除
```

因此 Zircon 的公开 terminated 信号晚于最后线程离场和地址空间销毁。源码没有独立的 process 级 active-CPU barrier；其证明链由每个线程在目标 CPU 安全退出、最后线程成员计数归零及后续地址空间销毁共同组成。

### 5. Job 递归封口与完成

`JobDispatcher::Kill(return_code)` 在 job 锁内只允许 READY → KILLING，写入 return code，并取得已有 child jobs/processes 的强引用；解锁后先递归 kill child jobs，再 kill child processes。

`AddChildJob()` 与 `AddChildProcess()` 在同一把 job 锁下要求状态仍为 READY。因此 READY → KILLING 是递归域的创建封口点：转换前已加入者一定进入已有成员集合，转换后加入失败。

Job 只有在 `state == KILLING && jobs_.empty() && procs_.empty()` 时才能转为 DEAD。`FinishDeadTransitionUnlocked()` 自底向上发布 `ZX_JOB_TERMINATED` 并从父树移除；child 尚未离开树时 parent 不能先完成。该源码事实支持「递归 kill 异步发起，终止信号表示整棵既有子树已经收束」，但不规定其他系统必须采用 Zircon 的具体树锁与引用结构。

## 六、Linux 源码级线程组退出对照

### 1. 固定版本

本节固定 Linux stable `v7.1.10`、commit [`8d4e6356173a7b2e4a6a8ee1669060c33528fdb9`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/)。Linux 是抢占式宏内核，只用于对照线程组离场、阻塞取消和地址空间回收，不把其 signal、zombie 或 tasklist 结构类推为本项目方案。

### 2. 线程组终因仲裁

[`do_group_exit()`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/kernel/exit.c#L1099) 在 `sighand->siglock` 下首次写入 `signal->group_exit_code`、设置 `SIGNAL_GROUP_EXIT` 并调用 `zap_other_threads()`；后到调用读取既有 group exit code，不再覆盖。

[`zap_other_threads()`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/kernel/signal.c#L1335) 不直接删除其他线程，而是向每个尚未退出的线程加入 pending `SIGKILL` 并调用 `signal_wake_up(t, 1)`。非 core-dump fatal signal 在 `complete_signal()` 中同样设置 group exit 状态、向线程组投递 SIGKILL 并唤醒成员。

### 3. Running、runnable 与 blocked 成员

[`signal_wake_up_state()`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/kernel/signal.c#L721) 先设置 `TIF_SIGPENDING`。普通 `wake_up_state()` 未推进目标时调用 `kick_process()`，使正在其他 CPU 运行的目标尽快经过信号检查点；目标仍由自己在 `get_signal()`/`do_group_exit()` 路径退出。

runnable 成员保持或进入运行队列，稍后消费 pending signal。`TASK_INTERRUPTIBLE` 等待可被 pending signal 打断；`TASK_KILLABLE` 通过 `TASK_WAKEKILL` 允许 fatal signal 打断；纯 `TASK_UNINTERRUPTIBLE` 不满足 signal 唤醒条件。因此即使是 SIGKILL，也不能保证处于不可中断内核等待、硬件 I/O 或关闭中断路径的线程立即完成退出。

### 4. 最后成员、mm 与 zombie 是不同状态机

[`do_exit()`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/kernel/exit.c#L901) 先经 `exit_signals()` 设置 PF_EXITING，再以 `atomic_dec_and_test(&signal->live)` 判断最后活跃线程，随后执行 `exit_mm()`、`exit_notify()` 和最终 task 停止。

[`exit_mm()`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/kernel/exit.c#L463) 在本 CPU 上把 `current->mm` 清空、进入 lazy TLB 状态并 `mmput(mm)`；最后一个 `mm_users` 引用才触发整体地址空间拆除。之后 `exit_notify()` 才在 `tasklist_lock` 下发布 `EXIT_ZOMBIE` 并通知 parent/pidfd，最终 wait/reap 再释放 PID 与 task 壳。

所以 PF_EXITING、最后活线程、离开 mm、mm_users 归零、zombie 可观察和最终 task 释放并非同一个时点。Linux 的 zombie/reap 依赖父子政策，不构成 capability dead shell 的直接先例。

### 5. RISC-V TLB 回收边界

[`exit_mmap()`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/mm/mmap.c#L1273) 以 mmu gather 批量解除映射和释放页表；`tlb_finish_mmu()` 先执行 TLB flush，再释放收集的页和页表内存。

[`arch/riscv/mm/tlbflush.c`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/stable/linux.git/+/8d4e6356173a7b2e4a6a8ee1669060c33528fdb9/arch/riscv/mm/tlbflush.c#L118) 以 `mm_cpumask(mm)` 定位可能缓存该地址空间的 CPU：仅本 CPU 时本地 flush；多 CPU 时使用 SBI remote fence，或以 `on_each_cpu_mask(..., 1)` 同步等待每个目标 CPU 执行本地 flush。页表 walker/RCU 读者的安全回收另有 SMP call 与 RCU 同步，不能把“已发送 IPI”当作“旧页表已可释放”。

## 七、跨系统可确认的共同边界

本轮源码级取证可确认以下共同事实，但不替本项目选择具体数据结构：

1. 终止请求不会由发送 CPU 直接析构远端 Running 线程；成熟 SMP 路径均先发布请求，再由目标执行点在安全边界离场。
2. Ready/runnable 与 Waiting/blocked 需要各自可达的推进路径；不能只给 Running hart 发 IPI，也不能假设所有等待天然可取消。
3. 新线程或新 child 的接入与终止封口必须共享一个可线性化状态边界，否则递归终止无法证明成员全集闭合。
4. 最后线程离场、地址空间不再被 CPU 使用、TLB 失效完成、物理页/页表释放和终态可观察具有明确先后，单一“killed”布尔值不足以证明资源安全。
5. Zircon 的 terminated 信号晚于最后线程和地址空间销毁；Linux 的 zombie 也晚于线程自己的 `exit_mm()`，但仍保留父进程 wait/reap 政策壳。
6. RISC-V 本地 `SFENCE.VMA` 不构成远端完成证明；释放地址空间前必须有覆盖相关 hart 的完成确认。
