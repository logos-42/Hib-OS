//! 异步 IO（提交队列 SQ / 完成队列 CQ，io_uring 简化版）。
//!
//! 提供 `AioContext`：提交读/写请求到 SQ，后台工作线程模拟异步处理，
//! 结果写入 CQ，通过 `poll` / `poll_many` 查询完成。支持 `cancel` 与
//! `shutdown`。
//!
//! 注意：此为用户态原型模拟，后台工作线程在本线程上轮询执行（不依赖
//! 真实 io_uring），供 `fs://` 通道异步读写使用。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use crate::fs::block::{BlockDevice, BlockError, BLOCK_SIZE};

/// 异步请求 ID。
pub type AioReqId = u64;
/// 文件描述符。
pub type Fd = u64;

/// 异步 IO 请求类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AioOp {
    Read,
    Write,
}

/// 请求状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AioStatus {
    /// 已提交，待处理。
    Submitted,
    /// 处理中。
    Processing,
    /// 已完成。
    Completed,
    /// 失败。
    Failed,
}

/// 异步 IO 结果。
#[derive(Clone, Debug)]
pub struct AioResult {
    pub req_id: AioReqId,
    pub op: AioOp,
    pub fd: Fd,
    pub offset: u64,
    /// 成功时为读写字节数，失败时为错误码。
    pub result: Result<usize, i32>,
}

/// 单个异步 IO 请求。
#[derive(Clone, Debug)]
struct AioReq {
    pub req_id: AioReqId,
    pub op: AioOp,
    pub fd: Fd,
    pub buf: Vec<u8>,
    pub offset: u64,
    pub status: AioStatus,
    pub result: Option<AioResult>,
    /// 完成回调（可选）。
    pub callback: Option<Box<dyn FnOnce(AioResult) + Send>>,
}

/// 提交队列条目（SQE）。
#[derive(Clone, Debug)]
pub struct Sqe {
    pub op: AioOp,
    pub fd: Fd,
    pub buf: Vec<u8>,
    pub offset: u64,
}

/// 完成队列条目（CQE）。
#[derive(Clone, Debug)]
pub struct Cqe {
    pub req_id: AioReqId,
    pub result: Result<usize, i32>,
}

/// 环缓冲区简化模型：固定容量的头尾指针环。
struct RingBuffer<T> {
    buf: Vec<Option<T>>,
    head: usize,
    tail: usize,
    count: usize,
}

impl<T: Clone> RingBuffer<T> {
    fn new(capacity: usize) -> Self {
        Self {
            buf: vec![None; capacity],
            head: 0,
            tail: 0,
            count: 0,
        }
    }
    fn push(&mut self, item: T) -> Result<(), T> {
        if self.count == self.buf.len() {
            return Err(item);
        }
        self.buf[self.tail] = Some(item);
        self.tail = (self.tail + 1) % self.buf.len();
        self.count += 1;
        Ok(())
    }
    fn pop(&mut self) -> Option<T> {
        if self.count == 0 {
            return None;
        }
        let item = self.buf[self.head].take();
        self.head = (self.head + 1) % self.buf.len();
        self.count -= 1;
        item
    }
    fn len(&self) -> usize {
        self.count
    }
    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// 异步 IO 上下文内部共享状态。
struct AioContextInner {
    /// 提交队列（环缓冲）。
    sq: Mutex<RingBuffer<Sqe>>,
    /// 完成队列（环缓冲）。
    cq: Mutex<RingBuffer<Cqe>>,
    /// 请求表：req_id -> 请求。
    reqs: Mutex<BTreeMap<AioReqId, AioReq>>,
    /// 下一个 req_id。
    next_id: AtomicU64,
    /// 设备表：fd -> 块设备。
    devices: Mutex<BTreeMap<Fd, Box<dyn BlockDevice + Send>>>,
    /// 运行标志。
    running: AtomicBool,
    /// 完成条件变量（用于 poll 阻塞等待）。
    cq_cvar: Condvar,
}

/// 异步 IO 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AioError {
    /// 队列已满。
    QueueFull,
    /// 文件描述符不存在。
    NoFd,
    /// 请求不存在。
    NoReq,
    /// 设备 IO 错误。
    IoError(String),
    /// 已关闭。
    Shutdown,
    /// 参数无效。
    Invalid,
}

impl std::fmt::Display for AioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AioError::QueueFull => write!(f, "提交队列已满"),
            AioError::NoFd => write!(f, "文件描述符不存在"),
            AioError::NoReq => write!(f, "异步请求不存在"),
            AioError::IoError(e) => write!(f, "异步 IO 错误: {}", e),
            AioError::Shutdown => write!(f, "异步上下文已关闭"),
            AioError::Invalid => write!(f, "无效的异步参数"),
        }
    }
}

