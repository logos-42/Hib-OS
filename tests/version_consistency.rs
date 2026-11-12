//! 版本一致性测试 —— 守住「版本号唯一真相」这条约束。
//!
//! `kernel/version.rs` 从 `Cargo.toml` 的 `package.version` 派生版本号，
//! 但 `kernel/` 目录被两个独立的 Cargo 包同时编译：
//!
//! - 根包 `hibos`（`Cargo.toml`）
//! - 裸机包 `hibos-kernel`（`kernel/Cargo.toml`）
//!
//! 两份清单各写各的版本号，迟早会漂移 —— 一旦漂移，同一套代码在宿主
//! 与裸机上会报出不同的版本号，用户无从判断装的是哪一版。
//! 本测试在 `cargo test` 时把两者钉在一起。

use std::fs;
use std::path::Path;

/// 读取某个 `Cargo.toml` 中 `[package]` 段声明的 `version`。
fn package_version(manifest: &Path) -> String {
    let text = fs::read_to_string(manifest)
        .unwrap_or_else(|e| panic!("读取 {} 失败: {}", manifest.display(), e));

    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = line.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                return value.to_string();
            }
        }
    }
    panic!("{} 的 [package] 段没有 version 字段", manifest.display());
}

#[test]
fn root_and_kernel_crates_share_the_same_version() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root_version = package_version(&root_dir.join("Cargo.toml"));
    let kernel_version = package_version(&root_dir.join("kernel").join("Cargo.toml"));

    assert_eq!(
        root_version, kernel_version,
        "根 crate (hibos) 与裸机内核 crate (hibos-kernel) 的版本号必须一致；\
         请同时更新 Cargo.toml 与 kernel/Cargo.toml"
    );
}

#[test]
fn manifest_version_matches_compiled_in_version() {
    // 编译进二进制的版本号，必须等于清单里声明的版本号 ——
    // 这条能抓住「改了 Cargo.toml 但忘了重新编译」之类的错位。
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root_version = package_version(&root_dir.join("Cargo.toml"));

    assert_eq!(
        hibos::kernel::version::VERSION,
        root_version,
        "编译期嵌入的版本号与 Cargo.toml 不一致"
    );
}

#[test]
fn version_components_reconstruct_the_full_version() {
    use hibos::kernel::version::{VERSION, VERSION_MAJOR, VERSION_MINOR, VERSION_PATCH};

    let expected = format!("{}.{}.{}", VERSION_MAJOR, VERSION_MINOR, VERSION_PATCH);
    assert_eq!(VERSION, expected, "版本号的数字分量与完整版本号对不上");
}
