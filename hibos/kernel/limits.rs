//! 资源限制 (rlimit) —— AI 原生的「能力边界」。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

pub struct Limits {
    pub r: BTreeMap<String, u64>,
}

impl Limits {
    fn new() -> Self {
        let mut r = BTreeMap::new();
        for (k, v) in [
            ("nofile", 1024u64),
            ("nproc", 64),
            ("memlock_kb", 65536),
            ("stack_kb", 8192),
            ("core", 0),
            ("max_mmap_regions", 65530),
        ] {
            r.insert(k.to_string(), v);
        }
        Self { r }
    }

    pub fn list(&self) -> String {
        self.r
            .iter()
            .map(|(k, v)| format!("{:<20} {}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn set(&mut self, k: &str, v: u64) -> Result<String, String> {
        if !self.r.contains_key(k) {
            return Err(format!("未知限制: {}", k));
        }
        self.r.insert(k.to_string(), v);
        Ok(format!("{} = {}", k, v))
    }

    pub fn get(&self, k: &str) -> String {
        match self.r.get(k) {
            Some(v) => format!("{} = {}", k, v),
            None => format!("（未知限制 {}）", k),
        }
    }
}

static LIMITS: OnceLock<Mutex<Limits>> = OnceLock::new();

pub fn limits() -> &'static Mutex<Limits> {
    LIMITS.get_or_init(|| Mutex::new(Limits::new()))
}
