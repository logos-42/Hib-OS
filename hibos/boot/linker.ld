OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kernel_main)

SECTIONS
{
    /* 1MB 处开始，方便 BIOS 引导加载 */
    . = 0x100000;

    _kernel_start = .;

    .text : {
        *(.text)
        *(.text.*)
    }

    .rodata : {
        *(.rodata)
        *(.rodata.*)
    }

    .data : {
        *(.data)
        *(.data.*)
    }

    .bss : {
        *(.bss)
        *(.bss.*)
        *(COMMON)
    }

    _kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.comment)
        *(.note.*)
    }
}
