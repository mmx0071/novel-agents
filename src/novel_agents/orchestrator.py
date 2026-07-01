from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from novel_agents.models import (
    ChapterStatus,
    PipelineResult,
    PipelineStep,
    ProjectState,
)
from novel_agents.registry import AgentRegistry
from novel_agents.agents import AGENT_INSTANCES


class Orchestrator:
    """Central scheduler: activation check → pipeline execution → persistence."""

    def __init__(
        self,
        project_dir: Path,
        agent_registry: AgentRegistry | None = None,
    ):
        self.project_dir = Path(project_dir)
        self.registry = agent_registry or AgentRegistry()
        self.state_path = self.project_dir / "state.json"
        self.state = self._load_state()

    def _load_state(self) -> ProjectState:
        if self.state_path.exists():
            return ProjectState.model_validate_json(
                self.state_path.read_text(encoding="utf-8")
            )
        meta_path = self.project_dir / "meta.json"
        if meta_path.exists():
            meta = json.loads(meta_path.read_text(encoding="utf-8"))
            state = ProjectState(**meta)
        else:
            state = ProjectState(name=self.project_dir.name)
        if not state.active_agents:
            state.active_agents = self.registry.initial_active_ids()
        return state

    def save(self) -> None:
        self.project_dir.mkdir(parents=True, exist_ok=True)
        self.state_path.write_text(
            self.state.model_dump_json(indent=2),
            encoding="utf-8",
        )

    def check_and_activate_agents(
        self, chapter_number: int | None = None
    ) -> list[str]:
        chapter = (
            self.state.get_chapter(chapter_number) if chapter_number else None
        )
        recommendations = self.registry.evaluate_activation(self.state, chapter)
        return self.registry.activate_recommendations(self.state, recommendations)

    def run_chapter(
        self,
        chapter_number: int,
        context: dict[str, Any] | None = None,
    ) -> PipelineResult:
        context = dict(context or {})
        context["chapter_number"] = chapter_number

        chapter = self.state.ensure_chapter(chapter_number)
        chapter.status = ChapterStatus.PLANNING

        prev = self.state.get_chapter(chapter_number - 1)
        if prev and prev.summary:
            context.setdefault("prev_summary", prev.summary.event_summary)

        activated = self.check_and_activate_agents(chapter_number)
        recommendations = self.registry.evaluate_activation(self.state, chapter)

        steps: list[PipelineStep] = []
        run_context = dict(context)
        completed: set[str] = set()
        recheck_after = {"chapter_planner", "writer"}

        while True:
            pipeline_ids = self.registry.pipeline_for_chapter(self.state, chapter)
            pending = [aid for aid in pipeline_ids if aid not in completed]
            if not pending:
                break

            agent_id = pending[0]
            agent = AGENT_INSTANCES.get(agent_id)
            if not agent:
                completed.add(agent_id)
                continue

            step = PipelineStep(agent_id=agent_id, agent_name=agent.name, status="running")
            try:
                output = agent.run(self.state, run_context)
                run_context.update(output)
                step.status = "done"
                step.output_preview = _preview(output)

                if agent_id in recheck_after:
                    newly = self.check_and_activate_agents(chapter_number)
                    activated.extend(newly)
            except Exception as exc:
                step.status = "failed"
                step.output_preview = str(exc)
                chapter.status = ChapterStatus.FAILED
                result = PipelineResult(
                    chapter_number=chapter_number,
                    steps=steps + [step],
                    activated_agents=recommendations,
                    final_status=chapter.status,
                    message=f"Agent {agent_id} failed: {exc}",
                )
                self.save()
                return result

            steps.append(step)
            completed.add(agent_id)

            if agent_id == "consistency_auditor" and not run_context.get("passed"):
                chapter.revision_count += 1
                chapter.status = ChapterStatus.FAILED
                result = PipelineResult(
                    chapter_number=chapter_number,
                    steps=steps,
                    activated_agents=recommendations,
                    final_status=chapter.status,
                    message="Consistency audit failed — revision required",
                )
                self._persist_chapter_artifacts(chapter_number, run_context)
                self.save()
                return result

        chapter.status = ChapterStatus.PUBLISHED
        result = PipelineResult(
            chapter_number=chapter_number,
            steps=steps,
            activated_agents=recommendations,
            final_status=chapter.status,
            message="Chapter pipeline completed",
        )
        self._persist_chapter_artifacts(chapter_number, run_context)
        self.save()
        return result

    def run_setup_phase(self) -> dict[str, Any]:
        """Run world/story setup agents if activated."""
        outputs: dict[str, Any] = {}
        setup_order = ["world_architect", "master_planner", "arc_planner"]
        for agent_id in setup_order:
            if agent_id not in self.state.active_agents:
                recs = self.registry.evaluate_activation(self.state)
                self.registry.activate_recommendations(self.state, recs)
            if agent_id in self.state.active_agents:
                agent = AGENT_INSTANCES[agent_id]
                outputs[agent_id] = agent.run(self.state, {})
                self._write_artifact(f"{agent_id}.md", str(outputs[agent_id].get(
                    "bible"
                ) or outputs[agent_id].get("master_outline") or outputs[agent_id].get(
                    "arc_outline", ""
                )))
        self.save()
        return outputs

    def _persist_chapter_artifacts(
        self, chapter_number: int, context: dict[str, Any]
    ) -> None:
        ch_dir = self.project_dir / "chapters" / f"{chapter_number:03d}"
        ch_dir.mkdir(parents=True, exist_ok=True)
        if "outline_text" in context:
            (ch_dir / "outline.md").write_text(
                context["outline_text"], encoding="utf-8"
            )
        if "draft" in context:
            (ch_dir / "draft.md").write_text(context["draft"], encoding="utf-8")
        chapter = self.state.get_chapter(chapter_number)
        if chapter and chapter.summary:
            (ch_dir / "summary.json").write_text(
                chapter.summary.model_dump_json(indent=2), encoding="utf-8"
            )

    def _write_artifact(self, filename: str, content: str) -> None:
        out = self.project_dir / "artifacts" / filename
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(content, encoding="utf-8")

    def status_report(self) -> dict[str, Any]:
        all_ids = list(self.registry.agents.keys())
        inactive = [a for a in all_ids if a not in self.state.active_agents]
        pending_activation = self.registry.evaluate_activation(self.state)
        return {
            "project": self.state.name,
            "published_chapters": self.state.published_count,
            "active_agents": self.state.active_agents,
            "inactive_agents": inactive,
            "pending_activation": [
                {"agent": r.agent_id, "reason": r.reason} for r in pending_activation
            ],
        }


def _preview(output: dict[str, Any], max_len: int = 120) -> str:
    text = json.dumps(output, ensure_ascii=False, default=str)
    return text[:max_len] + ("..." if len(text) > max_len else "")
