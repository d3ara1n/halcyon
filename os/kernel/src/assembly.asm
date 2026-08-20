.option norvc
.attribute arch, "rv64gc"

.section .text.init
.global _start
# OpenSBI 进入：a0 = boot hartid，a1 = dtb 物理地址
_start:
    mv      a2, a1              # a2 = dtb
    la      a1, main            # rustc 生成的 lang_start 包装器
    j       _awaken

.section .text
.global _awaken
# _awaken(hartid: a0, entry: a1, dtb: a2)
# primary 经 _start 进入（entry = rustc 包装器，dtb 有效）；
# secondary 经 SBI HSM hart_start 进入（entry = opaque，dtb 无效）。
_awaken:
    # tp = &HART_LOCALS[hartid]（HartLocal 恒 64 字节，见 hart.rs 静态断言）
    la      t0, HART_LOCALS
    slli    t1, a0, 6
    add     tp, t0, t1
    sd      a0, 0(tp)           # HartLocal.hartid
    # sp = _kernel_end - hartid * _stack_size（各 hart 独占栈区）
    la      t0, _stack_size
    mul     t1, a0, t0
    la      sp, _kernel_end
    sub     sp, sp, t1
    sd      sp, 8(tp)           # HartLocal.kernel_sp
    la      t0, _kernel_trap
    csrw    stvec, t0
    li      t0, 0b01 << 13      # sstatus.FS = Initial，SIE = 0（协作式内核）
    csrw    sstatus, t0
    mv      ra, a1
    mv      a1, a2
    ret

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
