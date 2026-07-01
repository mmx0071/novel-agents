from __future__ import annotations

from typing import Any

from novel_agents.agents.base import BaseAgent
from novel_agents.models import ProjectState


class WorldArchitectAgent(BaseAgent):
    agent_id = "world_architect"
    name = "World Architect"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        bible = f"""# {state.name} · 世界观 Bible

## 核心规则
- 题材：{state.genre or '待定'}
- 硬约束：（demo 占位，接入 LLM 后生成完整 Bible）

## 势力与地理
- （待补充）

## 禁忌清单
- 不可违反的设定边界
"""
        state.has_bible = True
        state.bible_revision_count = 0
        return {"bible": bible}


class LoreLibrarianAgent(BaseAgent):
    agent_id = "lore_librarian"
    name = "Lore Librarian"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter_num = context.get("chapter_number")
        queries = context.get("lore_queries", [])
        return {
            "lore_slice": {
                "chapter": chapter_num,
                "queries": queries,
                "entities": state.entity_count,
                "note": "（demo）按需检索知识库切片",
            }
        }


class MasterPlannerAgent(BaseAgent):
    agent_id = "master_planner"
    name = "Master Planner"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        outline = f"""# {state.name} · 全书总纲

- 目标章节：{state.target_chapters}
- 三幕结构：起（1–{state.target_chapters // 4}）→ 承（…）→ 合（…）
- 主线：（demo 占位）
"""
        state.has_master_outline = True
        return {"master_outline": outline}


class ArcPlannerAgent(BaseAgent):
    agent_id = "arc_planner"
    name = "Arc Planner"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        arc = state.current_arc
        outline = f"""# 第 {arc} 卷 · 卷纲

- 卷目标：本卷冲突升级
- 预估章节：8–15 章
- 伏笔计划：（demo 占位）
"""
        state.has_arc_outline = True
        state.chapters_in_current_arc = 0
        return {"arc_outline": outline, "arc": arc}


class DialogueSpecialistAgent(BaseAgent):
    agent_id = "dialogue_specialist"
    name = "Dialogue Specialist"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter = state.ensure_chapter(context["chapter_number"])
        draft = chapter.draft
        enhanced = draft.replace(
            '"你确定要走这一步？"',
            '"你确定要走这一步？"声音压得很低，像是在确认，又像是在挽留。',
        )
        chapter.draft = enhanced
        return {"draft": enhanced, "note": "对话润色完成（demo）"}


class SceneSpecialistAgent(BaseAgent):
    agent_id = "scene_specialist"
    name = "Scene Specialist"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter = state.ensure_chapter(context["chapter_number"])
        draft = chapter.draft
        enhanced = draft.replace(
            "风从远处吹来，带着说不清的意味。",
            "风从远处吹来，带着铁锈与潮气混杂的味道，城墙下的阴影被拉得很长。",
        )
        chapter.draft = enhanced
        return {"draft": enhanced, "note": "场景描写增强（demo）"}


class ForeshadowTrackerAgent(BaseAgent):
    agent_id = "foreshadow_tracker"
    name = "Foreshadow Tracker"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter_num = context["chapter_number"]
        warnings = []
        if state.active_foreshadows > 10:
            warnings.append(f"活跃伏笔 {state.active_foreshadows} 条，建议优先安排回收")
        return {
            "foreshadow_report": {
                "chapter": chapter_num,
                "active_count": state.active_foreshadows,
                "warnings": warnings,
            }
        }


class PacingReviewerAgent(BaseAgent):
    agent_id = "pacing_reviewer"
    name = "Pacing Reviewer"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter = state.ensure_chapter(context["chapter_number"])
        suggestions = []
        if chapter.word_count > 5000:
            suggestions.append("篇幅偏长，建议拆分或删减过渡段")
        if chapter.dialogue_ratio < 0.1:
            suggestions.append("对话偏少，可考虑增加人物互动")
        return {"pacing_suggestions": suggestions, "passed": len(suggestions) == 0}


class LiteraryEditorAgent(BaseAgent):
    agent_id = "literary_editor"
    name = "Literary Editor"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter = state.ensure_chapter(context["chapter_number"])
        draft = chapter.draft
        edited = draft.replace("有些决定", "有些选择")
        chapter.draft = edited
        return {"draft": edited, "note": "文学润色完成（demo）"}
