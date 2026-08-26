# Trapframe、上下文切换与 CSR

- 基线：`c449f00186a8533c4ca8fb04c93dc8e735602b3a`
- 状态：首审、独立核验与架构决策完成；代码修复统一纳入执行环境重构
- 外部规范：RISC-V Privileged v1.13（`references/normative/riscv-isa-v20250508/`）、RISC-V psABI v1.0
- 项目设计：`notes/impls/execution-context.md`
- 重构导航：`archived/todo-2026-08-execution-context-redesign.md`（已完成，随 a9a65cb 落地）

## 代码表面

- trap 入口/出口：`os/kernel/src/assembly.asm`
- Rust trap 分发与帧布局：`os/kernel/src/trap.rs`
- hart 锚与执行点：`os/kernel/src/hart.rs`
- 初始帧：`os/kernel/src/task/proc.rs`
- CSR 初始化和中断使能：`os/kernel/src/mm.rs`、`sched.rs`
- 致命 trap：`os/kernel/src/rt.rs`

## 契约表

| ID | 契约 | 来源 | 实现点 | 结论 |
|---|---|---|---|---|
| CTX-1 | 用户 x1..x31、sepc 在 trap 往返后保持 | psABI Register Convention、`supervisor.adoc`「sepc」 | `_user_trap`、`_resume_user` | 当前 GPR 主路径完整，x30/x31 中转正确 |
| CTX-2 | 标准 ABI 调用期间 sp 保持 16-byte 对齐 | `riscv-cc.adoc`「Integer Calling Convention」 | `_ret_to_user`、`handle_user_trap` call | 违反，见 TRAP-003 |
| CTX-3 | `gp`/`tp` 是 fixed register；内核必须建立自己的运行环境 | psABI Register Convention | trap 入口、HART_SETUP | tp 已建立，gp 契约未闭合，见 TRAP-005 |
| CSR-1 | SPP 决定 SRET 返回特权级；HSM 未保证其它 sstatus 字段 | `supervisor.adoc`「sstatus」、SBI HSM | HART_SETUP、`_resume_user` | 违反，见 TRAP-001 |
| CSR-2 | U 态忽略 SIE；中断来源由 `sie` 逐项使能 | `supervisor.adoc`「sstatus」「sip/sie」 | HART_SETUP、`sched::run`、notes | `sie` 未精确初始化，notes 漂移 |
| TRAP-1 | scause Interrupt bit 与 code 必须共同解释 | `supervisor.adoc`「scause」 | `handle_user_trap` | 违反，见 TRAP-004 |
| TRAP-2 | U 态同步异常不得破坏内核 | `notes/impls/task.md` 生命周期 | `handle_user_trap` | 部分异常会误分发或 panic |
| FP-1 | Dirty 才需保存；Initial 恢复必须建立初始常量 | `machine.adoc`「Extension Context Status」 | FP save/restore、`fp_valid` | Dirty/valid 路径正确，invalid 路径泄漏，见 TRAP-002 |
| ANCHOR-1 | 内核态 tp 指 HartLocal、stvec 指内核入口 | `notes/impls/internals.md` | trap 过渡序列 | 稳态正确，过渡窗口不闭合，见 TRAP-006 |
| LAYOUT-1 | 汇编偏移与 Rust 实际字段布局机械绑定 | `notes/impls/internals.md`、模块注释 | `frame_off`、`hart::off` | 当前数值正确，绑定不完整，见 TRAP-007 |

## Findings

## TRAP-001：首次用户返回依赖 HSM 未定义的 CSR 状态

- 分类：缺陷
- 严重度：P0
- 置信度：已确认
- 状态：已核验，纳入执行环境重构

### 契约

SBI HSM「Hart Start Register State」只保证 `satp=0`、`sstatus.SIE=0`、`a0/a1`。`supervisor.adoc`「Supervisor Status」规定 SPP=0 才使 SRET 返回 U 态；`sie` 中置位的中断源在 U 态均可进入 S 态。

### 实现

HART_SETUP 只清 SIE、设置 FS。`_resume_user` 未确定 SPP 就执行 SRET；`sched::run` 使用 `csrs sie`，会保留固件遗留的其它中断使能位。UXL、UBE 及已实现可写扩展状态也没有成文基线。

### 影响

SPP=1 时首次 SRET 留在 S 态，随后执行 U 页立即 fault；额外 `sie` 位可能把未接入的中断送入只处理 SSIP/STIP 的路径。OpenSBI 当前初值安全不能替代契约。

### 处理方向

定义并集中建立 hart CSR 基线：精确写 `sie=SSIE|STIE`；在每次用户出口显式保证 SPP=0，并决定 SPIE、UBE、UXL、SDT 等项目实际依赖字段。不能整写未知/WPRI 位。

