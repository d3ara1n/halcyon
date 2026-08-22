# 执行环境

执行环境覆盖 hart 从固件入口进入正式运行、用户上下文在 trap 与调度间保存、以及异构能力对任务放置的约束。具体汇编与结构字段可以演进，下列边界不随实现方式改变。

## Bootstrap 与正式环境

cold boot 是独立临时环境，不属于任何正式 hart：使用专用 stack、临时页表和早期 fatal vector，单核完成平台发现与正式全局状态构造。它不借用 `HartSlot(0)` 或 `HartLocal[0]`。

正式 hart 由 `HartBootRecord` 描述。boot hart 以固件传入的 raw hartid 查询自己的正式 slot；secondary 由 HSM opaque 获得 record PA。secondary 从 Bare 经永久 PA 前导和 identity/高半区别名过渡页表进入高半区；切换过渡与正式 satp 后分别执行地址翻译同步。两条路径在高半区 formal entry 汇合，统一建立 gp、tp、sscratch、stack、satp、CSR 和 trap vector。

cold-bootstrap 专用页对齐区间在最后引用消失后回收；secondary PA 前导与过渡页表属于永久 hart-entry 设施。过渡页表由 cold boot 一次建成；secondary 只幂等补写自己使用的槽位，禁止对共享暂存结构做先清后建的重建——并发执行下清零会拆掉先行者脚下的翻译。所有 admitted hart 的启动记录先发布，再发 HSM start；record 与 RuntimeGate 使用 Release/Acquire。全体 Online、调度域与初始任务就绪后才发布 Ready。任何 HSM 状态矛盾、启动错误或上线超时使本次启动整体失败，不在不确定集合上降级运行。

## 身份、能力与拓扑

三者互不混用：

- `HartId` 是 DT/SBI 的 raw hartid，可以稀疏；
- `HartSlot` 是内核按 admitted raw hartid 升序分配的稠密身份；
- `HartTopology` 是可选 `cpu-map` 给出的 socket/cluster/core/thread 层级。

HartLocal、stack 和内部 hart set 按 slot 索引；SBI 边界显式转换回 raw hartid。拓扑只服务 affinity、电源和共享资源策略，不能推断 ISA capability 或决定 slot。

平台 DT 只接受现代 `riscv,isa-base` + `riscv,isa-extensions`，不兼容已弃用的 `riscv,isa`。每个 hart 独立读取 status、ISA、MMU 和其它能力。内核基线为 RV64IMAC、Zicsr、Zifencei、Zicntr 与 Sv39，S 态 `time` 必须可读并与 DT timebase/SBI TIME 同源。

每种用户可见持久状态必须满足：完整保存恢复、由已核验的硬件 gate 保持 Off、或对应 hart 不进入用户调度域。扩展出现在 DT 中不等于 S 态已经拥有隔离它的控制权。

## 内核与用户 ABI

内核使用 RV64IMAC/LP64 整数 ABI，不依赖 F/D/V；内核稳态 FS/VS=Off。kernel gp 是正式运行环境的一部分，在所有 hart entry 和用户 trap 边界建立。汇编调用 Rust 前保持 psABI 16-byte stack alignment。

用户执行需求来自 ELF flags 与 `Tag_RISCV_arch`，不由服务名推断。当前支持：

- Base64：LP64，FS=Off；
- D64：LP64D，F/D 状态可用，只能运行于有效 FLEN 恰为 64 的 hart。

F-only、Q、V、TSO 及未建模状态扩展在 loader 明确拒绝。Q-capable hart 可以运行 Base64，但在 Fp128 模型建立前不能运行 D64。

能力决定 domain eligibility，调度 class 只表达调度策略。线程始终只归属一个 compatible domain 的一个 class；跨域迁移是显式 dequeue→换归属→enqueue 事务，不在 pick 后跳过不兼容线程。

## 用户上下文与 FP

持久用户状态统一属于线程的 `UserContext`，包含 GPR、sepc 与嵌入的 `FpState`。`FpState` 创建时完整清零，不存在依赖 hart 残留的 valid 状态。

Base64 从不执行 FP 保存恢复，用户出口 FS=Off。D64 每次切入完整恢复 FPR/fcsr 并请求 FS=Clean；trap 时仅在 FS=Dirty 时更新内存状态，随后内核恢复 FS=Off。FP helper 是只有 eligible hart 才能进入的局部 F/D 汇编代码；它的代码 section 与 FpState 的数据所有权无关。

## Trap 与 CSR

formal stvec 恒指共同 direct-mode 入口。正式环境中 sscratch 恒指 HartLocal；入口用 HartLocal scratch 保全临时寄存器，硬件 SPP 是来源的唯一真值：SPP=0 才保存 UserContext，SPP=1 进入 per-hart emergency fatal。返回用户尾部发生同步异常时仍按 S 态 fatal 处理，不存在双 vector 过渡窗口。

fatal 首帧保存到独立 FatalFrame；递归 guard 防止诊断故障覆盖原始证据。存在 Ssdbltrp 时，首要状态保存期间保留 SDT 的硬件保护，首帧建立后再清 SDT进入软件诊断。

CSR 由 formal entry、内核稳态和 pre-sret 三个边界集中拥有：`sie` 精确为已接入来源，SIE/FS/VS/SUM/MXR关闭，SPP/UXL/UBE/SPIE、senvcfg、可访问的 sstateen 与 SDT 都有明确值和必要的 WARL 核验；未知/WPRI 位不整写。U 态忽略 SIE，SSIP/STIP 是否进入 S 态由 `sie` 来源位控制。

`scause` 按 `(is_interrupt, code)` 分发。U ecall 是 exception 8；其它用户同步异常只终止进程。每次用户出口在最后一个内核 LR/SC 之后以 dummy SC 清除 reservation，reservation 不属于 UserContext。

Rust `offset_of!` 是 UserContext、FpState、HartLocal、FatalFrame 与 SchedulerFrame 的布局真值，通过 `global_asm!` 常量注入汇编。调度现场总大小保持 16-byte 对齐。

## 用户访问与指令发布

内核稳态 SUM=0；只有完成用户地址验证并持有所需对象锁后，user-copy guard 才临时开启 SUM。

可执行页按 W^X 发布。writer 完成数据写和 data fence 后以 Release 使线程可运行；调度器 Acquire 取得线程，目标 hart 在该地址空间新代码代次首次执行前本地 `fence.i`。已发布 executable page 不原地写；动态装载必须先让相关线程 quiescent、撤销执行许可，再发布新代次。
