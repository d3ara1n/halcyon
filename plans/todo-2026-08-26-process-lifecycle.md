# 进程生命周期与终止屏障计划

## 目标

在现有 Job、ProcessBuilder、ProcessControl 基座上完成显式 ProcessKill/JobKill、状态查询与多线程安全的资源收束。终止必须统一覆盖 Ready、Running、Waiting 和多 hart 执行，不把关闭 capability 等同于政策动作，不以一进程一线程的偶然条件设计一次性接口。

## 当前基座

- ProcessBuilder 是 affine Building authority；ProcessStart 成功后永久消费。
- ProcessControl 可 duplicate/TRANSIT/GRANT，进程退出后发布 CLOSED；关闭 control 不终止目标。
- exit code 已进入轻量 control shell，但尚无固定宽查询 ABI。
- 当前一进程一线程，进程不持线程成员表；Waiting 所有权位于 WaitContext，Running 所有权位于 HartLocal，Ready 所有权位于调度类。
- D64 调度域 eligibility 尚未接线，ProcessStart 明确拒绝 D64。

## 设计前必须确认

### Process 状态与观察壳

定义 Building、Running、Terminating、Dead 的唯一真值与线性化点；明确 ProcessControl 查询的 fixed-width snapshot、exit reason/code、PID 与状态。Running 资源在何时与 Dead shell 解耦必须有单一所有权路径。

### Kill 仲裁

ProcessKill 是显式 MANAGE 操作；需要定义重复 kill、Exit/fault/kill 同时到达、目标已 Dead、调用者杀自身时的结果。Kill 只请求终止，真正 teardown 必须由不再运行目标地址空间的收束点完成。

### 各容器撤销

- Ready：从调度类取得线程所有权，或以统一终止标志在 pick 边界收束；
- Running：记录 active hart，IPI/remote call 请求离开用户态，并等待确认；
- Waiting：通过 WaitContext 的正式 Cancelled/Abandoned outcome 竞争并清理对象订阅与 deadline；
- 多线程：成员表与 active-hart bitmap 必须先于 ThreadSpawn 建立，最后线程离开后才能 drain Handles、清 PTE 和归还帧。

不得为单线程首版分别写三条不相容的 kill 快路径。

### Job 终止域

定义 Job 成员关系的引用方向、JobKill 的快照/并发创建边界、子 Job 传播和 authority 消散。JobControl close 继续只关闭 authority；JobKill 才是政策动作。

### 调度域 eligibility

把 ELF execution profile 转换为 compatible domain，明确无兼容 hart、运行中 capability 变化及迁移语义；完成后再接受 D64。

## 实施顺序

1. 调研成熟系统的 process/job kill、wait 与 dead-object shell，形成方向文档并确认状态机；
2. 建立 Process 线程成员表、active-hart 与 WaitContext cancellation 契约；
3. 实现 fixed-width ProcessQuery、ProcessKill 与 ProcessControl rights；
4. 实现 Job 成员记账与 JobKill；
5. 接入 ThreadSpawn 前完成多线程 teardown barrier 与远端 TLB/fence 协议；
6. 接入 capability-derived 调度域 eligibility，再开放 D64；
7. 对 Ready/Running/Waiting、自杀、重复 kill、并发 Exit/fault、最后 control 关闭做 host/virt 多核验证；
8. 同步 `notes/ideas/task.md`、`notes/impls/{task,execution-context,ipc}.md`。

## 完成标准

- 用户可通过显式 capability 终止进程/Job，关闭 control 永不隐式终止；
- 任意状态的目标最终只 teardown 一次，等待订阅、Handle、地址空间和线程容器无泄漏；
- teardown 前所有 active hart 已确认离开目标地址空间；
- Dead shell 可稳定查询终态，资源已释放且不保留 Process/Thread 环；
- 用户可触发的所有竞态只返回确定状态或完成终止，不 panic 内核。
