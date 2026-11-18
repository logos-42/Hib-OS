//! 实时生成应用（AppGen）—— 未来的 App 不再预先编译进内核，而是当场生成。
//!
//! 设计信条：
//! 1. 应用 = 一张声明式「说明书」(`AppSpec`)，不是可执行代码。说明书只允许声明
//!    白名单内的界面组件与动作，运行时只解释这些动作，因此天然处于沙箱中，
//!    AI 或用户生成的内容都无法调用任意系统接口。
//! 2. 一切皆通道：每个生成的应用立刻获得专属通道 `app/app-<序号>`，
//!    `sense` 看状态、`act` 调动作，与内核其它资源完全同构。
//! 3. 生成引擎可插拔（`SpecGenerator`）：默认离线模板引擎，无网无密钥也能生成；
//!    可选 LLM 引擎复用 `llm::chat`，失败自动回退模板，不阻塞使用。
//! 4. 全生命周期可管：生成 → 校验 → 注册 → 调用 → 热替换 → 销毁，全程计数与审计。
//!
//! 统一入口：通道 `app/runtime`（生成/列表/状态/销毁）、`app/app-<序号>`（单应用状态与动作）。

use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{OnceLock, RwLock};

/// 单条需求的最大字符数（防止说明书无限膨胀）。
const MAX_REQUIREMENT_CHARS: usize = 200;
/// 单个应用允许的最大组件数。
const MAX_COMPONENTS: usize = 12;
/// 审计日志保留的最大条数。
const MAX_LOG: usize = 32;

/// 动作白名单：说明书只能声明这些动作，运行时按名字解释执行，绝不执行任意代码。
pub const ALLOWED_ACTIONS: &[&str] = &["echo", "note.add", "counter.inc", "clock", "refresh"];

/// 界面组件白名单：说明书里只允许这几种数据元素。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentKind {
    Label,
    Button,
    Input,
    List,
    Progress,
}

impl ComponentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentKind::Label => "label",
            ComponentKind::Button => "button",
            ComponentKind::Input => "input",
            ComponentKind::List => "list",
            ComponentKind::Progress => "progress",
        }
    }

    /// 解析外部（含 LLM 输出）的组件名，未知值一律返回 `None`，由调用方降级处理。
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "label" | "text" => Some(ComponentKind::Label),
            "button" => Some(ComponentKind::Button),
            "input" | "textinput" | "entry" => Some(ComponentKind::Input),
            "list" => Some(ComponentKind::List),
            "progress" | "bar" => Some(ComponentKind::Progress),
            _ => None,
        }
    }
}

/// 说明书中的一个界面组件：`text` 只用于显示，`action` 只能指向白名单动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub kind: ComponentKind,
    pub text: String,
    pub action: String,
}

/// 生成引擎产出的「草稿」：还没分配应用编号，便于引擎保持无状态、可替换。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDraft {
    pub title: String,
    pub components: Vec<Component>,
    pub actions: Vec<String>,
}

/// 应用说明书：一个实时生成应用的全部定义与元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSpec {
    /// 唯一编号，形如 `app-1`；热替换时保持不变。
    pub id: String,
    /// 便于人记忆的短名（中文需求回落为编号）。
    pub name: String,
    pub title: String,
    /// 原始需求，留作审计与再次生成。
    pub requirement: String,
    pub components: Vec<Component>,
    pub actions: Vec<String>,
    /// 生成引擎：`template`（离线）/ `llm`（AI）。
    pub engine: String,
    /// 版本号，热替换时递增。
    pub version: u32,
    pub created_ns: u64,
}

impl AppSpec {
    /// 每个应用一出生就拥有自己的通道，这是「一切皆通道」的落点。
    pub fn channel(&self) -> String {
        format!("app/{}", self.id)
    }

    /// 机器可读视图（能力层 / `--json` / 审计共用）。
    pub fn to_json(&self) -> Value {
        let components: Vec<Value> = self
            .components
            .iter()
            .map(|c| {
                json!({
                    "kind": c.kind.as_str(),
                    "text": c.text,
                    "action": c.action,
                })
            })
            .collect();
        json!({
            "id": self.id,
            "name": self.name,
            "title": self.title,
            "requirement": self.requirement,
            "channel": self.channel(),
            "engine": self.engine,
            "version": self.version,
            "created_ns": self.created_ns,
            "components": components,
            "actions": self.actions,
        })
    }

