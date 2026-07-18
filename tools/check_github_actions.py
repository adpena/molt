#!/usr/bin/env python3
"""Run the repository's canonical GitHub Actions static validation."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.bootstrap_actionlint import ensure_actionlint  # noqa: E402


def main() -> int:
    try:
        executable = ensure_actionlint()
    except (OSError, RuntimeError) as exc:
        print(f"actionlint: {exc}", file=sys.stderr)
        return 2
    return subprocess.run([str(executable)], cwd=ROOT, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
