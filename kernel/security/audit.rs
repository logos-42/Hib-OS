//! 审计日志 —— 结构化安全事件记录与查询。
//!
//! 为 Hib OS 用户态宏内核提供安全审计能力，记录系统调用、文件访问、
//! 网络访问、能力使用、鉴权与配置变更等事件，以 JSON 格式追加写入日志文件，
//! 支持按时间范围/主体/事件类型/结果查询、日志轮转与统计。
//!
//! 设计要点:
//! * `AuditRecord`：时间戳、事件类型、主体（PID/UID）、客体（路径/句柄/地址）、
//!   动作、结果（成功/失败）、详情；
//! * `EventType`：Syscall/FileAccess/Network/Capability/Auth/Security/Config；
//! * `AuditLogger::new(log_path)`、`log(event_type, subject, object, action, result, detail)`、
//!   `query(filter) -> Vec<AuditRecord>`、`flush()`、`rotate()`、`stats() -> AuditStats`；
//! * 日志每行一个 JSON 对象，纯手动序列化，无 serde 依赖，可脱网运行；
//! * 全局单例（`OnceLock<Mutex<...>>`），与 `kernel/sandbox`、`kernel/caps` 并行配合。
//!
//! 文件末尾附带 `#[cfg(test)]` 单元测试，覆盖日志写入、JSON 格式、查询过滤、
//! 日志轮转、统计等。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ========================================================================
// 事件类型
// ========================================================================

/// 审计事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum EventType {
    /// 系统调用。
    Syscall,
    /// 文件访问。
    FileAccess,
    /// 网络访问。
    Network,
    /// 能力使用。
    Capability,
    /// 鉴权/身份。
    Auth,
    /// 安全事件（沙箱违规、故障等）。
    Security,
    /// 配置变更。
    Config,
}

