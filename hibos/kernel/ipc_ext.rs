//! IPC 扩展：共享内存、消息队列、管道、Unix 套接字（简化版）。
//!
//! 提供四种进程间通信原语，全部以宿主内存模拟，不依赖真实操作系统 IPC：
//!
//! * `SharedMemory` —— 创建/附加/分离/销毁命名共享内存区；
//! * `MessageQueue` —— 带优先级的消息队列，支持发送/接收（带超时）/销毁；
//! * `Pipe` —— 单向管道，read/write/close；
//! * `UnixSocket` —— 简化版 socketpair，sendto/recvfrom。
//!
//! 所有状态均通过 `OnceLock<Mutex<...>>` 全局单例持有，与现有 `kernel/ipc.rs`
//! 风格保持一致。

use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// =====================================================================================
// 共享内存
// =====================================================================================

/// 共享内存标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShmId(pub u64);

/// 共享内存区：名称、数据缓冲区、附加计数、销毁标志。
#[derive(Clone, Debug)]
pub struct SharedMemory {
    pub id: ShmId,
    pub name: String,
    pub data: Vec<u8>,
    pub attach_count: usize,
    pub destroyed: bool,
}

impl SharedMemory {
    fn new(id: ShmId, name: &str, size: usize) -> Self {
        Self {
            id,
            name: name.to_string(),
            data: vec![0u8; size],
            attach_count: 0,
            destroyed: false,
        }
    }

    /// 返回大小。
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// 附加：增加附加计数，返回数据切片的可变引用（模拟映射）。
    pub fn attach(&mut self) -> usize {
        self.attach_count += 1;
        self.attach_count
    }

    /// 分离：减少附加计数。
    pub fn detach(&mut self) -> bool {
        if self.attach_count > 0 {
            self.attach_count -= 1;
            true
        } else {
            false
        }
    }

    /// 读取数据（从 offset 开始，最多 len 字节）。
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, String> {
        if self.destroyed {
            return Err("共享内存已销毁".to_string());
        }
        if offset >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - offset);
        buf[..n].copy_from_slice(&self.data[offset..offset + n]);
        Ok(n)
    }

    /// 写入数据（从 offset 开始）。
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> Result<usize, String> {
        if self.destroyed {
            return Err("共享内存已销毁".to_string());
        }
        if offset >= self.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - offset);
        self.data[offset..offset + n].copy_from_slice(&buf[..n]);
        Ok(n)
    }
}

/// 共享内存管理器。
pub struct SharedMemoryManager {
    segments: HashMap<String, SharedMemory>,
    name_to_id: HashMap<String, ShmId>,
    next_id: u64,
}

impl SharedMemoryManager {
    pub fn new() -> Self {
        Self {
            segments: HashMap::new(),
            name_to_id: HashMap::new(),
            next_id: 1,
        }
    }

    /// 创建共享内存段，返回 ShmId。若同名已存在则返回错误。
    pub fn create(&mut self, name: &str, size: usize) -> Result<ShmId, String> {
        if self.name_to_id.contains_key(name) {
            return Err(format!("共享内存已存在: {}", name));
        }
        if size == 0 {
            return Err("共享内存大小必须大于 0".to_string());
        }
        let id = ShmId(self.next_id);
        self.next_id += 1;
        let shm = SharedMemory::new(id, name, size);
        self.name_to_id.insert(name.to_string(), id);
        self.segments.insert(name.to_string(), shm);
        Ok(id)
    }

    /// 附加共享内存，返回 (ShmId, 附加计数)。
    pub fn attach(&mut self, name: &str) -> Result<(ShmId, usize), String> {
        let id = *self
            .name_to_id
            .get(name)
            .ok_or_else(|| format!("共享内存不存在: {}", name))?;
        let attach_count = self
            .segments
            .get_mut(name)
            .ok_or_else(|| format!("共享内存段不存在: {}", name))?
            .attach();
        Ok((id, attach_count))
    }

