//! 判断力通道 —— AI 原生的「据此怎么裁决」`judgment/self`。
//!
//! 前两个通道各管一半，本通道把它们**组合起来**产生结论：
//!
//! ```text
//! 判断分 = 信念置信度 × 0.6 + 主导动机强度 × 0.4
//!   >= 70  commit  可以执行
//!   40-69  probe   证据不足，先验证
//!   < 40   hold    暂缓
//! ```
//!
//! 也就是说：**动机决定有多想做，信念决定有多大把握，判断力决定做不做。**
//! 本通道只做裁决，不修改动机与信念（那是它们各自的职责）。
//!
//! * `sense judgment/self`
//! * `act judgment/self judge [命题]`（省略命题时自动挑最弱信念）
//! * `act judgment/self decide`（自主判断，结果同样计入统计）

use std::sync::{Mutex, OnceLock};

use crate::belief::{beliefs, Beliefs};
use crate::motive::{motives, Motives};

/// 一次裁决记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub proposition: String,
    pub score: u8,
    /// commit / probe / hold
    pub verdict: String,
    pub reason: String,
}

/// 判断力状态。
pub struct Judgment {
    pub history: Vec<Verdict>,
    pub commits: u64,
    pub probes: u64,
    pub holds: u64,
}

impl Judgment {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            commits: 0,
            probes: 0,
            holds: 0,
        }
    }

    /// 感知快照：策略、统计、最近裁决。
    pub fn status(&self) -> String {
        let total = self.commits + self.probes + self.holds;
        let last = match self.history.last() {
            Some(v) => format!("\"{}\" score={} verdict={}", v.proposition, v.score, v.verdict),
            None => "(尚未裁决)".to_string(),
        };
        let recent = if self.history.is_empty() {
            "  (无)".to_string()
        } else {
            self.history
                .iter()
                .rev()
                .take(3)
                .map(|v| format!("  \"{}\" {} {}", v.proposition, v.score, v.verdict))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "policy: commit>=70, probe 40-69, hold<40\ntotal: {} (commit {} / probe {} / hold {})\nlast: {}\nrecent:\n{}",
            total, self.commits, self.probes, self.holds, last, recent
        )
    }

    /// 默认视图裁决（root Cell）：读取全局动机与信念。
    pub fn judge(&mut self, proposition: Option<&str>) -> Result<String, String> {
        let mot = motives().lock().unwrap();
        let bel = beliefs().lock().unwrap();
        self.judge_with(&mot, &bel, proposition)
    }

    /// 用**指定的**动机 / 信念视图裁决。
    ///
    /// 这是 per-Cell 判断力的关键：子意识主体用自己的世界观判断，
    /// 不会读到父体或其它子体的信念。
    pub fn judge_with(
        &mut self,
        mot: &Motives,
        bel: &Beliefs,
        proposition: Option<&str>,
    ) -> Result<String, String> {
        let (prop, conf, known) = match proposition.map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => {
                let c = bel.confidence(p);
                (p.to_string(), c.unwrap_or(50), c.is_some())
            }
            None => match bel.weakest() {
                Some((p, c)) => (p.to_string(), c, true),
                None => {
                    return Err("没有可用信念，先走 act belief/self believe <命题> <置信度>".to_string());
                }
            },
        };

        let (mname, mlevel) = mot.dominant().ok_or_else(|| "没有可用动机".to_string())?;

        let (score, verdict) = evaluate(conf, mlevel);
        let mut reason = format!(
            "信念置信 {}% ×0.6 + 主导动机 {}({}) ×0.4 = {}",
            conf, mname, mlevel, score
        );
        if !known {
            reason.push_str(" (未建立的命题，按中性 50% 处理)");
        }

        let advice = match verdict {
            "commit" => "可以执行，无需额外验证".to_string(),
            "probe" => format!("建议先 act belief/self observe \"{}\" +10 补充证据", prop),
            _ => format!(
                "建议先 act belief/self observe \"{}\" +20，或提升 motive/self 的相关动机",
                prop
            ),
        };
        self.record(&prop, score, verdict, &reason);

        Ok(format!(
            "judge \"{}\": score={} verdict={}\nreason: {}\n建议: {}",
            prop, score, verdict, reason, advice
        ))
    }

    /// 自主判断：自动挑最弱的信念来裁决。
    pub fn decide(&mut self) -> Result<String, String> {
        let out = self.judge(None)?;
        Ok(format!("自主判断:\n{}", out))
    }

    fn record(&mut self, prop: &str, score: u8, verdict: &str, reason: &str) {
        match verdict {
            "commit" => self.commits += 1,
            "probe" => self.probes += 1,
            _ => self.holds += 1,
        }
        self.history.push(Verdict {
            proposition: prop.to_string(),
            score,
            verdict: verdict.to_string(),
            reason: reason.to_string(),
        });
        if self.history.len() > 10 {
            self.history.remove(0);
        }
    }
}

