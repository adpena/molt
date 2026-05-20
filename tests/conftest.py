from __future__ import annotations

import os
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MOLT_STDLIB_ROOT = str(ROOT / "src" / "molt" / "stdlib")
_PYTEST_SENTINEL_ATTR = "_molt_repo_process_sentinel"


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


def _ensure_pytest_process_scope() -> None:
    os.environ.setdefault("MOLT_SESSION_ID", f"pytest-{os.getpid()}")


def pytest_configure() -> None:
    _ensure_src_on_path()
    _ensure_pytest_process_scope()


def pytest_sessionstart(session) -> None:  # type: ignore[no-untyped-def]
    _ensure_src_on_path()
    _ensure_pytest_process_scope()
    from tools import harness_memory_guard

    sentinel = harness_memory_guard.repo_process_sentinel(
        repo_root=ROOT,
        artifact_root=ROOT / "tmp" / "pytest-memory-guard",
        label=f"pytest-{os.getpid()}",
        limits=harness_memory_guard.limits_from_env("MOLT_PYTEST"),
        drain_on_exit=True,
    )
    setattr(session.config, _PYTEST_SENTINEL_ATTR, sentinel)
    sentinel.__enter__()


def pytest_sessionfinish(session, exitstatus) -> None:  # type: ignore[no-untyped-def]
    sentinel = getattr(session.config, _PYTEST_SENTINEL_ATTR, None)
    if sentinel is not None:
        sentinel.__exit__(None, None, None)


def pytest_collect_file() -> None:
    _remove_molt_stdlib_top_level_root()