    /// 真像素 GUI 窗口里显示的内容（纯文本行，复用现有应用渲染路径）。
    pub fn summary_lines(&self) -> Vec<String> {
        let mut rows = vec![
            format!("HIB OS / {}", self.title),
            format!("实时生成 v{} · 引擎 {} · 通道 {}", self.version, self.engine, self.channel()),
            format!("需求: {}", truncate(&self.requirement, 40)),
            "组件:".to_string(),
        ];
        for c in &self.components {
            rows.push(format!("  [{}] {} -> {}", c.kind.as_str(), c.text, c.action));
        }
        rows.push(format!("动作: {}", self.actions.join(", ")));
        rows.push("输入文字后回车（执行 echo 动作）".to_string());
        rows
    }
}

/// 生成/运行期错误，全部是「可解释、可显示」的普通数据，不携带密钥等敏感信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppGenError {
    EmptyRequirement,
    RequirementTooLong(usize),
    InvalidAction(String),
    BadSpec(String),
    NotFound(String),
    Unavailable(String),
}

impl std::fmt::Display for AppGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppGenError::EmptyRequirement => write!(f, "需求不能为空"),
            AppGenError::RequirementTooLong(n) => write!(f, "需求过长: {n} 字符 (上限 {MAX_REQUIREMENT_CHARS})"),
            AppGenError::InvalidAction(a) => write!(f, "动作不在白名单内: {a}"),
            AppGenError::BadSpec(s) => write!(f, "说明书不合法: {s}"),
            AppGenError::NotFound(id) => write!(f, "应用不存在: {id}"),
            AppGenError::Unavailable(s) => write!(f, "生成引擎不可用: {s}"),
        }
    }
}

impl std::error::Error for AppGenError {}

/// 生成引擎接口（策略模式）：换引擎不用改运行时。
pub trait SpecGenerator: Send + Sync {
    fn name(&self) -> &'static str;
    fn draft(&self, requirement: &str) -> Result<AppDraft, AppGenError>;
}

/// 默认引擎：离线、确定性、按需求关键词拼装说明书，永远可用。
pub struct TemplateGenerator;

/// 可选引擎：调用既有 `llm::chat`，让 AI 直接产出说明书 JSON。
pub struct LlmGenerator {
    pub provider: String,
    pub api_key: String,
}

/// 按字符截断，避免中文被切成半个 UTF-8 字节。
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 需求入口校验：空、过长、含控制字符一律拒绝。
fn check_requirement(requirement: &str) -> Result<&str, AppGenError> {
    let r = requirement.trim();
    if r.is_empty() {
        return Err(AppGenError::EmptyRequirement);
    }
    let len = r.chars().count();
    if len > MAX_REQUIREMENT_CHARS {
        return Err(AppGenError::RequirementTooLong(len));
    }
    if r.chars().any(|c| c.is_control()) {
        return Err(AppGenError::BadSpec("需求包含控制字符".to_string()));
    }
    Ok(r)
}

/// 关键词命中判断（大小写不敏感，中文原样比较）。
fn contains_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// 组件与动作成对登记，保证「组件引用的动作一定被声明」。
fn push_component(
    components: &mut Vec<Component>,
    actions: &mut Vec<String>,
    kind: ComponentKind,
    text: &str,
    action: &str,
) {
    components.push(Component {
        kind,
        text: text.to_string(),
        action: action.to_string(),
    });
    if !actions.iter().any(|a| a == action) {
        actions.push(action.to_string());
    }
}

/// 通用形态：任何需求都能生成一个能看、能输入、能点的最小应用。
fn generic_draft(requirement: &str) -> AppDraft {
    let mut components = Vec::new();
    let mut actions = Vec::new();
    push_component(&mut components, &mut actions, ComponentKind::Label, &truncate(requirement, 48), "refresh");
    push_component(&mut components, &mut actions, ComponentKind::Input, "输入内容…", "echo");
    push_component(&mut components, &mut actions, ComponentKind::Button, "回显", "echo");
    push_component(&mut components, &mut actions, ComponentKind::List, "最近输出", "refresh");
    AppDraft {
        title: format!("{} · 实时生成", truncate(requirement, 24)),
        components,
        actions,
    }
}

