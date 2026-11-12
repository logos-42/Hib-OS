//! L3 系统休眠 —— `power://hibernate`。
//!
//! L1 = 执行体 sleep（`exec::block_on`）、L2 = 子意识冻结（`cell://self freeze`），
//! 本模块是 L3：把**全部子意识 + 世界版本**写成一份镜像，能从磁盘重建。
//!
//! 与 Linux suspend-to-disk 的差别：镜像里保存的不是寄存器，而是
//! **意识状态**（动机 / 信念 / 裁决历史 / 冻结标志 / 血缘）与 `epoch`。
//! 恢复时比对当前世界版本，就能知道"睡下去之后世界变了没有"。
//!
//! * `sense power://hibernate`
//! * `act power://hibernate save [路径]`
//! * `act power://hibernate restore [路径]`
//! * `act power://hibernate snapshot`

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::cell::{self, SubConscious};
use crate::lease;

const MAGIC: &str = "#hibos-hibernate v1";

/// 休眠记录（统计用）。
pub struct Hibernation {
    pub saves: u64,
    pub restores: u64,
    pub last_saved_epoch: u64,
    pub last_path: String,
    pub last_report: String,
}

static STATE: OnceLock<Mutex<Hibernation>> = OnceLock::new();

fn state() -> &'static Mutex<Hibernation> {
    STATE.get_or_init(|| {
        Mutex::new(Hibernation {
            saves: 0,
            restores: 0,
            last_saved_epoch: 0,
            last_path: default_path().to_string_lossy().to_string(),
            last_report: "（尚无休眠记录）".to_string(),
        })
    })
}

pub fn default_path() -> PathBuf {
    std::env::temp_dir().join("hibos_hibernate.img")
}

/// 把当前全部意识状态导出为纯文本镜像。
///
/// 每行用 `\t` 分栏，避免中文命题里的空格破坏解析。
pub fn snapshot() -> String {
    let mut out = vec![MAGIC.to_string()];
    out.push(format!("epoch\t{}", lease::epoch("cell://self")));
    let m = cell::subs().lock().unwrap();
    for c in m.values() {
        out.push(format!(
            "cell\t{}\t{}\t{}\t{}",
            c.id, c.parent, c.frozen as u8, c.merges
        ));
        for mo in &c.motives.list {
            out.push(format!("motive\t{}\t{}\t{}", c.id, mo.name, mo.level));
        }
        for b in &c.beliefs.list {
            out.push(format!(
                "belief\t{}\t{}\t{}\t{}\t{}\t{}",
                c.id, b.proposition, b.confidence, b.evidence, b.supports, b.refutes
            ));
        }
        for v in &c.judgment.history {
            out.push(format!(
                "verdict\t{}\t{}\t{}\t{}",
                c.id, v.proposition, v.score, v.verdict
            ));
        }
    }
    out.join("\n")
}

