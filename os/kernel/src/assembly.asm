.option norvc
.attribute arch, "rv64imac_zicsr_zifencei"

# ---------------------------------------------------------------------------
# 执行环境装配（契约见 notes/impls/execution-context.md；偏移常量由 Rust
# offset_of! 经 main.rs 的 global_asm! 注入，本文件不维护数字真值）。
#
# 三段空间：
# - .text.init / .bootstrap.*（PA，VMA=LMA）：cold boot 专用，全员 Online
#   后回收帧池；
# - .text.entry（PA，VMA=LMA）：永久 hart-entry 设施——secondary PA 前导、
#   早期 fatal vector；共享过渡页表 root 在链接脚本 .bss.entry；
# - 高半区段：正式运行环境。
#
# 启动发布协议：record 以 Release/Acquire 发布（Starting 后 secondary 才
# 消费；Online 由 formal entry 发布）。平台对普通主存硬件 cache coherent。
# ---------------------------------------------------------------------------

.section .text.init
.global _start
# cold boot 入口：OpenSBI 跳到加载地址起点。a0 = boot hartid，a1 = dtb PA；
# bare satp，PC = PA。职责：建立只覆盖内核镜像与实际 DTB 页的共享过渡表，
# 同时提供 identity 与高半区别名，开 MMU 后跳高半区。a0/a1 原样带过。
_start:
    la      sp, __bootstrap_stack_top       # bootstrap 临时栈
    la      t0, __transition_root_pa
    la      t1, __transition_leaf_end
    mv      t2, t0
0:  sd      zero, 0(t2)                     # 清零 root + middle + leaf arena
    addi    t2, t2, 8
    bltu    t2, t1, 0b

    # middle/leaf arena 由映射 helper 单调消费；每个物理 vpn2 槽同时建立
    # identity 与高半区别名。
    la      a4, __transition_middle_base
    la      a3, __transition_leaf_base
    la      t3, _start
    li      t0, -4096
    and     t3, t3, t0
    la      t0, _BOOT_CONSTS
    ld      a2, 8(t0)                       # __kernel_pa_end（已页对齐）
1:  bgeu    t3, a2, 2f
    call    _transition_map_page
    li      t0, 4096
    add     t3, t3, t0
    j       1b

    # DTB totalsize 是大端 u32；bare 模式可直接读 header，再精确映射其页面。
2:  lbu     t0, 4(a1)
    slli    t0, t0, 24
    lbu     t1, 5(a1)
    slli    t1, t1, 16
    or      t0, t0, t1
    lbu     t1, 6(a1)
    slli    t1, t1, 8
    or      t0, t0, t1
    lbu     t1, 7(a1)
    or      t0, t0, t1                     # t0 = totalsize
    li      t1, 40
    bltu    t0, t1, _transition_fail
    li      t1, 0x200000                    # transition DTB 映射上限 2MiB
    bltu    t1, t0, _transition_fail
    add     a2, a1, t0
    bltu    a2, a1, _transition_fail
    li      t1, 4095
    add     a2, a2, t1
    bltu    a2, t1, _transition_fail
    li      t1, -4096
    and     a2, a2, t1
    and     t3, a1, t1
3:  bgeu    t3, a2, 4f
    call    _transition_map_page
    li      t0, 4096
    add     t3, t3, t0
    j       3b

4:  la      t0, __transition_root_pa
    srli    t0, t0, 12                      # satp.ppn
    li      t1, 8 << 60                     # Sv39
    or      t0, t0, t1
    csrw    satp, t0
    sfence.vma                              # 过渡翻译同步
    la      t0, _BOOT_CONSTS
    ld      t5, 0(t0)                       # _start_high VMA
    jr      t5                              # → 高半区 _start_high

# 映射一个 4KiB 物理页到共享 identity/high-half 子树。
# 输入：t3 = page PA，a4/a3 = 下一张空闲 middle/leaf table；输出：arena 指针单调推进。
_transition_map_page:
    srli    t0, t3, 30                      # 物理 vpn2 槽
    li      t1, 256
    bgeu    t0, t1, _transition_fail        # identity/high-half root 槽必须结构性互斥
    slli    t1, t0, 3
    la      t2, __transition_root_pa
    add     t1, t2, t1                     # identity root PTE 地址
    ld      t4, 0(t1)
    andi    t5, t4, 1
    bnez    t5, 5f
    la      t5, __transition_middle_end
    bgeu    a4, t5, _transition_fail
    srli    t4, a4, 2
    ori     t4, t4, 1                       # 新 middle table branch PTE
    sd      t4, 0(t1)
    addi    t0, t0, 256
    slli    t0, t0, 3
    add     t0, t2, t0
    sd      t4, 0(t0)                       # 高半区别名共享同一 middle table
    li      t5, 4096
    add     a4, a4, t5
