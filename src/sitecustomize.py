from __future__ import annotations

from pathlib import Path
import sys


_ROOT = Path(__file__).resolve().parents[1]
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))

from tools.pytest_memory_guard_bootstrap import (  # noqa: E402
    ensure_python_test_memory_guard,
)


ensure_python_test_memory_guard()
