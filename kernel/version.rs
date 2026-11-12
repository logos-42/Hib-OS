//! 版本来源 —— 全系统版本号的唯一真相 (single source of truth)。
//!
//! 版本号不再散落在各模块里硬编码，而是统一由 `Cargo.toml` 的
//! `package.version` 在编译期派生：
//!
//! - [`VERSION`] / [`VERSION_MAJOR`] / [`VERSION_MINOR`] / [`VERSION_PATCH`]
//!   —— 运行期可用的常量。
//! - [`hibos_version!`] —— 编译期展开为字符串字面量的宏，可直接嵌进
//!   `concat!`，得到 `&'static str`（零分配，no_std 友好）。
//!
//! # 注意：三个编译单元共享本模块
//!
//! `kernel/` 目录会被三个 crate 同时编译：
//!
//! | crate | 入口 | 用途 |
//! |---|---|---|
//! | `hibos`（lib） | `lib.rs` | 库目标，宿主 OS 上跑 |
//! | `hibos`（bin） | `kernel/mod.rs` | 可执行文件原型 |
//! | `hibos-kernel` | `kernel/kernel.rs` | 裸机 no_std 内核 |
//!
//! 前两个共用根目录 `Cargo.toml`，第三个用 `kernel/Cargo.toml`。
//! 两份清单的版本号必须保持一致，否则同一套代码在宿主与裸机上会报出
//! 不同的版本号；`tests/version_consistency.rs` 会守住这条约束。

/// 完整版本号，例如 `"0.2.0"`。
///
/// 取自当前 crate 的 `Cargo.toml` 中 `package.version`。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 主版本号（`0`）。
pub const VERSION_MAJOR: u32 = parse_u32(env!("CARGO_PKG_VERSION_MAJOR"));

/// 次版本号（`2`）。
pub const VERSION_MINOR: u32 = parse_u32(env!("CARGO_PKG_VERSION_MINOR"));

/// 修订号（`0`）。
pub const VERSION_PATCH: u32 = parse_u32(env!("CARGO_PKG_VERSION_PATCH"));

/// 展开为编译期的版本号字面量。
///
/// 因为展开结果是字面量，所以可以直接参与 `concat!`，得到 `&'static str`，
/// 不需要运行期分配，可在 no_std 裸机内核中使用：
///
/// ```ignore
/// let banner = concat!("Hib OS ", crate::version::hibos_version!(), " (AI-native)");
/// ```
///
/// 也可用于 `format!` / `println!` 等：
///
/// ```ignore
/// println!("Hib OS {}", crate::version::hibos_version!());
/// ```
macro_rules! hibos_version {
    () => {
        env!("CARGO_PKG_VERSION")
    };
}
pub(crate) use hibos_version;

/// 编译期把十进制文本解析成 `u32`。
///
/// Cargo 为我们提供了 `CARGO_PKG_VERSION_MAJOR` 等已拆分的字符串版本号，
/// 这里在 const 语境下把它们还原成数字，供 `modules::Version` 之类的
/// 结构化版本使用。
const fn parse_u32(s: &str) -> u32 {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut value = 0u32;
    while i < bytes.len() {
        let c = bytes[i];
        if c < b'0' || c > b'9' {
            break;
        }
        value = value * 10 + (c - b'0') as u32;
        i += 1;
    }
    value
}
