#!/usr/bin/env python3
"""Run the repository's canonical GitHub Actions static validation."""

from __future__ import annotations

import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]


from tools.bootstrap_actionlint import ensure_actionlint  # noqa: E402

from tools.command_execution import CommandExecutor  # type: ignore  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_CI")


def main() -> int:
    try:
        executable = ensure_actionlint()
    except (OSError, RuntimeError) as exc:
        print(f"actionlint: {exc}", file=sys.stderr)
        return 2
    return _COMMANDS.run([str(executable)], cwd=ROOT, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