    /// 按 id 附加。
    pub fn attach_by_id(&mut self, id: ShmId) -> Result<usize, String> {
        let name = self
            .segments
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(n, _)| n.clone())
            .ok_or_else(|| format!("共享内存 ID 不存在: {:?}", id))?;
        let attach_count = self
            .segments
            .get_mut(&name)
            .unwrap()
            .attach();
        Ok(attach_count)
    }

    /// 分离。
    pub fn detach_by_id(&mut self, id: ShmId) -> Result<bool, String> {
        let name = self
            .segments
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(n, _)| n.clone())
            .ok_or_else(|| format!("共享内存 ID 不存在: {:?}", id))?;
        Ok(self.segments.get_mut(&name).unwrap().detach())
    }

    /// 销毁共享内存段。
    pub fn destroy(&mut self, id: ShmId) -> Result<(), String> {
        let name = self
            .segments
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(n, _)| n.clone())
            .ok_or_else(|| format!("共享内存 ID 不存在: {:?}", id))?;
        let shm = self.segments.get_mut(&name).unwrap();
        shm.destroyed = true;
        self.name_to_id.remove(&name);
        self.segments.remove(&name);
        Ok(())
    }

    /// 读取指定段数据。
    pub fn read(&mut self, id: ShmId, offset: usize, buf: &mut [u8]) -> Result<usize, String> {
        let name = self
            .segments
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(n, _)| n.clone())
            .ok_or_else(|| format!("共享内存 ID 不存在: {:?}", id))?;
        self.segments.get_mut(&name).unwrap().read(offset, buf)
    }

    /// 写入指定段数据。
    pub fn write(&mut self, id: ShmId, offset: usize, buf: &[u8]) -> Result<usize, String> {
        let name = self
            .segments
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(n, _)| n.clone())
            .ok_or_else(|| format!("共享内存 ID 不存在: {:?}", id))?;
        self.segments.get_mut(&name).unwrap().write(offset, buf)
    }

    /// 返回段总数。
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// 返回摘要。
    pub fn summary(&self) -> String {
        format!(
            "shm segments={} total_size_bytes={}",
            self.len(),
            self.segments.values().map(|s| s.size()).sum::<usize>()
        )
    }
}

impl Default for SharedMemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 消息队列
// =====================================================================================

/// 消息队列标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MsgQId(pub u64);

/// 消息：数据 + 优先级 + 发送时间戳。
#[derive(Clone, Debug)]
pub struct Message {
    pub id: u64,
    pub data: Vec<u8>,
    pub priority: i32, // 数值越大优先级越高
    pub enqueued_at: u64,
}

/// 消息队列：带优先级的有界队列。
pub struct MessageQueue {
    pub id: MsgQId,
    pub name: String,
    pub max_msgs: usize,
    pub max_msg_size: usize,
    /// 优先级堆：(priority, enqueue_seq, message id)。
    heap: BinaryHeap<(i32, u64, u64)>,
    /// 消息 id -> 消息。
    messages: HashMap<u64, Message>,
    /// 当前消息数。
    count: usize,
    /// 下一个消息 id。
    next_msg_id: u64,
    /// 下一个入队序列号（用于同优先级 FIFO）。
    next_seq: u64,
    /// 已销毁。
    destroyed: bool,
    /// 统计。
    send_count: u64,
    recv_count: u64,
}

impl MessageQueue {
    pub fn new(id: MsgQId, name: &str, max_msgs: usize, max_msg_size: usize) -> Self {
        Self {
            id,
            name: name.to_string(),
            max_msgs,
            max_msg_size,
            heap: BinaryHeap::new(),
            messages: HashMap::new(),
            count: 0,
            next_msg_id: 1,
            next_seq: 0,
            destroyed: false,
            send_count: 0,
            recv_count: 0,
        }
    }

    /// 发送消息（带优先级）。队列满时返回错误。
    pub fn send(&mut self, data: &[u8], priority: i32) -> Result<(), String> {
        if self.destroyed {
            return Err("消息队列已销毁".to_string());
        }
        if data.len() > self.max_msg_size {
            return Err("消息大小超过上限".to_string());
        }
        if self.count >= self.max_msgs {
            return Err("消息队列已满".to_string());
        }
        let msg_id = self.next_msg_id;
        self.next_msg_id += 1;
        let seq = self.next_seq;
        self.next_seq += 1;
        let msg = Message {
            id: msg_id,
            data: data.to_vec(),
            priority,
            enqueued_at: now_millis(),
        };
        self.messages.insert(msg_id, msg);
        self.heap.push((priority, seq, msg_id));
        self.count += 1;
        self.send_count += 1;
        Ok(())
    }

    /// 接收消息（按优先级，同优先级 FIFO）。`timeout` 为0表示非阻塞（本模拟中
    /// 直接返回 None 若无消息）。
    pub fn recv(&mut self, _timeout: Duration) -> Option<Message> {
        if self.destroyed {
            return None;
        }
        if self.count == 0 {
            return None;
        }
        // 取出最高优先级的消息 id。
        while let Some((_p, _seq, msg_id)) = self.heap.pop() {
            if let Some(msg) = self.messages.remove(&msg_id) {
                self.count -= 1;
                self.recv_count += 1;
                return Some(msg);
            }
            // 消息已被移除，继续下一个。
        }
        None
    }

    /// 销毁队列。
    pub fn destroy(&mut self) {
        self.destroyed = true;
        self.messages.clear();
        self.heap.clear();
        self.count = 0;
    }

