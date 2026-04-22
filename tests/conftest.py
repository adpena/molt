from __future__ import annotations

import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MOLT_STDLIB_ROOT = str(ROOT / "src" / "molt" / "stdlib")


def _remove_molt_stdlib_top_level_root() -> None:
    """Keep host pytest imports on CPython's stdlib.

    Surface tests may load Molt stdlib files directly, but `src/molt/stdlib`
    must not remain as a top-level import root during collection. If it does,
    host imports such as `ctypes`, `fractions`, `statistics`, and `tarfile`
    resolve to Molt intrinsic-gated wrappers and fail before the runtime exists.
    """

    while MOLT_STDLIB_ROOT in sys.path:
        sys.path.remove(MOLT_STDLIB_ROOT)


def _ensure_src_on_path() -> None:
    for subdir in ("src", "tools"):
        p = str(ROOT / subdir)
        if p not in sys.path:
            sys.path.insert(0, p)
    _remove_molt_stdlib_top_level_root()


def pytest_configure() -> None:
    _ensure_src_on_path()


def pytest_collect_file() -> None:
    _remove_molt_stdlib_top_level_root()
