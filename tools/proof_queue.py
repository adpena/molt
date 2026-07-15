#!/usr/bin/env python3
"""Stable source-checkout entrypoint for the proof queue CLI."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from tools.proof_queue_pkg.cli import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
