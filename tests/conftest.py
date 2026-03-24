from __future__ import annotations

import sys
from pathlib import Path


def _ensure_src_on_path() -> None:
    root = Path(__file__).resolve().parents[1]
    for subdir in ("src", "tools"):
        p = str(root / subdir)
        if p not in sys.path:
            sys.path.insert(0, p)


def pytest_configure() -> None:
    _ensure_src_on_path()
