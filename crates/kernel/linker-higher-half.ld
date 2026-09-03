ENTRY(_start)

PHYSICAL_BASE = 0x00200000;
VIRTUAL_OFFSET = 0xffffffff80000000;
VIRTUAL_BASE = VIRTUAL_OFFSET + PHYSICAL_BASE;

PHDRS
{
    text   PT_LOAD FLAGS(5); /* read + execute */
    rodata PT_LOAD FLAGS(4); /* read */
    data   PT_LOAD FLAGS(6); /* read + write */
}

SECTIONS
{
    . = VIRTUAL_BASE;
    __kernel_start = .;

    .text ALIGN(0x1000) : AT(ADDR(.text) - VIRTUAL_OFFSET)
    {
        KEEP(*(.text._start))
        *(.text .text.*)
    } :text

    .rodata ALIGN(0x1000) : AT(ADDR(.rodata) - VIRTUAL_OFFSET)
    {
        KEEP(*(.rodata.bootstrap))
        *(.rodata .rodata.*)
    } :rodata

    .data ALIGN(0x1000) : AT(ADDR(.data) - VIRTUAL_OFFSET)
    {
        KEEP(*(.data.bootstrap))
        *(.data .data.*)
        *(.got .got.*)
        *(.data.rel.ro .data.rel.ro.*)
    } :data

    __kernel_end = ALIGN(0x1000);

    .bss ALIGN(0x1000) (NOLOAD) : AT(ADDR(.bss) - VIRTUAL_OFFSET)
    {
        KEEP(*(.bss.bootstrap))
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ :
    {
        *(.eh_frame .eh_frame.*)
        *(.comment)
    }
}

ASSERT(ADDR(.text) == VIRTUAL_BASE, "higher-half entry base changed")
ASSERT(LOADADDR(.text) == PHYSICAL_BASE, "higher-half physical base changed")
ASSERT((ADDR(.text) - LOADADDR(.text)) == VIRTUAL_OFFSET, "text translation changed")
ASSERT((ADDR(.rodata) - LOADADDR(.rodata)) == VIRTUAL_OFFSET, "rodata translation changed")
ASSERT((ADDR(.data) - LOADADDR(.data)) == VIRTUAL_OFFSET, "data translation changed")
ASSERT((ADDR(.bss) - LOADADDR(.bss)) == VIRTUAL_OFFSET, "bss translation changed")