5:  srli    t4, t4, 10
    slli    t4, t4, 12                      # root branch → middle table PA

    srli    t0, t3, 21
    andi    t0, t0, 0x1ff
    slli    t0, t0, 3
    add     t1, t4, t0                     # middle PTE 地址
    ld      t2, 0(t1)
    andi    t5, t2, 1
    bnez    t5, 6f
    la      t5, __transition_leaf_end
    bgeu    a3, t5, _transition_fail
    srli    t2, a3, 2
    ori     t2, t2, 1                       # 新 leaf table branch PTE
    sd      t2, 0(t1)
    li      t5, 4096
    add     a3, a3, t5
6:  srli    t2, t2, 10
    slli    t2, t2, 12                      # middle branch → leaf table PA
    srli    t0, t3, 12
    andi    t0, t0, 0x1ff
    slli    t0, t0, 3
    add     t2, t2, t0
    srli    t0, t3, 2
    ori     t0, t0, 0xef                    # level-0 RWX global leaf
    sd      t0, 0(t2)
    ret

_transition_fail:
    wfi
    j       _transition_fail

# 低段跨空间字面量：入口代码经此间接取高半区地址与 ABS 常量
#（R_RISCV_64 无范围限制）。
.section .data.init
.global _BOOT_CONSTS
_BOOT_CONSTS:
    .quad _start_high           # [0] boot 高半区续段 VMA
    .quad __kernel_pa_end       # [1] 内核物理末端

# 高半区跨空间字面量表：高地址代码（Rust）经 PCREL 读内存取值，
# 杜绝对低段符号与 ABS 常量的跨空间寻址（.quad 绝对重定位无范围限制）。
.section .data
.global _ENTRY_CONSTS
_ENTRY_CONSTS:
    .quad SBI_START             # [0] SBI 段物理起点
    .quad STACK_SIZE            # [1] 每 hart 栈大小
    .quad HART_NUM_LIMIT        # [2] hart 数上限
    .quad _awaken               # [3] secondary PA 入口
    .quad __bootstrap_start     # [4] bootstrap 可回收区间起点
    .quad __bootstrap_free_end  # [5] bootstrap 可回收区间终点
    .quad __bootstrap_stack_top # [6] bootstrap 临时栈顶（PA）
    .quad KERNEL_VA_BASE        # [7] 高半区基址
    .quad __bootstrap_stack_top # [8] 同 [6]：_start_high 别名换算用
    .quad __stack_window_base   # [9] 栈窗口基（高半区顶槽 VMA）
    .quad __kernel_pa_end       # [10] 内核静态占用物理末端
    .quad STACK_GUARD           # [11] guard 洞跨度（≥ 审计最大单帧）
    .quad EMERGENCY_SIZE        # [12] emergency 栈大小（占槽顶）
    .quad __transition_root_pa  # [13] transition root PA

.section .text.entry
.global _pa_fatal
# Bare 下早期 fatal vector：任何异常停驻等待复位（无栈、无诊断面）。
_pa_fatal:
1:  wfi
    j       1b

# PA 段永久常量：高半区基址（_awaken 换算 record VA 用）。
.section .data.entry
.global _ENTRY_PA_CONSTS
_ENTRY_PA_CONSTS:
    .quad KERNEL_VA_BASE

.section .text.entry
.global _awaken
# HSM secondary PA 前导（永久设施）：a0 = hartid，a1 = opaque = record PA。
# Acquire 消费 record → 切过渡 satp → 跳高半区。
#
# 并发纪律：cold boot 建表后，在 secondary 可能进入的阶段不改写 transition 表；
# 全员 Online 的同步点只撤销 cold-bootstrap 临时叶，此后永久保持不变。DTB 叶在
# 启动 secondary 前撤销。可见性由 PTE 写屏障及 record 的 Release/Acquire 保证。
_awaken:
    la      t0, _pa_fatal
    csrw    stvec, t0