    /// 返回当前消息数。
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// 返回摘要。
    pub fn summary(&self) -> String {
        format!(
            "msgq name={} max_msgs={} max_msg_size={} count={} sends={} recvs={}",
            self.name, self.max_msgs, self.max_msg_size, self.count, self.send_count, self.recv_count
        )
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =====================================================================================
// 消息队列管理器
// =====================================================================================

pub struct MessageQueueManager {
    queues: HashMap<String, MessageQueue>,
    name_to_id: HashMap<String, MsgQId>,
    next_id: u64,
}

impl MessageQueueManager {
    pub fn new() -> Self {
        Self {
            queues: HashMap::new(),
            name_to_id: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, name: &str, max_msgs: usize, max_msg_size: usize) -> Result<MsgQId, String> {
        if self.name_to_id.contains_key(name) {
            return Err(format!("消息队列已存在: {}", name));
        }
        let id = MsgQId(self.next_id);
        self.next_id += 1;
        let q = MessageQueue::new(id, name, max_msgs, max_msg_size);
        self.name_to_id.insert(name.to_string(), id);
        self.queues.insert(name.to_string(), q);
        Ok(id)
    }

    pub fn send(&mut self, qid: MsgQId, data: &[u8], priority: i32) -> Result<(), String> {
        let name = self.queue_name(qid).ok_or_else(|| format!("消息队列不存在: {:?}", qid))?;
        self.queues.get_mut(&name).unwrap().send(data, priority)
    }

    pub fn recv(&mut self, qid: MsgQId, timeout: Duration) -> Option<Message> {
        let name = match self.queue_name(qid) {
            Some(n) => n,
            None => return None,
        };
        self.queues.get_mut(&name).unwrap().recv(timeout)
    }

    pub fn destroy(&mut self, qid: MsgQId) -> Result<(), String> {
        let name = self.queue_name(qid).ok_or_else(|| format!("消息队列不存在: {:?}", qid))?;
        if let Some(q) = self.queues.get_mut(&name) {
            q.destroy();
        }
        self.name_to_id.remove(&name);
        self.queues.remove(&name);
        Ok(())
    }

    fn queue_name(&self, qid: MsgQId) -> Option<String> {
        self.queues
            .iter()
            .find(|(_, q)| q.id == qid)
            .map(|(n, _)| n.clone())
    }

    pub fn len(&self) -> usize {
        self.queues.len()
    }

    pub fn summary(&self) -> String {
        format!("msgq_manager queues={}", self.len())
    }
}

impl Default for MessageQueueManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 管道
// =====================================================================================

/// 管道读端标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PipeReadId(pub u64);
/// 管道写端标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PipeWriteId(pub u64);

/// 管道：单向字节流，内部环形缓冲区。
pub struct Pipe {
    pub read_id: PipeReadId,
    pub write_id: PipeWriteId,
    pub buffer: VecDeque<u8>,
    pub capacity: usize,
    pub closed_read: bool,
    pub closed_write: bool,
    pub read_count: u64,
    pub write_count: u64,
}

impl Pipe {
    pub fn new(read_id: PipeReadId, write_id: PipeWriteId, capacity: usize) -> Self {
        Self {
            read_id,
            write_id,
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            closed_read: false,
            closed_write: false,
            read_count: 0,
            write_count: 0,
        }
    }

    /// 写入数据。返回实际写入字节数。
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, String> {
        if self.closed_write {
            return Err("管道写端已关闭".to_string());
        }
        let space = self.capacity.saturating_sub(self.buffer.len());
        let n = buf.len().min(space);
        for b in &buf[..n] {
            self.buffer.push_back(*b);
        }
        self.write_count += n as u64;
        Ok(n)
    }

    /// 读取数据。返回实际读取字节数。
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        if self.closed_read {
            return Err("管道读端已关闭".to_string());
        }
        let n = buf.len().min(self.buffer.len());
        for i in 0..n {
            buf[i] = self.buffer.pop_front().unwrap();
        }
        self.read_count += n as u64;
        Ok(n)
    }

    /// 关闭读端。
    pub fn close_read(&mut self) {
        self.closed_read = true;
    }

    /// 关闭写端。
    pub fn close_write(&mut self) {
        self.closed_write = true;
    }

    /// 是否已关闭读写端。
    pub fn is_closed(&self) -> bool {
        self.closed_read && self.closed_write
    }

    /// 返回缓冲区中待读字节数。
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn summary(&self) -> String {
        format!(
            "pipe cap={} buffered={} reads={} writes={} closed_r={} closed_w={}",
            self.capacity,
            self.buffer.len(),
            self.read_count,
            self.write_count,
            self.closed_read,
            self.closed_write,
        )
    }
}

// =====================================================================================
// 管道管理器
// =====================================================================================

pub struct PipeManager {
    pipes: HashMap<u64, Pipe>,
    next_pipe_id: u64,
}

impl PipeManager {
    pub fn new() -> Self {
        Self {
            pipes: HashMap::new(),
            next_pipe_id: 1,
        }
    }

    /// 创建管道，返回 (read_id, write_id)。
    pub fn create(&mut self, capacity: usize) -> (PipeReadId, PipeWriteId) {
        let pid = self.next_pipe_id;
        self.next_pipe_id += 1;
        let read_id = PipeReadId(pid * 2);
        let write_id = PipeWriteId(pid * 2 + 1);
        let pipe = Pipe::new(read_id, write_id, capacity);
        self.pipes.insert(pid, pipe);
        (read_id, write_id)
    }

    /// 按读端 id 写入（查找对应管道）。
    pub fn write_by_read(&mut self, read_id: PipeReadId, buf: &[u8]) -> Result<usize, String> {
        let pid = read_id.0 / 2;
        let pipe = self.pipes.get_mut(&pid).ok_or("管道不存在")?;
        pipe.write(buf)
    }

    /// 按读端 id 读取。
    pub fn read_by_read(&mut self, read_id: PipeReadId, buf: &mut [u8]) -> Result<usize, String> {
        let pid = read_id.0 / 2;
        let pipe = self.pipes.get_mut(&pid).ok_or("管道不存在")?;
        pipe.read(buf)
    }

    pub fn close_read(&mut self, read_id: PipeReadId) {
        let pid = read_id.0 / 2;
        if let Some(pipe) = self.pipes.get_mut(&pid) {
            pipe.close_read();
        }
    }

    pub fn close_write(&mut self, write_id: PipeWriteId) {
        let pid = (write_id.0 - 1) / 2;
        if let Some(pipe) = self.pipes.get_mut(&pid) {
            pipe.close_write();
        }
    }

    pub fn len(&self) -> usize {
        self.pipes.len()
    }

    pub fn summary(&self) -> String {
        format!("pipe_manager pipes={}", self.len())
    }
}

impl Default for PipeManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// Unix Socket（简化版）
// =====================================================================================

/// 套接字文件描述符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SockFd(pub u64);

/// Unix 套接字对：两个互相连接的 fd。
pub struct UnixSocketPair {
    pub fd1: SockFd,
    pub fd2: SockFd,
    pub buf1: VecDeque<u8>,
    pub buf2: VecDeque<u8>,
    pub closed1: bool,
    pub closed2: bool,
}

impl UnixSocketPair {
    fn new(fd1: SockFd, fd2: SockFd) -> Self {
        Self {
            fd1,
            fd2,
            buf1: VecDeque::new(),
            buf2: VecDeque::new(),
            closed1: false,
            closed2: false,
        }
    }

    /// 向 fd1 的对端（fd2）发送数据（即写入 buf2）。
    pub fn sendto(&mut self, from: SockFd, buf: &[u8]) -> Result<usize, String> {
        if from == self.fd1 {
            if self.closed2 {
                return Err("对端已关闭".to_string());
            }
            self.buf2.extend(buf.iter().copied());
            Ok(buf.len())
        } else if from == self.fd2 {
            if self.closed1 {
                return Err("对端已关闭".to_string());
            }
            self.buf1.extend(buf.iter().copied());
            Ok(buf.len())
        } else {
            Err("未知的套接字 fd".to_string())
        }
    }

    /// 从 fd1 的对端接收数据（即从 buf2 读取，对应从 fd2 发送来的）。
    pub fn recvfrom(&mut self, to: SockFd, buf: &mut [u8]) -> Result<usize, String> {
        let src = if to == self.fd1 { &mut self.buf2 } else if to == self.fd2 { &mut self.buf1 } else { return Err("未知的套接字 fd".to_string()); };
        let n = buf.len().min(src.len());
        for i in 0..n {
            buf[i] = src.pop_front().unwrap();
        }
        Ok(n)
    }

    pub fn close(&mut self, fd: SockFd) {
        if fd == self.fd1 {
            self.closed1 = true;
        } else if fd == self.fd2 {
            self.closed2 = true;
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "socketpair fd1={:?} fd2={:?} buf1={} buf2={} closed1={} closed2={}",
            self.fd1.0, self.fd2.0, self.buf1.len(), self.buf2.len(), self.closed1, self.closed2
        )
    }
}

// =====================================================================================
// 全局单例管理器
// =====================================================================================

static SHM_MGR: OnceLock<Mutex<SharedMemoryManager>> = OnceLock::new();
static MSGQ_MGR: OnceLock<Mutex<MessageQueueManager>> = OnceLock::new();
static PIPE_MGR: OnceLock<Mutex<PipeManager>> = OnceLock::new();
static SOCKET_MGR: OnceLock<Mutex<HashMap<u64, UnixSocketPair>>> = OnceLock::new();
static NEXT_SOCK_ID: OnceLock<Mutex<u64>> = OnceLock::new();

fn shm_mgr() -> &'static Mutex<SharedMemoryManager> {
    SHM_MGR.get_or_init(|| Mutex::new(SharedMemoryManager::new()))
}

fn msgq_mgr() -> &'static Mutex<MessageQueueManager> {
    MSGQ_MGR.get_or_init(|| Mutex::new(MessageQueueManager::new()))
}

