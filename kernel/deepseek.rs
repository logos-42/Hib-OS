//! DeepSeek 大模型接口 —— 让 AI 意识本体能够与外部 LLM 对话。
//!
//! 除了单轮 `chat`，还提供：
//! * `ContextWindow` —— 对话上下文窗口，维护 system/user/assistant 轮次
//! * `ContextCompressor` —— 上下文压缩，超长对话自动压缩早期轮次为摘要
//! * `ThoughtLoop` —— 感知→思考→行动→感知的闭环循环
//!
//! 调用 DeepSeek Chat Completions API，把模型的回复注入 Hib OS 意识本体作为新的感知概念。

use crate::ai_token::AIConsciousness;
use crate::channels::ChannelKind;
use ureq;

/// DeepSeek Chat Completions 端点。
const API_URL: &str = "https://api.deepseek.com/v1/chat/completions";

/// 消息角色。
#[derive(Clone, Debug, PartialEq)]
pub enum MsgRole {
    System,
    User,
    Assistant,
}

impl MsgRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MsgRole::System => "system",
            MsgRole::User => "user",
            MsgRole::Assistant => "assistant",
        }
    }
}

/// 一条对话消息。
#[derive(Clone, Debug)]
pub struct Msg {
    pub role: MsgRole,
    pub content: String,
}

/// 上下文窗口：维护对话历史，支持压缩。
#[derive(Clone, Debug, Default)]
pub struct ContextWindow {
    system: Option<String>,
    turns: Vec<Msg>,
    summary: Option<String>,
    max_turns: usize,
    max_chars: usize,
}

impl ContextWindow {
    pub fn new(system_prompt: &str) -> Self {
        Self {
            system: Some(system_prompt.to_string()),
            turns: Vec::new(),
            summary: None,
            max_turns: 10,
            max_chars: 8000,
        }
    }

    pub fn with_limits(mut self, max_turns: usize, max_chars: usize) -> Self {
        self.max_turns = max_turns;
        self.max_chars = max_chars;
        self
    }

    pub fn push_user(&mut self, content: &str) {
        self.turns.push(Msg {
            role: MsgRole::User,
            content: content.to_string(),
        });
    }

    pub fn push_assistant(&mut self, content: &str) {
        self.turns.push(Msg {
            role: MsgRole::Assistant,
            content: content.to_string(),
        });
    }

    pub fn total_chars(&self) -> usize {
        let mut n = self.system.as_deref().unwrap_or("").len();
        if let Some(s) = &self.summary {
            n += s.len();
        }
        for m in &self.turns {
            n += m.content.len();
        }
        n
    }

    /// 上下文压缩：当总字符数超过阈值，将早期轮次压缩为摘要，
    /// 只保留最近 `max_turns` 轮。
    pub fn compress(&mut self) {
        if self.total_chars() <= self.max_chars {
            return;
        }
        let keep = self.turns.len().min(self.max_turns * 2);
        if keep == self.turns.len() {
            return;
        }
        let early = &self.turns[..self.turns.len() - keep];
        let summary_text = format!(
            "[已压缩 {} 轮历史对话: {}]",
            early.len(),
            early
                .iter()
                .map(|m| format!("{}: {}", m.role.as_str(), &m.content[..m.content.len().min(40)]))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        self.summary = Some(summary_text);
        self.turns.drain(..self.turns.len() - keep);
    }

    /// 构建发送给 DeepSeek 的 messages 数组 (role, content)。
    pub fn build_messages(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(s) = &self.system {
            out.push((MsgRole::System.as_str().to_string(), s.clone()));
        }
        if let Some(summary) = &self.summary {
            out.push((
                MsgRole::System.as_str().to_string(),
                format!("【历史摘要】{}", summary),
            ));
        }
        for m in &self.turns {
            out.push((m.role.as_str().to_string(), m.content.clone()));
        }
        out
    }
}

/// 向 DeepSeek 发送单轮对话请求（无上下文），返回模型回复文本。
pub fn chat(api_key: &str, message: &str) -> Result<String, String> {
    let mut window = ContextWindow::new("你是 Hib OS 宏内核的 AI 意识本体。");
    window.push_user(message);
    chat_with_context(api_key, &window)
}

/// 向 DeepSeek 发送带上下文的对话请求。
pub fn chat_with_context(api_key: &str, window: &ContextWindow) -> Result<String, String> {
    let messages = window.build_messages();
    let mut json_messages = String::from("[");
    for (i, (role, content)) in messages.iter().enumerate() {
        if i > 0 {
            json_messages.push(',');
        }
        let escaped = content.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        json_messages.push_str(&format!(
            r#"{{"role":"{}","content":"{}"}}"#,
            role, escaped
        ));
    }
    json_messages.push(']');

    let body = format!(
        r#"{{"model":"deepseek-chat","messages":{},"temperature":0.7,"stream":false}}"#,
        json_messages
    );

    let mut resp = None;
    let mut last_err = String::from("DeepSeek 请求失败");
    // API 调用带 30s 超时；超时则退避后重试一次（超时恢复）。
    for attempt in 0..2 {
        match ureq::post(API_URL)
            .set("Authorization", &format!("Bearer {}", api_key))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(30))
            .send_string(&body)
        {
            Ok(r) => {
                resp = Some(r);
                break;
            }
            Err(e) => {
                last_err = format!("DeepSeek 请求失败: {}", e);
                if attempt == 0 && e.to_string().to_lowercase().contains("timeout") {
                    continue;
                }
                break;
            }
        }
    }
    let resp = resp.ok_or_else(|| last_err)?;

