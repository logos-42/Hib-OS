//! 系统调用处理器 —— 把 `AiSystemCall` 分派到具体内核组件。

use std::collections::BTreeMap;

use crate::ai_token::{
    self, ActionProcess, AIConsciousness, ConceptId, ProcessState, TokenCreateRequest,
    TokenId, TokenStatus,
};
use crate::scheduler::GpuScheduler;
use crate::syscall::{AiSystemCall, AiSystemCallError, AiSystemCallResult};

/// 内核态进程表。
#[derive(Default)]
pub struct ProcessTable {
    processes: BTreeMap<TokenId, ActionProcess>,
}

impl ProcessTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, p: ActionProcess) -> TokenId {
        let pid = p.pid;
        self.processes.insert(pid, p);
        pid
    }

    pub fn get(&self, pid: TokenId) -> Option<&ActionProcess> {
        self.processes.get(&pid)
    }

    pub fn get_mut(&mut self, pid: TokenId) -> Option<&mut ActionProcess> {
        self.processes.get_mut(&pid)
    }

    pub fn remove(&mut self, pid: TokenId) -> Option<ActionProcess> {
        self.processes.remove(&pid)
    }

    pub fn len(&self) -> usize {
        self.processes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&TokenId, &ActionProcess)> {
        self.processes.iter()
    }
}

/// 系统调用处理器上下文。
pub struct SyscallHandler {
    pub consciousness: AIConsciousness,
    pub scheduler: GpuScheduler,
    pub processes: ProcessTable,
}

impl SyscallHandler {
    pub fn new(mut consciousness: AIConsciousness) -> Self {
        consciousness.add_common_concepts();
        consciousness.add_extended_concepts();
        let scheduler = GpuScheduler::new(consciousness.clone());
        Self {
            consciousness,
            scheduler,
            processes: ProcessTable::new(),
        }
    }

