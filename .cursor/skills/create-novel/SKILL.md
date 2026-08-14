---
name: create-novel
description: >-
  超长篇多 Agent 小说创作工作流。Use when the user asks to create a novel,
  start a novel project, run chapter pipeline, activate writing agents, or
  work with the novel-agents system. 项目会自动判断是否需要激活扩展 Agent。
---

# Create Novel — dsh NovelX 预设写作

本 Skill 指导在 **DeepSeek Harness** 里用 NovelX **预设**写长篇（不是 `dsh plugin add`）。

源：`integrations/dsh-preset-novelx/preset/`。子 agent 直接改 `projects/`。阅读台由预设内 `desk.ts` 的 `novelx_open_desk` 拉起。

改框架见 [novelx-dev](../novelx-dev/SKILL.md)。禁止手改 `projects/<name>/` 代替预设落盘。

## 何时加载

- 用户要**创建新小说**、**写下一章**、**查看投放进度**
- 用户提到多 Agent 小说、章纲、一致性审计、伏笔追踪
- 用户在 `novel-agents` 项目目录中工作

## 项目结构

```
novel-agents/
├── integrations/dsh-preset-novelx/preset/  # dsh 写作预设
├── config/                                 # 禁名 / 字数 / 硬规则
├── web/                                    # 阅读台 + novelx_check
└── projects/<name>/
```

## 快速开始

接入步骤见 [`integrations/dsh-preset-novelx/README.md`](../../../integrations/dsh-preset-novelx/README.md)（官方用户预设目录 `~/.dsh/.agent-presets/novelx/`）。

```bash
cd /path/to/novel-agents
npx @deepseek-ai/dsh web
# 工作区为本仓库根；新开会话；预设选 NovelX。不要用标准模式。
```

阅读台由 Agent 调 `novelx_open_desk`（Node：`web/desk-server.mjs`）；落盘后调 `novelx_check`。也可 `cd web && npm run desk`。

## 主路由

| ID | 职责 |
|----|------|
| `novelx_progress` | 问进度、对照剧情卡 |
| `novelx_canon` | 总纲 / 卷纲 / Bible / 设定 / 剧情卡 |
| `novelx_write` | 按 `pipeline.yaml` 委派章步骤 |

## 章步骤（`novelx_write` 委派）

见 `config/pipeline.yaml` 的 `order`：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator → dialogue_specialist → scene_specialist
→ pacing_reviewer → literary_editor → consistency_auditor
→ foreshadow_tracker → summarizer → plot_acceptor
```

## 工作流

### 新建小说

1. 在 dsh 点名 `projects/<目录>/`（没有则让 `novelx_canon` 建目录）
2. `novelx_canon`：总纲 / 卷纲 / Bible
3. `novelx_check`（scope=setting）通过后再写章

### 写每一章

1. `novelx_progress` 看进度
2. `novelx_write` 按流水线委派；落盘后 `novelx_check`
3. 有 blocker 或审校 P0：先改，不要推进 `next_chapter`
4. 旧格式（`outline.md` / 扁卷纲）先 `novelx_migrate`

### 篇幅

| 配置 | 作用 |
|------|------|
| `config/chapter.yaml` | 长篇：目标 5000–6000；硬门 4500 |
| `config/script.yaml` | 短剧篇幅默认 |

## 添加新 Agent

1. 在预设 `agent.cordis.yml` 加子 agent
2. 添加 `integrations/dsh-preset-novelx/preset/skills/{kebab-name}/SKILL.md`
3. 若进章流水线：更新 `config/pipeline.yaml` 的 `order` 与 `novelx-write`

## 禁止事项

- 不要跳过 consistency_auditor 直接推进 `next_chapter`
- 不要在 Writer 中擅自新增重大设定
- 不要把全部已写正文塞进单次 prompt（用 summaries + 局部 span）
- **不要手改** `projects/<name>/` 下的设定卡、Bible、章稿、剧情卡来「修同步 / 补伤势 / 拆地点」。改预设 Skill 或 `novelx_check` / `novelx_migrate`，再开 NovelX 会话

## 参考

- 预设：`integrations/dsh-preset-novelx/`
- 检查：`web/src/deskCheck.js`
- 阅读台：`web/desk-server.mjs`
