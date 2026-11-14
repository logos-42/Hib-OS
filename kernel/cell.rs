//! 子 AI 意识主体 (Cell) —— `cell/self`。
//!
//! 之前意志三通道（`motive/self` / `belief/self` / `judgment/self`）是
//! **全局单例**，所有智能体共享一套动机与信念 —— 这等于"所有子意识共用一个脑子"。
//! 本模块把它们变成 **per-Cell 的私有视图**：
//!
//! * `root` Cell —— 就是原来的三条全局通道，保持完全兼容；
//! * 子 Cell —— 各自持有独立的 `Motives` / `Beliefs` / `Judgment`，有血缘（parent）；
//! * `judge` 用 `judge_with(自己的动机, 自己的信念)`，不会串味；
//! * 子体退出时用 `merge` 把信念作为**证言**按证据加权合并给父体；
//! * `freeze` / `thaw` 是 L2 睡眠：冻结期间拒绝写入，唤醒时报告
//!   "离开期间世界变了几个 epoch"，让醒来的意识决定要不要重新感知。
//!
//! * `sense cell/self`
//! * `act cell/self create <id> [parent]`
//! * `act cell/self urge|believe|observe <id> ...`
//! * `act cell/self judge <id> [命题]`
//! * `act cell/self merge <child> [parent]`
//! * `act cell/self freeze|thaw <id>` / `show <id>` / `remove <id>`

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::belief::{beliefs, Beliefs};
use crate::judgment::{judgment, Judgment};
use crate::lease;
use crate::motive::{motives, Motives};

/// 根意识的名字；root 复用全局意志三通道。
pub const ROOT: &str = "root";

/// 一个子意识主体。
pub struct SubConscious {
    pub id: String,
    pub parent: String,
    pub motives: Motives,
    pub beliefs: Beliefs,
    pub judgment: Judgment,
    /// 语义冻结：冻结期间所有写操作被拒绝。
    pub frozen: bool,
    /// 冻结时的世界版本。
    pub frozen_epoch: u64,
    /// 证言被合并的次数。
    pub merges: u64,
}

impl SubConscious {
    pub fn new(id: &str, parent: &str) -> Self {
        Self {
            id: id.to_string(),
            parent: parent.to_string(),
            motives: Motives::new(),
            beliefs: Beliefs::new(),
            judgment: Judgment::new(),
            frozen: false,
            frozen_epoch: 0,
            merges: 0,
        }
    }

    fn view(&self) -> String {
        format!(
            "cell {} (parent={}) frozen={}\n{}\n{}\n{}",
            self.id,
            self.parent,
            self.frozen,
            self.motives.status(),
            self.beliefs.status(),
            self.judgment.status()
        )
    }
}

static SUBS: OnceLock<Mutex<BTreeMap<String, SubConscious>>> = OnceLock::new();

/// 全部子意识主体（不含 root，root 是全局三通道）。
pub fn subs() -> &'static Mutex<BTreeMap<String, SubConscious>> {
    SUBS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 总体摘要。
pub fn status() -> String {
    let m = subs().lock().unwrap();
    let mut out = format!(
        "root: 全局意志三通道 (motive/self / belief/self / judgment/self)\ncells: {} 个",
        m.len()
    );
    for c in m.values() {
        out.push_str(&format!(
            "\n  {:<12} parent={:<10} frozen={:<5} beliefs={} merges={}",
            c.id,
            c.parent,
            c.frozen,
            c.beliefs.list.len(),
            c.merges
        ));
    }
    out
}

/// 查看某个主体的完整意识视图。
pub fn show(id: &str) -> Result<String, String> {
    if id == ROOT {
        return Ok(format!(
            "cell root (全局意志)\n{}\n{}\n{}",
            motives().lock().unwrap().status(),
            beliefs().lock().unwrap().status(),
            judgment().lock().unwrap().status()
        ));
    }
    let m = subs().lock().unwrap();
    let c = m.get(id).ok_or_else(|| format!("子意识 {} 不存在", id))?;
    Ok(c.view())
}

/// 创建子意识。
pub fn create(id: &str, parent: &str) -> Result<String, String> {
    if id.is_empty() || id == ROOT {
        return Err("子意识名不能为空，也不能叫 root".to_string());
    }
    let mut m = subs().lock().unwrap();
    if m.contains_key(id) {
        return Err(format!("子意识 {} 已存在", id));
    }
    if parent != ROOT && !m.contains_key(parent) {
        return Err(format!("父意识 {} 不存在", parent));
    }
    m.insert(id.to_string(), SubConscious::new(id, parent));
    Ok(format!("已创建子意识 {} (parent={})", id, parent))
}

