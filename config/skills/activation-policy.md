# Agent 激活与章节流水线策略

Harness 范式：`agents.yaml` 中的 `activation` 条件产生**建议**；本章实际步骤由

`resolve_pipeline_agents(active_agents ∪ pipeline.mvp ∪ suggestions)`

按 `config/pipeline.yaml` 的 `order` 排序得到。建议会进入**本章流水线**，但不会自动写回 `state.active_agents`。

> 说明：当前实现**没有**独立 Orchestrator LLM 输出 `pipeline` JSON 再执行；顺序以 `pipeline.yaml` 为准。

## MVP 核心 Agent（`pipeline.yaml` → `mvp`，每章通常包含）

| ID | 职责 |
|----|------|
| chapter_planner | 章纲 |
| lore_librarian | Lore query（摘要后 assert 挂在 summarizer） |
| writer | 正文 |
| consistency_auditor | 一致性审计（FAIL 时阻断发布） |
| pacing_reviewer | 节奏审查 |
| summarizer | 摘要入库 |
| plot_acceptor | 对照剧情卡收束条件验收；发布后才 completed |

## 推荐执行顺序

以 `config/pipeline.yaml` 的 `order` 为准，默认：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator（hook，亦可出现在顺序表中）
→ dialogue_specialist → scene_specialist
→ consistency_auditor → foreshadow_tracker
→ pacing_reviewer → literary_editor → summarizer → plot_acceptor
```

原则：**先生产、再专改、再质检、再摘要，最后剧情验收**；**发布收尾只在完整章流水线末尾执行一次**（单步 SPAWN 子 Agent 不推进 `next_chapter` / 剧情卡）。

## Handler 注册（`pipeline.yaml` → `handlers`）

章步骤按 **kind** 分发，不再按 Agent 名写死巨型 `match`：

| kind | 用途 |
|------|------|
| `chapter_planner` / `lore_query` / `writer` / `nomenclature` | 生产与名词 |
| `specialist_rewrite` | 专改正文（`dialogue_specialist` / `scene_specialist` / `literary_editor`…）；`focus` 写在 YAML |
| `consistency` / `pacing` / `foreshadow` / `summarizer` / `plot_accept` | 质检与收束 |

新增「只改文笔、不改情节」类 Agent：在 `order` + `handlers` 声明 `kind: specialist_rewrite` 与 `focus` 即可，**不必改 Rust**。其它 kind 仍需在 `novelx-pipeline` 实现。

`handlers` 允许局部覆盖：未写出的 agent 会与 embedded 默认表合并，避免只改一条 focus 导致其它步骤静默跳过。

## 扩展 Agent 激活建议

| Agent | 何时考虑激活 |
|-------|-------------|
| world_architect | 尚无 Bible |
| nomenclature_curator | 尚无名词表 / 有 Bible / 章纲有新实体 |
| dialogue_specialist | 对话密集 |
| scene_specialist | 场景/动作高潮 |
| foreshadow_tracker | 未收束伏笔 |
| literary_editor | 风格润色需求 |
| master_planner / arc_planner | 尚无总纲/卷纲 |

## 人工门控

审校后的选项（局部修订 / 接受 / 队列下一步等）见 `config/gates.yaml`：
每项用 `tool` + `args` 模板（`{{project}}` / `{{chapter}}` …）或 `resolve: skip_volume`，
不再在 Rust 里按 action 枚举写死工具参数。`steer_run.choice` 用 `revise` / `accept`。
