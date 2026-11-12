//! 进程子系统 —— AI 原生的「行动执行」。
//!
//! fork/exec 映射为**真实的主机进程派生**（`std::process`），信号为终止语义，
//! 进程树与状态可被 `sense proc://manager` 直接感知。

use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock};

use crate::monitor::get_time_ns;

pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub state: String,
    pub uid: u32,
    pub started_ns: u128,
    pub child: Option<Child>,
}

pub struct ProcTable {
    procs: Vec<ProcInfo>,
    next_pid: u32,
}

impl ProcTable {
    fn new() -> Self {
        let mut p = Self { procs: Vec::new(), next_pid: 2 };
        p.procs.push(ProcInfo {
            pid: 1,
            ppid: 0,
            name: "hibos-kernel".to_string(),
            state: "running".to_string(),
            uid: 0,
            started_ns: 0,
            child: None,
        });
        p
    }

    /// fork/exec：真实派生一个主机进程。
    pub fn spawn(&mut self, cmdline: &str) -> Result<String, String> {
        let cmdline = cmdline.trim();
        if cmdline.is_empty() {
            return Err("用法: act proc://manager spawn <命令>".to_string());
        }
        let pid = self.next_pid;
        self.next_pid += 1;

        let mut cmd = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
        if cfg!(windows) {
            cmd.arg("/C");
        } else {
            cmd.arg("-c");
        }
        cmd.arg(cmdline);

        match cmd.spawn() {
            Ok(child) => {
                let name = cmdline.split_whitespace().next().unwrap_or("proc").to_string();
                let uid = crate::user::userdb().lock().unwrap().current_uid;
                self.procs.push(ProcInfo {
                    pid,
                    ppid: 1,
                    name,
                    state: "running".to_string(),
                    uid,
                    started_ns: get_time_ns() as u128,
                    child: Some(child),
                });
                Ok(format!("已 fork/exec pid={} \"{}\"", pid, cmdline))
            }
            Err(e) => Err(format!("spawn 失败: {}", e)),
        }
    }

    /// 向进程发送信号（Windows 下为终止语义）。
    pub fn signal(&mut self, pid: u32, sig: &str) -> Result<String, String> {
        let Some(p) = self.procs.iter_mut().find(|p| p.pid == pid) else {
            return Err(format!("进程 {} 不存在", pid));
        };
        if let Some(child) = p.child.as_mut() {
            let _ = child.kill();
            p.state = format!("killed({})", sig);
            Ok(format!("已向 pid={} 发送信号 {} (已终止)", pid, sig))
        } else {
            p.state = format!("signaled({})", sig);
            Ok(format!("pid={} 是内核自身，信号 {} 仅记录", pid, sig))
        }
    }

    fn refresh(&mut self) {
        for p in self.procs.iter_mut() {
            if let Some(child) = p.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    p.state = format!("exited({:?})", status.code());
                    p.child = None;
                }
            }
        }
    }

    /// 进程树文本。
    pub fn tree(&mut self) -> String {
        self.refresh();
        let mut out = String::from("hibos-kernel (1) [running] uid=0\n");
        fn walk(t: &ProcTable, pid: u32, depth: usize, out: &mut String) {
            for p in t.procs.iter().filter(|p| p.ppid == pid) {
                out.push_str(&format!(
                    "{}{} ({}) [{}] uid={}\n",
                    "  ".repeat(depth),
                    p.name,
                    p.pid,
                    p.state,
                    p.uid
                ));
                walk(t, p.pid, depth + 1, out);
            }
        }
        walk(self, 1, 1, &mut out);
        out.trim_end().to_string()
    }

    /// 进程总数（含内核自身），供 procfs 汇总负载。
    pub fn count(&mut self) -> usize {
        self.refresh();
        self.procs.len()
    }

    /// 扁平进程列表。
    pub fn list(&mut self) -> String {
        self.refresh();
        let mut out = String::new();
        for p in &self.procs {
            out.push_str(&format!(
                "pid={:<4} ppid={:<4} {:<16} {:<16} uid={}\n",
                p.pid, p.ppid, p.name, p.state, p.uid
            ));
        }
        out.trim_end().to_string()
    }
}

static PROCTABLE: OnceLock<Mutex<ProcTable>> = OnceLock::new();

/// 全局进程表（`proc://manager` 的实体）。
pub fn proctable() -> &'static Mutex<ProcTable> {
    PROCTABLE.get_or_init(|| Mutex::new(ProcTable::new()))
}
