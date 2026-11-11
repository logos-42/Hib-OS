//! AI 原生文件系统层 —— AI 原生 OS 的「记忆文件系统」 (AI-Native VFS)
//!
//! 本模块实现 AI 原生 OS 的语义文件系统，对应传统 OS 中的
//! 「VFS / inode / 目录 / 文件」概念，但重新映射到记忆域：
//!
//! * SemanticInode (语义 inode) —— 带语义向量的文件元数据，等价于传统 inode；
//! * SemanticDentry (语义目录项) —— 路径到语义 inode 的映射，等价于 dentry；
//! * SemanticPath (语义路径) —— 基于语义相似度的路径解析，等价于路径查找；
//! * MemoryFile (记忆文件) —— 内存中的文件内容，等价于页缓存文件；
//! * SemanticDirectory (语义目录) —— 按语义聚类的目录，等价于目录文件；
//! * SemanticLink (语义链接) —— 基于语义相似度的软链接，等价于符号链接；
//! * MemoryMount (记忆挂载点) —— 语义文件系统的挂载点，等价于挂载点；
//! * SemanticFS (语义文件系统) —— 整个语义文件系统，等价于超级块；
//! * AIFileWatcher (AI 文件监视器) —— 基于语义变化的文件监视，等价于 inotify；
//! * VFS_AI (AI 原生 VFS 引擎) —— 统一入口，通过 vfs_ai://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无磁盘依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

use crate::fs::{make_dir, read_file, write_file};

// =====================================================================================
// 语义 inode (SemanticInode)
// =====================================================================================

/// 文件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AINodeType {
    Regular,   // 普通文件
    Directory, // 目录
    Symlink,   // 符号链接
    Socket,    // 套接字
    Device,    // 设备文件
}

/// 语义 inode：带语义向量的文件元数据。
#[derive(Clone, Debug)]
pub struct SemanticInode {
    pub ino: u64,
    pub r#type: AINodeType,
    pub name: String,
    pub size: u64,
    pub blocks: u64,
    pub atime_ns: u128,
    pub mtime_ns: u128,
    pub ctime_ns: u128,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub nlink: u64,
    /// 语义向量（用于语义检索）。
    pub semantic_vector: Option<Vec<f32>>,
    /// 语义标签。
    pub semantic_tags: BTreeSet<String>,
    /// 内容引用（指向 MemoryFile）。
    pub content_ref: Option<u64>,
}

impl SemanticInode {
    pub fn new(ino: u64, r#type: AINodeType, name: &str) -> Self {
        Self {
            ino,
            r#type,
            name: name.to_string(),
            size: 0,
            blocks: 0,
            atime_ns: crate::ai_os::native::now_ns(),
            mtime_ns: crate::ai_os::native::now_ns(),
            ctime_ns: crate::ai_os::native::now_ns(),
            uid: 0,
            gid: 0,
            mode: if r#type == AINodeType::Directory { 0o755 } else { 0o644 },
            nlink: 1,
            semantic_vector: None,
            semantic_tags: BTreeSet::new(),
            content_ref: None,
        }
    }

    pub fn update_mtime(&mut self) {
        self.mtime_ns = crate::ai_os::native::now_ns();
        self.ctime_ns = self.mtime_ns;
    }

    pub fn update_atime(&mut self) {
        self.atime_ns = crate::ai_os::native::now_ns();
    }

    pub fn set_semantic_vector(&mut self, vec: Vec<f32>) {
        self.semantic_vector = Some(vec);
    }

    pub fn add_tag(&mut self, tag: &str) {
        self.semantic_tags.insert(tag.to_string());
    }

    pub fn semantic_similarity(&self, other: &SemanticInode) -> f32 {
        match (&self.semantic_vector, &other.semantic_vector) {
            (Some(a), Some(b)) => {
                if a.len() != b.len() {
                    return 0.0;
                }
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
                let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
                if na == 0.0 || nb == 0.0 {
                    0.0
                } else {
                    dot / (na * nb)
                }
            }
            _ => 0.0,
        }
    }
}

// =====================================================================================
// 记忆文件 (MemoryFile)
// =====================================================================================

/// 记忆文件：内存中的文件内容。
#[derive(Clone, Debug)]
pub struct MemoryFile {
    pub content: Vec<u8>,
    pub readonly: bool,
    pub append_only: bool,
}

impl MemoryFile {
    pub fn new() -> Self {
        Self {
            content: Vec::new(),
            readonly: false,
            append_only: false,
        }
    }

