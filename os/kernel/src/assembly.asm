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
    li      t0, 0b01 << 13      # sstatus.FS = Initial，SIE = 0（协作式内核）
    csrw    sstatus, t0
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