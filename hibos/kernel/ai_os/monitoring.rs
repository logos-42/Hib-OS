//! 监控告警层 —— AI 原生 OS 的「可观测性」 (Monitoring & Alerting)
//!
//! 本模块实现 AI 原生 OS 的监控告警，对应传统 OS 中的
//! 「性能监控 / 日志审计 / 告警通知」概念，但重新映射到监控域：
//!
//! * MetricCollector (指标采集器) —— 采集系统各项指标，等价于 Prometheus 采集器；
//! * MetricRegistry (指标注册表) —— 指标定义注册，等价于指标字典；
//! * TimeSeriesDB (时间序列数据库) —— 存储时间序列数据，等价于 TSDB；
//! * AlertRule (告警规则) —— 告警触发条件，等价于告警策略；
//! * AlertManager (告警管理器) —— 告警去重/抑制/路由，等价于 Alertmanager；
//! * NotificationChannel (通知通道) —— 告警通知发送，等价于 Webhook/邮件/短信；
//! * Dashboard (仪表板) —— 可视化面板，等价于 Grafana 面板；
//! * LogCollector (日志采集器) —— 采集系统日志，等价于 Fluentd/Filebeat；
//! * LogStore (日志存储) —— 日志持久化存储，等价于 Elasticsearch；
//! * TraceCollector (追踪采集器) —— 分布式追踪采集，等价于 Jaeger/Zipkin；
//! * Span (追踪跨度) —— 单个追踪跨度，等价于 Trace Span；
//! * MetricAggregator (指标聚合器) —— 指标聚合/下采样，等价于录音规则；
//! * SLOMonitor (SLO 监控) —— 服务等级目标监控，等价于 SLO 面板；
//! * MonitoringEngine (监控引擎) —— 统一入口，通过 monitoring://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无外部依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 指标类型 (MetricType)
// =====================================================================================

/// 指标类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetricType {
    Counter,   // 计数器，单调递增
    Gauge,     // 仪表，可增可减
    Histogram, // 直方图，分布统计
    Summary,   // 汇总，分位数统计
}

impl MetricType {
    pub fn name(self) -> &'static str {
        match self {
            MetricType::Counter => "counter",
            MetricType::Gauge => "gauge",
            MetricType::Histogram => "histogram",
            MetricType::Summary => "summary",
        }
    }
}

// =====================================================================================
// 指标定义 (MetricDef)
// =====================================================================================

/// 指标定义。
#[derive(Clone, Debug)]
pub struct MetricDef {
    pub id: u64,
    pub name: String,
    pub r#type: MetricType,
    pub unit: String,
    pub description: String,
    pub labels: Vec<String>,
    pub created_at: u128,
}

impl MetricDef {
    pub fn new(id: u64, name: &str, r#type: MetricType, unit: &str, description: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            r#type,
            unit: unit.to_string(),
            description: description.to_string(),
            labels: Vec::new(),
            created_at: crate::ai_os::native::now_ns(),
        }
    }

    pub fn add_label(&mut self, label: &str) {
        if !self.labels.contains(&label.to_string()) {
            self.labels.push(label.to_string());
        }
    }
}

// =====================================================================================
// 指标值 (MetricValue)
// =====================================================================================

/// 指标值。
#[derive(Clone, Debug)]
pub struct MetricValue {
    pub metric_id: u64,
    pub value: f64,
    pub timestamp_ns: u128,
    pub label_values: BTreeMap<String, String>,
}

/// 指标采集器：采集和存储指标值。
pub struct MetricCollector {
    pub values: VecDeque<MetricValue>,
    pub max_values: usize,
    pub total_collected: u64,
}

impl MetricCollector {
    pub fn new() -> Self {
        Self {
            values: VecDeque::new(),
            max_values: 100000,
            total_collected: 0,
        }
    }

    pub fn collect(&mut self, metric_id: u64, value: f64, labels: BTreeMap<String, String>) {
        self.values.push_back(MetricValue {
            metric_id,
            value,
            timestamp_ns: crate::ai_os::native::now_ns(),
            label_values: labels,
        });
        if self.values.len() > self.max_values {
            self.values.pop_front();
        }
        self.total_collected += 1;
    }

    pub fn get_latest(&self, metric_id: u64) -> Option<&MetricValue> {
        self.values.iter().rev().find(|v| v.metric_id == metric_id)
    }

    pub fn get_values(&self, metric_id: u64, limit: usize) -> Vec<&MetricValue> {
        self.values.iter()
            .filter(|v| v.metric_id == metric_id)
            .rev()
            .take(limit)
            .collect()
    }

    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    pub fn total_collected(&self) -> u64 {
        self.total_collected
    }
}

// =====================================================================================
// 指标注册表 (MetricRegistry)
// =====================================================================================

/// 指标注册表：注册和管理指标定义。
pub struct MetricRegistry {
    pub metrics: BTreeMap<String, MetricDef>,
    pub next_id: u64,
}

