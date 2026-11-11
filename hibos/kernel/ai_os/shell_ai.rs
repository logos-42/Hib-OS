//! AI 原生 Shell 层 —— AI 原生 OS 的「智能命令行」 (AI-Native Shell)
//!
//! 本模块实现 AI 原生 OS 的 shell，对应传统 OS 中的
//! 「命令行解释器 / 命令补全 / 历史记录 / 别名」概念，但重新映射到 AI 域：
//!
//! * NaturalLanguageParser (自然语言解析器) —— 将自然语言转换为命令，等价于命令解析器；
//! * CommandIntent (命令意图) —— 命令的语义意图，等价于系统调用编号；
//! * SmartCompletion (智能补全) —— 基于上下文的命令补全，等价于 Tab 补全；
//! * CommandHistory (命令历史) —— 命令执行历史记录，等价于 shell 历史文件；
//! * Session (会话) —— shell 会话状态，等价于终端会话；
//! * AliasManager (别名管理器) —— 命令别名映射，等价于 shell 别名；
//! * Environment (环境变量) —— 环境变量管理，等价于 environ；
//! * Pipeline (管道) —— 命令管道，等价于 Unix 管道；
//! * ShellAI (AI 原生 Shell 引擎) —— 统一入口，通过 shell_ai://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无终端依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 命令意图 (CommandIntent)
// =====================================================================================

/// 命令意图：命令的语义意图。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandIntent {
    ListFiles,       // 列出文件
    ReadFile,        // 读取文件
    WriteFile,       // 写入文件
    CreateDir,       // 创建目录
    RemoveFile,      // 删除文件
    CopyFile,        // 复制文件
    MoveFile,        // 移动文件
    SearchContent,   // 搜索内容
    SystemInfo,      // 系统信息
    ProcessList,     // 进程列表
    NetworkInfo,     // 网络信息
    Help,            // 帮助
    Exit,            // 退出
    Unknown,         // 未知
}

/// 解析结果。
#[derive(Clone, Debug)]
pub struct ParseResult {
    pub intent: CommandIntent,
    pub args: Vec<String>,
    pub confidence: f32,
    pub original: String,
}

/// 自然语言解析器：将自然语言转换为命令意图。
pub struct NaturalLanguageParser {
    keyword_map: BTreeMap<&'static str, CommandIntent>,
}

impl NaturalLanguageParser {
    pub fn new() -> Self {
        let mut map = BTreeMap::new();
        map.insert("list", CommandIntent::ListFiles);
        map.insert("ls", CommandIntent::ListFiles);
        map.insert("目录", CommandIntent::ListFiles);
        map.insert("文件", CommandIntent::ListFiles);
        map.insert("read", CommandIntent::ReadFile);
        map.insert("cat", CommandIntent::ReadFile);
        map.insert("读取", CommandIntent::ReadFile);
        map.insert("write", CommandIntent::WriteFile);
        map.insert("echo", CommandIntent::WriteFile);
        map.insert("写入", CommandIntent::WriteFile);
        map.insert("mkdir", CommandIntent::CreateDir);
        map.insert("创建目录", CommandIntent::CreateDir);
        map.insert("rm", CommandIntent::RemoveFile);
        map.insert("删除", CommandIntent::RemoveFile);
        map.insert("cp", CommandIntent::CopyFile);
        map.insert("复制", CommandIntent::CopyFile);
        map.insert("mv", CommandIntent::MoveFile);
        map.insert("移动", CommandIntent::MoveFile);
        map.insert("grep", CommandIntent::SearchContent);
        map.insert("搜索", CommandIntent::SearchContent);
        map.insert("info", CommandIntent::SystemInfo);
        map.insert("系统", CommandIntent::SystemInfo);
        map.insert("ps", CommandIntent::ProcessList);
        map.insert("进程", CommandIntent::ProcessList);
        map.insert("net", CommandIntent::NetworkInfo);
        map.insert("网络", CommandIntent::NetworkInfo);
        map.insert("help", CommandIntent::Help);
        map.insert("帮助", CommandIntent::Help);
        map.insert("exit", CommandIntent::Exit);
        map.insert("quit", CommandIntent::Exit);
        map.insert("退出", CommandIntent::Exit);
        Self { keyword_map: map }
    }

    pub fn parse(&self, input: &str) -> ParseResult {
        let input_lower = input.to_lowercase();
        let mut best_intent = CommandIntent::Unknown;
        let mut best_confidence = 0.0;
        let mut args: Vec<String> = Vec::new();

        for (keyword, intent) in &self.keyword_map {
            if input_lower.contains(keyword) {
                let confidence = 0.8;
                if confidence > best_confidence {
                    best_confidence = confidence;
                    best_intent = *intent;
                }
            }
        }

        // 提取参数（非关键词部分）
        let mut input_copy = input_lower.clone();
        for keyword in self.keyword_map.keys() {
            input_copy = input_copy.replace(keyword, " ");
        }
        for arg in input_copy.split_whitespace() {
            if !arg.is_empty() && arg.len() > 1 {
                args.push(arg.to_string());
            }
        }

        ParseResult {
            intent: best_intent,
            args,
            confidence: best_confidence,
            original: input.to_string(),
        }
    }

    pub fn intent_count(&self) -> usize {
        self.keyword_map.len()
    }
}

// =====================================================================================
// 智能补全 (SmartCompletion)
// =====================================================================================

