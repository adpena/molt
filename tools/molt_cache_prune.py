#!/usr/bin/env python3
"""Source-checkout entrypoint for installed canonical cache retention."""

from __future__ import annotations

import sys
from pathlib import Path

SRC_ROOT = Path(__file__).resolve().parents[1] / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.cache_eviction import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
