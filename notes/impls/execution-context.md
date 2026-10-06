# 执行环境实现

方向见 [`../ideas/execution-context.md`](../ideas/execution-context.md)。本篇记录 hart 启动、用户上下文、trap/CSR 与 capability-derived 调度域的当前落地。

## Bootstrap 与正式环境

cold boot 使用专用 stack、临时页表、bootstrap Lock Ladder 帧和早期 fatal vector，不占用正式 `HartSlot(0)`。boot hart 以固件 raw hartid 查找正式 slot；secondary 通过 HSM opaque 取得 `HartBootRecord` PA，从 Bare 经永久 PA 前导与 identity/高半区别名过渡页表进入高半区。

两条路径在 formal entry 汇合，统一建立 gp、tp、sscratch、正式 stack、satp、CSR 和 stvec。过渡表在 cold boot 建成后只读；全体启动记录以 Release 发布后才发 HSM start。`os/runtime_gate` 将全局状态限制为 `Preparing -> Ready | Failed`：HSM 错误、formal CSR 拒绝或 Online 超时先以 Release 广播 Failed，晚到 secondary 以 Acquire 观察后停驻；只有全体 admitted hart Online、调度域和 primordial Bootstrap Work 准入后才发布 Ready 并进入调度循环。ELF/初始 VM 装载由该维护记录分批完成；异步装载失败使用既有 runtime_stop 全 hart 停驻，不回退 Ready gate。

`rt.rs` 的 panic、fatal trap、`fatal_msg` 和高半区 bootstrap fatal 在任何诊断前统一调用 `registry::publish_failed`。它只对 Preparing 执行 Release CAS；Failed 幂等，Ready 保持不变，错误终段不再次 panic，不取 registry 锁、不分配、不读取 tp。分配失败经 panic handler 进入同一终段。GATE 位于 `_start_high` 清零的普通 BSS；高半区 Rust 入口可访问，Bare PA fatal 不访问该状态，未 Online 的 secondary 由 boot 的 Online 超时收束。

`tools/check-boot-failure.py` 在正式 debug binary 的 `boot::load` 入口用 GDB 注入坏 envelope 长度、分配失败 handler 和真实非法指令 trap；断言注入时 Preparing，并在四个不同 hart 的 park 入口确认 Failed。`just virt-boot-failure` 提供单独路线，`just acceptance` 聚合执行；正常内核没有故障注入开关。原子发布的外部依据见固定 ISA `a-st-ext.adoc`「Specifying Ordering of Atomic Instructions」。

## 身份、能力与域

`HartId` 保存 DT/SBI raw hartid，`HartSlot` 是按 admitted hartid 升序分配的稠密索引，`HartTopology` 保存可选 cpu-map。HartLocal、内核栈、active set 与 per-hart timer queue 都按 slot 索引；SBI 调用边界才转换回 raw hartid，IPI 的 `(mask, base)` 不接收内部 slot bitmap。

`os/dtb::cpu` 在零分配 admission 中一次读取现代 `riscv,isa-base`、`riscv,isa-extensions`、`mmu-type`、status、时钟与 raw hartid，固定容量内排序并拒绝重复。status 缺省或严格 `okay` 才准入，其余 DTSpec 合法不可用状态跳过，未知/畸形值拒绝整份平台输入；能力同时强制 `q => d => f`。内核基线为 RV64IMAC、Zicsr、Zifencei、Zicntr 和系统选定页表模式；S 态 time 与 SBI TIME 共用已验证的非零 DT timebase。

普通 Ready、termination、Remote 与 deferred-work IPI 都只作已发布业务真值的门铃，发送 API 返回失败 slot mask；失败不撤销 Pending/Ready、不伪造完成，也不在发送者上 panic。

`os/sched_domain` 按执行需求满足签名划分域，`sched::DomainTable` 在全员 Online 后构造并冻结，为每个稳定域分配非零 index，hart→域映射保存在 `by_slot`。ProcessBindExecution 解析平铺的 execution profile，选择最弱兼容域，并在 Building 短租约内冻结 Body 的 requirement/domain；ThreadCreate 拷取不变的域与 FP 档位，ProcessStart 验证需求存在。重复 BindExecution 返回 ObjectNotAvailable。ELF requirement 到 profile 的映射由用户态 loader 完成。enqueue/pick 只访问已绑定域的调度类。

## ABI 档位与 UserContext

内核按 RV64IMAC/LP64 整数 ABI 构建，不在普通 Rust 路径使用 F/D/V；内核稳态 FS/VS=Off。用户 requirement 来自 ELF flags 与 `Tag_RISCV_arch`：

- Base64：LP64，FS 恒 Off；
- D64：LP64D，只进入有效 FLEN 恰为 64 的域。

