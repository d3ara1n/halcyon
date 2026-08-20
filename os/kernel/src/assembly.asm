.option norvc
.attribute arch, "rv64gc"

# ---------------------------------------------------------------------------
# PA 入口段（VMA = LMA = PA）：OpenSBI / SBI HSM 在裸 satp 下跳入。
#
# 纪律：本段代码不得用 `la` 引用高半区符号——PC 相对寻址 ±2GiB，
# 跨空间必溢出；对高半区的引用一律经 .data.init 的 64 位字面量间接。
# raw binary 引导无 ELF 加载器清 bss：各空间 bss 由各自入口清零
#（.bss.init 在此清，高半区 .bss 由 _start_high 清）。
# ---------------------------------------------------------------------------

.section .text.init
.global _start
# OpenSBI 进入：a0 = boot hartid，a1 = dtb PA；bare satp，PC = PA。
# 职责：写跳板 root 表两项（DRAM 槽 identity + 高半区别名），开 MMU，
# 跳高半区。a0/a1 原样带过。
_start:
    # 清 .bss.init（含跳板 root 表），再写入两项
    la      t0, __bss_init_start
    la      t2, __bss_init_end
1:  sd      zero, 0(t0)
    addi    t0, t0, 8
    bltu    t0, t2, 1b
    la      t0, TRAMPOLINE_PG_DIR
    la      t1, _BOOT_CONSTS
    ld      t2, 0(t1)          # 跳板 PTE：V|R|W|X|G|A|D + DRAM 槽基 ppn
    ld      t3, 8(t1)          # DRAM 槽 vpn2 槽号
    slli    t4, t3, 3
    add     t4, t0, t4
    sd      t2, 0(t4)          # root[slot]：identity（切换后旧 PC 仍有效）
    addi    t5, t3, 256
    slli    t5, t5, 3
    add     t5, t0, t5
    sd      t2, 0(t5)          # root[slot+256]：高半区别名（同一物理段）
    ld      t6, 16(t1)         # _start_high 的 VMA 字面量
    srli    t0, t0, 12         # satp.ppn = 表 PA >> 12
    li      t1, 8 << 60        # satp.mode = Sv39
    or      t0, t0, t1
    csrw    satp, t0
    sfence.vma
    jr      t6

.global _awaken
# HSM secondary 入口：a0 = hartid，a1 = opaque（secondary_entry VMA）。
# 跳板表项由 boot hart 在 _start 写好；这里只装跳板 satp 跳高半区。
_awaken:
    la      t0, TRAMPOLINE_PG_DIR
    srli    t0, t0, 12
    li      t1, 8 << 60
    or      t0, t0, t1
    csrw    satp, t0
    sfence.vma
    la      t1, _BOOT_CONSTS
    ld      t0, 24(t1)         # _awaken_high 的 VMA 字面量
    jr      t0

# ---------------------------------------------------------------------------
# 高半区段：跳板 satp（boot 早期）或正式内核表（其后）下执行
# ---------------------------------------------------------------------------

.section .text
# hart 装配：tp/sp/stvec/sstatus（约定见 hart.rs）
.macro HART_SETUP
    la      t0, HART_LOCALS
    slli    t1, a0, 6
    add     tp, t0, t1
    sd      a0, 0(tp)           # HartLocal.hartid
    la      t3, _PA_CONSTS
    ld      t0, 8(t3)           # STACK_SIZE
    mul     t1, a0, t0
    la      sp, _kernel_end
    sub     sp, sp, t1
    sd      sp, 8(tp)           # HartLocal.kernel_sp
    la      t0, _kernel_trap
    csrw    stvec, t0
    # sstatus 只动本宏职责位：SIE 清零（协作式）、FS 置 Initial；
    # 不整写——SUM 等其余位由 mm::init 管理，secondary 启动时已置位。
    li      t0, 0b10
    csrc    sstatus, t0
    li      t0, 3 << 13
    csrc    sstatus, t0
    li      t0, 1 << 13
    csrs    sstatus, t0
.endm

.global _start_high
# boot hart 高半区继续：a0 = hartid，a1 = dtb PA；跳板 satp。
# 正式内核表由 Rust 侧 mm::init 构建并切换。
_start_high:
    # 清高半区 .bss（同空间 la，无跨空间溢出）
    la      t0, _bss_start
    la      t2, _bss_end
1:  sd      zero, 0(t0)
    addi    t0, t0, 8
    bltu    t0, t2, 1b
    HART_SETUP
    mv      a2, a1              # dtb
    la      a1, main            # rustc lang_start 包装器
    mv      ra, a1
    mv      a1, a2
    ret                         # → main(argc=hartid, argv=dtb)

