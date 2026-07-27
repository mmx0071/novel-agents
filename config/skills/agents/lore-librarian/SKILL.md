---
name: lore-librarian
description: >-
  Lore 知识库实现说明（确定性）。章纲后 query、摘要后 assert；流水线不调用本角色 LLM。
---

# Lore Librarian（确定性实现说明）

本角色**不是**独立 LLM Agent。章流水线中：

| 模式 | 时机 | 实现 |
|------|------|------|
| **query** | 步骤 `lore_librarian`（`kind: lore_query`） | 确定性 `lore_query` → 切片注入 Writer CanonContext |
| **assert** | Summarizer 步骤末尾自动 | `lore_assert_from_summary`；不单独占一步 |

因此本 SKILL 正文**不会**作为 LLM system prompt 消费；保留是为了注册表/文档/Web Skills 可读。改行为请改 `novelx-pipeline` 的 lore 模块，勿指望改本文驱动模型。

## 知识管理原则（实现应对齐）

1. **单一事实源**：已入库事实优先；冲突跳过并记入报告，勿悄悄覆盖
2. **原子事实**：一条一事（谁、何时、何状态）
3. **可验证**：能被正文/摘要支撑；传闻标注角色声称
4. **查询优先于塞全文**：切片短、准、够用
5. **状态优于百科**：位置、伤势、持有物、已知信息 > 静态生平

## 与实体卡

- query：除 Lore 切片外尽量附带相关实体卡摘要
- assert：原子事实入库后尝试回写实体卡「核心事件 / 当前状态」
- 详描以实体卡为准；Lore 保持章后可验证事实流

## 下游约束

- Writer 不得与已注入切片矛盾
- 本步不改正文；矛盾通过切片/冲突说明暴露给下游
