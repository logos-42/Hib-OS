# 真像素 GUI（`gui` feature）

Hib OS 不再只有终端 ASCII 假桌面，而是一套真正的像素 GUI 栈：

| 模块 | 说明 |
|---|---|
| `kernel/gui/surface.rs` | 32bpp ARGB 表面，alpha 混合、矩形/圆角/渐变、5×7 字体光栅化。 |
| `kernel/gui/window.rs` | 窗口对象，每个窗口拥有独立客户端表面；标题栏/关闭/最小化/最大化/缩放按钮；命中测试；内置终端模型（键盘 → 窗口 → 像素闭环）。 |
| `kernel/gui/compositor.rs` | 壁纸、z-order、焦点、自上而下命中测试、窗口提升/置顶、损坏区域合成、光标。 |
| `kernel/gui/input.rs` | 键盘/鼠标事件队列，按 z-order 路由到焦点窗口；标题栏拖动、右下角缩放、滚轮滚动。 |
| `kernel/gui/bmp.rs` | 24 位 BMP 编码/保存，零依赖，可直接在资源管理器查看。 |
| `kernel/gui/backend.rs` | （`--features gui`）winit 0.30 + softbuffer 0.4.8 真实窗口，最近邻缩放，400ms 节流实时刷新。 |

三者共享同一份像素：终端 ANSI 预览、`gui shot` 导出的 BMP、`gui run` 打开的真实窗口，都由同一个 `Compositor` 合成。

## 快速上手

> `gui` 是**可选 feature**，默认构建（`cargo run`）不含真实窗口后端。此时输入 `gui run`
> 会提示"真实窗口后端未编译"——这不是 GUI 失败，只是没编译进去。
> 也可以不开 shell 直接启动：`cargo run --features gui -- gui run`。
> 注意每次运行无 feature 的 `cargo run` / `cargo test` 都会覆盖 `target/debug/hibos.exe`，
> 之后要重新 `cargo build --features gui`。

```sh
cargo build --features gui
cargo run --features gui -- --shell
# 进入 shell 后：
gui desktop      # 显示桌面摘要 + 真彩预览
gui shot         # 导出 gui-output.bmp (960x600)
gui run          # 打开真实窗口（阻塞到关闭；窗口内 Enter 的命令由 shell 事后执行）
# 说明：标题栏**右上角**是 Mac 风格红绿灯（绿/黄/红圆球，抗锯齿+高光+描边），
# 聚焦或悬停时圆球上才浮出 × / − / + 细线符号，与 macOS 一致。
gui open TERMINAL
gui focus 1
gui click 100 100
gui key hello\n
```

## 通道

| 通道 | 类别 | 说明 |
|---|---|---|
| `gui://windows` | Desktop | 窗口合成状态 |
| `input://kbd` | Desktop | 键盘输入 |
| `input://mouse` | Desktop | 鼠标输入 |

## 返回索引

[返回目录](../README.md)
