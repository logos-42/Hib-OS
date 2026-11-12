//! 模型加载器 —— GGUF/ONNX/RawTensor 模型加载、张量视图、量化与元数据。
//!
//! `ModelLoader` 提供 `load` / `unload` / `get_tensor` / `get_metadata` /
//! `quantize` / `dequantize`。`TensorView` 描述形状、数据类型、数据指针与步长；
//! `ModelMetadata` 描述名称、版本、输入/输出形状、参数量与显存占用。
//! 量化/反量化为简化版（INT8/FP16）；内存映射加载与懒加载以标志位表示。
//! 与 `kernel/ai_token.rs` 的 `GpuCapabilities` / `VramPartitionId` 互补。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 模型格式
// ============================================================================

/// 模型文件格式（简化版）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    GGUF,
    ONNX,
    RawTensor,
}

impl ModelFormat {
    pub fn from_extension(path: &str) -> Option<Self> {
        if path.ends_with(".gguf") {
            Some(ModelFormat::GGUF)
        } else if path.ends_with(".onnx") {
            Some(ModelFormat::ONNX)
        } else if path.ends_with(".bin") || path.ends_with(".raw") || path.ends_with(".tensor") {
            Some(ModelFormat::RawTensor)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ModelFormat::GGUF => "GGUF",
            ModelFormat::ONNX => "ONNX",
            ModelFormat::RawTensor => "RawTensor",
        }
    }
}

// ============================================================================
// 张量数据类型
// ============================================================================

/// 张量数据类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TensorDtype {
    F32,
    F16,
    I8,
}

impl TensorDtype {
    pub fn size_of(self) -> usize {
        match self {
            TensorDtype::F32 => 4,
            TensorDtype::F16 => 2,
            TensorDtype::I8 => 1,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TensorDtype::F32 => "f32",
            TensorDtype::F16 => "f16",
            TensorDtype::I8 => "i8",
        }
    }
}

// ============================================================================
// 张量视图
// ============================================================================

/// 张量视图：形状、数据类型、数据、步长。
#[derive(Clone, Debug)]
pub struct TensorView {
    pub shape: Vec<usize>,
    pub dtype: TensorDtype,
    pub data: Vec<u8>,
    pub strides: Vec<usize>,
    pub offset: usize,
    pub readonly: bool,
}

impl TensorView {
    pub fn new(shape: Vec<usize>, dtype: TensorDtype, data: Vec<u8>) -> Self {
        let strides = compute_strides(&shape);
        Self {
            shape,
            dtype,
            data,
            strides,
            offset: 0,
            readonly: false,
        }
    }

    /// 元素总数。
    pub fn len(&self) -> usize {
        self.shape.iter().product()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 数据字节数（预期）。
    pub fn expected_byte_len(&self) -> usize {
        self.len() * self.dtype.size_of()
    }

    /// 按多维索引读取 f32 值（仅 f32 支持）。
    pub fn get_f32(&self, indices: &[usize]) -> Option<f32> {
        if self.dtype != TensorDtype::F32 {
            return None;
        }
        let flat = self.flat_index(indices)?;
        let byte_offset = self.offset + flat * 4;
        if byte_offset + 4 > self.data.len() {
            return None;
        }
        let bytes = [self.data[byte_offset], self.data[byte_offset + 1], self.data[byte_offset + 2], self.data[byte_offset + 3]];
        Some(f32::from_le_bytes(bytes))
    }

    /// 按多维索引写入 f32 值。
    pub fn set_f32(&mut self, indices: &[usize], value: f32) -> Option<()> {
        if self.readonly {
            return None;
        }
        if self.dtype != TensorDtype::F32 {
            return None;
        }
        let flat = self.flat_index(indices)?;
        let bytes = value.to_le_bytes();
        let byte_offset = self.offset + flat * 4;
        if byte_offset + 4 > self.data.len() {
            return None;
        }
        self.data[byte_offset..byte_offset + 4].copy_from_slice(&bytes);
        Some(())
    }

    /// 按多维索引读取 i8 值。
    pub fn get_i8(&self, indices: &[usize]) -> Option<i8> {
        let flat = self.flat_index(indices)?;
        let byte_offset = self.offset + flat;
        if byte_offset >= self.data.len() {
            return None;
        }
        Some(self.data[byte_offset] as i8)
    }

    /// 扁平索引。
    fn flat_index(&self, indices: &[usize]) -> Option<usize> {
        if indices.len() != self.strides.len() {
            return None;
        }
        let mut flat = 0usize;
        for (idx, stride) in indices.iter().zip(self.strides.iter()) {
            if *idx >= self.shape[*stride.min(self.shape.len().saturating_sub(1))] {
                return None;
            }
            // 简化：假设 strides 与 shape 对齐。
            flat += idx * stride;
        }
        Some(flat)
    }

    /// 返回数据切片引用。
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// 克隆为可变副本。
    pub fn to_owned_mut(&self) -> Self {
        Self {
            shape: self.shape.clone(),
            dtype: self.dtype,
            data: self.data.clone(),
            strides: self.strides.clone(),
            offset: self.offset,
            readonly: false,
        }
    }
}

/// 计算行优先步长。
fn compute_strides(shape: &[usize]) -> Vec<usize> {
    let mut strides = vec![1usize; shape.len()];
    let mut acc = 1usize;
    for i in (0..shape.len()).rev() {
        strides[i] = acc;
        acc *= shape[i];
    }
    strides
}

// ============================================================================
// 模型元数据
// ============================================================================

/// 模型元数据。
#[derive(Clone, Debug, Default)]
pub struct ModelMetadata {
    pub name: String,
    pub version: String,
    pub author: String,
    pub input_shape: Vec<usize>,
    pub output_shape: Vec<usize>,
    pub parameter_count: u64,
    pub vram_usage: u64, // 字节
    pub format: ModelFormat,
    pub loaded: bool,
}

impl ModelMetadata {
    pub fn new(name: &str, format: ModelFormat) -> Self {
        Self {
            name: name.to_string(),
            format,
            ..Default::default()
        }
    }

