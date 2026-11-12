//! 动作空间层 —— AI 原生 OS 的「执行系统」 (Action Space)
//!
//! 本模块实现 AI 原生 OS 的动作执行，对应传统 OS 中的「系统调用 /
//! 设备驱动 / I/O 调度」概念，但重新映射到动作域：
//!
//! * ActionPrimitive (动作原语) —— 最小可执行动作单元，等价于系统调用；
//! * ActionSpace (动作空间) —— 所有可执行动作的集合，等价于系统调用表；
//! * MotorCommand (运动命令) —— 发送给执行器的命令，等价于 I/O 请求；
//! * MotorExecutor (运动执行器) —— 动作原语的实际执行，等价于设备驱动；
//! * ActionSequence (动作序列) —— 多个动作的有序组合，等价于脚本 / 事务；
//! * ActionScheduler (动作调度器) —— 动作的时序与并发调度，等价于 I/O 调度器；
//! * ActionFeedback (动作反馈) —— 动作执行后的状态反馈，等价于中断返回值；
//! * ActionError (动作错误) —— 动作执行失败的错误码，等价于 errno；
//! * ActionConstraint (动作约束) —— 动作的前置条件与后置条件，等价于文件锁；
//! * ActionEngine (动作引擎) —— 统一入口，通过 action://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无硬件依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 动作原语 (ActionPrimitive)
// =====================================================================================

/// 动作原语类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActionPrimitiveType {
    Move,           // 移动
    Grasp,          // 抓取
    Release,        // 释放
    Look,           // 观察
    Listen,         // 倾听
    Speak,          // 说话
    Manipulate,     // 操作
    Navigate,       // 导航
    Compute,        // 计算
    Communicate,    // 通信
    Rest,           // 休息
    Custom,         // 自定义
}

/// 动作原语：最小可执行动作单元。
#[derive(Clone, Debug)]
pub struct ActionPrimitive {
    pub id: u64,
    pub r#type: ActionPrimitiveType,
    pub name: String,
    pub parameters: BTreeMap<String, String>,
    pub preconditions: Vec<String>,   // 前置条件
    pub postconditions: Vec<String>,  // 后置条件
    pub estimated_duration_ns: u128,  // 预估执行时间
    pub energy_cost: f32,             // 能量代价 0~1
    pub success_probability: f32,     // 成功概率 0~1
    pub enabled: bool,
}

/// 动作原语注册表：动作空间的目录。
pub struct ActionPrimitiveRegistry {
    primitives: BTreeMap<u64, ActionPrimitive>,
    name_index: BTreeMap<String, u64>,
    next_id: u64,
}

impl ActionPrimitiveRegistry {
    pub fn new() -> Self {
        Self {
            primitives: BTreeMap::new(),
            name_index: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn register(
        &mut self,
        r#type: ActionPrimitiveType,
        name: &str,
        parameters: &[(&str, &str)],
        preconditions: &[&str],
        postconditions: &[&str],
        estimated_duration_ns: u128,
        energy_cost: f32,
        success_probability: f32,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let params: BTreeMap<String, String> = parameters.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let primitive = ActionPrimitive {
            id,
            r#type,
            name: name.to_string(),
            parameters: params,
            preconditions: preconditions.iter().map(|s| s.to_string()).collect(),
            postconditions: postconditions.iter().map(|s| s.to_string()).collect(),
            estimated_duration_ns,
            energy_cost: energy_cost.clamp(0.0, 1.0),
            success_probability: success_probability.clamp(0.0, 1.0),
            enabled: true,
        };
        self.name_index.insert(name.to_string(), id);
        self.primitives.insert(id, primitive);
        id
    }

    pub fn get(&self, id: u64) -> Option<&ActionPrimitive> {
        self.primitives.get(&id)
    }

    pub fn get_by_name(&self, name: &str) -> Option<&ActionPrimitive> {
        self.name_index.get(name).and_then(|id| self.primitives.get(id))
    }

    pub fn enable(&mut self, id: u64) -> bool {
        if let Some(p) = self.primitives.get_mut(&id) {
            p.enabled = true;
            true
        } else {
            false
        }
    }

    pub fn disable(&mut self, id: u64) -> bool {
        if let Some(p) = self.primitives.get_mut(&id) {
            p.enabled = false;
            true
        } else {
            false
        }
    }

    pub fn count(&self) -> usize {
        self.primitives.len()
    }

    pub fn enabled_count(&self) -> usize {
        self.primitives.values().filter(|p| p.enabled).count()
    }
}

// =====================================================================================
// 运动命令 (MotorCommand)
// =====================================================================================

/// 运动命令状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotorCommandStatus {
    Pending,
    Executing,
    Completed,
    Failed,
    Cancelled,
}

/// 运动命令：发送给执行器的命令。
#[derive(Clone, Debug)]
pub struct MotorCommand {
    pub id: u64,
    pub primitive_id: u64,
    pub parameters: BTreeMap<String, String>,
    pub status: MotorCommandStatus,
    pub issued_at: u128,
    pub started_at: Option<u128>,
    pub completed_at: Option<u128>,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// 运动命令队列。
pub struct MotorCommandQueue {
    commands: BTreeMap<u64, MotorCommand>,
    next_id: u64,
    pub max_queue_size: usize,
}

impl MotorCommandQueue {
    pub fn new() -> Self {
        Self {
            commands: BTreeMap::new(),
            next_id: 1,
            max_queue_size: 10000,
        }
    }

