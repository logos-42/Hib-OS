# Hib OS AI-Native Architecture

## Design Philosophy

Hib OS is rebuilt as an **AI-native operating system** where the core abstractions
are derived from Tanenbaum's *Modern Operating Systems*, remapped onto AI primitives:

| Traditional OS | AI-Native OS | Kernel Module |
|---|---|---|
| Process / Thread | Agent (智能体) | `kernel/ai_os/agent_proc.rs` |
| Address space / Page table | Context window / KV cache | `kernel/ai_os/ctx_mem.rs` |
| Page fault / Swap | Context overflow / Knowledge swap | `kernel/ai_os/knowledge.rs` |
| File / Inode / Directory | Knowledge unit / Semantic FS | `kernel/ai_os/knowledge.rs` |
| Device / Driver / Interrupt | Tool (MCP-style registry) | `kernel/ai_os/tool.rs` |
| Deadlock (resource graph) | Agent wait-for graph / leases | `kernel/ai_os/deadlock.rs` |
| Security (ACL / Capability) | Capability sandbox + prompt injection guard | `kernel/ai_os/security.rs` |
| Multiprocessor / Scheduler | Multi-agent parallel / VRuntime fair scheduling | `kernel/ai_os/sched_ai.rs` |
| Microkernel / Exokernel | Model container + agent sandbox | `kernel/ai_os/container.rs` |

## AI-Native Primitives (Phase 2+)

Beyond classical OS remapping, Hib OS adds **consciousness-grade primitives**:

| Primitive | Purpose | Module |
|---|---|---|
| Native (意识原子) | Qualia, intentionality, self-model | `kernel/ai_os/native.rs` |
| Mind (心智架构) | Metacognition, self-reflection | `kernel/ai_os/mind.rs` |
| Intent (意向性) | Goal hierarchy, planning | `kernel/ai_os/intent.rs` |
| Attention (注意力) | Focus, salience, filtering | `kernel/ai_os/attention.rs` |
| Semantic (语义空间) | Embeddings, vector algebra | `kernel/ai_os/semantic.rs` |
| Memory WS (工作记忆) | Episodic / semantic / procedural | `kernel/ai_os/memory_ws.rs` |
| Perception (多模态感知) | Vision / audio / text / proprioception | `kernel/ai_os/perception.rs` |
| Action (动作空间) | Motor control, tool execution | `kernel/ai_os/action.rs` |
| Temporal (时间处理) | Circadian, rhythms, sequencing | `kernel/ai_os/temporal.rs` |
| Social (多智能体社会) | Agent-to-agent communication | `kernel/ai_os/social.rs` |
| Ethics (伦理对齐) | Value alignment, constitutional AI | `kernel/ai_os/ethics.rs` |
| Evolution (自我进化) | Self-modification, recursive improvement | `kernel/ai_os/evolution.rs` |
| Dream (离线巩固) | Replay, synthesis, consolidation | `kernel/ai_os/dream.rs` |

## Channel Mapping (sense/act)

All AI-native subsystems expose `aios://*`, `model://*`, `ctx://*`,
`knowledge://*`, `tool://*`, `security://*`, `deadlock://*`, `container://*`
channels through the unified `sense`/`act` perception layer.

## Build & Test

```
cargo build       # zero errors
cargo test        # all ai_os::* tests pass
```

## Line-of-Code Roadmap

- Phase 1: core ai_os subsystems (~150k lines)
- Phase 2: consciousness primitives (~250k lines)
- Phase 3: perception/action/temporal (~200k lines)
- Phase 4: social/ethics/evolution/dream (~200k lines)
- Phase 5: AI-native FS/net/driver/shell/gui (~150k lines)
- Phase 6: tests/docs/benchmarks (~50k lines)
- **Target: 1,000,000 lines**
