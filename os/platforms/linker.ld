OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    HART_NUM_LIMIT = 8;

    /* 内核高半区偏移（VMA = PA + KERNEL_VA_BASE）与跳板几何（由加载基址推导） */
    KERNEL_VA_BASE = 0xFFFFFFC000000000;
    TRAMPOLINE_PA_BASE = KERNEL_PA_START & 0xFFFFFFFFC0000000;
    _trampoline_pte = (TRAMPOLINE_PA_BASE >> 2) | 0xEF;
    _trampoline_slot = TRAMPOLINE_PA_BASE >> 30;

    /* ---- PA 段：裸跳入口与跳板机构（VMA = LMA）---- */
    .text.init KERNEL_PA_START : { *(.text.init) }
    .data.init : { *(.data.init) }
    .bss.init (NOLOAD) : {
        PROVIDE(__bss_init_start = .);
        *(.bss.init)
        . = ALIGN(8);
        PROVIDE(__bss_init_end = .);
    }

    /* ---- 高半区段：VMA = PA + KERNEL_VA_BASE，LMA = PA ---- */
    . = ADDR(.bss.init) + SIZEOF(.bss.init) + KERNEL_VA_BASE;

    .text : AT(ADDR(.text) - KERNEL_VA_BASE) ALIGN(8) {
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VA_BASE) ALIGN(8) {
        *(.srodata)
        *(.rodata)
    }

    .data : AT(ADDR(.data) - KERNEL_VA_BASE) ALIGN(8) {
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