    /// 分派一个系统调用。
    pub fn handle(&mut self, call: AiSystemCall) -> AiSystemCallResult {
        match call {
            AiSystemCall::CreateToken {
                model_id,
                capabilities,
                vram_size,
                priority,
                concept_id,
            } => {
                let req = TokenCreateRequest {
                    model_id,
                    requested_caps: capabilities,
                    requested_vram: vram_size,
                    priority,
                };
                match ai_token::create_token_external(&req) {
                    Ok(token) => {
                        // 立刻按概念入调度队列
                        let concept = concept_id.filter(|c| self.consciousness.exists(*c));
                        self.scheduler
                            .enqueue(token.token_id, priority, concept);
                        AiSystemCallResult {
                            success: true,
                            token_id: Some(token.token_id),
                            ..Default::default()
                        }
                    }
                    Err(e) => AiSystemCallResult::err(e.into()),
                }
            }

            AiSystemCall::CreateProcess {
                ontology_id,
                compute_graph,
                vram_size,
                priority,
            } => {
                if !self.consciousness.exists(ontology_id) {
                    return AiSystemCallResult::err(AiSystemCallError::ConceptNotFound);
                }
                match ActionProcess::new(
                    &self.consciousness,
                    ontology_id,
                    compute_graph,
                    vram_size,
                ) {
                    Ok(mut p) => {
                        p.priority = priority;
                        let pid = p.pid;
                        let submit = p.submit(&mut self.scheduler);
                        match submit {
                            Ok(()) => {
                                self.processes.insert(p);
                                AiSystemCallResult {
                                    success: true,
                                    process_id: Some(pid),
                                    ..Default::default()
                                }
                            }
                            Err(_) => AiSystemCallResult::err(
                                AiSystemCallError::InvalidProcessState,
                            ),
                        }
                    }
                    Err(_) => AiSystemCallResult::err(AiSystemCallError::InsufficientVram),
                }
            }

            AiSystemCall::UpdateCaps {
                token_id,
                new_caps,
            } => match ai_token::update_caps_external(token_id, new_caps) {
                Ok(()) => AiSystemCallResult::ok(),
                Err(e) => AiSystemCallResult::err(e.into()),
            },

            AiSystemCall::SetPriority {
                token_id,
                new_priority,
            } => match ai_token::update_priority_external(token_id, new_priority) {
                Ok(()) => AiSystemCallResult::ok(),
                Err(e) => AiSystemCallResult::err(e.into()),
            },

            AiSystemCall::QueryToken { token_id } => {
                match ai_token::query_token_status_external(token_id) {
                    Ok(status) => AiSystemCallResult {
                        success: true,
                        token_status: Some(status),
                        ..Default::default()
                    },
                    Err(e) => AiSystemCallResult::err(e.into()),
                }
            }

            AiSystemCall::QueryProcess { process_id } => match self.processes.get(process_id) {
                Some(p) => {
                    let status = (
                        p.pid,
                        p.state,
                        Option::<crate::ai_token::ProcessResult>::None,
                    );
                    AiSystemCallResult {
                        success: true,
                        process_status: Some(status),
                        ..Default::default()
                    }
                }
                None => AiSystemCallResult::err(AiSystemCallError::ProcessNotFound),
            },

            AiSystemCall::SubmitLoad {
                token_id,
                program_id: _,
                data_ptr: _,
            } => {
                if ai_token::query_token_status_external(token_id).is_err() {
                    return AiSystemCallResult::err(AiSystemCallError::TokenNotFound);
                }
                self.scheduler.enqueue(
                    token_id,
                    crate::scheduler::CpuPriority::Medium,
                    None,
                );
                AiSystemCallResult::ok()
            }

            AiSystemCall::CompleteProcess {
                process_id,
                success,
                output,
            } => match self.processes.remove(process_id) {
                Some(p) => {
                    let result = if success {
                        p.complete(output)
                    } else {
                        p.fail(crate::ai_token::ErrorCode::GpuError)
                    };
                    let status = (
                        process_id,
                        if result.success {
                            ProcessState::Completed
                        } else {
                            ProcessState::Failed
                        },
                        Some(result),
                    );
                    AiSystemCallResult {
                        success: true,
                        process_status: Some(status),
                        ..Default::default()
                    }
                }
                None => AiSystemCallResult::err(AiSystemCallError::ProcessNotFound),
            },

            AiSystemCall::FailProcess { process_id, error } => {
                match self.processes.remove(process_id) {
                    Some(p) => {
                        let result = p.fail(error);
                        AiSystemCallResult {
                            success: true,
                            process_status: Some((
                                process_id,
                                ProcessState::Failed,
                                Some(result),
                            )),
                            ..Default::default()
                        }
                    }
                    None => AiSystemCallResult::err(AiSystemCallError::ProcessNotFound),
                }
            }

            AiSystemCall::QuerySchedulePolicy { concept_id } => {
                match self.consciousness.get(concept_id) {
                    Some(node) => {
                        let policy = self
                            .consciousness
                            .find_schedule_policy(&node.compute_pattern)
                            .unwrap_or(crate::ai_token::SchedulePolicy::Batch);
                        AiSystemCallResult {
                            success: true,
                            schedule_policy: Some(policy),
                            ..Default::default()
                        }
                    }
                    None => AiSystemCallResult::err(AiSystemCallError::ConceptNotFound),
                }
            }
        }
    }

    /// 取出当前应当运行的 Token。
    pub fn dispatch_next(&mut self) -> Option<TokenId> {
        self.scheduler.pick_next()
    }
}

/// 便捷函数: 查询某个概念 ID。
pub fn concept_for(consciousness: &AIConsciousness, name: &str) -> Option<ConceptId> {
    consciousness.search_by_name(name).map(|n| n.id)
}

