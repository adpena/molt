#!/usr/bin/env python3
"""Stable source-checkout entrypoint for the proof queue CLI."""

from __future__ import annotations

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
    raise SystemExit(main())