/// 补全建议。
#[derive(Clone, Debug)]
pub struct CompletionSuggestion {
    pub text: String,
    pub description: String,
}

/// 智能补全：基于上下文的命令补全。
pub struct SmartCompletion {
    commands: BTreeSet<String>,
    history_commands: Vec<String>,
}

impl SmartCompletion {
    pub fn new() -> Self {
        let mut commands = BTreeSet::new();
        commands.insert("ls".to_string());
        commands.insert("cd".to_string());
        commands.insert("cat".to_string());
        commands.insert("mkdir".to_string());
        commands.insert("rm".to_string());
        commands.insert("cp".to_string());
        commands.insert("mv".to_string());
        commands.insert("grep".to_string());
        commands.insert("help".to_string());
        commands.insert("exit".to_string());
        commands.insert("status".to_string());
        commands.insert("ai".to_string());
        Self {
            commands,
            history_commands: Vec::new(),
        }
    }

    pub fn complete(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        let mut suggestions: Vec<CompletionSuggestion> = self
            .commands
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| CompletionSuggestion {
                text: c.clone(),
                description: format!("命令: {}", c),
            })
            .collect();
        suggestions.sort_by(|a, b| a.text.len().cmp(&b.text.len()));
        suggestions
    }

    pub fn add_history_command(&mut self, cmd: &str) {
        self.history_commands.push(cmd.to_string());
        if self.history_commands.len() > 1000 {
            self.history_commands.remove(0);
        }
    }

    pub fn command_count(&self) -> usize {
        self.commands.len()
    }
}

// =====================================================================================
// 命令历史 (CommandHistory)
// =====================================================================================

/// 历史记录条目。
#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub id: u64,
    pub command: String,
    pub timestamp_ns: u128,
    pub success: bool,
    pub output_preview: String,
}

/// 命令历史：命令执行历史记录。
pub struct CommandHistory {
    entries: VecDeque<HistoryEntry>,
    next_id: u64,
    pub max_entries: usize,
}

impl CommandHistory {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: 1,
            max_entries: 10000,
        }
    }

    pub fn add(&mut self, command: &str, success: bool, output: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push_back(HistoryEntry {
            id,
            command: command.to_string(),
            timestamp_ns: crate::ai_os::native::now_ns(),
            success,
            output_preview: output.chars().take(100).collect(),
        });
        if self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }
        id
    }

    pub fn recent(&self, n: usize) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().take(n).collect()
    }

    pub fn search(&self, pattern: &str) -> Vec<&HistoryEntry> {
        self.entries.iter().filter(|e| e.command.contains(pattern)).collect()
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

// =====================================================================================
// 会话 (Session)
// =====================================================================================

/// shell 会话状态。
#[derive(Clone, Debug)]
pub struct Session {
    pub id: u64,
    pub current_dir: String,
    pub prompt: String,
    pub env: BTreeMap<String, String>,
    pub created_at: u128,
    pub last_activity_ns: u128,
    pub command_count: u64,
}

impl Session {
    pub fn new(id: u64) -> Self {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/root".to_string());
        env.insert("PATH".to_string(), "/bin:/usr/bin".to_string());
        env.insert("SHELL".to_string(), "/bin/hibos".to_string());
        Self {
            id,
            current_dir: "/".to_string(),
            prompt: "hibos> ".to_string(),
            env,
            created_at: crate::ai_os::native::now_ns(),
            last_activity_ns: crate::ai_os::native::now_ns(),
            command_count: 0,
        }
    }

    pub fn set_dir(&mut self, dir: &str) {
        self.current_dir = dir.to_string();
        self.last_activity_ns = crate::ai_os::native::now_ns();
    }

    pub fn get_dir(&self) -> &str {
        &self.current_dir
    }

    pub fn set_env(&mut self, key: &str, value: &str) {
        self.env.insert(key.to_string(), value.to_string());
    }

    pub fn get_env(&self, key: &str) -> Option<&String> {
        self.env.get(key)
    }

    pub fn increment_command(&mut self) {
        self.command_count += 1;
        self.last_activity_ns = crate::ai_os::native::now_ns();
    }
}

// =====================================================================================
// 别名管理器 (AliasManager)
// =====================================================================================

/// 别名管理器：命令别名映射。
pub struct AliasManager {
    aliases: BTreeMap<String, String>,
}

impl AliasManager {
    pub fn new() -> Self {
        Self {
            aliases: BTreeMap::new(),
        }
    }

    pub fn set_alias(&mut self, alias: &str, command: &str) {
        self.aliases.insert(alias.to_string(), command.to_string());
    }

    pub fn resolve(&self, alias: &str) -> Option<&String> {
        self.aliases.get(alias)
    }

    pub fn remove_alias(&mut self, alias: &str) -> bool {
        self.aliases.remove(alias).is_some()
    }

    pub fn count(&self) -> usize {
        self.aliases.len()
    }
}

// =====================================================================================
// 管道 (Pipeline)
// =====================================================================================

/// 管道步骤。
#[derive(Clone, Debug)]
pub struct PipelineStep {
    pub command: String,
    pub args: Vec<String>,
}

/// 命令管道。
pub struct Pipeline {
    pub steps: Vec<PipelineStep>,
    pub input: Option<String>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            input: None,
        }
    }

    pub fn add_step(&mut self, command: &str, args: &[&str]) {
        self.steps.push(PipelineStep {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        });
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}

