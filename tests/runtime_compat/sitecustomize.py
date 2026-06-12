from __future__ import annotations

from pathlib import Path
import sys


for _parent in Path(__file__).resolve().parents:
    if (_parent / "_sitecustomize.py").is_file():
        sys.path.insert(0, str(_parent))
        break
else:
    raise RuntimeError("could not locate tests/_sitecustomize.py")

from _sitecustomize import install_test_memory_guard_sitecustomize  # noqa: E402


install_test_memory_guard_sitecustomize(__file__)