1:  ld      t2, {REC_STATE}(a1)             # 等 record ≥ Starting
    fence   r, rw                           # acquire：保证可见 boot 早先写下的过渡 PTE
    andi    t2, t2, 0xff
    li      t3, 1                           # Starting
    blt     t2, t3, 1b
    la      t0, __transition_root_pa        # 过渡 root（只读）
    srli    t0, t0, 12
    li      t1, 8 << 60
    or      t0, t0, t1
    csrw    satp, t0
    sfence.vma                              # 过渡翻译同步
    # record VA = PA + KERNEL_VA_BASE（过渡别名下换算；此后正式映射下
    # 只有高半区 VA 可达，_enter_hart_high 统一以 VA 消费 record）。
    la      t5, _ENTRY_PA_CONSTS
    ld      t5, 0(t5)
    add     a1, a1, t5
    ld      t6, {REC_ENTRY_HIGH}(a1)
    jr      t6                              # → _enter_hart_high

# ---------------------------------------------------------------------------
# 高半区段：正式运行环境
# ---------------------------------------------------------------------------

.section .text
.align 4
.global _start_high
# boot 高半区 bootstrap 续段（过渡 satp 别名空间）：清 .bss、装最小环境，
# 进入 Rust main 前半（单核构造 registry/records）。a0 = hartid, a1 = dtb。
_start_high:
    la      t0, _bss_start
    la      t1, _bss_end
1:  sd      zero, 0(t0)                     # 清高半区 .bss
    addi    t0, t0, 8
    bltu    t0, t1, 1b
.option push
.option norelax
    la      gp, __global_pointer$           # kernel gp 规范序列
.option pop
    la      t0, _ENTRY_CONSTS
    ld      sp, 64(t0)                      # bootstrap 栈顶（PA）
    ld      t1, 56(t0)                      # KERNEL_VA_BASE
    add     sp, sp, t1                      # sp 切到高半区别名：bootstrap 阶段
                                            # 全部地址为规范高半区 VA
    la      t0, _bootstrap_fatal
    csrw    stvec, t0                       # 正式环境建立前的兜底 vector
    csrw    sscratch, zero                  # bootstrap 无锚：trap 即 fatal
    call    main                            # rustc lang_start 包装器
1:  wfi                                     # main 不返回；防御性停驻
    j       1b

.align 2
.global _bootstrap_fatal
# bootstrap 阶段 fatal（正式 trap 环境建立前）：最小诊断后停驻。
# 对齐是硬性要求：本地址会装入 stvec，低位非零会改变 mode 字段。
_bootstrap_fatal:
    csrr a0, scause
    csrr a1, stval
    csrr a2, sepc
    call bootstrap_fatal_report
1:  wfi
    j       1b

.align 4
.global _enter_hart_high
# 两条启动路径的唯一汇合点（非返回）：a0 = raw hartid，a1 = &HartBootRecord
# （VA）。切换正式 satp 并同步翻译，建立 gp/tp/sscratch/sp 与 HartLocal
# 初值后进入 Rust formal entry。
_enter_hart_high:
    ld      t0, {REC_KERNEL_SATP}(a1)
    csrw    satp, t0
    sfence.vma                              # 正式地址翻译同步
.option push
.option norelax
    la      gp, __global_pointer$           # kernel gp 规范序列
.option pop
    ld      tp, {REC_HART_LOCAL}(a1)
    ld      sp, {REC_STACK_TOP}(a1)
    csrw    sscratch, tp                    # 正式环境恒 HartLocal
    sd      a0, {HL_HARTID}(tp)
    sd      sp, {HL_KERNEL_SP}(tp)
    ld      t0, {REC_EMERGENCY_SP}(a1)
    sd      t0, {HL_EMERGENCY_SP}(tp)
    ld      t0, {REC_SLOT}(a1)
    sd      t0, {HL_SLOT}(tp)
    li      t0, -1                          # fatal guard = 无 fatal
    sd      t0, {HL_FATAL_GUARD}(tp)
    sd      zero, {HL_FP_ENABLED}(tp)
    mv      a0, a1
    call    hart_formal_entry               # CSR 基线/stvec/Online/分流

# ---------------------------------------------------------------------------
# 共同 trap 入口（direct mode，正式 stvec 恒指此处）
# ---------------------------------------------------------------------------