    pub fn len(&self) -> u64 {
        self.content.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub fn read(&self, offset: u64, len: u64) -> Vec<u8> {
        let start = offset as usize;
        let end = (offset + len) as usize;
        self.content.get(start..end.min(self.content.len())).unwrap_or(&[]).to_vec()
    }

    pub fn write(&mut self, offset: u64, data: &[u8]) -> Result<u64, String> {
        if self.readonly {
            return Err("文件只读".into());
        }
        let end = offset as usize + data.len();
        if end > self.content.len() {
            self.content.resize(end, 0);
        }
        self.content[offset as usize..end].copy_from_slice(data);
        Ok(data.len() as u64)
    }

    pub fn append(&mut self, data: &[u8]) -> Result<u64, String> {
        if self.append_only || self.readonly {
            return Err("文件只追加/只读".into());
        }
        let _offset = self.content.len();
        self.content.extend_from_slice(data);
        Ok(data.len() as u64)
    }

    pub fn truncate(&mut self, len: u64) -> Result<(), String> {
        if self.readonly {
            return Err("文件只读".into());
        }
        self.content.truncate(len as usize);
        Ok(())
    }
}

// =====================================================================================
// 语义目录项 (SemanticDentry)
// =====================================================================================

/// 语义目录项：路径到语义 inode 的映射。
#[derive(Clone, Debug)]
pub struct SemanticDentry {
    pub name: String,
    pub ino: u64,
    pub r#type: AINodeType,
}

/// 语义目录：按语义聚类的目录。
#[derive(Clone, Debug)]
pub struct SemanticDirectory {
    pub entries: BTreeMap<String, SemanticDentry>,
}

impl SemanticDirectory {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, name: &str, ino: u64, r#type: AINodeType) {
        self.entries.insert(
            name.to_string(),
            SemanticDentry {
                name: name.to_string(),
                ino,
                r#type,
            },
        );
    }

    pub fn remove(&mut self, name: &str) -> Option<SemanticDentry> {
        self.entries.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<&SemanticDentry> {
        self.entries.get(name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn list(&self) -> Vec<&SemanticDentry> {
        self.entries.values().collect()
    }
}

// =====================================================================================
// 语义路径解析 (SemanticPath)
// =====================================================================================

/// 语义路径解析器：基于语义相似度的路径查找。
pub struct SemanticPathResolver {
    root_inode: u64,
    path_cache: BTreeMap<String, u64>, // 路径 -> ino
}

impl SemanticPathResolver {
    pub fn new(root_inode: u64) -> Self {
        Self {
            root_inode,
            path_cache: BTreeMap::new(),
        }
    }

    pub fn resolve(&self, path: &str, inodes: &BTreeMap<u64, SemanticInode>) -> Option<u64> {
        if let Some(&ino) = self.path_cache.get(path) {
            return Some(ino);
        }
        // 简化：按 / 分割，逐层查找
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Some(self.root_inode);
        }
        let mut current = self.root_inode;
        for part in parts {
            let inode = inodes.get(&current)?;
            if inode.r#type != AINodeType::Directory {
                return None;
            }
            // 查找子目录项（简化：假设目录 ino 就是下一级）
            // 实际需要 dentry 表，这里简化为直接查找同名 inode
            let next = inodes.values().find(|ino| {
                ino.r#type == AINodeType::Directory && ino.name == part && ino.nlink > 0
            })?;
            current = next.ino;
        }
        Some(current)
    }

    pub fn cache_path(&mut self, path: &str, ino: u64) {
        self.path_cache.insert(path.to_string(), ino);
    }
}

// =====================================================================================
// 语义链接 (SemanticLink)
// =====================================================================================

/// 语义链接：基于语义相似度的软链接。
#[derive(Clone, Debug)]
pub struct SemanticLink {
    pub link_id: u64,
    pub source_path: String,
    pub target_path: String,
    pub similarity: f32,
}

/// 语义链接表。
pub struct SemanticLinkTable {
    links: BTreeMap<u64, SemanticLink>,
    next_id: u64,
}

impl SemanticLinkTable {
    pub fn new() -> Self {
        Self {
            links: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add(&mut self, source: &str, target: &str, similarity: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.links.insert(
            id,
            SemanticLink {
                link_id: id,
                source_path: source.to_string(),
                target_path: target.to_string(),
                similarity: similarity.clamp(0.0, 1.0),
            },
        );
        id
    }

    pub fn resolve(&self, source: &str) -> Option<&str> {
        self.links.values().find(|l| l.source_path == source).map(|l| l.target_path.as_str())
    }

    pub fn count(&self) -> usize {
        self.links.len()
    }
}

// =====================================================================================
// AI 文件监视器 (AIFileWatcher)
// =====================================================================================

/// 文件事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileEventType {
    Create,
    Modify,
    Delete,
    Access,
    SemanticChange, // 语义内容发生显著变化
}

/// 文件事件。
#[derive(Clone, Debug)]
pub struct FileEvent {
    pub id: u64,
    pub ino: u64,
    pub r#type: FileEventType,
    pub timestamp_ns: u128,
    pub details: String,
}

/// AI 文件监视器：基于语义变化的文件监视。
pub struct AIFileWatcher {
    events: VecDeque<FileEvent>,
    next_id: u64,
    pub max_events: usize,
    pub watched_inodes: BTreeSet<u64>,
}

impl AIFileWatcher {
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            next_id: 1,
            max_events: 100000,
            watched_inodes: BTreeSet::new(),
        }
    }

