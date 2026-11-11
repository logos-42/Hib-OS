; Hib OS BIOS bootloader (16-bit real mode -> 32-bit protected mode -> long mode)
; 加载内核到 0x100000 (1MB)，切换到 64-bit 长模式，跳转到 kernel_main。

[bits 16]
[org 0x7C00]

start:
    cli
    mov ax, 0
    mov ss, ax
    mov sp, 0x7C00
    mov ds, ax
    mov es, ax
    sti

    ; 显示启动消息
    mov si, msg_boot
    call print

    ; 加载内核：从磁盘第2扇区开始读，512 扇区到 0x100000
    ; (这里简化：实际可配合 grub/limine，这里仅作原型演示)
    mov ax, 0x1000      ; ES = 0x1000，目标段 = 0x10000
    mov es, ax
    mov bx, 0           ; BX = 0，目标偏移
    mov ah, 0x02        ; 读磁盘扇区
    mov al, 128         ; 最多读 128 扇区 (64KB)
    mov ch, 0           ; 柱面 0
    mov cl, 2           ; 扇区 2 (第1扇区是 bootloader)
    mov dh, 0           ; 磁头 0
    mov dl, 0           ; 驱动器 0 (软盘/第一个硬盘)
    int 0x13
    jc disk_error

    ; 切换到保护模式
    cli
    lgdt [gdt64_ptr]

    mov eax, cr0
    or eax, 1
    mov cr0, eax

    ; 远跳转到 64-bit 代码段
    jmp 0x08:long_mode

[bits 64]
long_mode:
    ; 在 64-bit 模式下，RSP 尚未设置，先设一个临时栈
    mov ax, 0x10
    mov ss, ax
    mov rsp, 0x7C00
    mov ds, ax
    mov es, ax

    ; 跳转到 Rust 内核入口
    extern kernel_main
    call kernel_main

    hlt

disk_error:
    mov si, msg_disk
    call print
    hlt

print:
    lodsb
    or al, al
    jz .done
    mov ah, 0x0E
    int 0x10
    jmp print
.done:
    ret

msg_boot: db "Hib OS bootloader...", 0
msg_disk: db "Disk error!", 0

gdt64:
    dq 0                                ; 空描述符
    dq 0x00AF9A000000FFFF               ; 代码段 (64-bit, 可执行, 环0)
    dq 0x00CF92000000FFFF               ; 数据段 (64-bit, 可读写, 环0)
gdt64_ptr:
    dw $ - gdt64 - 1
    dq gdt64

times 510 - ($ - $$) db 0
dw 0xAA55