// =====================================================================================
// AI 原生 Shell 引擎 (ShellAI) —— 统一入口
// =====================================================================================

/// AI 原生 Shell 引擎：整合自然语言解析、智能补全、命令历史、会话、别名、环境、管道。
pub struct ShellAI {
    pub parser: NaturalLanguageParser,
    pub completion: SmartCompletion,
    pub history: CommandHistory,
    pub session: Session,
    pub aliases: AliasManager,
    pub pipelines: BTreeMap<u64, Pipeline>,
    pub next_pipeline_id: u64,
    pub enabled: bool,
    pub total_commands: u64,
    pub total_nl_parses: u64,
}

impl ShellAI {
    pub fn new() -> Self {
        Self {
            parser: NaturalLanguageParser::new(),
            completion: SmartCompletion::new(),
            history: CommandHistory::new(),
            session: Session::new(1),
            aliases: AliasManager::new(),
            pipelines: BTreeMap::new(),
            next_pipeline_id: 1,
            enabled: true,
            total_commands: 0,
            total_nl_parses: 0,
        }
    }

    pub fn parse_command(&mut self, input: &str) -> ParseResult {
        let result = self.parser.parse(input);
        self.total_nl_parses += 1;
        result
    }

    pub fn execute_command(&mut self, command: &str) -> String {
        let resolved = self.aliases.resolve(command).unwrap_or(&command.to_string()).clone();
        self.session.increment_command();
        self.history.add(command, true, "executed");
        self.completion.add_history_command(command);
        self.total_commands += 1;
        format!("已执行: {}", resolved)
    }

    pub fn execute_nl(&mut self, input: &str) -> String {
        let parse = self.parse_command(input);
        let intent_str = match parse.intent {
            CommandIntent::ListFiles => "列出文件",
            CommandIntent::ReadFile => "读取文件",
            CommandIntent::WriteFile => "写入文件",
            CommandIntent::CreateDir => "创建目录",
            CommandIntent::RemoveFile => "删除文件",
            CommandIntent::CopyFile => "复制文件",
            CommandIntent::MoveFile => "移动文件",
            CommandIntent::SearchContent => "搜索内容",
            CommandIntent::SystemInfo => "系统信息",
            CommandIntent::ProcessList => "进程列表",
            CommandIntent::NetworkInfo => "网络信息",
            CommandIntent::Help => "帮助",
            CommandIntent::Exit => "退出",
            CommandIntent::Unknown => "未知命令",
        };
        self.session.increment_command();
        self.history.add(input, true, intent_str);
        self.total_commands += 1;
        format!("意图: {} (置信度={:.2}, 参数={:?})", intent_str, parse.confidence, parse.args)
    }

    pub fn complete(&self, prefix: &str) -> Vec<CompletionSuggestion> {
        self.completion.complete(prefix)
    }

    pub fn set_alias(&mut self, alias: &str, command: &str) {
        self.aliases.set_alias(alias, command);
    }

    pub fn resolve_alias(&self, alias: &str) -> Option<String> {
        self.aliases.resolve(alias).cloned()
    }

    pub fn set_dir(&mut self, dir: &str) {
        self.session.set_dir(dir);
    }

    pub fn get_dir(&self) -> &str {
        &self.session.current_dir
    }

    pub fn set_env(&mut self, key: &str, value: &str) {
        self.session.set_env(key, value);
    }

    pub fn get_env(&self, key: &str) -> Option<String> {
        self.session.get_env(key).cloned()
    }

    pub fn add_pipeline(&mut self) -> u64 {
        let id = self.next_pipeline_id;
        self.next_pipeline_id += 1;
        self.pipelines.insert(id, Pipeline::new());
        id
    }

    pub fn pipeline_add_step(&mut self, pipeline_id: u64, command: &str, args: &[&str]) -> Result<(), String> {
        let pipeline = self.pipelines.get_mut(&pipeline_id).ok_or("管道不存在")?;
        pipeline.add_step(command, args);
        Ok(())
    }

    pub fn command_count(&self) -> u64 {
        self.total_commands
    }

    pub fn history_count(&self) -> usize {
        self.history.count()
    }

    pub fn alias_count(&self) -> usize {
        self.aliases.count()
    }

    pub fn session_dir(&self) -> &str {
        &self.session.current_dir
    }
}

