# novel-agents (NovelX)

超长篇小说多 Agent 协同创作系统（Rust）。对齐 OpenAI Codex CLI 的 Thread / Turn / Item 与 Skills 渐进披露。

## 特性

- **Codex 式 Agent Loop**：Thread → Turn → Item（Skill / Tool / DraftPatch）
- **Skills 渐进披露**：启动只注入 name+description；`$skill` 激活后才加载全文
- **局部补丁优先**：修订默认段级 grep 替换，失败才全文回退
- **MVP Agent 流水线**：章纲 → Writer → 审校 → 摘要（扩展 Agent 按需）
- **NovelX Web**：Turn 时间线、Skill 弹出、ToolCall / Patch 卡片

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

# 创建项目
cargo run -p novelx-cli -- init my-novel --genre 玄幻 --chapters 100

# 跑一章（新章全文）
cargo run -p novelx-cli -- run my-novel 1

# 局部优先修订
cargo run -p novelx-cli -- run my-novel 1 --revise --instructions "改第2段，加强冲突"

# 状态
cargo run -p novelx-cli -- status my-novel
```

### Web（NovelX）

```bash
# 终端 1：Rust API（默认 127.0.0.1:8765）
cargo run -p novelx-cli -- web

# 终端 2：前端
cd web && npm install && npm run dev
# → http://127.0.0.1:5173
```

生产可先 `cd web && npm run build`，再只启 `novel web`（静态资源由 axum 托管）。

## LLM 配置

1. 复制 `.env.example` → `.env`，填入 `DEEPSEEK_API_KEY`
2. 任务/模型映射：`config/llm.yaml`
3. Agent Skills：`config/skills/**/SKILL.md`（YAML frontmatter：`name` / `description`）

## 架构

```
crates/
  novelx-protocol/     # EventMsg / TurnItem / Op
  novelx-skills/       # 渐进披露 loader
  novelx-draft-patch/  # 局部段落补丁
  novelx-harness/      # gates / 硬规则 / 优先级
  novelx-llm/          # OpenAI-compatible 客户端
  novelx-pipeline/     # 章流水线（local-first revise）
  novelx-tools/        # continue / revise / audit …
  novelx-core/         # ThreadManager + run_turn
  novelx-app-server/   # axum HTTP + WS
  novelx-cli/          # novel 二进制
config/                # agents.yaml / llm.yaml / skills
projects/<name>/       # 小说数据（state.json、chapters、entities…）
web/                   # NovelX React 前端
```

## Cursor Skill

`.cursor/skills/create-novel/SKILL.md` 说明如何用本系统写长篇。