impl SpecGenerator for TemplateGenerator {
    fn name(&self) -> &'static str {
        "template"
    }

    fn draft(&self, requirement: &str) -> Result<AppDraft, AppGenError> {
        // 先校验，再按关键词挑选组件：整个过程只拼数据，不调用任何外部接口。
        let req = check_requirement(requirement)?;
        let lower = req.to_ascii_lowercase();
        let mut base = generic_draft(req);

        if contains_any(&lower, &["计时", "时钟", "时间", "番茄", "timer", "clock"]) {
            push_component(&mut base.components, &mut base.actions, ComponentKind::Progress, "时间进度", "clock");
        }
        if contains_any(&lower, &["笔记", "记事", "备忘", "清单", "note", "todo", "list"]) {
            push_component(&mut base.components, &mut base.actions, ComponentKind::Input, "写一条笔记…", "note.add");
            push_component(&mut base.components, &mut base.actions, ComponentKind::List, "笔记列表", "refresh");
        }
        if contains_any(&lower, &["计数", "点击", "counter", "click", "count"]) {
            push_component(&mut base.components, &mut base.actions, ComponentKind::Button, "点我 +1", "counter.inc");
        }
        if contains_any(&lower, &["监控", "状态", "系统", "monitor", "status"]) {
            push_component(&mut base.components, &mut base.actions, ComponentKind::List, "内核状态", "clock");
        }
        Ok(base)
    }
}

impl SpecGenerator for LlmGenerator {
    fn name(&self) -> &'static str {
        "llm"
    }

    fn draft(&self, requirement: &str) -> Result<AppDraft, AppGenError> {
        let req = check_requirement(requirement)?;
        // 要求模型只输出 JSON；即便输出夹带解释文字，解析器也只取花括号部分。
        let prompt = format!(
            "你是 Hib OS 的应用说明书编译器。把用户需求编译成 JSON，不要输出多余文字。\
             格式: {{\"title\":\"...\",\"components\":[{{\"kind\":\"label|button|input|list|progress\",\
             \"text\":\"...\",\"action\":\"echo|note.add|counter.inc|clock|refresh\"}}]}}。\
             组件最多 12 个，text 不超过 80 字，action 只能从列出的动作里选。需求: {req}"
        );
        let raw = crate::llm::chat(&self.provider, &self.api_key, &prompt)
            .map_err(|e| AppGenError::Unavailable(format!("LLM 调用失败: {e}")))?;
        parse_llm_draft(&raw)
    }
}

/// 把 LLM 回复解析成草稿：字段缺失走默认值，越界组件/动作一律丢弃，最后仍由 `validate` 兜底。
fn parse_llm_draft(raw: &str) -> Result<AppDraft, AppGenError> {
    let start = raw.find('{');
    let end = raw.rfind('}');
    let (Some(start), Some(end)) = (start, end) else {
        return Err(AppGenError::BadSpec("LLM 回复中没有 JSON".to_string()));
    };
    if end < start {
        return Err(AppGenError::BadSpec("LLM 回复中的 JSON 不完整".to_string()));
    }
    let value: Value = serde_json::from_str(&raw[start..=end])
        .map_err(|e| AppGenError::BadSpec(format!("JSON 解析失败: {e}")))?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(|s| truncate(s, 48))
        .unwrap_or_else(|| "AI 实时生成应用".to_string());

    let mut components = Vec::new();
    let mut actions = Vec::new();
    if let Some(items) = value.get("components").and_then(Value::as_array) {
        for item in items.iter().take(MAX_COMPONENTS) {
            // 未知组件名降级为纯文本标签；非法动作直接跳过，不进入说明书。
            let kind = item
                .get("kind")
                .and_then(Value::as_str)
                .and_then(ComponentKind::parse)
                .unwrap_or(ComponentKind::Label);
            let text = truncate(item.get("text").and_then(Value::as_str).unwrap_or(""), 80);
            let action = item.get("action").and_then(Value::as_str).unwrap_or("echo");
            if !ALLOWED_ACTIONS.contains(&action) {
                continue;
            }
            push_component(&mut components, &mut actions, kind, &text, action);
        }
    }
    if components.is_empty() {
        return Err(AppGenError::BadSpec("LLM 未给出可用组件".to_string()));
    }
    Ok(AppDraft { title, components, actions })
}

