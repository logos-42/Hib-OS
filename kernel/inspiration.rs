//! 灵感调度核心 —— 宏内核的「一个」调度意志 `sched/inspiration`。
//!
//! 五个调度域各自跑 MLFQ，但**宏内核不能因此有五个互相割裂的意志**。
//! 本模块把五域收回为同一个核心的五个「相位」(phase)：
//!
//! ```text
//!                 ┌──────────────────────────────┐
//!   sense / act ─►│  Inspiration（唯一调度意志）  │─► 一个 pick 决策
//!                 │  reflex / perception /       │
//!                 │  deliberation / memory /     │
//!                 │  governance = 内部相位       │
//!                 └──────────────────────────────┘
//! ```
//!
//! 对外的变化：
//! * **只有一个通道** `sched/inspiration` 表达调度意志；
//! * **只有一个决策点** `pick()` 跨全部相位统一仲裁；
//! * 五域仍保留（兼容既有 `sched/<域>` 与测试），但降级为内部相位，
//!   不再是五个并列的内核。
//!
//! 一次灵感循环：感知(perceive) → 起念(inspire) → 裁决(pick) → 回响(reflect)。
//!
//! * `sense sched/inspiration`
//! * `act sched/inspiration inspire <任务描述>`
//! * `act sched/inspiration pick`
//! * `act sched/inspiration reflect`
//! * `act sched/inspiration mode <unified|domains>`

use std::sync::{Mutex, OnceLock};

use crate::ai_token::TokenId;
use crate::scheduler_domains::{self, SchedDomain};

/// 调度模式：统一意志 / 退回按域直控（调试用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Unified,
    Domains,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Unified => "unified",
            Mode::Domains => "domains",
        }
    }
}

/// 灵感核心状态。
pub struct Inspiration {
    pub mode: Mode,
    pub cycles: u64,
    pub picks: u64,
    pub last_phase: Option<SchedDomain>,
    pub last_token: Option<TokenId>,
    pub trail: Vec<String>,
}

impl Inspiration {
    fn new() -> Self {
        Self {
            mode: Mode::Unified,
            cycles: 0,
            picks: 0,
            last_phase: None,
            last_token: None,
            trail: Vec::new(),
        }
    }

    pub fn status(&self) -> String {
        let last = match (self.last_phase, self.last_token) {
            (Some(p), Some(t)) => format!("{}/token{}", p.name(), t.0),
            _ => "(尚无裁决)".to_string(),
        };
        let trail = if self.trail.is_empty() {
            "  (空)".to_string()
        } else {
            self.trail
                .iter()
                .rev()
                .take(3)
                .map(|s| format!("  {}", s))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "core=Inspiration mode={} cycles={} picks={} last={}\nphases (内部相位, 非独立内核): {}\nready_total={}\ntrail:\n{}",
            self.mode.name(),
            self.cycles,
            self.picks,
            last,
            scheduler_domains::status_line(),
            scheduler_domains::total_ready(),
            trail
        )
    }

    fn note(&mut self, line: String) {
        self.trail.push(line);
        if self.trail.len() > 20 {
            self.trail.remove(0);
        }
    }

    /// 起念：把一个任务按语义归入某个相位并入队。
    pub fn inspire(&mut self, task: &str) -> Result<String, String> {
        let t = task.trim();
        if t.is_empty() {
            return Err("用法: inspire <任务描述>".to_string());
        }
        let phase = infer_phase(t);
        let token = TokenId::new();
        let prio = phase.default_priority();
        scheduler_domains::enqueue(phase, token, prio, None);
        self.cycles += 1;
        let msg = format!(
            "灵感 #{}: \"{}\" -> 相位 {} (优先级 {}) token={}",
            self.cycles,
            t,
            phase.name(),
            prio.name(),
            token.0
        );
        self.note(msg.clone());
        Ok(msg)
    }