.global _awaken_high
# secondary 高半区继续：a0 = hartid，a1 = secondary_entry。
# 切到正式内核表（boot hart 的 mm::init 已写好 KERNEL_SATP）。
_awaken_high:
    la      t0, KERNEL_SATP
    ld      t0, 0(t0)
    csrw    satp, t0
    sfence.vma
    HART_SETUP
    mv      ra, a1
    ret                         # → secondary_entry(a0=hartid)

.section .text
.align 4
.global _kernel_trap
# 协作式内核：内核态 trap 一律视为致命，现场转储后停机
_kernel_trap:
    csrr    a0, scause
    csrr    a1, stval
    csrr    a2, sepc
    call    handle_kernel_trap
1:  wfi
    j       1b

# ---------------------------------------------------------------------------
# 用户态 trap 与返回（契约见 trap.rs；偏移与 HartLocal::off 绑定）
# ---------------------------------------------------------------------------

.section .text
.align 4
.global _user_trap
# 从用户态陷入：sscratch = HartLocal。存用户现场 → 切内核栈 → 进 Rust。
_user_trap:
    csrrw  t6, sscratch, t6        # t6 = HartLocal；sscratch = 用户 t6
    sd     t5, 40(t6)              # HL_SCRATCH      # 用户 t5 暂存锚槽
    ld     t5, 24(t6)              # HL_FRAME        # t5 = TrapFrame*
    sd     x1, 8(t5)
    sd     x2, 16(t5)
    sd     x3, 24(t5)
    sd     x4, 32(t5)              # 用户 tp
    sd     x5, 40(t5)
    sd     x6, 48(t5)
    sd     x7, 56(t5)
    sd     x8, 64(t5)
    sd     x9, 72(t5)
    sd     x10, 80(t5)
    sd     x11, 88(t5)
    sd     x12, 96(t5)
    sd     x13, 104(t5)
    sd     x14, 112(t5)
    sd     x15, 120(t5)
    sd     x16, 128(t5)
    sd     x17, 136(t5)
    sd     x18, 144(t5)
    sd     x19, 152(t5)
    sd     x20, 160(t5)
    sd     x21, 168(t5)
    sd     x22, 176(t5)
    sd     x23, 184(t5)
    sd     x24, 192(t5)
    sd     x27, 216(t5)
    sd     x28, 224(t5)
    sd     x29, 232(t5)
    ld     t0, 40(t6)              # HL_SCRATCH     # t0 = 用户 x30（进入时暂存）
    sd     t0, 240(t5)
    csrr   t0, sscratch            # t0 = 用户 x31（进入时交换暂存）
    sd     t0, 248(t5)
    # 浮点条件保存：FS==Dirty 才存，存后置 Clean（内核不用浮点，fcsr 不动）
    csrr   t0, sstatus
    srli   t0, t0, 13
    andi   t0, t0, 3
    li     t1, 3
    bne    t0, t1, 1f
    .irp i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    fsd    f\i, 256+8*\i(t5)
    .endr
    li     t0, 3 << 13
    csrc   sstatus, t0
    li     t0, 2 << 13
    csrs   sstatus, t0
1:  csrr   t0, sepc
    sd     t0, 512(t5)             # TF_SEPC
    # 内核现场：tp 换锚、sp 切调度栈、stvec 指内核致命路径
    mv     tp, t6
    ld     sp, 16(t6)              # HL_SCHED_SP
    la     t0, _kernel_trap
    csrw   stvec, t0
    csrr   a0, scause
    csrr   a1, stval
    mv     a2, t5
    call   handle_user_trap        # 返回 a0 = Outcome（0 = Resume）
    bnez   a0, 2f
    j      _resume_user            # Resume：tp 在 Rust 调用中保持不变
2:  # Switch：恢复调度循环现场（sched_sp 起 ra + s0..s11）
    ld     ra, 0(sp)
    ld     s0, 8(sp)
    ld     s1, 16(sp)
    ld     s2, 24(sp)
    ld     s3, 32(sp)
    ld     s4, 40(sp)
    ld     s5, 48(sp)
    ld     s6, 56(sp)
    ld     s7, 64(sp)
    ld     s8, 72(sp)
    ld     s9, 80(sp)
    ld     s10, 88(sp)
    ld     s11, 96(sp)
    addi   sp, sp, 104
    ret

