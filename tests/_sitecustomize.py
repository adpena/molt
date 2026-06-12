from __future__ import annotations

from pathlib import Path
import sys


def _repo_root_from_sitecustomize(path: str) -> Path:
    for parent in Path(path).resolve().parents:
        guard = parent / "tools" / "pytest_memory_guard_bootstrap.py"
        if guard.is_file():
            return parent
    raise RuntimeError("could not locate Molt repo root for test memory guard")


def install_test_memory_guard_sitecustomize(path: str) -> None:
    root = _repo_root_from_sitecustomize(path)
    root_text = str(root)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)

    from tools.pytest_memory_guard_bootstrap import (  # noqa: PLC0415
        ensure_repo_test_script_memory_guard,
    )

    ensure_repo_test_script_memory_guard()
