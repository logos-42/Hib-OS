//! AI 子系统 —— 多模态感知、知识图谱、推理引擎、模型加载器、元认知。
//!
//! 模块组织:
//! * `multimodal` —— 图像/文本/音频感知与多模态融合
//! * `knowledge_graph` —— 三元组知识图谱与图查询
//! * `reasoning` —— 正向/逆向推理引擎
//! * `model_loader` —— GGUF/ONNX/RawTensor 模型加载与量化
//! * `metacognition` —— 元认知：自我监控/评估/修正/元学习
//!
//! 与 `kernel/ai_token.rs` 的概念图、kernel/inspiration.rs 的灵感核心互补。

pub mod multimodal;
pub mod knowledge_graph;
pub mod reasoning;
pub mod model_loader;
pub mod metacognition;
