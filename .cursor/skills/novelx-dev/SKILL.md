---
name: novelx-dev
description: >-
  NovelX / novel-agents 项目开发规范。Use when editing Rust crates, Web UI,
  config/skills, harness rules, schemas, tests, or any product code in this repo.
  Prefer loading this before implementing features that touch prompts, defaults,
  fixtures, or reader/tool UX.
---

# NovelX 项目开发规范

本 Skill 约束 **产品代码与框架配置** 的通用性，避免把某一本样例小说写进系统内核。

## 题材 / 作品中立（硬规则）

1. **禁止在框架代码中写死特定小说**  
   不得在 `crates/**`、`web/**`、`config/**`（Agent skill / YAML / 契约文档）中嵌入某一作品的专名、角色名、势力名、地名、梗概或卷名（例如某样例项目里的书名、主角名、机构名）。
2. **样例只存在于 `projects/<name>/`**  
   具体小说内容、实体卡、剧情卡、章纲仅作为用户数据落在 `projects/`。框架通过路径参数 / `project` 参数读写，不假设当前打开的是哪一本。
3. **测试与演示用通用占位**  
   单测、fixtures、UI 文案示例使用中性名称：`sample-novel`、`demo`、`主角`、`甲`、`样例小说` 等。需要情节语义时，用抽象句（「抵达落点」「目击异象」），不要抄某本现成小说的句子。
4. **Prompt / Skill 保持题材中立**  
   Agent SKILL、studio 提示、工具描述不得预设玄幻/修仙/科幻神话等模板为默认世界观；由用户 brief 与项目 Bible 决定。
5. **迁移与集成测试勿绑死磁盘上的某本样例书**  
   不要写死 `projects/某书名`。优先在临时目录构造最小合法骨架；若可选地扫描 `projects/`，须按目录枚举且失败时 skip，而不是硬编码书名。

## 允许与例外

| 允许 | 不允许 |
|------|--------|
| `projects/` 下任意用户/样例小说全文 | 把该书专名写进 Rust/前端/默认 config |
| 文档中用「例如某项目」说明用法（不写死实现） | 默认 genre、默认角色、默认势力写进 registry |
| 抽象占位符用于单测 | 从真实样例书复制专名进 assert / fixture |

发现违规时：改为通用占位，或把内容下沉到 `projects/`，并补一条回归测试证明框架不依赖该书名。

## 与其它规范的关系

- 阅读区格式契约见 `config/schemas/README.md`（机器以 `novelx_pipeline::schemas` 为准）。
- 创作工作流见 `.cursor/skills/create-novel/SKILL.md`（面向「写小说」；本 Skill 面向「改代码」）。
