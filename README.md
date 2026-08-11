# NovelX

用对话写长篇：立项、写章、审阅、改稿，一条链路走完。面向想稳定连载的作者，也可支撑数百章规模的卷式推进。

## 能做什么

- **立项**：说清灵感 → 总纲 / 卷纲 / 设定 → 确认后开始写章
- **写章**：按卷推进；可一章一章写，也可批量续写到需要你拍板的地方
- **审阅**：逐章或整卷检查前后文、节奏与设定是否对得上
- **改稿**：按你的要求局部改或整章修订；重要设定改动会先给你确认
- **写作台**：左侧看大纲 / 设定 / 正文，右侧用创作助手对话推进

题材与作品内容由你决定；框架本身不绑定某一本书。

## 快速开始（Web）

1. 准备 API Key：复制 `.env.example` → `.env`，填入 `DEEPSEEK_API_KEY`（也可稍后在 Web「配置 → 模型」粘贴）
2. 启动：

```bash
# 终端 1：后端（默认 127.0.0.1:8765）
cargo run -p novelx-cli -- web

# 终端 2：前端
cd web && npm install && npm run dev
# → http://127.0.0.1:5173
```

3. 在页面里新建作品，用自然语言告诉助手你想写什么。

一键重启（编译 + 启动）：

```bash
./scripts/restart.sh          # macOS / Linux
./scripts/restart.sh --quick  # 不重新编译
```

```powershell
.\scripts\restart.ps1
.\scripts\restart.ps1 -Quick
```

生产可先 `cd web && npm run build`，再只启 `novel web`（静态资源由服务端托管）。

---

## 开发者指南

以下面向改框架、跑 CLI、接配置的人。作者日常使用请看上文。

### 特性（工程视角）

- Codex 式运行时：CLI / Web → Session → tools；协作角色按需启动
- Skills 渐进披露：启动只注入摘要，激活后再加载全文
- 章流水线可配置：`config/pipeline.yaml`；扩展角色按需激活
- 局部改稿优先；立项与卷生命周期门控；批写到卡点；Studio 安全闸

### 依赖

- Rust 1.75+（推荐最新 stable）
- Node.js 18+（前端开发）
- DeepSeek API Key（可选；无 Key 时占位模式）

### CLI

```bash
cargo build -p novelx-cli --release
cargo run -p novelx-cli -- --help

cargo run -p novelx-cli -- init my-novel --genre 未定
cargo run -p novelx-cli -- run my-novel 1
cargo run -p novelx-cli -- run my-novel 1 --revise --instructions "改第2段，加强冲突"
cargo run -p novelx-cli -- run my-novel --batch --max-chapters 10

cargo run -p novelx-cli -- status my-novel
cargo run -p novelx-cli -- projects
cargo run -p novelx-cli -- agents
cargo run -p novelx-cli -- migrate my-novel
```

Release 二进制：`./target/release/novel`。

### LLM 配置

1. `.env` 中的 `DEEPSEEK_API_KEY`
2. 任务 / 模型映射：`config/llm.yaml`（`NOVELX_LLM_PROFILE=dev|prod` 可覆盖）
3. Agent Skills：`config/skills/**/SKILL.md`
4. Web「配置 → 模型」可粘贴 Key（只写不读）

### 章流水线（摘要）

默认顺序见 `config/pipeline.yaml`：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator → dialogue_specialist → scene_specialist
→ pacing_reviewer → literary_editor → consistency_auditor
→ foreshadow_tracker → summarizer → plot_acceptor
```

典型立项：`lock_brief` → 总纲 / 卷纲 → Bible → `confirm_setup` → 写章；卷末 `sync_volume`。

### 架构

```
crates/
  novelx-protocol/     # Submission / Op / Event / AgentPath
  novelx-skills/       # 渐进披露 loader
  novelx-draft-patch/  # 局部段落补丁
  novelx-harness/      # gates / 硬规则 / 激活 / longform
  novelx-llm/          # OpenAI-compatible 客户端
  novelx-pipeline/     # 领域单步（run_step）+ schemas / 卷同步
  novelx-tools/        # continue / revise / spawn_agent …
  novelx-core/         # Codex Session：submission_loop + SubAgent
  novelx-app-server/   # axum HTTP + WS → Submission
  novelx-cli/          # novel 二进制
config/                # agents / pipeline / features / longform / skills …
projects/<name>/       # 小说数据（state、chapters、entities、artifacts…）
web/                   # NovelX React 前端
```

### 决策/执行审计（ops journal）

Studio 在 `projects/<name>/.novelx/ops_journal.jsonl` 追加记录工具调用、门控选择、mutation 预览/确认/应用等。`ChatHistoryReset` 只清聊天，不删 journal。开关：`config/features.yaml` → `studio.ops_journal`。

- CLI：`novel ops-log <project> [--limit N] [--kind mutation_applied] [--chapter 3] [--json]`
- HTTP：`GET /api/projects/{name}/ops_journal?limit=&chapter=&kind=&after=`

| 文件 | 作用 |
|------|------|
| `config/agents.yaml` | Agent 注册与激活条件 |
| `config/pipeline.yaml` | 章步骤顺序与 handler |
| `config/features.yaml` | Studio / 流水线特性开关 |
| `config/longform.yaml` | 质量档、审计档、批写上限、影响扫描 |
| `config/chapter.yaml` | 章长目标与字数硬门 |
| `config/volume.yaml` | 卷中审 / 厚卷频率 |
| `config/gates.yaml` / `intents.yaml` | 门控与确定性意图路由 |

更细见 [`config/README.md`](config/README.md)、[`config/schemas/README.md`](config/schemas/README.md)。

### Cursor Skills

| Skill | 用途 |
|-------|------|
| [`.cursor/skills/create-novel/SKILL.md`](.cursor/skills/create-novel/SKILL.md) | 用本系统写长篇（工作流） |
| [`.cursor/skills/novelx-dev/SKILL.md`](.cursor/skills/novelx-dev/SKILL.md) | 改框架：题材中立、配置/门控分层、Studio 闸 |
| [`.cursor/skills/novelx-dev/reference.md`](.cursor/skills/novelx-dev/reference.md) | 开发反模式与「改哪里」速查 |
