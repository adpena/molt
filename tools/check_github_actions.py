#!/usr/bin/env python3
"""Run the repository's canonical GitHub Actions static validation."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.bootstrap_actionlint import ensure_actionlint  # noqa: E402
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


def main() -> int:
    try:
        executable = ensure_actionlint()
    except (OSError, RuntimeError) as exc:
        print(f"actionlint: {exc}", file=sys.stderr)
        return 2
    return _COMMANDS.run([str(executable)], cwd=ROOT, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
