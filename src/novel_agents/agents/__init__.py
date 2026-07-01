from __future__ import annotations

from novel_agents.agents.base import BaseAgent
from novel_agents.agents.chapter_planner import ChapterPlannerAgent
from novel_agents.agents.consistency_auditor import ConsistencyAuditorAgent
from novel_agents.agents.extended import (
    ArcPlannerAgent,
    DialogueSpecialistAgent,
    ForeshadowTrackerAgent,
    LiteraryEditorAgent,
    LoreLibrarianAgent,
    MasterPlannerAgent,
    PacingReviewerAgent,
    SceneSpecialistAgent,
    WorldArchitectAgent,
)
from novel_agents.agents.summarizer import SummarizerAgent
from novel_agents.agents.writer import WriterAgent


def build_agent_registry() -> dict[str, BaseAgent]:
    agents: list[BaseAgent] = [
        ChapterPlannerAgent(),
        WriterAgent(),
        ConsistencyAuditorAgent(),
        SummarizerAgent(),
        WorldArchitectAgent(),
        LoreLibrarianAgent(),
        MasterPlannerAgent(),
        ArcPlannerAgent(),
        DialogueSpecialistAgent(),
        SceneSpecialistAgent(),
        ForeshadowTrackerAgent(),
        PacingReviewerAgent(),
        LiteraryEditorAgent(),
    ]
    return {a.agent_id: a for a in agents}


AGENT_INSTANCES = build_agent_registry()