    /// 唯一裁决点：跨所有相位选下一个运行者。
    pub fn pick(&mut self) -> String {
        match scheduler_domains::pick_next_unified() {
            Some((phase, token)) => {
                self.picks += 1;
                self.last_phase = Some(phase);
                self.last_token = Some(token);
                let msg = format!(
                    "裁决 #{}: 相位 {} -> token{} (unified 单一决策点)",
                    self.picks,
                    phase.name(),
                    token.0
                );
                self.note(msg.clone());
                msg
            }
            None => "所有相位队列为空，宏内核无待裁决任务".to_string(),
        }
    }

    /// 回响：观察一次循环的结果。
    pub fn reflect(&self) -> String {
        format!(
            "reflect: {} 次起念, {} 次裁决, 模式 {}, 剩余就绪 {}",
            self.cycles,
            self.picks,
            self.mode.name(),
            scheduler_domains::total_ready()
        )
    }

    pub fn set_mode(&mut self, mode: Mode) -> String {
        self.mode = mode;
        format!("调度模式 -> {} (相位始终由统一核心管理)", mode.name())
    }
}

/// 由任务语义推断它属于哪个相位（也就是"灵感落在哪").
fn infer_phase(task: &str) -> SchedDomain {
    let t = task.to_lowercase();
    if t.contains("irq") || t.contains("中断") || t.contains("fault") || t.contains("唤醒") {
        SchedDomain::Reflex
    } else if t.contains("power") || t.contains("cap") || t.contains("策略") || t.contains("治理") {
        SchedDomain::Governance
    } else if t.contains("mem") || t.contains("snapshot") || t.contains("记忆") || t.contains("历史") {
        SchedDomain::Memory
    } else if t.contains("sense") || t.contains("感知") || t.contains("观察") {
        SchedDomain::Perception
    } else {
        SchedDomain::Deliberation
    }
}

static CORE: OnceLock<Mutex<Inspiration>> = OnceLock::new();

pub fn core() -> &'static Mutex<Inspiration> {
    CORE.get_or_init(|| Mutex::new(Inspiration::new()))
}

/// 通道 `sched/inspiration` 的行动实现。
pub fn act(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let (sub, rest) = match text.split_once(' ') {
        Some((a, b)) => (a, b.trim()),
        None => (text, ""),
    };
    match sub {
        "inspire" => core().lock().unwrap().inspire(rest),
        "pick" => Ok(core().lock().unwrap().pick()),
        "reflect" | "status" | "" => Ok(core().lock().unwrap().status()),
        "mode" => {
            let mode = match rest {
                "domains" => Mode::Domains,
                "unified" | "" => Mode::Unified,
                other => return Err(format!("未知模式 {} (unified|domains)", other)),
            };
            Ok(core().lock().unwrap().set_mode(mode))
        }
        _ => Err("用法: inspire <任务> | pick | reflect | mode <unified|domains>".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_inference() {
        assert_eq!(infer_phase("sense gpu/vram"), SchedDomain::Perception);
        assert_eq!(infer_phase("irq timer"), SchedDomain::Reflex);
        assert_eq!(infer_phase("memory snapshot"), SchedDomain::Memory);
        assert_eq!(infer_phase("power policy"), SchedDomain::Governance);
        assert_eq!(infer_phase("act write"), SchedDomain::Deliberation);
    }

    #[test]
    fn inspire_then_pick_is_one_decision() {
        let mut c = Inspiration::new();
        let r = c.inspire("sense gpu/vram").unwrap();
        assert!(r.contains("perception"), "{}", r);
        assert_eq!(c.cycles, 1);
        let p = c.pick();
        assert!(p.contains("裁决"), "{}", p);
        assert_eq!(c.picks, 1);
        assert_eq!(c.last_phase, Some(SchedDomain::Perception));
    }

    #[test]
    fn reflect_and_mode() {
        let mut c = Inspiration::new();
        assert!(c.reflect().contains("0 次起念"));
        assert!(c.set_mode(Mode::Domains).contains("domains"));
        assert_eq!(c.mode, Mode::Domains);
        assert!(c.status().contains("core=Inspiration"));
    }

    #[test]
    fn act_rejects_empty_inspire() {
        assert!(act("inspire").is_err());
        assert!(act("nope").is_err());
    }
}