/// 说明书沙箱校验：所有外部来源（模板、LLM、能力层）都必须过这一关。
pub fn validate(spec: &AppSpec) -> Result<(), AppGenError> {
    if spec.id.is_empty() || spec.name.is_empty() || spec.title.is_empty() {
        return Err(AppGenError::BadSpec("标识或标题不能为空".to_string()));
    }
    let _ = check_requirement(&spec.requirement)?;
    if spec.components.is_empty() || spec.components.len() > MAX_COMPONENTS {
        return Err(AppGenError::BadSpec(format!(
            "组件数量必须在 1..={MAX_COMPONENTS}"
        )));
    }
    for action in &spec.actions {
        if !ALLOWED_ACTIONS.contains(&action.as_str()) {
            return Err(AppGenError::InvalidAction(action.clone()));
        }
    }
    for c in &spec.components {
        if c.text.chars().count() > 80 || c.text.chars().any(|ch| ch.is_control()) {
            return Err(AppGenError::BadSpec("组件文本超长或含控制字符".to_string()));
        }
        // 组件不能引用未声明 / 非白名单的动作，杜绝「说明书里夹带私货」。
        if !ALLOWED_ACTIONS.contains(&c.action.as_str()) || !spec.actions.contains(&c.action) {
            return Err(AppGenError::InvalidAction(c.action.clone()));
        }
    }
    Ok(())
}

/// 单个应用的轻量运行状态（只存数据，不持有任何可执行对象）。
#[derive(Clone, Debug, Default)]
pub struct AppInstanceState {
    pub counter: u64,
    pub notes: Vec<String>,
    pub invocations: u64,
    pub last_action: String,
}

/// 应用运行时：说明书注册表 + 状态 + 监控计数 + 审计日志。
#[derive(Debug)]
pub struct AppRuntime {
    apps: BTreeMap<String, AppSpec>,
    names: BTreeMap<String, String>,
    state: BTreeMap<String, AppInstanceState>,
    next_seq: u64,
    generated: u64,
    rejected: u64,
    invoked: u64,
    replaced: u64,
    dropped: u64,
    llm_fallbacks: u64,
    log: VecDeque<String>,
}

