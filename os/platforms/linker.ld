OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    HART_NUM_LIMIT = 8;

    /* 内核高半区偏移（VMA = PA + KERNEL_VA_BASE）与过渡页表几何（由加载基址推导，
       boot 与 secondary 共用同一张过渡表，见 assembly.asm `_start`/`_awaken`） */
    KERNEL_VA_BASE = 0xFFFFFFC000000000;
    TRAMPOLINE_PA_BASE = KERNEL_PA_START & 0xFFFFFFFFC0000000;
    _trampoline_pte = (TRAMPOLINE_PA_BASE >> 2) | 0xEF;
    _trampoline_slot = TRAMPOLINE_PA_BASE >> 30;

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
        . = . + 16384;
        PROVIDE(__bootstrap_stack_top = .);   /* 栈向下生长：符号在保留区顶部 */
        . = ALIGN(8);
    }

    /* ---- 永久 hart-entry 设施（VMA = LMA = PA，不随 bootstrap 回收）----
       共享过渡页表 root、secondary PA 前导与早期 fatal vector */
    .bss.entry (NOLOAD) : ALIGN(4096) {
        PROVIDE(__transition_root_pa = .);
        . = . + 4096;               /* 过渡页表 root（Sv39 一页）*/
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
        *(.sbss)
        *(.bss)
        . = ALIGN(8);
        PROVIDE(_bss_end = .);
    }

    .runtime (NOLOAD) : AT(ADDR(.runtime) - KERNEL_VA_BASE) ALIGN(16) {
        PROVIDE(_stack_start = .);
        . = . + STACK_SIZE * HART_NUM_LIMIT;
        . = ALIGN(4096);
        PROVIDE(_kernel_end = .);
    }

    /DISCARD/ : {
        *(.eh_frame)
    }
}
