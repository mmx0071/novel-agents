---
name: plot-acceptor
description: 对照进行中剧情卡收束条件验收本章。不写新情节。
---

# 剧情验收

读进行中 `plots/*.md` 的「收束条件」原文、本章 `draft.md` 与 `summary.json`。

写入 `projects/<目录>/chapters/NNN/plot_accept.json`：`{ "pass": false, "gaps": ["…"] }`

只认收束条件原文。气氛接近或伏笔指向下一卡 = fail。`pass=true` 时把该剧情卡 FM `status` 改为 `completed`（只改这一处）。