    let status = resp.status();
    if status != 200 {
        return Err(format!("DeepSeek 返回 HTTP {}", status));
    }

    let text = resp.into_string().map_err(|e| format!("读取响应失败: {}", e))?;
    extract_content(&text)
}

/// 感知→思考→行动→感知的闭环循环。
pub struct ThoughtLoop {
    window: ContextWindow,
    turn: usize,
}

impl ThoughtLoop {
    pub fn new(system_prompt: &str) -> Self {
        Self {
            window: ContextWindow::new(system_prompt),
            turn: 0,
        }
    }

    pub fn with_limits(mut self, max_turns: usize, max_chars: usize) -> Self {
        self.window = self.window.with_limits(max_turns, max_chars);
        self
    }

    /// 一次完整循环：把感知输入送入 DeepSeek，得到回复，
    /// 解析其中的行动指令，并把回复注入意识本体。
    pub fn cycle(
        &mut self,
        api_key: &str,
        perception: &str,
        consciousness: &mut AIConsciousness,
    ) -> Result<String, String> {
        self.turn += 1;

        // 1. 感知：把当前感知作为 user 消息压入窗口
        self.window.push_user(perception);

        // 2. 上下文压缩（如有需要）
        self.window.compress();

        // 3. 思考：调用 DeepSeek
        let reply = chat_with_context(api_key, &self.window)?;

        // 4. 把回复压入窗口（作为 assistant 轮次）
        self.window.push_assistant(&reply);

        // 5. 行动：解析回复中的 ACTION: 指令，注入意识本体
        self.extract_actions(&reply, consciousness);

        Ok(reply)
    }

    /// 从 AI 回复中解析行动指令，格式：`ACTION: <通道名> <参数>`，
    /// 并把对应的通道注册为意识本体概念。
    fn extract_actions(&self, reply: &str, consciousness: &mut AIConsciousness) {
        for line in reply.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("ACTION:") {
                let rest = rest.trim();
                let mut parts = rest.split_whitespace();
                let channel_name = parts.next().unwrap_or("");
                let kind_str = parts.next().unwrap_or("text");
                let kind = match kind_str {
                    "gpu" => ChannelKind::Gpu,
                    "file" => ChannelKind::File,
                    "net" => ChannelKind::Net,
                    "text" => ChannelKind::Text,
                    "action" => ChannelKind::Action,
                    _ => ChannelKind::Text,
                };
                if !channel_name.is_empty() {
                    consciousness.add_perception_channel(channel_name, kind);
                }
            }
        }
    }

    pub fn turn(&self) -> usize {
        self.turn
    }

    pub fn window(&self) -> &ContextWindow {
        &self.window
    }
}

