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

## 内核中断模型

协作式内核：内核态全程关中断（trap 进入 S 态即 SIE=0，sret 回用户态才恢复），调度只发生在用户 trap 返回路径，内核没有嵌套执行流。内核态 timer trap 不视为致命：重设 tick 后原路返回，避免长内核路径误杀。

推论：内核锁只需防跨核争用；锁原语的关中断语义在此模型下是零成本保险，保留以防纪律腐化。

## tp 寄存器

内核态的 tp 不是 thread pointer，是 hart pointer。每个 hart 启动时设置 tp 指向自己的 HartLocal 结构，此后保持不变量：

**内核态运行期间，tp ≡ 当前 hart 的 HartLocal 地址。**

推论：

- trap 进出与上下文切换必须保存/恢复 tp（tp 即通用寄存器 x4，用户态用它做 TLS，内核态占用它）
- HartLocal 按 cache line 对齐；跨 hart 交互只能走全局层或 IPI，HartLocal 严格私有
- 数组包装处唯一一处 `unsafe impl Sync`，SAFETY 注明上述访问不变量

## 地址空间

Sv39，canonical 半区边界即用户/内核分界（对齐 Linux 布局）：

- 用户：低半区 `[0, 2^38)`（256GiB），完全归用户。
- 内核：高半区 `[0xFFFFFFC0_00000000, 2^39)`，含内核镜像与物理内存直映射（phys_to_virt 线性偏移）；vmalloc 等区域 M4+ 按需扩展。
- 每个用户页表 root 复制内核高半区顶层项（创建时拷 8-16 字节）→ 任意时刻内核代码恒可执行：trap 不切 satp，stvec 恒指内核 .text；satp 更换只发生在进程地址空间更换。
- 用户内存访问开 SUM 位直接走 VA，不再软件遍历页表 translate。
- secondary hart 经 HSM 启动时 satp 为 bare：经 identity 早期页表开 MMU 后跳高半区（对应 Linux head.S 的 trampoline_pg_dir → relocate_enable_mmu 一次性机构）。

设计依据：trap 是最热路径，共享映射消灭了 trampoline 页/双地址空间切换/软件 translate 三套机制（旧内核复杂度最集中处）。

## trap 帧与上下文

共享映射下无 trampoline：trap 入口直接是内核 .text。trap 帧存内核侧每线程存储（内核堆/每线程内核栈，M3 定），sscratch 指向本 hart 的 trap 锚（内核 sp 等），汇编经锚定位帧。TrapFrame（纯用户现场）与调度控制字段类型分离；汇编偏移与 Rust 结构静态断言绑定。trap 进出仅保存/恢复通用+浮点寄存器与 sepc，tp 不变量在汇编中维护。

## hart 种类

`HartKind`（Disabled | Application 起步）是异构扩展点，M3 随调度器引入；dtb 的 mmu-type/isa 解析结果决定 kind。

- 效能核（有 MMU、无 FPU 或低频）：现有设计天然支持——FP 条件保存（FS=Dirty 才存）零开销、每 hart CpuClock 用自身频率、核类型作为 M3 调度器的任务放置输入。
- 实时核（无 MMU）：高半区内核镜像无法在其执行，形态为 AMP 独立镜像 + 物理内存 carveout + IPI 通信，作为重写完成后的独立项目；现阶段仅约束设计不闭死（跨 hart 共享数据不假设全体核有 MMU）。

## 锁原语

内核自研 Spinlock：LR/SC 争用，持有期间关本地中断（sstatus.SIE 清零）。关中断是正确性要求而非优化——中断处理函数若获取本 hart 正持有的锁，同核死锁。

Spinlock 包装为 `lock_api::RawMutex`，同一实现注入两处：

- talc 堆分配器（TalcLock），堆分配纳入统一的中断纪律
- 全局容器（`OnceLock<Spinlock<T>>`）

睡眠锁在任务模型落地、出现可睡眠上下文后再引入。

## 堆与物理帧

分层管理两种语义不同的资源：

- 帧池（os/frame_pool，自研 in-band 空闲链）：管页粒度、物理连续、整批生死（进程退出整批归还）的物理帧；元数据内嵌空闲区间首帧，零堆依赖。DTB 内存段剔除启动占用后注册；FrameTracker RAII 归还。设计细节见 notes/mm.md「帧池」。
- 堆（talc）：管任意尺寸小对象；内存源 FrameSource 从帧池按需取 1MiB 连续帧块建立堆区（talc 支持多块不连续区域），帧块所有权随 claim 终身归堆。设备树解析（os/dtb，就地游标）与启动路径零堆依赖，保证帧池先于堆就绪的线性引导序。

理由：帧与小对象生命周期模式不同，分层使碎片域独立、锁独立；这也是 Linux（buddy+slab）的通行分层。堆→帧池单向依赖（不逆向），彻底消除帧池元数据碰堆的运行时环。

## 进程表

`ProcessTable` 封装 `OnceLock<Spinlock<BTreeMap<Pid, Arc<Process>>>>`，只暴露 get/insert/remove。pid 由 `AtomicU64` 单调递增分配，不复用。内部实现可替换（如 slot array），调用方无感。