fn pipe_mgr() -> &'static Mutex<PipeManager> {
    PIPE_MGR.get_or_init(|| Mutex::new(PipeManager::new()))
}

fn socket_mgr() -> &'static Mutex<HashMap<u64, UnixSocketPair>> {
    SOCKET_MGR.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_sock_id() -> &'static Mutex<u64> {
    NEXT_SOCK_ID.get_or_init(|| Mutex::new(1))
}

// --- 共享内存全局 API ---

pub fn shm_create(name: &str, size: usize) -> Result<ShmId, String> {
    shm_mgr().lock().unwrap().create(name, size)
}

pub fn shm_attach(name: &str) -> Result<(ShmId, usize), String> {
    shm_mgr().lock().unwrap().attach(name)
}

pub fn shm_detach(id: ShmId) -> Result<bool, String> {
    shm_mgr().lock().unwrap().detach_by_id(id)
}

pub fn shm_destroy(id: ShmId) -> Result<(), String> {
    shm_mgr().lock().unwrap().destroy(id)
}

pub fn shm_read(id: ShmId, offset: usize, buf: &mut [u8]) -> Result<usize, String> {
    shm_mgr().lock().unwrap().read(id, offset, buf)
}

pub fn shm_write(id: ShmId, offset: usize, buf: &[u8]) -> Result<usize, String> {
    shm_mgr().lock().unwrap().write(id, offset, buf)
}

