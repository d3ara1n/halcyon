OUTPUT_ARCH(riscv)
ENTRY(_start)

INCLUDE "../artifacts/memory.x"

SECTIONS
{
    PROVIDE(_memory_start = ORIGIN(SBI));
    PROVIDE(_kernel_start = ORIGIN(RAM));
    PROVIDE(_stack_size = STACK_SIZE);

    HART_NUM_LIMIT = 8;

    .text : ALIGN(8) {
        *(.text.init)
        *(.text.*)
        . = ALIGN(0x1000);
        _trampoline_start = .;
        *(.trampoline.user_trap)
        *(.trampoline.*)
        . = ALIGN(0x1000);
    } > RAM

    .rodata : ALIGN(8) {
        *(.srodata)
        *(.rodata)
    } > RAM


    .data : ALIGN(8) {
        *(.sdata)
        *(.data)
    } > RAM


    .bss (NOLOAD) : ALIGN(8) {
        PROVIDE(_bss_start = .);
        *(.sbss)
        *(.bss)
        PROVIDE(_bss_end = .);
    } > RAM

    .runtime (NOLOAD) : ALIGN(16) {
        PROVIDE(_stack_start = .);
        . = . + STACK_SIZE * HART_NUM_LIMIT;
        . = ALIGN(4096);  
        PROVIDE(_kernel_end = .);
        PROVIDE(_frame_start = .);
    } > RAM

    /DISCARD/ : {
        *(.eh_frame)
    }
    
    PROVIDE(_memory_end = ORIGIN(RAM) + LENGTH(RAM));
}