/// 从镜像文本重建意识状态。
pub fn restore_text(text: &str) -> Result<String, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some(MAGIC) {
        return Err("不是有效的 Hib OS 休眠镜像".to_string());
    }
    let mut rebuilt: std::collections::BTreeMap<String, SubConscious> = Default::default();
    let mut saved_epoch = 0u64;
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        match f.first().copied() {
            Some("epoch") => saved_epoch = f.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            Some("cell") if f.len() >= 5 => {
                let id = f[1];
                let mut c = SubConscious::new(id, f[2]);
                c.frozen = f[3] == "1";
                c.frozen_epoch = saved_epoch;
                c.merges = f[4].parse().unwrap_or(0);
                // 清空默认信念，完全以镜像为准
                c.beliefs.list.clear();
                rebuilt.insert(id.to_string(), c);
            }
            Some("motive") if f.len() >= 4 => {
                if let Some(c) = rebuilt.get_mut(f[1]) {
                    if let Some(mo) = c.motives.list.iter_mut().find(|m| m.name == f[2]) {
                        mo.level = f[3].parse().unwrap_or(mo.level);
                    }
                }
            }
            Some("belief") if f.len() >= 7 => {
                if let Some(c) = rebuilt.get_mut(f[1]) {
                    c.beliefs.list.push(crate::belief::Belief {
                        proposition: f[2].to_string(),
                        confidence: f[3].parse().unwrap_or(0),
                        evidence: f[4].parse().unwrap_or(0),
                        supports: f[5].parse().unwrap_or(0),
                        refutes: f[6].parse().unwrap_or(0),
                    });
                }
            }
            Some("verdict") if f.len() >= 5 => {
                if let Some(c) = rebuilt.get_mut(f[1]) {
                    c.judgment.history.push(crate::judgment::Verdict {
                        proposition: f[2].to_string(),
                        score: f[3].parse().unwrap_or(0),
                        verdict: f[4].to_string(),
                        reason: "(从休眠镜像恢复)".to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    *cell::subs().lock().unwrap() = rebuilt;
    let now = lease::epoch("cell://self");
    let delta = now.saturating_sub(saved_epoch);
    let report = format!(
        "已恢复 {} 个子意识; 休眠时 epoch={} 现在 epoch={} (世界变化 {} 版)",
        cell::subs().lock().unwrap().len(),
        saved_epoch,
        now,
        delta
    );
    let mut s = state().lock().unwrap();
    s.restores += 1;
    s.last_report = report.clone();
    Ok(report)
}

/// 保存镜像到磁盘。
pub fn save(path: &Path) -> Result<String, String> {
    let text = snapshot();
    std::fs::write(path, &text).map_err(|e| format!("写镜像失败: {}", e))?;
    let mut s = state().lock().unwrap();
    s.saves += 1;
    s.last_saved_epoch = lease::epoch("cell://self");
    s.last_path = path.to_string_lossy().to_string();
    let r = format!("已保存意识镜像到 {} ({} 字节)", path.display(), text.len());
    s.last_report = r.clone();
    Ok(r)
}

/// 从磁盘恢复镜像。
pub fn restore(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读镜像失败: {}", e))?;
    let r = restore_text(&text)?;
    state().lock().unwrap().last_path = path.to_string_lossy().to_string();
    Ok(r)
}

pub fn status() -> String {
    let s = state().lock().unwrap();
    format!(
        "saves={} restores={} last_epoch={}\npath={}\nlast: {}\ncells={}",
        s.saves,
        s.restores,
        s.last_saved_epoch,
        s.last_path,
        s.last_report,
        cell::subs().lock().unwrap().len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_restores_cells_and_beliefs() {
        let _g = crate::cell::test_guard();
        {
            let mut m = cell::subs().lock().unwrap();
            m.clear();
            let mut c = SubConscious::new("dreamer", cell::ROOT);
            c.beliefs.list.clear();
            c.beliefs.believe("外部网络可达", 33).unwrap();
            c.motives.set("safety", 99).unwrap();
            m.insert("dreamer".into(), c);
        }
        let img = snapshot();
        assert!(img.contains("dreamer"));
        assert!(img.contains("外部网络可达"));

        // 清空后从镜像重建
        cell::subs().lock().unwrap().clear();
        let report = restore_text(&img).unwrap();
        assert!(report.contains("1 个子意识"), "{}", report);
        let m = cell::subs().lock().unwrap();
        let c = m.get("dreamer").unwrap();
        assert_eq!(c.beliefs.confidence("外部网络可达"), Some(33));
        assert_eq!(c.motives.level("safety"), Some(99));
        drop(m);
        cell::subs().lock().unwrap().clear();
    }

    #[test]
    fn rejects_bad_image() {
        assert!(restore_text("not an image").is_err());
    }

    #[test]
    fn save_and_restore_roundtrip() {
        let _g = crate::cell::test_guard();
        let p = std::env::temp_dir().join("hibos_hibernate_test.img");
        assert!(save(&p).is_ok());
        assert!(restore(&p).is_ok());
        let _ = std::fs::remove_file(&p);
    }
}
