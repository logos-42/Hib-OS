//! 多调度域 —— Hib OS 的「多内核」形态。
//!
//! 五调度域，每个域独立运行 MLFQ 调度器，彼此隔离，互不共享就绪队列与等待表：
//!
//! * `reflex`   —— 反射域，中断/信号级快速响应，最高优先级
//! * `perception` —— 感知域，sense 通道快照，通道事件驱动
//! * `deliberation` —— 审议域，act 通道写入与租约提交，单写者租约串行化
//! * `memory` —— 记忆域，状态快照与历史回溯，只读为主，读写并发
//! * `governance` —— 治理域，电源/能力/策略决策，root capability 门禁
//!
//! 与 Linux 的差异（AI 原生）：
//! * Linux 是一套 CFS 调度器管理所有 CPU；Hib OS 是五个独立调度域，每个域自己的 MLFQ。
//! * 任务明确归属某个调度域，跨域切换需要显式迁移。
//! * AI 意识通过 `sched://<域>` 通道感知与控制各域：查看状态、入队任务、唤醒等待者、抬升优先级。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::ai_token::{AIConsciousness, ConceptId, TokenId};
use crate::scheduler::{CpuPriority, GpuScheduler};

/// 调度域标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SchedDomain {
    Reflex,
    Perception,
    Deliberation,
    Memory,
    Governance,
}

impl SchedDomain {
    pub fn name(self) -> &'static str {
        match self {
            SchedDomain::Reflex => "reflex",
            SchedDomain::Perception => "perception",
            SchedDomain::Deliberation => "deliberation",
            SchedDomain::Memory => "memory",
            SchedDomain::Governance => "governance",
        }
    }

    /// 该域的默认调度优先级（新任务入队时的基础层级）。
    pub fn default_priority(self) -> CpuPriority {
        match self {
            SchedDomain::Reflex => CpuPriority::Critical,
            SchedDomain::Perception => CpuPriority::High,
            SchedDomain::Deliberation => CpuPriority::Medium,
            SchedDomain::Memory => CpuPriority::Low,
            SchedDomain::Governance => CpuPriority::Critical,
        }
    }
}

/// 调度域注册表：每个域一个独立的 GpuScheduler。
struct Domains {
    map: BTreeMap<SchedDomain, GpuScheduler>,
}

static DOMAINS: OnceLock<Mutex<Domains>> = OnceLock::new();

fn domains() -> &'static Mutex<Domains> {
    DOMAINS.get_or_init(|| {
        let mut map = BTreeMap::new();
        for d in [
            SchedDomain::Reflex,
            SchedDomain::Perception,
            SchedDomain::Deliberation,
            SchedDomain::Memory,
            SchedDomain::Governance,
        ] {
            let mut consciousness = AIConsciousness::new();
            consciousness.add_common_concepts();
            map.insert(d, GpuScheduler::new(consciousness));
        }
        Mutex::new(Domains { map })
    })
}

/// 获取指定调度域的调度器并执行闭包。
fn with_domain<F, R>(domain: SchedDomain, f: F) -> R
where
    F: FnOnce(&mut GpuScheduler) -> R,
{
    let mut g = domains().lock().unwrap();
    f(g.map.get_mut(&domain).unwrap())
}

/// 向指定调度域入队一个 token（带语义概念）。
pub fn enqueue(
    domain: SchedDomain,
    token_id: TokenId,
    priority: CpuPriority,
    concept_id: Option<ConceptId>,
) {
    with_domain(domain, |s| {
        s.enqueue(token_id, priority, concept_id);
    });
}

/// 从指定调度域选下一个要运行的 token。
pub fn pick_next(domain: SchedDomain) -> Option<TokenId> {
    with_domain(domain, |s| s.pick_next())
}

/// 指定域的时间片到期：运行中的任务降级后重入队。
pub fn on_time_slice_expiry(domain: SchedDomain) {
    with_domain(domain, |s| s.on_time_slice_expiry());
}

/// 把指定域的某个 token 标记为「等待某条通道事件」，移出就绪队列。
pub fn mark_waiting(domain: SchedDomain, token_id: TokenId, channel_key: &str) {
    with_domain(domain, |s| s.mark_waiting(token_id, channel_key));
}