impl AppRuntime {
    pub fn new() -> Self {
        Self {
            apps: BTreeMap::new(),
            names: BTreeMap::new(),
            state: BTreeMap::new(),
            next_seq: 0,
            generated: 0,
            rejected: 0,
            invoked: 0,
            replaced: 0,
            dropped: 0,
            llm_fallbacks: 0,
            log: VecDeque::new(),
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > MAX_LOG {
            self.log.pop_front();
        }
    }

    /// LLM 失败已回退模板，记一笔便于排查（不记录 API Key）。
    pub fn record_fallback(&mut self, note: &str) {
        self.llm_fallbacks += 1;
        self.push_log(note.to_string());
    }

    /// 把需求翻译成短名：英文数字保留，纯中文需求回落为编号。
    fn slug(&self, requirement: &str, seq: u64) -> String {
        let s: String = requirement
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(20)
            .collect::<String>()
            .to_ascii_lowercase();
        let candidate = if s.is_empty() { format!("app-{seq}") } else { s };
        if self.names.contains_key(&candidate) {
            format!("{candidate}-{seq}")
        } else {
            candidate
        }
    }

    /// 生成并注册一个新应用：分配编号 → 组成说明书 → 沙箱校验 → 登记。
    pub fn install(
        &mut self,
        requirement: &str,
        draft: AppDraft,
        engine: &str,
    ) -> Result<AppSpec, AppGenError> {
        let req = check_requirement(requirement)?;
        self.next_seq += 1;
        let seq = self.next_seq;
        let id = format!("app-{seq}");
        let name = self.slug(req, seq);
        let spec = AppSpec {
            id: id.clone(),
            name: name.clone(),
            title: truncate(&draft.title, 48),
            requirement: req.to_string(),
            components: draft.components,
            actions: draft.actions,
            engine: engine.to_string(),
            version: 1,
            created_ns: crate::monitor::get_time_ns(),
        };
        validate(&spec).map_err(|e| {
            self.rejected += 1;
            self.push_log(format!("拒绝生成: {e}"));
            e
        })?;
        self.apps.insert(id.clone(), spec.clone());
        self.names.insert(name, id.clone());
        self.state.insert(id.clone(), AppInstanceState::default());
        self.generated += 1;
        self.push_log(format!(
            "生成 id={id} engine={engine} channel={} 需求={}",
            spec.channel(),
            truncate(req, 24)
        ));
        Ok(spec)
    }

    /// 热替换：编号与通道保持不变，说明书与版本号更新，正在看的窗口不会"断线"。
    pub fn replace(
        &mut self,
        id: &str,
        requirement: &str,
        draft: AppDraft,
        engine: &str,
    ) -> Result<AppSpec, AppGenError> {
        let old = self
            .apps
            .get(id)
            .cloned()
            .ok_or_else(|| AppGenError::NotFound(id.to_string()))?;
        let req = check_requirement(requirement)?;
        let spec = AppSpec {
            requirement: req.to_string(),
            title: truncate(&draft.title, 48),
            components: draft.components,
            actions: draft.actions,
            engine: engine.to_string(),
            version: old.version + 1,
            ..old
        };
        validate(&spec).map_err(|e| {
            self.rejected += 1;
            self.push_log(format!("拒绝热替换 {id}: {e}"));
            e
        })?;
        self.apps.insert(id.to_string(), spec.clone());
        self.state.entry(id.to_string()).or_default();
        self.replaced += 1;
        self.push_log(format!("热替换 id={id} -> v{} engine={engine}", spec.version));
        Ok(spec)
    }

    /// 解释执行一个白名单动作：这里没有 `eval`，只有固定的几条分支。
    pub fn invoke(&mut self, id: &str, action: &str, payload: &str) -> Result<String, AppGenError> {
        let spec = self
            .apps
            .get(id)
            .cloned()
            .ok_or_else(|| AppGenError::NotFound(id.to_string()))?;
        if !ALLOWED_ACTIONS.contains(&action) || !spec.actions.contains(&action.to_string()) {
            return Err(AppGenError::InvalidAction(action.to_string()));
        }
        // 把状态借用限制在这个小块里，方便之后更新全局计数。
        let text = {
            let st = self.state.entry(id.to_string()).or_default();
            st.invocations += 1;
            st.last_action = action.to_string();
            match action {
                "echo" => format!("{} ← {}", spec.title, payload),
                "note.add" => {
                    if payload.trim().is_empty() {
                        "笔记内容不能为空".to_string()
                    } else {
                        st.notes.push(payload.to_string());
                        format!("已记下: {} (共 {} 条)", payload, st.notes.len())
                    }
                }
                "counter.inc" => {
                    st.counter += 1;
                    format!("计数 = {}", st.counter)
                }
                "clock" => format!("内核时间 = {} ns", crate::monitor::get_time_ns()),
                "refresh" => format!("{} 个组件已刷新", spec.components.len()),
                _ => return Err(AppGenError::InvalidAction(action.to_string())),
            }
        };
        self.invoked += 1;
        self.push_log(format!("调用 id={id} action={action}"));
        Ok(text)
    }

    /// 销毁应用：说明书、短名索引、运行状态一起移除，通道随之失效。
    pub fn drop(&mut self, id: &str) -> Result<AppSpec, AppGenError> {
        let spec = self
            .apps
            .remove(id)
            .ok_or_else(|| AppGenError::NotFound(id.to_string()))?;
        self.names.remove(&spec.name);
        self.state.remove(id);
        self.dropped += 1;
        self.push_log(format!("销毁 id={id}"));
        Ok(spec)
    }

    pub fn inspect(&self, id: &str) -> Option<AppSpec> {
        self.apps
            .get(id)
            .cloned()
            .or_else(|| self.names.get(id).and_then(|real| self.apps.get(real).cloned()))
    }

    pub fn list(&self) -> String {
        if self.apps.is_empty() {
            return "暂无实时生成的应用；用 app generate <需求> 生成第一个。".to_string();
        }
        let mut out = format!("实时生成的应用 {} 个:\n", self.apps.len());
        for spec in self.apps.values() {
            out.push_str(&format!(
                "  {} v{} [{}] {} | {} | actions={}\n",
                spec.id,
                spec.version,
                spec.engine,
                spec.title,
                spec.channel(),
                spec.actions.join(",")
            ));
        }
        out.trim_end().to_string()
    }

    /// 监控视图：计数 + 最近日志，供 `sense app/runtime` 与 `app status` 使用。
    pub fn status(&self) -> String {
        let mut out = format!(
            "实时应用运行时: 在线={} 生成={} 拒绝={} 调用={} 热替换={} 销毁={} LLM回退={}",
            self.apps.len(),
            self.generated,
            self.rejected,
            self.invoked,
            self.replaced,
            self.dropped,
            self.llm_fallbacks
        );
        if !self.log.is_empty() {
            out.push_str("\n最近记录:");
            for line in self.log.iter().rev().take(8) {
                out.push_str("\n  ");
                out.push_str(line);
            }
        }
        out
    }
}

static RUNTIME: OnceLock<RwLock<AppRuntime>> = OnceLock::new();

/// 全局唯一运行时（懒加载，读写锁与项目其它注册表保持一致）。
pub fn runtime() -> &'static RwLock<AppRuntime> {
    RUNTIME.get_or_init(|| RwLock::new(AppRuntime::new()))
}

