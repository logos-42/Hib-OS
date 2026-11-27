//! 块 I/O 请求队列 —— P0-4 子模块。
//!
//! 请求状态是显式的：队列中 = 未完成（`Some(false)`），
//! `finished` 集合 = 已完成（`Some(true)`），两者都查不到 = 不存在（`None`）。
//! 完成后的请求移出队列：同一 id 不可能被 `complete` 两次，
//! `block_queue_len()` 只统计未完成请求。

use std::collections::{HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};

use crate::block::BlockRequest;

/// 块 I/O 请求队列。
struct RequestQueueState {
    queue: VecDeque<BlockRequest>,
    /// 已完成请求的 id（完成即出队，这里只留 id 供状态查询与去重）。
    finished: HashSet<u64>,
    next_id: u64,
}

static RQ: OnceLock<Mutex<RequestQueueState>> = OnceLock::new();

fn rq_state() -> &'static Mutex<RequestQueueState> {
    RQ.get_or_init(|| Mutex::new(RequestQueueState {
        queue: VecDeque::new(),
        finished: HashSet::new(),
        next_id: 1,
    }))
}

/// 提交块 I/O 请求。
pub fn block_submit(device: &str, block: u64, count: u64, write: bool, data: Vec<u8>) -> u64 {
    let mut g = rq_state().lock().unwrap();
    let id = g.next_id;
    g.next_id += 1;
    let req = BlockRequest::new(id, device, block, count, write, data);
    g.queue.push_back(req);
    id
}

/// 完成一个请求。
///
/// 完成即出队并记入 `finished`：同一 id 第二次 `complete` 返回 `false`。
pub fn block_complete(id: u64) -> bool {
    let mut g = rq_state().lock().unwrap();
    if g.finished.contains(&id) {
        return false; // 已完成过，拒绝重复完成
    }
    let before = g.queue.len();
    g.queue.retain(|r| r.id != id);
    if g.queue.len() == before {
        return false; // 不存在
    }
    g.finished.insert(id);
    true
}

/// 查询请求状态：队列中 = `Some(false)`，已完成 = `Some(true)`，不存在 = `None`。
pub fn block_request_status(id: u64) -> Option<bool> {
    let g = rq_state().lock().unwrap();
    if g.queue.iter().any(|r| r.id == id) {
        return Some(false);
    }
    if g.finished.contains(&id) {
        return Some(true);
    }
    None
}

/// 队列长度。
pub fn block_queue_len() -> usize {
    let g = rq_state().lock().unwrap();
    g.queue.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_and_complete() {
        let id = block_submit("sda", 100, 4, true, vec![1u8; 2048]);
        assert_eq!(block_queue_len(), 1);
        assert_eq!(block_request_status(id), Some(false));
        assert!(block_complete(id));
        assert_eq!(block_request_status(id), Some(true));
        // 完成即出队：队列长度只统计未完成请求。
        assert_eq!(block_queue_len(), 0);
    }

    #[test]
    fn complete_nonexistent() {
        assert!(!block_complete(999));
    }

    #[test]
    fn double_complete_rejected() {
        let id = block_submit("sdb", 200, 1, false, vec![0u8; 512]);
        assert!(block_complete(id));
        assert!(!block_complete(id), "同一请求不得被完成两次");
        assert_eq!(block_request_status(id), Some(true));
    }
}
