---
name: novelx-write
description: 写一章的编排。按固定顺序委派子 agent，自己不写正文。
---

# 写章编排

加载 `content-formats`。确认 `projects/<目录>/` 与章号（默认 `state.json` 的 `next_chapter`）。开写前调用 `novelx_open_desk`（project + chapter），让阅读台跟上。

按这个顺序**前台**委派（一步结束再下一步；用户停止则不要开下一步）：

1. `chapter_planner`
2. `lore_librarian`（读设定，给后续步骤准备要点；无设定可跳过）
3. `writer`
4. 扩展（仅当章纲 `scene_tags` 或正文明显需要）：`nomenclature_curator` / `dialogue_specialist` / `scene_specialist` / `literary_editor`
5. `pacing_reviewer`
6. `consistency_auditor`
7. `foreshadow_tracker`（有未收伏笔或已有 `foreshadow` 文件时）
8. `summarizer`
9. `plot_acceptor`

一致性有 P0：停下来告诉用户，问是否再调 `writer` 修，不要擅自 finalize。

全部完成后：先调 `novelx_check`（project + chapter，scope=chapter）。有 blocker：不要改 `next_chapter`，把 blocker 告诉用户。无 blocker 且审校无 P0：把 `state.json` 的 `next_chapter` 设为本章+1（保留其它字段）。再调 `novelx_open_desk`（tab=draft）。不要调用 Host `continue_writing`。向用户用两三句总结，不要贴正文。
