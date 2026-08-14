---
name: novelx-desk
description: 汇报作品进度。只读 projects/，不写盘，不把整章正文贴进聊天。
---

# 进度

1. 先确认 `projects/<目录>/`（用户没说就列 `projects/` 下的目录名问清楚）。
2. 读 `state.json`、`plots/*.md`、最近有正文的 `chapters/NNN/draft.md` 开头与章末（不要通读全书）。
3. 用中文汇报：写到第几章、进行中剧情卡、下一章号。不要编造未读到的情节。
4. 用户要看整本或阅读台：调用 `novelx_open_desk`（带上 `project`，有章号就带 `chapter`）。不要把正文贴进聊天。
