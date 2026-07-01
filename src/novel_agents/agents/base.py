from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any

from novel_agents.models import ProjectState


class BaseAgent(ABC):
    """Base class for all novel-writing agents."""

    agent_id: str = ""
    name: str = ""

    @abstractmethod
    def run(self, state: ProjectState, context: dict[str, Any]) -> dict[str, Any]:
        """Execute agent task and return output dict for next agent."""

    def describe(self) -> str:
        return f"{self.name} ({self.agent_id})"