/// 异步 IO 上下文：管理 SQ/CQ、请求表、后台工作线程。
///
/// 内部状态通过 `Arc` 共享，工作线程持有 `Arc` 副本，无需 Clone 整个上下文。
pub struct AioContext {
    inner: Arc<AioContextInner>,
    /// 工作线程句柄。
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    /// SQ 容量。
    sq_capacity: usize,
    /// CQ 容量。
    cq_capacity: usize,
}

impl AioContext {
    /// 创建异步 IO 上下文（SQ/CQ 容量均为 `capacity`）。
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(4);
        let inner = Arc::new(AioContextInner {
            sq: Mutex::new(RingBuffer::new(capacity)),
            cq: Mutex::new(RingBuffer::new(capacity)),
            reqs: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            devices: Mutex::new(BTreeMap::new()),
            running: AtomicBool::new(false),
            cq_cvar: Condvar::new(),
        });
        Self {
            inner,
            worker: Mutex::new(None),
            sq_capacity: capacity,
            cq_capacity: capacity,
        }
    }

    /// 注册一个块设备到 fd。
    pub fn register_device(&self, fd: Fd, device: Box<dyn BlockDevice + Send>) {
        self.inner.devices.lock().unwrap().insert(fd, device);
    }

    /// 启动后台工作线程。
    pub fn start(&self) -> Result<(), AioError> {
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let ctx = Arc::clone(&self.inner);
        let handle = thread::spawn(move || {
            ctx.worker_loop();
        });
        self.worker.lock().unwrap().replace(handle);
        Ok(())
    }

    /// 提交一个异步读/写请求到 SQ，返回 req_id。
    pub fn submit(
        &self,
        op: AioOp,
        fd: Fd,
        buf: Vec<u8>,
        offset: u64,
    ) -> Result<AioReqId, AioError> {
        if !self.inner.running.load(Ordering::SeqCst) {
            return Err(AioError::Shutdown);
        }
        if !self.inner.devices.lock().unwrap().contains_key(&fd) {
            return Err(AioError::NoFd);
        }
        let req_id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let req = AioReq {
            req_id,
            op,
            fd,
            buf,
            offset,
            status: AioStatus::Submitted,
            result: None,
            callback: None,
        };
        {
            let mut reqs = self.inner.reqs.lock().unwrap();
            reqs.insert(req_id, req);
        }
        self.inner.sq.lock().unwrap().push(Sqe { op, fd, buf: Vec::new(), offset }).map_err(|_| AioError::QueueFull)?;
        // buf 已移入 Sqe？不，上面构造 Sqe 时 buf 被移走了。修正：先构造 Sqe 再 push。
        // 此处 buf 已在 req 中保留一份，Sqe 中 buf 留空供 worker 按 req_id 查表取。
        // 唤醒工作线程。
        self.inner.cq_cvar.notify_one();
        Ok(req_id)
    }

    /// 设置请求的完成回调。
    pub fn set_callback<F>(&self, req_id: AioReqId, cb: F) -> Result<(), AioError>
    where
        F: FnOnce(AioResult) + Send + 'static,
    {
        let mut reqs = self.inner.reqs.lock().unwrap();
        if let Some(req) = reqs.get_mut(&req_id) {
            req.callback = Some(Box::new(cb));
            Ok(())
        } else {
            Err(AioError::NoReq)
        }
    }

    /// 轮询单个请求完成（阻塞至超时）。
    pub fn poll(&self, req_id: AioReqId, timeout: Duration) -> Result<AioResult, AioError> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let reqs = self.inner.reqs.lock().unwrap();
                if let Some(req) = reqs.get(&req_id) {
                    if let Some(ref result) = req.result {
                        return Ok(result.clone());
                    }
                    if req.status == AioStatus::Failed {
                        return Err(AioError::IoError("请求处理失败".into()));
                    }
                } else {
                    return Err(AioError::NoReq);
                }
            }
            if Instant::now() >= deadline {
                return Err(AioError::IoError("poll 超时".into()));
            }
            let mut cq = self.inner.cq.lock().unwrap();
            let remaining = deadline.duration_since(Instant::now());
            if remaining.is_zero() || remaining.as_nanos() == 0 {
                drop(cq);
                continue;
            }
            let _ = self.inner.cq_cvar.wait_timeout(cq, remaining).unwrap().0;
            drop(cq);
        }
    }

    /// 轮询多个请求（返回每个请求的结果，未完成者返回错误）。
    pub fn poll_many(
        &self,
        req_ids: &[AioReqId],
        timeout: Duration,
    ) -> Vec<Result<AioResult, AioError>> {
        let deadline = Instant::now() + timeout;
        let mut results = Vec::with_capacity(req_ids.len());
        for &req_id in req_ids {
            results.push(self.poll(req_id, deadline.saturating_duration_since(Instant::now())));
        }
        results
    }

    /// 取消请求（仅未处理的可取消）。
    pub fn cancel(&self, req_id: AioReqId) -> Result<(), AioError> {
        let mut reqs = self.inner.reqs.lock().unwrap();
        if let Some(req) = reqs.get_mut(&req_id) {
            if req.status == AioStatus::Submitted {
                req.status = AioStatus::Failed;
                req.result = Some(AioResult {
                    req_id,
                    op: req.op,
                    fd: req.fd,
                    offset: req.offset,
                    result: Err(-125), // ECANCELED
                });
                return Ok(());
            }
            return Err(AioError::IoError("请求已处理，无法取消".into()));
        }
        Err(AioError::NoReq)
    }

    /// 关闭：停止工作线程，等待退出。
    pub fn shutdown(&self) -> Result<(), AioError> {
        self.inner.running.store(false, Ordering::SeqCst);
        self.inner.cq_cvar.notify_all();
        if let Some(handle) = self.worker.lock().unwrap().take() {
            let _ = handle.join();
        }
        Ok(())
    }

    /// 返回 SQ 长度。
    pub fn sq_len(&self) -> usize {
        self.inner.sq.lock().unwrap().len()
    }

    /// 返回 CQ 长度。
    pub fn cq_len(&self) -> usize {
        self.inner.cq.lock().unwrap().len()
    }

    /// 返回请求总数。
    pub fn req_count(&self) -> usize {
        self.inner.reqs.lock().unwrap().len()
    }

    /// 返回是否运行中。
    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }
}