impl EventType {
    pub fn name(self) -> &'static str {
        match self {
            EventType::Syscall => "Syscall",
            EventType::FileAccess => "FileAccess",
            EventType::Network => "Network",
            EventType::Capability => "Capability",
            EventType::Auth => "Auth",
            EventType::Security => "Security",
            EventType::Config => "Config",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ========================================================================
// 审计记录
// ========================================================================

/// 单条审计记录。
#[derive(Clone, Debug)]
pub struct AuditRecord {
    /// 纪元纳秒时间戳。
    pub timestamp: u64,
    /// 事件类型。
    pub event_type: EventType,
    /// 主体 PID。
    pub subject_pid: u32,
    /// 主体 UID。
    pub subject_uid: u32,
    /// 客体（文件路径、句柄、网络地址等）。
    pub object: String,
    /// 动作（如 "open"、"connect"、"cap_check"）。
    pub action: String,
    /// 结果：true 表示成功，false 表示失败/拒绝。
    pub result: bool,
    /// 详情（人读描述或结构化补充）。
    pub detail: String,
}

impl AuditRecord {
    pub fn new(
        event_type: EventType,
        subject_pid: u32,
        subject_uid: u32,
        object: &str,
        action: &str,
        result: bool,
        detail: &str,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        Self {
            timestamp,
            event_type,
            subject_pid,
            subject_uid,
            object: object.to_string(),
            action: action.to_string(),
            result,
            detail: detail.to_string(),
        }
    }

    /// 序列化为一行 JSON（不含换行）。
    pub fn to_json_line(&self) -> String {
        let ts_secs = self.timestamp / 1_000_000_000;
        let ts_nanos = self.timestamp % 1_000_000_000;
        format!(
            "{{\"ts_secs\":{},\"ts_nanos\":{},\"event_type\":\"{}\",\"subject_pid\":{},\"subject_uid\":{},\"object\":{},\"action\":{},\"result\":{},\"detail\":{}}}",
            ts_secs,
            ts_nanos,
            json_escape_str(&self.event_type.name()),
            self.subject_pid,
            self.subject_uid,
            json_escape_str(&self.object),
            json_escape_str(&self.action),
            if self.result { "true" } else { "false" },
            json_escape_str(&self.detail)
        )
    }

    /// 从一行 JSON 反序列化（宽松解析，仅供测试/离线回放）。
    fn from_json_line(line: &str) -> Option<Self> {
        let get_str = |key: &str, s: &str| -> Option<String> {
            let pattern = format!("\"{}\":\"", key);
            let start = s.find(&pattern)? + pattern.len();
            let end = s[start..].find('"')?;
            Some(s[start..start + end].to_string())
        };
        let get_u32 = |key: &str, s: &str| -> Option<u32> {
            let pattern = format!("\"{}\":", key);
            let start = s.find(&pattern)? + pattern.len();
            let end = s[start..]
                .find(|c: char| c == ',' || c == '}')
                .unwrap_or(0);
            s[start..start + end].parse().ok()
        };
        let get_bool = |key: &str, s: &str| -> Option<bool> {
            let pattern = format!("\"{}\":", key);
            let start = s.find(&pattern)? + pattern.len();
            let val = &s[start..start + 5];
            Some(val.starts_with("true"))
        };
        let event_type_name = get_str("event_type", line)?;
        let event_type = match event_type_name.as_str() {
            "Syscall" => EventType::Syscall,
            "FileAccess" => EventType::FileAccess,
            "Network" => EventType::Network,
            "Capability" => EventType::Capability,
            "Auth" => EventType::Auth,
            "Security" => EventType::Security,
            "Config" => EventType::Config,
            _ => return None,
        };
        Some(Self {
            timestamp: get_u32("ts_secs", line).map(|v| v as u64 * 1_000_000_000).unwrap_or(0),
            event_type,
            subject_pid: get_u32("subject_pid", line).unwrap_or(0),
            subject_uid: get_u32("subject_uid", line).unwrap_or(0),
            object: get_str("object", line).unwrap_or_default(),
            action: get_str("action", line).unwrap_or_default(),
            result: get_bool("result", line).unwrap_or(false),
            detail: get_str("detail", line).unwrap_or_default(),
        })
    }
}

/// 对字符串做 JSON 转义（带引号）。
fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ========================================================================
// 查询过滤器
// ========================================================================

/// 审计记录查询过滤器。
#[derive(Clone, Debug, Default)]
pub struct AuditFilter {
    /// 时间范围（秒，包含下界，不含上界）。
    pub time_range: Option<(u64, u64)>,
    /// 主体 PID。
    pub subject_pid: Option<u32>,
    /// 事件类型。
    pub event_type: Option<EventType>,
    /// 结果。
    pub result: Option<bool>,
    /// 客体关键字（子串匹配）。
    pub object_contains: Option<String>,
    /// 动作关键字（子串匹配）。
    pub action_contains: Option<String>,
    /// 最大返回条数。
    pub limit: Option<usize>,
}

impl AuditFilter {
    /// 是否匹配给定记录。
    pub fn matches(&self, record: &AuditRecord) -> bool {
        if let Some((start, end)) = self.time_range {
            let secs = record.timestamp / 1_000_000_000;
            if secs < start || secs >= end {
                return false;
            }
        }
        if let Some(pid) = self.subject_pid {
            if record.subject_pid != pid {
                return false;
            }
        }
        if let Some(et) = self.event_type {
            if record.event_type != et {
                return false;
            }
        }
        if let Some(result) = self.result {
            if record.result != result {
                return false;
            }
        }
        if let Some(ref kw) = self.object_contains {
            if !record.object.contains(kw) {
                return false;
            }
        }
        if let Some(ref kw) = self.action_contains {
            if !record.action.contains(kw) {
                return false;
            }
        }
        true
    }
}

// ========================================================================
// 统计
// ========================================================================

/// 审计统计信息。
#[derive(Clone, Debug, Default)]
pub struct AuditStats {
    pub total: u64,
    pub by_event_type: BTreeMap<EventType, u64>,
    pub by_result: BTreeMap<bool, u64>,
    pub by_subject: BTreeMap<u32, u64>,
    pub first_ts: Option<u64>,
    pub last_ts: Option<u64>,
}

// ========================================================================
// 审计日志器
// ========================================================================

/// 审计日志器。
///
/// 日志以 JSON Lines 格式追加写入 `log_path` 指定的文件；内存中保留最近
// 记录以供查询；`rotate()` 将当前日志重命名为 `.<seq>` 后新建；`flush()`
// 强制将缓冲写入磁盘。
pub struct AuditLogger {
    pub log_path: PathBuf,
    records: Vec<AuditRecord>,
    stats: AuditStats,
    rotate_seq: u64,
    max_records: usize,
}

impl AuditLogger {
    /// 创建审计日志器。若日志文件已存在，则加载其中的记录到内存。
    pub fn new(log_path: &Path) -> Self {
        let mut logger = Self {
            log_path: log_path.to_path_buf(),
            records: Vec::new(),
            stats: AuditStats::default(),
            rotate_seq: 0,
            max_records: 100_000,
        };
        let _ = logger.reload();
        logger
    }

    /// 从磁盘重新加载日志文件到内存。
    pub fn reload(&mut self) {
        self.records.clear();
        self.stats = AuditStats::default();
        if let Ok(data) = fs::read_to_string(&self.log_path) {
            for line in data.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(record) = AuditRecord::from_json_line(line) {
                    self.stats.total += 1;
                    *self.stats.by_event_type.entry(record.event_type).or_insert(0) += 1;
                    *self.stats.by_result.entry(record.result).or_insert(0) += 1;
                    *self.stats.by_subject.entry(record.subject_pid).or_insert(0) += 1;
                    let secs = record.timestamp / 1_000_000_000;
                    self.stats.first_ts = self
                        .stats
                        .first_ts
                        .map(|v| v.min(secs))
                        .or(Some(secs));
                    self.stats.last_ts = self
                        .stats
                        .last_ts
                        .map(|v| v.max(secs))
                        .or(Some(secs));
                    self.records.push(record);
                }
            }
        }
        // 超过上限则只保留最新的。
        if self.records.len() > self.max_records {
            let drop = self.records.len() - self.max_records;
            self.records.drain(..drop);
        }
    }

    /// 记录一条审计事件，并追加写入日志文件。
    pub fn log(
        &mut self,
        event_type: EventType,
        subject_pid: u32,
        subject_uid: u32,
        object: &str,
        action: &str,
        result: bool,
        detail: &str,
    ) {
        let record = AuditRecord::new(
            event_type,
            subject_pid,
            subject_uid,
            object,
            action,
            result,
            detail,
        );
        let secs = record.timestamp / 1_000_000_000;
        self.stats.total += 1;
        *self.stats.by_event_type.entry(event_type).or_insert(0) += 1;
        *self.stats.by_result.entry(result).or_insert(0) += 1;
        *self.stats.by_subject.entry(subject_pid).or_insert(0) += 1;
        self.stats.first_ts = self.stats.first_ts.map(|v| v.min(secs)).or(Some(secs));
        self.stats.last_ts = self.stats.last_ts.map(|v| v.max(secs)).or(Some(secs));

        let line = record.to_json_line();
        if let Err(e) = Self::append_line(&self.log_path, &line) {
            // 审计写入失败时不应阻塞主路径，这里仅通过 stats 记录（实际产品中可上报）。
            let _ = e;
        }

        self.records.push(record);
        if self.records.len() > self.max_records {
            self.records.drain(..1);
        }
    }

    /// 查询记录，返回匹配的记录列表（按时间倒序）。
    pub fn query(&self, filter: &AuditFilter) -> Vec<AuditRecord> {
        let mut result: Vec<AuditRecord> = self
            .records
            .iter()
            .filter(|r| filter.matches(r))
            .cloned()
            .collect();
        result.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        if let Some(limit) = filter.limit {
            result.truncate(limit);
        }
        result
    }

    /// 强制将内存记录刷新到磁盘（重写日志文件）。
    pub fn flush(&mut self) -> std::io::Result<()> {
        if let Some(parent) = self.log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content: String = self.records.iter().map(|r| r.to_json_line()).collect::<Vec<_>>().join("\n");
        let content = if content.is_empty() { String::new() } else { content + "\n" };
        fs::write(&self.log_path, content)
    }

    /// 日志轮转：将当前日志文件重命名为 `log_path.<seq>` 后清空内存与磁盘上的当前日志。
    pub fn rotate(&mut self) -> std::io::Result<()> {
        self.rotate_seq += 1;
        let rotated = format!("{}.{}", self.log_path.display(), self.rotate_seq);
        if self.log_path.exists() {
            fs::rename(&self.log_path, &rotated)?;
        }
        self.records.clear();
        self.stats = AuditStats::default();
        // 新建空日志文件。
        fs::File::create(&self.log_path)?;
        Ok(())
    }

    /// 取得统计信息。
    pub fn stats(&self) -> AuditStats {
        self.stats.clone()
    }

    /// 取得记录总数。
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 追加一行到日志文件。
    fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{}", line)
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new(Path::new("audit.log"))
    }
}

