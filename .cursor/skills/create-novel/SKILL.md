---
name: create-novel
description: >-
  超长篇多 Agent 小说创作工作流。Use when the user asks to create a novel,
  start a novel project, run chapter pipeline, activate writing agents, or
  work with the novel-agents system. 项目会自动判断是否需要激活扩展 Agent。
---

# Create Novel — 多 Agent 小说创作（Rust / NovelX）

本 Skill 指导 Agent 使用 `novel-agents`（Rust）进行超长篇协同创作。

改代码 / 改框架时另见 [novelx-dev](../novelx-dev/SKILL.md)：**产品代码须题材与作品中立，禁止把特定样例小说写进 crates/web/config。**

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
  → continue_writing 等工具按 config/pipeline.yaml 的 order
       spawn_agent(role) + wait_agent
```

写作 Agent（writer 等）是 **可 spawn 的 SubAgent Thread**，不是独立对等 UI。

## 快速开始

```bash
cd /path/to/novel-agents

cargo run -p novelx-cli -- init my-novel --genre 未定 --chapters 100
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
| lore_librarian | Lore query |
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
| foreshadow_tracker / literary_editor | 伏笔/风格 |
| entity_designer / plot_designer / setting_auditor | Studio 介入，不进章流水线 |

## 流水线顺序

见 `config/pipeline.yaml` 的 `order` / `mvp`（Rust 经 `PipelineConfig` 加载）：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator → dialogue_specialist → scene_specialist
→ consistency_auditor → foreshadow_tracker → pacing_reviewer
→ literary_editor → summarizer → plot_acceptor
```

修订路径默认 `prefer_local_patch`（`novelx-draft-patch` + `novelx-pipeline`）。

## 工作流

### 新建小说

1. `cargo run -p novelx-cli -- init <name> --genre <题材> --chapters <N>`
2. 检查 `projects/<name>/`
3. 向用户汇报状态

### 写每一章

1. `status` 查看进度
2. `run <name> <chapter>` 续写；修订用 `--revise --instructions ...`
3. 一致性 FAIL 时局部修订，不要跳过审校

## LLM 配置

| 配置 | 文件 |
|------|------|
| API Key | `.env` 的 `DEEPSEEK_API_KEY` |
| 任务 → 模型 | `config/llm.yaml` |
| Skills | `config/skills/**/SKILL.md` |

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
