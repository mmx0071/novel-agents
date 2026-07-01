from __future__ import annotations

from typing import Any

from novel_agents.agents.base import BaseAgent
from novel_agents.models import AuditIssue, AuditResult, ProjectState


class ConsistencyAuditorAgent(BaseAgent):
    agent_id = "consistency_auditor"
    name = "Consistency Auditor"

    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        chapter_num = context["chapter_number"]
        chapter = state.ensure_chapter(chapter_num)
        draft = chapter.draft
        issues: list[AuditIssue] = []

        if "[CONTRADICTION]" in draft:
            issues.append(
                AuditIssue(
                    type="LORE",
                    description="正文包含标记的设定冲突",
                    suggested_fix="对照 Lore 知识库修正矛盾段落",
                )
            )

        if chapter.outline and chapter.outline.characters:
            for char in chapter.outline.characters:
                if char in state.metadata.get("dead_characters", []):
                    issues.append(
                        AuditIssue(
                            type="CHARACTER",
                            description=f"已死亡角色「{char}」在本章出场",
                            suggested_fix=f"移除 {char} 或改为回忆/闪回并标注",
                        )
                    )

        passed = len(issues) == 0
        result = AuditResult(passed=passed, issues=issues)
        chapter.audit = result
        state.audit_fail_history.append(passed)

        return {"audit": result, "passed": passed}
