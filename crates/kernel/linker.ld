ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5); /* read + execute */
    rodata PT_LOAD FLAGS(4); /* read */
    data   PT_LOAD FLAGS(6); /* read + write */
}

SECTIONS
{
    . = 0x00200000;
    __kernel_start = .;

    .text ALIGN(0x1000) :
    {
        KEEP(*(.text._start))
        *(.text .text.*)
    } :text

    .rodata ALIGN(0x1000) :
    {
        KEEP(*(.rodata.bootstrap))
        *(.rodata .rodata.*)
    } :rodata

    .data ALIGN(0x1000) :
    {
        KEEP(*(.data.bootstrap))
        *(.data .data.*)
        *(.got .got.*)
        *(.data.rel.ro .data.rel.ro.*)
    } :data

    __kernel_end = ALIGN(0x1000);

    .bss ALIGN(0x1000) (NOLOAD) :
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
