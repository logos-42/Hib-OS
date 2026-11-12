//! 块 I/O 调度器 —— P0-4 子模块。

use std::collections::BTreeMap;

/// I/O 调度算法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoSchedulerAlg {
    Fifo,
    Sstf, // 最短寻道时间优先
    Scan, // 电梯算法
    CScan, // 循环扫描
}

/// 块 I/O 调度器。
pub struct IoScheduler {
    alg: IoSchedulerAlg,
    current_pos: u64,
    pending: BTreeMap<u64, u64>, // block -> 请求数
}

impl IoScheduler {
    pub fn new(alg: IoSchedulerAlg) -> Self {
        Self {
            alg,
            current_pos: 0,
            pending: BTreeMap::new(),
        }
    }

    /// 添加请求。
    pub fn add_request(&mut self, block: u64) {
        *self.pending.entry(block).or_insert(0) += 1;
    }

    /// 选择下一个请求块。
    pub fn next_request(&mut self) -> Option<u64> {
        if self.pending.is_empty() {
            return None;
        }
        let block = match self.alg {
            IoSchedulerAlg::Fifo => *self.pending.keys().next().unwrap(),
            IoSchedulerAlg::Sstf => {
                self.pending
                    .keys()
                    .min_by_key(|b| b.abs_diff(self.current_pos))
                    .copied()
                    .unwrap()
            }
            IoSchedulerAlg::Scan => {
                // 向前扫描，找 >= current_pos 的第一个
                self.pending
                    .range(self.current_pos..)
                    .next()
                    .map(|(b, _)| *b)
                    .or_else(|| self.pending.keys().next().copied())
                    .unwrap()
            }
            IoSchedulerAlg::CScan => {
                self.pending
                    .range(self.current_pos..)
                    .next()
                    .map(|(b, _)| *b)
                    .or_else(|| self.pending.keys().next().copied())
                    .unwrap()
            }
        };
        self.current_pos = block;
        let count = self.pending.get(&block).unwrap();
        if *count > 1 {
            self.pending.insert(block, count - 1);
        } else {
            self.pending.remove(&block);
        }
        Some(block)
    }

    /// 当前位置。
    pub fn position(&self) -> u64 {
        self.current_pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_scheduling() {
        let mut sched = IoScheduler::new(IoSchedulerAlg::Fifo);
        sched.add_request(100);
        sched.add_request(50);
        sched.add_request(200);
        assert_eq!(sched.next_request(), Some(100));
        assert_eq!(sched.next_request(), Some(50));
        assert_eq!(sched.next_request(), Some(200));
    }

    #[test]
    fn sstf_scheduling() {
        let mut sched = IoScheduler::new(IoSchedulerAlg::Sstf);
        sched.add_request(100);
        sched.add_request(50);
        sched.add_request(200);
        sched.current_pos = 75;
        // 50 和 100 都距离 25，SSTF 选其中一个
        let first = sched.next_request().unwrap();
        assert!(first == 50 || first == 100);
    }

    #[test]
    fn scan_scheduling() {
        let mut sched = IoScheduler::new(IoSchedulerAlg::Scan);
        sched.add_request(100);
        sched.add_request(200);
        sched.add_request(50);
        sched.current_pos = 75;
        // Scan 向前扫描，先选 100
        assert_eq!(sched.next_request(), Some(100));
    }
}
