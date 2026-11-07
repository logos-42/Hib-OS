//! 动机通道 —— AI 原生的「为什么想做」`motive://self`。
//!
//! 动机是**内在驱力**，用 0-100 的强度表示。它回答"我倾向于关注什么"，
//! 但**不**负责判断一件事是否可信（那是信念通道），也**不**负责下结论
//! （那是判断力通道）。三者分工：
//!
//! ```text
//! motive://self  我做事的倾向      curiosity / order / safety / efficacy
//! belief://self  我认为世界如何    命题 + 置信度 + 证据
//! judgment://self 我据此怎么裁决    动机强度 × 信念置信度 -> commit/probe/hold
//! ```
//!
//! * `sense motive://self`
//! * `act motive://self urge <动机> <±增量>`
//! * `act motive://self set <动机> <0-100>`
//! * `act motive://self reset`

use std::sync::{Mutex, OnceLock};

/// 一种内在动机。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Motive {
    pub name: String,
    /// 0-100 的强度。
    pub level: u8,
}

impl Motive {
    /// 十格强度条。
    pub fn bar(&self) -> String {
        let filled = (self.level / 10) as usize;
        format!("{}{}", "#".repeat(filled), "-".repeat(10 - filled))
    }
}

/// 动机集合。
pub struct Motives {
    pub list: Vec<Motive>,
    /// 最近若干次调整记录。
    pub history: Vec<String>,
}

impl Motives {
    fn new() -> Self {
        let m = |name: &str, level: u8| Motive {
            name: name.to_string(),
            level,
        };
        Self {
            list: vec![
                m("curiosity", 70),
                m("order", 60),
                m("safety", 55),
                m("efficacy", 50),
            ],
            history: Vec::new(),
        }
    }

    /// 感知快照。
    pub fn status(&self) -> String {
        let bars = self
            .list
            .iter()
            .map(|m| format!("  {:<10} [{}] {}", m.name, m.bar(), m.level))
            .collect::<Vec<_>>()
            .join("\n");
        let dom = match self.dominant() {
            Some((name, level)) => format!("{}({})", name, level),
            None => "(无)".to_string(),
        };
        let recent = if self.history.is_empty() {
            "(无)".to_string()
        } else {
            self.history.iter().rev().take(3).cloned().collect::<Vec<_>>().join("; ")
        };
        format!(
            "motives:\n{}\ndominant: {}\nadjustments: {}\nrecent: {}",
            bars,
            dom,
            self.history.len(),
            recent
        )
    }

    fn index_of(&self, name: &str) -> Result<usize, String> {
        self.list.iter().position(|m| m.name == name).ok_or_else(|| {
            let names: Vec<&str> = self.list.iter().map(|m| m.name.as_str()).collect();
            format!("无此动机 {} (可选: {})", name, names.join("|"))
        })
    }

    /// 按增量调整（自动裁剪到 0-100）。
    pub fn urge(&mut self, name: &str, delta: i32) -> Result<String, String> {
        let i = self.index_of(name)?;
        let next = (self.list[i].level as i32 + delta).clamp(0, 100) as u8;
        self.list[i].level = next;
        let msg = format!("动机 {} {:+} -> {}", name, delta, next);
        self.history.push(msg.clone());
        self.trim_history();
        Ok(msg)
    }

    /// 绝对设定强度。
    pub fn set(&mut self, name: &str, value: u8) -> Result<String, String> {
        if value > 100 {
            return Err("强度必须在 0-100".to_string());
        }
        let i = self.index_of(name)?;
        self.list[i].level = value;
        let msg = format!("动机 {} = {}", name, value);
        self.history.push(msg.clone());
        self.trim_history();
        Ok(msg)
    }

    /// 恢复默认强度。
    pub fn reset(&mut self) -> String {
        *self = Motives::new();
        "动机已恢复默认".to_string()
    }

    /// 最强动机。
    pub fn dominant(&self) -> Option<(String, u8)> {
        self.list
            .iter()
            .max_by_key(|m| m.level)
            .map(|m| (m.name.clone(), m.level))
    }

    /// 查询单一动机强度。
    pub fn level(&self, name: &str) -> Option<u8> {
        self.list.iter().find(|m| m.name == name).map(|m| m.level)
    }

    fn trim_history(&mut self) {
        if self.history.len() > 10 {
            self.history.remove(0);
        }
    }
}

static MOTIVES: OnceLock<Mutex<Motives>> = OnceLock::new();

/// 全局动机集合（`motive://self` 的实体）。
pub fn motives() -> &'static Mutex<Motives> {
    MOTIVES.get_or_init(|| Mutex::new(Motives::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_status() {
        let m = Motives::new();
        assert_eq!(m.list.len(), 4);
        assert_eq!(m.dominant(), Some(("curiosity".to_string(), 70)));
        let s = m.status();
        assert!(s.contains("curiosity"), "{}", s);
        assert!(s.contains("adjustments: 0"), "{}", s);
    }

    #[test]
    fn urge_clamps_and_records() {
        let mut m = Motives::new();
        m.urge("safety", 100).unwrap();
        assert_eq!(m.level("safety"), Some(100));
        m.urge("safety", -200).unwrap();
        assert_eq!(m.level("safety"), Some(0));
        assert_eq!(m.history.len(), 2);
        assert!(m.urge("unknown", 1).is_err());
    }

    #[test]
    fn set_and_reset() {
        let mut m = Motives::new();
        m.set("order", 90).unwrap();
        assert_eq!(m.dominant(), Some(("order".to_string(), 90)));
        assert!(m.set("order", 101).is_err());
        m.reset();
        assert_eq!(m.level("order"), Some(60));
    }

    #[test]
    fn history_is_bounded() {
        let mut m = Motives::new();
        for _ in 0..15 {
            m.urge("curiosity", 1).unwrap();
        }
        assert_eq!(m.history.len(), 10);
    }
}
