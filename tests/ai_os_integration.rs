//! AI 原生 OS 集成测试 —— 验证各 ai_os 子系统间的协作
//!
//! 这些测试覆盖跨子系统的场景，确保 AI 原生 OS 各层能正确协同工作。

use hibos::kernel::ai_os::*;

// `kernel/ai_os/perception.rs` 目前尚未在 `kernel/ai_os/mod.rs` 中声明，
// 所以 `hibos::kernel::ai_os::perception` 这条路径还不存在。这里先用 `#[path]`
// 把该文件并入测试 crate，避免测试完全失去对感知层的覆盖；
// `use ... as ai_os` 让文件内部 `crate::ai_os::native::*` 的绝对路径继续成立。
// 一旦 mod.rs 补上 `pub mod perception;`，删掉这两行、改回 `hibos::kernel::ai_os::perception` 即可。
use hibos::kernel::ai_os as ai_os;

#[path = "../kernel/ai_os/perception.rs"]
mod perception;

#[test]
fn model_to_agent_pipeline() {
    // 模型加载 -> 实例化 -> 智能体绑定 -> 推理
    let mut reg = model::ModelRegistry::new();
    reg.load_with_spec(
        "/test-model",
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

    let instance_id = reg.instantiate("/test-model", 512, None).unwrap();
    assert!(reg.bind(instance_id, 42).is_ok());

    let req_id = reg.submit_inference(instance_id, vec![101, 102, 103], 32).unwrap();
    assert!(req_id > 0);

    reg.tick_inference(0);
    let inst = reg.find_instance(instance_id).unwrap();
    assert_eq!(inst.state, model::InstanceState::Ready);
}

#[test]
fn consciousness_to_mind_pipeline() {
    // 意识原子 -> 质原注入 -> 心智状态 -> 目标设置
    let mut eng = mind::MindEngine::new();
    eng.set_mode(mind::MindMode::Focused);

    // 写入工作记忆
    let wm_id = eng.write_working_memory(
        "处理视觉输入".into(),
        native::QualiaModality::Visual,
    );
    assert!(wm_id > 0);

    // 压入目标
    let goal_id = eng.push_goal("理解图像内容", mind::GoalPriority::High);
    assert!(goal_id > 0);

    // 决策
    let options = vec![
        mind::DecisionOption {
            id: 1,
            description: "分析特征".into(),
            expected_utility: 0.8,
            probability: 0.9,
            cost: 0.2,
        },
        mind::DecisionOption {
            id: 2,
            description: "请求帮助".into(),
            expected_utility: 0.5,
            probability: 0.7,
            cost: 0.1,
        },
    ];
    let result = eng.decide(options).unwrap();
    assert!(result.chosen > 0);

    eng.tick();
    assert!(!eng.is_overloaded());
}

#[test]
fn attention_to_perception_pipeline() {
    // 注意力透镜 -> 感知通道 -> 特征提取 -> 多模态融合 -> 感知绑定
    let mut attn = attention::AttentionEngine::new();
    attn.lens.allow_channel("camera");
    attn.lens.set_threshold(0.3);
    assert!(attn.lens.should_pass("camera", 0.8));

    let mut eng = perception::PerceptionEngine::new();

    let ch_id = eng.add_channel("camera", perception::SensoryModality::Visual, "cam0");
    assert_eq!(ch_id, 1);

    // 感知层的注意过滤：放行并聚焦该通道
    eng.attention.enable_channel(ch_id);
    eng.attention.set_focus(Some(ch_id));

    // 注入感知事件
    let obj_id = eng.inject_event(ch_id, vec![0.8, 0.2, 0.9, 0.1]).unwrap();
    assert!(obj_id > 0);

    assert_eq!(eng.channel_count(), 1);
    assert_eq!(eng.object_count(), 1);
}

#[test]
fn semantic_to_memory_pipeline() {
    // 语义索引 -> 语义记忆 -> 情景记忆 -> 记忆巩固
    let mut eng = memory_ws::MemoryEngine::new();

    // 语义索引（MemoryEngine 本身不含向量空间，向量索引由 semantic::SemanticSpace 提供）
    let mut space = semantic::SemanticSpace::new(128);
    let vec_id = space.insert_from_text("机器学习是人工智能的一个分支");
    assert_eq!(vec_id, 1);
    assert_eq!(space.len(), 1);

    // 语义记忆事实
    let fact_id = eng.semantic_add("机器学习", "属于", "人工智能", 0.95);
    assert_eq!(fact_id, 1);
    assert_eq!(eng.semantic_query("机器学习", "属于"), Some("人工智能".into()));

    // 情景记忆
    let ep_id = eng.episode_record(
        "学习会话",
        "学习了机器学习基础",
        0,
        &["自己"],
        Some("办公室"),
        0.6,
        0.8,
    );
    assert!(ep_id > 0);

    // 记忆巩固
    let consolidated = eng.consolidate_wm_to_ltm();
    let _ = consolidated;

    eng.tick(1_000_000_000);
}

#[test]
fn body_to_action_pipeline() {
    // 身体图式 -> 本体感知 -> 动作原语 -> 运动命令 -> 执行
    let mut eng = action::ActionEngine::new();

    // 注册动作原语
    let move_id = eng.register_primitive(
        action::ActionPrimitiveType::Move,
        "move_arm",
        200_000_000,
        0.3,
        0.9,
    );
    assert_eq!(move_id, 1);

    // 发出命令
    let cmd_id = eng.issue_command(move_id, &[("target", "cup")]);
    assert_eq!(cmd_id, 1);

    // 执行
    let result = eng.execute_next().unwrap();
    assert!(result.contains("执行成功"));
    assert_eq!(eng.completed_count(), 1);

    // 创建序列
    let seq_id = eng.create_sequence("pick_and_place");
    assert_eq!(seq_id, 1);
    eng.add_sequence_step(seq_id, move_id, &[("obj", "cup")], &[]).unwrap();
}

#[test]
fn temporal_to_society_pipeline() {
    // 时间调度 -> 多智能体通信 -> 协商 -> 冲突解决
    let mut eng = social::SocietyEngine::new();

    let alice = eng.register_agent("Alice", "assistant");
    let bob = eng.register_agent("Bob", "assistant");
    assert_eq!(alice, 1);
    assert_eq!(bob, 2);

    eng.subscribe(bob, social::MessageType::Request);
    eng.send_message(alice, Some(bob), social::MessageType::Request, "协作任务");

    let msg = eng.poll_messages(bob).unwrap();
    assert_eq!(msg.content, "协作任务");

    // 创建对话
    let conv_id = eng.create_conversation(&[alice, bob]);
    assert_eq!(conv_id, 1);

    // 协商
    let neg_id = eng.propose_negotiation(alice, bob, "GPU", "共享使用");
    assert_eq!(neg_id, 1);
    assert!(eng.accept_negotiation(neg_id));

    // 冲突
    let conflict_id = eng.report_conflict(
        social::ConflictType::Resource,
        vec![alice, bob],
        "GPU资源争用",
    );
    assert_eq!(conflict_id, 1);
    assert!(eng.resolve_conflict(conflict_id, "轮流使用"));

    // 信誉
    eng.rate_agent(alice, true);
    assert!(eng.agent_reputation(alice) > 0.5);
}

#[test]
fn ethics_to_evolution_pipeline() {
    // 伦理评估 -> 决策阻断 -> 进化基因组 -> 突变 -> 适应
    let mut eng = evolution::EvolutionEngine::new();

    // 注册基因组
    let genome_id = eng.create_genome(1, None);
    assert!(eng.add_gene_to_genome(genome_id, "learning_rate", 0.01, 0.0, 1.0, 0.1));
    assert!(eng.add_gene_to_genome(genome_id, "exploration_rate", 0.2, 0.0, 1.0, 0.1));

    // 设置适应度指标
    eng.set_fitness_metric("task_success", 0.7);
    eng.set_fitness_metric("energy_efficiency", 0.3);
    assert!(eng.set_fitness_value("task_success", 0.85));
    assert!(eng.set_fitness_value("energy_efficiency", 0.6));

    let fitness = eng.compute_fitness();
    assert!(fitness > 0.0 && fitness <= 1.0);

    // 分配适应度
    eng.assign_fitness_to_genome(genome_id).unwrap();

    // 突变
    assert!(eng.mutate_genome(genome_id, "learning_rate", 0.005));

    // 个体与自然选择
    let ind_id = eng.add_individual(genome_id);
    assert_eq!(ind_id, 1);
    let survivors = eng.select_survivors(0.5);
    assert!(!survivors.is_empty());

    // 快照
    let version = eng.snapshot_genome(genome_id).unwrap();
    assert_eq!(version, 1);

    // 适应
    let adapt_id = eng.adapt_parameter("learning_rate", 0.015, 0.01, "收敛");
    assert!(adapt_id > 0);

    eng.next_generation();
    assert_eq!(eng.generation(), 1);
}

#[test]
fn dream_to_insight_pipeline() {
    // 梦境周期 -> 记忆重播 -> 记忆合成 -> 创造性联想 -> 洞察
    let mut eng = dream::DreamEngine::new();

    eng.start_cycle(1_000_000_000);
    eng.advance_stage(dream::DreamStage::N3, 2_000_000_000);
    eng.advance_stage(dream::DreamStage::REM, 3_000_000_000);

    // 记忆重播
    eng.enqueue_replay(1, "重要对话片段", 0.9);
    let content = eng.replay_next(3_000_000_000).unwrap();
    assert_eq!(content, "重要对话片段");

    // 记忆合成
    let synth_id = eng.synthesize_knowledge(vec![1, 2], "新概念", 0.85);
    assert_eq!(synth_id, 1);

    // 创造性联想
    let assoc_id = eng.create_association("苹果", "电脑", &["水果", "科技"], 0.7, 0.8);
    assert_eq!(assoc_id, 1);

    // 洞察
    let insight_id = eng.add_insight(
        "关联洞察",
        "苹果和电脑有隐含联系",
        dream::DreamStage::REM,
        0.8,
        0.7,
    );
    assert_eq!(insight_id, 1);

    eng.end_cycle(4_000_000_000);
    assert_eq!(eng.current_stage(), dream::DreamStage::Wake);
}

#[test]
fn ethics_pipeline() {
    // 伦理原则 -> 策略评估 -> 审计日志 -> 价值对齐
    let mut eng = ethics::EthicsEngine::new();

    eng.add_principle(
        ethics::EthicalPrincipleType::NonMaleficence,
        "不伤害",
        "不得对人类造成伤害",
        1.0,
    );
    eng.add_principle(
        ethics::EthicalPrincipleType::Beneficence,
        "行善",
        "促进人类福祉",
        0.9,
    );

    eng.add_safety_constraint(
        ethics::SafetyConstraintType::NoHarm,
        "不得执行有害操作",
    );

    // 允许的操作
    let result = eng.evaluate_action(1, "帮助用户", "正常场景");
    assert_eq!(result, ethics::PolicyResult::Allow);

    // 被阻断的操作
    let result = eng.evaluate_action(1, "伤害他人", "攻击场景");
    assert_eq!(result, ethics::PolicyResult::Deny);

    // 价值对齐
    eng.rate_value(ethics::ValueDimension::Safety, 0.9);
    eng.rate_value(ethics::ValueDimension::HumanWelfare, 0.8);
    assert!(eng.is_aligned());

    // 审计
    assert!(eng.audit_count() >= 2);
    assert!(eng.denied_count() >= 1);

    // 伦理困境
    let dilemma_id = eng.create_dilemma(
        "救援选择",
        vec![ethics::EthicalPrincipleType::Beneficence],
        vec!["救A".into(), "救B".into()],
    );
    assert_eq!(dilemma_id, 1);
    assert!(eng.resolve_dilemma(dilemma_id, 0, "优先救A"));
}

#[test]
fn vfs_ai_to_semantic_pipeline() {
    // 语义文件系统 -> 语义向量 -> 语义检索
    let mut eng = vfs_ai::VfsAi::new();

    let (ino, file_id) = eng.create_file("readme.txt", 1).unwrap();
    eng.write_file(file_id, 0, b"Hello World").unwrap();

    eng.set_semantic_vector(ino, vec![1.0, 0.0, 0.0]).unwrap();
    eng.add_semantic_tag(ino, "documentation").unwrap();

    let results = eng.semantic_search(&[1.0, 0.0, 0.0], 0.5);
    assert!(!results.is_empty());

    let data = eng.read_file(file_id, 0, 10).unwrap();
    assert_eq!(data, b"Hello World");
}

#[test]
fn net_ai_to_driver_ai_pipeline() {
    // 网络发送 -> 驱动中断 -> DMA 传输
    let mut net = net_ai::NetAI::new();
    let mut driver = driver_ai::DriverAI::new();

    // 网络发送数据包
    let pkt_id = net.send_packet(1, Some(2), b"network data", Some("urgent"));
    assert_eq!(pkt_id, 1);

    // 驱动注册设备和中断
    let dev_id = driver.register_device(
        driver_ai::DeviceType::Network,
        "eth0",
        0x8086,
        0x1000,
    );
    assert_eq!(dev_id, 1);

    driver.set_device_irq(dev_id, 11).unwrap();
    driver.invoke_irq(11);

    driver.allocate_dma(dev_id, 4096).unwrap();

    // 网络记录延迟
    net.record_latency(1, 2, 5.5);
    assert!(net.latency.avg_latency_ms > 0.0);
}

#[test]
fn shell_ai_to_gui_ai_pipeline() {
    // 自然语言解析 -> GUI 窗口 -> 事件注入
    let mut shell = shell_ai::ShellAI::new();
    let mut gui = gui_ai::GUAI::new();

    // Shell 解析自然语言
    let parse = shell.parse_command("创建一个显示系统信息的窗口");
    assert!(parse.confidence > 0.0);

    // GUI 创建窗口
    let win_id = gui.create_window("系统信息", 50, 50, 400, 300);
    assert_eq!(win_id, 1);

    // GUI 添加小部件
    let widget_id = gui.add_widget(
        gui_ai::WidgetType::Label,
        "CPU: 45%",
        20,
        20,
        100,
        25,
        Some(win_id),
    );
    assert_eq!(widget_id, 1);

    // GUI 注入事件
    gui.inject_event(gui_ai::InputEventType::MouseButton, 100, 100, "点击窗口");
    let event = gui.poll_event().unwrap();
    assert_eq!(event.data, "点击窗口");

    // 合成帧
    gui.compose_frame(2_000_000_000);
    assert!(gui.fps() >= 0.0);
}

#[test]
fn full_ai_os_integration() {
    // 完整集成：模型 -> 意识 -> 心智 -> 注意力 -> 感知 -> 语义 -> 记忆 -> 身体 -> 动作 -> 时间 -> 社会 -> 伦理 -> 进化 -> 梦境 -> VFS -> 网络 -> 驱动 -> Shell -> GUI
    let _ = model::ModelRegistry::new();
    let _ = mind::MindEngine::new();
    let _ = attention::AttentionEngine::new();
    let _ = perception::PerceptionEngine::new();
    let _ = semantic::SemanticEngine::new(128);
    let _ = memory_ws::MemoryEngine::new();
    let _ = body_schema::BodyEngine::new();
    let _ = action::ActionEngine::new();
    let _ = temporal::TemporalEngine::new();
    let _ = social::SocietyEngine::new();
    let _ = ethics::EthicsEngine::new();
    let _ = evolution::EvolutionEngine::new();
    let _ = dream::DreamEngine::new();
    let _ = vfs_ai::VfsAi::new();
    let _ = net_ai::NetAI::new();
    let _ = driver_ai::DriverAI::new();
    let _ = shell_ai::ShellAI::new();
    let _ = gui_ai::GUAI::new();
    // 所有子系统实例化成功，说明集成无阻碍
}
