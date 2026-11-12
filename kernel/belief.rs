//! 信念通道 —— AI 原生的「我认为世界如何」`belief://self`。
//!
//! 一条信念 = **一个可证伪的命题** + **置信度 0-100** + **证据计数**。
//! 信念不产生行动，只描述世界；判断力通道会读取它来做裁决。
//!
//! * `sense belief://self`
//! * `act belief://self believe <命题> <置信度 0-100>`
//! * `act belief://self observe <命题> <±增量>`（新增一条支持/反驳证据）
//! * `act belief://self retract <命题>`
//! * `act belief://self weak [阈值]`

use std::sync::{Mutex, OnceLock};

/// 一条信念。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Belief {
    pub proposition: String,
    /// 0-100 的置信度。
    pub confidence: u8,
    /// 证据总条数。
    pub evidence: u32,
    /// 支持 / 反驳证据条数。
    pub supports: u32,
    pub refutes: u32,
}

/// 信念集合。
#[derive(Clone, Debug)]
pub struct Beliefs {
    pub list: Vec<Belief>,
}

impl Beliefs {
    pub fn new() -> Self {
        let b = |p: &str, c: u8| Belief {
            proposition: p.to_string(),
            confidence: c,
            evidence: 0,
            supports: 0,
            refutes: 0,
        };
        Self {
            list: vec![
                b("GPU 显存可用", 80),
                b("外部网络可达", 50),
                b("宿主存储可写", 75),
            ],
        }
    }

