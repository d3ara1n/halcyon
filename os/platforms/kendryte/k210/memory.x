MEMORY
{
    SRAM (rwx) : ORIGIN = 0x80020000, LENGTH = 6M
}

HEAP_SIZE = 0xC000;
STACK_SIZE = 0x4000;

REGION_ALIAS("RAM", SRAM);