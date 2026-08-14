from __future__ import annotations

import argparse
from collections.abc import Callable
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib
from types import ModuleType
from typing import Any


def replay_recent_commits(
    plan: Any,
    count: int,
    *,
    run_git: Callable[[list[str]], str],
    diff_paths: Callable[..., list[str]],
) -> dict[str, Any]:
    """Replay first-parent commit diffs and quantify avoided family launches."""
    if count <= 0:
        raise ValueError("replay commit count must be positive")
    started = time.perf_counter()
    commits = run_git(
        ["rev-list", "--first-parent", f"--max-count={count}", "HEAD"]
    ).splitlines()
    launches = {family.name: 0 for family in plan.families}
    path_total = 0
    for commit in commits:
        paths = diff_paths(f"{commit}^", commit)
        path_total += len(paths)
        for family in plan.select(paths).selected:
            launches[family.name] += 1
    total = len(commits)
    return {
        "schema": "molt.proof-plan-replay.v1",
        "commits": total,
        "changed_paths_examined": path_total,
        "wall_time_ms": round((time.perf_counter() - started) * 1000, 2),
        "families": {
            name: {
                "selected": selected,
                "avoided": total - selected,
                "avoidable_percent": round(
                    100.0 * (total - selected) / total if total else 0.0,
                    1,
                ),
            }
            for name, selected in launches.items()
        },
    }


def write_github_outputs(path: Path, outputs: dict[str, str]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for name, value in outputs.items():
            print(f"{name}={value}", file=handle)


def main(api: ModuleType, argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=api.__doc__)
    parser.add_argument("--manifest", type=Path, default=api.DEFAULT_MANIFEST)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--path", action="append", default=[])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--replay-commits", type=int)
    parser.add_argument(
        "--verify-selected",
        help="JSON array of selected family names whose required results must pass.",
    )
    parser.add_argument(
        "--verify-scheduled",
        action="store_true",
        help="Verify receipts for every scheduled family.",
    )
    parser.add_argument("--receipt-dir", type=Path)
    parser.add_argument("--run-family")
    parser.add_argument("--run-command")
    parser.add_argument("--matrix-cell")
    parser.add_argument("--receipt", type=Path)
    parser.add_argument("--event-name", default=os.environ.get("GITHUB_EVENT_NAME", ""))
    parser.add_argument("--base-ref", default=os.environ.get("GITHUB_BASE_REF", ""))
    parser.add_argument("--event-path", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    parser.add_argument("--before", default="")
    parser.add_argument("--after", default=os.environ.get("GITHUB_SHA", ""))
    args = parser.parse_args(argv)

    try:
        plan = api.ProofPlan.load(args.manifest)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"proof-plan: {exc}", file=sys.stderr)
        return 2
    if args.check:
        print(
            f"proof-plan: OK families={len(plan.families)} "
            f"local_rules={len(plan.local_rules)} authority={plan.path}"
        )
        return 0
    if args.run_family is not None or args.run_command is not None:
        if args.run_family is not None and args.run_command is not None:
            print(
                "proof-plan: choose only one of --run-family/--run-command",
                file=sys.stderr,
            )
            return 2
        if args.matrix_cell is not None and args.run_family is None:
            print(
                "proof-plan: --matrix-cell requires --run-family",
                file=sys.stderr,
            )
            return 2
        if args.receipt is None:
            print("proof-plan: executable proofs require --receipt", file=sys.stderr)
            return 2
        try:
            commands = api._topological_commands(
                plan,
                family=args.run_family,
                command_id=args.run_command,
                matrix_cell=args.matrix_cell,
            )
            return api.execute_commands(plan, commands, args.receipt)
        except (OSError, ValueError) as exc:
            print(f"proof-plan execution: {exc}", file=sys.stderr)
            return 2
    if args.replay_commits is not None:
        try:
            replay = api.replay_recent_commits(plan, args.replay_commits)
        except (RuntimeError, subprocess.CalledProcessError, ValueError) as exc:
            print(f"proof-plan replay: {exc}", file=sys.stderr)
            return 2
        print(json.dumps(replay, indent=2, sort_keys=True))
        return 0
    if args.verify_selected is not None:
        try:
            selected = json.loads(args.verify_selected)
            if not isinstance(selected, list) or not all(
                isinstance(name, str) for name in selected
            ):
                raise ValueError("--verify-selected must be a JSON string array")
            if args.receipt_dir is None:
                raise ValueError("--verify-selected requires --receipt-dir")
            errors = api.verify_receipts(plan, selected, args.receipt_dir)
        except (ValueError, json.JSONDecodeError) as exc:
            print(f"proof-plan verdict: {exc}", file=sys.stderr)
            return 2
        if errors:
            for error in errors:
                print(f"proof-plan verdict: {error}", file=sys.stderr)
            return 1
        print(
            f"proof-plan verdict: OK selected={len(selected)} "
            f"required={sum(1 for family in plan.families if family.name in selected and family.data['required'])}"
        )
        return 0
    if args.verify_scheduled:
        if args.receipt_dir is None:
            print(
                "proof-plan scheduled verdict: --receipt-dir is required",
                file=sys.stderr,
            )
            return 2
        selected = [family.name for family in plan.scheduled_families]
        errors = api.verify_receipts(plan, selected, args.receipt_dir)
        if errors:
            for error in errors:
                print(f"proof-plan scheduled verdict: {error}", file=sys.stderr)
            return 1
        print(
            f"proof-plan scheduled verdict: OK families={len(selected)} "
            f"commands={sum(command.family in selected for command in plan.commands)}"
        )
        return 0
    selection = (
        plan.select(args.path)
        if args.path
        else api.selection_for_event(
            plan,
            event_name=args.event_name,
            base_ref=args.base_ref,
            event_path=args.event_path,
            before=args.before,
            after=args.after,
        )
    )
    outputs = api.family_outputs(plan, selection)
    if args.json:
        print(json.dumps(outputs, indent=2, sort_keys=True))
    else:
        for family in plan.families:
            print(f"{family.name}={outputs[family.name]}")
        print(f"matrix={outputs['matrix']}")
        if selection.fail_closed_reason:
            print(f"proof-plan: {selection.fail_closed_reason}", file=sys.stderr)
    if args.github_output is not None:
        api.write_github_outputs(args.github_output, outputs)
    return 0