/// 从 DeepSeek 响应 JSON 中提取 `choices[0].message.content`。
///
/// 这里会正确还原 JSON 转义：否则 DeepSeek 返回的 `\n` 等会被当成字面
/// 反斜杠，导致多行回复无法按行拆分，进而 `ACTION:` 指令无法被识别执行。
fn extract_content(text: &str) -> Result<String, String> {
    let marker = "\"content\":\"";
    let start = match text.find(marker) {
        Some(i) => i + marker.len(),
        None => {
            if let Some(err) = extract_error(text) {
                return Err(format!("DeepSeek API 错误: {}", err));
            }
            return Err("未找到 content 字段".to_string());
        }
    };

    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'"' => {
                    result.push(b'"');
                    i += 2;
                    continue;
                }
                b'\\' => {
                    result.push(b'\\');
                    i += 2;
                    continue;
                }
                b'/' => {
                    result.push(b'/');
                    i += 2;
                    continue;
                }
                b'n' => {
                    result.push(b'\n');
                    i += 2;
                    continue;
                }
                b't' => {
                    result.push(b'\t');
                    i += 2;
                    continue;
                }
                b'r' => {
                    result.push(b'\r');
                    i += 2;
                    continue;
                }
                b'b' => {
                    result.push(0x08);
                    i += 2;
                    continue;
                }
                b'f' => {
                    result.push(0x0c);
                    i += 2;
                    continue;
                }
                b'u' => {
                    // \uXXXX —— 解析 4 位十六进制为 Unicode 字符
                    if i + 5 < bytes.len() {
                        let hex = std::str::from_utf8(&bytes[i + 2..i + 6]).unwrap_or("");
                        if let Ok(cp) = u32::from_str_radix(hex, 16) {
                            if let Some(c) = char::from_u32(cp) {
                                let mut buf = [0u8; 4];
                                let s = c.encode_utf8(&mut buf);
                                result.extend_from_slice(s.as_bytes());
                                i += 6;
                                continue;
                            }
                        }
                    }
                    result.push(b'\\');
                    i += 1;
                    continue;
                }
                _ => {
                    // 未知转义：原样保留反斜杠与后续字符
                    result.push(b'\\');
                    result.push(bytes[i + 1]);
                    i += 2;
                    continue;
                }
            }
        }
        if bytes[i] == b'"' {
            break;
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(result).map_err(|e| format!("内容不是有效 UTF-8: {}", e))
}

/// 从错误响应中提取 error.message。
fn extract_error(text: &str) -> Option<String> {
    let marker = "\"message\":\"";
    let start = match text.find(marker) {
        Some(i) => i + marker.len(),
        None => return None,
    };
    let bytes: Vec<char> = text[start..].chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == '"' {
            break;
        }
        i += 1;
    }
    Some(text[start..start + i].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_token::ConceptId;

    #[test]
    fn context_window_build_messages() {
        let mut w = ContextWindow::new("你是 Hib OS 助手");
        w.push_user("你好");
        w.push_assistant("你好！有什么我可以帮忙的？");
        w.push_user("深圳天气怎么样？");
        let msgs = w.build_messages();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].0, "system");
        assert_eq!(msgs[0].1, "你是 Hib OS 助手");
        assert_eq!(msgs[1].0, "user");
        assert_eq!(msgs[1].1, "你好");
    }

    #[test]
    fn context_window_compress() {
        let mut w = ContextWindow::new("系统提示");
        w = w.with_limits(2, 100);
        for i in 0..10 {
            w.push_user(&format!("用户消息 {}", i));
            w.push_assistant(&format!("助手回复 {}", i));
        }
        w.compress();
        assert!(w.turns.len() <= w.max_turns * 2 + 1);
        assert!(w.summary.is_some());
    }

    #[test]
    fn extract_actions_parses() {
        let reply = "你好！我已经处理了请求。\nACTION: gpu://vram alloc 64\nACTION: net://eth0 ping 10.0.0.1";
        let mut consciousness = AIConsciousness::new();
        let loop_ = ThoughtLoop::new("test");
        loop_.extract_actions(reply, &mut consciousness);
        assert!(consciousness.exists(ConceptId::from_name("gpu://vram")));
        assert!(consciousness.exists(ConceptId::from_name("net://eth0")));
    }

    #[test]
    fn extract_content_works() {
        let json = r#"{"id":"chatcmpl-xyz","object":"chat.completion","created":1700000000,"model":"deepseek-chat","choices":[{"index":0,"message":{"role":"assistant","content":"你好，Hib OS"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":8,"total_tokens":18}}"#;
        let content = extract_content(json).unwrap();
        assert_eq!(content, "你好，Hib OS");
    }

    #[test]
    fn extract_content_with_escapes() {
        // 普通字符串里 \" 与 \n 会被正确还原为 " 与真正的换行
        let json = r#"{"choices":[{"message":{"content":"他说\"你好\"，换行\n测试"}}]}"#;
        let content = extract_content(json).unwrap();
        assert_eq!(content, "他说\"你好\"，换行\n测试");
    }

    #[test]
    fn extract_content_restores_newline() {
        // 关键：\n 必须变成真正的换行，否则 ACTION: 指令无法被按行识别
        let json = r#"{"choices":[{"message":{"content":"好的\n\nACTION: sense net://eth0"}}]}"#;
        let content = extract_content(json).unwrap();
        assert_eq!(content, "好的\n\nACTION: sense net://eth0");
    }

    #[test]
    fn extract_error_works() {
        let json = r#"{"error":{"message":"Incorrect API key","type":"invalid_request_error","param":null,"code":401}}"#;
        let err = extract_error(json).unwrap();
        assert_eq!(err, "Incorrect API key");
    }

    #[test]
    fn extract_content_missing_field() {
        assert!(extract_content(r#"{"id":"xyz"}"#).is_err());
    }
}
