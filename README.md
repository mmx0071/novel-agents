# NovelX

本仓库是 DeepSeek Harness（dsh）上的 **NovelX 写作预设**，外加本机阅读台。对话写书在 dsh 里完成；这里提供落盘格式、禁名，以及 Node 阅读台。

**融入方式是 dsh 官方的 agent preset**（`~/.dsh/.agent-presets/`），不是 `dsh plugin add`。完整步骤见 [接入文档](integrations/dsh-preset-novelx/README.md)。

| 层 | 做什么 | 不做什么 |
|----|--------|----------|
| **dsh 预设 `novelx`** | 会话模式：路由、子 agent、直接改 `projects/` | 不走 Host `continue_writing` |
| **预设内 `desk.ts` / `check.ts`** | 阅读台跟随；`novelx_check` 做 schema/禁名/字数/相位 | 不代替写章；不是 Host bundle |
| **本仓库 `web/`** | 阅读台（`desk-server.mjs`）、schema 展示 | 不再当对话写作入口 |

题材与作品内容由你决定；框架不绑定某一本书。

## 快速开始（dsh 写作）

一次性：装阅读台前端，把预设拷到官方用户预设目录。

```bash
cd web && npm install && npm run build && cd ..
mkdir -p ~/.dsh/.agent-presets/novelx
rsync -a --delete integrations/dsh-preset-novelx/preset/ ~/.dsh/.agent-presets/novelx/
```

每次写：工作区选 **本仓库根**，启动 dsh，**新开会话**，预设选 **NovelX**。

```bash
npx @deepseek-ai/dsh web          # 或源码：pnpm dsh web
# 打开提示的地址（常见 :3080）
```

写设定/写章时会拉起阅读台（`:8765`）跟随当前作品。落盘后 Agent 调 `novelx_check`；旧格式调 `novelx_migrate`。不要用标准模式写书。

## 阅读台（可视化，不是对话入口）

台子只看稿、跟章。对话仍在 dsh。需要时 Agent 调 `novelx_open_desk`，或：

```bash
cd web && npm run desk            # 127.0.0.1:8765（需先 npm run build，或已有 node_modules 时走 Vite）
```

一键重启：

```bash
./scripts/restart.sh              # 挂载 NovelX、构建前端、启动阅读台 + dsh
./scripts/restart.sh --quick      # 不重新 npm build
./scripts/restart.sh --foreground # 阅读台后台，dsh 前台
```

```powershell
.\scripts\restart.ps1
.\scripts\restart.ps1 -Quick
```

生产可先 `cd web && npm run build`，再 `npm run desk`（托管 `web/dist`）。

---

## 开发者指南

以下面向改**预设 / 阅读台 / 检查规则**的人。作者日常在 dsh 选 NovelX 即可。本仓库不再包含 Rust 引擎。

### 依赖

- Node.js 18+
- dsh（`npx @deepseek-ai/dsh` 或本机源码）

### 章流水线（摘要）

默认顺序见 `config/pipeline.yaml`：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator → dialogue_specialist → scene_specialist
→ pacing_reviewer → literary_editor → consistency_auditor
→ foreshadow_tracker → summarizer → plot_acceptor
```

典型立项：总纲 / 卷纲 → Bible → 写章。旧格式用 `novelx_migrate`。

### 架构

```
integrations/dsh-preset-novelx/preset/   # dsh 写作预设（权威入口）
web/                                     # 阅读台 + novelx_check / migrate
config/                                  # 禁名 / 字数 / 硬规则 / 章步骤顺序
projects/<name>/                         # 作品落盘（只经 dsh 预设写）
```

| 文件 | 作用 |
|------|------|
| `config/pipeline.yaml` | 章步骤顺序（dsh 子 agent 按此委派） |
| `config/chapter.yaml` | 章长目标与字数硬门（`novelx_check` 读取） |
| `config/naming_rules.yaml` | 系统禁名 |
| `config/content_rules.yaml` | 正文硬规则 |
| `config/features.yaml` | setup / 卷相位 / 跳章开关 |
| `config/script.yaml` | 短剧篇幅默认（阅读台字数条） |

更细见 [`config/README.md`](config/README.md)、[`config/schemas/README.md`](config/schemas/README.md)。

### Cursor Skills

| Skill | 用途 |
|-------|------|
| [`.cursor/skills/create-novel/SKILL.md`](.cursor/skills/create-novel/SKILL.md) | 在 dsh 用 NovelX 预设写长篇 |
| [`.cursor/skills/novelx-dev/SKILL.md`](.cursor/skills/novelx-dev/SKILL.md) | 改框架：预设 / 阅读台 / schema，作品交给 dsh 落盘 |
| [`.cursor/skills/novelx-dev/reference.md`](.cursor/skills/novelx-dev/reference.md) | 开发反模式与「改哪里」速查 |
