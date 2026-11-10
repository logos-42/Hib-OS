//! AI 原生 OS 基准测试 —— 测量各 ai_os 子系统性能
//!
//! 运行方式：cargo bench --bench ai_os_bench

use std::hint::black_box;
use std::time::Instant;

use hibos::kernel::ai_os::*;

fn bench_model_load(b: &mut criterion::Criterion) {
    b.bench_function("model_load", |b| {
        b.iter(|| {
            let mut reg = model::ModelRegistry::new();
            reg.load_with_spec(
                black_box("/bench-model"),
                model::ModelFormat::RawTensor,
                model::ModelSpec {
                    params: black_box(7_000_000_000),
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
        })
    });
}

fn bench_model_instantiate(b: &mut criterion::Criterion) {
    b.bench_function("model_instantiate", |b| {
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
        b.iter(|| {
            reg.instantiate(black_box("/bench-model"), black_box(256), None).unwrap();
        })
    });
}

fn bench_consciousness_tick(b: &mut criterion::Criterion) {
    b.bench_function("consciousness_tick", |b| {
        let mut eng = native::ConsciousnessEngine::new();
        b.iter(|| {
            eng.tick().unwrap();
        })
    });
}

fn bench_mind_decide(b: &mut criterion::Criterion) {
    b.bench_function("mind_decide", |b| {
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
        b.iter(|| {
            eng.decide(black_box(options.clone())).unwrap();
        })
    });
}

fn bench_attention_receive(b: &mut criterion::Criterion) {
    b.bench_function("attention_receive", |b| {
        let mut eng = attention::AttentionEngine::new();
        b.iter(|| {
            eng.receive_input(black_box("ch1"), black_box(0.7));
        })
    });
}

fn bench_perception_inject(b: &mut criterion::Criterion) {
    b.bench_function("perception_inject", |b| {
        let mut eng = perception::PerceptionEngine::new();
        let ch_id = eng.add_channel("ch1", perception::SensoryModality::Visual, "cam0");
        eng.lens.allow_channel("ch1");
        b.iter(|| {
            eng.inject_event(black_box(ch_id), black_box(vec![0.5, 0.3, 0.8])).unwrap();
        })
    });
}

fn bench_semantic_search(b: &mut criterion::Criterion) {
    b.bench_function("semantic_search", |b| {
        let mut eng = semantic::SemanticEngine::new(128);
        for i in 0..1000 {
            eng.index_text(black_box(&format!("document_{}", i)));
        }
        b.iter(|| {
            eng.search(black_box("machine learning"), black_box(10));
        })
    });
}

fn bench_memory_store(b: &mut criterion::Criterion) {
    b.bench_function("memory_store", |b| {
        let mut eng = memory_ws::MemoryEngine::new();
        b.iter(|| {
            eng.wm_store(black_box("test memory content".to_string()));
        })
    });
}

fn bench_body_add_part(b: &mut criterion::Criterion) {
    b.bench_function("body_add_part", |b| {
        let mut eng = body_schema::BodyEngine::new();
        b.iter(|| {
            eng.add_body_part(black_box(body_schema::BodyPartType::LeftArm), black_box("arm"), None);
        })
    });
}

fn bench_action_execute(b: &mut criterion::Criterion) {
    b.bench_function("action_execute", |b| {
        let mut eng = action::ActionEngine::new();
        eng.register_primitive(action::ActionPrimitiveType::Move, "move", 100_000_000, 0.1, 0.9);
        b.iter(|| {
            eng.issue_command(black_box(1), &[]);
            eng.execute_next().unwrap();
        })
    });
}

fn bench_temporal_tick(b: &mut criterion::Criterion) {
    b.bench_function("temporal_tick", |b| {
        let mut eng = temporal::TemporalEngine::new();
        b.iter(|| {
            eng.tick(black_box(1_000_000_000));
        })
    });
}

fn bench_society_send(b: &mut criterion::Criterion) {
    b.bench_function("society_send", |b| {
        let mut eng = society::SocietyEngine::new();
        eng.register_agent(black_box("alice"), black_box("assistant"));
        eng.register_agent(black_box("bob"), black_box("assistant"));
        eng.subscribe(black_box(2), black_box(society::MessageType::Request));
        b.iter(|| {
            eng.send_message(black_box(1), black_box(Some(2)), black_box(society::MessageType::Request), black_box("hello"));
        })
    });
}

fn bench_ethics_evaluate(b: &mut criterion::Criterion) {
    b.bench_function("ethics_evaluate", |b| {
        let mut eng = ethics::EthicsEngine::new();
        eng.add_principle(
            ethics::EthicalPrincipleType::NoHarm,
            "不伤害",
            "不得对人类造成伤害",
            1.0,
        );
        eng.add_safety_constraint(
            ethics::SafetyConstraintType::NoHarm,
            "不得执行有害操作",
        );
        b.iter(|| {
            eng.evaluate_action(black_box(1), black_box("help"), black_box("normal"));
        })
    });
}

fn bench_evolution_mutate(b: &mut criterion::Criterion) {
    b.bench_function("evolution_mutate", |b| {
        let mut eng = evolution::EvolutionEngine::new();
        let genome_id = eng.create_genome(1, None);
        eng.add_gene_to_genome(genome_id, "lr", 0.01, 0.0, 1.0, 0.1).unwrap();
        b.iter(|| {
            eng.mutate_genome(black_box(genome_id), black_box("lr"), black_box(0.001));
        })
    });
}

fn bench_dream_cycle(b: &mut criterion::Criterion) {
    b.bench_function("dream_cycle", |b| {
        let mut eng = dream::DreamEngine::new();
        b.iter(|| {
            eng.start_cycle(black_box(1_000_000_000));
            eng.advance_stage(black_box(dream::DreamStage::REM), black_box(2_000_000_000));
            eng.end_cycle(black_box(3_000_000_000));
        })
    });
}

fn bench_vfs_ai_write(b: &mut criterion::Criterion) {
    b.bench_function("vfs_ai_write", |b| {
        let mut eng = vfs_ai::VFS_AI::new();
        let (_, file_id) = eng.create_file(black_box("bench.txt"), 1).unwrap();
        b.iter(|| {
            eng.write_file(black_box(file_id), black_box(0), black_box(b"benchmark data")).unwrap();
        })
    });
}

fn bench_net_ai_send(b: &mut criterion::Criterion) {
    b.bench_function("net_ai_send", |b| {
        let mut eng = net_ai::NetAI::new();
        b.iter(|| {
            eng.send_packet(black_box(1), black_box(Some(2)), black_box(b"packet data"), black_box(Some("urgent")));
        })
    });
}

fn bench_driver_ai_register(b: &mut criterion::Criterion) {
    b.bench_function("driver_ai_register", |b| {
        let mut eng = driver_ai::DriverAI::new();
        b.iter(|| {
            eng.register_device(
                black_box(driver_ai::DeviceType::Storage),
                black_box("disk"),
                black_box(1),
                black_box(1),
            );
        })
    });
}

fn bench_shell_ai_parse(b: &mut criterion::Criterion) {
    b.bench_function("shell_ai_parse", |b| {
        let mut eng = shell_ai::ShellAI::new();
        b.iter(|| {
            eng.parse_command(black_box("列出所有文件"));
        })
    });
}

fn bench_gui_ai_window(b: &mut criterion::Criterion) {
    b.bench_function("gui_ai_window", |b| {
        let mut eng = gui_ai::GUAI::new();
        b.iter(|| {
            eng.create_window(black_box("win"), black_box(0), black_box(0), black_box(100), black_box(100));
        })
    });
}

criterion::criterion_group!(
    ai_os_benches,
    bench_model_load,
    bench_model_instantiate,
    bench_consciousness_tick,
    bench_mind_decide,
    bench_attention_receive,
    bench_perception_inject,
    bench_semantic_search,
    bench_memory_store,
    bench_body_add_part,
    bench_action_execute,
    bench_temporal_tick,
    bench_society_send,
    bench_ethics_evaluate,
    bench_evolution_mutate,
    bench_dream_cycle,
    bench_vfs_ai_write,
    bench_net_ai_send,
    bench_driver_ai_register,
    bench_shell_ai_parse,
    bench_gui_ai_window,
);

criterion::criterion_main!(ai_os_benches);