F-only、Q、V、TSO 与未建模扩展由 loader 拒绝。Q-capable hart 可运行 Base64，但不进入 D64 域。

`os/kernel/src/context.rs::UserContext` 每线程保存 GPR、sepc 与完整 `FpState`；`FpState` 含 32 个 FPR 和 fcsr，创建时全零。D64 切入时完整恢复并置 FS=Clean，trap 只在 FS=Dirty 时回写，随后恢复内核 FS=Off。局部 F/D helper 位于独立代码 section；`test_fp` 验证 FPR/fcsr 跨 ecall、Sleep 和调度轮转保持。

Building 首线程与 Running 期 ThreadCreate 共用 `ThreadStartContext` 到 `UserContext` 的编码：entry 写 sepc，sp 写 x2，arg1/arg2 写 a0/a1，其余 GPR、tp、FP 状态清零。ProcessBindExecution 在首次 ThreadCreate 前冻结 requirement 与兼容调度域；ThreadCreate 只继承已冻结的 execution binding，线程只能进入该域。ThreadExit 与被终止线程都走非 Resume trap 出口，先切回 kernel satp、清 active，再由调度侧释放执行容器并请求 `ThreadDeparture`，不会在仍使用用户 root 时发布 DONE。

当前用户首入口由 `user/platforms/linker.ld` 的 `ENTRY(_start)` 定位；`libruntime::_start` 以 RV64 norelax 入口适配段建立 `gp`，保持 a0/a1 StartupBlock 参数后进入 runtime start。runtime 完成 heap、StartupContext 与入口 ThreadRegistry 接管，再由 `start` lang item 接收 rustc 的 `main` shim。运行期线程由 `libthread` trampoline 进入，栈映射遇到 transient ObjectBusy 时让出后重试。Base64/D64 target 均显式使用 `riscv64-elf-ld`、`__global_pointer$`；入口与 trampoline 反汇编门已通过。当前 PT_TLS 不在 shared/elf admission 白名单，tp=0 不提供 TLS。

## Trap 与 CSR

formal stvec 指向共同 direct-mode 入口，sscratch 在正式环境中恒指 HartLocal。入口先经 sscratch 取得 hart 锚，并把原始用户 t5/t6 保存到 HartLocal scratch；恢复 sscratch 锚后才复用已保存的临时寄存器读取 SPP。SPP=0 保存 UserContext，SPP=1 进入 per-hart FatalFrame。UserContext 保存完成前不得覆盖任何尚未保存的用户寄存器；除 a0/a1 返回值外，全部用户 GPR 跨 ecall 保持。

fatal 首帧保存在独立 FatalFrame，递归 guard 防止诊断故障覆盖证据。CSR 由 formal entry、内核稳态和 pre-sret 三个边界集中设置；未知/WPRI 位不整写。用户出口在最后一个内核 LR/SC 后用 dummy SC 清 reservation。

`scause` 按 interrupt/code 分发；U ecall 是 exception 8，其它用户同步异常冻结 Fault 终因并终止进程，不 panic 内核。

## 地址空间归属纪律

用户 trap 的 Resume 热路径继续使用当前用户 root；非 Resume 出口在恢复调度循环现场前统一装载 `KERNEL_SATP` 并执行本地全量 SFENCE.VMA。只有完成该步骤后，Rust 调度侧才清 active 位、reap 或发布 park。VM active=0 因而只在没有 hart 仍使用目标用户 root 后成立，空间退休另等访问与结果责任。

进程资源由 Shell/RetireWork 自动推进，不存在 ProcessDrain syscall。维护在内核安全点以目标 AnySpace、表帧映射和物理直映射收束；Shell 先冻结入口，VM 记录持 active 快照并等退出/fence、访问租约与实际退款。细节见 [mm](mm.md) 与 [task](task.md)。

UserContext、FpState、HartLocal、FatalFrame 与 SchedulerFrame 的布局由 Rust `offset_of!` 生成常量注入汇编，调度现场总大小保持 16-byte 对齐。

## 用户访问与指令发布

内核稳态 SUM=0；uaccess 在持 AddressSpace 锁并完成范围验证后临时开启 SUM。同步 syscall 输出使用 `deliver_output`，复检失败记录 output_fault，锁外冻结调用进程 Fault；VM 与 Start 完成计划只返回寄存器单位值，不保留异步用户输出指针。

ProcessWrite 只在 Building 阶段经物理直映射写 backing。调度器 Acquire 取得发布线程后，由 AddressSpace 事务根据真实 Install/Protect 意图推进 instruction epoch；远端同步或 `_ret_to_user` 的统一地址空间切换出口执行本地 `fence.i`，Resume 不再依赖调度器重复补发。已发布 executable page 没有普通写入口。
