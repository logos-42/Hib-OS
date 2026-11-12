//! 浏览器通道 —— AI 原生的「外部页面感知」。
//!
//! `browser://gateway` 之前只是占位符（"offline"）。现在它真正落地：
//!
//! * 抓取 `http(s)://` 页面（真实网络，经 `gateway://net` 出站策略检查）
//! * 提取 `<title>`、把 HTML 转成纯文本、记录历史与字节数
//! * `file://` 仍由 shell 层直接读 VFS（本模块不碰文件系统）
//!
//! * `sense browser://gateway`
//! * `act browser://gateway open <url>`
//! * `act browser://gateway text [字符数]`
//! * `act browser://gateway history`

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::gateway::gateway;

/// 抓取超时（秒）。
const TIMEOUT_SECS: u64 = 6;
/// 正文缓存上限（字符）。
const BODY_LIMIT: usize = 8192;

/// 浏览器状态。
pub struct Browser {
    pub last_url: String,
    pub last_status: u16,
    pub last_title: String,
    pub last_bytes: usize,
    /// 最近一次页面的纯文本正文。
    pub body: String,
    /// 历史记录（最多 20 条）。
    pub history: Vec<String>,
}

impl Browser {
    fn new() -> Self {
        Self {
            last_url: "（尚未打开页面）".to_string(),
            last_status: 0,
            last_title: String::new(),
            last_bytes: 0,
            body: String::new(),
            history: Vec::new(),
        }
    }

    /// 感知快照。
    pub fn status(&self) -> String {
        let recent = self
            .history
            .iter()
            .rev()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ");
        format!(
            "url={} status={} title=\"{}\" bytes={} body={}B history={}\n  {}",
            self.last_url,
            self.last_status,
            self.last_title,
            self.last_bytes,
            self.body.len(),
            self.history.len(),
            if recent.is_empty() {
                "(空)"
            } else {
                recent.as_str()
            }
        )
    }

    /// 打开一个 http(s) 页面。
    pub fn open(&mut self, url: &str) -> Result<String, String> {
        let url = url.trim();
        let host = host_of(url).ok_or_else(|| {
            format!("URL 需形如 http(s)://host/path，收到: {}", url)
        })?;

        // 出站策略检查（不通过则不发起任何网络请求）。
        {
            let mut gw = gateway().lock().unwrap();
            if !gw.check(&host) {
                gw.record(&host, false, 0);
                return Err(format!("网关拦截: {} 被出站策略拒绝", host));
            }
        }

        match ureq::get(url).timeout(Duration::from_secs(TIMEOUT_SECS)).call() {
            Ok(resp) => {
                let status = resp.status();
                let html = resp.into_string().unwrap_or_default();
                self.finish(url, &host, status, html)
            }
            // 4xx / 5xx 也带回响应体，仍然展示给 AI
            Err(ureq::Error::Status(code, resp)) => {
                let html = resp.into_string().unwrap_or_default();
                self.finish(url, &host, code, html)
            }
            Err(e) => {
                gateway().lock().unwrap().record(&host, true, 0);
                Err(format!("抓取失败: {}", e))
            }
        }
    }

    /// 统一的收尾：提取标题、转正文、写历史、记账。
    fn finish(
        &mut self,
        url: &str,
        host: &str,
        status: u16,
        html: String,
    ) -> Result<String, String> {
        let title = extract_title(&html);
        let text: String = html_to_text(&html).chars().take(BODY_LIMIT).collect();

        self.last_url = url.to_string();
        self.last_status = status;
        self.last_title = title.clone();
        self.last_bytes = html.len();
        self.body = text;

        self.history.push(format!("{} [{}] {}", url, status, title));
        if self.history.len() > 20 {
            self.history.remove(0);
        }

        gateway().lock().unwrap().record(host, true, html.len());

        Ok(format!(
            "{} -> {} ({}) {}B, 正文 {}B",
            url,
            status,
            title,
            html.len(),
            self.body.len()
        ))
    }

    /// 记录一次本地 `file://` 打开（内容由 shell 从 VFS 读出后传入）。
    pub fn note_local(&mut self, url: &str, content: &str) {
        self.last_url = url.to_string();
        self.last_status = 200;
        self.last_title = "(本地文件)".to_string();
        self.last_bytes = content.len();
        self.body = content.chars().take(BODY_LIMIT).collect();
        self.history.push(format!("{} [200] (本地文件)", url));
        if self.history.len() > 20 {
            self.history.remove(0);
        }
    }

    /// 返回正文前 `limit` 个字符。
    pub fn text(&self, limit: usize) -> String {
        if self.body.is_empty() {
            return "（无正文，先 open 一个页面）".to_string();
        }
        self.body.chars().take(limit).collect()
    }
}

/// 从 URL 中取出主机名。
fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host = rest.split('/').next()?.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// 提取 `<title>` 文本。
fn extract_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<title") else {
        return "(无标题)".to_string();
    };
    let Some(gt) = lower[start..].find('>') else {
        return "(无标题)".to_string();
    };
    let content_start = start + gt + 1;
    let Some(end) = lower[content_start..].find("</title>") else {
        return "(无标题)".to_string();
    };
    let title = html[content_start..content_start + end].trim();
    if title.is_empty() {
        "(空标题)".to_string()
    } else {
        title.to_string()
    }
}

/// 剥离 HTML 标签并压缩空白。
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

static BROWSER: OnceLock<Mutex<Browser>> = OnceLock::new();

/// 全局浏览器（`browser://gateway` 的实体）。
pub fn browser() -> &'static Mutex<Browser> {
    BROWSER.get_or_init(|| Mutex::new(Browser::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_parsing() {
        assert_eq!(host_of("http://example.com/a/b").unwrap(), "example.com");
        assert_eq!(host_of("https://Example.COM").unwrap(), "example.com");
        assert_eq!(host_of("http://example.com:8080/x").unwrap(), "example.com");
        assert!(host_of("file:///etc/passwd").is_none());
        assert!(host_of("ftp://example.com").is_none());
        assert!(host_of("http://").is_none());
    }

    #[test]
    fn title_extraction() {
        let html = "<html><head><Title>Hib OS</Title></head><body>x</body></html>";
        assert_eq!(extract_title(html), "Hib OS");
        assert_eq!(extract_title("<html><body>no title</body></html>"), "(无标题)");
        assert_eq!(extract_title("<title></title>"), "(空标题)");
    }

    #[test]
    fn html_becomes_text() {
        let html = "<h1>Hello</h1><p>world &amp;   friends</p>";
        let text = html_to_text(html);
        assert_eq!(text, "Hello world &amp; friends");
        assert!(!text.contains('<'));
    }

    #[test]
    fn blocked_by_gateway_without_network() {
        // 默认 deny + 不命中白名单 => 在发起网络请求之前就被拦下
        {
            let mut gw = gateway().lock().unwrap();
            gw.set_policy("deny").unwrap();
        }
        let mut b = Browser::new();
        let err = b.open("http://blocked.invalid/").unwrap_err();
        assert!(err.contains("网关拦截"), "{}", err);
        // 恢复默认策略，避免影响其他测试
        gateway().lock().unwrap().set_policy("allow").unwrap();
    }

    #[test]
    fn status_and_text_when_empty() {
        let b = Browser::new();
        assert!(b.status().contains("history=0"));
        assert!(b.text(10).contains("无正文"));
    }
}
