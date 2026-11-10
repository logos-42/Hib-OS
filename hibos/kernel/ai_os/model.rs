//! 模型子系统 —— AI 原生 OS 的「可执行镜像」。
//!
//! 对应《现代操作系统》中的**可执行文件与进程映像**：模型文件之于智能体，
//! 如同 ELF 之于进程。模型注册表等价于内核的二进制格式注册表。
//!
//! 设计要点：
//! * `ModelFormat`：RawTensor / GGUF / ONNX / SafeTensors；
//! * `ModelInstance`：一次加载产生的可执行实例（占用 VRAM、上下文窗口、KV 缓存）；
//! * `ModelRegistry`：加载、卸载、按名查找、实例化（类似 fork/exec 的 exec 部分）；
//! * 量化信息与内存账本，便于内存管理器统一核算；
//! * 纯内存原型，无外部依赖。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 模型格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    RawTensor,
    Gguf,
    Onnx,
    SafeTensors,
}

impl ModelFormat {
    pub fn name(self) -> &'static str {
        match self {
            ModelFormat::RawTensor => "raw",
            ModelFormat::Gguf => "gguf",
            ModelFormat::Onnx => "onnx",
            ModelFormat::SafeTensors => "safetensors",
        }
    }
}

/// 模型元数据。
#[derive(Clone, Debug)]
pub struct ModelMeta {
    pub path: String,
    pub format: ModelFormat,
    pub params: u64,
    pub ctx_window: u64,
    pub dtype: String,
    pub vram_bytes: u64,
}

/// 模型实例 id。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(pub u64);

/// 一个模型实例：可被智能体认领执行。
#[derive(Clone, Debug)]
pub struct ModelInstance {
    pub id: InstanceId,
    pub model: String,
    pub loaded_at: u64,
    /// 已分配的 KV 缓存 token 数。
    pub kv_used: u64,
    /// 当前持有者（智能体 id），None 表示空闲。
    pub owner: Option<u64>,
}

/// 模型注册表错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    AlreadyLoaded { path: String },
    NotFound { name: String },
    NoSuchInstance { id: InstanceId },
    Busy { id: InstanceId },
    VramExhausted,
}

