---
name: create-novel
description: >-
  超长篇多 Agent 小说创作工作流。Use when the user asks to create a novel,
  start a novel project, run chapter pipeline, activate writing agents, or
  work with the novel-agents system. 项目会自动判断是否需要激活扩展 Agent。
---

# Create Novel — 多 Agent 小说创作（Rust / NovelX）

本 Skill 指导 Agent 使用 `novel-agents`（Rust）进行超长篇协同创作。

改代码 / 改框架时另见 [novelx-dev](../novelx-dev/SKILL.md)（[reference](../novelx-dev/reference.md)）：题材中立、Prompt/Schema/门控分层、features 优先、Studio 安全闸、超长篇与落盘/Web 双层。禁止把特定样例小说写进 `crates`/`web`/`config`。

## 何时加载

- 用户要**创建新小说**、**写下一章**、**查看 Agent 状态**
- 用户提到多 Agent 小说、章纲、一致性审计、伏笔追踪
- 用户在 `novel-agents` 项目目录中工作

## 项目结构

```
novel-agents/
├── Cargo.toml
├── crates/
│   ├── novelx-protocol/       # Submission / Op / Event / AgentPath
│   ├── novelx-core/           # Codex Session：submission_loop + SubAgent
│   ├── novelx-pipeline/       # 领域单步（run_step）；编排权在 core
│   ├── novelx-tools/          # 内置工具含 spawn_agent / wait_agent
│   ├── novelx-skills/         # Skills 渐进披露
│   ├── novelx-draft-patch/    # 段级局部修订
│   ├── novelx-harness/        # gates / 硬规则
│   ├── novelx-app-server/     # axum HTTP + WS → Submission
│   └── novelx-cli/            # novel 二进制（run 走 core Session）
├── config/
│   ├── agents.yaml            # 可 spawn 的角色注册表
│   ├── llm.yaml
│   └── skills/
│       ├── studio.md          # root Agent
│       └── agents/*/SKILL.md  # SubAgent 系统提示
├── web/
└── projects/<name>/
```

## 运行时（对齐 Codex）

```
CLI / Web → Op::UserInput (Submission)
  → submission_loop → steer | RegularTask
  → run_turn (sample ↔ tools)
  → continue_writing / revise / audit
       → run_pipeline_streaming → execute_pipeline（按 pipeline.yaml order，进程内）
  → spawn_agent 仅单步专精 / 只读旁路（禁止 mode=continue|revise|audit_only 整章路径）
```

写作角色（writer 等）在主路径里是 **流水线步骤**（UI 进度行），不是每步一个 SubAgent 线程。SubAgent 用于隔离可观测的单步专精，不是并行加速串行修正。

## 快速开始

```bash
cd /path/to/novel-agents

# --chapters 仅为 state 软上限/占位，不驱动「先写到第 N 章」；创作按卷推进
cargo run -p novelx-cli -- init my-novel --genre 未定
cargo run -p novelx-cli -- run my-novel 1
cargo run -p novelx-cli -- run my-novel 1 --revise --instructions "改第2段"
cargo run -p novelx-cli -- status my-novel
cargo run -p novelx-cli -- agents

# Web
cargo run -p novelx-cli -- web
cd web && npm run dev
```

Release 二进制：`cargo build -p novelx-cli --release` → `./target/release/novel`

## Agent 分层

### Root

| ID | 职责 |
|----|------|
| studio_agent | 用户面对的主 Agent（工具 + spawn） |

### MVP SubAgent（默认启用）

| ID | 职责 |
|----|------|
| chapter_planner | 输出章纲 |
| lore_librarian | Lore **确定性** query（非 LLM；摘要后 assert） |
| writer | 写正文初稿（修订默认局部补丁） |
| consistency_auditor | 一致性审计 |
| pacing_reviewer | 节奏审查 |
| summarizer | 生成摘要入库 |
| plot_acceptor | 对照收束条件验收；发布后 completed |

### 扩展（按需）