// --- 消息队列全局 API ---

pub fn msgq_create(name: &str, max_msgs: usize, max_msg_size: usize) -> Result<MsgQId, String> {
    msgq_mgr().lock().unwrap().create(name, max_msgs, max_msg_size)
}

pub fn msgq_send(qid: MsgQId, data: &[u8], priority: i32) -> Result<(), String> {
    msgq_mgr().lock().unwrap().send(qid, data, priority)
}

pub fn msgq_recv(qid: MsgQId, timeout: Duration) -> Option<Message> {
    msgq_mgr().lock().unwrap().recv(qid, timeout)
}

pub fn msgq_destroy(qid: MsgQId) -> Result<(), String> {
    msgq_mgr().lock().unwrap().destroy(qid)
}

// --- 管道全局 API ---

pub fn pipe_create(capacity: usize) -> (PipeReadId, PipeWriteId) {
    pipe_mgr().lock().unwrap().create(capacity)
}

pub fn pipe_write(read_id: PipeReadId, buf: &[u8]) -> Result<usize, String> {
    pipe_mgr().lock().unwrap().write_by_read(read_id, buf)
}

pub fn pipe_read(read_id: PipeReadId, buf: &mut [u8]) -> Result<usize, String> {
    pipe_mgr().lock().unwrap().read_by_read(read_id, buf)
}

pub fn pipe_close_read(read_id: PipeReadId) {
    pipe_mgr().lock().unwrap().close_read(read_id)
}

pub fn pipe_close_write(write_id: PipeWriteId) {
    pipe_mgr().lock().unwrap().close_write(write_id)
}

// --- Unix socket 全局 API ---

pub fn socket_pair() -> (SockFd, SockFd) {
    let mut mgr = socket_mgr().lock().unwrap();
    let id = *next_sock_id().lock().unwrap();
    *next_sock_id().lock().unwrap() += 1;
    let fd1 = SockFd(id * 2);
    let fd2 = SockFd(id * 2 + 1);
    let pair = UnixSocketPair::new(fd1, fd2);
    mgr.insert(id, pair);
    (fd1, fd2)
}

pub fn socket_sendto(fd: SockFd, buf: &[u8]) -> Result<usize, String> {
    let mut mgr = socket_mgr().lock().unwrap();
    // 找到包含该 fd 的 socket pair
    let pair_id = mgr.keys().find(|&&k| {
        let p = &mgr[&k];
        p.fd1 == fd || p.fd2 == fd
    });
    let pair_id = match pair_id {
        Some(k) => *k,
        None => return Err("套接字不存在".to_string()),
    };
    mgr.get_mut(&pair_id).unwrap().sendto(fd, buf)
}

