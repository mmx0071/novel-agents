from __future__ import annotations

from pathlib import Path
from typing import Any

import yaml

from novel_agents.models import (
    ActivationRecommendation,
    AgentDefinition,
    AgentLayer,
    AgentTier,
    ActivationRule,
    ChapterRecord,
    ProjectState,
)


class AgentRegistry:
    """Load agent definitions and evaluate activation conditions."""

    def __init__(self, config_path: Path | None = None):
        if config_path is None:
            config_path = Path(__file__).resolve().parents[2] / "config" / "agents.yaml"
        self.config_path = config_path
        self.agents: dict[str, AgentDefinition] = {}
        self._load()

    def _load(self) -> None:
        raw = yaml.safe_load(self.config_path.read_text(encoding="utf-8"))
        for agent_id, spec in raw["agents"].items():
            self.agents[agent_id] = AgentDefinition(
                id=agent_id,
                name=spec["name"],
                layer=AgentLayer(spec["layer"]),
                tier=AgentTier(spec["tier"]),
                default_active=spec["default_active"],
                description=spec["description"],
                depends_on=spec.get("depends_on", []),
                activation=[
                    ActivationRule(**rule) for rule in spec.get("activation", [])
                ],
            )

    def mvp_agents(self) -> list[AgentDefinition]:
        return [a for a in self.agents.values() if a.tier == AgentTier.MVP]

    def extended_agents(self) -> list[AgentDefinition]:
        return [a for a in self.agents.values() if a.tier == AgentTier.EXTENDED]

    def get(self, agent_id: str) -> AgentDefinition | None:
        return self.agents.get(agent_id)

    def initial_active_ids(self) -> list[str]:
        return [a.id for a in self.agents.values() if a.default_active]

    def evaluate_activation(
        self,
        state: ProjectState,
        chapter: ChapterRecord | None = None,
    ) -> list[ActivationRecommendation]:
        """Return agents that should be activated based on current project state."""
        recommendations: list[ActivationRecommendation] = []
        already_active = set(state.active_agents)

        for agent in self.extended_agents():
            if agent.id in already_active:
                continue
            for rule in agent.activation:
                if self._check_condition(rule, state, chapter):
                    recommendations.append(
                        ActivationRecommendation(
                            agent_id=agent.id,
                            agent_name=agent.name,
                            reason=rule.reason,
                            condition=rule.condition,
                        )
                    )
                    break

        return recommendations

    def _check_condition(
        self,
        rule: ActivationRule,
        state: ProjectState,
        chapter: ChapterRecord | None,
    ) -> bool:
        cond = rule.condition
        threshold = rule.threshold

        checks: dict[str, Any] = {
            "no_bible": lambda: not state.has_bible,
            "bible_stale": lambda: state.bible_revision_count >= 3,
            "no_master_outline": lambda: not state.has_master_outline,
            "no_arc_outline": lambda: not state.has_arc_outline,
            "entity_count_gt": lambda: state.entity_count > (threshold or 0),
            "chapter_count_gt": lambda: state.published_count > (threshold or 0),
            "chapters_in_arc_gt": lambda: state.chapters_in_current_arc
            > (threshold or 0),
            "active_foreshadows_gt": lambda: state.active_foreshadows
            > (threshold or 0),
            "audit_fail_rate_gt": lambda: self._audit_fail_rate(state)
            > (threshold or 0),
            "always_after_auditor": lambda: False,  # handled in pipeline, not pre-activation
        }

        if cond in checks:
            return bool(checks[cond]())

        if chapter is None:
            return False

        if cond == "dialogue_ratio_gt":
            return chapter.dialogue_ratio > (threshold or 0.4)
        if cond == "word_count_gt":
            return chapter.word_count > (threshold or 5000)
        if cond == "character_count_gt" and chapter.outline:
            return len(chapter.outline.characters) > (threshold or 4)
        if cond == "has_scene_tags" and chapter.outline and rule.tags:
            return bool(set(chapter.outline.scene_tags) & set(rule.tags))

        return False

    @staticmethod
    def _audit_fail_rate(state: ProjectState) -> float:
        recent = state.audit_fail_history[-5:]
        if not recent:
            return 0.0
        return sum(1 for passed in recent if not passed) / len(recent)

    def activate_recommendations(
        self,
        state: ProjectState,
        recommendations: list[ActivationRecommendation],
    ) -> list[str]:
        """Apply activation recommendations to project state."""
        newly_activated: list[str] = []
        for rec in recommendations:
            if rec.auto_activate and rec.agent_id not in state.active_agents:
                state.active_agents.append(rec.agent_id)
                newly_activated.append(rec.agent_id)
        return newly_activated

    def pipeline_for_chapter(
        self, state: ProjectState, chapter: ChapterRecord
    ) -> list[str]:
        """Build ordered agent pipeline for a chapter based on active agents."""
        order = [
            "chapter_planner",
            "writer",
            "dialogue_specialist",
            "scene_specialist",
            "consistency_auditor",
            "foreshadow_tracker",
            "pacing_reviewer",
            "literary_editor",
            "summarizer",
        ]
        active = set(state.active_agents)
        return [aid for aid in order if aid in active]