impl Default for ShellAI {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static SHELL_AI: OnceLock<Mutex<ShellAI>> = OnceLock::new();

pub fn shell_ai() -> &'static Mutex<ShellAI> {
    SHELL_AI.get_or_init(|| Mutex::new(ShellAI::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = shell_ai();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = shell_ai().lock().unwrap();
    format!(
        "shell_ai: commands={} nl_parses={} history={} aliases={} pipelines={} dir={} env={}",
        eng.command_count(),
        eng.total_nl_parses,
        eng.history_count(),
        eng.alias_count(),
        eng.next_pipeline_id - 1,
        eng.session_dir(),
        eng.session.env.len()
    )
}

pub fn shell_ai_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn shell_ai_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("parse") => {
            let input: String = it.collect::<Vec<&str>>().join(" ");
            if input.is_empty() {
                return Err("缺少输入".into());
            }
            let mut eng = shell_ai().lock().unwrap();
            let result = eng.parse_command(&input);
            Ok(format!(
                "解析结果: 意图={:?} 置信度={:.2} 参数={:?} 原文='{}'",
                result.intent, result.confidence, result.args, result.original
            ))
        }
        Some("execute") => {
            let command: String = it.collect::<Vec<&str>>().join(" ");
            if command.is_empty() {
                return Err("缺少命令".into());
            }
            let mut eng = shell_ai().lock().unwrap();
            Ok(eng.execute_command(&command))
        }
        Some("execute-nl") => {
            let input: String = it.collect::<Vec<&str>>().join(" ");
            if input.is_empty() {
                return Err("缺少自然语言输入".into());
            }
            let mut eng = shell_ai().lock().unwrap();
            Ok(eng.execute_nl(&input))
        }
        Some("complete") => {
            let prefix = it.next().unwrap_or("");
            let eng = shell_ai().lock().unwrap();
            let suggestions = eng.complete(prefix);
            let mut out = String::from("补全建议:\n");
            for s in suggestions {
                out.push_str(&format!("  {} - {}\n", s.text, s.description));
            }
            Ok(out)
        }
        Some("alias") => {
            let alias = it.next().ok_or("缺少别名")?;
            let command = it.next().ok_or("缺少命令")?;
            let mut eng = shell_ai().lock().unwrap();
            eng.set_alias(alias, command);
            Ok(format!("已设置别名 {} -> {}", alias, command))
        }
        Some("resolve-alias") => {
            let alias = it.next().ok_or("缺少别名")?;
            let eng = shell_ai().lock().unwrap();
            match eng.resolve_alias(alias) {
                Some(cmd) => Ok(format!("别名 {} -> {}", alias, cmd)),
                None => Err(format!("别名 {} 未定义", alias)),
            }
        }
        Some("cd") => {
            let dir = it.next().unwrap_or("/");
            let mut eng = shell_ai().lock().unwrap();
            eng.set_dir(dir);
            Ok(format!("已切换目录: {}", dir))
        }
        Some("pwd") => {
            let eng = shell_ai().lock().unwrap();
            Ok(format!("当前目录: {}", eng.session_dir()))
        }
        Some("env") => {
            let key = it.next().unwrap_or("");
            let eng = shell_ai().lock().unwrap();
            match eng.get_env(key) {
                Some(v) => Ok(format!("{}={}", key, v)),
                None => if key.is_empty() {
                    let mut out = String::from("环境变量:\n");
                    for (k, v) in &eng.session.env {
                        out.push_str(&format!("  {}={}\n", k, v));
                    }
                    Ok(out)
                } else {
                    Err(format!("环境变量 {} 未定义", key))
                },
            }
        }
        Some("set-env") => {
            let key = it.next().ok_or("缺少键")?;
            let value = it.next().ok_or("缺少值")?;
            let mut eng = shell_ai().lock().unwrap();
            eng.set_env(key, value);
            Ok(format!("已设置环境变量 {}={}", key, value))
        }
        Some("pipeline") => {
            let mut eng = shell_ai().lock().unwrap();
            let id = eng.add_pipeline();
            Ok(format!("已创建管道 id={}", id))
        }
        Some("pipeline-add") => {
            let pid = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少管道id")?;
            let command = it.next().ok_or("缺少命令")?;
            let args: Vec<&str> = it.collect();
            let mut eng = shell_ai().lock().unwrap();
            eng.pipeline_add_step(pid, command, &args)?;
            Ok(format!("已向管道 {} 添加步骤 {}", pid, command))
        }
        Some("history") => {
            let n = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(10);
            let eng = shell_ai().lock().unwrap();
            let recent = eng.history.recent(n);
            let mut out = String::from("命令历史:\n");
            for e in recent {
                out.push_str(&format!("  id={} 命令={} 成功={}\n", e.id, e.command, e.success));
            }
            Ok(out)
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 shell_ai 动作: {}", other)),
    }
}

pub fn shell_ai_act_dispatch(payload: &str) -> Result<String, String> {
    shell_ai_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_list() {
        let parser = NaturalLanguageParser::new();
        let result = parser.parse("ls -la");
        assert_eq!(result.intent, CommandIntent::ListFiles);
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn parser_read() {
        let parser = NaturalLanguageParser::new();
        let result = parser.parse("cat file.txt");
        assert_eq!(result.intent, CommandIntent::ReadFile);
    }

    #[test]
    fn parser_unknown() {
        let parser = NaturalLanguageParser::new();
        let result = parser.parse("xyzzy command");
        assert_eq!(result.intent, CommandIntent::Unknown);
    }

    #[test]
    fn parser_chinese() {
        let parser = NaturalLanguageParser::new();
        let result = parser.parse("列出目录");
        assert_eq!(result.intent, CommandIntent::ListFiles);
    }

    #[test]
    fn completion() {
        let comp = SmartCompletion::new();
        let suggestions = comp.complete("ls");
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.text == "ls"));
    }

    #[test]
    fn completion_no_match() {
        let comp = SmartCompletion::new();
        let suggestions = comp.complete("xyz");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn history_add() {
        let mut hist = CommandHistory::new();
        let id = hist.add("ls -la", true, "file1\nfile2");
        assert_eq!(id, 1);
        assert_eq!(hist.count(), 1);
    }

    #[test]
    fn history_recent() {
        let mut hist = CommandHistory::new();
        hist.add("cmd1", true, "o1");
        hist.add("cmd2", true, "o2");
        let recent = hist.recent(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].command, "cmd2");
    }