/// 模型注册表。
pub struct ModelRegistry {
    next_instance: u64,
    models: BTreeMap<String, ModelMeta>,
    instances: BTreeMap<InstanceId, ModelInstance>,
    /// VRAM 预算（字节）。
    pub vram_budget: u64,
    pub vram_used: u64,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            next_instance: 1,
            models: BTreeMap::new(),
            instances: BTreeMap::new(),
            vram_budget: 8 << 30,
            vram_used: 0,
        }
    }

    /// 加载模型元数据（不实例化）。重复加载同一路径返回错误。
    pub fn load(
        &mut self,
        path: &str,
        format: ModelFormat,
        params: u64,
        ctx_window: u64,
        dtype: &str,
        vram_bytes: u64,
    ) -> Result<String, ModelError> {
        if self.models.contains_key(path) {
            return Err(ModelError::AlreadyLoaded {
                path: path.to_string(),
            });
        }
        if self.vram_used + vram_bytes > self.vram_budget {
            return Err(ModelError::VramExhausted);
        }
        self.models.insert(
            path.to_string(),
            ModelMeta {
                path: path.to_string(),
                format,
                params,
                ctx_window,
                dtype: dtype.to_string(),
                vram_bytes,
            },
        );
        Ok(path.to_string())
    }

    /// 卸载模型（有实例仍持有则拒绝）。
    pub fn unload(&mut self, name: &str) -> Result<(), ModelError> {
        if self.instances.values().any(|i| i.model == name) {
            return Err(ModelError::Busy {
                id: InstanceId(0),
            });
        }
        if self.models.remove(name).is_none() {
            return Err(ModelError::NotFound {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    /// 实例化：为模型创建可执行实例。
    pub fn instantiate(
        &mut self,
        model: &str,
        kv_tokens: u64,
        owner: Option<u64>,
    ) -> Result<InstanceId, ModelError> {
        let meta = self
            .models
            .get(model)
            .ok_or_else(|| ModelError::NotFound {
                name: model.to_string(),
            })?
            .clone();
        if self.vram_used + meta.vram_bytes > self.vram_budget {
            return Err(ModelError::VramExhausted);
        }
        let id = InstanceId(self.next_instance);
        self.next_instance += 1;
        self.vram_used += meta.vram_bytes;
        self.instances.insert(
            id,
            ModelInstance {
                id,
                model: model.to_string(),
                loaded_at: 0,
                kv_used: kv_tokens.min(meta.ctx_window),
                owner,
            },
        );
        Ok(id)
    }

    /// 释放实例，回收 VRAM。
    pub fn destroy(&mut self, id: InstanceId) -> Result<(), ModelError> {
        let inst = self
            .instances
            .remove(&id)
            .ok_or(ModelError::NoSuchInstance { id })?;
        if let Some(meta) = self.models.get(&inst.model) {
            self.vram_used = self.vram_used.saturating_sub(meta.vram_bytes);
        }
        Ok(())
    }

    /// 按名查找元数据。
    pub fn find(&self, name: &str) -> Option<&ModelMeta> {
        self.models.get(name)
    }

    /// 绑定实例到所有者。
    pub fn bind(&mut self, id: InstanceId, owner: u64) -> Result<(), ModelError> {
        let inst = self
            .instances
            .get_mut(&id)
            .ok_or(ModelError::NoSuchInstance { id })?;
        if let Some(cur) = inst.owner {
            if cur != owner {
                return Err(ModelError::Busy { id });
            }
        }
        inst.owner = Some(owner);
        Ok(())
    }

    pub fn models(&self) -> &BTreeMap<String, ModelMeta> {
        &self.models
    }

    pub fn instances(&self) -> &BTreeMap<InstanceId, ModelInstance> {
        &self.instances
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static REGISTRY: OnceLock<Mutex<ModelRegistry>> = OnceLock::new();

/// 全局模型注册表。
pub fn model_registry() -> &'static Mutex<ModelRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(ModelRegistry::new()))
}

/// 初始化：预装文本/多模态模型描述。
pub fn init() {
    let mut reg = model_registry().lock().unwrap();
    if !reg.models().is_empty() {
        return;
    }
    let _ = reg.load(
        "memory://hibos-core-7b",
        ModelFormat::RawTensor,
        7_000_000_000,
        32_000,
        "fp16",
        (7u64 << 30) / 2,
    );
    let _ = reg.load(
        "memory://hibos-vision",
        ModelFormat::Gguf,
        2_000_000_000,
        8_192,
        "q4",
        (2u64 << 30) / 4,
    );
}

/// 状态摘要，供 `sense model://status` 使用。
pub fn status() -> String {
    let reg = model_registry().lock().unwrap();
    format!(
        "{} model(s), {} instance(s), vram={}/{}MiB",
        reg.models().len(),
        reg.instances().len(),
        reg.vram_used >> 20,
        reg.vram_budget >> 20
    )
}

impl ModelRegistry {
    /// 实例列表摘要，供 `sense model://instances` 使用。
    pub fn instances_status(&self) -> String {
        let mut out = String::from("模型实例:\n");
        for (id, inst) in &self.instances {
            out.push_str(&format!(
                "  inst={} model={} kv={} owner={:?}\n",
                id.0, inst.model, inst.kv_used, inst.owner
            ));
        }
        if self.instances.is_empty() {
            out.push_str("  （无）\n");
        }
        out
    }
}

/// `sense model://loader` —— 模型加载器视角的摘要。
pub fn model_loader_sense() -> String {
    let reg = model_registry().lock().unwrap();
    let mut out = String::from("模型加载器:\n");
    for (path, meta) in reg.models() {
        out.push_str(&format!(
            "  {} fmt={} params={} ctx={} dtype={} vram={}KiB\n",
            path,
            meta.format.name(),
            meta.params,
            meta.ctx_window,
            meta.dtype,
            meta.vram_bytes >> 10
        ));
    }
    if reg.models().is_empty() {
        out.push_str("  （未加载任何模型）\n");
    }
    out
}

/// `act model://*` 的后端动作：
/// * `load <path> <format> <params> <ctx> <dtype> <vram>` 加载元数据
/// * `instantiate <path> [kv_tokens]` 实例化
/// * `destroy <instance>` 释放
/// * `bind <instance> <owner>` 绑定所有者
/// * `unload <path>` 卸载
/// * `status` 摘要
pub fn model_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let mut reg = model_registry().lock().unwrap();
    match it.next() {
        Some("load") => {
            let path = it.next().ok_or("用法: model://loader load <path> <format> <params> <ctx> <dtype> <vram>")?;
            let fmt = match it.next().unwrap_or("raw").to_ascii_lowercase().as_str() {
                "gguf" => ModelFormat::Gguf,
                "onnx" => ModelFormat::Onnx,
                "safetensors" | "st" => ModelFormat::SafeTensors,
                _ => ModelFormat::RawTensor,
            };
            let params = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let ctx = it.next().and_then(|s| s.parse().ok()).unwrap_or(4096);
            let dtype = it.next().unwrap_or("fp16");
            let vram = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let name = reg
                .load(path, fmt, params, ctx, dtype, vram)
                .map_err(|e| format!("加载失败: {:?}", e))?;
            Ok(format!("已加载模型 {}", name))
        }
        Some("instantiate") => {
            let path = it.next().ok_or("用法: model://instances instantiate <path> [kv_tokens]")?;
            let kv = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let id = reg
                .instantiate(path, kv, None)
                .map_err(|e| format!("实例化失败: {:?}", e))?;
            Ok(format!("已创建实例 inst={}", id.0))
        }
        Some("destroy") => {
            let id = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .map(InstanceId)
                .ok_or("用法: model://instances destroy <instance>")?;
            reg.destroy(id).map_err(|e| format!("释放失败: {:?}", e))?;
            Ok(format!("已释放实例 inst={}", id.0))
        }
        Some("bind") => {
            let id = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .map(InstanceId)
                .ok_or("用法: model://instances bind <instance> <owner>")?;
            let owner = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: model://instances bind <instance> <owner>")?;
            reg.bind(id, owner)
                .map_err(|e| format!("绑定失败: {:?}", e))?;
            Ok(format!("实例 inst={} 已绑定 owner={}", id.0, owner))
        }
        Some("unload") => {
            let path = it.next().ok_or("用法: model://loader unload <path>")?;
            reg.unload(path).map_err(|e| format!("卸载失败: {:?}", e))?;
            Ok(format!("已卸载模型 {}", path))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 model 动作: {}", other)),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_and_instantiate() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::RawTensor, 1_000, 1024, "fp16", 1024)
            .unwrap();
        let id = reg.instantiate("/m.bin", 128, None).unwrap();
        assert_eq!(reg.instances().len(), 1);
        assert_eq!(reg.vram_used, 1024);
        reg.destroy(id).unwrap();
        assert_eq!(reg.vram_used, 0);
    }

    #[test]
    fn duplicate_load_rejected() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::Gguf, 1, 1, "q4", 1)
            .unwrap();
        assert_eq!(
            reg.load("/m.bin", ModelFormat::Gguf, 1, 1, "q4", 1)
                .unwrap_err(),
            ModelError::AlreadyLoaded {
                path: "/m.bin".to_string()
            }
        );
    }

    #[test]
    fn unload_busy_rejected() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::Onnx, 1, 1, "fp32", 1)
            .unwrap();
        reg.instantiate("/m.bin", 0, Some(9)).unwrap();
        assert!(matches!(reg.unload("/m.bin"), Err(ModelError::Busy { .. })));
    }

    #[test]
    fn bind_conflict() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::Onnx, 1, 1, "fp32", 1)
            .unwrap();
        let id = reg.instantiate("/m.bin", 0, Some(1)).unwrap();
        assert!(reg.bind(id, 1).is_ok());
        assert!(matches!(reg.bind(id, 2), Err(ModelError::Busy { .. })));
    }

    #[test]
    fn global_init_and_status() {
        init();
        init();
        assert!(status().contains("model(s)"));
    }
}