.align 4
.global _trap_entry
_trap_entry:
    csrrw   t6, sscratch, t6                # t6 = HL；原 t6 → sscratch
    sd      t5, {HL_SCRATCH}(t6)            # 原 t5 → 槽 1
    csrr    t5, sscratch
    sd      t5, {HL_SCRATCH2}(t6)           # 原 t6 → 槽 2
    csrw    sscratch, t6                    # 锚恢复：正式环境恒 HartLocal，
                                            # 同步异常窗口自此处闭合
    # 来源唯一真值：硬件 SPP。SPP=1 一律 fatal——返回用户尾部的同步异常
    # SPP 仍为 1，绝无把内核现场解释成 UserContext 的窗口。检查经已
    # 保存的 t5 中转：进入序列在 UserContext 保存前不得触碰任何未保存
    # 的用户寄存器。
    csrr    t5, sstatus
    srli    t5, t5, 8
    andi    t5, t5, 1
    bnez    t5, _fatal_entry

    # ---- U 态来源：保存 UserContext ----
    ld      t5, {HL_FRAME_PTR}(t6)
    .irp i, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29
    sd      x\i, ({UC_X0} + 8*\i)(t5)
    .endr
    ld      t0, {HL_SCRATCH}(t6)            # 用户 x30
    sd      t0, {UC_X30}(t5)
    ld      t0, {HL_SCRATCH2}(t6)           # 用户 x31
    sd      t0, {UC_X31}(t5)
    # FP 条件保存：Dirty ⇔ 当前线程为 D64 且用过 FP。Base 出口恒 FS=Off，
    # FS=Off 下用户 FP 指令 illegal 而 FS 不变 ⇒ Dirty 不可能出现在 Base；
    # 无 F/D hart 上 FS 恒 Off（WARL），此分支不可达 ⇒ helper 调用安全。
    csrr    t0, sstatus
    srli    t0, t0, 13
    andi    t0, t0, 3
    li      t1, 3
    bne     t0, t1, 1f
    addi    a0, t5, {UC_FP}
    call    _save_fp                        # ctx_fp helper：FPR/fcsr 全量入帧
1:
    li      t0, (3 << 13)
    csrc    sstatus, t0                     # 内核稳态 FS=Off
    csrr    t0, sepc
    sd      t0, {UC_SEPC}(t5)
    # 切内核现场：tp=HL、handler 栈=调度循环栈；stvec 恒共同入口，无需切换；
    # SUM/MXR 稳态恒 0（用户访问走 RAII guard）。
    mv      tp, t6
    ld      sp, {HL_SCHED_SP}(tp)
    csrr    a0, scause
    csrr    a1, stval
    mv      a2, t5
    call    handle_user_trap                # 返回 a0 = Outcome（0 = Resume）
    beqz    a0, _resume_user
    # 非 Resume 出口统一归一（teardown barrier 出口边界）：切内核页表 +
    # 本地全量 SFENCE.VMA。此后 Rust 侧（active 位清除、reap、park 发布）
    # 结构性只运行于内核地址空间——任何新终止来源无需各自记得归一
    # （notes/impls/execution-context.md「地址空间归属纪律」）。
    la      t0, {KERNEL_SATP_SYM}
    ld      t0, 0(t0)
    csrw    satp, t0
    sfence.vma
    # Switch：恢复调度循环现场（112B SchedulerFrame 对称恢复）
    ld      ra, {SF_RA}(sp)
    ld      s0, ({SF_S0} + 8*0)(sp)
    ld      s1, ({SF_S0} + 8*1)(sp)
    ld      s2, ({SF_S0} + 8*2)(sp)
    ld      s3, ({SF_S0} + 8*3)(sp)
    ld      s4, ({SF_S0} + 8*4)(sp)
    ld      s5, ({SF_S0} + 8*5)(sp)
    ld      s6, ({SF_S0} + 8*6)(sp)
    ld      s7, ({SF_S0} + 8*7)(sp)
    ld      s8, ({SF_S0} + 8*8)(sp)
    ld      s9, ({SF_S0} + 8*9)(sp)
    ld      s10, ({SF_S0} + 8*10)(sp)
    ld      s11, ({SF_S0} + 8*11)(sp)
    addi    sp, sp, {SF_SIZE}
    ret                                     # a0 = Outcome 直达 Rust 调用者

.align 4
.global _fatal_entry
# S 态来源（含返回用户尾部的同步异常）：per-hart emergency 栈上建立首帧。
_fatal_entry:
    addi    t1, tp, {HL_FATAL_GUARD}
    li      t2, 1
    amoswap.d.aqrl t0, t2, (t1)             # test-and-set 递归 guard
    li      t3, -1
    beq     t0, t3, 1f                      # 「无 fatal」（old==MAX）→ 首帧保存
