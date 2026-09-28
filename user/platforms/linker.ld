OUTPUT_ARCH("riscv")
ENTRY(_start)

SECTIONS
{
  . = ALIGN(4096);
  .text : {
    *(.text .text.*)
  }

  . = ALIGN(4096);
  .rodata : {
    *(.srodata .srodata.*)
    *(.rodata .rodata.*)
  }

  . = ALIGN(4096);
  .data : {
    *(.sdata .sdata.*)
    *(.data .data.*)
    PROVIDE(__global_pointer$ = . + 0x800);
  }

  . = ALIGN(4096);
  .bss (NOLOAD) : {
    PROVIDE(_bss_start = .);
    *(.sbss .sbss.*)
    *(.bss .bss.*)
    *(COMMON)
    PROVIDE(_bss_end = .);
  }

  /DISCARD/ : {
    *(.eh_frame .eh_frame.*)
  }
}