    pub fn issue(&mut self, primitive_id: u64, parameters: &[(&str, &str)]) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let params: BTreeMap<String, String> = parameters.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let cmd = MotorCommand {
            id,
            primitive_id,
            parameters: params,
            status: MotorCommandStatus::Pending,
            issued_at: crate::ai_os::native::now_ns(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
        };
        self.commands.insert(id, cmd);
        id
    }

    pub fn start(&mut self, id: u64) -> bool {
        if let Some(cmd) = self.commands.get_mut(&id) {
            if cmd.status == MotorCommandStatus::Pending {
                cmd.status = MotorCommandStatus::Executing;
                cmd.started_at = Some(crate::ai_os::native::now_ns());
                return true;
            }
        }
        false
    }

    pub fn complete(&mut self, id: u64, result: &str) -> bool {
        if let Some(cmd) = self.commands.get_mut(&id) {
            if cmd.status == MotorCommandStatus::Executing {
                cmd.status = MotorCommandStatus::Completed;
                cmd.completed_at = Some(crate::ai_os::native::now_ns());
                cmd.result = Some(result.to_string());
                return true;
            }
        }
        false
    }

    pub fn fail(&mut self, id: u64, error: &str) -> bool {
        if let Some(cmd) = self.commands.get_mut(&id) {
            if cmd.status == MotorCommandStatus::Executing {
                cmd.status = MotorCommandStatus::Failed;
                cmd.completed_at = Some(crate::ai_os::native::now_ns());
                cmd.error = Some(error.to_string());
                return true;
            }
        }
        false
    }

    pub fn get(&self, id: u64) -> Option<&MotorCommand> {
        self.commands.get(&id)
    }

    pub fn count(&self) -> usize {
        self.commands.len()
    }

    pub fn pending_count(&self) -> usize {
        self.commands.values().filter(|c| c.status == MotorCommandStatus::Pending).count()
    }

    pub fn completed_count(&self) -> usize {
        self.commands.values().filter(|c| c.status == MotorCommandStatus::Completed).count()
    }

    pub fn failed_count(&self) -> usize {
        self.commands.values().filter(|c| c.status == MotorCommandStatus::Failed).count()
    }
}

// =====================================================================================
// 运动执行器 (MotorExecutor)
// =====================================================================================

/// 运动执行器：动作原语的实际执行。
pub struct MotorExecutor {
    pub execution_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub total_execution_ns: u128,
}

impl MotorExecutor {
    pub fn new() -> Self {
        Self {
            execution_count: 0,
            success_count: 0,
            failure_count: 0,
            total_execution_ns: 0,
        }
    }

    pub fn execute(&mut self, primitive: &ActionPrimitive, start_ns: u128) -> (bool, u128, String) {
        self.execution_count += 1;
        let end_ns = crate::ai_os::native::now_ns();
        let duration = end_ns.saturating_sub(start_ns);
        self.total_execution_ns += duration;

        // 模拟成功/失败
        let success = primitive.success_probability > 0.5; // 简化：概率>0.5 为成功
        if success {
            self.success_count += 1;
            (true, duration, format!("执行成功: {}", primitive.name))
        } else {
            self.failure_count += 1;
            (false, duration, format!("执行失败: {}", primitive.name))
        }
    }

    pub fn success_rate(&self) -> f32 {
        if self.execution_count == 0 {
            0.0
        } else {
            self.success_count as f32 / self.execution_count as f32
        }
    }