    /// 参数量人类可读。
    pub fn param_count_str(&self) -> String {
        let n = self.parameter_count as f64;
        if n >= 1e9 {
            format!("{:.2}B", n / 1e9)
        } else if n >= 1e6 {
            format!("{:.2}M", n / 1e6)
        } else if n >= 1e3 {
            format!("{:.2}K", n / 1e3)
        } else {
            format!("{}", n)
        }
    }

    /// 显存占用人类可读。
    pub fn vram_str(&self) -> String {
        let n = self.vram_usage as f64;
        if n >= 1e9 {
            format!("{:.2}GiB", n / (1024.0 * 1024.0 * 1024.0))
        } else if n >= 1e6 {
            format!("{:.2}MiB", n / (1024.0 * 1024.0))
        } else {
            format!("{}B", n as u64)
        }
    }
}

// ============================================================================
// 加载的模型句柄
// ============================================================================

/// 模型唯一标识。
pub type ModelId = u64;

/// 已加载模型。
#[derive(Clone, Debug)]
pub struct LoadedModel {
    pub id: ModelId,
    pub path: String,
    pub format: ModelFormat,
    pub metadata: ModelMetadata,
    /// 张量名 -> TensorView。
    tensors: BTreeMap<String, TensorView>,
    /// 是否懒加载（张量尚未读入内存）。
    pub lazy: bool,
    /// 是否内存映射。
    pub mmap: bool,
}

impl LoadedModel {
    pub fn new(id: ModelId, path: &str, format: ModelFormat, lazy: bool, mmap: bool) -> Self {
        Self {
            id,
            path: path.to_string(),
            format,
            metadata: ModelMetadata::new(path, format),
            tensors: BTreeMap::new(),
            lazy,
            mmap,
        }
    }

    /// 注册张量。
    pub fn add_tensor(&mut self, name: &str, tensor: TensorView) {
        self.tensors.insert(name.to_string(), tensor);
    }

    /// 获取张量。
    pub fn tensor(&self, name: &str) -> Option<&TensorView> {
        self.tensors.get(name)
    }

    /// 张量数量。
    pub fn tensor_count(&self) -> usize {
        self.tensors.len()
    }

    /// 张量名列表。
    pub fn tensor_names(&self) -> Vec<String> {
        self.tensors.keys().cloned().collect()
    }
}

// ============================================================================
// 模型加载器
// ============================================================================

/// 模型加载器：模型注册表、加载/卸载、张量访问、量化/反量化。
pub struct ModelLoader {
    /// 已加载模型：id -> LoadedModel。
    models: BTreeMap<ModelId, LoadedModel>,
    /// 路径 -> id。
    path_index: BTreeMap<String, ModelId>,
    /// 下一个模型 id。
    next_id: ModelId,
    /// 累计加载次数。
    load_count: u64,
    /// 累计量化次数。
    quantize_count: u64,
}

impl Default for ModelLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelLoader {
    pub fn new() -> Self {
        Self {
            models: BTreeMap::new(),
            path_index: BTreeMap::new(),
            next_id: 1,
            load_count: 0,
            quantize_count: 0,
        }
    }