## TRAP-002：无有效 FP 帧时泄漏前一上下文的浮点状态

- 分类：缺陷
- 严重度：P0
- 置信度：已确认
- 状态：已核验，决策完成，纳入执行环境重构

### 契约

`machine.adoc`「Extension Context Status」规定 FS=Initial 对应的上下文在恢复时必须建立初始常量以避免安全漏洞；仅写 FS=Initial 不会清除 `f0..f31/fcsr`。

### 实现

新线程以 `fp_valid=0` 创建。`_resume_user` 的 invalid 分支只把 FS 置 Initial，没有初始化任何 FPR 或 fcsr。

### 影响

一个使用过 FP 的线程切走后，下一条无有效 FP 帧的线程可直接读取该 hart 残留的 FPR/fcsr，形成跨进程、跨 hart 信息泄漏。

### 处理方向

已选择能力感知的确定状态：FpState 嵌入 UserContext、创建时全零，不再保留 fp_valid。Base64 线程 FS=Off且不执行 FP helper；D64 线程只在有效 FLEN 恰为 64 的 hart 上完整恢复，Dirty 时保存。Q/其它未建模状态由 loader/domain 拒绝或硬件 gate 保持 Off。

## TRAP-003：调度现场破坏 psABI 栈对齐

- 分类：缺陷
- 严重度：P1
- 置信度：已确认
- 状态：已核验，纳入执行环境重构

### 契约

`riscv-cc.adoc`「Integer Calling Convention」要求标准过程入口及执行期间 sp 始终 128-bit 对齐；非标准汇编调用标准过程前必须重对齐。

### 实现

`_ret_to_user` 从对齐的 Rust 栈减去 104 字节并把它保存为 sched_sp。用户 trap 恢复该 sp 后直接调用 Rust `handle_user_trap`，此时 `sp % 16 == 8`。

### 影响

每次用户 trap 都违反 Rust 调用 ABI。当前编译器碰巧没有在该位置生成必须 16-byte 对齐的访问不能构成保证；一旦生成即可在内核态 fault。

### 处理方向

调度现场扩为 112 字节并对称恢复，保留 8 字节 padding；若后续保存其它状态，整个 frame 继续保持 16 的倍数。

## TRAP-004：scause 丢失 Interrupt 位后发生编号碰撞

- 分类：缺陷
- 严重度：P1
- 置信度：已确认
- 状态：已核验，纳入执行环境重构

### 契约

`supervisor.adoc`「Supervisor Cause」把 Interrupt bit 与 Exception Code 定义为两个字段。interrupt 1/5 是 SSIP/STIP，而 exception 1/5 是 instruction/load access fault；同步异常还包括 0、6、7 等。

### 实现

`handle_user_trap` 先清最高位，再只按 code 匹配。exception 1 被当成 IPI，exception 5 被当成 timer；0、6、7 等用户可触发异常落入 panic。当前枚举还混入保留码并遗漏部分 custom 范围。

### 影响

合规硬件上的用户异常可能被错误恢复、错误调度或直接杀死内核，违反“用户异常只终止进程”的隔离契约。

### 处理方向

以 `(is_interrupt, code)` 分发：只有 interrupt 1/5 进入 SSIP/STIP，只有 exception 8 进入 U ecall；其余来自 U 态的同步异常统一终止当前进程。未来接入其它中断时在 interrupt 分支扩展。

## TRAP-005：内核 gp 与浮点调用 ABI 缺少硬约束

- 分类：未证明不变量
- 严重度：P2
- 置信度：高
- 状态：已核验，决策完成，纳入执行环境重构

### 契约

psABI 把 gp/tp 定义为 fixed register；当前目标 ELF 标记为 double-float ABI，相应 ABI 对 `fs0..fs11` 有 callee-saved 要求。

### 实现

trap 保存用户 gp 后只恢复内核 tp，没有恢复内核 gp；`_ret_to_user` 也没有保存调度器的浮点 callee-saved 状态。当前反汇编除用户 gp 保存/恢复外没有 gp-relative 指令，Rust 内核也没有 FP 使用，因此现二进制未显化。

### 影响

正确性依赖未成文的工具链偶然结果。启用 gp relaxation、small data 或内核 FP 后，用户状态可污染 Rust 执行环境，汇编 shim 也会违反调用 ABI。

### 处理方向

kernel gp 正式初始化并在用户 trap 边界恢复。内核切换到 RV64IMAC/LP64 整数 ABI，普通内核代码禁止 FP/V；用户 FP 保存恢复放入 capability-guarded 的独立汇编 helper并接受链接后审计。

## TRAP-006：trap 过渡窗口不满足 tp/stvec 内核态不变量

- 分类：未证明不变量
- 严重度：P2
- 置信度：高
- 状态：已核验，决策完成，纳入执行环境重构