    pub fn avg_duration_ns(&self) -> u128 {
        if self.execution_count == 0 {
            0
        } else {
            self.total_execution_ns / self.execution_count as u128
        }
    }
}

// =====================================================================================
// 动作序列 (ActionSequence)
// =====================================================================================

/// 动作序列步骤。
#[derive(Clone, Debug)]
pub struct ActionStep {
    pub id: u64,
    pub primitive_id: u64,
    pub parameters: BTreeMap<String, String>,
    pub depends_on: Vec<u64>,   // 依赖的步骤 ID
    pub completed: bool,
}

/// 动作序列：多个动作的有序组合。
pub struct ActionSequence {
    pub id: u64,
    pub name: String,
    pub steps: Vec<ActionStep>,
    pub completed: bool,
    pub created_at: u128,
}

/// 动作序列库。
pub struct ActionSequenceLibrary {
    sequences: BTreeMap<u64, ActionSequence>,
    next_seq_id: u64,
    next_step_id: u64,
}

impl ActionSequenceLibrary {
    pub fn new() -> Self {
        Self {
            sequences: BTreeMap::new(),
            next_seq_id: 1,
            next_step_id: 1,
        }
    }

    pub fn create_sequence(&mut self, name: &str) -> u64 {
        let id = self.next_seq_id;
        self.next_seq_id += 1;
        let seq = ActionSequence {
            id,
            name: name.to_string(),
            steps: Vec::new(),
            completed: false,
            created_at: crate::ai_os::native::now_ns(),
        };
        self.sequences.insert(id, seq);
        id
    }

    pub fn add_step(&mut self, seq_id: u64, primitive_id: u64, parameters: &[(&str, &str)], depends_on: &[u64]) -> Result<u64, String> {
        let seq = self.sequences.get_mut(&seq_id).ok_or("序列不存在")?;
        let id = self.next_step_id;
        self.next_step_id += 1;
        let params: BTreeMap<String, String> = parameters.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        seq.steps.push(ActionStep {
            id,
            primitive_id,
            parameters: params,
            depends_on: depends_on.to_vec(),
            completed: false,
        });
        Ok(id)
    }

    pub fn complete_step(&mut self, seq_id: u64, step_id: u64) -> Result<(), String> {
        let seq = self.sequences.get_mut(&seq_id).ok_or("序列不存在")?;
        if let Some(step) = seq.steps.iter_mut().find(|s| s.id == step_id) {
            step.completed = true;
            Ok(())
        } else {
            Err("步骤不存在".into())
        }
    }

    pub fn sequence_count(&self) -> usize {
        self.sequences.len()
    }

    pub fn get_sequence(&self, id: u64) -> Option<&ActionSequence> {
        self.sequences.get(&id)
    }
}

// =====================================================================================
// 动作调度器 (ActionScheduler)
// =====================================================================================

/// 动作调度策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionSchedulePolicy {
    Fifo,      // 先进先出
    Priority,  // 优先级
    Deadline,  // 截止时间
    Fair,      // 公平调度
}

/// 动作调度器：动作的时序与并发调度。
pub struct ActionScheduler {
    pub policy: ActionSchedulePolicy,
    pub queue: VecDeque<u64>,              // 命令 ID 队列
    pub max_concurrent: usize,            // 最大并发数
    pub running: BTreeSet<u64>,           // 正在执行的命令 ID
    pub schedule_count: u64,
}

impl ActionScheduler {
    pub fn new() -> Self {
        Self {
            policy: ActionSchedulePolicy::Fifo,
            queue: VecDeque::new(),
            max_concurrent: 4,
            running: BTreeSet::new(),
            schedule_count: 0,
        }
    }

    pub fn enqueue(&mut self, cmd_id: u64) {
        self.queue.push_back(cmd_id);
    }

    pub fn dequeue(&mut self) -> Option<u64> {
        self.schedule_count += 1;
        self.queue.pop_front()
    }

    pub fn start_running(&mut self, cmd_id: u64) {
        self.running.insert(cmd_id);
    }

    pub fn finish_running(&mut self, cmd_id: u64) {
        self.running.remove(&cmd_id);
    }