/// 从环境变量找可用的 LLM 凭据；找不到就返回 `None`，由调用方回退离线模板。
fn llm_credentials() -> Option<(String, String)> {
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        if !key.is_empty() {
            return Some(("deepseek".to_string(), key));
        }
    }
    for p in crate::llm::known_providers() {
        let env_key = format!("HIBOS_LLM_{}_KEY", p.name.to_ascii_uppercase());
        if let Ok(key) = std::env::var(&env_key) {
            if !key.is_empty() {
                return Some((p.name.clone(), key));
            }
        }
    }
    None
}

fn try_llm_draft(requirement: &str) -> Result<AppDraft, AppGenError> {
    let (provider, api_key) =
        llm_credentials().ok_or_else(|| AppGenError::Unavailable("未配置 LLM API Key".to_string()))?;
    LlmGenerator { provider, api_key }.draft(requirement)
}

/// 选引擎：要 AI 就试 LLM，任何失败都回退离线模板，保证「随时能生成」。
fn draft_with_fallback(
    requirement: &str,
    use_llm: bool,
) -> Result<(AppDraft, &'static str, Option<String>), AppGenError> {
    if use_llm {
        match try_llm_draft(requirement) {
            Ok(draft) => return Ok((draft, "llm", None)),
            Err(e) => {
                let note = format!("LLM 生成不可用（{e}），已回退离线模板");
                return Ok((TemplateGenerator.draft(requirement)?, "template", Some(note)));
            }
        }
    }
    Ok((TemplateGenerator.draft(requirement)?, "template", None))
}

/// 对外主入口：一句话需求 → 一个立即可用的应用。
pub fn generate_app(requirement: &str, use_llm: bool) -> Result<AppSpec, AppGenError> {
    let (draft, engine, note) = draft_with_fallback(requirement, use_llm)?;
    let mut rt = runtime().write().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = note {
        rt.record_fallback(&n);
    }
    rt.install(requirement, draft, engine)
}

/// 热替换入口：编号与通道不变，内容按新需求重新生成。
pub fn replace_app(id: &str, requirement: &str, use_llm: bool) -> Result<AppSpec, AppGenError> {
    let (draft, engine, note) = draft_with_fallback(requirement, use_llm)?;
    let mut rt = runtime().write().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = note {
        rt.record_fallback(&n);
    }
    rt.replace(id, requirement, draft, engine)
}

pub fn drop_app(id: &str) -> Result<AppSpec, AppGenError> {
    // 写守卫本身实现了 `Drop`，链式 `.drop(id)` 会被误解析成析构方法；
    // 这里用完全限定形式显式调用 AppRuntime 自己的移除方法。
    let mut rt = runtime().write().unwrap_or_else(|e| e.into_inner());
    AppRuntime::drop(&mut *rt, id)
}

pub fn invoke_by_id(id: &str, action: &str, payload: &str) -> Result<String, String> {
    runtime()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .invoke(id, action, payload)
        .map_err(|e| e.to_string())
}

