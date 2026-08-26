OUTPUT_ARCH("riscv")
ENTRY(main)

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