    pub fn can_schedule(&self) -> bool {
        self.running.len() < self.max_concurrent
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn running_count(&self) -> usize {
        self.running.len()
    }
}

// =====================================================================================
// 动作反馈 (ActionFeedback)
// =====================================================================================

/// 动作反馈：动作执行后的状态反馈。
#[derive(Clone, Debug)]
pub struct ActionFeedback {
    pub command_id: u64,
    pub timestamp_ns: u128,
    pub success: bool,
    pub actual_duration_ns: u128,
    pub observed_result: String,
    pub error_message: Option<String>,
}

/// 动作反馈历史。
pub struct ActionFeedbackHistory {
    feedbacks: VecDeque<ActionFeedback>,
    max_history: usize,
}

impl ActionFeedbackHistory {
    pub fn new() -> Self {
        Self {
            feedbacks: VecDeque::new(),
            max_history: 10000,
        }
    }

    pub fn record(&mut self, feedback: ActionFeedback) {
        self.feedbacks.push_back(feedback);
        if self.feedbacks.len() > self.max_history {
            self.feedbacks.pop_front();
        }
    }

    pub fn recent(&self, n: usize) -> Vec<&ActionFeedback> {
        self.feedbacks.iter().rev().take(n).collect()
    }

    pub fn success_rate(&self) -> f32 {
        if self.feedbacks.is_empty() {
            0.0
        } else {
            let successes = self.feedbacks.iter().filter(|f| f.success).count();
            successes as f32 / self.feedbacks.len() as f32
        }
    }

    pub fn count(&self) -> usize {
        self.feedbacks.len()
    }
}

// =====================================================================================
// 动作约束 (ActionConstraint)
// =====================================================================================

/// 动作约束类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionConstraintType {
    Precondition,  // 前置条件
    Postcondition, // 后置条件
    Resource,      // 资源约束
    Temporal,      // 时序约束
    Spatial,       // 空间约束
}

/// 动作约束。
#[derive(Clone, Debug)]
pub struct ActionConstraint {
    pub id: u64,
    pub r#type: ActionConstraintType,
    pub target_action: String,
    pub condition: String,
    pub satisfied: bool,
}

/// 动作约束管理器。
pub struct ActionConstraintManager {
    constraints: BTreeMap<u64, ActionConstraint>,
    next_id: u64,
}

impl ActionConstraintManager {
    pub fn new() -> Self {
        Self {
            constraints: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add(&mut self, r#type: ActionConstraintType, target_action: &str, condition: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.constraints.insert(id, ActionConstraint {
            id,
            r#type,
            target_action: target_action.to_string(),
            condition: condition.to_string(),
            satisfied: false,
        });
        id
    }

    pub fn satisfy(&mut self, id: u64) -> bool {
        if let Some(c) = self.constraints.get_mut(&id) {
            c.satisfied = true;
            true
        } else {
            false
        }
    }

    pub fn unsatisfied_count(&self) -> usize {
        self.constraints.values().filter(|c| !c.satisfied).count()
    }

    pub fn count(&self) -> usize {
        self.constraints.len()
    }
}

// =====================================================================================
// 动作引擎 (ActionEngine) —— 统一入口
// =====================================================================================

/// 动作引擎：整合动作原语、运动命令、运动执行器、动作序列、动作调度器、动作反馈、动作约束。
pub struct ActionEngine {
    pub primitive_registry: ActionPrimitiveRegistry,
    pub command_queue: MotorCommandQueue,
    pub executor: MotorExecutor,
    pub sequence_library: ActionSequenceLibrary,
    pub scheduler: ActionScheduler,
    pub feedback_history: ActionFeedbackHistory,
    pub constraint_manager: ActionConstraintManager,
    pub enabled: bool,
    pub total_actions: u64,
}

impl ActionEngine {
    pub fn new() -> Self {
        Self {
            primitive_registry: ActionPrimitiveRegistry::new(),
            command_queue: MotorCommandQueue::new(),
            executor: MotorExecutor::new(),
            sequence_library: ActionSequenceLibrary::new(),
            scheduler: ActionScheduler::new(),
            feedback_history: ActionFeedbackHistory::new(),
            constraint_manager: ActionConstraintManager::new(),
            enabled: true,
            total_actions: 0,
        }
    }

    pub fn register_primitive(
        &mut self,
        r#type: ActionPrimitiveType,
        name: &str,
        duration_ns: u128,
        energy_cost: f32,
        success_prob: f32,
    ) -> u64 {
        self.primitive_registry.register(
            r#type, name, &[], &[], &[], duration_ns, energy_cost, success_prob
        )
    }

    pub fn issue_command(&mut self, primitive_id: u64, params: &[(&str, &str)]) -> u64 {
        let id = self.command_queue.issue(primitive_id, params);
        self.scheduler.enqueue(id);
        self.total_actions += 1;
        id
    }

