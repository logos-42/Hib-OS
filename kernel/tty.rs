//! 终端 / 作业控制 —— AI 原生的「注意力焦点」。
//!
//! 把前台 / 后台作业、进程组绑定到 `tty://jobs` 通道。

use std::sync::{Mutex, OnceLock};

pub struct Tty {
    pub fg: Option<u32>,
    pub suspended: Vec<u32>,
}

impl Tty {
    fn new() -> Self {
        Self { fg: None, suspended: Vec::new() }
    }

    pub fn jobs(&mut self) -> String {
        let mut t = crate::proc::proctable().lock().unwrap();
        let list = t.list();
        let fg = self.fg;
        let mut out = String::new();
        for line in list.lines() {
            let pid = line
                .split_whitespace()
                .next()
                .and_then(|s| s.trim_start_matches("pid=").parse::<u32>().ok());
            let mark = if pid == fg { "  [fg]" } else { "" };
            out.push_str(&format!("{}{}\n", line, mark));
        }
        out.trim_end().to_string()
    }

    pub fn fg(&mut self, pid: u32) -> String {
        self.fg = Some(pid);
        self.suspended.retain(|p| *p != pid);
        format!("作业 {} 已切到前台", pid)
    }

    pub fn bg(&mut self, pid: u32) -> String {
        if self.fg == Some(pid) {
            self.fg = None;
        }
        if !self.suspended.contains(&pid) {
            self.suspended.push(pid);
        }
        format!("作业 {} 已放到后台", pid)
    }

    pub fn kill(&mut self, pid: u32) -> Result<String, String> {
        if self.fg == Some(pid) {
            self.fg = None;
        }
        self.suspended.retain(|p| *p != pid);
        crate::proc::proctable().lock().unwrap().signal(pid, "TERM")
    }
}

static TTY: OnceLock<Mutex<Tty>> = OnceLock::new();

pub fn tty() -> &'static Mutex<Tty> {
    TTY.get_or_init(|| Mutex::new(Tty::new()))
}
