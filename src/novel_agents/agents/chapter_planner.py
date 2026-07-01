from __future__ import annotations

from typing import Any

from novel_agents.agents.base import BaseAgent
from novel_agents.models import ChapterOutline, ProjectState


class ChapterPlannerAgent(BaseAgent):
    agent_id = "chapter_planner"
    name = "Chapter Planner"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter_num = context["chapter_number"]
        prev_summary = context.get("prev_summary", "")

        outline = ChapterOutline(
            number=chapter_num,
            title=f"第{chapter_num}章",
            pov=context.get("pov", "主角"),
            time_location=context.get("time_location", "待定"),
            goal=context.get("goal", "推进主线冲突"),
            conflict=context.get("conflict", "内外矛盾交织"),
            emotion_curve="低→高→悬念",
            key_events=[
                f"事件 A：第{chapter_num}章开场",
                f"事件 B：冲突升级",
                f"事件 C：章末钩子",
            ],
            characters=context.get("characters", ["主角"]),
            scene_tags=context.get("scene_tags", []),
            cliffhanger=f"第{chapter_num}章结尾留下未解悬念",
            lore_queries=[f"查询主角当前状态（基于：{prev_summary[:50]}...）"]
            if prev_summary
            else [],
        )

        chapter = state.ensure_chapter(chapter_num)
        chapter.outline = outline
        chapter.status = context.get("_next_status", chapter.status)

        return {"outline": outline, "outline_text": self._format_outline(outline)}

    @staticmethod
    def _format_outline(o: ChapterOutline) -> str:
        events = "\n".join(f"  - {e}" for e in o.key_events)
        return f"""## {o.title}
- POV：{o.pov}
- 时间/地点：{o.time_location}
- 本章目标：{o.goal}
- 冲突：{o.conflict}
- 情绪曲线：{o.emotion_curve}
- 关键事件：
{events}
- 出场人物：{', '.join(o.characters)}
- 场景标签：{', '.join(o.scene_tags) or '无'}
- 章末钩子：{o.cliffhanger}
"""
