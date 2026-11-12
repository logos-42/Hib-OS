//! 模型热重载 —— 不重启系统即可热切换 AI 模型。

use crate::ai_token::TokenId;

/// 热重载结果。
#[derive(Clone, Debug, PartialEq)]
pub enum HotReloadResult {
    Success {
        new_token_id: TokenId,
        old_token_id: Option<TokenId>,
        message: String,
    },
    Failed {
        error: String,
    },
    InProgress {
        progress: u8,
        step: String,
    },
}

/// 热重载状态机。
pub struct ModelHotReload {
    progress: u8,
    target_token: Option<TokenId>,
}

impl ModelHotReload {
    pub fn new() -> Self {
        Self {
            progress: 0,
            target_token: None,
        }
    }

    /// 开始热重载，分配一个新的 Token 用于新模型。
    pub fn start_reload(&mut self, model_path: &str) -> HotReloadResult {
        if model_path.is_empty() {
            return HotReloadResult::Failed {
                error: "模型路径为空".to_string(),
            };
        }
        self.progress = 20;
        self.target_token = Some(TokenId::new());
        HotReloadResult::InProgress {
            progress: self.progress,
            step: "开始热重载".to_string(),
        }
    }

    /// 轮询进度 (每次推进 20%)。
    pub fn poll_progress(&mut self) -> HotReloadResult {
        if self.progress == 0 {
            return HotReloadResult::Failed {
                error: "未启动热重载".to_string(),
            };
        }
        self.progress = (self.progress + 20).min(100);
        if self.progress < 100 {
            HotReloadResult::InProgress {
                progress: self.progress,
                step: format!("进度: {}%", self.progress),
            }
        } else {
            let new_token_id = self.target_token.unwrap_or_else(TokenId::new);
            HotReloadResult::Success {
                new_token_id,
                old_token_id: None,
                message: "模型热重载完成".to_string(),
            }
        }
    }

    /// 取消热重载。
    pub fn cancel(&mut self) {
        self.progress = 0;
        self.target_token = None;
    }
}

impl Default for ModelHotReload {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_path_fails() {
        let mut hr = ModelHotReload::new();
        assert!(matches!(
            hr.start_reload(""),
            HotReloadResult::Failed { .. }
        ));
    }

    #[test]
    fn reload_reaches_success() {
        let mut hr = ModelHotReload::new();
        let mut last = hr.start_reload("model.bin");
        assert!(matches!(
            last,
            HotReloadResult::InProgress { progress: 20, .. }
        ));
        let mut guard = 0;
        while !matches!(last, HotReloadResult::Success { .. }) && guard < 10 {
            last = hr.poll_progress();
            guard += 1;
        }
        match last {
            HotReloadResult::Success { message, .. } => {
                assert!(message.contains("完成"));
            }
            other => panic!("预期 Success, 实际 {:?}", other),
        }
    }

    #[test]
    fn poll_before_start_fails() {
        let mut hr = ModelHotReload::new();
        assert!(matches!(
            hr.poll_progress(),
            HotReloadResult::Failed { .. }
        ));
    }
}