    pub fn execute_next(&mut self) -> Result<String, String> {
        let cmd_id = self.scheduler.dequeue().ok_or("队列为空")?;
        let cmd = self.command_queue.get(cmd_id).ok_or("命令不存在")?;
        let primitive_id = cmd.primitive_id;
        let issued_at = cmd.issued_at;
        let primitive = self.primitive_registry.get(primitive_id).ok_or("原语不存在")?;

        if !primitive.enabled {
            return Err("动作原语已禁用".into());
        }

        self.command_queue.start(cmd_id);
        self.scheduler.start_running(cmd_id);

        let (success, duration, result) = self.executor.execute(primitive, issued_at);

        if success {
            self.command_queue.complete(cmd_id, &result);
        } else {
            self.command_queue.fail(cmd_id, &result);
        }

        self.scheduler.finish_running(cmd_id);

        // 记录反馈
        let feedback = ActionFeedback {
            command_id: cmd_id,
            timestamp_ns: crate::ai_os::native::now_ns(),
            success,
            actual_duration_ns: duration,
            observed_result: result.clone(),
            error_message: if success { None } else { Some(result.clone()) },
        };
        self.feedback_history.record(feedback);

        Ok(result)
    }

    pub fn create_sequence(&mut self, name: &str) -> u64 {
        self.sequence_library.create_sequence(name)
    }

    pub fn add_sequence_step(
        &mut self,
        seq_id: u64,
        primitive_id: u64,
        params: &[(&str, &str)],
        depends_on: &[u64],
    ) -> Result<u64, String> {
        self.sequence_library.add_step(seq_id, primitive_id, params, depends_on)
    }