pub fn find_spec(name_or_id: &str) -> Option<AppSpec> {
    runtime()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .inspect(name_or_id)
}

/// 动态通道是否存在（Shell 的 `ensure_dynamic_channel` 用它按需开通）。
pub fn has_channel(channel: &str) -> bool {
    let Some(id) = channel.strip_prefix("app/") else {
        return false;
    };
    runtime()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .apps
        .contains_key(id)
}

pub fn list_text() -> String {
    runtime().read().unwrap_or_else(|e| e.into_inner()).list()
}

pub fn status_text() -> String {
    runtime().read().unwrap_or_else(|e| e.into_inner()).status()
}

// ---------------------------------------------------------------------------
// 通道层：`sense` / `act` 的落点（由 kernel/sense.rs 分发）
// ---------------------------------------------------------------------------

/// `sense app/runtime`：运行时监控视图。
pub fn sense_runtime() -> String {
    status_text()
}

/// `sense app/app-<序号>`：单个应用的状态。
pub fn sense_app(channel: &str) -> Option<String> {
    let id = channel.strip_prefix("app/")?;
    let rt = runtime().read().unwrap_or_else(|e| e.into_inner());
    let spec = rt.apps.get(id)?;
    let st = rt.state.get(id);
    Some(format!(
        "{} v{} 引擎={} 标题={} 需求={} 组件={} 动作={} 调用={} 计数={} 笔记={} 最近动作={}",
        spec.id,
        spec.version,
        spec.engine,
        spec.title,
        truncate(&spec.requirement, 32),
        spec.components.len(),
        spec.actions.join(","),
        st.map(|s| s.invocations).unwrap_or(0),
        st.map(|s| s.counter).unwrap_or(0),
        st.map(|s| s.notes.len()).unwrap_or(0),
        st.map(|s| s.last_action.as_str()).unwrap_or("-"),
    ))
}

/// `act app/runtime <负载>`：generate / list / status / inspect / invoke / drop。
pub fn act_runtime(payload: &str) -> Result<String, String> {
    let payload = payload.trim();
    let (verb, rest) = payload.split_once(' ').unwrap_or((payload, ""));
    let rest = rest.trim();
    match verb {
        "generate" | "gen" => {
            let spec = generate_app(rest, false).map_err(|e| e.to_string())?;
            Ok(format!(
                "已生成 {} v{} 通道={} 引擎={} 组件={} 动作={}",
                spec.id,
                spec.version,
                spec.channel(),
                spec.engine,
                spec.components.len(),
                spec.actions.join(",")
            ))
        }
        "list" => Ok(list_text()),
        "status" => Ok(status_text()),
        "inspect" => find_spec(rest)
            .map(|s| s.to_json().to_string())
            .ok_or_else(|| format!("应用不存在: {rest}")),
        "drop" | "remove" => drop_app(rest)
            .map(|s| format!("已销毁 {} (v{})", s.id, s.version))
            .map_err(|e| e.to_string()),
        "invoke" => {
            let mut it = rest.splitn(3, ' ');
            let id = it.next().unwrap_or("");
            let action = it.next().unwrap_or("");
            let arg = it.next().unwrap_or("");
            invoke_by_id(id, action, arg)
        }
        _ => Err(
            "用法: act app/runtime generate <需求> | list | status | inspect <id> | invoke <id> <动作> [参数] | drop <id>"
                .to_string(),
        ),
    }
}

