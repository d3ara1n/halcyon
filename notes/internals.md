# 内核内部机制

对外行为的设计（任务模型、IPC、FAL）见各专题篇；本篇记录内核内部的结构性决策。

## 全局状态分层

按归属与访问频率分三层管理状态，目标是热路径无锁，而非处处无锁。

| 层 | 归属 | 访问方式 | 典型内容 |
|---|---|---|---|
| hart 私有 | 单个 hart | tp 指针，无锁 | 当前线程、运行队列、trap 上下文、hart 状态 |
| 对象私有 | 进程/线程 | 所属对象的锁、Arc 引用计数 | 内存布局、邮箱、子进程列表 |
| 全局 | 全系统 | `OnceLock` + Spinlock 粗锁 | 进程表、mount 表、设备表、帧分配器 |

冷路径的一把大锁在本系统规模（4-5 hart）下争用可忽略；无锁结构（slot array、RCU 等）只在争用真实出现后才值得引入。

## tp 寄存器

内核态的 tp 不是 thread pointer，是 hart pointer。每个 hart 启动时设置 tp 指向自己的 HartLocal 结构，此后保持不变量：

**内核态运行期间，tp ≡ 当前 hart 的 HartLocal 地址。**

推论：

- trap 进出与上下文切换必须保存/恢复 tp（tp 即通用寄存器 x4，用户态用它做 TLS，内核态占用它）
- HartLocal 按 cache line 对齐；跨 hart 交互只能走全局层或 IPI，HartLocal 严格私有
- 数组包装处唯一一处 `unsafe impl Sync`，SAFETY 注明上述访问不变量

## 锁原语

内核自研 Spinlock：LR/SC 争用，持有期间关本地中断（sstatus.SIE 清零）。关中断是正确性要求而非优化——中断处理函数若获取本 hart 正持有的锁，同核死锁。

Spinlock 包装为 `lock_api::RawMutex`，同一实现注入两处：

- talc 堆分配器（TalcLock），堆分配纳入统一的中断纪律
- 全局容器（`OnceLock<Spinlock<T>>`）

睡眠锁在任务模型落地、出现可睡眠上下文后再引入。

## 堆分配器

talc。选择理由：

- TalcLock 的锁由内核注入（见锁原语）
- `claim` 支持注册 DTB 解析出的多段可用内存，堆可按需扩展
- 小对象开销低（每次分配 1×usize 元数据），适配 Arc/Vec 混合负载

堆分配不在热路径，不设 per-hart 堆，留作演进方向。

## 进程表

`ProcessTable` 封装 `OnceLock<Spinlock<BTreeMap<Pid, Arc<Process>>>>`，只暴露 get/insert/remove。pid 由 `AtomicU64` 单调递增分配，不复用。内部实现可替换（如 slot array），调用方无感。
