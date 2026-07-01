from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from novel_agents.orchestrator import Orchestrator
from novel_agents.registry import AgentRegistry


def _projects_root() -> Path:
    return Path(__file__).resolve().parents[2] / "projects"


def cmd_init(args: argparse.Namespace) -> None:
    project_dir = _projects_root() / args.name
    project_dir.mkdir(parents=True, exist_ok=True)
    meta = {
        "name": args.name,
        "genre": args.genre,
        "target_chapters": args.chapters,
        "has_bible": False,
        "has_master_outline": False,
        "has_arc_outline": False,
    }
    (project_dir / "meta.json").write_text(
        json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    orch = Orchestrator(project_dir)
    if args.setup:
        orch.run_setup_phase()
    print(f"✓ 项目已创建：{project_dir}")
    print(f"  活跃 Agent：{', '.join(orch.state.active_agents)}")


def cmd_run(args: argparse.Namespace) -> None:
    project_dir = _projects_root() / args.name
    if not project_dir.exists():
        print(f"项目不存在：{project_dir}", file=sys.stderr)
        sys.exit(1)

    orch = Orchestrator(project_dir)
    context = {}
    if args.scene_tags:
        context["scene_tags"] = args.scene_tags.split(",")
    if args.characters:
        context["characters"] = args.characters.split(",")

    result = orch.run_chapter(args.chapter, context)
    print(f"\n=== 第 {result.chapter_number} 章流水线 ===\n")
    for step in result.steps:
        icon = "✓" if step.status == "done" else "✗"
        print(f"  {icon} [{step.agent_name}] {step.status}")
        if step.output_preview:
            print(f"      {step.output_preview}")

    if result.activated_agents:
        print("\n--- 建议激活的 Agent ---")
        for rec in result.activated_agents:
            active = rec.agent_id in orch.state.active_agents
            mark = "已激活" if active else "待激活"
            print(f"  · {rec.agent_name} ({mark})：{rec.reason}")

    print(f"\n结果：{result.message} → {result.final_status.value}")
    print(f"已发布章节：{orch.state.published_count}")


def cmd_status(args: argparse.Namespace) -> None:
    project_dir = _projects_root() / args.name
    orch = Orchestrator(project_dir)
    report = orch.status_report()
    print(json.dumps(report, ensure_ascii=False, indent=2))


def cmd_agents(args: argparse.Namespace) -> None:
    registry = AgentRegistry()
    print("\n=== Agent 注册表 ===\n")
    for tier in ("mvp", "extended"):
        print(f"【{tier.upper()}】")
        for agent in registry.agents.values():
            if agent.tier.value != tier:
                continue
            deps = f" → 依赖 {agent.depends_on}" if agent.depends_on else ""
            active = "默认启用" if agent.default_active else "按需激活"
            print(f"  · {agent.name} ({agent.id})")
            print(f"    {agent.description} [{active}]{deps}")
            for rule in agent.activation:
                print(f"    ↳ 激活条件：{rule.condition} — {rule.reason}")
        print()


def cmd_activate(args: argparse.Namespace) -> None:
    project_dir = _projects_root() / args.name
    orch = Orchestrator(project_dir)
    activated = orch.check_and_activate_agents()
    orch.save()
    if activated:
        print(f"新激活 Agent：{', '.join(activated)}")
    else:
        print("当前无需新激活 Agent")
    report = orch.status_report()
    if report["pending_activation"]:
        print("\n仍待满足条件的 Agent：")
        for item in report["pending_activation"]:
            print(f"  · {item['agent']}：{item['reason']}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="novel-agents — 超长篇多 Agent 小说创作系统"
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_init = sub.add_parser("init", help="创建新小说项目")
    p_init.add_argument("name", help="项目名称")
    p_init.add_argument("--genre", default="玄幻", help="题材")
    p_init.add_argument("--chapters", type=int, default=50, help="目标章节数")
    p_init.add_argument(
        "--setup", action="store_true", help="立即运行世界观/总纲/setup Agent"
    )
    p_init.set_defaults(func=cmd_init)

    p_run = sub.add_parser("run", help="运行单章流水线")
    p_run.add_argument("name", help="项目名称")
    p_run.add_argument("chapter", type=int, help="章节号")
    p_run.add_argument("--scene-tags", help="场景标签，逗号分隔，如 battle,action")
    p_run.add_argument("--characters", help="出场人物，逗号分隔")
    p_run.set_defaults(func=cmd_run)

    p_status = sub.add_parser("status", help="查看项目状态")
    p_status.add_argument("name", help="项目名称")
    p_status.set_defaults(func=cmd_status)

    p_agents = sub.add_parser("agents", help="列出所有 Agent")
    p_agents.set_defaults(func=cmd_agents)

    p_activate = sub.add_parser("activate", help="检查并激活所需 Agent")
    p_activate.add_argument("name", help="项目名称")
    p_activate.set_defaults(func=cmd_activate)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