    pub fn watch(&mut self, ino: u64) {
        self.watched_inodes.insert(ino);
    }

    pub fn unwatch(&mut self, ino: u64) {
        self.watched_inodes.remove(&ino);
    }

    pub fn notify(&mut self, ino: u64, r#type: FileEventType, details: &str) -> u64 {
        if !self.watched_inodes.contains(&ino) {
            return 0;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.events.push_back(FileEvent {
            id,
            ino,
            r#type,
            timestamp_ns: crate::ai_os::native::now_ns(),
            details: details.to_string(),
        });
        if self.events.len() > self.max_events {
            self.events.pop_front();
        }
        id
    }

    pub fn poll(&mut self) -> Option<FileEvent> {
        self.events.pop_front()
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }
}

// =====================================================================================
// AI 原生 VFS 引擎 (VFS_AI) —— 统一入口
// =====================================================================================

/// AI 原生 VFS 引擎：整合语义 inode、记忆文件、语义目录、语义路径、语义链接、AI 文件监视器。
pub struct VFS_AI {
    pub inodes: BTreeMap<u64, SemanticInode>,
    pub files: BTreeMap<u64, MemoryFile>,
    pub directories: BTreeMap<u64, SemanticDirectory>,
    pub path_resolver: SemanticPathResolver,
    pub links: SemanticLinkTable,
    pub watcher: AIFileWatcher,
    pub next_ino: u64,
    pub next_file_id: u64,
    pub root_inode: u64,
    pub enabled: bool,
    pub total_operations: u64,
}

impl VFS_AI {
    pub fn new() -> Self {
        let root_inode = 1;
        let mut inodes = BTreeMap::new();
        let mut directories = BTreeMap::new();
        inodes.insert(
            root_inode,
            SemanticInode::new(root_inode, AINodeType::Directory, "/"),
        );
        directories.insert(root_inode, SemanticDirectory::new());
        let path_resolver = SemanticPathResolver::new(root_inode);
        Self {
            inodes,
            files: BTreeMap::new(),
            directories,
            path_resolver,
            links: SemanticLinkTable::new(),
            watcher: AIFileWatcher::new(),
            next_ino: 2,
            next_file_id: 1,
            root_inode,
            enabled: true,
            total_operations: 0,
        }
    }