pub fn socket_recvfrom(fd: SockFd, buf: &mut [u8]) -> Result<usize, String> {
    let mut mgr = socket_mgr().lock().unwrap();
    let pair_id = mgr.keys().find(|&&k| {
        let p = &mgr[&k];
        p.fd1 == fd || p.fd2 == fd
    });
    let pair_id = match pair_id {
        Some(k) => *k,
        None => return Err("套接字不存在".to_string()),
    };
    mgr.get_mut(&pair_id).unwrap().recvfrom(fd, buf)
}

pub fn socket_close(fd: SockFd) {
    let mut mgr = socket_mgr().lock().unwrap();
    let pair_id = mgr.iter().find(|(_, p)| p.fd1 == fd || p.fd2 == fd).map(|(k, _)| *k);
    if let Some(k) = pair_id {
        mgr.get_mut(&k).unwrap().close(fd);
    }
}

// =====================================================================================
// 信号量（Semaphore）—— IPC 扩展第五种原语
// =====================================================================================

/// 信号量标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemId(pub u64);

/// 信号量：维护一个计数器，P 操作（wait）将计数器减 1（若为 0 则阻塞），
// V 操作（post）将计数器加 1 并唤醒一个等待者。本模拟版本为非阻谢式：
// P 成功返回 true，计数器为 0 时返回 false。
pub struct Semaphore {
    pub id: SemId,
    pub name: String,
    pub value: i64,
    pub max_value: i64,
    pub wait_count: u64,
    pub post_count: u64,
    pub destroy: bool,
}

impl Semaphore {
    pub fn new(id: SemId, name: &str, initial_value: i64, max_value: i64) -> Self {
        Self {
            id,
            name: name.to_string(),
            value: initial_value,
            max_value,
            wait_count: 0,
            post_count: 0,
            destroy: false,
        }
    }

