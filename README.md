# novel-agents (NovelX)

超长篇多 Agent 协同创作系统（Rust）。对齐 OpenAI Codex CLI：Submission / Session / SubAgent spawn，Skills 渐进披露；面向 800–1000 章 × 5–6k 字的卷式推进。

## 特性

- **Codex 式运行时**：CLI / Web → `Op::UserInput` → Session `submission_loop` → `run_turn`（sample ↔ tools）→ `spawn_agent` / `wait_agent`
- **Skills 渐进披露**：启动只注入 name + description；`$skill` 激活后才加载全文
- **章流水线可配置**：`config/pipeline.yaml` 的 `order` / `mvp` / `handlers`；扩展 Agent 按需激活
- **局部补丁优先**：修订默认段级替换，失败才全文回退
- **立项与卷生命周期**：setup 门控 → 卷纲/剧情卡 → `drafting_volume` → 卷审 / `sync_volume` → 下一卷
- **超长篇吞吐**：`--batch` 无人值守续写到卡点；`longform.yaml` 控制质量档 / 审计档 / 影响扫描范围
- **Studio 安全闸**：跳章硬拦、突变预览确认、impact 级联、卷中审软门（见 `config/features.yaml`）
- **NovelX Web**：对话 Turn 时间线、Skill / ToolCall / Patch 卡片、阅读区与在线配置热重载

框架代码与 `config/` 保持**题材与作品中立**；具体小说只落在 `projects/<name>/`。

## 依赖

- Rust 1.75+（推荐最新 stable）
- Node.js 18+（前端开发）
- DeepSeek API Key（可选；无 Key 时占位模式）

## 安装与运行

```bash
# 编译 CLI
cargo build -p novelx-cli --release
# 或开发模式
cargo run -p novelx-cli -- --help

# 创建项目（--chapters 仅为 state 软上限；创作按卷推进）
cargo run -p novelx-cli -- init my-novel --genre 未定
# 可选：--chapters 900

# 跑一章（新章全文）
cargo run -p novelx-cli -- run my-novel 1

# 局部优先修订
cargo run -p novelx-cli -- run my-novel 1 --revise --instructions "改第2段，加强冲突"

# 批写到卡点（max-chapters=成功发布上限；遇一致性 FAIL / 卷审 / 卷末 / 字数等硬门即停）
cargo run -p novelx-cli -- run my-novel --batch --max-chapters 10

# 状态 / 项目列表 / Agent 注册表
cargo run -p novelx-cli -- status my-novel
cargo run -p novelx-cli -- projects
cargo run -p novelx-cli -- agents

# 将旧项目迁向锁定的阅读区 / content-formats schema
cargo run -p novelx-cli -- migrate my-novel
```

Release 二进制：`./target/release/novel`。

### Web（NovelX）

```bash
# 终端 1：Rust API（默认 127.0.0.1:8765）
cargo run -p novelx-cli -- web

# 终端 2：前端
cd web && npm install && npm run dev
# → http://127.0.0.1:5173
```

生产可先 `cd web && npm run build`，再只启 `novel web`（静态资源由 axum 托管）。

一键重启（编译 CLI + `web` dist，杀旧进程后启动）：

```bash
# macOS / Linux
./scripts/restart.sh
./scripts/restart.sh --quick      # 不编译
./scripts/restart.sh --daemon     # 后台，日志 .novelx/web.log
```

```powershell
# Windows（PowerShell；若被策略拦截：Set-ExecutionPolicy -Scope CurrentUser RemoteSigned）
.\scripts\restart.ps1
.\scripts\restart.ps1 -Quick
.\scripts\restart.ps1 -Daemon
```

## LLM 配置

1. 复制 `.env.example` → `.env`，填入 `DEEPSEEK_API_KEY`
2. 任务 / 模型映射与档位：`config/llm.yaml`（`NOVELX_LLM_PROFILE=dev|prod` 可覆盖）
3. Agent Skills：`config/skills/**/SKILL.md`（YAML frontmatter：`name` / `description`）
4. Web「配置 → 模型」可粘贴 Key（只写不读，写入 `.env` 并热重载）

无 Key 时 LLM 客户端降级占位回复。

## 章流水线（摘要）

默认顺序见 `config/pipeline.yaml`：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator → dialogue_specialist → scene_specialist
→ pacing_reviewer → literary_editor → consistency_auditor
→ foreshadow_tracker → summarizer → plot_acceptor
```

MVP 每章必考：章纲、Lore 检索、Writer、一致性、节奏、摘要、剧情验收。其余扩展角色按激活条件或 Studio `activate_agents` 介入。

典型立项路径：`lock_brief` → 总纲 / 卷纲 → Bible（0/1/2/7）→ `confirm_setup` → 写章；剧情卡收束后设定巡检；卷末走 `sync_volume`。

## 架构

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

Studio 在 `projects/<name>/.novelx/ops_journal.jsonl` 追加记录工具调用、门控选择、mutation 预览/确认/应用、impact、流水线步骤与发布结果（与内容审校 `audit` 无关）。`ChatHistoryReset` 只清聊天，**不删** journal。开关：`config/features.yaml` → `studio.ops_journal`。

- CLI（主入口）：`novel ops-log <project> [--limit N] [--kind mutation_applied] [--chapter 3] [--json]`
- HTTP（可用，Web 无 UI）：`GET /api/projects/{name}/ops_journal?limit=&chapter=&kind=&after=`

常用配置旋钮：

| 文件 | 作用 |
|------|------|
| `config/agents.yaml` | Agent 注册与激活条件 |
| `config/pipeline.yaml` | 章步骤顺序与 handler |
| `config/features.yaml` | Studio / 流水线特性开关 |
| `config/longform.yaml` | 质量档、审计档、批写上限、影响扫描 |
| `config/chapter.yaml` | 章长目标与字数硬门 |
| `config/volume.yaml` | 卷中审 / 厚卷阈值 |
| `config/gates.yaml` / `intents.yaml` | 门控与确定性意图路由 |

更细的配置边界见 [`config/README.md`](config/README.md)；阅读区格式契约见 [`config/schemas/README.md`](config/schemas/README.md)。

## Cursor Skills

| Skill | 用途 |
|-------|------|
| [`.cursor/skills/create-novel/SKILL.md`](.cursor/skills/create-novel/SKILL.md) | 用本系统写长篇（工作流） |
| [`.cursor/skills/novelx-dev/SKILL.md`](.cursor/skills/novelx-dev/SKILL.md) | 改框架：题材中立、配置/门控分层、Studio 闸、超长篇与双层格式 |
| [`.cursor/skills/novelx-dev/reference.md`](.cursor/skills/novelx-dev/reference.md) | 开发反模式与「改哪里」速查（按需） |