    pub fn create_inode(&mut self, r#type: AINodeType, name: &str, parent_ino: u64) -> Result<u64, String> {
        if !self.enabled {
            return Err("VFS 已禁用".into());
        }
        let ino = self.next_ino;
        self.next_ino += 1;
        let inode = SemanticInode::new(ino, r#type, name);
        self.inodes.insert(ino, inode);
        if r#type == AINodeType::Directory {
            self.directories.insert(ino, SemanticDirectory::new());
        }
        // 将目录项加入父目录
        if let Some(dir) = self.directories.get_mut(&parent_ino) {
            dir.insert(name, ino, r#type);
        }
        // 更新父目录 nlink
        if let Some(parent) = self.inodes.get_mut(&parent_ino) {
            if r#type == AINodeType::Directory {
                parent.nlink += 1;
            }
        }
        self.total_operations += 1;
        self.watcher.notify(ino, FileEventType::Create, &format!("创建 {} {:?}", name, r#type));
        Ok(ino)
    }

    pub fn create_file(&mut self, name: &str, parent_ino: u64) -> Result<(u64, u64), String> {
        let ino = self.create_inode(AINodeType::Regular, name, parent_ino)?;
        let file_id = self.next_file_id;
        self.next_file_id += 1;
        self.files.insert(file_id, MemoryFile::new());
        // 更新 inode 的 content_ref
        if let Some(inode) = self.inodes.get_mut(&ino) {
            inode.content_ref = Some(file_id);
        }
        self.total_operations += 1;
        Ok((ino, file_id))
    }

    pub fn write_file(&mut self, file_id: u64, offset: u64, data: &[u8]) -> Result<u64, String> {
        let file = self.files.get_mut(&file_id).ok_or("文件不存在")?;
        let written = file.write(offset, data)?;
        // 更新关联 inode 的大小和 mtime
        for inode in self.inodes.values_mut() {
            if inode.content_ref == Some(file_id) {
                inode.size = file.len();
                inode.update_mtime();
                break;
            }
        }
        self.total_operations += 1;
        self.watcher.notify(file_id, FileEventType::Modify, "文件写入");
        Ok(written)
    }

    pub fn read_file(&mut self, file_id: u64, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        let file = self.files.get(&file_id).ok_or("文件不存在")?;
        let data = file.read(offset, len);
        // 更新 atime
        for inode in self.inodes.values_mut() {
            if inode.content_ref == Some(file_id) {
                inode.update_atime();
                break;
            }
        }
        self.total_operations += 1;
        Ok(data)
    }

    pub fn delete_file(&mut self, file_id: u64) -> Result<(), String> {
        self.files.remove(&file_id).ok_or("文件不存在")?;
        // 移除关联的 inode
        let mut to_remove = Vec::new();
        for (ino, inode) in &self.inodes {
            if inode.content_ref == Some(file_id) {
                to_remove.push(*ino);
            }
        }
        for ino in to_remove {
            self.inodes.remove(&ino);
            self.directories.remove(&ino);
        }
        self.total_operations += 1;
        Ok(())
    }

    pub fn lookup(&self, path: &str) -> Option<u64> {
        self.path_resolver.resolve(path, &self.inodes)
    }

    pub fn add_semantic_tag(&mut self, ino: u64, tag: &str) -> Result<(), String> {
        let inode = self.inodes.get_mut(&ino).ok_or("inode 不存在")?;
        inode.add_tag(tag);
        self.total_operations += 1;
        Ok(())
    }

    pub fn set_semantic_vector(&mut self, ino: u64, vector: Vec<f32>) -> Result<(), String> {
        let inode = self.inodes.get_mut(&ino).ok_or("inode 不存在")?;
        inode.set_semantic_vector(vector);
        self.total_operations += 1;
        Ok(())
    }

    pub fn semantic_search(&self, query_vector: &[f32], threshold: f32) -> Vec<(u64, f32)> {
        let mut results: Vec<(u64, f32)> = self
            .inodes
            .iter()
            .filter_map(|(ino, inode)| {
                inode.semantic_vector.as_ref().map(|v| {
                    let dot: f32 = v.iter().zip(query_vector.iter()).map(|(a, b)| a * b).sum();
                    let nv: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let nq: f32 = query_vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let sim = if nv == 0.0 || nq == 0.0 { 0.0 } else { dot / (nv * nq) };
                    (*ino, sim)
                })
            })
            .filter(|(_, sim)| *sim >= threshold)
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    pub fn inode_count(&self) -> usize {
        self.inodes.len()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn link_count(&self) -> usize {
        self.links.count()
    }

    pub fn watcher_count(&self) -> usize {
        self.watcher.watched_inodes.len()
    }

    pub fn tick(&mut self) {
        // 处理文件监视器事件
        let _ = self.watcher.poll();
    }
}

impl Default for VFS_AI {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static VFS_AI: OnceLock<Mutex<VFS_AI>> = OnceLock::new();

pub fn vfs_ai() -> &'static Mutex<VFS_AI> {
    VFS_AI.get_or_init(|| Mutex::new(VFS_AI::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = vfs_ai();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = vfs_ai().lock().unwrap();
    format!(
        "vfs_ai: inodes={} files={} directories={} links={} watchers={} operations={} root={}",
        eng.inode_count(),
        eng.file_count(),
        eng.directories.len(),
        eng.link_count(),
        eng.watcher_count(),
        eng.total_operations,
        eng.root_inode
    )
}

pub fn vfs_ai_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn vfs_ai_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("mkdir") => {
            let name = it.next().ok_or("用法: vfs_ai:// mkdir <名称> [父inode]")?;
            let parent = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
            let mut eng = vfs_ai().lock().unwrap();
            let ino = eng.create_inode(AINodeType::Directory, name, parent)?;
            Ok(format!("已创建目录 ino={} 名称={} 父={}", ino, name, parent))
        }
        Some("create") => {
            let name = it.next().ok_or("用法: vfs_ai:// create <名称> [父inode]")?;
            let parent = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
            let mut eng = vfs_ai().lock().unwrap();
            let (ino, file_id) = eng.create_file(name, parent)?;
            Ok(format!("已创建文件 ino={} file_id={} 父={}", ino, file_id, parent))
        }
        Some("write") => {
            let file_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 file_id")?;
            let offset = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let data: Vec<u8> = it.map(|s| s.as_bytes().iter().cloned().collect::<Vec<_>>().into_iter().next().unwrap_or(0)).collect();
            if data.is_empty() {
                return Err("缺少写入数据".into());
            }
            let mut eng = vfs_ai().lock().unwrap();
            let written = eng.write_file(file_id, offset, &data)?;
            // 注入：同步写入 L1 POSIX VFS，使 `sense fs://vfs` 可见。
            let _ = make_dir("/ai_os/vfs_ai");
            let path = format!("/ai_os/vfs_ai/{}", file_id);
            let _ = write_file(0, &path, &data);
            Ok(format!("已写入 {} 字节 到 file_id={} offset={}", written, file_id, offset))
        }
        Some("read") => {
            let file_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 file_id")?;
            let offset = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let len = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(4096);
            let mut eng = vfs_ai().lock().unwrap();
            let data = eng.read_file(file_id, offset, len)?;
            // 注入：也从 L1 POSIX VFS 读取，建立跨层可见性。
            let path = format!("/ai_os/vfs_ai/{}", file_id);
            let _fs_data = read_file(&path);
            Ok(format!("已读取 {} 字节 从 file_id={} offset={}", data.len(), file_id, offset))
        }
        Some("delete") => {
            let file_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 file_id")?;
            let mut eng = vfs_ai().lock().unwrap();
            eng.delete_file(file_id)?;
            Ok(format!("已删除文件 file_id={}", file_id))
        }
        Some("lookup") => {
            let path = it.next().ok_or("用法: vfs_ai:// lookup <路径>")?;
            let eng = vfs_ai().lock().unwrap();
            match eng.lookup(path) {
                Some(ino) => Ok(format!("路径 {} 解析到 ino={}", path, ino)),
                None => Err(format!("路径 {} 未找到", path)),
            }
        }
        Some("tag") => {
            let ino = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 ino")?;
            let tag = it.next().ok_or("缺少标签")?;
            let mut eng = vfs_ai().lock().unwrap();
            eng.add_semantic_tag(ino, tag)?;
            Ok(format!("已为 ino={} 添加标签 {}", ino, tag))
        }
        Some("semantic-vector") => {
            let ino = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 ino")?;
            let values: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
            if values.is_empty() {
                return Err("缺少向量值".into());
            }
            let mut eng = vfs_ai().lock().unwrap();
            eng.set_semantic_vector(ino, values)?;
            Ok(format!("已为 ino={} 设置语义向量", ino))
        }
        Some("semantic-search") => {
            let values: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
            if values.is_empty() {
                return Err("缺少查询向量".into());
            }
            let threshold = 0.5;
            let eng = vfs_ai().lock().unwrap();
            let results = eng.semantic_search(&values, threshold);
            let mut out = String::from("语义检索结果:\n");
            for (ino, sim) in results.iter().take(10) {
                out.push_str(&format!("  ino={} 相似度={:.3}\n", ino, sim));
            }
            Ok(out)
        }
        Some("watch") => {
            let ino = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 ino")?;
            let mut eng = vfs_ai().lock().unwrap();
            eng.watcher.watch(ino);
            Ok(format!("已监视 ino={}", ino))
        }
        Some("poll") => {
            let mut eng = vfs_ai().lock().unwrap();
            match eng.watcher.poll() {
                Some(event) => Ok(format!("文件事件 id={} ino={} 类型={:?} 详情={}", event.id, event.ino, event.r#type, event.details)),
                None => Ok("无文件事件".into()),
            }
        }
        Some("tick") => {
            let mut eng = vfs_ai().lock().unwrap();
            eng.tick();
            Ok("VFS_AI 已推进".into())
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 vfs_ai 动作: {}", other)),
    }
}

pub fn vfs_ai_act_dispatch(payload: &str) -> Result<String, String> {
    vfs_ai_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inode_creation() {
        let mut vfs = VFS_AI::new();
        let ino = vfs.create_inode(AINodeType::Directory, "home", 1).unwrap();
        assert_eq!(ino, 2);
        assert_eq!(vfs.inode_count(), 2);
        let inode = vfs.inodes.get(&ino).unwrap();
        assert_eq!(inode.name, "home");
    }

    #[test]
    fn file_create_write_read() {
        let mut vfs = VFS_AI::new();
        let (ino, file_id) = vfs.create_file("test.txt", 1).unwrap();
        assert_eq!(ino, 2);
        assert_eq!(file_id, 1);
        let written = vfs.write_file(file_id, 0, b"hello").unwrap();
        assert_eq!(written, 5);
        let data = vfs.read_file(file_id, 0, 10).unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn file_append() {
        let mut vfs = VFS_AI::new();
        let (_, file_id) = vfs.create_file("append.txt", 1).unwrap();
        vfs.write_file(file_id, 0, b"hello").unwrap();
        vfs.write_file(file_id, 5, b" world").unwrap();
        let data = vfs.read_file(file_id, 0, 20).unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn file_delete() {
        let mut vfs = VFS_AI::new();
        let (_, file_id) = vfs.create_file("temp.txt", 1).unwrap();
        vfs.delete_file(file_id).unwrap();
        assert!(vfs.read_file(file_id, 0, 10).is_err());
    }

    #[test]
    fn directory_insert_lookup() {
        let mut vfs = VFS_AI::new();
        vfs.create_inode(AINodeType::Directory, "docs", 1).unwrap();
        let dir = vfs.directories.get(&1).unwrap();
        assert!(dir.get("docs").is_some());
    }

    #[test]
    fn semantic_tag() {
        let mut vfs = VFS_AI::new();
        let ino = vfs.create_inode(AINodeType::Regular, "file1", 1).unwrap();
        vfs.add_semantic_tag(ino, "important").unwrap();
        let inode = vfs.inodes.get(&ino).unwrap();
        assert!(inode.semantic_tags.contains("important"));
    }

    #[test]
    fn semantic_vector() {
        let mut vfs = VFS_AI::new();
        let ino = vfs.create_inode(AINodeType::Regular, "file1", 1).unwrap();
        vfs.set_semantic_vector(ino, vec![1.0, 0.0, 0.0]).unwrap();
        let inode = vfs.inodes.get(&ino).unwrap();
        assert_eq!(inode.semantic_vector, Some(vec![1.0, 0.0, 0.0]));
    }

    #[test]
    fn semantic_search() {
        let mut vfs = VFS_AI::new();
        vfs.create_inode(AINodeType::Regular, "file1", 1).unwrap();
        vfs.set_semantic_vector(2, vec![1.0, 0.0, 0.0]).unwrap();
        vfs.create_inode(AINodeType::Regular, "file2", 1).unwrap();
        vfs.set_semantic_vector(3, vec![0.0, 1.0, 0.0]).unwrap();
        let results = vfs.semantic_search(&[1.0, 0.0, 0.0], 0.5);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 2);
    }

    #[test]
    fn semantic_link() {
        let mut links = SemanticLinkTable::new();
        let id = links.add("/src/main.rs", "/docs/main.md", 0.8);
        assert_eq!(id, 1);
        assert_eq!(links.resolve("/src/main.rs"), Some("/docs/main.md"));
        assert_eq!(links.count(), 1);
    }

    #[test]
    fn file_watcher() {
        let mut watcher = AIFileWatcher::new();
        watcher.watch(1);
        watcher.notify(1, FileEventType::Modify, "content changed");
        let event = watcher.poll().unwrap();
        assert_eq!(event.ino, 1);
        assert_eq!(event.r#type, FileEventType::Modify);
    }

    #[test]
    fn file_watcher_unwatch() {
        let mut watcher = AIFileWatcher::new();
        watcher.watch(1);
        watcher.unwatch(1);
        let id = watcher.notify(1, FileEventType::Modify, "test");
        assert_eq!(id, 0); // 未监视，不通知
    }

    #[test]
    fn vfs_ai_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("vfs_ai:"));
        assert!(s.contains("inodes="));
    }

    #[test]
    fn vfs_ai_act_mkdir() {
        init();
        let res = vfs_ai_act("mkdir docs 1").unwrap();
        assert!(res.contains("已创建目录"));
    }

    #[test]
    fn vfs_ai_act_create() {
        init();
        let res = vfs_ai_act("create test.txt 1").unwrap();
        assert!(res.contains("已创建文件"));
    }

    #[test]
    fn vfs_ai_act_write_read() {
        init();
        vfs_ai_act("create test.txt 1").unwrap();
        let res = vfs_ai_act("write 1 0 hello").unwrap();
        assert!(res.contains("已写入"));
        let res = vfs_ai_act("read 1 0 10").unwrap();
        assert!(res.contains("已读取"));
    }

    #[test]
    fn vfs_ai_act_lookup() {
        init();
        vfs_ai_act("mkdir docs 1").unwrap();
        let res = vfs_ai_act("lookup /docs").unwrap();
        assert!(res.contains("解析到"));
    }

    #[test]
    fn vfs_ai_act_tag() {
        init();
        vfs_ai_act("create file1 1").unwrap();
        let res = vfs_ai_act("tag 2 important").unwrap();
        assert!(res.contains("标签"));
    }

    #[test]
    fn vfs_ai_act_semantic_vector() {
        init();
        vfs_ai_act("create file1 1").unwrap();
        let res = vfs_ai_act("semantic-vector 2 1.0 0.0 0.0").unwrap();
        assert!(res.contains("语义向量"));
    }

    #[test]
    fn vfs_ai_act_semantic_search() {
        init();
        vfs_ai_act("create file1 1").unwrap();
        vfs_ai_act("semantic-vector 2 1.0 0.0 0.0").unwrap();
        let res = vfs_ai_act("semantic-search 1.0 0.0 0.0").unwrap();
        assert!(res.contains("语义检索结果"));
    }

    #[test]
    fn vfs_ai_act_watch_poll() {
        init();
        vfs_ai_act("create file1 1").unwrap();
        vfs_ai_act("watch 2").unwrap();
        vfs_ai_act("write 1 0 data").unwrap();
        let res = vfs_ai_act("poll").unwrap();
        assert!(res.contains("文件事件"));
    }

    #[test]
    fn vfs_ai_act_tick() {
        init();
        let res = vfs_ai_act("tick").unwrap();
        assert!(res.contains("VFS_AI 已推进"));
    }

    #[test]
    fn vfs_ai_act_status() {
        init();
        let res = vfs_ai_act("status").unwrap();
        assert!(res.contains("vfs_ai:"));
    }

    #[test]
    fn node_type_names() {
        assert_eq!(AINodeType::Regular as u32, 0);
        assert_eq!(AINodeType::Directory as u32, 1);
        assert_eq!(AINodeType::Symlink as u32, 2);
    }

    #[test]
    fn file_event_types() {
        assert_eq!(FileEventType::Create as u32, 0);
        assert_eq!(FileEventType::SemanticChange as u32, 4);
    }
}

// =====================================================================================
// 文件版本控制 (FileVersionControl)
// =====================================================================================

/// 文件版本。
#[derive(Clone, Debug)]
pub struct FileVersion {
    pub version_id: u64,
    pub ino: u64,
    pub content_hash: String,
    pub size: u64,
    pub created_at: u128,
    pub message: String,
}

/// 文件版本控制：Git 风格的文件版本管理。
pub struct FileVersionControl {
    pub versions: BTreeMap<u64, Vec<FileVersion>>, // ino -> versions
    pub next_version_id: u64,
    pub current_version: BTreeMap<u64, u64>, // ino -> current_version_id
}

impl FileVersionControl {
    pub fn new() -> Self {
        Self {
            versions: BTreeMap::new(),
            next_version_id: 1,
            current_version: BTreeMap::new(),
        }
    }

    pub fn commit(&mut self, ino: u64, content_hash: &str, size: u64, message: &str) -> u64 {
        let version_id = self.next_version_id;
        self.next_version_id += 1;
        self.versions.entry(ino).or_default().push(FileVersion {
            version_id,
            ino,
            content_hash: content_hash.to_string(),
            size,
            created_at: crate::ai_os::native::now_ns(),
            message: message.to_string(),
        });
        self.current_version.insert(ino, version_id);
        version_id
    }

    pub fn get_version(&self, ino: u64, version_id: u64) -> Option<&FileVersion> {
        self.versions.get(&ino).and_then(|vs| vs.iter().find(|v| v.version_id == version_id))
    }

    pub fn get_current(&self, ino: u64) -> Option<&FileVersion> {
        let vid = self.current_version.get(&ino)?;
        self.versions.get(&ino).and_then(|vs| vs.iter().find(|v| v.version_id == *vid))
    }

    pub fn version_count(&self, ino: u64) -> usize {
        self.versions.get(&ino).map(|vs| vs.len()).unwrap_or(0)
    }

    pub fn total_commits(&self) -> usize {
        self.versions.values().map(|vs| vs.len()).sum()
    }
}

// =====================================================================================
// 访问控制 (AccessControl)
// =====================================================================================

/// 权限类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionType {
    Read,
    Write,
    Execute,
    Admin,
}

/// 访问控制条目。
#[derive(Clone, Debug)]
pub struct AccessControlEntry {
    pub agent_id: u64,
    pub ino: u64,
    pub permissions: BTreeSet<PermissionType>,
    pub granted_at: u128,
}

/// 访问控制列表：文件级权限管理。
pub struct AccessControlList {
    pub entries: BTreeMap<(u64, u64), AccessControlEntry>, // (agent_id, ino) -> entry
}

impl AccessControlList {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn grant(&mut self, agent_id: u64, ino: u64, permissions: &[PermissionType]) {
        self.entries.insert((agent_id, ino), AccessControlEntry {
            agent_id,
            ino,
            permissions: permissions.iter().copied().collect(),
            granted_at: crate::ai_os::native::now_ns(),
        });
    }

    pub fn check(&self, agent_id: u64, ino: u64, permission: PermissionType) -> bool {
        self.entries.get(&(agent_id, ino))
            .map(|e| e.permissions.contains(&permission))
            .unwrap_or(false)
    }

    pub fn revoke(&mut self, agent_id: u64, ino: u64) -> bool {
        self.entries.remove(&(agent_id, ino)).is_some()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

// =====================================================================================
// 语义搜索优化 (SemanticSearchOptimizer)
// =====================================================================================

/// 索引类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexType {
    Flat,       // 线性扫描
    IVF,        // 倒排文件索引
    PQ,         // 产品量化
    HNSW,       // 分层小世界图
}

/// 语义搜索优化器：优化语义检索性能。
pub struct SemanticSearchOptimizer {
    pub index_type: IndexType,
    pub index: BTreeMap<u64, Vec<f32>>,
    pub query_count: u64,
    pub avg_latency_ns: u128,
}

impl SemanticSearchOptimizer {
    pub fn new() -> Self {
        Self {
            index_type: IndexType::Flat,
            index: BTreeMap::new(),
            query_count: 0,
            avg_latency_ns: 0,
        }
    }

    pub fn set_index_type(&mut self, index_type: IndexType) {
        self.index_type = index_type;
    }

    pub fn add_vector(&mut self, id: u64, vector: Vec<f32>) {
        self.index.insert(id, vector);
    }

    pub fn search(&mut self, query: &[f32], k: usize) -> Vec<(u64, f32)> {
        let start = crate::ai_os::native::now_ns();
        let mut results: Vec<(u64, f32)> = self.index.iter()
            .map(|(id, v)| {
                let dot: f32 = query.iter().zip(v.iter()).map(|(a, b)| a * b).sum();
                let nq: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
                let nv: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                let sim = if nq == 0.0 || nv == 0.0 { 0.0 } else { dot / (nq * nv) };
                (*id, sim)
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        let end = crate::ai_os::native::now_ns();
        self.query_count += 1;
        self.avg_latency_ns = (self.avg_latency_ns * (self.query_count - 1) as u128 + (end - start)) / self.query_count as u128;
        results
    }

    pub fn vector_count(&self) -> usize {
        self.index.len()
    }
}

// =====================================================================================
// 文件元数据管理 (FileMetadataManager)
// =====================================================================================

/// 文件元数据。
#[derive(Clone, Debug)]
pub struct FileMetadata {
    pub ino: u64,
    pub name: String,
    pub mime_type: String,
    pub tags: BTreeSet<String>,
    pub custom_fields: BTreeMap<String, String>,
    pub updated_at: u128,
}

/// 文件元数据管理器。
pub struct FileMetadataManager {
    pub metadata: BTreeMap<u64, FileMetadata>,
    pub next_id: u64,
}

impl FileMetadataManager {
    pub fn new() -> Self {
        Self {
            metadata: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, ino: u64, name: &str, mime_type: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.metadata.insert(ino, FileMetadata {
            ino,
            name: name.to_string(),
            mime_type: mime_type.to_string(),
            tags: BTreeSet::new(),
            custom_fields: BTreeMap::new(),
            updated_at: crate::ai_os::native::now_ns(),
        });
        id
    }

    pub fn add_tag(&mut self, ino: u64, tag: &str) -> bool {
        if let Some(meta) = self.metadata.get_mut(&ino) {
            meta.tags.insert(tag.to_string());
            meta.updated_at = crate::ai_os::native::now_ns();
            true
        } else {
            false
        }
    }

    pub fn set_custom_field(&mut self, ino: u64, key: &str, value: &str) -> bool {
        if let Some(meta) = self.metadata.get_mut(&ino) {
            meta.custom_fields.insert(key.to_string(), value.to_string());
            meta.updated_at = crate::ai_os::native::now_ns();
            true
        } else {
            false
        }
    }

    pub fn get(&self, ino: u64) -> Option<&FileMetadata> {
        self.metadata.get(&ino)
    }

    pub fn metadata_count(&self) -> usize {
        self.metadata.len()
    }
}

// =====================================================================================
// 文件版本控制测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn file_version_control() {
        let mut fvc = FileVersionControl::new();
        let vid1 = fvc.commit(1, "hash1", 100, "初始版本".into());
        let vid2 = fvc.commit(1, "hash2", 200, "更新内容".into());
        assert_eq!(vid1, 1);
        assert_eq!(vid2, 2);
        assert_eq!(fvc.version_count(1), 2);
        let current = fvc.get_current(1).unwrap();
        assert_eq!(current.version_id, 2);
        let v1 = fvc.get_version(1, 1).unwrap();
        assert_eq!(v1.content_hash, "hash1");
        assert_eq!(fvc.total_commits(), 2);
    }

    #[test]
    fn access_control() {
        let mut acl = AccessControlList::new();
        acl.grant(1, 10, &[PermissionType::Read, PermissionType::Write]);
        assert!(acl.check(1, 10, PermissionType::Read));
        assert!(acl.check(1, 10, PermissionType::Write));
        assert!(!acl.check(1, 10, PermissionType::Execute));
        assert!(!acl.check(2, 10, PermissionType::Read));
        acl.revoke(1, 10);
        assert!(!acl.check(1, 10, PermissionType::Read));
        assert_eq!(acl.entry_count(), 0);
    }

    #[test]
    fn semantic_search_optimizer() {
        let mut so = SemanticSearchOptimizer::new();
        so.set_index_type(IndexType::HNSW);
        so.add_vector(1, vec![1.0, 0.0, 0.0]);
        so.add_vector(2, vec![0.0, 1.0, 0.0]);
        so.add_vector(3, vec![1.0, 0.1, 0.0]);
        let results = so.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 1);
        assert_eq!(so.vector_count(), 3);
        assert!(so.query_count > 0);
    }

    #[test]
    fn file_metadata_manager() {
        let mut mm = FileMetadataManager::new();
        let id = mm.create(1, "document.pdf", "application/pdf");
        assert_eq!(id, 1);
        mm.add_tag(1, "important");
        mm.add_tag(1, "work");
        mm.set_custom_field(1, "author", "Alice");
        let meta = mm.get(1).unwrap();
        assert_eq!(meta.name, "document.pdf");
        assert!(meta.tags.contains("important"));
        assert_eq!(meta.custom_fields.get("author"), Some(&"Alice".into()));
        assert_eq!(mm.metadata_count(), 1);
    }

    #[test]
    fn permission_types() {
        assert_eq!(PermissionType::Read as u32, 0);
        assert_eq!(PermissionType::Admin as u32, 3);
    }

    #[test]
    fn index_types() {
        assert_eq!(IndexType::Flat as u32, 0);
        assert_eq!(IndexType::HNSW as u32, 3);
    }
}