/// 便捷函数: 构造一个空的 Token 状态 (用于测试)。
pub fn empty_token_status() -> Option<TokenStatus> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_token::{ComputeGraph, GraphOpType, SchedulePolicy};
    use crate::gpu::{GpuCapability, GpuCapabilities};
    use crate::scheduler::CpuPriority;

    fn handler() -> SyscallHandler {
        SyscallHandler::new(AIConsciousness::new())
    }

    #[test]
    fn create_token_and_query() {
        let mut h = handler();
        let concept = h
            .consciousness
            .search_by_name("MatMul_Benchmark")
            .unwrap()
            .id;

        let r = h.handle(AiSystemCall::CreateToken {
            model_id: "sys-test".into(),
            capabilities: GpuCapabilities::from_caps(&[GpuCapability::Compute]),
            vram_size: 1024 * 1024,
            priority: CpuPriority::High,
            concept_id: Some(concept),
        });
        assert!(r.success);
        let tid = r.token_id.unwrap();
        assert_eq!(h.scheduler.ready_count(), 1);

        let q = h.handle(AiSystemCall::QueryToken { token_id: tid });
        assert!(q.success && q.token_status.is_some());

        let p = h.handle(AiSystemCall::QuerySchedulePolicy { concept_id: concept });
        assert_eq!(p.schedule_policy, Some(SchedulePolicy::HighThroughput));
    }

    #[test]
    fn create_process_and_complete() {
        let mut h = handler();
        let concept = h
            .consciousness
            .search_by_name("Transformer_Attention_Base")
            .unwrap()
            .id;

        let mut graph = ComputeGraph::new();
        let a = graph.add_node(GraphOpType::MatMul);
        let b = graph.add_node(GraphOpType::Attention);
        graph.add_edge(a, b, 2048);

        let r = h.handle(AiSystemCall::CreateProcess {
            ontology_id: concept,
            compute_graph: graph,
            vram_size: 1024 * 1024,
            priority: CpuPriority::High,
        });
        assert!(r.success, "创建进程失败: {:?}", r.error);
        let pid = r.process_id.unwrap();
        assert_eq!(h.processes.len(), 1);

        let q = h.handle(AiSystemCall::QueryProcess { process_id: pid });
        assert!(q.success);

        let done = h.handle(AiSystemCall::CompleteProcess {
            process_id: pid,
            success: true,
            output: Some(vec![0u8; 4]),
        });
        assert!(done.success);
        assert_eq!(h.processes.len(), 0);
        let (_, state, result) = done.process_status.unwrap();
        assert_eq!(state, ProcessState::Completed);
        assert!(result.unwrap().success);
    }

    #[test]
    fn unknown_concept_rejected() {
        let mut h = handler();
        let r = h.handle(AiSystemCall::CreateProcess {
            ontology_id: ConceptId([9u8; 16]),
            compute_graph: ComputeGraph::new(),
            vram_size: 1024,
            priority: CpuPriority::Medium,
        });
        assert_eq!(r.error, Some(AiSystemCallError::ConceptNotFound));
    }

    #[test]
    fn fail_process_marks_failed() {
        let mut h = handler();
        let concept = h.consciousness.search_by_name("QLearning_Batch").unwrap().id;
        let r = h.handle(AiSystemCall::CreateProcess {
            ontology_id: concept,
            compute_graph: ComputeGraph::new(),
            vram_size: 4096,
            priority: CpuPriority::Low,
        });
        let pid = r.process_id.unwrap();

        let failed = h.handle(AiSystemCall::FailProcess {
            process_id: pid,
            error: crate::ai_token::ErrorCode::GpuError,
        });
        let (_, state, result) = failed.process_status.unwrap();
        assert_eq!(state, ProcessState::Failed);
        assert_eq!(result.unwrap().error_code, Some(crate::ai_token::ErrorCode::GpuError));
    }

    #[test]
    fn submit_load_unknown_token_fails() {
        let mut h = handler();
        let r = h.handle(AiSystemCall::SubmitLoad {
            token_id: TokenId(999_999),
            program_id: 1,
            data_ptr: 0,
        });
        assert_eq!(r.error, Some(AiSystemCallError::TokenNotFound));
    }
}

