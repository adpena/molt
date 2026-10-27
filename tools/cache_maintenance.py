#!/usr/bin/env python3
"""Cache-policy worker for the existing retained maintenance supervisor."""

from __future__ import annotations

import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.checkout_maintenance import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main(operation="cache"))
