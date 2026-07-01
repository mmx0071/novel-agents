from __future__ import annotations

from typing import Any

from novel_agents.agents.base import BaseAgent
from novel_agents.models import ChapterStatus, ChapterSummary, ProjectState


class SummarizerAgent(BaseAgent):
    agent_id = "summarizer"
    name = "Summarizer"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter_num = context["chapter_number"]
        chapter = state.ensure_chapter(chapter_num)
        outline = chapter.outline

        title = outline.title if outline else f"第{chapter_num}章"
        events = outline.key_events if outline else [f"第{chapter_num}章事件"]

        summary = ChapterSummary(
            number=chapter_num,
            event_summary=f"{title}：" + "；".join(events[:3]),
            relationship_changes="（demo）人物关系暂无重大变化",
            new_facts=[f"[NEW_FACT] 第{chapter_num}章新增事实占位"],
            foreshadow_updates=[f"伏笔 #{chapter_num} 已登记"],
        )

        chapter.summary = summary
        chapter.status = ChapterStatus.PUBLISHED
        state.chapters_in_current_arc += 1
        state.active_foreshadows += 1
        state.entity_count += len(outline.characters) if outline else 1

        return {"summary": summary}
