//! 内存文件系统 (VFS) —— 类 Linux 目录树。
//!
//! 提供绝对/相对路径解析、`.` / `..`、读写、创建、删除与目录遍历。
//! 全部驻留内存，用于原型阶段承载 `/etc`、`/proc`、`/dev` 等伪文件系统语义。

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// 节点类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    /// 设备节点 (伪文件)
    Device,
}

impl NodeKind {
    pub fn type_char(self) -> char {
        match self {
            NodeKind::Dir => 'd',
            NodeKind::File => '-',
            NodeKind::Device => 'c',
        }
    }
}

/// 文件系统节点。
#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub kind: NodeKind,
    pub mode: u16,
    pub content: String,
    pub children: BTreeMap<String, Node>,
}

impl Node {
    pub fn dir(name: &str) -> Self {
        Self {
            name: name.to_string(),
            kind: NodeKind::Dir,
            mode: 0o755,
            content: String::new(),
            children: BTreeMap::new(),
        }
    }

    pub fn file(name: &str, content: &str) -> Self {
        Self {
            name: name.to_string(),
            kind: NodeKind::File,
            mode: 0o644,
            content: content.to_string(),
            children: BTreeMap::new(),
        }
    }

    pub fn device(name: &str, content: &str) -> Self {
        Self {
            name: name.to_string(),
            kind: NodeKind::Device,
            mode: 0o666,
            content: content.to_string(),
            children: BTreeMap::new(),
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == NodeKind::Dir
    }

    pub fn size(&self) -> usize {
        self.content.len()
    }
}

/// 文件系统错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    NotADirectory,
    IsADirectory,
    AlreadyExists,
    NotEmpty,
    InvalidPath,
}

impl std::fmt::Display for VfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            VfsError::NotFound => "路径不存在",
            VfsError::NotADirectory => "不是目录",
            VfsError::IsADirectory => "是一个目录",
            VfsError::AlreadyExists => "已存在",
            VfsError::NotEmpty => "目录非空",
            VfsError::InvalidPath => "非法路径",
        };
        write!(f, "{}", s)
    }
}

/// 内存文件系统。
pub struct Vfs {
    root: Node,
    cwd: Vec<String>,
}

impl Vfs {
    /// 构造默认的类 Linux 目录树。
    pub fn new() -> Self {
        let mut root = Node::dir("/");

        // /bin
        let mut bin = Node::dir("bin");
        bin.children.insert(
            "hibos-sh".into(),
            Node::file("hibos-sh", "#!/hibos/sh\n# Hib OS 交互 shell 入口\n"),
        );
        bin.children.insert(
            "syscall".into(),
            Node::file("syscall", "# syscall 工具 (10 种 AI 系统调用)\n"),
        );
        root.children.insert("bin".into(), bin);

        // /etc
        let mut etc = Node::dir("etc");
        etc.children.insert(
            "hibos.toml".into(),
            Node::file(
                "hibos.toml",
                concat!(
                    "version = \"",
                    crate::version::hibos_version!(),
                    "\"\ntime_slice_ns = 10000000\ngpu_centric = true\n"
                ),
            ),
        );
        etc.children.insert(
            "ontology.conf".into(),
            Node::file(
                "ontology.conf",
                "concepts = 21\npolicies = [RealTime, SoftRealTime, Batch, EnergySaving, HighThroughput]\n",
            ),
        );
        root.children.insert("etc".into(), etc);

        // /dev
        let mut dev = Node::dir("dev");
        dev.children.insert("gpu0".into(), Node::device("gpu0", "<gpu device>"));
        dev.children.insert("iommu0".into(), Node::device("iommu0", "<iommu device>"));
        dev.children.insert("null".into(), Node::device("null", ""));
        root.children.insert("dev".into(), dev);

        // /proc
        let mut procfs = Node::dir("proc");
        procfs.children.insert(
            "meminfo".into(),
            Node::file(
                "meminfo",
                "VramTotal: 17179869184 B\nVramReserved: 0 B\n",
            ),
        );
        procfs.children.insert(
            "cpuinfo".into(),
            Node::file("cpuinfo", "arch: x86_64\nscheduler: MLFQ + semantic\n"),
        );
        procfs.children.insert(
            "scheduler".into(),
            Node::file(
                "scheduler",
                "policy_rotation: RealTime -> SoftRealTime -> HighThroughput -> Batch -> EnergySaving\n",
            ),
        );
        root.children.insert("proc".into(), procfs);

        // /models
        let mut models = Node::dir("models");
        models.children.insert(
            "README".into(),
            Node::file(
                "README",
                "模型权重由 Token 管理，映射到独立 VRAM 分区 (IOMMU 隔离)。\n",
            ),
        );
        root.children.insert("models".into(), models);

        // /var/log
        let mut var = Node::dir("var");
        let mut log = Node::dir("log");
        log.children.insert(
            "kernel.log".into(),
            Node::file("kernel.log", "[boot] Hib OS kernel started\n"),
        );
        var.children.insert("log".into(), log);
        root.children.insert("var".into(), var);

        // /tmp
        root.children.insert("tmp".into(), Node::dir("tmp"));

        // /home/user
        let mut home = Node::dir("home");
        let mut user = Node::dir("user");
        user.children.insert(
            ".hibosrc".into(),
            Node::file(".hibosrc", "# Hib OS 用户配置\nexport PROMPT=hibos\n"),
        );
        home.children.insert("user".into(), user);
        root.children.insert("home".into(), home);

        Self {
            root,
            cwd: Vec::new(),
        }
    }