impl MetricRegistry {
    pub fn new() -> Self {
        Self {
            metrics: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn register(&mut self, name: &str, r#type: MetricType, unit: &str, description: &str) -> u64 {
        if self.metrics.contains_key(name) {
            return self.metrics[name].id;
        }
        let id = self.next_id;
        self.next_id += 1;
        let def = MetricDef::new(id, name, r#type, unit, description);
        self.metrics.insert(name.to_string(), def);
        id
    }

    pub fn get(&self, name: &str) -> Option<&MetricDef> {
        self.metrics.get(name)
    }

    pub fn get_by_id(&self, id: u64) -> Option<&MetricDef> {
        self.metrics.values().find(|m| m.id == id)
    }

    pub fn add_label(&mut self, name: &str, label: &str) -> bool {
        if let Some(def) = self.metrics.get_mut(name) {
            def.add_label(label);
            true
        } else {
            false
        }
    }

    pub fn metric_count(&self) -> usize {
        self.metrics.len()
    }
}

// =====================================================================================
// 告警规则 (AlertRule)
// =====================================================================================

/// 告警严重级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,      // 信息
    Warning,   // 警告
    Critical,  // 严重
    Emergency, // 紧急
}

/// 告警规则。
#[derive(Clone, Debug)]
pub struct AlertRule {
    pub id: u64,
    pub name: String,
    pub metric_name: String,
    pub condition: String,   // 比较表达式，如 "> 90"
    pub threshold: f64,
    pub severity: AlertSeverity,
    pub for_duration_ns: u128, // 持续时间
    pub enabled: bool,
    pub annotations: BTreeMap<String, String>,
}

/// 告警规则引擎。
pub struct AlertRuleEngine {
    pub rules: BTreeMap<u64, AlertRule>,
    pub next_id: u64,
}

impl AlertRuleEngine {
    pub fn new() -> Self {
        Self {
            rules: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn create_rule(&mut self, name: &str, metric_name: &str, condition: &str, threshold: f64, severity: AlertSeverity, for_duration_ns: u128) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut annotations = BTreeMap::new();
        annotations.insert("summary".to_string(), format!("{}: {}", name, metric_name));
        annotations.insert("description".to_string(), format!("Metric {} {} {}", metric_name, condition, threshold));
        self.rules.insert(id, AlertRule {
            id,
            name: name.to_string(),
            metric_name: metric_name.to_string(),
            condition: condition.to_string(),
            threshold,
            severity,
            for_duration_ns,
            enabled: true,
            annotations,
        });
        id
    }

    pub fn evaluate(&self, rule_id: u64, value: f64) -> bool {
        if let Some(rule) = self.rules.get(&rule_id) {
            if !rule.enabled {
                return false;
            }
            match rule.condition.as_str() {
                ">" => value > rule.threshold,
                "<" => value < rule.threshold,
                ">=" => value >= rule.threshold,
                "<=" => value <= rule.threshold,
                "==" => (value - rule.threshold).abs() < 1e-6,
                "!=" => (value - rule.threshold).abs() >= 1e-6,
                _ => false,
            }
        } else {
            false
        }
    }

    pub fn get_rule(&self, id: u64) -> Option<&AlertRule> {
        self.rules.get(&id)
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn enabled_count(&self) -> usize {
        self.rules.values().filter(|r| r.enabled).count()
    }
}

// =====================================================================================
// 告警 (Alert)
// =====================================================================================

/// 告警状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertStatus {
    Firing,    // 触发中
    Resolved,  // 已解决
    Suppressed, // 已抑制
}

/// 告警实例。
#[derive(Clone, Debug)]
pub struct Alert {
    pub id: u64,
    pub rule_id: u64,
    pub rule_name: String,
    pub severity: AlertSeverity,
    pub status: AlertStatus,
    pub started_at: u128,
    pub resolved_at: Option<u128>,
    pub value: f64,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
}

/// 告警管理器：管理告警实例。
pub struct AlertManager {
    pub alerts: BTreeMap<u64, Alert>,
    pub next_id: u64,
    pub suppression_rules: BTreeMap<String, u128>, // 规则名 -> 抑制到的时间
}

impl AlertManager {
    pub fn new() -> Self {
        Self {
            alerts: BTreeMap::new(),
            next_id: 1,
            suppression_rules: BTreeMap::new(),
        }
    }

    pub fn fire(&mut self, rule_id: u64, rule_name: &str, severity: AlertSeverity, value: f64, labels: BTreeMap<String, String>, annotations: BTreeMap<String, String>) -> u64 {
        // 检查是否已有激活告警
        for alert in self.alerts.values() {
            if alert.rule_id == rule_id && alert.status == AlertStatus::Firing {
                return alert.id;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.alerts.insert(id, Alert {
            id,
            rule_id,
            rule_name: rule_name.to_string(),
            severity,
            status: AlertStatus::Firing,
            started_at: crate::ai_os::native::now_ns(),
            resolved_at: None,
            value,
            labels,
            annotations,
        });
        id
    }

    pub fn resolve(&mut self, rule_id: u64) -> bool {
        if let Some(alert) = self.alerts.values_mut().find(|a| a.rule_id == rule_id && a.status == AlertStatus::Firing) {
            alert.status = AlertStatus::Resolved;
            alert.resolved_at = Some(crate::ai_os::native::now_ns());
            true
        } else {
            false
        }
    }

    pub fn suppress(&mut self, rule_name: &str, until_ns: u128) {
        self.suppression_rules.insert(rule_name.to_string(), until_ns);
    }

    pub fn is_suppressed(&self, rule_name: &str, now_ns: u128) -> bool {
        if let Some(until) = self.suppression_rules.get(rule_name) {
            now_ns < *until
        } else {
            false
        }
    }

    pub fn active_alerts(&self) -> Vec<&Alert> {
        self.alerts.values().filter(|a| a.status == AlertStatus::Firing).collect()
    }

    pub fn alert_count(&self) -> usize {
        self.alerts.len()
    }

    pub fn active_count(&self) -> usize {
        self.active_alerts().len()
    }
}

// =====================================================================================
// 通知通道 (NotificationChannel)
// =====================================================================================

/// 通知通道类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationChannelType {
    Email,      // 邮件
    Webhook,    // Webhook
    Slack,      // Slack
    SMS,        // 短信
    PagerDuty,  // PagerDuty
}

/// 通知通道。
#[derive(Clone, Debug)]
pub struct NotificationChannel {
    pub id: u64,
    pub channel_type: NotificationChannelType,
    pub name: String,
    pub config: BTreeMap<String, String>,
    pub enabled: bool,
    pub sent_count: u64,
    pub failed_count: u64,
}

/// 通知通道管理器。
pub struct NotificationChannelManager {
    pub channels: BTreeMap<u64, NotificationChannel>,
    pub next_id: u64,
}

impl NotificationChannelManager {
    pub fn new() -> Self {
        Self {
            channels: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add_channel(&mut self, channel_type: NotificationChannelType, name: &str, config: BTreeMap<String, String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.channels.insert(id, NotificationChannel {
            id,
            channel_type,
            name: name.to_string(),
            config,
            enabled: true,
            sent_count: 0,
            failed_count: 0,
        });
        id
    }

    pub fn send(&mut self, channel_id: u64, message: &str) -> bool {
        if let Some(channel) = self.channels.get_mut(&channel_id) {
            if channel.enabled {
                channel.sent_count += 1;
                true
            } else {
                channel.failed_count += 1;
                false
            }
        } else {
            false
        }
    }

    pub fn get_channel(&self, id: u64) -> Option<&NotificationChannel> {
        self.channels.get(&id)
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn enabled_count(&self) -> usize {
        self.channels.values().filter(|c| c.enabled).count()
    }
}

// =====================================================================================
// 仪表板 (Dashboard)
// =====================================================================================

/// 面板小部件。
#[derive(Clone, Debug)]
pub struct DashboardWidget {
    pub id: u64,
    pub title: String,
    pub widget_type: String,  // graph, stat, table, heatmap
    pub query: String,
    pub refresh_interval_ns: u128,
}

/// 仪表板。
#[derive(Clone, Debug)]
pub struct Dashboard {
    pub id: u64,
    pub name: String,
    pub widgets: Vec<DashboardWidget>,
    pub created_at: u128,
}

/// 仪表板管理器。
pub struct DashboardManager {
    pub dashboards: BTreeMap<u64, Dashboard>,
    pub next_id: u64,
}

impl DashboardManager {
    pub fn new() -> Self {
        Self {
            dashboards: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, name: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.dashboards.insert(id, Dashboard {
            id,
            name: name.to_string(),
            widgets: Vec::new(),
            created_at: crate::ai_os::native::now_ns(),
        });
        id
    }

    pub fn add_widget(&mut self, dashboard_id: u64, title: &str, widget_type: &str, query: &str, refresh_interval_ns: u128) -> u64 {
        if let Some(dashboard) = self.dashboards.get_mut(&dashboard_id) {
            let widget_id = dashboard.widgets.len() as u64 + 1;
            dashboard.widgets.push(DashboardWidget {
                id: widget_id,
                title: title.to_string(),
                widget_type: widget_type.to_string(),
                query: query.to_string(),
                refresh_interval_ns,
            });
            widget_id
        } else {
            0
        }
    }

    pub fn get_dashboard(&self, id: u64) -> Option<&Dashboard> {
        self.dashboards.get(&id)
    }

    pub fn dashboard_count(&self) -> usize {
        self.dashboards.len()
    }
}

// =====================================================================================
// 日志条目 (LogEntry)
// =====================================================================================

/// 日志级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

/// 日志条目。
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub id: u64,
    pub level: LogLevel,
    pub message: String,
    pub source: String,
    pub timestamp_ns: u128,
    pub fields: BTreeMap<String, String>,
}

/// 日志采集器：采集和存储日志。
pub struct LogCollector {
    pub entries: VecDeque<LogEntry>,
    pub max_entries: usize,
    pub total_collected: u64,
}

impl LogCollector {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries: 100000,
            total_collected: 0,
        }
    }

    pub fn collect(&mut self, level: LogLevel, message: &str, source: &str, fields: BTreeMap<String, String>) -> u64 {
        let id = self.next_id();
        self.entries.push_back(LogEntry {
            id,
            level,
            message: message.to_string(),
            source: source.to_string(),
            timestamp_ns: crate::ai_os::native::now_ns(),
            fields,
        });
        if self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }
        self.total_collected += 1;
        id
    }

    fn next_id(&mut self) -> u64 {
        self.entries.iter().map(|e| e.id).max().unwrap_or(0) + 1
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<&LogEntry> {
        self.entries.iter()
            .filter(|e| e.message.contains(query) || e.source.contains(query))
            .take(limit)
            .collect()
    }

    pub fn count_by_level(&self, level: LogLevel) -> usize {
        self.entries.iter().filter(|e| e.level == level).count()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

// =====================================================================================
// 追踪跨度 (Span)
// =====================================================================================

/// 跨度类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Server,   // 服务端
    Client,   // 客户端
    Producer, // 生产者
    Consumer, // 消费者
    Internal, // 内部
}

/// 追踪跨度：分布式追踪中的单个跨度。
#[derive(Clone, Debug)]
pub struct Span {
    pub id: u64,
    pub trace_id: u64,
    pub parent_id: Option<u64>,
    pub name: String,
    pub kind: SpanKind,
    pub start_time_ns: u128,
    pub end_time_ns: Option<u128>,
    pub attributes: BTreeMap<String, String>,
    pub events: Vec<(u128, String)>, // (timestamp, message)
}

/// 追踪跨度收集器。
pub struct SpanCollector {
    pub spans: BTreeMap<u64, Span>,
    pub next_id: u64,
}

impl SpanCollector {
    pub fn new() -> Self {
        Self {
            spans: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn start_span(&mut self, trace_id: u64, parent_id: Option<u64>, name: &str, kind: SpanKind) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.spans.insert(id, Span {
            id,
            trace_id,
            parent_id,
            name: name.to_string(),
            kind,
            start_time_ns: crate::ai_os::native::now_ns(),
            end_time_ns: None,
            attributes: BTreeMap::new(),
            events: Vec::new(),
        });
        id
    }

    pub fn end_span(&mut self, id: u64) -> bool {
        if let Some(span) = self.spans.get_mut(&id) {
            span.end_time_ns = Some(crate::ai_os::native::now_ns());
            true
        } else {
            false
        }
    }

    pub fn add_attribute(&mut self, id: u64, key: &str, value: &str) -> bool {
        if let Some(span) = self.spans.get_mut(&id) {
            span.attributes.insert(key.to_string(), value.to_string());
            true
        } else {
            false
        }
    }

    pub fn add_event(&mut self, id: u64, message: &str) -> bool {
        if let Some(span) = self.spans.get_mut(&id) {
            span.events.push((crate::ai_os::native::now_ns(), message.to_string()));
            true
        } else {
            false
        }
    }

    pub fn get_span(&self, id: u64) -> Option<&Span> {
        self.spans.get(&id)
    }

    pub fn span_count(&self) -> usize {
        self.spans.len()
    }

    pub fn trace_count(&self) -> usize {
        self.spans.values().map(|s| s.trace_id).collect::<BTreeSet<_>>().len()
    }
}

// =====================================================================================
// SLO 监控 (SLOMonitor)
// =====================================================================================

/// SLO 目标。
#[derive(Clone, Debug)]
pub struct SLOTarget {
    pub name: String,
    pub metric_name: String,
    pub target_value: f64,      // 目标值
    pub window_ns: u128,        // 评估窗口
    pub comparison: String,     // 比较方式，如 ">=", "<="
}

/// SLO 监控器：监控服务等级目标。
pub struct SLOMonitor {
    pub targets: BTreeMap<String, SLOTarget>,
    pub evaluations: Vec<(u64, bool, f64, u128)>, // (target_id, passed, value, timestamp)
}

impl SLOMonitor {
    pub fn new() -> Self {
        Self {
            targets: BTreeMap::new(),
            evaluations: Vec::new(),
        }
    }

    pub fn add_target(&mut self, name: &str, metric_name: &str, target_value: f64, window_ns: u128, comparison: &str) -> u64 {
        let id = self.next_id();
        self.targets.insert(name.to_string(), SLOTarget {
            name: name.to_string(),
            metric_name: metric_name.to_string(),
            target_value,
            window_ns,
            comparison: comparison.to_string(),
        });
        id
    }

    fn next_id(&mut self) -> u64 {
        self.targets.len() as u64 + 1
    }

    pub fn evaluate(&mut self, target_name: &str, current_value: f64, timestamp_ns: u128) -> bool {
        if let Some(target) = self.targets.get(target_name) {
            let passed = match target.comparison.as_str() {
                ">=" => current_value >= target.target_value,
                "<=" => current_value <= target.target_value,
                ">" => current_value > target.target_value,
                "<" => current_value < target.target_value,
                _ => false,
            };
            self.evaluations.push((self.targets.get(target_name).map(|_| 0).unwrap_or(0), passed, current_value, timestamp_ns));
            passed
        } else {
            false
        }
    }

    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    pub fn evaluation_count(&self) -> usize {
        self.evaluations.len()
    }

    pub fn sli(&self, target_name: &str) -> Option<f64> {
        let evaluations: Vec<&(u64, bool, f64, u128)> = self.evaluations.iter()
            .filter(|(tid, _, _, _)| {
                self.targets.iter().any(|(name, t)| t.name == target_name && *tid == self.targets.iter().position(|(n, _)| n == target_name).unwrap_or(0) as u64)
            })
            .collect();
        if evaluations.is_empty() {
            None
        } else {
            let passed = evaluations.iter().filter(|(_, passed, _, _)| *passed).count();
            Some(passed as f64 / evaluations.len() as f64)
        }
    }
}

// =====================================================================================
// 指标聚合器 (MetricAggregator)
// =====================================================================================

/// 聚合函数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregationFunction {
    Avg,    // 平均
    Sum,    // 求和
    Min,    // 最小
    Max,    // 最大
    Count,  // 计数
    P50,    // 第 50 百分位
    P95,    // 第 95 百分位
    P99,    // 第 99 百分位
}

/// 聚合规则。
#[derive(Clone, Debug)]
pub struct AggregationRule {
    pub id: u64,
    pub source_metric: String,
    pub target_metric: String,
    pub function: AggregationFunction,
    pub window_ns: u128,
    pub enabled: bool,
}

/// 指标聚合器：聚合和下采样指标。
pub struct MetricAggregator {
    pub rules: BTreeMap<u64, AggregationRule>,
    pub next_id: u64,
    pub aggregated_values: BTreeMap<u64, Vec<(u128, f64)>>, // rule_id -> (timestamp, value)
}

impl MetricAggregator {
    pub fn new() -> Self {
        Self {
            rules: BTreeMap::new(),
            next_id: 1,
            aggregated_values: BTreeMap::new(),
        }
    }

    pub fn add_rule(&mut self, source_metric: &str, target_metric: &str, function: AggregationFunction, window_ns: u128) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.rules.insert(id, AggregationRule {
            id,
            source_metric: source_metric.to_string(),
            target_metric: target_metric.to_string(),
            function,
            window_ns,
            enabled: true,
        });
        id
    }

    pub fn aggregate(&mut self, rule_id: u64, values: &[f64], timestamp_ns: u128) -> Option<f64> {
        if let Some(rule) = self.rules.get(&rule_id) {
            if !rule.enabled {
                return None;
            }
            let result = match rule.function {
                AggregationFunction::Avg => values.iter().sum::<f64>() / values.len() as f64,
                AggregationFunction::Sum => values.iter().sum(),
                AggregationFunction::Min => values.iter().fold(f64::INFINITY, |a, &b| a.min(b)),
                AggregationFunction::Max => values.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b)),
                AggregationFunction::Count => values.len() as f64,
                AggregationFunction::P50 => {
                    let mut sorted: Vec<f64> = values.to_vec();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    sorted[sorted.len() * 50 / 100]
                }
                AggregationFunction::P95 => {
                    let mut sorted: Vec<f64> = values.to_vec();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    sorted[sorted.len() * 95 / 100]
                }
                AggregationFunction::P99 => {
                    let mut sorted: Vec<f64> = values.to_vec();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    sorted[sorted.len() * 99 / 100]
                }
            };
            self.aggregated_values.entry(rule_id).or_default().push((timestamp_ns, result));
            Some(result)
        } else {
            None
        }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

// =====================================================================================
// 监控引擎 (MonitoringEngine) —— 统一入口
// =====================================================================================

/// 监控引擎：整合指标采集、指标注册、告警规则、告警管理、通知通道、仪表板、日志采集、追踪跨度、SLO 监控、指标聚合。
pub struct MonitoringEngine {
    pub collector: MetricCollector,
    pub registry: MetricRegistry,
    pub alert_engine: AlertRuleEngine,
    pub alert_manager: AlertManager,
    pub notification_manager: NotificationChannelManager,
    pub dashboard_manager: DashboardManager,
    pub log_collector: LogCollector,
    pub span_collector: SpanCollector,
    pub slo_monitor: SLOMonitor,
    pub aggregator: MetricAggregator,
    pub enabled: bool,
    pub total_operations: u64,
}

impl MonitoringEngine {
    pub fn new() -> Self {
        Self {
            collector: MetricCollector::new(),
            registry: MetricRegistry::new(),
            alert_engine: AlertRuleEngine::new(),
            alert_manager: AlertManager::new(),
            notification_manager: NotificationChannelManager::new(),
            dashboard_manager: DashboardManager::new(),
            log_collector: LogCollector::new(),
            span_collector: SpanCollector::new(),
            slo_monitor: SLOMonitor::new(),
            aggregator: MetricAggregator::new(),
            enabled: true,
            total_operations: 0,
        }
    }

    pub fn register_metric(&mut self, name: &str, r#type: MetricType, unit: &str, description: &str) -> u64 {
        let id = self.registry.register(name, r#type, unit, description);
        self.total_operations += 1;
        id
    }

    pub fn collect_metric(&mut self, metric_id: u64, value: f64, labels: BTreeMap<String, String>) {
        self.collector.collect(metric_id, value, labels);
        self.total_operations += 1;
    }

    pub fn create_alert_rule(&mut self, name: &str, metric_name: &str, condition: &str, threshold: f64, severity: AlertSeverity, for_duration_ns: u128) -> u64 {
        let id = self.alert_engine.create_rule(name, metric_name, condition, threshold, severity, for_duration_ns);
        self.total_operations += 1;
        id
    }

    pub fn evaluate_alert(&mut self, rule_id: u64, value: f64) -> bool {
        let result = self.alert_engine.evaluate(rule_id, value);
        if result {
            if let Some(rule) = self.alert_engine.get_rule(rule_id) {
                let mut labels = BTreeMap::new();
                labels.insert("severity".to_string(), format!("{:?}", rule.severity));
                let mut annotations = BTreeMap::new();
                annotations.insert("summary".to_string(), format!("{}: {} {}", rule.name, rule.metric_name, rule.condition));
                self.alert_manager.fire(rule_id, &rule.name, rule.severity, value, labels, annotations);
            }
        }
        self.total_operations += 1;
        result
    }

    pub fn add_notification_channel(&mut self, channel_type: NotificationChannelType, name: &str, config: BTreeMap<String, String>) -> u64 {
        let id = self.notification_manager.add_channel(channel_type, name, config);
        self.total_operations += 1;
        id
    }

    pub fn send_notification(&mut self, channel_id: u64, message: &str) -> bool {
        let result = self.notification_manager.send(channel_id, message);
        if result {
            self.total_operations += 1;
        }
        result
    }

    pub fn create_dashboard(&mut self, name: &str) -> u64 {
        let id = self.dashboard_manager.create(name);
        self.total_operations += 1;
        id
    }

    pub fn add_dashboard_widget(&mut self, dashboard_id: u64, title: &str, widget_type: &str, query: &str, refresh_interval_ns: u128) -> u64 {
        let id = self.dashboard_manager.add_widget(dashboard_id, title, widget_type, query, refresh_interval_ns);
        if id > 0 {
            self.total_operations += 1;
        }
        id
    }

    pub fn collect_log(&mut self, level: LogLevel, message: &str, source: &str, fields: BTreeMap<String, String>) -> u64 {
        let id = self.log_collector.collect(level, message, source, fields);
        self.total_operations += 1;
        id
    }

    pub fn start_span(&mut self, trace_id: u64, parent_id: Option<u64>, name: &str, kind: SpanKind) -> u64 {
        let id = self.span_collector.start_span(trace_id, parent_id, name, kind);
        self.total_operations += 1;
        id
    }

    pub fn end_span(&mut self, id: u64) -> bool {
        let result = self.span_collector.end_span(id);
        if result {
            self.total_operations += 1;
        }
        result
    }

    pub fn add_slo_target(&mut self, name: &str, metric_name: &str, target_value: f64, window_ns: u128, comparison: &str) -> u64 {
        let id = self.slo_monitor.add_target(name, metric_name, target_value, window_ns, comparison);
        self.total_operations += 1;
        id
    }

    pub fn evaluate_slo(&mut self, target_name: &str, value: f64, timestamp_ns: u128) -> bool {
        let result = self.slo_monitor.evaluate(target_name, value, timestamp_ns);
        self.total_operations += 1;
        result
    }

    pub fn add_aggregation_rule(&mut self, source_metric: &str, target_metric: &str, function: AggregationFunction, window_ns: u128) -> u64 {
        let id = self.aggregator.add_rule(source_metric, target_metric, function, window_ns);
        self.total_operations += 1;
        id
    }

    pub fn aggregate(&mut self, rule_id: u64, values: &[f64], timestamp_ns: u128) -> Option<f64> {
        let result = self.aggregator.aggregate(rule_id, values, timestamp_ns);
        if result.is_some() {
            self.total_operations += 1;
        }
        result
    }

    pub fn tick(&mut self) {
        // 周期性维护
        let _ = self.collector.value_count();
        let _ = self.alert_manager.active_count();
        let _ = self.log_collector.entry_count();
        let _ = self.span_collector.span_count();
    }

    pub fn stats(&self) -> String {
        format!(
            "monitoring: metrics={} values={} rules={} alerts={} active_alerts={} channels={} dashboards={} widgets={} logs={} spans={} slo_targets={} agg_rules={} total_ops={}",
            self.registry.metric_count(),
            self.collector.value_count(),
            self.alert_engine.rule_count(),
            self.alert_manager.alert_count(),
            self.alert_manager.active_count(),
            self.notification_manager.channel_count(),
            self.dashboard_manager.dashboard_count(),
            self.dashboard_manager.dashboards.values().map(|d| d.widgets.len()).sum::<usize>(),
            self.log_collector.entry_count(),
            self.span_collector.span_count(),
            self.slo_monitor.target_count(),
            self.aggregator.rule_count(),
            self.total_operations
        )
    }
}

impl Default for MonitoringEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static MONITORING: OnceLock<Mutex<MonitoringEngine>> = OnceLock::new();

pub fn monitoring_engine() -> &'static Mutex<MonitoringEngine> {
    MONITORING.get_or_init(|| Mutex::new(MonitoringEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = monitoring_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = monitoring_engine().lock().unwrap();
    eng.stats()
}

pub fn monitoring_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn monitoring_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("register-metric") => {
            let name = it.next().ok_or("缺少指标名")?;
            let type_str = it.next().unwrap_or("gauge");
            let r#type = match type_str {
                "counter" => MetricType::Counter,
                "gauge" => MetricType::Gauge,
                "histogram" => MetricType::Histogram,
                "summary" => MetricType::Summary,
                _ => MetricType::Gauge,
            };
            let unit = it.next().unwrap_or("");
            let description: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = monitoring_engine().lock().unwrap();
            let id = eng.register_metric(name, r#type, unit, &description);
            Ok(format!("已注册指标 id={} 名称={} 类型={:?}", id, name, r#type))
        }
        Some("collect") => {
            let metric_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少指标id")?;
            let value = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let mut eng = monitoring_engine().lock().unwrap();
            let mut labels = BTreeMap::new();
            eng.collect_metric(metric_id, value, labels);
            Ok(format!("已采集指标 id={} 值={}", metric_id, value))
        }
        Some("create-alert-rule") => {
            let name = it.next().ok_or("缺少规则名")?;
            let metric_name = it.next().ok_or("缺少指标名")?;
            let condition = it.next().unwrap_or(">");
            let threshold = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let severity_str = it.next().unwrap_or("warning");
            let severity = match severity_str {
                "info" => AlertSeverity::Info,
                "warning" => AlertSeverity::Warning,
                "critical" => AlertSeverity::Critical,
                "emergency" => AlertSeverity::Emergency,
                _ => AlertSeverity::Warning,
            };
            let for_duration = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = monitoring_engine().lock().unwrap();
            let id = eng.create_alert_rule(name, metric_name, condition, threshold, severity, for_duration);
            Ok(format!("已创建告警规则 id={} 名称={}", id, name))
        }
        Some("evaluate-alert") => {
            let rule_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少规则id")?;
            let value = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let mut eng = monitoring_engine().lock().unwrap();
            let fired = eng.evaluate_alert(rule_id, value);
            Ok(format!("告警评估 规则={} 触发={}", rule_id, fired))
        }
        Some("add-channel") => {
            let type_str = it.next().unwrap_or("webhook");
            let channel_type = match type_str {
                "email" => NotificationChannelType::Email,
                "webhook" => NotificationChannelType::Webhook,
                "slack" => NotificationChannelType::Slack,
                "sms" => NotificationChannelType::SMS,
                "pagerduty" => NotificationChannelType::PagerDuty,
                _ => NotificationChannelType::Webhook,
            };
            let name = it.next().ok_or("缺少通道名")?;
            let mut eng = monitoring_engine().lock().unwrap();
            let mut config = BTreeMap::new();
            config.insert("endpoint".to_string(), it.next().unwrap_or("").to_string());
            let id = eng.add_notification_channel(channel_type, name, config);
            Ok(format!("已添加通知通道 id={} 名称={}", id, name))
        }
        Some("send-notification") => {
            let channel_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let message: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = monitoring_engine().lock().unwrap();
            if eng.send_notification(channel_id, &message) {
                Ok(format!("已发送通知 通道={}", channel_id))
            } else {
                Err("通道未启用或不存在".into())
            }
        }
        Some("create-dashboard") => {
            let name = it.next().ok_or("缺少仪表板名")?;
            let mut eng = monitoring_engine().lock().unwrap();
            let id = eng.create_dashboard(name);
            Ok(format!("已创建仪表板 id={} 名称={}", id, name))
        }
        Some("add-widget") => {
            let dashboard_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少仪表板id")?;
            let title = it.next().ok_or("缺少部件名")?;
            let widget_type = it.next().unwrap_or("graph");
            let rest: Vec<&str> = it.collect();
            let query: String = rest.join(" ");
            let refresh = rest.get(0).and_then(|s| s.parse::<u128>().ok()).unwrap_or(60_000_000_000);
            let mut eng = monitoring_engine().lock().unwrap();
            let id = eng.add_dashboard_widget(dashboard_id, title, widget_type, &query, refresh);
            if id > 0 {
                Ok(format!("已添加面板部件 id={} 仪表板={}", id, dashboard_id))
            } else {
                Err("仪表板不存在".into())
            }
        }
        Some("collect-log") => {
            let level_str = it.next().unwrap_or("info");
            let level = match level_str {
                "debug" => LogLevel::Debug,
                "info" => LogLevel::Info,
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                "fatal" => LogLevel::Fatal,
                _ => LogLevel::Info,
            };
            let message: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = monitoring_engine().lock().unwrap();
            let mut fields = BTreeMap::new();
            let id = eng.collect_log(level, &message, "shell", fields);
            Ok(format!("已采集日志 id={} 级别={:?}", id, level))
        }
        Some("start-span") => {
            let trace_id = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
            let parent_id = it.next().and_then(|s| s.parse::<u64>().ok());
            let name: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = monitoring_engine().lock().unwrap();
            let id = eng.start_span(trace_id, parent_id, &name, SpanKind::Internal);
            Ok(format!("已启动追踪跨度 id={} 跟踪id={}", id, trace_id))
        }
        Some("end-span") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少跨度id")?;
            let mut eng = monitoring_engine().lock().unwrap();
            if eng.end_span(id) {
                Ok(format!("已结束追踪跨度 id={}", id))
            } else {
                Err("跨度不存在".into())
            }
        }
        Some("add-slo") => {
            let name = it.next().ok_or("缺少目标名")?;
            let metric_name = it.next().ok_or("缺少指标名")?;
            let target_value = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.99);
            let window_ns = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(86_400_000_000_000);
            let comparison = it.next().unwrap_or(">=");
            let mut eng = monitoring_engine().lock().unwrap();
            let id = eng.add_slo_target(name, metric_name, target_value, window_ns, comparison);
            Ok(format!("已添加 SLO 目标 id={} 名称={}", id, name))
        }
        Some("evaluate-slo") => {
            let target_name = it.next().ok_or("缺少目标名")?;
            let value = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let timestamp = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = monitoring_engine().lock().unwrap();
            let passed = eng.evaluate_slo(target_name, value, timestamp);
            Ok(format!("SLO 评估 目标={} 通过={}", target_name, passed))
        }
        Some("add-agg-rule") => {
            let source = it.next().ok_or("缺少源指标名")?;
            let target = it.next().ok_or("缺少目标指标名")?;
            let func_str = it.next().unwrap_or("avg");
            let function = match func_str {
                "avg" => AggregationFunction::Avg,
                "sum" => AggregationFunction::Sum,
                "min" => AggregationFunction::Min,
                "max" => AggregationFunction::Max,
                "count" => AggregationFunction::Count,
                "p50" => AggregationFunction::P50,
                "p95" => AggregationFunction::P95,
                "p99" => AggregationFunction::P99,
                _ => AggregationFunction::Avg,
            };
            let window = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(60_000_000_000);
            let mut eng = monitoring_engine().lock().unwrap();
            let id = eng.add_aggregation_rule(source, target, function, window);
            Ok(format!("已添加聚合规则 id={} 源={} 目标={}", id, source, target))
        }
        Some("aggregate") => {
            let rule_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少规则id")?;
            let rest: Vec<&str> = it.collect();
            let values: Vec<f64> = rest.iter().filter_map(|s| s.parse::<f64>().ok()).collect();
            if values.is_empty() {
                return Err("缺少数值".into());
            }
            let timestamp = rest.get(0).and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = monitoring_engine().lock().unwrap();
            match eng.aggregate(rule_id, &values, timestamp) {
                Some(result) => Ok(format!("聚合结果 规则={} 值={}", rule_id, result)),
                None => Err("规则不存在或未启用".into()),
            }
        }
        Some("tick") => {
            let mut eng = monitoring_engine().lock().unwrap();
            eng.tick();
            Ok("监控引擎已推进".into())
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 monitoring 动作: {}", other)),
    }
}

pub fn monitoring_act_dispatch(payload: &str) -> Result<String, String> {
    monitoring_act(payload)
}

// =====================================================================================
// 监控扩展测试
// =====================================================================================

#[cfg(test)]
mod monitoring_tests {
    use super::*;

    #[test]
    fn metric_registry() {
        let mut reg = MetricRegistry::new();
        let id = reg.register("cpu_usage", MetricType::Gauge, "percent", "CPU 使用率");
        assert_eq!(id, 1);
        let def = reg.get("cpu_usage").unwrap();
        assert_eq!(def.name, "cpu_usage");
        assert_eq!(def.r#type, MetricType::Gauge);
        reg.add_label("cpu_usage", "instance");
        let def = reg.get("cpu_usage").unwrap();
        assert!(def.labels.contains(&"instance".to_string()));
        assert_eq!(reg.metric_count(), 1);
    }

    #[test]
    fn metric_collector() {
        let mut collector = MetricCollector::new();
        let mut labels = BTreeMap::new();
        labels.insert("instance".to_string(), "node1".to_string());
        collector.collect(1, 50.5, labels.clone());
        collector.collect(1, 60.0, labels);
        assert_eq!(collector.value_count(), 2);
        let latest = collector.get_latest(1).unwrap();
        assert!((latest.value - 60.0).abs() < 1e-6);
        let values = collector.get_values(1, 5);
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn alert_rule() {
        let mut engine = AlertRuleEngine::new();
        let id = engine.create_rule("high_cpu", "cpu_usage", ">", 90.0, AlertSeverity::Critical, 60_000_000_000);
        assert_eq!(id, 1);
        assert!(engine.evaluate(1, 95.0));
        assert!(!engine.evaluate(1, 50.0));
        assert_eq!(engine.rule_count(), 1);
        assert_eq!(engine.enabled_count(), 1);
    }

    #[test]
    fn alert_manager() {
        let mut am = AlertManager::new();
        let mut labels = BTreeMap::new();
        labels.insert("instance".to_string(), "node1".to_string());
        let mut annotations = BTreeMap::new();
        annotations.insert("summary".to_string(), "High CPU".to_string());
        let id = am.fire(1, "high_cpu", AlertSeverity::Critical, 95.0, labels, annotations);
        assert_eq!(id, 1);
        assert_eq!(am.alert_count(), 1);
        assert_eq!(am.active_count(), 1);
        am.resolve(1);
        assert_eq!(am.active_count(), 0);
        am.suppress("high_cpu", 999999999999);
        assert!(am.is_suppressed("high_cpu", 100));
    }

    #[test]
    fn notification_channel() {
        let mut mgr = NotificationChannelManager::new();
        let id = mgr.add_channel(NotificationChannelType::Email, "ops", BTreeMap::new());
        assert_eq!(id, 1);
        assert!(mgr.send(1, "alert fired"));
        assert_eq!(mgr.channel_count(), 1);
        assert_eq!(mgr.enabled_count(), 1);
    }

    #[test]
    fn dashboard() {
        let mut dm = DashboardManager::new();
        let id = dm.create("system_overview".into());
        assert_eq!(id, 1);
        let wid = dm.add_widget(id, "cpu_graph".into(), "graph".into(), "cpu_usage".into(), 60_000_000_000);
        assert_eq!(wid, 1);
        let dash = dm.get_dashboard(id).unwrap();
        assert_eq!(dash.widgets.len(), 1);
        assert_eq!(dm.dashboard_count(), 1);
    }

    #[test]
    fn log_collector() {
        let mut lc = LogCollector::new();
        let id = lc.collect(LogLevel::Error, "disk full", "storage", BTreeMap::new());
        assert_eq!(id, 1);
        assert_eq!(lc.entry_count(), 1);
        assert_eq!(lc.count_by_level(LogLevel::Error), 1);
        let results = lc.search("disk", 5);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn span_collector() {
        let mut sc = SpanCollector::new();
        let id = sc.start_span(1, None, "http_request".into(), SpanKind::Server);
        assert_eq!(id, 1);
        sc.add_attribute(id, "http.method".into(), "GET".into());
        sc.add_event(id, "request received".into());
        sc.end_span(id).unwrap();
        let span = sc.get_span(id).unwrap();
        assert_eq!(span.name, "http_request");
        assert_eq!(sc.span_count(), 1);
        assert_eq!(sc.trace_count(), 1);
    }

    #[test]
    fn slo_monitor() {
        let mut sm = SLOMonitor::new();
        let id = sm.add_target("availability", "uptime", 99.9, 86_400_000_000_000, ">=");
        assert_eq!(id, 1);
        assert!(sm.evaluate("availability", 99.95, 0));
        assert!(!sm.evaluate("availability", 99.8, 0));
        assert_eq!(sm.target_count(), 1);
        assert_eq!(sm.evaluation_count(), 2);
        let sli = sm.sli("availability");
        assert!(sli.is_some());
        assert!((sli.unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn metric_aggregator() {
        let mut agg = MetricAggregator::new();
        let id = agg.add_rule("cpu_usage".into(), "cpu_usage_avg".into(), AggregationFunction::Avg, 60_000_000_000);
        assert_eq!(id, 1);
        let values = vec![50.0, 60.0, 70.0, 80.0, 90.0];
        let result = agg.aggregate(id, &values, 0).unwrap();
        assert!((result - 60.0).abs() < 1e-6);
        assert_eq!(agg.rule_count(), 1);
    }

    #[test]
    fn monitoring_engine() {
        let mut eng = MonitoringEngine::new();
        let mid = eng.register_metric("cpu_usage", MetricType::Gauge, "percent", "CPU 使用率");
        assert_eq!(mid, 1);
        eng.collect_metric(mid, 75.0, BTreeMap::new());
        let rid = eng.create_alert_rule("high_cpu", "cpu_usage", ">", 90.0, AlertSeverity::Critical, 0);
        assert_eq!(rid, 1);
        eng.evaluate_alert(rid, 95.0);
        let cid = eng.add_notification_channel(NotificationChannelType::Email, "ops", BTreeMap::new());
        assert_eq!(cid, 1);
        eng.send_notification(cid, "alert fired");
        let did = eng.create_dashboard("overview".into());
        assert_eq!(did, 1);
        eng.add_dashboard_widget(did, "cpu".into(), "graph".into(), "cpu_usage".into(), 60_000_000_000);
        eng.collect_log(LogLevel::Info, "system started".into(), "kernel".into(), BTreeMap::new());
        let sid = eng.start_span(1, None, "request".into(), SpanKind::Server);
        eng.end_span(sid).unwrap();
        let slo_id = eng.add_slo_target("availability".into(), "uptime".into(), 99.9, 86_400_000_000_000, ">=");
        eng.evaluate_slo("availability".into(), 99.95, 0);
        let agg_id = eng.add_aggregation_rule("cpu_usage".into(), "cpu_avg".into(), AggregationFunction::Avg, 60_000_000_000);
        eng.aggregate(agg_id, &[50.0, 60.0, 70.0], 0).unwrap();
        eng.tick();
        let s = eng.stats();
        assert!(s.contains("monitoring:"));
    }

    #[test]
    fn monitoring_act_register() {
        init();
        let res = monitoring_act("register-metric cpu_usage gauge percent CPU 使用率").unwrap();
        assert!(res.contains("已注册指标"));
    }

    #[test]
    fn monitoring_act_collect() {
        init();
        monitoring_act("register-metric cpu_usage gauge percent CPU 使用率").unwrap();
        let res = monitoring_act("collect 1 75.5").unwrap();
        assert!(res.contains("已采集指标"));
    }

    #[test]
    fn monitoring_act_alert() {
        init();
        monitoring_act("create-alert-rule high_cpu cpu_usage > 90.0 critical 60000000000").unwrap();
        let res = monitoring_act("evaluate-alert 1 95.0").unwrap();
        assert!(res.contains("告警评估"));
    }

    #[test]
    fn monitoring_act_channel() {
        init();
        let res = monitoring_act("add-channel webhook ops http://webhook.example.com").unwrap();
        assert!(res.contains("已添加通知通道"));
        let res = monitoring_act("send-notification 1 alert fired").unwrap();
        assert!(res.contains("已发送通知"));
    }

    #[test]
    fn monitoring_act_dashboard() {
        init();
        let res = monitoring_act("create-dashboard overview").unwrap();
        assert!(res.contains("已创建仪表板"));
        let res = monitoring_act("add-widget 1 cpu graph cpu_usage 60000000000").unwrap();
        assert!(res.contains("已添加面板部件"));
    }

    #[test]
    fn monitoring_act_log() {
        init();
        let res = monitoring_act("collect-log error disk full storage").unwrap();
        assert!(res.contains("已采集日志"));
    }

    #[test]
    fn monitoring_act_span() {
        init();
        let res = monitoring_act("start-span 1 none http_request").unwrap();
        assert!(res.contains("已启动追踪跨度"));
        // 提取跨度id并结束
        let id = 1; // 第一个跨度id
        let res = monitoring_act(&format!("end-span {}", id)).unwrap();
        assert!(res.contains("已结束追踪跨度"));
    }

    #[test]
    fn monitoring_act_slo() {
        init();
        let res = monitoring_act("add-slo availability uptime 99.9 86400000000000 >=").unwrap();
        assert!(res.contains("已添加 SLO 目标"));
        let res = monitoring_act("evaluate-slo availability 99.95 0").unwrap();
        assert!(res.contains("SLO 评估"));
    }

    #[test]
    fn monitoring_act_agg() {
        init();
        monitoring_act("add-agg-rule cpu_usage cpu_avg avg 60000000000").unwrap();
        let res = monitoring_act("aggregate 1 50 60 70 80 90 0").unwrap();
        assert!(res.contains("聚合结果"));
    }

    #[test]
    fn monitoring_act_tick() {
        init();
        let res = monitoring_act("tick").unwrap();
        assert!(res.contains("监控引擎已推进"));
    }

    #[test]
    fn monitoring_act_status() {
        init();
        let res = monitoring_act("status").unwrap();
        assert!(res.contains("monitoring:"));
    }

    #[test]
    fn log_levels() {
        assert_eq!(LogLevel::Debug as u32, 0);
        assert_eq!(LogLevel::Fatal as u32, 4);
    }

    #[test]
    fn span_kinds() {
        assert_eq!(SpanKind::Server as u32, 0);
        assert_eq!(SpanKind::Internal as u32, 4);
    }

    #[test]
    fn alert_severities() {
        assert_eq!(AlertSeverity::Info as u32, 0);
        assert_eq!(AlertSeverity::Emergency as u32, 3);
    }

    #[test]
    fn agg_functions() {
        assert_eq!(AggregationFunction::Avg as u32, 0);
        assert_eq!(AggregationFunction::P99 as u32, 7);
    }

    #[test]
    fn notification_channel_types() {
        assert_eq!(NotificationChannelType::Email as u32, 0);
        assert_eq!(NotificationChannelType::PagerDuty as u32, 4);
    }
}