### 契约

`notes/impls/internals.md` 规定内核态 tp 恒指 HartLocal、stvec 指内核致命入口。SIE=0 只能屏蔽中断，不能防止同步异常。

### 实现

用户 trap 入口在保存大部分现场后才切 tp/stvec；返回路径在 FPR/GPR 恢复前就把 stvec 切回 `_user_trap`，并在仍处 S 态时恢复用户 tp。该入口也不依据 SPP 区分嵌套来源。

### 影响

过渡窗口内的同步异常会进入假定“来自 U 态”的入口，并可能把用户值当作 HartLocal。正常有效帧下触发概率低，但非法 FP/CSR 状态、映射损坏或硬件错误会把原本应诊断的内核 fault 变成二次破坏。

### 处理方向

正式 stvec 恒指共同入口、sscratch 恒指 HartLocal，入口以 SPP 作为唯一来源真值；S 态来源保存 FatalFrame并切 emergency stack，U 态来源才保存 UserContext。由结构消除双入口切换窗口。

## TRAP-007：汇编偏移没有完整绑定 Rust 实际布局

- 分类：未证明不变量
- 严重度：P2
- 置信度：已确认
- 状态：已核验，纳入执行环境重构

### 契约

模块文档声称 TrapFrame 和 HartLocal 与汇编偏移双向绑定。

### 实现

TrapFrame 只对 fcsr/fp_valid/sepc 使用实际 `offset_of!`，没有绑定 x/f 基址。HartLocal 只断言总大小/对齐；`hart::off` 与汇编数字是两份手写常量，现有断言没有引用 HartLocal 实际字段。

### 影响

当前布局人工核对正确，但字段重排后可以继续编译并静默破坏用户现场或 hart 锚。

### 处理方向

为全部汇编访问字段增加 `offset_of!`，并优先通过 `global_asm!` const operands 把 Rust 计算值注入汇编，消除第二份数字真值。

## TRAP-008：SIE 文档把 U 态中断条件写反

- 分类：文档漂移
- 严重度：P3
- 置信度：已确认
- 状态：已收口

`supervisor.adoc` 规定 U 态忽略 SIE，S 级中断天然全局可进入，具体来源由 `sie` 控制。已在 `notes/impls/internals.md` 与 `notes/impls/execution-context.md` 收口为：内核 S 态 SIE=0；U 态不受 SIE gate，SSIE/STIE 等来源由 `sie` 精确使能。

## 正确实现与边界

- 用户 x1..x31 均被保存/恢复；x30 经 HartLocal scratch、x31 经 sscratch 中转，当前数值正确。
- 有效 FP 帧覆盖 f0..f31 与 fcsr；Dirty 保存、Clean 恢复和跨 hart 迁移思路正确。
- sscratch 的锚交换、sepc 保存/恢复、ecall 前进 4 字节正确。
- stvec 地址满足 Direct 模式对齐。
- SUM 在每个 hart 进入调度器时设置，符合当前 S 态直访用户 VA 的项目策略。
- 整数调度现场保存 ra+s0..s11，集合正确；问题是 frame 总大小未保持 ABI 对齐。

## 跨片登记

- 启动/SMP：SBI-005 的页表、KERNEL_SATP 和全局状态发布协议。
- 启动：boot hartid 在边界检查前直接索引 HartLocal/栈。
- SMP/生命周期：`clear_context` 未清 current_thread，注释与悬挂指针纪律不一致。
- 平台：无 F 扩展 hart 不能执行当前 FP 恢复路径，现 RV64GC 基线不触发。
- 致命诊断：`_kernel_trap` 使用可能已受损的当前栈且不保存完整现场；是否引入 emergency stack 属后续可靠性设计。

## 已决策

- 持久用户状态改为 UserContext；FpState 创建时全零，Base64 FS=Off，D64 在 FLEN=64 hart 上完整恢复、Dirty 保存。
- kernel gp 正式初始化；汇编偏移由 Rust `offset_of!` 经 `global_asm!` 注入。
- 内核使用 RV64IMAC/LP64 整数 ABI，普通内核代码禁止 FP/V；用户 FP helper 是 capability-guarded 的局部汇编。
- stvec 恒指共同入口，sscratch 恒为 HartLocal，SPP 是来源真值；S 态 trap 进入 emergency fatal。
- CSR、SUM、LR/SC reservation、I-cache 发布、HartSlot/bootstrap 与 capability-aware domain 按 `notes/impls/execution-context.md` 统一收口。

TRAP-001 至 TRAP-007 不在旧路径逐项打补丁，防止形成马上被删除的过渡模型；当前代码在整体重构完成前不能宣称支持恶意用户负载或无 F/D 异构 hart。