    /// 当前工作目录 (绝对路径)。
    pub fn cwd_path(&self) -> String {
        if self.cwd.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", self.cwd.join("/"))
        }
    }

    /// 解析路径为组件序列。
    pub fn resolve(&self, path: &str) -> Result<Vec<String>, VfsError> {
        let mut comps: Vec<String> = if path.starts_with('/') {
            Vec::new()
        } else {
            self.cwd.clone()
        };
        for part in path.split('/') {
            match part {
                "" | "." => continue,
                ".." => {
                    comps.pop();
                }
                name => comps.push(name.to_string()),
            }
        }
        Ok(comps)
    }

    fn node_at(&self, comps: &[String]) -> Option<&Node> {
        let mut cur = &self.root;
        for c in comps {
            cur = cur.children.get(c)?;
        }
        Some(cur)
    }

    fn node_at_mut(&mut self, comps: &[String]) -> Option<&mut Node> {
        let mut cur = &mut self.root;
        for c in comps {
            cur = cur.children.get_mut(c)?;
        }
        Some(cur)
    }

    /// 切换目录。
    pub fn cd(&mut self, path: &str) -> Result<(), VfsError> {
        let comps = self.resolve(path)?;
        match self.node_at(&comps) {
            Some(n) if n.is_dir() => {
                self.cwd = comps;
                Ok(())
            }
            Some(_) => Err(VfsError::NotADirectory),
            None => Err(VfsError::NotFound),
        }
    }

    /// 列出目录条目: (名称, 类型, 权限, 大小)。
    pub fn ls(&self, path: Option<&str>) -> Result<Vec<(String, NodeKind, u16, usize)>, VfsError> {
        let comps = self.resolve(path.unwrap_or("."))?;
        let node = self.node_at(&comps).ok_or(VfsError::NotFound)?;
        if !node.is_dir() {
            return Err(VfsError::NotADirectory);
        }
        Ok(node
            .children
            .values()
            .map(|n| (n.name.clone(), n.kind, n.mode, n.size()))
            .collect())
    }

    /// 读取文件内容。
    pub fn cat(&self, path: &str) -> Result<String, VfsError> {
        let comps = self.resolve(path)?;
        let node = self.node_at(&comps).ok_or(VfsError::NotFound)?;
        if node.is_dir() {
            return Err(VfsError::IsADirectory);
        }
        Ok(node.content.clone())
    }

    /// 写入 (覆盖) 文件，自动创建。
    pub fn write(&mut self, path: &str, content: &str) -> Result<(), VfsError> {
        let comps = self.resolve(path)?;
        if comps.is_empty() {
            return Err(VfsError::InvalidPath);
        }
        let (parent, name) = comps.split_at(comps.len() - 1);
        let pnode = self.node_at_mut(parent).ok_or(VfsError::NotFound)?;
        if !pnode.is_dir() {
            return Err(VfsError::NotADirectory);
        }
        let name = name[0].clone();
        match pnode.children.get_mut(&name) {
            Some(n) if n.is_dir() => Err(VfsError::IsADirectory),
            Some(n) => {
                n.content = content.to_string();
                Ok(())
            }
            None => {
                pnode.children.insert(name.clone(), Node::file(&name, content));
                Ok(())
            }
        }
    }

    /// 追加内容。
    pub fn append(&mut self, path: &str, content: &str) -> Result<(), VfsError> {
        let mut old = match self.cat(path) {
            Ok(c) => c,
            Err(VfsError::NotFound) => String::new(),
            Err(e) => return Err(e),
        };
        old.push_str(content);
        self.write(path, &old)
    }

    /// 创建空文件。
    pub fn touch(&mut self, path: &str) -> Result<(), VfsError> {
        let comps = self.resolve(path)?;
        if let Some(n) = self.node_at(&comps) {
            if n.is_dir() {
                return Err(VfsError::IsADirectory);
            }
            return Ok(());
        }
        self.write(path, "")
    }

    /// 创建目录。
    pub fn mkdir(&mut self, path: &str) -> Result<(), VfsError> {
        let comps = self.resolve(path)?;
        if comps.is_empty() {
            return Err(VfsError::AlreadyExists);
        }
        let (parent, name) = comps.split_at(comps.len() - 1);
        let name = name[0].clone();
        let pnode = self.node_at_mut(parent).ok_or(VfsError::NotFound)?;
        if !pnode.is_dir() {
            return Err(VfsError::NotADirectory);
        }
        if pnode.children.contains_key(&name) {
            return Err(VfsError::AlreadyExists);
        }
        pnode.children.insert(name.clone(), Node::dir(&name));
        Ok(())
    }

    /// 删除文件或空目录。
    pub fn rm(&mut self, path: &str) -> Result<(), VfsError> {
        let comps = self.resolve(path)?;
        if comps.is_empty() {
            return Err(VfsError::InvalidPath);
        }
        let (parent, name) = comps.split_at(comps.len() - 1);
        let name = name[0].clone();
        let pnode = self.node_at_mut(parent).ok_or(VfsError::NotFound)?;
        match pnode.children.get(&name) {
            None => Err(VfsError::NotFound),
            Some(n) if n.is_dir() && !n.children.is_empty() => Err(VfsError::NotEmpty),
            Some(_) => {
                pnode.children.remove(&name);
                Ok(())
            }
        }
    }

    /// 递归创建目录（等价 `mkdir -p`）；已存在且是目录则直接成功。
    ///
    /// 挂载表与 VFS 联动的基石：挂载点必须在目录树里真实存在，
    /// 否则 `ls /sys` 之类的命令看不到它。
    pub fn ensure_dir_tree(&mut self, path: &str) -> Result<(), VfsError> {
        let comps = self.resolve(path)?;
        let mut cur = &mut self.root;
        for c in comps {
            if !cur.children.contains_key(&c) {
                cur.children.insert(c.clone(), Node::dir(&c));
            }
            let next = cur.children.get_mut(&c).unwrap();
            if !next.is_dir() {
                return Err(VfsError::NotADirectory);
            }
            cur = next;
        }
        Ok(())
    }

    /// 递归删除目录（含内容），用于卸载伪文件系统时清理挂载点。
    pub fn remove_dir_all(&mut self, path: &str) -> Result<(), VfsError> {
        let comps = self.resolve(path)?;
        if comps.is_empty() {
            return Err(VfsError::InvalidPath);
        }
        let (parent, name) = comps.split_at(comps.len() - 1);
        let name = name[0].clone();
        let pnode = self.node_at_mut(parent).ok_or(VfsError::NotFound)?;
        match pnode.children.get(&name) {
            None => Err(VfsError::NotFound),
            Some(n) if !n.is_dir() => Err(VfsError::NotADirectory),
            Some(_) => {
                pnode.children.remove(&name);
                Ok(())
            }
        }
    }

    /// 路径补全：返回同一目录下匹配前缀的名称。
    pub fn complete_path(&self, prefix: &str) -> Vec<String> {
        let (dir_part, file_prefix) = match prefix.rfind('/') {
            Some(i) => (&prefix[..=i], &prefix[i + 1..]),
            None => ("", prefix),
        };
        let dir = if dir_part.is_empty() { "." } else { dir_part };
        match self.ls(Some(dir)) {
            Ok(items) => items
                .into_iter()
                .filter(|(name, ..)| name.starts_with(file_prefix))
                .map(|(name, kind, ..)| {
                    if kind == NodeKind::Dir {
                        format!("{}{}/", dir_part, name)
                    } else {
                        format!("{}{}", dir_part, name)
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 递归打印目录树。
    pub fn tree(&self) -> String {
        let mut out = String::from(".\n");
        let mut prefix = String::new();
        self.tree_into(&self.root, &mut prefix, &mut out);
        out
    }

    fn tree_into(&self, node: &Node, prefix: &mut String, out: &mut String) {
        let count = node.children.len();
        for (i, (_, child)) in node.children.iter().enumerate() {
            let last = i + 1 == count;
            out.push_str(prefix);
            out.push_str(if last { "└── " } else { "├── " });
            out.push_str(&child.name);
            if child.is_dir() {
                out.push('/');
            }
            out.push('\n');

            if child.is_dir() {
                let old_len = prefix.len();
                prefix.push_str(if last { "    " } else { "│   " });
                self.tree_into(child, prefix, out);
                prefix.truncate(old_len);
            }
        }
    }

    /// 统计文件/目录数量。
    pub fn node_count(&self) -> (usize, usize) {
        fn walk(n: &Node, dirs: &mut usize, files: &mut usize) {
            for c in n.children.values() {
                if c.is_dir() {
                    *dirs += 1;
                    walk(c, dirs, files);
                } else {
                    *files += 1;
                }
            }
        }
        let mut dirs = 0;
        let mut files = 0;
        walk(&self.root, &mut dirs, &mut files);
        (dirs, files)
    }
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

// ======================================================================
// 持久化：极简 JSON 序列化 / 反序列化 (零依赖)
// ======================================================================

/// 极简 JSON 值。
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
}

/// 解析 JSON 文本。
pub fn parse_json(input: &str) -> Result<Json, String> {
    let mut p = JsonParser {
        chars: input.chars().collect(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.pos != p.chars.len() {
        return Err(format!("多余字符, 位置 {}", p.pos));
    }
    Ok(v)
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r' | '\n')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, want: char) -> Result<(), String> {
        match self.bump() {
            Some(c) if c == want => Ok(()),
            Some(c) => Err(format!("期望 '{}', 实际 '{}' (位置 {})", want, c, self.pos)),
            None => Err(format!("期望 '{}', 但已到末尾", want)),
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), String> {
        for want in word.chars() {
            if self.bump() != Some(want) {
                return Err(format!("非法字面量, 期望 {}", word));
            }
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('t') => {
                self.literal("true")?;
                Ok(Json::Bool(true))
            }
            Some('f') => {
                self.literal("false")?;
                Ok(Json::Bool(false))
            }
            Some('n') => {
                self.literal("null")?;
                Ok(Json::Null)
            }
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("非法字符 '{}' (位置 {})", c, self.pos)),
            None => Err("空输入".to_string()),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect('{')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Json::Obj(items));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(':')?;
            let val = self.value()?;
            items.push((key, val));
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => break,
                _ => return Err(format!("对象缺少 , 或 }} (位置 {})", self.pos)),
            }
        }
        Ok(Json::Obj(items))
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some(']') => break,
                _ => return Err(format!("数组缺少 , 或 ] (位置 {})", self.pos)),
            }
        }
        Ok(Json::Arr(items))
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err("字符串未闭合".to_string()),
                Some('"') => return Ok(out),
                Some('\\') => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('b') => out.push('\u{08}'),
                    Some('f') => out.push('\u{0C}'),
                    Some('u') => {
                        let hi = self.hex4()?;
                        let code = if (0xD800..=0xDBFF).contains(&hi) {
                            // 代理对
                            self.expect('\\')?;
                            self.expect('u')?;
                            let lo = self.hex4()?;
                            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                        } else {
                            hi
                        };
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    other => return Err(format!("非法转义 {:?}", other)),
                },
                Some(c) if (c as u32) < 0x20 => return Err("字符串含控制字符".to_string()),
                Some(c) => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.bump().ok_or("\\u 不完整")?;
            let d = c.to_digit(16).ok_or("\\u 需要 4 位十六进制")?;
            v = v * 16 + d;
        }
        Ok(v)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        {
            self.pos += 1;
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("非法数字 {}", s))
    }
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

impl Json {
    /// 紧凑序列化。
    pub fn dump(&self) -> String {
        match self {
            Json::Null => "null".to_string(),
            Json::Bool(b) => b.to_string(),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            Json::Str(s) => format!("\"{}\"", escape_json(s)),
            Json::Arr(items) => {
                let body: Vec<String> = items.iter().map(|i| i.dump()).collect();
                format!("[{}]", body.join(","))
            }
            Json::Obj(items) => {
                let body: Vec<String> = items
                    .iter()
                    .map(|(k, v)| format!("\"{}\":{}", escape_json(k), v.dump()))
                    .collect();
                format!("{{{}}}", body.join(","))
            }
        }
    }
}

fn node_to_json(n: &Node) -> Json {
    let kind = match n.kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Device => "device",
    };
    Json::Obj(vec![
        ("name".to_string(), Json::Str(n.name.clone())),
        ("kind".to_string(), Json::Str(kind.to_string())),
        ("mode".to_string(), Json::Num(n.mode as f64)),
        ("content".to_string(), Json::Str(n.content.clone())),
        (
            "children".to_string(),
            Json::Arr(n.children.values().map(node_to_json).collect()),
        ),
    ])
}

