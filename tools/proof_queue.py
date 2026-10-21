#!/usr/bin/env python3
"""Stable source-checkout entrypoint for the proof queue CLI."""

from __future__ import annotations

import os
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"


from tools.proof_queue_pkg.cli import main  # noqa: E402

if __name__ == "__main__":
    if sys.argv[1:2] == ["--detached-state-use"]:
        from tools.command_execution import detached_command_use

        if len(sys.argv) < 4:
            raise SystemExit("detached state-use descriptor requires path and token")
        ready_path, token = Path(sys.argv[2]), sys.argv[3]
        del sys.argv[1:4]
        with detached_command_use(
            ROOT,
            dict(os.environ),
            cwd=Path.cwd(),
            ready_path=ready_path,
            use_token=token,
        ):
            raise SystemExit(main())
    raise SystemExit(main())
