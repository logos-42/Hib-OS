//! 意识镜像加载器 —— `loader/<镜像>` 的后端。
//!
//! 加载 = 把一段可调用 `sense`/`act` 的意识镜像（DSL/WASM 字节码/脚本）
//! 映射到一个新进程的通道集；返回 proc-id。
//!
//! 内置镜像：
//! * echo       —— 回声镜像，等待外部 act 唤醒后回显
//! * sense-loop —— 持续感知 gpu/vram，适合感知驱动调度演示
//! * act-loop   —— 持续向 lease/channels 提交，适合租约竞争演示
//! * idle       —— 空闲，只 checkpoint，不占 CPU

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 意识镜像元数据。
#[derive(Clone, Debug)]
pub struct ImageMeta {
    pub name: String,
    pub description: String,
    pub channels: Vec<String>, // 镜像主要使用的通道
    pub load_count: u64,
}

/// 镜像注册表。
struct ImageRegistry {
    images: BTreeMap<String, ImageMeta>,
}

static IMAGES: OnceLock<Mutex<ImageRegistry>> = OnceLock::new();

fn images() -> &'static Mutex<ImageRegistry> {
    IMAGES.get_or_init(|| {
        let mut images = BTreeMap::new();
        images.insert(
            "echo".to_string(),
            ImageMeta {
                name: "echo".to_string(),
                description: "回声镜像：等待 proc/echo/<id> 被 signal 后回显".into(),
                channels: vec!["proc/echo/<id>".into()],
                load_count: 0,
            },
        );
        images.insert(
            "sense-loop".to_string(),
            ImageMeta {
                name: "sense-loop".into(),
                description: "持续感知 gpu/vram 的镜像".into(),
                channels: vec!["gpu/vram".into(), "proc/sense/<id>".into()],
                load_count: 0,
            },
        );
        images.insert(
            "act-loop".into(),
            ImageMeta {
                name: "act-loop".into(),
                description: "持续向 lease/channels 提交的镜像".into(),
                channels: vec!["lease/channels".into(), "proc/act/<id>".into()],
                load_count: 0,
            },
        );
        images.insert(
            "idle".into(),
            ImageMeta {
                name: "idle".into(),
                description: "空闲镜像：仅 checkpoint，不占 CPU".into(),
                channels: vec![],
                load_count: 0,
            },
        );
        Mutex::new(ImageRegistry { images })
    })
}

/// 查询镜像元数据。
pub fn meta(name: &str) -> Option<ImageMeta> {
    let g = images().lock().unwrap();
    g.images.get(name).cloned()
}

/// 列出所有镜像。
pub fn list() -> String {
    let g = images().lock().unwrap();
    if g.images.is_empty() {
        return "（无意识镜像）".to_string();
    }
    let mut out = String::new();
    for (_, m) in g.images.iter() {
        out.push_str(&format!(
            "image={:<14} desc={} channels={}\n",
            m.name,
            m.description,
            m.channels.join(",")
        ));
    }
    out.trim_end().to_string()
}

/// 记录一次镜像加载。
pub fn record_load(name: &str) {
    let mut g = images().lock().unwrap();
    if let Some(m) = g.images.get_mut(name) {
        m.load_count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_images() {
        let s = list();
        assert!(s.contains("echo"));
        assert!(s.contains("sense-loop"));
        assert!(s.contains("act-loop"));
        assert!(s.contains("idle"));
    }

    #[test]
    fn meta_lookup() {
        let m = meta("echo").unwrap();
        assert_eq!(m.name, "echo");
        record_load("echo");
        let m2 = meta("echo").unwrap();
        assert_eq!(m2.load_count, 1);
    }
}