fn node_from_json(j: &Json, depth: usize) -> Result<Node, VfsError> {
    if depth > 64 {
        return Err(VfsError::InvalidPath);
    }
    let name = j
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or(VfsError::InvalidPath)?;
    if name.is_empty() || (name != "/" && name.contains('/')) {
        return Err(VfsError::InvalidPath);
    }
    let kind = match j.get("kind").and_then(|v| v.as_str()) {
        Some("dir") => NodeKind::Dir,
        Some("file") => NodeKind::File,
        Some("device") => NodeKind::Device,
        _ => return Err(VfsError::InvalidPath),
    };
    let mode = j
        .get("mode")
        .and_then(|v| v.as_num())
        .map(|n| n as u16)
        .unwrap_or(0o644);
    let content = j
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut node = Node {
        name: name.to_string(),
        kind,
        mode,
        content,
        children: BTreeMap::new(),
    };
    if let Some(items) = j.get("children").and_then(|v| v.as_arr()) {
        for child in items {
            let c = node_from_json(child, depth + 1)?;
            node.children.insert(c.name.clone(), c);
        }
    }
    Ok(node)
}

impl Vfs {
    /// 默认持久化文件路径。
    pub fn default_state_path() -> PathBuf {
        match std::env::var_os("HIBOS_STATE") {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => std::env::temp_dir().join("hibos_vfs.json"),
        }
    }

