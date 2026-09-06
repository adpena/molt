from __future__ import annotations

from pathlib import Path
import sys

_SOURCE_ROOT = str(Path(__file__).resolve().parent / "src")
if _SOURCE_ROOT not in sys.path:
    sys.path.insert(0, _SOURCE_ROOT)

from molt.pytest_memory_guard_bootstrap import ensure_python_test_memory_guard  # noqa: E402


ensure_python_test_memory_guard()