    /// 加载模型（简化：不真正读文件，生成元数据与示例张量）。
    pub fn load(&mut self, path: &str, format: Option<ModelFormat>) -> Result<ModelId, String> {
        if self.path_index.contains_key(path) {
            return Ok(*self.path_index.get(path).unwrap());
        }
        let fmt = format.unwrap_or_else(|| ModelFormat::from_extension(path).unwrap_or(ModelFormat::RawTensor));
        let id = self.next_id;
        self.next_id += 1;
        let mut model = LoadedModel::new(id, path, fmt, false, true);
        // 生成示例元数据。
        model.metadata.name = path.split('/').last().unwrap_or(path).to_string();
        model.metadata.version = "1.0".to_string();
        model.metadata.author = "hibos".to_string();
        model.metadata.input_shape = vec![1, 3, 224, 224];
        model.metadata.output_shape = vec![1, 1000];
        model.metadata.parameter_count = 25_000_000;
        model.metadata.vram_usage = 100 * 1024 * 1024;
        model.metadata.loaded = true;
        // 添加示例张量。
        let w: Vec<u8> = vec![0; 3 * 224 * 224 * 4];
        model.add_tensor("input", TensorView::new(vec![1, 3, 224, 224], TensorDtype::F32, w.clone()));
        let out: Vec<u8> = vec![0; 1000 * 4];
        model.add_tensor("output", TensorView::new(vec![1, 1000], TensorDtype::F32, out));
        self.path_index.insert(path.to_string(), id);
        self.models.insert(id, model);
        self.load_count += 1;
        Ok(id)
    }

    /// 卸载模型。
    pub fn unload(&mut self, model_id: ModelId) -> bool {
        if let Some(model) = self.models.remove(&model_id) {
            self.path_index.remove(&model.path);
            true
        } else {
            false
        }
    }

    /// 获取张量视图。
    pub fn get_tensor(&self, model_id: ModelId, name: &str) -> Option<&TensorView> {
        self.models.get(&model_id)?.tensor(name)
    }

    /// 获取元数据。
    pub fn get_metadata(&self, model_id: ModelId) -> Option<&ModelMetadata> {
        self.models.get(&model_id).map(|m| &m.metadata)
    }

    /// 量化模型（FP32 -> INT8 简化版），返回新模型 id。
    pub fn quantize(&mut self, model_id: ModelId, bits: u32) -> Result<ModelId, String> {
        let model = self.models.get(&model_id).ok_or("模型不存在")?;
        if bits != 8 {
            return Err("仅支持量化到 8 位".to_string());
        }
        let new_id = self.next_id;
        self.next_id += 1;
        let mut new_model = LoadedModel::new(new_id, &model.path, model.format, false, true);
        new_model.metadata = model.metadata.clone();
        new_model.metadata.vram_usage = model.metadata.vram_usage / 4; // INT8 约 1/4。
        // 量化张量：f32 -> i8（简化：缩放后截断）。
        for (name, tensor) in &model.tensors {
            if tensor.dtype == TensorDtype::F32 {
                let mut quant_data = Vec::new();
                for chunk in tensor.data.chunks(4) {
                    if chunk.len() == 4 {
                        let f = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        let q = (f.clamp(-1.0, 1.0) * 127.0) as i8;
                        quant_data.push(q as u8);
                    }
                }
                new_model.add_tensor(name, TensorView::new(tensor.shape.clone(), TensorDtype::I8, quant_data));
            } else {
                new_model.add_tensor(name, tensor.clone());
            }
        }
        self.path_index.insert(format!("{}:quant{}", model.path, bits), new_id);
        self.models.insert(new_id, new_model);
        self.quantize_count += 1;
        Ok(new_id)
    }

