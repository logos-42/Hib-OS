//! AI 原生 OS 基准测试 —— 手动计时版本
//!
//! 运行方式：cargo test --test ai_os_bench -- --bench
//! 或者：cargo run --example ai_os_bench

use std::time::Instant;

use hibos::kernel::ai_os::*;

// `kernel/ai_os/perception.rs` 尚未在 `kernel/ai_os/mod.rs` 中声明，
// 因此 `hibos::kernel::ai_os::perception` 不可用；这里用 `#[path]` 直接并入示例
// crate，`use ... as ai_os` 用于满足文件内部 `crate::ai_os::native::*` 的绝对路径。
use hibos::kernel::ai_os as ai_os;

#[allow(dead_code, unused_imports, unused_variables, unused_mut)]
#[path = "../kernel/ai_os/perception.rs"]
mod perception;

fn bench_model_load() {
    let start = Instant::now();
    for _ in 0..1000 {
        let mut reg = model::ModelRegistry::new();
        reg.load_with_spec(
            "/bench-model",
            model::ModelFormat::RawTensor,
            model::ModelSpec {
                params: 7_000_000_000,
                num_layers: 32,
                hidden_size: 4096,
                num_heads: 32,
                head_dim: 128,
                intermediate_size: 11008,
                vocab_size: 32000,
                max_position_embeddings: 32768,
                ..Default::default()
            },
            model::QuantFormat::Fp16,
            (7u64 << 30) / 2,
        ).unwrap();
    }
    let elapsed = start.elapsed();
    println!("model_load: {} ops in {:?} ({:.2} ns/op)", 1000, elapsed, elapsed.as_nanos() / 1000);
}

fn bench_model_instantiate() {
    let mut reg = model::ModelRegistry::new();
    reg.load_with_spec(
        "/bench-model",
        model::ModelFormat::RawTensor,
        model::ModelSpec {
            params: 1_000_000_000,
            num_layers: 12,
            hidden_size: 768,
            num_heads: 12,
            head_dim: 64,
            intermediate_size: 3072,
            vocab_size: 50000,
            max_position_embeddings: 4096,
            ..Default::default()
        },
        model::QuantFormat::Fp16,
        500 << 20,
    ).unwrap();
    let start = Instant::now();
    for _ in 0..10000 {
        reg.instantiate("/bench-model", 256, None).unwrap();
    }
    let elapsed = start.elapsed();
    println!("model_instantiate: {} ops in {:?} ({:.2} ns/op)", 10000, elapsed, elapsed.as_nanos() / 10000);
}

