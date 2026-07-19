#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any, Mapping
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


REQUIRED_PHASES = frozenset(
    {
        "ir_lowering",
        "frontend_lowering",
        "backend_codegen",
        "final_app_codegen",
        "wasm_link_total",
        "wasm_link_core",
        "wasm_strip",
        "split_runtime_processing",
        "fail_closed_validation",
        "seal",
    }
)


def _load_attribution(path: Path) -> Mapping[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    attribution = payload.get("phase_attribution")
    if not isinstance(attribution, Mapping):
        raise ValueError(f"{path} has no phase_attribution object")
    phase_sec = attribution.get("phase_sec")
    phase_share = attribution.get("phase_share")
    if not isinstance(phase_sec, Mapping) or not isinstance(phase_share, Mapping):
        raise ValueError(f"{path} has an incomplete phase attribution schema")
    missing = sorted(REQUIRED_PHASES - set(phase_sec))
    if missing:
        raise ValueError(f"{path} is missing required phases: {', '.join(missing)}")
    return attribution


def _print_attribution(attribution: Mapping[str, Any]) -> None:
    phase_sec = attribution["phase_sec"]
    phase_share = attribution["phase_share"]
    ranked = attribution.get("ranked_phases", sorted(phase_sec))
    print("Build phase attribution:")
    for name in ranked:
        if name not in phase_sec or name not in phase_share:
            continue
        print(
            f"- {name}: {float(phase_sec[name]):.6f}s "
            f"({float(phase_share[name]) * 100.0:.2f}%)"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--diagnostics", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command:
        command = list(args.command)
        if command[0] == "--":
            command = command[1:]
        env = os.environ.copy()
        env["MOLT_BUILD_DIAGNOSTICS"] = "1"
        env["MOLT_BUILD_DIAGNOSTICS_FILE"] = str(args.diagnostics.resolve())
        completed = _COMMANDS.run(command, env=env, check=False)
        if completed.returncode != 0:
            return completed.returncode
    try:
        attribution = _load_attribution(args.diagnostics)
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as exc:
        print(f"build phase attribution failed: {exc}", file=sys.stderr)
        return 1
    _print_attribution(attribution)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
