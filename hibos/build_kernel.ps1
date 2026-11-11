# Hib OS 内核构建脚本（PowerShell）
# 用途：将 no_std 内核编译为裸机 ELF，结合 BIOS bootloader，生成可启动镜像 hibos.img
#
# 前提工具（Windows，可用 choco 安装）：
#   choco install nasm qemu llvm
#   # 或手动下载并加入 PATH：nasm, qemu-system-x86_64, llvm-objcopy (llvm)
# Rust 目标：
#   rustup target add x86_64-unknown-none

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $root

Write-Host "[Hib OS Kernel] 检查 Rust 目标 x86_64-unknown-none ..." -ForegroundColor Cyan
$targets = rustup target list --installed
if ($targets -notcontains "x86_64-unknown-none") {
    Write-Host "[Hib OS Kernel] 目标未安装，正在添加 ..." -ForegroundColor Yellow
    rustup target add x86_64-unknown-none
}

Write-Host "[Hib OS Kernel] 编译 no_std 内核 (kernel/Cargo.toml) ..." -ForegroundColor Cyan
Push-Location kernel
cargo build --release --target x86_64-unknown-none
Pop-Location

$kernelElf = "kernel/target/x86_64-unknown-none/release/hibos_kernel"
$kernelBin = "kernel/target/x86_64-unknown-none/release/hibos_kernel.bin"
$bootBin   = "boot/boot.bin"
$img       = "hibos.img"

if (-not (Test-Path $kernelElf)) {
    Write-Error "内核 ELF 未生成: $kernelElf"
}

Write-Host "[Hib OS Kernel] ELF -> 二进制 (llvm-objcopy) ..." -ForegroundColor Cyan
if (Get-Command "llvm-objcopy.exe" -ErrorAction SilentlyContinue) {
    llvm-objcopy -O binary $kernelElf $kernelBin
} elseif (Get-Command "rust-objcopy.exe" -ErrorAction SilentlyContinue) {
    rust-objcopy -O binary $kernelElf $kernelBin
} else {
    Write-Error "未找到 objcopy，请安装 llvm 或 rust-llvm-tools (rustup component add llvm-tools)"
}

Write-Host "[Hib OS Kernel] 编译 BIOS bootloader (boot.asm -> boot.bin) ..." -ForegroundColor Cyan
if (Get-Command "nasm.exe" -ErrorAction SilentlyContinue) {
    nasm -f bin boot/boot.asm -o $bootBin
} else {
    Write-Error "未找到 nasm，请安装 nasm 后再运行本脚本"
}

Write-Host "[Hib OS Kernel] 组装可启动镜像 hibos.img ..." -ForegroundColor Cyan
# boot sector (512B) + 内核二进制（从磁盘第2扇区起，由 boot.asm 读取）
Copy-Item $bootBin $img
$kernelBytes = [System.IO.File]::ReadAllBytes($kernelBin)
[System.IO.File]::AppendAllBytes($img, $kernelBytes)

Write-Host "[Hib OS Kernel] 镜像已生成: $img" -ForegroundColor Green
Write-Host "[Hib OS Kernel] 启动 QEMU (串口输出到终端，VGA 显示启动画面) ..." -ForegroundColor Cyan
qemu-system-x86_64 `
    -drive format=raw,file=$img `
    -serial stdio `
    -vga stdio `
    -m 256M
