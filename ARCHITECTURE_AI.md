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

## AI-Native Primitives

Beyond classical OS remapping, Hib OS adds **consciousness-grade primitives**:

| Primitive | Purpose | Module |
|---|---|---|
| Native (意识原子) | Qualia, intentionality, self-model | `kernel/ai_os/native.rs` |
| Mind (心智架构) | Metacognition, self-reflection | `kernel/ai_os/mind.rs` |
| Attention (注意力) | Focus, salience, filtering | `kernel/ai_os/attention.rs` |
| Semantic (语义空间) | Embeddings, vector algebra | `kernel/ai_os/semantic.rs` |
| Memory WS (工作记忆) | Episodic / semantic / procedural | `kernel/ai_os/memory_ws.rs` |
| Perception (多模态感知) | Vision / audio / text / proprioception | `kernel/ai_os/perception.rs` |
| Action (动作空间) | Motor control, tool execution | `kernel/ai_os/action.rs` |
| Body Schema (身体图式) | Proprioception, body ownership | `kernel/ai_os/body_schema.rs` |
| Temporal (时间处理) | Circadian, rhythms, sequencing | `kernel/ai_os/temporal.rs` |
| Social (多智能体社会) | Agent-to-agent communication | `kernel/ai_os/social.rs` |
| Ethics (伦理对齐) | Value alignment, constitutional AI | `kernel/ai_os/ethics.rs` |
| Evolution (自我进化) | Self-modification, recursive improvement | `kernel/ai_os/evolution.rs` |
| Dream (离线巩固) | Replay, synthesis, consolidation | `kernel/ai_os/dream.rs` |

## AI-Native System Services (Phase 5)

| Service | Purpose | Module | Channel |
|---|---|---|---|
| VFS_AI (语义文件系统) | Semantic inode, memory file, semantic search | `kernel/ai_os/vfs_ai.rs` | `vfs_ai://*` |
| Net_AI (智能网络) | Semantic packets, intent routing, firewall | `kernel/ai_os/net_ai.rs` | `net_ai://*` |
| Driver_AI (智能驱动) | PCI, IRQ, DMA, device tree, hotplug | `kernel/ai_os/driver_ai.rs` | `driver_ai://*` |
| Shell_AI (智能命令行) | NL parsing, completion, history, aliases | `kernel/ai_os/shell_ai.rs` | `shell_ai://*` |
| GUI_AI (智能图形界面) | Semantic window, widget, compositor | `kernel/ai_os/gui_ai.rs` | `gui_ai://*` |

## Channel Mapping (sense/act)

All AI-native subsystems expose `aios://*`, `model://*`, `ctx://*`,
`knowledge://*`, `tool://*`, `security://*`, `deadlock://*`, `container://*`,
`native://*`, `mind://*`, `attention://*`, `semantic://*`, `memory://*`,
`body://*`, `action://*`, `temporal://*`, `society://*`, `ethics://*`,
`evolution://*`, `dream://*`, `vfs_ai://*`, `net_ai://*`, `driver_ai://*`,
`shell_ai://*`, `gui_ai://*` channels through the unified `sense`/`act`
perception layer.

## Build & Test

```
cargo build       # zero errors in ai_os modules
cargo test        # all ai_os::* tests pass (pre-existing errors in other modules unrelated)
cargo run --example ai_os_bench  # run performance benchmarks
```

## Line-of-Code Progress

| Phase | Scope | Status | Estimated Lines |
|---|---|---|---|
| Phase 1 | Core ai_os subsystems (model/agent/ctx/knowledge/tool/security/deadlock/container) | Complete | ~150k |
| Phase 2 | Consciousness primitives (native/mind/attention/semantic/memory_ws) | Complete | ~250k |
| Phase 3 | Perception/action/body/temporal | Complete | ~200k |
| Phase 4 | Social/ethics/evolution/dream | Complete | ~200k |
| Phase 5 | AI-native FS/net/driver/shell/gui | Complete | ~150k |
| Phase 6 | Integration tests, benchmarks, docs | In Progress | ~50k |
| **Total Target** | **1,000,000 lines** | **~40% complete** | **~1,000,000** |

## Key Design Principles

1. **Model-Agent Duality**: Model (可执行镜像) + Agent (进程) form the dual core of AI-native OS
2. **Context Window = Address Space**: KV cache replaces page tables; context overflow = page fault
3. **Tools = Devices**: MCP-style tool registry replaces device drivers
4. **Semantic FS = Knowledge**: Vector embeddings replace path-based file lookup
5. **Consciousness = OS Primitives**: Qualia, attention, stream of consciousness are first-class OS abstractions
6. **Ethics = Security**: Constitutional AI replaces traditional ACL/capability model
7. **Evolution = Kernel Updates**: Genome mutation replaces kernel patching
