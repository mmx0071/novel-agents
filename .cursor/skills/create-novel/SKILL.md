---
name: create-novel
description: >-
  超长篇多 Agent 小说创作工作流。Use when the user asks to create a novel,
  start a novel project, run chapter pipeline, activate writing agents, or
  work with the novel-agents system. 项目会自动判断是否需要激活扩展 Agent。
---

# Create Novel — 多 Agent 小说创作

本 Skill 指导 Agent 使用 `novel-agents` 项目进行超长篇协同创作。

## 何时加载

- 用户要**创建新小说**、**写下一章**、**查看 Agent 状态**
- 用户提到多 Agent 小说、章纲、一致性审计、伏笔追踪
- 用户在 `novel-agents` 项目目录中工作

## 项目结构

```
novel-agents/
├── config/agents.yaml      # 全部 Agent 定义与激活条件
├── src/novel_agents/       # 核心代码
├── projects/<name>/        # 每部小说的工作区
│   ├── meta.json
│   ├── state.json
│   ├── artifacts/          # Bible、总纲、卷纲
│   └── chapters/NNN/       # 章纲、正文、摘要
└── scripts/demo.py
```

## 快速开始

```bash
cd /path/to/novel-agents
pip install -e .

# 创建项目（可选 --setup 立即生成 Bible/总纲/卷纲）
novel init my-novel --genre 玄幻 --chapters 100 --setup

# 运行单章流水线
novel run my-novel 1
novel run my-novel 2 --characters 主角,甲,乙,丙,丁
novel run my-novel 3 --scene-tags battle,climax

# 查看状态与 Agent 激活建议
novel status my-novel
novel activate my-novel
novel agents
```

## Agent 分层

### MVP（默认启用）

| ID | 职责 |
|----|------|
| orchestrator | 调度流水线，不写正文 |
| chapter_planner | 输出章纲 |
| writer | 写正文初稿 |
| consistency_auditor | 一致性审计 |
| summarizer | 生成摘要入库 |

### 扩展（按需自动激活）

| ID | 典型触发条件 |
|----|-------------|
| world_architect | 尚无 Bible |
| lore_librarian | 实体 > 20 或章节 > 5 |
| master_planner | 尚无总纲 / 章节 > 30 |
| arc_planner | 尚无卷纲 / 本卷章节 > 8 |
| dialogue_specialist | 对话占比 > 40% 或同场人物 > 4 |
| scene_specialist | 章纲含 battle/chase/action/climax |
| foreshadow_tracker | 活跃伏笔 > 5 或章节 > 10 |
| pacing_reviewer | 字数 > 5000 或质检 FAIL 率 > 20% |
| literary_editor | 章节 > 3（风格锚点建立后） |

## 自动激活逻辑（核心）

**每次运行章节流水线前**，Orchestrator 会：

1. 读取 `config/agents.yaml` 中的 `activation` 规则
2. 对照 `state.json`（章节数、实体数、伏笔数、审计历史等）
3. 对照当前章纲/正文指标（对话占比、场景标签、字数）
4. 将满足条件的扩展 Agent 加入 `active_agents`
5. 按固定顺序组装 pipeline 并执行

**Agent 执行顺序**（仅运行已激活的）：

```
chapter_planner → writer → dialogue_specialist → scene_specialist
→ consistency_auditor → foreshadow_tracker → pacing_reviewer
→ literary_editor → summarizer
```

## 工作流指引

### 1. 新建小说

1. 确认题材、目标章节数、项目名称
2. 运行 `novel init <name> --genre <题材> --chapters <N> --setup`
3. 检查 `projects/<name>/artifacts/` 是否生成 Bible/总纲/卷纲
4. 向用户汇报已激活 Agent 列表

### 2. 写每一章

1. 先 `novel status <name>` 看 pending_activation
2. 根据剧情需要传入 `--scene-tags` / `--characters`
3. 运行 `novel run <name> <chapter>`
4. 若 consistency_auditor **FAIL**：修复后重跑，不要跳过
5. 定稿后摘要自动写入 `chapters/NNN/summary.json`

### 3. 何时手动介入

| 节点 | 动作 |
|------|------|
| Bible / 总纲 / 卷纲 | 人类审批 `artifacts/` |
| 一致性 FAIL | 修正正文或更新 Lore |
| pending_activation 出现 | 确认是否 `novel activate` |

### 4. 接入真实 LLM

当前 MVP 使用占位正文。接入 LLM 时修改：

- `src/novel_agents/agents/writer.py` — 正文生成
- `src/novel_agents/agents/chapter_planner.py` — 章纲生成
- `src/novel_agents/agents/extended.py` — 扩展 Agent

保持 `BaseAgent.run(state, context) -> dict` 接口不变。

## 添加新 Agent

1. 在 `config/agents.yaml` 注册 Agent 与 `activation` 规则
2. 在 `src/novel_agents/agents/` 实现 `BaseAgent` 子类
3. 在 `agents/__init__.py` 的 `build_agent_registry()` 注册
4. 若需插入 pipeline，更新 `registry.py` 的 `pipeline_for_chapter()` 顺序
5. 运行 `novel agents` 验证注册

## 判断是否需要增加 Agent（决策树）

```
项目刚创建？
  └─ 是 → 激活 world_architect, master_planner, arc_planner

已写 ≥3 章？
  └─ 是 → 激活 literary_editor

对话密集 / 多人同场？
  └─ 是 → 激活 dialogue_specialist

战斗 / 追逐 / 高潮章？
  └─ 是 → 激活 scene_specialist

章节 ≥10 或伏笔 ≥5？
  └─ 是 → 激活 foreshadow_tracker

实体 ≥20 或章节 ≥5？
  └─ 是 → 激活 lore_librarian

质检 FAIL 率 >20%？
  └─ 是 → 激活 pacing_reviewer

章节 ≥30？
  └─ 是 → 激活 master_planner（复盘总纲）
```

## 禁止事项

- 不要跳过 consistency_auditor 直接发布
- 不要在 Writer 中擅自新增重大设定（minor 事实用 `[NEW_FACT]` 标注）
- 不要手动改 `state.json` 的 `active_agents` 除非用户明确要求
- 不要把全部已写正文塞进单次 prompt（用 summaries + RAG）

## 参考

- Agent 完整定义：`config/agents.yaml`
- 激活评估：`src/novel_agents/registry.py`
- 流水线调度：`src/novel_agents/orchestrator.py`