.align 4
.global _ret_to_user
# 调度循环调用（tp = HartLocal，执行点已装）：保存循环现场并记恢复点，
# 尾接 _resume_user 装帧 sret；Switch 发生时经恢复点正常返回。
_ret_to_user:
    addi   sp, sp, -104
    sd     ra, 0(sp)
    sd     s0, 8(sp)
    sd     s1, 16(sp)
    sd     s2, 24(sp)
    sd     s3, 32(sp)
    sd     s4, 40(sp)
    sd     s5, 48(sp)
    sd     s6, 56(sp)
    sd     s7, 64(sp)
    sd     s8, 72(sp)
    sd     s9, 80(sp)
    sd     s10, 88(sp)
    sd     s11, 96(sp)
    sd     sp, 16(tp)              # HL_SCHED_SP
    # 换用户地址空间：调度循环在此处切换线程，用户 satp 从锚装入。
    # （Resume 路径不经此段——同进程返回，satp 未变。）
    ld     t0, 32(tp)              # HL_USER_SATP
    csrw   satp, t0
    sfence.vma
    # 尾接装帧

_resume_user:
# 装帧回用户态（tp = HartLocal；用户态锚 sscratch = tp、stvec = _user_trap）
    la     t0, _user_trap
    csrw   stvec, t0
    ld     t5, 24(tp)              # HL_FRAME
    csrw   sscratch, tp
    # 浮点条件恢复：FS==Clean → 恢复 + 置 Dirty；否则置 Initial
    csrr   t0, sstatus
    srli   t0, t0, 13
    andi   t0, t0, 3
    li     t1, 2
    bne    t0, t1, 3f
    .irp i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    fld    f\i, 256+8*\i(t5)
    .endr
    li     t0, 3 << 13
    csrc   sstatus, t0
    li     t0, 3 << 13
    csrs   sstatus, t0
    j      4f
3:  li     t0, 3 << 13
    csrc   sstatus, t0
    li     t0, 1 << 13
    csrs   sstatus, t0
4:  ld     t0, 512(t5)             # TF_SEPC
    csrw   sepc, t0
    # 恢复通用：t6 先备好不再用，t5 基址最后恢复
    ld     x1, 8(t5)
    ld     sp, 16(t5)
    ld     gp, 24(t5)
    ld     tp, 32(t5)
    ld     x5, 40(t5)
    ld     x6, 48(t5)
    ld     x7, 56(t5)
    ld     x8, 64(t5)
    ld     x9, 72(t5)
    ld     x10, 80(t5)
    ld     x11, 88(t5)
    ld     x12, 96(t5)
    ld     x13, 104(t5)
    ld     x14, 112(t5)
    ld     x15, 120(t5)
    ld     x16, 128(t5)
    ld     x17, 136(t5)
    ld     x18, 144(t5)
    ld     x19, 152(t5)
    ld     x20, 160(t5)
    ld     x21, 168(t5)
    ld     x22, 176(t5)
    ld     x23, 184(t5)
    ld     x24, 192(t5)
    ld     x27, 216(t5)
    ld     x28, 224(t5)
    ld     x29, 232(t5)
    ld     t6, 248(t5)             # x31（t6 此后不再作锚）
    ld     t5, 240(t5)             # x30 最后恢复（t5 基址终结）
    sret

# ---------------------------------------------------------------------------
# PA 段数据：跳板 root 表与跨空间字面量
# ---------------------------------------------------------------------------

# 高半区链接期常量表：高地址代码（Rust/汇编）经 PCREL 读内存取值，
# 杜绝对低段符号与 ABS 常量的跨空间寻址（.quad 绝对重定位无范围限制）
.section .data
.global _PA_CONSTS
_PA_CONSTS:
    .quad SBI_START         # [0] SBI 段物理起点
    .quad STACK_SIZE        # [1] 每 hart 栈大小
    .quad HART_NUM_LIMIT    # [2] hart 数上限
    .quad _awaken           # [3] secondary PA 入口

# 低段跨空间字面量：入口代码经此间接取高半区地址（R_RISCV_64 无范围限制）
.section .data.init
.global _BOOT_CONSTS
_BOOT_CONSTS:
    .quad _trampoline_pte       # (DRAM 槽 PA >> 2) | 0xEF
    .quad _trampoline_slot      # DRAM 槽 vpn2 槽号
    .quad _start_high
    .quad _awaken_high

.section .bss.init
.align 12
.global TRAMPOLINE_PG_DIR
TRAMPOLINE_PG_DIR:
    .zero 4096