    #[test]
    fn history_search() {
        let mut hist = CommandHistory::new();
        hist.add("grep pattern file", true, "match");
        hist.add("ls", true, "files");
        let results = hist.search("grep");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn session() {
        let mut session = Session::new(1);
        assert_eq!(session.get_dir(), "/");
        session.set_dir("/home/user");
        assert_eq!(session.get_dir(), "/home/user");
        session.set_env("FOO", "bar");
        assert_eq!(session.get_env("FOO"), Some(&"bar".to_string()));
        assert_eq!(session.command_count, 0);
        session.increment_command();
        assert_eq!(session.command_count, 1);
    }

    #[test]
    fn alias_manager() {
        let mut am = AliasManager::new();
        am.set_alias("ll", "ls -la");
        assert_eq!(am.resolve("ll"), Some(&"ls -la".to_string()));
        assert!(am.resolve("nonexistent").is_none());
        am.remove_alias("ll");
        assert!(am.resolve("ll").is_none());
    }

    #[test]
    fn pipeline() {
        let mut pipeline = Pipeline::new();
        pipeline.add_step("grep", &["pattern"]);
        pipeline.add_step("wc", &["-l"]);
        assert_eq!(pipeline.step_count(), 2);
    }

    #[test]
    fn shell_engine_parse() {
        let mut eng = ShellAI::new();
        let result = eng.parse_command("list files");
        assert_eq!(result.intent, CommandIntent::ListFiles);
        assert!(eng.total_nl_parses > 0);
    }

    #[test]
    fn shell_engine_execute() {
        let mut eng = ShellAI::new();
        let output = eng.execute_command("ls");
        assert!(output.contains("已执行"));
        assert_eq!(eng.command_count(), 1);
    }

    #[test]
    fn shell_engine_execute_nl() {
        let mut eng = ShellAI::new();
        let output = eng.execute_nl("读取文件 test.txt");
        assert!(output.contains("意图:"));
    }

    #[test]
    fn shell_engine_alias() {
        let mut eng = ShellAI::new();
        eng.set_alias("ll", "ls -la");
        assert_eq!(eng.resolve_alias("ll"), Some("ls -la".to_string()));
    }

    #[test]
    fn shell_engine_cd_pwd() {
        let mut eng = ShellAI::new();
        eng.set_dir("/home");
        assert_eq!(eng.get_dir(), "/home");
    }

    #[test]
    fn shell_engine_env() {
        let mut eng = ShellAI::new();
        eng.set_env("TEST", "value");
        assert_eq!(eng.get_env("TEST"), Some("value".to_string()));
    }

    #[test]
    fn shell_engine_pipeline() {
        let mut eng = ShellAI::new();
        let pid = eng.add_pipeline();
        assert_eq!(pid, 1);
        eng.pipeline_add_step(pid, "grep", &["pattern"]).unwrap();
    }

    #[test]
    fn shell_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("shell_ai:"));
        assert!(s.contains("commands="));
    }

    #[test]
    fn shell_ai_act_parse() {
        init();
        let res = shell_ai_act("parse 列出所有文件").unwrap();
        assert!(res.contains("解析结果"));
    }

    #[test]
    fn shell_ai_act_execute() {
        init();
        let res = shell_ai_act("execute ls").unwrap();
        assert!(res.contains("已执行"));
    }

    #[test]
    fn shell_ai_act_execute_nl() {
        init();
        let res = shell_ai_act("execute-nl 创建目录 test").unwrap();
        assert!(res.contains("意图:"));
    }

    #[test]
    fn shell_ai_act_complete() {
        init();
        let res = shell_ai_act("complete ls").unwrap();
        assert!(res.contains("补全建议"));
    }

    #[test]
    fn shell_ai_act_alias() {
        init();
        let res = shell_ai_act("alias ll ls -la").unwrap();
        assert!(res.contains("已设置别名"));
        let res = shell_ai_act("resolve-alias ll").unwrap();
        assert!(res.contains("ls -la"));
    }

    #[test]
    fn shell_ai_act_cd_pwd() {
        init();
        let res = shell_ai_act("cd /home").unwrap();
        assert!(res.contains("已切换目录"));
        let res = shell_ai_act("pwd").unwrap();
        assert!(res.contains("/home"));
    }

    #[test]
    fn shell_ai_act_env() {
        init();
        let res = shell_ai_act("set-env FOO bar").unwrap();
        assert!(res.contains("已设置环境变量"));
        let res = shell_ai_act("env FOO").unwrap();
        assert!(res.contains("bar"));
    }

    #[test]
    fn shell_ai_act_history() {
        init();
        shell_ai_act("execute ls").unwrap();
        let res = shell_ai_act("history 5").unwrap();
        assert!(res.contains("命令历史"));
    }

    #[test]
    fn shell_ai_act_status() {
        init();
        let res = shell_ai_act("status").unwrap();
        assert!(res.contains("shell_ai:"));
    }

    #[test]
    fn command_intents() {
        assert_eq!(CommandIntent::ListFiles as u32, 0);
        assert_eq!(CommandIntent::Exit as u32, 11);
        assert_eq!(CommandIntent::Unknown as u32, 12);
    }
}

// =====================================================================================
// 命令历史搜索 (CommandHistorySearch)
// =====================================================================================