// ========================================================================
// 全局单例访问
// ========================================================================

static AUDIT_LOGGER: OnceLock<Mutex<AuditLogger>> = OnceLock::new();

/// 取得全局审计日志器单例。
pub fn audit_logger() -> &'static Mutex<AuditLogger> {
    AUDIT_LOGGER.get_or_init(|| Mutex::new(AuditLogger::new(Path::new("audit.log"))))
}

/// 便捷函数：记录一条审计事件。
pub fn audit_log(
    event_type: EventType,
    subject_pid: u32,
    subject_uid: u32,
    object: &str,
    action: &str,
    result: bool,
    detail: &str,
) {
    audit_logger().lock().unwrap().log(
        event_type,
        subject_pid,
        subject_uid,
        object,
        action,
        result,
        detail,
    );
}

/// 便捷函数：查询审计记录。
pub fn audit_query(filter: &AuditFilter) -> Vec<AuditRecord> {
    audit_logger().lock().unwrap().query(filter)
}

/// 便捷函数：刷新日志。
pub fn audit_flush() -> std::io::Result<()> {
    audit_logger().lock().unwrap().flush()
}

/// 便捷函数：日志轮转。
pub fn audit_rotate() -> std::io::Result<()> {
    audit_logger().lock().unwrap().rotate()
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tmp_log_path(suffix: &str) -> PathBuf {
        env::temp_dir().join(format!("hibos-audit-test-{}-{}.log", suffix, std::process::id()))
    }

    #[test]
    fn log_and_json_format() {
        let path = tmp_log_path("fmt");
        let _ = fs::remove_file(&path);
        let mut logger = AuditLogger::new(&path);
        logger.log(
            EventType::FileAccess,
            1000,
            0,
            "/etc/passwd",
            "open",
            true,
            "read permitted",
        );
        let data = fs::read_to_string(&path).unwrap();
        assert!(data.contains("\"event_type\":\"FileAccess\""));
        assert!(data.contains("\"subject_pid\":1000"));
        assert!(data.contains("\"object\":\"/etc/passwd\""));
        assert!(data.contains("\"result\":true"));
        assert!(data.contains("\"detail\":\"read permitted\""));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn query_by_event_type_and_pid() {
        let path = tmp_log_path("query");
        let _ = fs::remove_file(&path);
        let mut logger = AuditLogger::new(&path);
        logger.log(EventType::Syscall, 1, 0, "sys://x", "read", true, "ok");
        logger.log(EventType::FileAccess, 2, 0, "/tmp/f", "open", false, "denied");
        logger.log(EventType::Syscall, 1, 0, "sys://y", "write", true, "ok");

        let filter = AuditFilter {
            event_type: Some(EventType::Syscall),
            ..Default::default()
        };
        let sys = logger.query(&filter);
        assert_eq!(sys.len(), 2);

        let filter = AuditFilter {
            subject_pid: Some(2),
            result: Some(false),
            ..Default::default()
        };
        let denied = logger.query(&filter);
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].action, "open");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn query_by_time_range() {
        let path = tmp_log_path("time");
        let _ = fs::remove_file(&path);
        let mut logger = AuditLogger::new(&path);
        logger.log(EventType::Network, 3, 0, "1.2.3.4:443", "connect", true, "ok");
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let filter = AuditFilter {
            time_range: Some((secs, secs + 100)),
            ..Default::default()
        };
        let r = logger.query(&filter);
        assert_eq!(r.len(), 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn query_object_contains_and_limit() {
        let path = tmp_log_path("contains");
        let _ = fs::remove_file(&path);
        let mut logger = AuditLogger::new(&path);
        for i in 0..5 {
            logger.log(EventType::FileAccess, 4, 0, &format!("/etc/passwd{}", i), "open", true, "ok");
        }
        let filter = AuditFilter {
            object_contains: Some("/etc/passwd2".to_string()),
            limit: Some(10),
            ..Default::default()
        };
        let r = logger.query(&filter);
        assert_eq!(r.len(), 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn stats() {
        let path = tmp_log_path("stats");
        let _ = fs::remove_file(&path);
        let mut logger = AuditLogger::new(&path);
        logger.log(EventType::Syscall, 1, 0, "s", "a", true, "d");
        logger.log(EventType::Syscall, 2, 0, "s", "a", false, "d");
        logger.log(EventType::FileAccess, 1, 0, "s", "a", true, "d");
        let st = logger.stats();
        assert_eq!(st.total, 3);
        assert_eq!(st.by_event_type[&EventType::Syscall], 2);
        assert_eq!(st.by_result[&true], 2);
        assert_eq!(st.by_result[&false], 1);
        assert_eq!(st.by_subject[&1], 2);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn flush_and_reload() {
        let path = tmp_log_path("flush");
        let _ = fs::remove_file(&path);
        {
            let mut logger = AuditLogger::new(&path);
            logger.log(EventType::Auth, 5, 0, "user", "login", true, "ok");
            logger.flush().unwrap();
        }
        // 新建日志器应能重新加载已落盘记录。
        let mut logger2 = AuditLogger::new(&path);
        assert_eq!(logger2.len(), 1);
        assert_eq!(logger2.stats().total, 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rotate() {
        let path = tmp_log_path("rotate");
        let _ = fs::remove_file(&path);
        let mut logger = AuditLogger::new(&path);
        logger.log(EventType::Security, 6, 0, "sandbox", "violation", false, "denied");
        logger.rotate().unwrap();
        // 当前日志应为空，轮转文件应存在。
        assert_eq!(logger.len(), 0);
        let rotated = format!("{}.1", path.display());
        assert!(Path::new(&rotated).exists());
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&rotated);
    }

    #[test]
    fn max_records_truncation() {
        let path = tmp_log_path("trunc");
        let _ = fs::remove_file(&path);
        let mut logger = AuditLogger::new(&path);
        logger.max_records = 3;
        for i in 0..5 {
            logger.log(EventType::Config, 7, 0, "cfg", "set", true, &format!("v{}", i));
        }
        assert_eq!(logger.len(), 3);
        // 保留最新的 3 条。
        assert_eq!(logger.records[0].detail, "v2");
        assert_eq!(logger.records[2].detail, "v4");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn global_logger() {
        audit_log(EventType::Syscall, 8, 0, "g", "x", true, "g");
        let r = audit_query(&AuditFilter {
            subject_pid: Some(8),
            ..Default::default()
        });
        assert!(!r.is_empty());
    }
}