    /// P 操作：尝试将信号量计数器减 1。若计数器 > 0 则成功并返回 true；
    // 若计数器 <= 0 则失败并返回 false（模拟非阻塞尝试）。
    pub fn wait(&mut self) -> Result<bool, String> {
        if self.destroy {
            return Err("信号量已销毁".to_string());
        }
        if self.value > 0 {
            self.value -= 1;
            self.wait_count += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// V 操作：将信号量计数器加 1（不超过 max_value），返回操作后的值。
    pub fn post(&mut self) -> Result<i64, String> {
        if self.destroy {
            return Err("信号量已销毁".to_string());
        }
        if self.value < self.max_value {
            self.value += 1;
        }
        self.post_count += 1;
        Ok(self.value)
    }

    /// 返回当前值。
    pub fn value(&self) -> i64 {
        self.value
    }

    /// 销毁信号量。
    pub fn destroy(&mut self) {
        self.destroy = true;
    }

    pub fn summary(&self) -> String {
        format!(
            "sem name={} id={:?} value={} max={} waits={} posts={} destroyed={}",
            self.name, self.id.0, self.value, self.max_value, self.wait_count, self.post_count, self.destroy
        )
    }
}

/// 信号量管理器。
pub struct SemaphoreManager {
    semaphores: HashMap<String, Semaphore>,
    name_to_id: HashMap<String, SemId>,
    next_id: u64,
}

impl SemaphoreManager {
    pub fn new() -> Self {
        Self {
            semaphores: HashMap::new(),
            name_to_id: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, name: &str, initial_value: i64, max_value: i64) -> Result<SemId, String> {
        if self.name_to_id.contains_key(name) {
            return Err(format!("信号量已存在: {}", name));
        }
        let id = SemId(self.next_id);
        self.next_id += 1;
        let sem = Semaphore::new(id, name, initial_value, max_value);
        self.name_to_id.insert(name.to_string(), id);
        self.semaphores.insert(name.to_string(), sem);
        Ok(id)
    }

    pub fn wait(&mut self, id: SemId) -> Result<bool, String> {
        let name = self.sem_name(id).ok_or_else(|| format!("信号量不存在: {:?}", id))?;
        self.semaphores.get_mut(&name).unwrap().wait()
    }

    pub fn post(&mut self, id: SemId) -> Result<i64, String> {
        let name = self.sem_name(id).ok_or_else(|| format!("信号量不存在: {:?}", id))?;
        self.semaphores.get_mut(&name).unwrap().post()
    }

    pub fn destroy(&mut self, id: SemId) -> Result<(), String> {
        let name = self.sem_name(id).ok_or_else(|| format!("信号量不存在: {:?}", id))?;
        if let Some(s) = self.semaphores.get_mut(&name) {
            s.destroy();
        }
        self.name_to_id.remove(&name);
        self.semaphores.remove(&name);
        Ok(())
    }

    fn sem_name(&self, id: SemId) -> Option<String> {
        self.semaphores
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(n, _)| n.clone())
    }

    pub fn len(&self) -> usize {
        self.semaphores.len()
    }

    pub fn summary(&self) -> String {
        format!("sem_manager semaphores={}", self.len())
    }
}

impl Default for SemaphoreManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 信号量全局单例与 API
// =====================================================================================

static SEM_MGR: OnceLock<Mutex<SemaphoreManager>> = OnceLock::new();

fn sem_mgr() -> &'static Mutex<SemaphoreManager> {
    SEM_MGR.get_or_init(|| Mutex::new(SemaphoreManager::new()))
}

/// 创建信号量（全局）。
pub fn sem_create(name: &str, initial_value: i64, max_value: i64) -> Result<SemId, String> {
    sem_mgr().lock().unwrap().create(name, initial_value, max_value)
}

/// P 操作（全局）。
pub fn sem_wait(id: SemId) -> Result<bool, String> {
    sem_mgr().lock().unwrap().wait(id)
}

/// V 操作（全局）。
pub fn sem_post(id: SemId) -> Result<i64, String> {
    sem_mgr().lock().unwrap().post(id)
}

/// 销毁信号量（全局）。
pub fn sem_destroy(id: SemId) -> Result<(), String> {
    sem_mgr().lock().unwrap().destroy(id)
}

/// 返回信号量摘要（全局）。
pub fn sem_summary() -> String {
    sem_mgr().lock().unwrap().summary()
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shm_create_attach_read_write_destroy() {
        let mut mgr = SharedMemoryManager::new();
        let id = mgr.create("shm://test", 4096).unwrap();
        let (aid, acnt) = mgr.attach("shm://test").unwrap();
        assert_eq!(aid, id);
        assert_eq!(acnt, 1);
        let data = b"hello shm";
        mgr.write(id, 0, data).unwrap();
        let mut buf = [0u8; 9];
        let n = mgr.read(id, 0, &mut buf).unwrap();
        assert_eq!(n, 9);
        assert_eq!(&buf, b"hello shm");
        assert!(mgr.destroy(id).is_ok());
        assert_eq!(mgr.read(id, 0, &mut buf).unwrap_err().as_str(), "共享内存已销毁");
    }

    #[test]
    fn shm_duplicate_create_rejected() {
        let mut mgr = SharedMemoryManager::new();
        let _ = mgr.create("shm://dup", 100);
        assert_eq!(mgr.create("shm://dup", 100).unwrap_err().as_str(), "共享内存已存在: shm://dup");
    }

    #[test]
    fn msgq_send_recv_priority() {
        let mut mgr = MessageQueueManager::new();
        let qid = mgr.create("mq://test", 10, 256).unwrap();
        mgr.send(qid, b"low", 1).unwrap();
        mgr.send(qid, b"high", 10).unwrap();
        mgr.send(qid, b"mid", 5).unwrap();
        // 优先级最高的先被接收
        let m1 = mgr.recv(qid, Duration::from_secs(0)).unwrap();
        assert_eq!(&m1.data, b"high");
        let m2 = mgr.recv(qid, Duration::from_secs(0)).unwrap();
        assert_eq!(&m2.data, b"mid");
        let m3 = mgr.recv(qid, Duration::from_secs(0)).unwrap();
        assert_eq!(&m3.data, b"low");
        assert!(mgr.recv(qid, Duration::from_secs(0)).is_none());
        assert!(mgr.destroy(qid).is_ok());
    }

    #[test]
    fn msgq_full_rejected() {
        let mut mgr = MessageQueueManager::new();
        let qid = mgr.create("mq://full", 2, 64).unwrap();
        mgr.send(qid, b"a", 1).unwrap();
        mgr.send(qid, b"b", 1).unwrap();
        assert_eq!(mgr.send(qid, b"c", 1).unwrap_err().as_str(), "消息队列已满");
    }

    #[test]
    fn pipe_write_read() {
        let mut mgr = PipeManager::new();
        let (rid, wid) = mgr.create(64);
        let _ = wid;
        let data = b"hello pipe";
        assert_eq!(mgr.write_by_read(rid, data).unwrap(), 10);
        let mut buf = [0u8; 10];
        let n = mgr.read_by_read(rid, &mut buf).unwrap();
        assert_eq!(n, 10);
        assert_eq!(&buf, b"hello pipe");
        mgr.close_read(rid);
        mgr.close_write(wid);
    }

    #[test]
    fn pipe_capacity_limit() {
        let mut mgr = PipeManager::new();
        let (rid, _) = mgr.create(4);
        let data = b"12345678";
        assert_eq!(mgr.write_by_read(rid, data).unwrap(), 4); // 仅写入 4 字节
    }

    #[test]
    fn socket_pair_send_recv() {
        let (fd1, fd2) = socket_pair();
        let data = b"hello socket";
        assert_eq!(socket_sendto(fd1, data).unwrap(), 12);
        let mut buf = [0u8; 12];
        let n = socket_recvfrom(fd2, &mut buf).unwrap();
        assert_eq!(n, 12);
        assert_eq!(&buf, b"hello socket");
        socket_close(fd1);
        socket_close(fd2);
    }

    #[test]
    fn socket_sendto_unknown_fd_rejected() {
        assert_eq!(
            socket_sendto(SockFd(9999), b"x").unwrap_err().as_str(),
            "套接字不存在"
        );
    }

    #[test]
    fn global_ipc_functions() {
        let id = shm_create("gshm", 256).unwrap();
        let (aid, _) = shm_attach("gshm").unwrap();
        assert_eq!(aid, id);
        shm_write(id, 0, b"g").unwrap();
        let mut buf = [0u8; 1];
        shm_read(id, 0, &mut buf).unwrap();
        assert_eq!(buf, [b'g']);
        shm_destroy(id).unwrap();

        let qid = msgq_create("gmq", 5, 64).unwrap();
        msgq_send(qid, b"gm", 3).unwrap();
        let m = msgq_recv(qid, Duration::from_secs(0)).unwrap();
        assert_eq!(&m.data, b"gm");
        msgq_destroy(qid).unwrap();

        let (rid, wid) = pipe_create(16);
        pipe_write(rid, b"gp").unwrap();
        let mut buf = [0u8; 2];
        pipe_read(rid, &mut buf).unwrap();
        assert_eq!(&buf, b"gp");
        pipe_close_read(rid);
        pipe_close_write(wid);

        let (fd1, fd2) = socket_pair();
        socket_sendto(fd1, b"gs").unwrap();
        let mut buf = [0u8; 2];
        socket_recvfrom(fd2, &mut buf).unwrap();
        socket_close(fd1);
        socket_close(fd2);
    }

    #[test]
    fn summaries() {
        let mut shm = SharedMemoryManager::new();
        let _ = shm.create("s1", 100);
        let _s = shm.summary();
        let _ = _s;

        let mut mq = MessageQueueManager::new();
        let _ = mq.create("q1", 5, 32);
        let _s = mq.summary();
        let _ = _s;

        let mut pm = PipeManager::new();
        let _ = pm.create(16);
        let _s = pm.summary();
        let _ = _s;
    }

    #[test]
    fn semaphore_wait_post() {
        let mut mgr = SemaphoreManager::new();
        let id = mgr.create("sem://test", 1, 5).unwrap();
        assert_eq!(mgr.wait(id).unwrap(), true); // 计数器 1 -> 0
        assert_eq!(mgr.wait(id).unwrap(), false); // 计数器 0，失败
        assert_eq!(mgr.post(id).unwrap(), 1); // 计数器 0 -> 1
        assert_eq!(mgr.wait(id).unwrap(), true); // 计数器 1 -> 0
        assert!(mgr.destroy(id).is_ok());
    }

    #[test]
    fn semaphore_max_value_clamp() {
        let mut mgr = SemaphoreManager::new();
        let id = mgr.create("sem://cap", 2, 3).unwrap();
        // post 多次不应超过 max_value
        let _ = mgr.post(id);
        let _ = mgr.post(id);
        let _ = mgr.post(id);
        assert_eq!(mgr.post(id).unwrap(), 3); // 已达到上限，保持 3
    }

    #[test]
    fn semaphore_destroyed_rejected() {
        let mut mgr = SemaphoreManager::new();
        let id = mgr.create("sem://dead", 1, 5).unwrap();
        mgr.destroy(id).unwrap();
        assert_eq!(mgr.wait(id).unwrap_err().as_str(), "信号量已销毁");
        assert_eq!(mgr.post(id).unwrap_err().as_str(), "信号量已销毁");
    }

    #[test]
    fn semaphore_duplicate_create_rejected() {
        let mut mgr = SemaphoreManager::new();
        let _ = mgr.create("sem://dup", 1, 5);
        assert_eq!(
            mgr.create("sem://dup", 1, 5).unwrap_err().as_str(),
            "信号量已存在: sem://dup"
        );
    }

    #[test]
    fn global_semaphore_functions() {
        let id = sem_create("gsem", 3, 10).unwrap();
        assert_eq!(sem_wait(id).unwrap(), true);
        assert_eq!(sem_post(id).unwrap(), 3);
        assert!(sem_destroy(id).is_ok());
        let s = sem_summary();
        assert!(s.contains("sem_manager"));
    }

    #[test]
    fn semaphore_summary_text() {
        let mut mgr = SemaphoreManager::new();
        let _ = mgr.create("s1", 0, 5);
        let s = mgr.summary();
        assert!(s.contains("semaphores="));
    }
}
