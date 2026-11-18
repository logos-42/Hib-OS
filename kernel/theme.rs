//! ANSI 颜色宏模块 —— 统一在命令行中使用绿色、红色、黄色等颜色与加粗显示。
//!
//! 所有宏仅在真实终端（stdout 是 tty）时上色，否则原样输出，避免管道/重定向出现乱码转义符。
//!
//! 使用示例：
//! ```rust
//! use hibos::kernel::theme::ansi_green;
//! print!("{}", ansi_green!("OK"));
//! ```

/// 绿色（仅真实终端生效，否则原样）。
#[macro_export]
macro_rules! ansi_green {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[32m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 红色（仅真实终端生效，否则原样）。
#[macro_export]
macro_rules! ansi_red {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[31m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 黄色（仅真实终端生效，否则原样）。
#[macro_export]
macro_rules! ansi_yellow {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[33m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 蓝色（仅真实终端生效，否则原样）。
#[macro_export]
macro_rules! ansi_blue {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[34m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 青色（仅真实终端生效，否则原样）。
#[macro_export]
macro_rules! ansi_cyan {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[36m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 加粗（仅真实终端生效，否则原样）。
#[macro_export]
macro_rules! ansi_bold {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[1m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 加粗 + 绿色（重点显示）。
#[macro_export]
macro_rules! ansi_bold_green {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[1m\x1b[32m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 加粗 + 红色（重点显示）。
#[macro_export]
macro_rules! ansi_bold_red {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[1m\x1b[31m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 加粗 + 黄色（重点显示）。
#[macro_export]
macro_rules! ansi_bold_yellow {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[1m\x1b[33m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 加粗 + 蓝色（重点显示）。
#[macro_export]
macro_rules! ansi_bold_blue {
    ($s:expr) => {{
        if std::io::stdout().is_terminal() {
            format!("\x1b[1m\x1b[34m{}\x1b[0m", $s)
        } else {
            format!("{}", $s)
        }
    }};
}

/// 绿色 + 加粗（别名，与 ansi_bold_green 等价）。
#[macro_export]
macro_rules! ansi_green_bold {
    ($s:expr) => {{
        crate::theme::ansi_bold_green!($s)
    }};
}

/// 红色 + 加粗（别名，与 ansi_bold_red 等价）。
#[macro_export]
macro_rules! ansi_red_bold {
    ($s:expr) => {{
        crate::theme::ansi_bold_red!($s)
    }};
}

/// 黄色 + 加粗（别名，与 ansi_bold_yellow 等价）。
#[macro_export]
macro_rules! ansi_yellow_bold {
    ($s:expr) => {{
        crate::theme::ansi_bold_yellow!($s)
    }};
}