    /// 导出为 JSON 值。
    pub fn to_json(&self) -> Json {
        Json::Obj(vec![
            ("version".to_string(), Json::Num(1.0)),
            (
                "cwd".to_string(),
                Json::Arr(self.cwd.iter().map(|c| Json::Str(c.clone())).collect()),
            ),
            ("root".to_string(), node_to_json(&self.root)),
        ])
    }

    /// 导出为 JSON 文本。
    pub fn to_json_string(&self) -> String {
        self.to_json().dump()
    }

    /// 从 JSON 值恢复。
    pub fn from_json(j: &Json) -> Result<Vfs, VfsError> {
        let root = j.get("root").ok_or(VfsError::InvalidPath)?;
        let root = node_from_json(root, 0)?;
        if !root.is_dir() {
            return Err(VfsError::InvalidPath);
        }
        let cwd: Vec<String> = j
            .get("cwd")
            .and_then(|v| v.as_arr())
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut vfs = Vfs { root, cwd };
        // 校验 cwd 仍然存在且是目录
        let ok = match vfs.node_at(&vfs.cwd) {
            Some(n) => n.is_dir(),
            None => false,
        };
        if !ok {
            vfs.cwd.clear();
        }
        Ok(vfs)
    }

    /// 从 JSON 文本恢复。
    pub fn from_json_str(s: &str) -> Result<Vfs, VfsError> {
        let j = parse_json(s).map_err(|_| VfsError::InvalidPath)?;
        Vfs::from_json(&j)
    }