    /// 反量化（INT8 -> FP32 简化版），返回新模型 id。
    pub fn dequantize(&mut self, model_id: ModelId) -> Result<ModelId, String> {
        let model = self.models.get(&model_id).ok_or("模型不存在")?;
        let new_id = self.next_id;
        self.next_id += 1;
        let mut new_model = LoadedModel::new(new_id, &model.path, model.format, false, true);
        new_model.metadata = model.metadata.clone();
        new_model.metadata.vram_usage = model.metadata.vram_usage * 4;
        for (name, tensor) in &model.tensors {
            if tensor.dtype == TensorDtype::I8 {
                let mut dequant_data = Vec::new();
                for &b in &tensor.data {
                    let q = b as i8;
                    let f = (q as f32) / 127.0;
                    dequant_data.extend_from_slice(&f.to_le_bytes());
                }
                new_model.add_tensor(name, TensorView::new(tensor.shape.clone(), TensorDtype::F32, dequant_data));
            } else {
                new_model.add_tensor(name, tensor.clone());
            }
        }
        self.path_index.insert(format!("{}:dequant", model.path), new_id);
        self.models.insert(new_id, new_model);
        Ok(new_id)
    }

    /// 返回模型总数。
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// 返回已加载模型 id 列表。
    pub fn model_ids(&self) -> Vec<ModelId> {
        self.models.keys().copied().collect()
    }

