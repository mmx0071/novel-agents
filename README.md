# novel-agents

超长篇小说的多 Agent 协同创作系统 Demo。

## 特性

- **MVP 最小集**：Orchestrator + Chapter Planner + Writer + Consistency Auditor + Summarizer
- **扩展 Agent 池**：9 个按需激活的 Agent（世界观、总纲、对话、场景、伏笔等）
- **自动激活**：根据项目状态（章节数、实体数、对话占比、场景标签等）判断是否需要新 Agent
- **Cursor Skill**：`.cursor/skills/create-novel/SKILL.md` 指导 AI 使用本系统

## 安装

```bash
pip install -e .
```

## 快速 Demo

```bash
python scripts/demo.py
```

或手动：

```bash
novel init my-novel --genre 玄幻 --chapters 100 --setup
novel run my-novel 1
novel status my-novel
novel agents
```

## CLI 命令

| 命令 | 说明 |
|------|------|
| `novel init <name>` | 创建小说项目 |
| `novel run <name> <chapter>` | 运行单章流水线 |
| `novel status <name>` | 查看项目与 Agent 状态 |
| `novel activate <name>` | 检查并激活所需 Agent |
| `novel agents` | 列出全部 Agent 及激活条件 |

## 架构

```
Orchestrator
  ├─ AgentRegistry（读取 agents.yaml，评估激活条件）
  ├─ Pipeline（按序执行已激活 Agent）
  └─ ProjectState（state.json 持久化）
```

## 接入 LLM

当前为占位实现，便于理解流水线。接入真实模型时修改 `src/novel_agents/agents/` 下各 Agent 的 `run()` 方法。
