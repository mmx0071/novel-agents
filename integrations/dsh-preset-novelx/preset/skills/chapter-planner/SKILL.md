---
name: chapter-planner
description: 输出单章 outline.json。一章一事，不写正文。
---

# 章纲

加载 `content-formats` 与 `naming-rules`。读总纲/卷纲、进行中剧情卡、前章摘要与钩子。`characters` 只用已有规范名或未在禁名清单中的新名。

写入 `projects/<目录>/chapters/NNN/outline.json`（只 JSON）。

必填：`title, pov, time_location, goal, conflict, emotion_curve, plot_includes, plot_defers, key_events, characters, items, locations, scene_tags, cliffhanger, lore_queries`

- 正文预算 5000–6000 字：`plot_includes` 短、`key_events` 2–4 条
- 有进行中剧情卡时 `plot_defers` 至少 1 条（默认本章不兑现整卡收束）
- 只覆盖本章切片，禁止一次写完整条弧
- 题材跟项目已有设定，不套模板
- 写完调 `novelx_check`（scope=outline）
