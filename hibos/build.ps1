# Hib OS 构建脚本（PowerShell）
# 前提：已安装 nasm、qemu-system-x86_64、rust-objcopy / llvm-objcopy
# 安装方式（Windows）：
#   choco install nasm qemu llvm   # 或从官网下载并加入 PATH
#   rustup target add x86_64-unknown-none

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $root

Write-Host "[Hib OS] 检查 Rust 目标 x86_64-unknown-none ..." -ForegroundColor Cyan
$targets = rustup target list --installed
if ($targets -notcontains "x86_64-unknown-none") {
    Write-Host "[Hib OS] 目标未安装，正在添加 ..." -ForegroundColor Yellow
    rustup target add x86_64-unknown-none
}

Write-Host "[Hib OS] 编译裸机内核 (x86_64-unknown-none) ..." -ForegroundColor Cyan
cargo build --release --target x86_64-unknown-none

$kernelElf = "target/x86_64-unknown-none/release/hibos"
$kernelBin = "target/x86_64-unknown-none/release/hibos.bin"
$bootBin   = "boot/boot.bin"
$img       = "hibos.img"

if (-not (Test-Path $kernelElf)) {
    Write-Error "内核 ELF 未生成: $kernelElf"
}

Write-Host "[Hib OS] ELF 转二进制 ..." -ForegroundColor Cyan
# 优先使用 rust-objcopy，其次 llvm-objcopy
if (Get-Command "rust-objcopy.exe" -ErrorAction SilentlyContinue) {
    rust-objcopy -O binary $kernelElf $kernelBin
} elseif (Get-Command "llvm-objcopy.exe" -ErrorAction SilentlyContinue) {
    llvm-objcopy -O binary $kernelElf $kernelBin
} else {
    Write-Error "未找到 objcopy，请安装 llvm 或 rust-llvm-tools"
}

Write-Host "[Hib OS] 编译 bootloader (boot.asm -> boot.bin) ..." -ForegroundColor Cyan
if (Get-Command "nasm.exe" -ErrorAction SilentlyContinue) {
    nasm -f bin boot/boot.asm -o $bootBin
} else {
    Write-Error "未找到 nasm，请安装 nasm 后再运行本脚本"
}

Write-Host "[Hib OS] 组装可启动镜像 hibos.img ..." -ForegroundColor Cyan
# boot sector (512B) + 内核二进制
Copy-Item $bootBin $img
# 将内核追加到镜像（boot.asm 中从第2扇区开始读取）
# 这里简单追加；实际可按扇区对齐
$kernelBytes = [System.IO.File]::ReadAllBytes($kernelBin)
[System.IO.File]::AppendAllBytes($img, $kernelBytes)

Write-Host "[Hib OS] 镜像已生成: $img" -ForegroundColor Green
Write-Host "[Hib OS] 启动 QEMU ..." -ForegroundColor Cyan
qemu-system-x86_64 -drive format=raw,file=$img -serial stdio -vga stdio