/// 通道事件到达：指定域中等待该键的 token 放回就绪队列。返回唤醒数量。
pub fn wake_key(domain: SchedDomain, channel_key: &str) -> usize {
    with_domain(domain, |s| s.wake_key(channel_key))
}

/// 优先级继承：抬升指定域中某个 token 的优先级。
pub fn boost(domain: SchedDomain, token_id: TokenId, to: CpuPriority) -> bool {
    with_domain(domain, |s| s.boost(token_id, to))
}

/// 查询指定域的就绪数量。
pub fn ready_count(domain: SchedDomain) -> usize {
    with_domain(domain, |s| s.ready_count())
}

/// 查询指定域的等待数量。
pub fn waiting_count(domain: SchedDomain) -> usize {
    with_domain(domain, |s| s.waiting_count())
}

/// 全部调度域的状态摘要。
pub fn status() -> String {
    let g = domains().lock().unwrap();
    let mut out = String::new();
    for (d, s) in g.map.iter() {
        out.push_str(&format!("domain={:<14} {}\n", d.name(), s.summary()));
    }
    out.trim_end().to_string()
}

/// 全部调度域的简要状态行（供 `sense sched://domains`）。
pub fn status_line() -> String {
    let g = domains().lock().unwrap();
    let mut parts = Vec::new();
    for (d, s) in g.map.iter() {
        parts.push(format!(
            "{}={}ready/{}waiting",
            d.name(),
            s.ready_count(),
            s.waiting_count()
        ));
    }
    parts.join(" | ")
}

/// 解析域名字符串。
pub fn parse_domain(s: &str) -> Result<SchedDomain, String> {
    match s {
        "reflex" => Ok(SchedDomain::Reflex),
        "perception" => Ok(SchedDomain::Perception),
        "deliberation" => Ok(SchedDomain::Deliberation),
        "memory" => Ok(SchedDomain::Memory),
        "governance" => Ok(SchedDomain::Governance),
        other => Err(format!(
            "未知调度域 {}（reflex/perception/deliberation/memory/governance）",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_token::AIConsciousness;

    #[allow(dead_code)]
    fn consciousness() -> AIConsciousness {
        let mut c = AIConsciousness::new();
        c.add_common_concepts();
        c
    }

    #[test]
    fn domains_are_isolated() {
        let t1 = TokenId::new();
        let t2 = TokenId::new();
        enqueue(SchedDomain::Reflex, t1, CpuPriority::Low, None);
        enqueue(SchedDomain::Perception, t2, CpuPriority::Low, None);
        assert_eq!(ready_count(SchedDomain::Reflex), 1);
        assert_eq!(ready_count(SchedDomain::Perception), 1);
        assert_eq!(pick_next(SchedDomain::Reflex), Some(t1));
        assert_eq!(pick_next(SchedDomain::Perception), Some(t2));
        // reflex 的 pick_next 不应把 perception 的任务带走
        assert_eq!(pick_next(SchedDomain::Reflex), None);
    }

    #[test]
    fn wake_key_wakes_only_target_domain() {
        let t = TokenId::new();
        enqueue(SchedDomain::Deliberation, t, CpuPriority::Medium, None);
        mark_waiting(SchedDomain::Deliberation, t, "gpu://vram");
        assert_eq!(ready_count(SchedDomain::Deliberation), 0);
        assert_eq!(waiting_count(SchedDomain::Deliberation), 1);
        let n = wake_key(SchedDomain::Deliberation, "gpu://vram");
        assert_eq!(n, 1);
        assert_eq!(ready_count(SchedDomain::Deliberation), 1);
        // 其他域不受影响
        assert_eq!(ready_count(SchedDomain::Reflex), 0);
    }

    #[test]
    fn status_line_is_observable() {
        let s = status_line();
        assert!(s.contains("reflex="));
        assert!(s.contains("perception="));
        assert!(s.contains("deliberation="));
        assert!(s.contains("memory="));
        assert!(s.contains("governance="));
    }

    #[test]
    fn boost_in_domain() {
        let low = TokenId::new();
        let high = TokenId::new();
        enqueue(SchedDomain::Governance, low, CpuPriority::Low, None);
        enqueue(SchedDomain::Governance, high, CpuPriority::High, None);
        assert!(boost(SchedDomain::Governance, low, CpuPriority::Critical));
        assert_eq!(pick_next(SchedDomain::Governance), Some(low));
    }
}