    pub fn add_constraint(
        &mut self,
        r#type: ActionConstraintType,
        target_action: &str,
        condition: &str,
    ) -> u64 {
        self.constraint_manager.add(r#type, target_action, condition)
    }

    pub fn satisfy_constraint(&mut self, id: u64) -> bool {
        self.constraint_manager.satisfy(id)
    }

    pub fn primitive_count(&self) -> usize {
        self.primitive_registry.count()
    }

    pub fn command_count(&self) -> usize {
        self.command_queue.count()
    }

    pub fn completed_count(&self) -> usize {
        self.command_queue.completed_count()
    }

    pub fn failed_count(&self) -> usize {
        self.command_queue.failed_count()
    }

    pub fn success_rate(&self) -> f32 {
        self.executor.success_rate()
    }

    pub fn avg_duration_ns(&self) -> u128 {
        self.executor.avg_duration_ns()
    }
}

impl Default for ActionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static ACTION: OnceLock<Mutex<ActionEngine>> = OnceLock::new();

pub fn action_engine() -> &'static Mutex<ActionEngine> {
    ACTION.get_or_init(|| Mutex::new(ActionEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = action_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = action_engine().lock().unwrap();
    format!(
        "action: primitives={} commands={} completed={} failed={} success_rate={:.2} avg_duration_ns={} sequences={} scheduler_running={} constraints_unsat={} total_actions={}",
        eng.primitive_count(),
        eng.command_count(),
        eng.completed_count(),
        eng.failed_count(),
        eng.success_rate(),
        eng.avg_duration_ns(),
        eng.sequence_library.sequence_count(),
        eng.scheduler.running_count(),
        eng.constraint_manager.unsatisfied_count(),
        eng.total_actions
    )
}

pub fn action_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn action_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("register-primitive") => {
            let type_str = it.next().unwrap_or("move");
            let r#type = match type_str {
                "move" => ActionPrimitiveType::Move,
                "grasp" => ActionPrimitiveType::Grasp,
                "release" => ActionPrimitiveType::Release,
                "look" => ActionPrimitiveType::Look,
                "listen" => ActionPrimitiveType::Listen,
                "speak" => ActionPrimitiveType::Speak,
                "manipulate" => ActionPrimitiveType::Manipulate,
                "navigate" => ActionPrimitiveType::Navigate,
                "compute" => ActionPrimitiveType::Compute,
                "communicate" => ActionPrimitiveType::Communicate,
                "rest" => ActionPrimitiveType::Rest,
                _ => ActionPrimitiveType::Custom,
            };
            let name = it.next().ok_or("缺少名称")?;
            let duration = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(100_000_000);
            let energy = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.1);
            let success = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.9);
            let mut eng = action_engine().lock().unwrap();
            let id = eng.register_primitive(r#type, name, duration, energy, success);
            Ok(format!("已注册动作原语 id={} 类型={:?} 名称={}", id, r#type, name))
        }
        Some("issue") => {
            let primitive_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少原语id")?;
            let params: Vec<&str> = it.collect();
            let param_pairs: Vec<(&str, &str)> = params.iter().map(|s| (*s, "")).collect();
            let mut eng = action_engine().lock().unwrap();
            let id = eng.issue_command(primitive_id, &param_pairs);
            Ok(format!("已发出运动命令 id={} 原语={}", id, primitive_id))
        }
        Some("execute") => {
            let mut eng = action_engine().lock().unwrap();
            match eng.execute_next() {
                Ok(result) => Ok(format!("动作执行: {}", result)),
                Err(e) => Err(e),
            }
        }
        Some("execute-all") => {
            let mut results = Vec::new();
            loop {
                let mut eng = action_engine().lock().unwrap();
                match eng.execute_next() {
                    Ok(r) => results.push(r),
                    Err(_) => break,
                }
            }
            let mut out = String::from("批量执行完成:\n");
            for r in results {
                out.push_str(&format!("  {}\n", r));
            }
            Ok(out)
        }
        Some("create-sequence") => {
            let name = it.next().ok_or("缺少序列名称")?;
            let mut eng = action_engine().lock().unwrap();
            let id = eng.create_sequence(name);
            Ok(format!("已创建动作序列 id={} 名称={}", id, name))
        }
        Some("add-step") => {
            let seq_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少序列id")?;
            let primitive_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少原语id")?;
            let params: Vec<&str> = it.collect();
            let param_pairs: Vec<(&str, &str)> = params.iter().map(|s| (*s, "")).collect();
            let mut eng = action_engine().lock().unwrap();
            match eng.add_sequence_step(seq_id, primitive_id, &param_pairs, &[]) {
                Ok(step_id) => Ok(format!("已添加步骤 id={} 到序列 {}", step_id, seq_id)),
                Err(e) => Err(e),
            }
        }
        Some("add-constraint") => {
            let type_str = it.next().unwrap_or("precondition");
            let r#type = match type_str {
                "precondition" => ActionConstraintType::Precondition,
                "postcondition" => ActionConstraintType::Postcondition,
                "resource" => ActionConstraintType::Resource,
                "temporal" => ActionConstraintType::Temporal,
                "spatial" => ActionConstraintType::Spatial,
                _ => ActionConstraintType::Precondition,
            };
            let target = it.next().ok_or("缺少目标动作")?;
            let condition = it.next().ok_or("缺少条件")?;
            let mut eng = action_engine().lock().unwrap();
            let id = eng.add_constraint(r#type, target, condition);
            Ok(format!("已添加动作约束 id={}", id))
        }
        Some("satisfy-constraint") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少约束id")?;
            let mut eng = action_engine().lock().unwrap();
            if eng.satisfy_constraint(id) {
                Ok(format!("约束 id={} 已满足", id))
            } else {
                Err(format!("约束 id={} 不存在", id))
            }
        }
        Some("primitive") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少原语id")?;
            let eng = action_engine().lock().unwrap();
            match eng.primitive_registry.get(id) {
                Some(p) => Ok(format!("原语[id={}] 类型={:?} 名称={} 启用={} 能量={:.2} 成功率={:.2}", id, p.r#type, p.name, p.enabled, p.energy_cost, p.success_probability)),
                None => Err(format!("原语[id={}] 不存在", id)),
            }
        }
        Some("list-primitives") => {
            let eng = action_engine().lock().unwrap();
            let mut out = String::from("动作原语列表:\n");
            for (id, p) in eng.primitive_registry.primitives.iter() {
                out.push_str(&format!("  id={} 类型={:?} 名称={} 启用={}\n", id, p.r#type, p.name, p.enabled));
            }
            Ok(out)
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 action 动作: {}", other)),
    }
}

pub fn action_act_dispatch(payload: &str) -> Result<String, String> {
    action_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_primitive_type() {
        assert_eq!(ActionPrimitiveType::Move as u32, 0);
        assert_eq!(ActionPrimitiveType::Speak as u32, 5);
        assert_eq!(ActionPrimitiveType::Custom as u32, 11);
    }

    #[test]
    fn primitive_registry_register() {
        let mut reg = ActionPrimitiveRegistry::new();
        let id = reg.register(
            ActionPrimitiveType::Move,
            "move_forward",
            &[("distance", "1.0")],
            &["obstacle_free"],
            &["position_changed"],
            200_000_000,
            0.3,
            0.95,
        );
        assert_eq!(id, 1);
        let p = reg.get(id).unwrap();
        assert_eq!(p.name, "move_forward");
        assert_eq!(reg.count(), 1);
        assert_eq!(reg.enabled_count(), 1);
    }

    #[test]
    fn primitive_enable_disable() {
        let mut reg = ActionPrimitiveRegistry::new();
        let id = reg.register(ActionPrimitiveType::Move, "move", &[], &[], &[], 100_000_000, 0.1, 0.9);
        assert!(reg.disable(id));
        assert_eq!(reg.enabled_count(), 0);
        assert!(reg.enable(id));
        assert_eq!(reg.enabled_count(), 1);
    }

    #[test]
    fn primitive_get_by_name() {
        let mut reg = ActionPrimitiveRegistry::new();
        reg.register(ActionPrimitiveType::Speak, "say_hello", &[], &[], &[], 100_000_000, 0.1, 0.9);
        let p = reg.get_by_name("say_hello").unwrap();
        assert_eq!(p.name, "say_hello");
        assert!(reg.get_by_name("nonexistent").is_none());
    }

    #[test]
    fn motor_command_queue() {
        let mut queue = MotorCommandQueue::new();
        let id1 = queue.issue(1, &[("a", "b")]);
        let id2 = queue.issue(2, &[("c", "d")]);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(queue.count(), 2);
        assert_eq!(queue.pending_count(), 2);
    }

    #[test]
    fn motor_command_start_complete_fail() {
        let mut queue = MotorCommandQueue::new();
        let id = queue.issue(1, &[]);
        assert!(queue.start(id));
        assert!(queue.complete(id, "done"));
        let cmd = queue.get(id).unwrap();
        assert_eq!(cmd.status, MotorCommandStatus::Completed);
        assert_eq!(cmd.result, Some("done".into()));
    }

    #[test]
    fn motor_command_fail() {
        let mut queue = MotorCommandQueue::new();
        let id = queue.issue(1, &[]);
        assert!(queue.start(id));
        assert!(queue.fail(id, "error"));
        let cmd = queue.get(id).unwrap();
        assert_eq!(cmd.status, MotorCommandStatus::Failed);
        assert_eq!(cmd.error, Some("error".into()));
    }

    #[test]
    fn motor_executor() {
        let mut exec = MotorExecutor::new();
        let prim = ActionPrimitive {
            id: 1, r#type: ActionPrimitiveType::Move, name: "move".into(),
            parameters: BTreeMap::new(), preconditions: Vec::new(), postconditions: Vec::new(),
            estimated_duration_ns: 100_000_000, energy_cost: 0.1, success_probability: 0.9, enabled: true,
        };
        let (success, _duration, _result) = exec.execute(&prim, 0);
        assert!(success);
        assert_eq!(exec.execution_count, 1);
        assert_eq!(exec.success_count, 1);
        assert!(exec.success_rate() > 0.0);
    }

    #[test]
    fn motor_executor_failure() {
        let mut exec = MotorExecutor::new();
        let prim = ActionPrimitive {
            id: 1, r#type: ActionPrimitiveType::Move, name: "move".into(),
            parameters: BTreeMap::new(), preconditions: Vec::new(), postconditions: Vec::new(),
            estimated_duration_ns: 100_000_000, energy_cost: 0.1, success_probability: 0.1, enabled: true,
        };
        let (success, _, _) = exec.execute(&prim, 0);
        assert!(!success);
        assert_eq!(exec.failure_count, 1);
    }

    #[test]
    fn action_sequence_library() {
        let mut lib = ActionSequenceLibrary::new();
        let seq_id = lib.create_sequence("pick_and_place");
        assert_eq!(seq_id, 1);
        let step_id = lib.add_step(seq_id, 1, &[("obj", "block")], &[]).unwrap();
        assert_eq!(step_id, 1);
        assert!(lib.complete_step(seq_id, step_id).is_ok());
        assert_eq!(lib.sequence_count(), 1);
    }

    #[test]
    fn action_scheduler() {
        let mut sched = ActionScheduler::new();
        sched.enqueue(1);
        sched.enqueue(2);
        assert_eq!(sched.queue_len(), 2);
        assert_eq!(sched.dequeue(), Some(1));
        assert_eq!(sched.queue_len(), 1);
        sched.start_running(2);
        assert_eq!(sched.running_count(), 1);
        assert!(sched.can_schedule());
        sched.finish_running(2);
        assert_eq!(sched.running_count(), 0);
    }

    #[test]
    fn action_feedback_history() {
        let mut hist = ActionFeedbackHistory::new();
        hist.record(ActionFeedback {
            command_id: 1,
            timestamp_ns: 0,
            success: true,
            actual_duration_ns: 100_000_000,
            observed_result: "ok".into(),
            error_message: None,
        });
        hist.record(ActionFeedback {
            command_id: 2,
            timestamp_ns: 0,
            success: false,
            actual_duration_ns: 200_000_000,
            observed_result: "fail".into(),
            error_message: Some("error".into()),
        });
        assert_eq!(hist.count(), 2);
        assert!((hist.success_rate() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn action_constraint_manager() {
        let mut mgr = ActionConstraintManager::new();
        let id = mgr.add(ActionConstraintType::Precondition, "move", "obstacle_free");
        assert_eq!(id, 1);
        assert_eq!(mgr.unsatisfied_count(), 1);
        assert!(mgr.satisfy(id));
        assert_eq!(mgr.unsatisfied_count(), 0);
    }

    #[test]
    fn action_engine_full() {
        let mut eng = ActionEngine::new();
        let prim_id = eng.register_primitive(ActionPrimitiveType::Move, "move", 100_000_000, 0.1, 0.9);
        let cmd_id = eng.issue_command(prim_id, &[("dist", "1.0")]);
        assert_eq!(cmd_id, 1);
        let result = eng.execute_next();
        assert!(result.is_ok());
        assert_eq!(eng.completed_count(), 1);
        assert!(eng.success_rate() > 0.0);
    }

    #[test]
    fn action_engine_sequence() {
        let mut eng = ActionEngine::new();
        let prim_id = eng.register_primitive(ActionPrimitiveType::Grasp, "grasp", 100_000_000, 0.2, 0.9);
        let seq_id = eng.create_sequence("grasp_seq");
        let step_id = eng.add_sequence_step(seq_id, prim_id, &[("obj", "cup")], &[]).unwrap();
        assert_eq!(step_id, 1);
    }

    #[test]
    fn action_engine_constraint() {
        let mut eng = ActionEngine::new();
        let cid = eng.add_constraint(ActionConstraintType::Resource, "move", "battery>20%");
        assert_eq!(cid, 1);
        assert_eq!(eng.constraint_manager.unsatisfied_count(), 1);
        assert!(eng.satisfy_constraint(cid));
        assert_eq!(eng.constraint_manager.unsatisfied_count(), 0);
    }

    #[test]
    fn action_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("action:"));
        assert!(s.contains("primitives="));
    }

    #[test]
    fn action_act_register() {
        init();
        let res = action_act("register-primitive move 移动 100000000 0.1 0.9").unwrap();
        assert!(res.contains("已注册动作原语"));
    }

    #[test]
    fn action_act_issue() {
        init();
        action_act("register-primitive move 移动 100000000 0.1 0.9").unwrap();
        let res = action_act("issue 1 dist=1.0").unwrap();
        assert!(res.contains("运动命令"));
    }

    #[test]
    fn action_act_execute() {
        init();
        action_act("register-primitive move 移动 100000000 0.1 0.9").unwrap();
        action_act("issue 1 dist=1.0").unwrap();
        let res = action_act("execute").unwrap();
        assert!(res.contains("动作执行"));
    }

    #[test]
    fn action_act_create_sequence() {
        init();
        let res = action_act("create-sequence 抓取放置").unwrap();
        assert!(res.contains("动作序列"));
    }

    #[test]
    fn action_act_add_step() {
        init();
        action_act("register-primitive grasp 抓取 100000000 0.2 0.9").unwrap();
        action_act("create-sequence 抓取序列").unwrap();
        let res = action_act("add-step 1 1 obj=cup").unwrap();
        assert!(res.contains("步骤"));
    }

    #[test]
    fn action_act_constraint() {
        init();
        let res = action_act("add-constraint precondition move 无障碍").unwrap();
        assert!(res.contains("动作约束"));
        let res = action_act("satisfy-constraint 1").unwrap();
        assert!(res.contains("已满足"));
    }

    #[test]
    fn action_act_status() {
        init();
        let res = action_act("status").unwrap();
        assert!(res.contains("action:"));
    }
}