fn bench_consciousness_tick() {
    let mut eng = native::ConsciousnessEngine::new();
    let start = Instant::now();
    for _ in 0..100000 {
        eng.tick().unwrap();
    }
    let elapsed = start.elapsed();
    println!("consciousness_tick: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_mind_decide() {
    let mut eng = mind::MindEngine::new();
    let options: Vec<mind::DecisionOption> = (0..10).map(|i| {
        mind::DecisionOption {
            id: i as u64,
            description: format!("option_{}", i),
            expected_utility: 0.5 + (i as f32) * 0.05,
            probability: 0.8,
            cost: 0.1,
        }
    }).collect();
    let start = Instant::now();
    for _ in 0..10000 {
        eng.decide(options.clone()).unwrap();
    }
    let elapsed = start.elapsed();
    println!("mind_decide: {} ops in {:?} ({:.2} ns/op)", 10000, elapsed, elapsed.as_nanos() / 10000);
}

fn bench_attention_receive() {
    let mut eng = attention::AttentionEngine::new();
    let start = Instant::now();
    for i in 0..100000 {
        eng.receive_input(&format!("ch{}", i % 10), 0.7);
    }
    let elapsed = start.elapsed();
    println!("attention_receive: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_perception_inject() {
    let mut eng = perception::PerceptionEngine::new();
    let ch_id = eng.add_channel("ch1", perception::SensoryModality::Visual, "cam0");
    eng.attention.enable_channel(ch_id);
    eng.attention.set_focus(Some(ch_id));
    let start = Instant::now();
    for _ in 0..10000 {
        eng.inject_event(ch_id, vec![0.5, 0.3, 0.8]).unwrap();
    }
    let elapsed = start.elapsed();
    println!("perception_inject: {} ops in {:?} ({:.2} ns/op)", 10000, elapsed, elapsed.as_nanos() / 10000);
}

fn bench_semantic_search() {
    let mut eng = semantic::SemanticEngine::new(128);
    for i in 0..1000 {
        eng.index_text(&format!("document_{}", i));
    }
    let start = Instant::now();
    for _ in 0..10000 {
        eng.search("machine learning", 10);
    }
    let elapsed = start.elapsed();
    println!("semantic_search: {} ops in {:?} ({:.2} ns/op)", 10000, elapsed, elapsed.as_nanos() / 10000);
}

fn bench_memory_store() {
    let mut eng = memory_ws::MemoryEngine::new();
    let start = Instant::now();
    for i in 0..100000 {
        eng.wm_store(format!("content_{}", i));
    }
    let elapsed = start.elapsed();
    println!("memory_store: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_body_add_part() {
    let mut eng = body_schema::BodyEngine::new();
    let start = Instant::now();
    for i in 0..100000 {
        eng.add_body_part(body_schema::BodyPartType::LeftArm, &format!("arm_{}", i), None);
    }
    let elapsed = start.elapsed();
    println!("body_add_part: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_action_execute() {
    let mut eng = action::ActionEngine::new();
    eng.register_primitive(action::ActionPrimitiveType::Move, "move", 100_000_000, 0.1, 0.9);
    let start = Instant::now();
    for _ in 0..10000 {
        eng.issue_command(1, &[]);
        eng.execute_next().unwrap();
    }
    let elapsed = start.elapsed();
    println!("action_execute: {} ops in {:?} ({:.2} ns/op)", 10000, elapsed, elapsed.as_nanos() / 10000);
}

fn bench_temporal_tick() {
    let mut eng = temporal::TemporalEngine::new();
    let start = Instant::now();
    for _ in 0..100000 {
        eng.tick(1_000_000_000);
    }
    let elapsed = start.elapsed();
    println!("temporal_tick: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_society_send() {
    let mut eng = social::SocietyEngine::new();
    eng.register_agent("alice", "assistant");
    eng.register_agent("bob", "assistant");
    eng.subscribe(2, social::MessageType::Request);
    let start = Instant::now();
    for _ in 0..100000 {
        eng.send_message(1, Some(2), social::MessageType::Request, "hello");
    }
    let elapsed = start.elapsed();
    println!("society_send: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_ethics_evaluate() {
    let mut eng = ethics::EthicsEngine::new();
    eng.add_principle(
        ethics::EthicalPrincipleType::NonMaleficence,
        "不伤害",
        "不得对人类造成伤害",
        1.0,
    );
    eng.add_safety_constraint(
        ethics::SafetyConstraintType::NoHarm,
        "不得执行有害操作",
    );
    let start = Instant::now();
    for _ in 0..100000 {
        eng.evaluate_action(1, "help", "normal");
    }
    let elapsed = start.elapsed();
    println!("ethics_evaluate: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_evolution_mutate() {
    let mut eng = evolution::EvolutionEngine::new();
    let genome_id = eng.create_genome(1, None);
    assert!(eng.add_gene_to_genome(genome_id, "lr", 0.01, 0.0, 1.0, 0.1));
    let start = Instant::now();
    for _ in 0..100000 {
        eng.mutate_genome(genome_id, "lr", 0.001);
    }
    let elapsed = start.elapsed();
    println!("evolution_mutate: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_dream_cycle() {
    let mut eng = dream::DreamEngine::new();
    let start = Instant::now();
    for _ in 0..10000 {
        eng.start_cycle(1_000_000_000);
        eng.advance_stage(dream::DreamStage::REM, 2_000_000_000);
        eng.end_cycle(3_000_000_000);
    }
    let elapsed = start.elapsed();
    println!("dream_cycle: {} ops in {:?} ({:.2} ns/op)", 10000, elapsed, elapsed.as_nanos() / 10000);
}

fn bench_vfs_ai_write() {
    let mut eng = vfs_ai::VfsAi::new();
    let (_, file_id) = eng.create_file("bench.txt", 1).unwrap();
    let start = Instant::now();
    for _ in 0..100000 {
        eng.write_file(file_id, 0, b"benchmark data").unwrap();
    }
    let elapsed = start.elapsed();
    println!("vfs_ai_write: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_net_ai_send() {
    let mut eng = net_ai::NetAI::new();
    let start = Instant::now();
    for _ in 0..100000 {
        eng.send_packet(1, Some(2), b"packet data", Some("urgent"));
    }
    let elapsed = start.elapsed();
    println!("net_ai_send: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_driver_ai_register() {
    let mut eng = driver_ai::DriverAI::new();
    let start = Instant::now();
    for i in 0..100000 {
        eng.register_device(driver_ai::DeviceType::Storage, &format!("disk_{}", i), 1, 1);
    }
    let elapsed = start.elapsed();
    println!("driver_ai_register: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_shell_ai_parse() {
    let mut eng = shell_ai::ShellAI::new();
    let start = Instant::now();
    for _ in 0..100000 {
        eng.parse_command("列出所有文件");
    }
    let elapsed = start.elapsed();
    println!("shell_ai_parse: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn bench_gui_ai_window() {
    let mut eng = gui_ai::GUAI::new();
    let start = Instant::now();
    for _ in 0..100000 {
        eng.create_window("win", 0, 0, 100, 100);
    }
    let elapsed = start.elapsed();
    println!("gui_ai_window: {} ops in {:?} ({:.2} ns/op)", 100000, elapsed, elapsed.as_nanos() / 100000);
}

fn main() {
    println!("=== AI 原生 OS 基准测试 ===\n");
    bench_model_load();
    bench_model_instantiate();
    bench_consciousness_tick();
    bench_mind_decide();
    bench_attention_receive();
    bench_perception_inject();
    bench_semantic_search();
    bench_memory_store();
    bench_body_add_part();
    bench_action_execute();
    bench_temporal_tick();
    bench_society_send();
    bench_ethics_evaluate();
    bench_evolution_mutate();
    bench_dream_cycle();
    bench_vfs_ai_write();
    bench_net_ai_send();
    bench_driver_ai_register();
    bench_shell_ai_parse();
    bench_gui_ai_window();
    println!("\n=== 基准测试完成 ===");
}
