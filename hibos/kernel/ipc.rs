//! 进程间通信 (IPC) —— AI 原生的「协作通道」。
//!
//! 管道 / 共享内存 / 消息队列 / 信号量，全部建模为可感知 / 可写入的通道。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};

pub struct Pipe {
    pub id: u64,
    pub buf: Vec<u8>,
    pub pos: usize,
}

pub struct Ipc {
    pub next_id: u64,
    pub pipes: Vec<Pipe>,
    pub shm: BTreeMap<String, Vec<u8>>,
    pub msgq: BTreeMap<String, VecDeque<String>>,
    pub sem: BTreeMap<String, i64>,
}

impl Ipc {
    fn new() -> Self {
        Self {
            next_id: 1,
            pipes: Vec::new(),
            shm: BTreeMap::new(),
            msgq: BTreeMap::new(),
            sem: BTreeMap::new(),
        }
    }

    pub fn pipe_create(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.pipes.push(Pipe { id, buf: Vec::new(), pos: 0 });
        format!("已创建管道 id={}", id)
    }

    pub fn pipe_write(&mut self, id: u64, data: &str) -> Result<String, String> {
        let p = self
            .pipes
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("管道 {} 不存在", id))?;
        p.buf.extend_from_slice(data.as_bytes());
        Ok(format!("已写入管道 {}: {}B (共 {}B)", id, data.len(), p.buf.len()))
    }

    pub fn pipe_read(&mut self, id: u64) -> Result<String, String> {
        let p = self
            .pipes
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("管道 {} 不存在", id))?;
        let rest = &p.buf[p.pos..];
        if rest.is_empty() {
            return Err(format!("管道 {} 无剩余数据", id));
        }
        let s = String::from_utf8_lossy(rest).to_string();
        p.pos = p.buf.len();
        Ok(format!("从管道 {} 读出: {}", id, s))
    }

    pub fn pipes(&self) -> String {
        if self.pipes.is_empty() {
            return "（无管道）".to_string();
        }
        self.pipes
            .iter()
            .map(|p| format!("pipe id={} size={}B pos={}", p.id, p.buf.len(), p.pos))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn shm_create(&mut self, name: &str, size: usize) -> String {
        self.shm.insert(name.to_string(), vec![0u8; size]);
        format!("已创建共享内存 {} ({}B)", name, size)
    }

    pub fn shm_write(&mut self, name: &str, data: &str) -> Result<String, String> {
        let seg = self.shm.get_mut(name).ok_or_else(|| format!("共享内存 {} 不存在", name))?;
        let n = data.len().min(seg.len());
        seg[..n].copy_from_slice(&data.as_bytes()[..n]);
        Ok(format!("写入共享内存 {}: {}B", name, n))
    }

    pub fn shm_read(&self, name: &str) -> Result<String, String> {
        let seg = self.shm.get(name).ok_or_else(|| format!("共享内存 {} 不存在", name))?;
        let end = seg.iter().position(|b| *b == 0).unwrap_or(seg.len());
        let s = String::from_utf8_lossy(&seg[..end]).to_string();
        Ok(format!("共享内存 {} ({}B): {}", name, seg.len(), s))
    }

    pub fn shm_list(&self) -> String {
        if self.shm.is_empty() {
            return "（无共享内存段）".to_string();
        }
        self.shm
            .iter()
            .map(|(k, v)| format!("shm {} = {}B", k, v.len()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn msgq_send(&mut self, name: &str, msg: &str) -> Result<String, String> {
        self.msgq
            .entry(name.to_string())
            .or_insert_with(VecDeque::new)
            .push_back(msg.to_string());
        Ok(format!("已向消息队列 {} 发送: {}", name, msg))
    }

    pub fn msgq_recv(&mut self, name: &str) -> Result<String, String> {
        let q = self.msgq.get_mut(name).ok_or_else(|| format!("消息队列 {} 不存在", name))?;
        match q.pop_front() {
            Some(m) => Ok(format!("从消息队列 {} 收到: {}", name, m)),
            None => Err(format!("消息队列 {} 为空", name)),
        }
    }

    pub fn msgq_list(&self) -> String {
        if self.msgq.is_empty() {
            return "（无消息队列）".to_string();
        }
        self.msgq
            .iter()
            .map(|(k, v)| format!("msgq {} = {} 条", k, v.len()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn sem_init(&mut self, name: &str, n: i64) -> String {
        self.sem.insert(name.to_string(), n);
        format!("已初始化信号量 {} = {}", name, n)
    }

    pub fn sem_post(&mut self, name: &str) -> Result<String, String> {
        let s = self.sem.get_mut(name).ok_or_else(|| format!("信号量 {} 不存在", name))?;
        *s += 1;
        Ok(format!("信号量 {} 已 post -> {}", name, *s))
    }

    pub fn sem_wait(&mut self, name: &str) -> Result<String, String> {
        let s = self.sem.get_mut(name).ok_or_else(|| format!("信号量 {} 不存在", name))?;
        if *s <= 0 {
            return Err(format!("信号量 {} 已阻塞 (值为 0)", name));
        }
        *s -= 1;
        Ok(format!("信号量 {} 已 wait -> {}", name, *s))
    }

    pub fn sem_list(&self) -> String {
        if self.sem.is_empty() {
            return "（无信号量）".to_string();
        }
        self.sem
            .iter()
            .map(|(k, v)| format!("sem {} = {}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

static IPC: OnceLock<Mutex<Ipc>> = OnceLock::new();

pub fn ipc() -> &'static Mutex<Ipc> {
    IPC.get_or_init(|| Mutex::new(Ipc::new()))
}