/// 移除子意识（还有子体时拒绝）。
pub fn remove(id: &str) -> Result<String, String> {
    let mut m = subs().lock().unwrap();
    if !m.contains_key(id) {
        return Err(format!("子意识 {} 不存在", id));
    }
    if m.values().any(|c| c.parent == id) {
        return Err(format!("{} 还有子意识，先移除后代", id));
    }
    m.remove(id);
    Ok(format!("已移除子意识 {}", id))
}

/// 取一个可写的子意识；冻结时拒绝。
fn writable<'a>(
    m: &'a mut BTreeMap<String, SubConscious>,
    id: &str,
) -> Result<&'a mut SubConscious, String> {
    let c = m.get_mut(id).ok_or_else(|| format!("子意识 {} 不存在", id))?;
    if c.frozen {
        return Err(format!("子意识 {} 处于冻结状态，先 thaw", id));
    }
    Ok(c)
}

pub fn urge(id: &str, motive: &str, delta: i32) -> Result<String, String> {
    if id == ROOT {
        return motives().lock().unwrap().urge(motive, delta);
    }
    let mut m = subs().lock().unwrap();
    writable(&mut m, id)?.motives.urge(motive, delta)
}

pub fn believe(id: &str, prop: &str, conf: u8) -> Result<String, String> {
    if id == ROOT {
        return beliefs().lock().unwrap().believe(prop, conf);
    }
    let mut m = subs().lock().unwrap();
    writable(&mut m, id)?.beliefs.believe(prop, conf)
}

pub fn observe(id: &str, prop: &str, delta: i32) -> Result<String, String> {
    if id == ROOT {
        return beliefs().lock().unwrap().observe(prop, delta);
    }
    let mut m = subs().lock().unwrap();
    writable(&mut m, id)?.beliefs.observe(prop, delta)
}

/// 用**该主体自己的**动机与信念裁决。
pub fn judge(id: &str, prop: Option<&str>) -> Result<String, String> {
    if id == ROOT {
        return judgment().lock().unwrap().judge(prop);
    }
    let mut m = subs().lock().unwrap();
    let c = writable(&mut m, id)?;
    c.judgment.judge_with(&c.motives, &c.beliefs, prop)
}

/// 把 `source` 的信念按证据权重合并进 `target`（证言合并）。
///
/// 同名命题：`new = (父置信×父权重 + 子置信×子权重) / (父权重+子权重)`，
/// 权重 = `证据数 + 1`；新命题直接复制。
fn merge_beliefs(target: &mut Beliefs, source: &Beliefs) -> String {
    let mut lines = Vec::new();
    for src in &source.list {
        let w_src = src.evidence as u64 + 1;
        match target.list.iter_mut().find(|b| b.proposition == src.proposition) {
            Some(dst) => {
                let w_dst = dst.evidence as u64 + 1;
                let before = dst.confidence;
                dst.confidence = ((before as u64 * w_dst + src.confidence as u64 * w_src)
                    / (w_dst + w_src)) as u8;
                dst.evidence += src.evidence;
                dst.supports += src.supports;
                dst.refutes += src.refutes;
                lines.push(format!("{}: {}% -> {}%", dst.proposition, before, dst.confidence));
            }
            None => {
                lines.push(format!("{}: 新增 {}%", src.proposition, src.confidence));
                target.list.push(src.clone());
            }
        }
    }
    if lines.is_empty() {
        "（子意识无信念可合并）".to_string()
    } else {
        lines.join("; ")
    }
}

/// 子体退出：把证言合并给父体，并累加合并次数。
pub fn merge(child_id: &str, parent: Option<&str>) -> Result<String, String> {
    let mut m = subs().lock().unwrap();
    let child = m
        .get(child_id)
        .ok_or_else(|| format!("子意识 {} 不存在", child_id))?;
    if child.frozen {
        return Err(format!("子意识 {} 处于冻结状态，先 thaw", child_id));
    }
    let target = parent.unwrap_or(&child.parent).to_string();
    if target == child_id {
        return Err("不能把证言合并给自己".to_string());
    }
    let source = child.beliefs.clone();

    let report = if target == ROOT {
        // 先释放 subs 锁再拿全局信念锁，避免与别的路径形成锁环。
        drop(m);
        merge_beliefs(&mut beliefs().lock().unwrap(), &source)
    } else {
        let dst = m
            .get_mut(&target)
            .ok_or_else(|| format!("父意识 {} 不存在", target))?;
        let r = merge_beliefs(&mut dst.beliefs, &source);
        dst.merges += 1;
        r
    };
    Ok(format!("{} 的证言已合并给 {}: {}", child_id, target, report))
}

