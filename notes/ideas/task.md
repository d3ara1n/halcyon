# 任务模型

任务是对“受管理作业”的统称，内核中由 Job、进程和线程三种不同职责承载，不以一个万能 Task 结构混合资源、执行与政策。

## Job：创建域与资源预算

Job 是进程创建与资源核算的层级容器。root Job 由内核在初始 launch 中交给 init；持有相应 capability 的服务可以创建子 Job、设定预算并在该域内创建进程。

Job 表达配额、故障收束和管理域，不是用户身份或进程权限等级。设备、目录和服务访问仍由各自 capability 授权。

## 进程：独立资源环境

进程持有：

- 用户地址空间与内存布局；
- HandleTable；
- 线程成员关系；
- Job 归属与资源记账；
- 仅用于诊断的 PID、`parent_pid` 与退出信息。

进程不以“驱动级”“服务级”等 ambient 权限授权 syscall。创建者取得显式 Process Controller capability；观察者可持 Observer capability。创建关系本身不产生管理权。

进程生命周期应统一为：

```text
Building -> Running -> Terminating -> Dead
```

Building 阶段完成 ELF/地址空间、StartupBlock 与 GRANT 安装；ProcessStart 是唯一首次发布 runnable 的提交点。Terminating 阶段禁止新线程和新等待，撤销 Ready、取消 Waiting、收束各 hart 上的 Running 线程，完成地址空间失效后 drain Handles；最终发布持续可见的终态和 exit status。

## 线程：执行单元

线程属于且仅属于一个进程，持有独立 UserContext、栈、FP 状态和调度状态。只有线程参与调度；进程提供资源环境与执行需求，不是调度队列成员。

线程任意时刻恰处于一个所有权容器：

```text
某调度类 Ready 队列 | 某 hart current | 无容器（Waiting/Dead）
```

硬件 capability 决定线程可进入的调度域，调度类只表达选择策略；两者不得称为进程权限。

## 权利派生

创建子进程不隐式继承父进程权限。launcher 通过 ProcessStart 明确选择 opaque payload 与 GRANT entries；每项目标 rights 只能缩小。`parent_pid` 仅记录创建关系，与 capability 派生无关。

## 多线程终止边界

多线程进程必须维护成员关系与 active-hart 集合。进程回收要先阻止任何线程再次返回用户态，再取消 Ready/Waiting，等待 Running 全部退出，执行必要的远端 TLB invalidate/ack，最后才能回收页表和数据帧。单线程实现可以从简，但 ThreadSpawn 前必须具备该结构。
