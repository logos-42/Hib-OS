# Hib OS

GPU-Centric 宏内核原型：把系统资源（文件、显存、设备、网络、桌面…）统一建模为"通道"的操作系统内核，并内置 **AI 意识本体闭环**（感知 → 思考 → 行动）。

## 构建

```sh
cargo build
```

## 运行

默认进入 **AI 闭环模式（loop）**：AI 自动感知系统、思考，并自动执行回复中的 `ACTION:` 指令（覆盖全部已注册通道）。

运行需要 DeepSeek API Key，通过环境变量提供：

```sh
# PowerShell
$env:DEEPSEEK_API_KEY = "sk-你的key"
# CMD
set DEEPSEEK_API_KEY=sk-你的key

cargo run
```

未设置 Key 时，会退回普通命令行 shell（`help` 查看命令，`exit` 退出）。

## 许可证

[MIT](LICENSE) © logos-42