/// 搜索结果。
#[derive(Clone, Debug)]
pub struct HistorySearchResult {
    pub entry_id: u64,
    pub command: String,
    pub relevance_score: f32,
}

/// 命令历史搜索器：搜索历史命令。
pub struct CommandHistorySearch {
    pub history: Vec<String>,
}

impl CommandHistorySearch {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    pub fn add(&mut self, command: &str) {
        self.history.push(command.to_string());
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<HistorySearchResult> {
        let mut results: Vec<HistorySearchResult> = self.history.iter().enumerate()
            .filter_map(|(i, cmd)| {
                let score = self.compute_relevance(query, cmd);
                if score > 0.0 {
                    Some(HistorySearchResult {
                        entry_id: i as u64,
                        command: cmd.clone(),
                        relevance_score: score,
                    })
                } else {
                    None
                }
            })
            .collect();
        results.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        results
    }

    fn compute_relevance(&self, query: &str, command: &str) -> f32 {
        if command.contains(query) {
            1.0
        } else {
            let query_words: Vec<&str> = query.split_whitespace().collect();
            let mut matches = 0;
            for qw in &query_words {
                if command.contains(qw) {
                    matches += 1;
                }
            }
            if matches > 0 {
                matches as f32 / query_words.len() as f32 * 0.5
            } else {
                0.0
            }
        }
    }

    pub fn history_count(&self) -> usize {
        self.history.len()
    }
}

// =====================================================================================
// 别名展开 (AliasExpansion)
// =====================================================================================

/// 别名展开器：展开命令别名。
pub struct AliasExpander {
    pub aliases: BTreeMap<String, String>,
    pub expansion_count: u64,
}

impl AliasExpander {
    pub fn new() -> Self {
        Self {
            aliases: BTreeMap::new(),
            expansion_count: 0,
        }
    }

    pub fn define(&mut self, alias: &str, command: &str) {
        self.aliases.insert(alias.to_string(), command.to_string());
    }

    pub fn expand(&mut self, input: &str) -> String {
        let words: Vec<&str> = input.split_whitespace().collect();
        if words.is_empty() {
            return input.to_string();
        }
        let first_word = words[0];
        if let Some(expanded) = self.aliases.get(first_word) {
            self.expansion_count += 1;
            let rest: String = words[1..].iter().map(|w| w.to_string()).collect::<Vec<_>>().join(" ");
            if rest.is_empty() {
                expanded.clone()
            } else {
                format!("{} {}", expanded, rest)
            }
        } else {
            input.to_string()
        }
    }

    pub fn undefine(&mut self, alias: &str) -> bool {
        self.aliases.remove(alias).is_some()
    }

    pub fn alias_count(&self) -> usize {
        self.aliases.len()
    }

    pub fn expansion_count(&self) -> u64 {
        self.expansion_count
    }
}

// =====================================================================================
// 脚本执行 (ScriptExecution)
// =====================================================================================

/// 脚本执行状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptStatus {
    Pending,      // 待处理
    Running,      // 运行中
    Completed,    // 完成
    Failed,       // 失败
}

/// 脚本执行记录。
#[derive(Clone, Debug)]
pub struct ScriptExecutionRecord {
    pub id: u64,
    pub script_name: String,
    pub status: ScriptStatus,
    pub exit_code: Option<i32>,
    pub output: String,
    pub started_at: u128,
    pub completed_at: Option<u128>,
}

/// 脚本执行器：执行 shell 脚本。
pub struct ScriptExecutor {
    pub scripts: BTreeMap<u64, ScriptExecutionRecord>,
    pub next_id: u64,
}