2:  wfi                                     # 二次 fatal：首帧已是证据，
    j       2b                              # 无栈停驻等待复位
1:
    # a0/a1 直接入帧，不复用入口槽——槽内是入口序列抢救的原 t5/t6。
    sd      sp, {HL_FATAL_SP}(tp)
    ld      sp, {HL_EMERGENCY_SP}(tp)
    addi    sp, sp, -{FF_SIZE}
    sd      a0, {FF_X10}(sp)
    sd      a1, {FF_X11}(sp)
    mv      a0, sp                          # &FatalFrame
    .irp i, 1,3,4,5,6,7,8,9,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29
    sd      x\i, ({FF_X0} + 8*\i)(a0)
    .endr
    ld      t0, {HL_FATAL_SP}(tp)
    sd      t0, {FF_X2}(a0)                 # 原 sp
    ld      t0, {HL_SCRATCH}(tp)
    sd      t0, {FF_X30}(a0)                # 原 t5（入口槽 1）
    ld      t0, {HL_SCRATCH2}(tp)
    sd      t0, {FF_X31}(a0)                # 原 t6（入口槽 2）
    csrr    t0, scause
    sd      t0, {FF_SCAUSE}(a0)
    csrr    t0, stval
    sd      t0, {FF_STVAL}(a0)
    csrr    t0, sepc
    sd      t0, {FF_SEPC}(a0)
    csrr    t0, satp
    sd      t0, {FF_SATP}(a0)
    csrr    t0, sstatus
    sd      t0, {FF_SSTATUS}(a0)
    call    handle_fatal                    # 完整证据已建立，进入软件诊断

.align 4
.global _ret_to_user
# 调度循环调用（tp = HL，执行点已装）：保存循环现场并记恢复点，
# 切换地址空间（伴随 fence.i），尾接 pre-sret 装帧。
_ret_to_user:
    addi    sp, sp, -{SF_SIZE}
    sd      ra, {SF_RA}(sp)
    .irp i, 0,1,2,3,4,5,6,7,8,9,10,11
    sd      s\i, ({SF_S0} + 8*\i)(sp)
    .endr
    sd      sp, {HL_SCHED_SP}(tp)
    # 换用户地址空间：调度循环在此切换线程。Resume 路径不经此处
    # （同进程返回，satp 未变）；每次空间切换保守执行本地 fence.i，
    # 覆盖该空间任何新发布代码代次的首次执行。
    ld      t0, {HL_USER_SATP}(tp)
    csrw    satp, t0
    sfence.vma
    fence.i
    # fall through → pre-sret 装帧

_resume_user:
# pre-sret CSR 边界 + 用户现场装载（tp = HL；sscratch/stvec 恒定不动）
    ld      t0, {HL_FP_ENABLED}(tp)
    beqz    t0, 1f
    li      t0, {CSR_FS_CLEAN}
    csrs    sstatus, t0                     # 先开 FS=Clean：FS=Off 下 fld 非法
    ld      a0, {HL_FRAME_PTR}(tp)
    addi    a0, a0, {UC_FP}
    call    _restore_fp                     # D64：完整恢复 FPR/fcsr
    j       2f
1:
    li      t0, {CSR_FS_MASK}
    csrc    sstatus, t0                     # Base：FS=Off，从不触碰 FP helper
2:
    li      t0, {CSR_PRE_SRET_CLEAR}
    csrc    sstatus, t0                     # SIE/SPIE/SPP/SUM/MXR 清零、VS=Off；
                                            # U 态忽略 SIE，中断来源由 sie 控制
    # LR/SC reservation 清除：对 per-hart 保留槽 dummy SC（失败即目的达成）
    addi    t0, tp, {HL_RESERVATION}
    lr.d    t1, (t0)
    sc.d    t1, t1, (t0)
    # 装载用户现场
    ld      t5, {HL_FRAME_PTR}(tp)
    ld      t0, {UC_SEPC}(t5)
    csrw    sepc, t0
    ld      x1, ({UC_X0} + 8*1)(t5)
    ld      sp, ({UC_X0} + 8*2)(t5)
    ld      gp, ({UC_X0} + 8*3)(t5)
    ld      tp, ({UC_X0} + 8*4)(t5)
    .irp i, 5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29
    ld      x\i, ({UC_X0} + 8*\i)(t5)
    .endr
    ld      t6, {UC_X31}(t5)                # x31 先于 x30（t5 最后终结）
    ld      t5, {UC_X30}(t5)
    sret
