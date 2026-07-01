from __future__ import annotations

import re
from typing import Any

from novel_agents.agents.base import BaseAgent
from novel_agents.models import ProjectState


class WriterAgent(BaseAgent):
    agent_id = "writer"
    name = "Writer"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter_num = context["chapter_number"]
        outline_text = context.get("outline_text", "")
        chapter = state.ensure_chapter(chapter_num)
        outline = chapter.outline

        title = outline.title if outline else f"第{chapter_num}章"
        pov = outline.pov if outline else "主角"
        conflict = outline.conflict if outline else "未知冲突"
        cliffhanger = outline.cliffhanger if outline else "悬念待续"

        draft = f"""# {title}

{outline_text}

---

{pov}站在风口，心里清楚——{conflict}。

这一章，故事继续向前。风从远处吹来，带着说不清的意味。

"你确定要走这一步？"身旁有人低声问。

{pov}没有立刻回答。有些决定，一旦做出，就再也回不了头。

（此处为 MVP demo 正文占位，接入 LLM 后将按章纲生成 3000–5000 字正文。）

---

{cliffhanger}
"""

        chapter.draft = draft
        chapter.word_count = len(draft)
        chapter.dialogue_ratio = self._estimate_dialogue_ratio(draft)

        return {"draft": draft, "word_count": chapter.word_count}

    @staticmethod
    def _estimate_dialogue_ratio(text: str) -> float:
        dialogue_chars = sum(len(m.group()) for m in re.finditer(r'"[^"]*"', text))
        total = max(len(text), 1)
        return round(dialogue_chars / total, 3)
