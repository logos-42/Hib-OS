//! 块 I/O 请求队列 —— P0-4 子模块。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use crate::block::BlockRequest;

/// 块 I/O 请求队列。
struct RequestQueueState {
    queue: VecDeque<BlockRequest>,
    next_id: u64,
}

static RQ: OnceLock<Mutex<RequestQueueState>> = OnceLock::new();

fn rq_state() -> &'static Mutex<RequestQueueState> {
    RQ.get_or_init(|| Mutex::new(RequestQueueState {
        queue: VecDeque::new(),
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
pub fn block_complete(id: u64) -> bool {
    let mut g = rq_state().lock().unwrap();
    if let Some(pos) = g.queue.iter().position(|r| r.id == id) {
        if let Some(req) = g.queue.get_mut(pos) {
            req.completed = true;
            return true;
        }
    }
    false
}

/// 查询请求状态。
pub fn block_request_status(id: u64) -> Option<bool> {
    let g = rq_state().lock().unwrap();
    g.queue.iter().find(|r| r.id == id).map(|r| r.completed)
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
        assert_eq!(id, 1);
        assert_eq!(block_queue_len(), 1);
        assert_eq!(block_request_status(id), Some(false));
        assert!(block_complete(id));
        assert_eq!(block_request_status(id), Some(true));
    }

    #[test]
    fn complete_nonexistent() {
        assert!(!block_complete(999));
    }
}
