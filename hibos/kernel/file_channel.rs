//! 文件通道化 —— `file://<path>` 走统一 `sense`/`act` 路由。
//!
//! Linux 思维：inode/dentry、文件描述符、read()/write() 系统调用。
//! AI 通道思维：每个文件/目录 = 一个通道 `file://<path>`；
//! `cat` = `sense file://<path>`，`write` = `act file://<path> <内容>`；
//! 目录 = 通道树；文件元数据 = 「感知标签」（上次被哪个 AI 意识修改、epoch 等）。
//!
//! 本模块维护一个内存文件树，与 `vfs.rs` 解耦，专门为通道模型服务。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 文件节点。
#[derive(Clone, Debug)]
pub struct FileNode {
    pub path: String,
    pub content: String,
    pub last_writer: Option<u64>, // exec-id
    pub last_write_ns: u64,
    pub read_count: u64,
    pub write_count: u64,
}

/// 文件通道表。
struct FileChannelTable {
    files: BTreeMap<String, FileNode>,
}

static FILE_CHANNELS: OnceLock<Mutex<FileChannelTable>> = OnceLock::new();

fn file_channels() -> &'static Mutex<FileChannelTable> {
    FILE_CHANNELS.get_or_init(|| {
        Mutex::new(FileChannelTable {
            files: BTreeMap::new(),
        })
    })
}

/// 感知（读取）一个文件通道。
pub fn sense(path: &str) -> Result<String, String> {
    let mut g = file_channels().lock().unwrap();
    let node = g
        .files
        .get_mut(path)
        .ok_or_else(|| format!("文件不存在: {}", path))?;
    node.read_count += 1;
    Ok(format!(
        "file://{} size={} last_writer={:?} last_write={}ns reads={} writes={}\n{}",
        path,
        node.content.len(),
        node.last_writer,
        node.last_write_ns,
        node.read_count,
        node.write_count,
        node.content
    ))
}

/// 向一个文件通道写入内容。
pub fn act(path: &str, content: &str, writer: Option<u64>) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut g = file_channels().lock().unwrap();
    let node = g
        .files
        .entry(path.to_string())
        .or_insert_with(|| FileNode {
            path: path.to_string(),
            content: String::new(),
            last_writer: None,
            last_write_ns: 0,
            read_count: 0,
            write_count: 0,
        });
    node.content.push_str(content);
    node.content.push('\n');
    node.last_writer = writer;
    node.last_write_ns = now;
    node.write_count += 1;
    Ok(format!(
        "已写入 file://{} {} 字节 (writer={:?})",
        path,
        content.len(),
        writer
    ))
}

/// 列出所有文件通道。
pub fn list() -> String {
    let g = file_channels().lock().unwrap();
    if g.files.is_empty() {
        return "（无文件通道）".to_string();
    }
    let mut out = String::new();
    for (path, node) in g.files.iter() {
        out.push_str(&format!(
            "file://{} size={} writer={:?} reads={} writes={}\n",
            path,
            node.content.len(),
            node.last_writer,
            node.read_count,
            node.write_count
        ));
    }
    out.trim_end().to_string()
}

/// 删除一个文件通道。
pub fn rm(path: &str) -> Result<(), String> {
    let mut g = file_channels().lock().unwrap();
    if g.files.remove(path).is_some() {
        Ok(())
    } else {
        Err(format!("文件不存在: {}", path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sense_and_act_file_channel() {
        let _ = act("/etc/hibos.toml", "hostname=ai-node", Some(1));
        let s = sense("/etc/hibos.toml").unwrap();
        assert!(s.contains("hostname=ai-node"));
        assert!(s.contains("writer=Some(1)"));
        let l = list();
        assert!(l.contains("/etc/hibos.toml"));
    }

    #[test]
    fn sense_missing_file_errors() {
        assert!(sense("/nonexistent").is_err());
    }
}