| ID | 典型触发条件 |
|----|-------------|
| world_architect | 尚无 Bible |
| master_planner / arc_planner | 尚无总纲/卷纲 |
| dialogue_specialist / scene_specialist | 对话密集 / 战斗高潮 |
| foreshadow_tracker / literary_editor | 伏笔/风格（润色：activation 建议 + Studio `activate_agents`，非每章必跑） |
| entity_designer / plot_designer / setting_auditor | Studio 介入；剧情收束后自动巡检（±轻量同步），不进章流水线 order |
| volume_auditor | Studio `audit_volume`；不进章流水线 |
| expectation_reviewer | 预期事件检阅（硬条件满足时）；不进章流水线 order |

## 流水线顺序

见 `config/pipeline.yaml` 的 `order` / `mvp`（Rust 经 `PipelineConfig` 加载）：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator → dialogue_specialist → scene_specialist
→ pacing_reviewer → literary_editor → consistency_auditor
→ foreshadow_tracker → summarizer → plot_acceptor
```

修订路径默认 `prefer_local_patch`（`novelx-draft-patch` + `novelx-pipeline`）。

## 工作流

### 新建小说

1. CLI：`init <name> --genre <题材>`（可选 `--chapters` 仅软上限）；或 Web/Studio：`create_novel` / `init_novel`
2. 立项字段约定见 `config/skills/novel-draft.md`（非独立提取器）
3. `lock_brief` → 总纲/卷纲 → **补齐 Bible（0/1/2/7）** → `confirm_setup`
4. 剧情卡收束后会自动设定巡检（±轻量同步）；卷末仍走 `sync_volume` 门控

### 写每一章

1. `status` 查看进度
2. `run <name> <chapter>` 续写；修订用 `--revise --instructions ...`
3. 一致性 FAIL 时局部修订，不要跳过审校
4. Studio：跳章硬拦；落盘突变默认预览，用户确认后再写；局部修订先出 diff（见 `studio.enforce_chapter_order` / `studio.require_mutation_confirm`）
5. **批写到卡点**（超长篇吞吐）：`run <name> --batch [--max-chapters N] [--until-chapter M]`，或工具 `continue_writing_batch`；遇一致性/卷审/卷末/字数阻断即停
6. **卷软重规划**：`replan_volume` 生成不锁章号的台阶草案（`artifacts/arc_outlines/{NN}.replan.md`）

### 超长篇旋钮

| 配置 | 作用 |
|------|------|
| `config/chapter.yaml` | 章长 5000–6000；硬门 4500；连续 SoftShort 升格 |
| `config/longform.yaml` | `quality_tier` / `audit_tier` / `impact_scan_mode` / `batch_max_chapters` |
| `config/continuity.yaml` | 晚期章 CanonContext 预算 |
| `config/volume.yaml` | 薄卷中审 / 厚卷警告阈值 |

## LLM 配置

| 配置 | 文件 |
|------|------|
| API Key | `.env` 的 `DEEPSEEK_API_KEY` |
| 任务 → 模型 | `config/llm.yaml` |
| Skills | `config/skills/**`（含共享 pitfalls / formats / volume-lifecycle） |

无 Key 时 LLM 客户端降级占位回复。

## 添加新 Agent

1. 在 `config/agents.yaml` 注册
2. 添加 `config/skills/agents/{kebab-name}/SKILL.md`（含 frontmatter）
3. 若进章流水线：更新 `config/pipeline.yaml` 的 `order` / `mvp` / `handlers`
   - 专改正文类：`kind: specialist_rewrite` + `focus`，无需改 Rust
   - 新 kind：再补 `novelx-pipeline` 步骤逻辑
4. `cargo run -p novelx-cli -- agents` 验证

## 禁止事项

- 不要跳过 consistency_auditor 直接发布
- 不要在 Writer 中擅自新增重大设定
- 不要手动改 `state.json` 的 `active_agents` 除非用户明确要求
- 不要把全部已写正文塞进单次 prompt（用 summaries + 局部 span）

## 参考

- Agent 定义：`config/agents.yaml`
- Policy：`config/skills/activation-policy.md`
- 流水线：`crates/novelx-pipeline`
- Agent loop：`crates/novelx-core`
- 局部补丁：`crates/novelx-draft-patch`