impl AioContextInner {
    /// 工作线程主循环：从 SQ 取请求，处理，写入 CQ。
    fn worker_loop(self: &Arc<Self>) {
        while self.running.load(Ordering::SeqCst) {
            let sqe = {
                let mut sq = self.sq.lock().unwrap();
                sq.pop()
            };
            match sqe {
                Some(_sqe) => {
                    // 处理第一个 Submitted 状态的请求（按 req_id 顺序）。
                    let req_id = self.process_next_submitted();
                    if let Some(reqs) = self.reqs.lock().unwrap().get(&req_id) {
                        if let Some(ref result) = reqs.result {
                            let cqe = Cqe {
                                req_id,
                                result: result.result.clone(),
                            };
                            let mut cq = self.cq.lock().unwrap();
                            let _ = cq.push(cqe);
                            drop(cq);
                            self.cq_cvar.notify_all();
                        }
                    }
                }
                None => {
                    // SQ 为空，短暂休眠避免忙等。
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    /// 处理下一个 Submitted 状态的请求，返回 req_id。
    fn process_next_submitted(&self) -> AioReqId {
        let req_id = {
            let mut reqs = self.reqs.lock().unwrap();
            let found = reqs.iter_mut().find(|(_, r)| r.status == AioStatus::Submitted);
            if let Some((id, r)) = found {
                r.status = AioStatus::Processing;
                *id
            } else {
                return 0;
            }
        };
        // 查找对应的 SQE（按 req_id 的 fd/offset/op 简化匹配：取 req 记录）。
        let (op, fd, offset, buf) = {
            let reqs = self.reqs.lock().unwrap();
            let r = reqs.get(&req_id).expect("req must exist");
            (r.op, r.fd, r.offset, r.buf.clone())
        };
        let result = (|| -> Result<usize, i32> {
            let devices = self.devices.lock().unwrap();
            let device = devices.get(&fd).ok_or(-2)?; // ENOENT
            match op {
                AioOp::Read => {
                    let mut buf2 = vec![0u8; buf.len()];
                    device
                        .read_at(offset, &mut buf2)
                        .map(|()| buf2.len())
                        .map_err(|e| -(e.to_string().len() as i32))
                }
                AioOp::Write => {
                    device
                        .write_at(offset, &buf)
                        .map(|()| buf.len())
                        .map_err(|e| -(e.to_string().len() as i32))
                }
            }
        })();
        let mut reqs = self.reqs.lock().unwrap();
        if let Some(r) = reqs.get_mut(&req_id) {
            r.status = match result {
                Ok(n) => AioStatus::Completed,
                Err(_) => AioStatus::Failed,
            };
            r.result = Some(AioResult {
                req_id,
                op,
                fd,
                offset,
                result: result.map_err(|e| e),
            });
            if let Some(cb) = r.callback.take() {
                let res = r.result.clone().unwrap();
                drop(reqs);
                cb(res);
            }
        }
        req_id
    }
}

impl Default for AioContext {
    fn default() -> Self {
        Self::new(64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::block::MemBlockDevice;

    #[test]
    fn aio_submit_poll_read_write() {
        let ctx = AioContext::new(32);
        let fd = 1;
        ctx.register_device(fd, Box::new(MemBlockDevice::new("ram0", 64)));
        ctx.start().unwrap();

        // 写入一块数据。
        let mut write_buf = [0u8; BLOCK_SIZE];
        write_buf[0] = 0xAB;
        let write_id = ctx
            .submit(AioOp::Write, fd, write_buf.to_vec(), 0)
            .unwrap();
        let res = ctx.poll(write_id, Duration::from_secs(2)).unwrap();
        assert!(res.result.is_ok());
        assert_eq!(res.result.unwrap(), BLOCK_SIZE);

        // 读回。
        let read_id = ctx.submit(AioOp::Read, fd, vec![0u8; BLOCK_SIZE], 0).unwrap();
        let res = ctx.poll(read_id, Duration::from_secs(2)).unwrap();
        assert!(res.result.is_ok());
        assert_eq!(res.result.unwrap(), BLOCK_SIZE);
        ctx.shutdown().unwrap();
    }

    #[test]
    fn aio_poll_many() {
        let ctx = AioContext::new(32);
        let fd = 1;
        ctx.register_device(fd, Box::new(MemBlockDevice::new("ram0", 64)));
        ctx.start().unwrap();
        let mut ids = Vec::new();
        for i in 0..4 {
            let mut buf = [0u8; BLOCK_SIZE];
            buf[0] = i as u8;
            ids.push(ctx.submit(AioOp::Write, fd, buf.to_vec(), i as u64).unwrap());
        }
        let results = ctx.poll_many(&ids, Duration::from_secs(5));
        for r in results {
            assert!(r.is_ok());
        }
        ctx.shutdown().unwrap();
    }

    #[test]
    fn aio_cancel() {
        let ctx = AioContext::new(32);
        let fd = 1;
        ctx.register_device(fd, Box::new(MemBlockDevice::new("ram0", 64)));
        ctx.start().unwrap();
        let id = ctx.submit(AioOp::Read, fd, vec![0u8; BLOCK_SIZE], 5).unwrap();
        assert!(ctx.cancel(id).is_ok());
        ctx.shutdown().unwrap();
    }

    #[test]
    fn aio_no_fd() {
        let ctx = AioContext::new(32);
        ctx.start().unwrap();
        assert_eq!(
            ctx.submit(AioOp::Read, 999, vec![0u8; BLOCK_SIZE], 0).unwrap_err(),
            AioError::NoFd
        );
        ctx.shutdown().unwrap();
    }

    #[test]
    fn aio_sq_full() {
        let ctx = AioContext::new(2);
        let fd = 1;
        ctx.register_device(fd, Box::new(MemBlockDevice::new("ram0", 64)));
        ctx.start().unwrap();
        let _ = ctx.submit(AioOp::Read, fd, vec![0u8; BLOCK_SIZE], 0).unwrap();
        let _ = ctx.submit(AioOp::Read, fd, vec![0u8; BLOCK_SIZE], 1).unwrap();
        assert_eq!(
            ctx.submit(AioOp::Read, fd, vec![0u8; BLOCK_SIZE], 2).unwrap_err(),
            AioError::QueueFull
        );
        ctx.shutdown().unwrap();
    }

    #[test]
    fn aio_shutdown_rejects_submit() {
        let ctx = AioContext::new(32);
        ctx.start().unwrap();
        ctx.shutdown().unwrap();
        assert_eq!(
            ctx.submit(AioOp::Read, 1, vec![0u8; BLOCK_SIZE], 0).unwrap_err(),
            AioError::Shutdown
        );
    }

    #[test]
    fn aio_callback() {
        let ctx = AioContext::new(32);
        let fd = 1;
        ctx.register_device(fd, Box::new(MemBlockDevice::new("ram0", 64)));
        ctx.start().unwrap();
        let fired = Arc::new(AtomicBool::new(false));
        let fired2 = fired.clone();
        let id = ctx.submit(AioOp::Write, fd, vec![0x01u8; BLOCK_SIZE], 0).unwrap();
        ctx.set_callback(id, move |_result| {
            fired2.store(true, Ordering::SeqCst);
        }).unwrap();
        ctx.poll(id, Duration::from_secs(2)).unwrap();
        assert!(fired.load(Ordering::SeqCst));
        ctx.shutdown().unwrap();
    }

    #[test]
    fn aio_stats() {
        let ctx = AioContext::new(32);
        assert_eq!(ctx.sq_len(), 0);
        assert_eq!(ctx.cq_len(), 0);
        assert_eq!(ctx.req_count(), 0);
        assert!(!ctx.is_running());
        ctx.start().unwrap();
        assert!(ctx.is_running());
        ctx.shutdown().unwrap();
    }
}