impl ScriptExecutor {
    pub fn new() -> Self {
        Self {
            scripts: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn execute(&mut self, script_name: &str, output: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let exit_code = if output.contains("error") || output.contains("fail") {
            Some(1)
        } else {
            Some(0)
        };
        let status = if exit_code == Some(0) {
            ScriptStatus::Completed
        } else {
            ScriptStatus::Failed
        };
        self.scripts.insert(id, ScriptExecutionRecord {
            id,
            script_name: script_name.to_string(),
            status,
            exit_code,
            output: output.to_string(),
            started_at: crate::ai_os::native::now_ns(),
            completed_at: Some(crate::ai_os::native::now_ns()),
        });
        id
    }

    pub fn get_record(&self, id: u64) -> Option<&ScriptExecutionRecord> {
        self.scripts.get(&id)
    }

    pub fn script_count(&self) -> usize {
        self.scripts.len()
    }

    pub fn success_count(&self) -> usize {
        self.scripts.values().filter(|s| s.status == ScriptStatus::Completed).count()
    }
}

// =====================================================================================
// 环境变量模板 (EnvironmentTemplate)
// =====================================================================================

/// 环境变量模板。
#[derive(Clone, Debug)]
pub struct EnvironmentTemplate {
    pub name: String,
    pub variables: BTreeMap<String, String>,
    pub description: String,
}

/// 环境变量模板管理器：管理环境变量模板。
pub struct EnvironmentTemplateManager {
    pub templates: BTreeMap<String, EnvironmentTemplate>,
}

impl EnvironmentTemplateManager {
    pub fn new() -> Self {
        Self {
            templates: BTreeMap::new(),
        }
    }

    pub fn create(&mut self, name: &str, description: &str, variables: &[(&str, &str)]) -> u64 {
        let vars: BTreeMap<String, String> = variables.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        self.templates.insert(name.to_string(), EnvironmentTemplate {
            name: name.to_string(),
            variables: vars,
            description: description.to_string(),
        });
        self.templates.len() as u64
    }

    pub fn apply(&mut self, template_name: &str, env: &mut BTreeMap<String, String>) -> bool {
        if let Some(template) = self.templates.get(template_name) {
            for (k, v) in &template.variables {
                env.insert(k.clone(), v.clone());
            }
            true
        } else {
            false
        }
    }

    pub fn template_count(&self) -> usize {
        self.templates.len()
    }
}

// =====================================================================================
// Shell 扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn command_history_search() {
        let mut chs = CommandHistorySearch::new();
        chs.add("ls -la");
        chs.add("ls -la /home");
        chs.add("grep pattern file.txt");
        chs.add("cat file.txt");
        let results = chs.search("ls", 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].command, "ls -la");
        assert!(results[0].relevance_score > 0.0);
        assert_eq!(chs.history_count(), 4);
    }

    #[test]
    fn alias_expansion() {
        let mut ae = AliasExpander::new();
        ae.define("ll", "ls -la");
        ae.define("g", "grep");
        let expanded = ae.expand("ll /home");
        assert_eq!(expanded, "ls -la /home");
        let expanded = ae.expand("g pattern file");
        assert_eq!(expanded, "grep pattern file");
        let expanded = ae.expand("unknown cmd");
        assert_eq!(expanded, "unknown cmd");
        assert_eq!(ae.expansion_count(), 2);
        ae.undefine("ll");
        assert_eq!(ae.alias_count(), 1);
    }

    #[test]
    fn script_execution() {
        let mut se = ScriptExecutor::new();
        let id = se.execute("backup.sh", "backup completed successfully");
        assert_eq!(id, 1);
        let record = se.get_record(id).unwrap();
        assert_eq!(record.status, ScriptStatus::Completed);
        assert_eq!(record.exit_code, Some(0));
        let id2 = se.execute("bad.sh", "error: command not found");
        let record2 = se.get_record(id2).unwrap();
        assert_eq!(record2.status, ScriptStatus::Failed);
        assert_eq!(se.script_count(), 2);
        assert_eq!(se.success_count(), 1);
    }

    #[test]
    fn environment_template() {
        let mut tm = EnvironmentTemplateManager::new();
        let id = tm.create("dev", "开发环境", &[("NODE_ENV", "development"), ("DEBUG", "true")]);
        assert_eq!(id, 1);
        let mut env = BTreeMap::new();
        tm.apply("dev", &mut env);
        assert_eq!(env.get("NODE_ENV"), Some(&"development".into()));
        assert_eq!(env.get("DEBUG"), Some(&"true".into()));
        assert_eq!(tm.template_count(), 1);
    }

    #[test]
    fn script_statuses() {
        assert_eq!(ScriptStatus::Pending as u32, 0);
        assert_eq!(ScriptStatus::Failed as u32, 3);
    }
}

// =====================================================================================
// 脚本解析器 (ScriptParser)
// =====================================================================================

/// 脚本命令类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptCommandType {
    Assignment,    // 变量赋值
    Command,       // 外部命令
    If,            // 条件判断
    Then,          // 条件真分支
    Else,          // 条件假分支
    Fi,            // 条件结束
    For,           // 循环开始
    Do,            // 循环体
    Done,          // 循环结束
    While,         // while 循环
    Function,      // 函数定义
    Call,          // 函数调用
    Return,        // 函数返回
    Echo,          // 输出
    Comment,       // 注释
    Export,        // 导出变量
    Unset,         // 取消变量
    Source,        // source 命令
}

/// 脚本解析节点。
#[derive(Clone, Debug)]
pub struct ScriptNode {
    pub id: u64,
    pub command_type: ScriptCommandType,
    pub args: Vec<String>,
    pub children: Vec<u64>,
    pub line_number: u32,
}

/// 脚本解析器：解析 shell 脚本为 AST。
pub struct ScriptParser {
    pub nodes: BTreeMap<u64, ScriptNode>,
    pub next_id: u64,
    pub root_id: Option<u64>,
    pub parse_errors: Vec<String>,
}

impl ScriptParser {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            next_id: 1,
            root_id: None,
            parse_errors: Vec::new(),
        }
    }

    pub fn parse_line(&mut self, line: &str, line_number: u32) -> Option<u64> {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            let id = self.next_id;
            self.next_id += 1;
            self.nodes.insert(id, ScriptNode {
                id,
                command_type: ScriptCommandType::Comment,
                args: vec![trimmed.to_string()],
                children: Vec::new(),
                line_number,
            });
            return Some(id);
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let command_type = match parts[0] {
            "if" => ScriptCommandType::If,
            "then" => ScriptCommandType::Then,
            "else" => ScriptCommandType::Else,
            "fi" => ScriptCommandType::Fi,
            "for" => ScriptCommandType::For,
            "do" => ScriptCommandType::Do,
            "done" => ScriptCommandType::Done,
            "while" => ScriptCommandType::While,
            "function" => ScriptCommandType::Function,
            "return" => ScriptCommandType::Return,
            "echo" => ScriptCommandType::Echo,
            "export" => ScriptCommandType::Export,
            "unset" => ScriptCommandType::Unset,
            "source" => ScriptCommandType::Source,
            _ => {
                if parts[0].contains('=') {
                    ScriptCommandType::Assignment
                } else {
                    ScriptCommandType::Command
                }
            }
        };

        let id = self.next_id;
        self.next_id += 1;
        let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
        self.nodes.insert(id, ScriptNode {
            id,
            command_type,
            args,
            children: Vec::new(),
            line_number,
        });

        if self.root_id.is_none() {
            self.root_id = Some(id);
        }

        Some(id)
    }

    pub fn add_child(&mut self, parent_id: u64, child_id: u64) -> bool {
        if let Some(node) = self.nodes.get_mut(&parent_id) {
            node.children.push(child_id);
            true
        } else {
            false
        }
    }

    pub fn get_node(&self, id: u64) -> Option<&ScriptNode> {
        self.nodes.get(&id)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn error_count(&self) -> usize {
        self.parse_errors.len()
    }
}

