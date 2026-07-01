from __future__ import annotations

from datetime import datetime
from enum import Enum
from typing import Any, Optional, Union

from pydantic import BaseModel, Field


class ChapterStatus(str, Enum):
    PENDING = "pending"
    PLANNING = "planning"
    WRITING = "writing"
    AUDITING = "auditing"
    SUMMARIZING = "summarizing"
    PUBLISHED = "published"
    FAILED = "failed"


class AgentTier(str, Enum):
    MVP = "mvp"
    EXTENDED = "extended"


class AgentLayer(str, Enum):
    ORCHESTRATION = "orchestration"
    WORLD = "world"
    STORY = "story"
    PRODUCTION = "production"
    QA = "qa"
    MEMORY = "memory"


class ActivationRule(BaseModel):
    condition: str
    reason: str
    threshold: Optional[Union[float, int]] = None
    tags: Optional[list[str]] = None


class AgentDefinition(BaseModel):
    id: str
    name: str
    layer: AgentLayer
    tier: AgentTier
    default_active: bool
    description: str
    depends_on: list[str] = Field(default_factory=list)
    activation: list[ActivationRule] = Field(default_factory=list)


class ChapterOutline(BaseModel):
    number: int
    title: str = ""
    pov: str = ""
    time_location: str = ""
    goal: str = ""
    conflict: str = ""
    emotion_curve: str = ""
    key_events: list[str] = Field(default_factory=list)
    characters: list[str] = Field(default_factory=list)
    scene_tags: list[str] = Field(default_factory=list)
    cliffhanger: str = ""
    lore_queries: list[str] = Field(default_factory=list)
    lore_updates: list[str] = Field(default_factory=list)


class AuditIssue(BaseModel):
    type: str
    description: str
    severity: str = "BLOCKER"
    suggested_fix: str = ""


class AuditResult(BaseModel):
    passed: bool
    issues: list[AuditIssue] = Field(default_factory=list)


class ChapterSummary(BaseModel):
    number: int
    event_summary: str = ""
    relationship_changes: str = ""
    new_facts: list[str] = Field(default_factory=list)
    foreshadow_updates: list[str] = Field(default_factory=list)


class ChapterRecord(BaseModel):
    number: int
    status: ChapterStatus = ChapterStatus.PENDING
    outline: Optional[ChapterOutline] = None
    draft: str = ""
    audit: Optional[AuditResult] = None
    summary: Optional[ChapterSummary] = None
    word_count: int = 0
    dialogue_ratio: float = 0.0
    revision_count: int = 0


class ProjectState(BaseModel):
    name: str
    genre: str = ""
    target_chapters: int = 50
    has_bible: bool = False
    bible_revision_count: int = 0
    has_master_outline: bool = False
    has_arc_outline: bool = False
    current_arc: int = 1
    chapters_in_current_arc: int = 0
    entity_count: int = 0
    active_foreshadows: int = 0
    chapters: list[ChapterRecord] = Field(default_factory=list)
    active_agents: list[str] = Field(default_factory=list)
    audit_fail_history: list[bool] = Field(default_factory=list)
    created_at: datetime = Field(default_factory=datetime.now)
    metadata: dict[str, Any] = Field(default_factory=dict)

    @property
    def published_count(self) -> int:
        return sum(1 for c in self.chapters if c.status == ChapterStatus.PUBLISHED)

    def get_chapter(self, number: int) -> ChapterRecord | None:
        for ch in self.chapters:
            if ch.number == number:
                return ch
        return None

    def ensure_chapter(self, number: int) -> ChapterRecord:
        existing = self.get_chapter(number)
        if existing:
            return existing
        record = ChapterRecord(number=number)
        self.chapters.append(record)
        return record


class ActivationRecommendation(BaseModel):
    agent_id: str
    agent_name: str
    reason: str
    condition: str
    auto_activate: bool = True


class PipelineStep(BaseModel):
    agent_id: str
    agent_name: str
    status: str = "pending"
    output_preview: str = ""


class PipelineResult(BaseModel):
    chapter_number: int
    steps: list[PipelineStep] = Field(default_factory=list)
    activated_agents: list[ActivationRecommendation] = Field(default_factory=list)
    final_status: ChapterStatus = ChapterStatus.PENDING
    message: str = ""