    /// 保存到文件 (自动创建父目录)。
    pub fn save_to_path(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, self.to_json_string())
    }

    /// 从文件加载；文件不存在返回 `None`。
    pub fn load_from_path(path: &Path) -> io::Result<Option<Vfs>> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path)?;
        Vfs::from_json_str(&text)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }

    /// 恢复默认目录树。
    pub fn reset(&mut self) {
        *self = Vfs::new();
    }
}

/// 全局 VFS 单例（供统一系统能力层使用）。
///
/// 与能力注册表同理：文件读操作远多于写操作，用 `RwLock` 让
/// `ls` / `cat` / `stat` 等只读调用可以并发执行，只有写入才独占。
static GLOBAL_VFS: std::sync::OnceLock<std::sync::RwLock<Vfs>> =
    std::sync::OnceLock::new();

/// 获取全局 VFS（读用 `.read()`，写用 `.write()`）。
pub fn global_vfs() -> &'static std::sync::RwLock<Vfs> {
    GLOBAL_VFS.get_or_init(|| std::sync::RwLock::new(Vfs::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局 VFS 改为 RwLock 后，多个只读调用应能并发进入而不死锁。
    #[test]
    fn global_vfs_allows_concurrent_reads() {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    let vfs = global_vfs().read().unwrap();
                    assert!(vfs.cat("/etc/hibos.toml").is_ok());
                    assert!(vfs.ls(None).is_ok());
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn initial_tree_is_linux_like() {
        let mut v = Vfs::new();
        assert_eq!(v.cwd_path(), "/");
        for p in [
            "/", "/bin", "/etc", "/dev", "/proc", "/models", "/var", "/tmp", "/home",
        ] {
            assert!(v.cd(p).is_ok(), "缺少目录 {}", p);
        }
        assert!(v.cat("/etc/hibos.toml").unwrap().contains("version"));
        assert!(v.cat("/proc/meminfo").unwrap().contains("VramTotal"));
        assert!(v.cat("/etc/ontology.conf").unwrap().contains("concepts = 21"));
    }

    #[test]
    fn relative_and_parent_paths() {
        let mut v = Vfs::new();
        v.cd("/etc").unwrap();
        assert_eq!(v.cwd_path(), "/etc");
        v.cd("..").unwrap();
        assert_eq!(v.cwd_path(), "/");
        v.cd("/home/user").unwrap();
        assert!(v.cat(".hibosrc").is_ok());
        v.cd("../../etc").unwrap();
        assert_eq!(v.cwd_path(), "/etc");
    }

    #[test]
    fn ls_returns_entries() {
        let v = Vfs::new();
        let root = v.ls(None).unwrap();
        let names: Vec<&str> = root.iter().map(|(n, ..)| n.as_str()).collect();
        assert!(names.contains(&"etc"));
        assert!(names.contains(&"proc"));
        assert!(v.ls(Some("/nonexistent")).is_err());
    }

    #[test]
    fn write_touch_mkdir_rm() {
        let mut v = Vfs::new();
        v.cd("/tmp").unwrap();
        v.write("a.txt", "hello").unwrap();
        assert_eq!(v.cat("a.txt").unwrap(), "hello");
        v.append("a.txt", " world").unwrap();
        assert_eq!(v.cat("a.txt").unwrap(), "hello world");

        v.touch("empty").unwrap();
        assert_eq!(v.cat("empty").unwrap(), "");

        v.mkdir("d1").unwrap();
        assert!(v.mkdir("d1").is_err(), "重复创建应失败");
        v.rm("d1").unwrap();
        v.mkdir("d1").unwrap();
        v.cd("d1").unwrap();
        v.write("f", "x").unwrap();
        v.cd("..").unwrap();
        assert_eq!(v.rm("d1").unwrap_err(), VfsError::NotEmpty);
        v.rm("d1/f").unwrap();
        v.rm("d1").unwrap();
        assert!(v.cat("d1").is_err());
    }

    #[test]
    fn ensure_dir_tree_and_remove_dir_all() {
        let mut v = Vfs::new();
        // /sys 默认不存在，递归创建后才能 ls
        assert!(v.ls(Some("/sys")).is_err());
        v.ensure_dir_tree("/sys/class/gpu").unwrap();
        assert!(v.ls(Some("/sys")).is_ok());
        assert!(v.ls(Some("/sys/class/gpu")).is_ok());
        // 已存在时幂等
        v.ensure_dir_tree("/sys/class/gpu").unwrap();
        // 路径上有文件则拒绝
        v.write("/sys/class/note", "x").unwrap();
        assert_eq!(
            v.ensure_dir_tree("/sys/class/note/sub").unwrap_err(),
            VfsError::NotADirectory
        );

        // 递归删除非空目录
        v.remove_dir_all("/sys").unwrap();
        assert!(v.ls(Some("/sys")).is_err());
        assert_eq!(v.remove_dir_all("/sys").unwrap_err(), VfsError::NotFound);
        assert_eq!(v.remove_dir_all("/").unwrap_err(), VfsError::InvalidPath);
    }

    #[test]
    fn cat_directory_fails() {
        let v = Vfs::new();
        assert_eq!(v.cat("/etc").unwrap_err(), VfsError::IsADirectory);
        assert_eq!(v.ls(Some("/etc/hibos.toml")).unwrap_err(), VfsError::NotADirectory);
    }

    #[test]
    fn write_missing_parent_fails() {
        let mut v = Vfs::new();
        assert_eq!(v.write("/no/such/file", "x").unwrap_err(), VfsError::NotFound);
    }

    #[test]
    fn path_completion() {
        let v = Vfs::new();
        let all = v.complete_path("");
        assert!(all.iter().any(|n| n == "etc/"));
        let filtered = v.complete_path("e");
        assert_eq!(filtered, vec!["etc/"]);
        let abs = v.complete_path("/et");
        assert_eq!(abs, vec!["/etc/"]);
        assert!(v.complete_path("zzz").is_empty());
    }

    #[test]
    fn load_saved_state_file_when_present() {
        let path = std::env::temp_dir().join("hibos_vfs.json");
        if path.exists() {
            match Vfs::load_from_path(&path) {
                Ok(Some(loaded)) => {
                    assert!(loaded.cat("/etc/hibos.toml").is_ok());
                }
                Ok(None) => {}
                Err(e) => panic!("加载已保存状态失败: {}", e),
            }
        }
    }

    #[test]
    fn tree_renders() {
        let v = Vfs::new();
        let t = v.tree();
        assert!(t.contains("etc"));
        assert!(t.contains("hibos.toml"));
        assert!(t.contains("└──"));
    }

    #[test]
    fn counts_nodes() {
        let (dirs, files) = Vfs::new().node_count();
        assert!(dirs >= 8, "目录数 {}", dirs);
        assert!(files >= 8, "文件数 {}", files);
    }
}