    /// 感知快照。
    pub fn status(&self) -> String {
        let lines = self
            .list
            .iter()
            .map(|b| {
                format!(
                    "  {:<22} 置信 {}% 证据 {} (支持 {} / 反驳 {})",
                    b.proposition, b.confidence, b.evidence, b.supports, b.refutes
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mean = self.mean_confidence();
        let weak = match self.weakest() {
            Some((p, c)) => format!("{}({}%)", p, c),
            None => "(无)".to_string(),
        };
        format!(
            "beliefs:\n{}\nmean_confidence: {}%\nweakest: {}\ncount: {}",
            lines,
            mean,
            weak,
            self.list.len()
        )
    }

    /// 设定 / 新增信念。
    pub fn believe(&mut self, proposition: &str, confidence: u8) -> Result<String, String> {
        let p = proposition.trim();
        if p.is_empty() {
            return Err("命题不能为空".to_string());
        }
        if confidence > 100 {
            return Err("置信度必须在 0-100".to_string());
        }
        match self.list.iter_mut().find(|b| b.proposition == p) {
            Some(b) => {
                b.confidence = confidence;
                Ok(format!("信念 \"{}\" 置信度 -> {}%", p, confidence))
            }
            None => {
                self.list.push(Belief {
                    proposition: p.to_string(),
                    confidence,
                    evidence: 0,
                    supports: 0,
                    refutes: 0,
                });
                Ok(format!("新信念 \"{}\" 置信度 {}%", p, confidence))
            }
        }
    }

    /// 用一条新证据调整置信度。
    pub fn observe(&mut self, proposition: &str, delta: i32) -> Result<String, String> {
        if delta == 0 {
            return Err("证据增量不能为 0".to_string());
        }
        let p = proposition.trim();
        let b = self
            .list
            .iter_mut()
            .find(|b| b.proposition == p)
            .ok_or_else(|| format!("无此信念 \"{}\" (先用 believe 建立)", p))?;
        b.confidence = (b.confidence as i32 + delta).clamp(0, 100) as u8;
        b.evidence += 1;
        if delta > 0 {
            b.supports += 1;
        } else {
            b.refutes += 1;
        }
        Ok(format!(
            "信念 \"{}\" 置信度 -> {}% (证据 {}: 支持 {} / 反驳 {})",
            p, b.confidence, b.evidence, b.supports, b.refutes
        ))
    }

    /// 撤销一条信念。
    pub fn retract(&mut self, proposition: &str) -> Result<String, String> {
        let p = proposition.trim();
        let before = self.list.len();
        self.list.retain(|b| b.proposition != p);
        if self.list.len() == before {
            Err(format!("无此信念 \"{}\"", p))
        } else {
            Ok(format!("已撤销信念 \"{}\"", p))
        }
    }

    /// 查询置信度。
    pub fn confidence(&self, proposition: &str) -> Option<u8> {
        self.list
            .iter()
            .find(|b| b.proposition == proposition.trim())
            .map(|b| b.confidence)
    }

    /// 最低置信的信念。
    pub fn weakest(&self) -> Option<(String, u8)> {
        self.list
            .iter()
            .min_by_key(|b| b.confidence)
            .map(|b| (b.proposition.clone(), b.confidence))
    }

    /// 低于阈值的信念。
    pub fn below(&self, threshold: u8) -> Vec<(String, u8)> {
        self.list
            .iter()
            .filter(|b| b.confidence < threshold)
            .map(|b| (b.proposition.clone(), b.confidence))
            .collect()
    }

    /// 平均置信度。
    pub fn mean_confidence(&self) -> u8 {
        if self.list.is_empty() {
            return 0;
        }
        let sum: u32 = self.list.iter().map(|b| b.confidence as u32).sum();
        (sum / self.list.len() as u32) as u8
    }
}

static BELIEFS: OnceLock<Mutex<Beliefs>> = OnceLock::new();

/// 全局信念集合（`belief://self` 的实体）。
pub fn beliefs() -> &'static Mutex<Beliefs> {
    BELIEFS.get_or_init(|| Mutex::new(Beliefs::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_status() {
        let b = Beliefs::new();
        assert_eq!(b.list.len(), 3);
        assert_eq!(b.confidence("GPU 显存可用"), Some(80));
        assert_eq!(b.weakest(), Some(("外部网络可达".to_string(), 50)));
        assert!(b.status().contains("mean_confidence"), "{}", b.status());
    }

    #[test]
    fn believe_updates_or_inserts() {
        let mut b = Beliefs::new();
        b.believe("GPU 显存可用", 95).unwrap();
        assert_eq!(b.list.len(), 3);
        let r = b.believe("模型已加载", 60).unwrap();
        assert!(r.contains("新信念"), "{}", r);
        assert_eq!(b.list.len(), 4);
        assert!(b.believe("  ", 50).is_err());
        assert!(b.believe("x", 101).is_err());
    }

    #[test]
    fn observe_tracks_support_and_refute() {
        let mut b = Beliefs::new();
        b.observe("外部网络可达", 10).unwrap();
        b.observe("外部网络可达", -30).unwrap();
        let e = b.list.iter().find(|x| x.proposition == "外部网络可达").unwrap();
        assert_eq!(e.confidence, 30);
        assert_eq!(e.evidence, 2);
        assert_eq!(e.supports, 1);
        assert_eq!(e.refutes, 1);
        assert!(b.observe("不存在的命题", 1).is_err());
        assert!(b.observe("外部网络可达", 0).is_err());
    }

    #[test]
    fn clamp_and_retract() {
        let mut b = Beliefs::new();
        b.observe("GPU 显存可用", 100).unwrap();
        assert_eq!(b.confidence("GPU 显存可用"), Some(100));
        b.observe("GPU 显存可用", -200).unwrap();
        assert_eq!(b.confidence("GPU 显存可用"), Some(0));

        assert!(b.retract("GPU 显存可用").is_ok());
        assert!(b.retract("GPU 显存可用").is_err());
    }

    #[test]
    fn below_and_mean() {
        let b = Beliefs::new();
        let weak = b.below(60);
        assert_eq!(weak.len(), 1);
        assert_eq!(weak[0].0, "外部网络可达");
        // (80 + 50 + 75) / 3 = 68
        assert_eq!(b.mean_confidence(), 68);
    }
}
