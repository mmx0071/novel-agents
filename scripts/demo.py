#!/usr/bin/env python3
"""Demo script: init project and run 3 chapters with agent auto-activation."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROJECT = "demo"


def run(cmd: list[str]) -> None:
    print(f"\n$ {' '.join(cmd)}\n")
    subprocess.run(cmd, cwd=ROOT, check=True)


def main() -> None:
    python = sys.executable
    pkg = str(ROOT / "src")

    env = {"PYTHONPATH": pkg}

    def invoke(args: list[str]) -> None:
        full = [python, "-m", "novel_agents.cli", *args]
        print(f"\n$ {' '.join(full)}\n")
        subprocess.run(full, cwd=ROOT, env={**os.environ, **env}, check=True)

    invoke(["init", PROJECT, "--genre", "科幻悬疑", "--chapters", "100", "--setup"])
    invoke(["run", PROJECT, "1"])
    invoke(["run", PROJECT, "2", "--characters", "主角,配角A,配角B,配角C,配角D"])
    invoke(["run", PROJECT, "3", "--scene-tags", "battle,climax"])
    invoke(["status", PROJECT])
    invoke(["agents"])

    state_path = ROOT / "projects" / PROJECT / "state.json"
    if state_path.exists():
        state = json.loads(state_path.read_text(encoding="utf-8"))
        print("\n=== Demo 完成 ===")
        print(f"已发布：{sum(1 for c in state.get('chapters', []) if c.get('status') == 'published')} 章")
        print(f"活跃 Agent：{len(state.get('active_agents', []))} 个")
        print(f"  {', '.join(state.get('active_agents', []))}")


if __name__ == "__main__":
    main()
