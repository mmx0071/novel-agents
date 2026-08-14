---
name: novelx-canon
description: 设计或修改总纲、卷纲、Bible、剧情卡、人物/物品/地点卡。按 content-formats 落盘。
---

# 设定

加载 `content-formats` 与 `naming-rules`。只改 `projects/<目录>/` 下设定文件，不写章正文。新实体名先对两层禁名。

- 总纲 / 卷纲 / Bible：按固定 H2，题材跟项目已有世界观，不套玄幻或爽文模板。
- 剧情卡：一卷同时只推一张 `in_progress`；新卡默认 `planned`。
- 实体卡：规范名与已有卡对齐；气氛道具不要建物品卡。
- 作品目录改名 / 存档：用 `bash`（Windows 用 `pwsh`）整目录操作，不要逐文件复制，也不要让用户去文件管理器里改。
  - 改名：`mv projects/<旧> projects/<新>`（目标必须还不存在）
  - 复制存档：`cp -R projects/<源> projects/<副本>`
  - 只动 `projects/` 下的作品目录；禁止 `config/`、`web/`
  - 禁止 `rm -rf`，除非用户明确确认要删哪一个目录
  - 改完后后续路径用新目录名；不要编书名
- 打开旧项目若缺 `outline.json` 或仍有扁卷纲 `artifacts/arc_outline.md`，先调 `novelx_migrate`。
- 写完先调 `novelx_check`（scope=setting）。有 blocker 先改文件。再用中文说明改了哪些路径，不要把整卡贴进聊天。调用 `novelx_open_desk` 让阅读台切到对应页（总纲 `master`，卷/剧情 `volume`/`arcs`，人物 `characters`）。