/// L2 睡眠：冻结子意识，记录当时的世界版本。
pub fn freeze(id: &str) -> Result<String, String> {
    if id == ROOT {
        return Err("root 不能冻结（请用 power/state）".to_string());
    }
    let mut m = subs().lock().unwrap();
    let c = m.get_mut(id).ok_or_else(|| format!("子意识 {} 不存在", id))?;
    c.frozen = true;
    c.frozen_epoch = lease::epoch("cell/self");
    Ok(format!(
        "子意识 {} 已冻结 (world epoch={})",
        id, c.frozen_epoch
    ))
}

/// L2 唤醒：解冻并报告离开期间世界变了多少版。
pub fn thaw(id: &str) -> Result<String, String> {
    if id == ROOT {
        return Err("root 未冻结".to_string());
    }
    let mut m = subs().lock().unwrap();
    let c = m.get_mut(id).ok_or_else(|| format!("子意识 {} 不存在", id))?;
    if !c.frozen {
        return Err(format!("子意识 {} 未冻结", id));
    }
    c.frozen = false;
    let now = lease::epoch("cell/self");
    let delta = now.saturating_sub(c.frozen_epoch);
    let hint = if delta == 0 {
        "世界未变，可沿用冻结前的感知".to_string()
    } else {
        format!("世界已变化 {} 版，建议重新 sense 后再行动", delta)
    };
    Ok(format!("子意识 {} 已唤醒: {}", id, hint))
}

/// 测试专用串行锁：任何操作全局子意识注册表的测试都先取它。
#[cfg(test)]
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static G: OnceLock<Mutex<()>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(())).lock().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        test_guard()
    }

    #[test]
    fn create_and_list() {
        let _g = guard();
        let mut m = subs().lock().unwrap();
        m.clear();
        drop(m);
        assert!(create("a", ROOT).is_ok());
        assert!(create("a", ROOT).is_err());
        assert!(create("b", "a").is_ok());
        assert!(create("c", "missing").is_err());
        assert!(status().contains("cells: 2"));
        assert!(remove("a").is_err(), "还有子体时不能删");
        assert!(remove("b").is_ok());
        assert!(remove("a").is_ok());
    }

    #[test]
    fn each_cell_has_own_beliefs_and_judgment() {
        let _g = guard();
        let mut m = subs().lock().unwrap();
        m.clear();
        m.insert("x".into(), SubConscious::new("x", ROOT));
        m.insert("y".into(), SubConscious::new("y", ROOT));
        let x = m.get_mut("x").unwrap();
        x.beliefs.believe("天空是蓝的", 90).unwrap();
        x.motives.set("curiosity", 100).unwrap();
        assert!(x
            .judgment
            .judge_with(&x.motives, &x.beliefs, Some("天空是蓝的"))
            .unwrap()
            .contains("commit"));
        assert!(m.get("y").unwrap().beliefs.confidence("天空是蓝的").is_none());
        drop(m);
        subs().lock().unwrap().clear();
    }

    #[test]
    fn weighted_testimony_merge() {
        let mut parent = Beliefs::new();
        let mut child = Beliefs::new();
        child.believe("GPU 显存可用", 20).unwrap();
        child.observe("GPU 显存可用", 1).unwrap(); // 一条支持证据
        child.believe("只有子体知道", 66).unwrap();
        let report = merge_beliefs(&mut parent, &child);
        assert!(report.contains("GPU 显存可用"), "{}", report);
        assert!(report.contains("只有子体知道"), "{}", report);
        // 父 80%(证据0,权1) 与子 21%(证据1,权2) -> (80+42)/3 = 40
        assert_eq!(parent.confidence("GPU 显存可用"), Some(40));
        assert_eq!(parent.confidence("只有子体知道"), Some(66));
    }

    #[test]
    fn freeze_blocks_writes_and_thaw_reports_epoch_delta() {
        let _g = guard();
        let mut m = subs().lock().unwrap();
        m.clear();
        drop(m);
        create("sleeper", ROOT).unwrap();
        assert!(freeze("sleeper").is_ok());
        assert!(believe("sleeper", "新命题", 50).is_err());
        assert!(judge("sleeper", None).is_err());
        // 世界推进一版
        lease::force("cell/self", "外部改动");
        let r = thaw("sleeper").unwrap();
        assert!(r.contains("世界已变化 1 版"), "{}", r);
        assert!(believe("sleeper", "新命题", 50).is_ok());
        subs().lock().unwrap().clear();
    }

    #[test]
    fn root_view_uses_global_channels() {
        let _g = guard();
        let s = show(ROOT).unwrap();
        assert!(s.contains("motives:"), "{}", s);
        assert!(s.contains("beliefs:"), "{}", s);
    }
}