/// 纯函数：由信念置信度与动机强度算出判断分与裁决。
///
/// 拆出来是为了能独立测试阈值边界，不受全局状态影响。
pub fn evaluate(confidence: u8, motive_level: u8) -> (u8, &'static str) {
    let score = ((confidence as u32 * 6) + (motive_level as u32 * 4)) / 10;
    let verdict = if score >= 70 {
        "commit"
    } else if score >= 40 {
        "probe"
    } else {
        "hold"
    };
    (score as u8, verdict)
}

static JUDGMENT: OnceLock<Mutex<Judgment>> = OnceLock::new();

/// 全局判断力（`judgment/self` 的实体）。
pub fn judgment() -> &'static Mutex<Judgment> {
    JUDGMENT.get_or_init(|| Mutex::new(Judgment::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_thresholds() {
        // 100% 信念 + 100 动机 = 100 -> commit
        assert_eq!(evaluate(100, 100), (100, "commit"));
        // 70% + 70 = 70 -> commit（边界）
        assert_eq!(evaluate(70, 70), (70, "commit"));
        // 50% + 70 = 58 -> probe
        assert_eq!(evaluate(50, 70), (58, "probe"));
        // 0% + 100 = 40 -> probe（边界）
        assert_eq!(evaluate(0, 100), (40, "probe"));
        // 0% + 50 = 20 -> hold
        assert_eq!(evaluate(0, 50), (20, "hold"));
    }

    #[test]
    fn judge_records_and_counts() {
        let mut j = Judgment::new();
        let r = j.judge(Some("GPU 显存可用")).unwrap();
        assert!(r.contains("verdict="), "{}", r);
        assert!(r.contains("reason:"), "{}", r);
        assert_eq!(j.history.len(), 1);
        assert_eq!(j.commits + j.probes + j.holds, 1);
        assert!(j.status().contains("total: 1"), "{}", j.status());
    }

    #[test]
    fn judge_without_proposition_picks_weakest() {
        let mut j = Judgment::new();
        let r = j.judge(None).unwrap();
        assert!(r.contains("judge"), "{}", r);
        assert_eq!(j.history.len(), 1);
    }

    #[test]
    fn unknown_proposition_uses_neutral_confidence() {
        let mut j = Judgment::new();
        let r = j.judge(Some("从未建立过的命题")).unwrap();
        assert!(r.contains("中性 50%"), "{}", r);
    }

    #[test]
    fn decide_and_history_bound() {
        let mut j = Judgment::new();
        let r = j.decide().unwrap();
        assert!(r.starts_with("自主判断:"), "{}", r);
        for _ in 0..12 {
            j.judge(Some("宿主存储可写")).unwrap();
        }
        assert_eq!(j.history.len(), 10);
    }

    #[test]
    fn status_before_any_verdict() {
        let j = Judgment::new();
        let s = j.status();
        assert!(s.contains("total: 0"), "{}", s);
        assert!(s.contains("(尚未裁决)"), "{}", s);
    }
}