// =====================================================================================
// 作业控制 (JobControl)
// =====================================================================================

/// 作业状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Stopped,
    Completed,
    Failed,
}

/// 作业：后台运行的任务。
#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub command: String,
    pub status: JobStatus,
    pub pid: u64,
    pub start_time_ns: u128,
    pub end_time_ns: Option<u128>,
    pub output: String,
}

/// 作业控制器：管理后台作业。
pub struct JobController {
    pub jobs: BTreeMap<u64, Job>,
    pub next_id: u64,
    pub max_jobs: usize,
}

impl JobController {
    pub fn new() -> Self {
        Self {
            jobs: BTreeMap::new(),
            next_id: 1,
            max_jobs: 100,
        }
    }

    pub fn start_job(&mut self, command: String, pid: u64) -> u64 {
        if self.jobs.len() >= self.max_jobs {
            return 0;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.insert(id, Job {
            id,
            command,
            status: JobStatus::Running,
            pid,
            start_time_ns: crate::ai_os::native::now_ns(),
            end_time_ns: None,
            output: String::new(),
        });
        id
    }

    pub fn complete_job(&mut self, id: u64, output: String) -> bool {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.status = JobStatus::Completed;
            job.end_time_ns = Some(crate::ai_os::native::now_ns());
            job.output = output;
            true
        } else {
            false
        }
    }

    pub fn fail_job(&mut self, id: u64, output: String) -> bool {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.status = JobStatus::Failed;
            job.end_time_ns = Some(crate::ai_os::native::now_ns());
            job.output = output;
            true
        } else {
            false
        }
    }

    pub fn stop_job(&mut self, id: u64) -> bool {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.status = JobStatus::Stopped;
            true
        } else {
            false
        }
    }

    pub fn get_job(&self, id: u64) -> Option<&Job> {
        self.jobs.get(&id)
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    pub fn running_count(&self) -> usize {
        self.jobs.values().filter(|j| j.status == JobStatus::Running).count()
    }

    pub fn completed_count(&self) -> usize {
        self.jobs.values().filter(|j| j.status == JobStatus::Completed).count()
    }
}

// =====================================================================================
// 环境变量管理器 (EnvironmentManager)
// =====================================================================================

/// 环境变量作用域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvScope {
    Local,   // 局部
    Global,  // 全局
    Exported, // 导出
}

/// 环境变量条目。
#[derive(Clone, Debug)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
    pub scope: EnvScope,
    pub set_at: u128,
}

/// 环境变量管理器。
pub struct EnvironmentManager {
    pub variables: BTreeMap<String, EnvVar>,
    pub next_id: u64,
}

impl EnvironmentManager {
    pub fn new() -> Self {
        let mut vars = BTreeMap::new();
        vars.insert("PATH".to_string(), EnvVar {
            name: "PATH".to_string(),
            value: "/bin:/usr/bin".to_string(),
            scope: EnvScope::Exported,
            set_at: 0,
        });
        vars.insert("HOME".to_string(), EnvVar {
            name: "HOME".to_string(),
            value: "/root".to_string(),
            scope: EnvScope::Exported,
            set_at: 0,
        });
        vars.insert("SHELL".to_string(), EnvVar {
            name: "SHELL".to_string(),
            value: "/bin/hibos".to_string(),
            scope: EnvScope::Exported,
            set_at: 0,
        });
        Self {
            variables: vars,
            next_id: 1,
        }
    }

    pub fn set_var(&mut self, name: &str, value: &str, scope: EnvScope) {
        self.variables.insert(name.to_string(), EnvVar {
            name: name.to_string(),
            value: value.to_string(),
            scope,
            set_at: crate::ai_os::native::now_ns(),
        });
    }

    pub fn get_var(&self, name: &str) -> Option<&String> {
        self.variables.get(name).map(|v| &v.value)
    }

    pub fn unset_var(&mut self, name: &str) -> bool {
        self.variables.remove(name).is_some()
    }

    pub fn export_var(&mut self, name: &str) -> bool {
        if let Some(var) = self.variables.get_mut(name) {
            var.scope = EnvScope::Exported;
            true
        } else {
            false
        }
    }

    pub fn variable_count(&self) -> usize {
        self.variables.len()
    }

    pub fn exported_count(&self) -> usize {
        self.variables.values().filter(|v| v.scope == EnvScope::Exported).count()
    }
}

// =====================================================================================
// 命令别名 (CommandAlias)
// =====================================================================================