    /// 是否已加载路径。
    pub fn is_loaded(&self, path: &str) -> bool {
        self.path_index.contains_key(path)
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

// ============================================================================
// 全局 ModelLoader 单例（供 kernel/sense.rs 调用）
// ============================================================================

static MODEL_LOADER: OnceLock<Mutex<ModelLoader>> = OnceLock::new();

/// 获取全局模型加载器。
pub fn model_loader() -> &'static Mutex<ModelLoader> {
    MODEL_LOADER.get_or_init(|| Mutex::new(ModelLoader::new()))
}

/// `sense ai://model`：返回模型加载器状态摘要。
pub fn model_loader_sense() -> String {
    let l = model_loader().lock().unwrap();
    format!(
        "models={} load_count={} quantize_count={}",
        l.model_count(),
        l.load_count,
        l.quantize_count
    )
}

/// `act ai://model <op> ...`：对模型加载器执行行动。
pub fn model_loader_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let l = model_loader();
    let mut l = l.lock().unwrap();
    match it.next().unwrap_or("") {
        "load" => {
            let path = it.next().ok_or("缺少路径")?;
            let fmt = match it.next().unwrap_or("auto") {
                "gguf" => Some(ModelFormat::GGUF),
                "onnx" => Some(ModelFormat::ONNX),
                "raw" => Some(ModelFormat::RawTensor),
                _ => None,
            };
            let id = l.load(path, fmt)?;
            Ok(format!("已加载模型 #{} ({})", id, path))
        }
        "unload" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            if l.unload(id) {
                Ok(format!("已卸载模型 #{}", id))
            } else {
                Err(format!("模型 #{} 不存在", id))
            }
        }
        "get_tensor" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let name = it.next().ok_or("缺少张量名")?;
            match l.get_tensor(id, name) {
                Some(t) => Ok(format!(
                    "张量 {}: shape={:?} dtype={} len={}",
                    name,
                    t.shape,
                    t.dtype.name(),
                    t.len()
                )),
                None => Err(format!("张量 {} 不存在", name)),
            }
        }
        "metadata" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            match l.get_metadata(id) {
                Some(meta) => Ok(format!(
                    "模型 {} v{} 参数={} 显存={} 输入={:?} 输出={:?}",
                    meta.name, meta.version, meta.param_count_str(), meta.vram_str(), meta.input_shape, meta.output_shape
                )),
                None => Err(format!("模型 #{} 不存在", id)),
            }
        }
        "quantize" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let bits: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(8);
            let new_id = l.quantize(id, bits)?;
            Ok(format!("已量化模型 #{} -> #{} ({}位)", id, new_id, bits))
        }
        "dequantize" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let new_id = l.dequantize(id)?;
            Ok(format!("已反量化模型 #{} -> #{}", id, new_id))
        }
        "list" => Ok(model_loader_sense()),
        _ => Err(
            "用法: model load|unload|get_tensor|metadata|quantize|dequantize|list".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_format_from_extension() {
        assert_eq!(ModelFormat::from_extension("model.gguf"), Some(ModelFormat::GGUF));
        assert_eq!(ModelFormat::from_extension("model.onnx"), Some(ModelFormat::ONNX));
        assert_eq!(ModelFormat::from_extension("model.bin"), Some(ModelFormat::RawTensor));
        assert_eq!(ModelFormat::from_extension("model.txt"), None);
    }

    #[test]
    fn tensor_dtype_size() {
        assert_eq!(TensorDtype::F32.size_of(), 4);
        assert_eq!(TensorDtype::F16.size_of(), 2);
        assert_eq!(TensorDtype::I8.size_of(), 1);
    }

    #[test]
    fn tensor_view_new_and_len() {
        let data = vec![0u8; 16];
        let t = TensorView::new(vec![2, 2], TensorDtype::F32, data);
        assert_eq!(t.len(), 4);
        assert_eq!(t.expected_byte_len(), 16);
        assert_eq!(t.strides.len(), 2);
    }

    #[test]
    fn tensor_get_set_f32() {
        let mut data = vec![0u8; 16];
        // 放入 1.0f32 到第一个元素。
        data[0..4].copy_from_slice(&1.0f32.to_le_bytes());
        let t = TensorView::new(vec![2, 2], TensorDtype::F32, data);
        assert_eq!(t.get_f32(&[0, 0]), Some(1.0));
        assert_eq!(t.get_f32(&[0, 1]), Some(0.0));
        assert_eq!(t.get_f32(&[5, 5]), None); // 越界。
    }

    #[test]
    fn compute_strides() {
        let strides = compute_strides(&[2, 3, 4]);
        assert_eq!(strides, vec![12, 4, 1]);
    }

    #[test]
    fn model_metadata_strs() {
        let mut meta = ModelMetadata::new("test", ModelFormat::GGUF);
        meta.parameter_count = 1_500_000_000;
        assert!(meta.param_count_str().contains("B"));
        meta.parameter_count = 2_500_000;
        assert!(meta.param_count_str().contains("M"));
        meta.vram_usage = 2 * 1024 * 1024 * 1024;
        assert!(meta.vram_str().contains("GiB"));
    }

    #[test]
    fn model_loader_load_unload() {
        let mut l = ModelLoader::new();
        let id = l.load("/tmp/model.gguf", Some(ModelFormat::GGUF)).unwrap();
        assert!(id > 0);
        assert!(l.is_loaded("/tmp/model.gguf"));
        assert_eq!(l.model_count(), 1);
        let meta = l.get_metadata(id).unwrap();
        assert_eq!(meta.name, "model.gguf");
        assert!(l.unload(id));
        assert_eq!(l.model_count(), 0);
        assert!(!l.is_loaded("/tmp/model.gguf"));
    }

    #[test]
    fn get_tensor() {
        let mut l = ModelLoader::new();
        let id = l.load("/tmp/model.onnx", Some(ModelFormat::ONNX)).unwrap();
        let t = l.get_tensor(id, "input").unwrap();
        assert_eq!(t.dtype, TensorDtype::F32);
        assert_eq!(t.shape, vec![1, 3, 224, 224]);
        assert!(l.get_tensor(id, "nonexistent").is_none());
    }

    #[test]
    fn quantize_dequantize() {
        let mut l = ModelLoader::new();
        let id = l.load("/tmp/model.bin", Some(ModelFormat::RawTensor)).unwrap();
        let qid = l.quantize(id, 8).unwrap();
        assert_ne!(qid, id);
        let qt = l.get_tensor(qid, "input").unwrap();
        assert_eq!(qt.dtype, TensorDtype::I8);
        let dqid = l.dequantize(qid).unwrap();
        let dt = l.get_tensor(dqid, "input").unwrap();
        assert_eq!(dt.dtype, TensorDtype::F32);
        assert_eq!(l.quantize_count, 1);
    }

    #[test]
    fn model_loader_sense_and_act() {
        let mut l = model_loader().lock().unwrap();
        let _ = l.load("/tmp/m.gguf", Some(ModelFormat::GGUF));
        drop(l);
        let sense = model_loader_sense();
        assert!(sense.contains("models="));
        let res = model_loader_act("load /tmp/n.onnx onnx").unwrap();
        assert!(res.contains("已加载模型"));
        let res = model_loader_act("metadata 1").unwrap();
        assert!(res.contains("模型"));
        let res = model_loader_act("list").unwrap();
        assert!(sense.contains("load_count="));
    }

    #[test]
    fn loaded_model_tensor_names() {
        let mut l = ModelLoader::new();
        let id = l.load("/tmp/m.raw", None).unwrap();
        let names = l.models.get(&id).unwrap().tensor_names();
        assert!(names.contains(&"input".to_string()));
        assert!(names.contains(&"output".to_string()));
    }
}
