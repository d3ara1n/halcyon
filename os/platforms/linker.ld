OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    HART_NUM_LIMIT = 8;

    /* 内核高半区偏移与过渡页表几何。内核与 DTB 各最多跨两个 1GiB 槽，
       boot 与 secondary 共用固定 arena（见 assembly.asm `_start`/`_awaken`）。 */
    KERNEL_VA_BASE = 0xFFFFFFC000000000;
    TRANSITION_MIDDLE_MAX = 4;
    TRANSITION_KERNEL_LEAF_MAX = 16;
    TRANSITION_DTB_LEAF_MAX = 2;
    TRANSITION_LEAF_MAX = TRANSITION_KERNEL_LEAF_MAX + TRANSITION_DTB_LEAF_MAX;

    /* ---- cold-bootstrap 区间（VMA = LMA = PA；全员 Online 后
       [__bootstrap_start, __bootstrap_free_end) 整批回收帧池）----
       _start 必须位于加载地址起点（OpenSBI 跳入点）。 */
    .bootstrap.text KERNEL_PA_START : {
        PROVIDE(__bootstrap_start = .);
        *(.text.init)
    }
    .bootstrap.data : { *(.data.init) }
    .bootstrap.bss (NOLOAD) : {
        *(.bss.init)
        . = ALIGN(16);
        . = . + 32768;
        PROVIDE(__bootstrap_stack_top = .);   /* 栈向下生长：符号在保留区顶部 */
        . = ALIGN(8);
    }

    /* ---- 永久 hart-entry 设施（VMA = LMA = PA，不随 bootstrap 回收）----
       共享过渡 root、中间表、固定叶表 arena、secondary PA 前导与早期 fatal vector */
    .bss.entry (NOLOAD) : ALIGN(4096) {
        PROVIDE(__transition_root_pa = .);
        . = . + 4096;
        PROVIDE(__transition_middle_base = .);
        . = . + 4096 * TRANSITION_MIDDLE_MAX;
        PROVIDE(__transition_middle_end = .);
        PROVIDE(__transition_leaf_base = .);
        . = . + 4096 * TRANSITION_LEAF_MAX;
        PROVIDE(__transition_leaf_end = .);
    }
    .text.entry : {
        *(.text.entry)
    }
    .data.entry : {
        *(.data.entry)
    }
    PROVIDE(__bootstrap_free_end = ADDR(.bss.entry));

    /* ---- 高半区段：VMA = PA + KERNEL_VA_BASE，LMA = PA ---- */
    . = ADDR(.data.entry) + SIZEOF(.data.entry) + KERNEL_VA_BASE;

    .text.ctx_fp : AT(ADDR(.text.ctx_fp) - KERNEL_VA_BASE) {
        *(.text.ctx_fp)
    }

    .text : AT(ADDR(.text) - KERNEL_VA_BASE) ALIGN(8) {
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VA_BASE) ALIGN(8) {
        *(.srodata)
        *(.rodata)
    }

    .data : AT(ADDR(.data) - KERNEL_VA_BASE) ALIGN(8) {
        __global_pointer$ = . + 0x800;
        *(.sdata)
        *(.data)
    }

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VA_BASE) ALIGN(8) {
        PROVIDE(_bss_start = .);
        /* 后缀变体必须显式收编：漏收则成孤儿段被排在段外，
           物理占用脱离 _bss_end 记账（栈打包基址会踩上去）。 */
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        . = ALIGN(8);
        PROVIDE(_bss_end = .);
    }

    /* 栈窗口布局真值（数字只写此处；Rust 经 _ENTRY_CONSTS、audit_elf.py
       从 ELF 符号表读取同一值）：
       - STACK_GUARD：guard 洞跨度，必须 ≥ 构建审计允许的单函数最大帧
         （audit_elf.py 构建期强制），否则一次 sp 下调可越过 guard 洞；
       - EMERGENCY_SIZE：fatal 路径专用 emergency 栈，占槽顶。
       每槽 VA 布局（stride = STACK_SIZE + 2*STACK_GUARD）：
         [ 槽底 guard | formal | emergency guard | emergency ]
       物理侧每槽连续打包 STACK_SIZE（formal+emergency 相邻），guard 纯虚拟。 */
    STACK_GUARD = 0x2000;
    EMERGENCY_SIZE = 0x1000;
    STACK_WINDOW_VA_BASE = 0xFFFFFFFFC0000000;
    .stack_window STACK_WINDOW_VA_BASE (NOLOAD) : ALIGN(4096) {
        PROVIDE(__stack_window_base = .);
        . = . + (STACK_SIZE + 2 * STACK_GUARD) * HART_NUM_LIMIT;
        PROVIDE(__stack_window_end = .);
    }

    /* 内核静态占用的物理末端（SBI + 镜像 + 栈），帧池注册剔除区间用；
       页对齐保证栈打包基址与帧粒度一致。 */
    PROVIDE(__kernel_pa_end = ALIGN(ADDR(.bss) - KERNEL_VA_BASE + SIZEOF(.bss)
                                    + STACK_SIZE * HART_NUM_LIMIT, 4096));
    ASSERT((((__kernel_pa_end - 1) >> 21) - (KERNEL_PA_START >> 21) + 1)
           <= TRANSITION_KERNEL_LEAF_MAX,
           "kernel physical span exceeds transition leaf-table budget")

    /DISCARD/ : {
        *(.eh_frame)
    }
}
