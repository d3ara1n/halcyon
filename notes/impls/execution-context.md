# 执行环境实现

方向见 [`../ideas/execution-context.md`](../ideas/execution-context.md)。本篇记录 hart 启动、用户上下文、trap/CSR 与 capability-derived 调度域的当前落地。

## Bootstrap 与正式环境

cold boot 使用专用 stack、临时页表、bootstrap Lock Ladder 帧和早期 fatal vector，不占用正式 `HartSlot(0)`。boot hart 以固件 raw hartid 查找正式 slot；secondary 通过 HSM opaque 取得 `HartBootRecord` PA，从 Bare 经永久 PA 前导与 identity/高半区别名过渡页表进入高半区。

两条路径在 formal entry 汇合，统一建立 gp、tp、sscratch、正式 stack、satp、CSR 和 stvec。过渡表在 cold boot 建成后只读；全体启动记录以 Release 发布后才发 HSM start，RuntimeGate 以 Release/Acquire 发布 Online/Ready。全体 admitted hart Online、调度域和初始任务就绪后才进入调度循环。

## 身份、能力与域

`HartId` 保存 DT/SBI raw hartid，`HartSlot` 是按 admitted hartid 升序分配的稠密索引，`HartTopology` 保存可选 cpu-map。HartLocal、内核栈、active set 与 per-hart timer queue 都按 slot 索引；SBI 调用边界转换回 raw hartid。

`os/kernel/src/board.rs` 逐 hart 读取现代 `riscv,isa-base`、`riscv,isa-extensions` 与 `mmu-type`。内核准入基线为 RV64IMAC、Zicsr、Zifencei、Zicntr 和系统选定页表模式；S 态 time 与 SBI TIME 共用 DT timebase。

`os/sched_domain` 按执行需求满足签名划分域，`sched::DomainTable` 在全员 Online 后构造并冻结，为每个稳定域分配非零 index，hart→域映射保存在 `by_slot`。ProcessStart 解析平铺的 execution profile，选择最弱兼容域，并在 lifecycle 提交后以单个 `AtomicUsize` 一次冻结 requirement 与域 index；零值只表示未绑定，不会与 Base64 混淆。ELF requirement 到 profile 的映射由用户态 loader 完成。enqueue/pick 只访问已绑定域的调度类。

## ABI 档位与 UserContext

内核按 RV64IMAC/LP64 整数 ABI 构建，不在普通 Rust 路径使用 F/D/V；内核稳态 FS/VS=Off。用户 requirement 来自 ELF flags 与 `Tag_RISCV_arch`：

- Base64：LP64，FS 恒 Off；
- D64：LP64D，只进入有效 FLEN 恰为 64 的域。

F-only、Q、V、TSO 与未建模扩展由 loader 拒绝。Q-capable hart 可运行 Base64，但不进入 D64 域。

`os/kernel/src/context.rs::UserContext` 每线程保存 GPR、sepc 与完整 `FpState`；`FpState` 含 32 个 FPR 和 fcsr，创建时全零。D64 切入时完整恢复并置 FS=Clean，trap 只在 FS=Dirty 时回写，随后恢复内核 FS=Off。局部 F/D helper 位于独立代码 section；`srv_fp` 验证 FPR/fcsr 跨 ecall、Sleep 和调度轮转保持。

## Trap 与 CSR

formal stvec 指向共同 direct-mode 入口，sscratch 在正式环境中恒指 HartLocal。入口先经 sscratch 取得 hart 锚，并把原始用户 t5/t6 保存到 HartLocal scratch；恢复 sscratch 锚后才复用已保存的临时寄存器读取 SPP。SPP=0 保存 UserContext，SPP=1 进入 per-hart FatalFrame。UserContext 保存完成前不得覆盖任何尚未保存的用户寄存器；除 a0/a1 返回值外，全部用户 GPR 跨 ecall 保持。

fatal 首帧保存在独立 FatalFrame，递归 guard 防止诊断故障覆盖证据。CSR 由 formal entry、内核稳态和 pre-sret 三个边界集中设置；未知/WPRI 位不整写。用户出口在最后一个内核 LR/SC 后用 dummy SC 清 reservation。

`scause` 按 interrupt/code 分发；U ecall 是 exception 8，其它用户同步异常冻结 Fault 终因并终止进程，不 panic 内核。

## 地址空间归属纪律

用户 trap 的 Resume 热路径继续使用当前用户 root；非 Resume 出口在恢复调度循环现场前统一装载 `KERNEL_SATP` 并执行本地全量 SFENCE.VMA。只有完成该步骤后，Rust 调度侧才清 active 位、reap 或发布 park。因此 REAPABLE 的 active=0 保证没有 hart 仍使用目标用户 root。

这一约束针对目标线程离场和调度侧 teardown barrier。`ProcessDrain` 是管理进程发起的普通 Completed syscall，可以在管理者自己的用户 root 激活时执行；所有用户 root 共享内核高半区，内核通过目标 AddressSpace 对象、页表映射接口和物理直映射访问并收束目标资源，不依赖目标用户映射。Drain 只在目标 REAPABLE 后准入。

UserContext、FpState、HartLocal、FatalFrame 与 SchedulerFrame 的布局由 Rust `offset_of!` 生成常量注入汇编，调度现场总大小保持 16-byte 对齐。

## 用户访问与指令发布

内核稳态 SUM=0；uaccess 在持 AddressSpace 锁并完成范围验证后临时开启 SUM。同步 syscall 输出使用 `deliver_output`，复检失败冻结调用进程 Fault；异步 WaitMany 结果尽力写回，失败经错误通道交付。

ProcessWrite 只在 Building 阶段经物理直映射写 backing。调度器 Acquire 取得发布线程后，在每次新 dispatch 前执行本 hart `fence.i`；Resume 不重复执行。已发布 executable page 没有普通写入口。