/// `act app/app-<序号> <负载>`：`status` / `invoke <动作> [参数]`，也支持直接写动作名。
pub fn act_app(channel: &str, payload: &str) -> Result<String, String> {
    let id = channel
        .strip_prefix("app/")
        .ok_or_else(|| format!("非法应用通道: {channel}"))?;
    let payload = payload.trim();
    let (verb, rest) = payload.split_once(' ').unwrap_or((payload, ""));
    let rest = rest.trim();
    match verb {
        "" | "status" => sense_app(channel).ok_or_else(|| format!("应用不存在: {id}")),
        "invoke" => {
            let (action, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            invoke_by_id(id, action, arg.trim())
        }
        other if ALLOWED_ACTIONS.contains(&other) => invoke_by_id(id, other, rest),
        _ => Err(format!(
            "用法: act {channel} status | invoke <动作> [参数]；可用动作: {}",
            ALLOWED_ACTIONS.join(",")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(req: &str) -> AppDraft {
        TemplateGenerator.draft(req).unwrap()
    }

    #[test]
    fn generate_validate_invoke_replace_drop_lifecycle() {
        let mut rt = AppRuntime::new();
        let spec = rt.install("做一个记事清单", draft("做一个记事清单"), "template").unwrap();
        assert_eq!(spec.version, 1);
        assert_eq!(spec.channel(), format!("app/{}", spec.id));
        assert!(spec.actions.contains(&"note.add".to_string()));

        let out = rt.invoke(&spec.id, "note.add", "买牛奶").unwrap();
        assert!(out.contains("买牛奶"));
        assert!(rt.invoke(&spec.id, "system.rm", "").is_err());

        let newer = rt
            .replace(&spec.id, "改成番茄钟", draft("改成番茄钟"), "template")
            .unwrap();
        assert_eq!(newer.version, 2);
        assert_eq!(newer.id, spec.id);
        assert_eq!(newer.channel(), spec.channel());
        assert!(newer.actions.contains(&"clock".to_string()));

        let dropped = rt.drop(&spec.id).unwrap();
        assert_eq!(dropped.id, spec.id);
        assert!(rt.invoke(&spec.id, "echo", "hi").is_err());
        assert_eq!((rt.generated, rt.replaced, rt.dropped), (1, 1, 1));
    }

    #[test]
    fn sandbox_rejects_bad_input() {
        let mut rt = AppRuntime::new();
        assert!(matches!(
            TemplateGenerator.draft("  "),
            Err(AppGenError::EmptyRequirement)
        ));
        assert!(matches!(
            TemplateGenerator.draft(&"长".repeat(MAX_REQUIREMENT_CHARS + 1)),
            Err(AppGenError::RequirementTooLong(_))
        ));

        // 说明书里夹带非白名单动作，必须在校验阶段被挡住。
        let mut bad = draft("普通应用");
        bad.components[0].action = "system.shutdown".to_string();
        bad.actions.push("system.shutdown".to_string());
        let err = rt.install("普通应用", bad, "template").unwrap_err();
        assert!(matches!(err, AppGenError::InvalidAction(_)));
        assert_eq!(rt.rejected, 1);
        assert_eq!(rt.generated, 0);
    }

    #[test]
    fn llm_output_is_sandboxed() {
        let raw = "这是说明 {\"title\":\"AI 应用\",\"components\":[\
            {\"kind\":\"button\",\"text\":\"点我\",\"action\":\"counter.inc\"},\
            {\"kind\":\"unknown\",\"text\":\"带私货\",\"action\":\"system.rm\"},\
            {\"kind\":\"label\",\"text\":\"你好\",\"action\":\"echo\"}]} 结束";
        let parsed = parse_llm_draft(raw).unwrap();
        // 非法动作的组件被丢弃；其余组件仍可用，且全部动作都在白名单里。
        assert_eq!(parsed.components.len(), 2);
        assert!(parsed.actions.iter().all(|a| ALLOWED_ACTIONS.contains(&a.as_str())));
        assert!(parse_llm_draft("模型没有返回 JSON").is_err());
    }

    #[test]
    fn template_is_deterministic_and_keyword_aware() {
        let a = draft("做一个番茄钟");
        let b = draft("做一个番茄钟");
        assert_eq!(a, b);
        assert!(a.actions.contains(&"clock".to_string()));
        assert!(!draft("随便写点东西").actions.contains(&"clock".to_string()));
    }

    #[test]
    fn monitoring_and_channel_view() {
        let mut rt = AppRuntime::new();
        let spec = rt.install("计数小工具", draft("计数小工具"), "template").unwrap();
        rt.invoke(&spec.id, "counter.inc", "").unwrap();
        assert_eq!(rt.state.get(&spec.id).unwrap().counter, 1);
        assert!(rt.status().contains("生成=1"));
        assert!(rt.list().contains(&spec.id));
        assert!(rt.inspect(&spec.name).is_some());
        assert!(rt.inspect("不存在").is_none());
    }
}
