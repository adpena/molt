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
    # Under pytest-xdist each worker is a separate process that inherits the
    # master's environment. A plain ``setdefault`` would leave every worker
    # sharing the master's ``MOLT_SESSION_ID``, collapsing their backend
    # daemons, build state, and compile cache onto a single session — which
    # serialises (and races) compilation and makes parallel runs fail. Give
    # each xdist worker a distinct, stable session keyed on its worker id
    # (``gw0``/``gw1``/…); fall back to the pid for serial (non-xdist) runs.
    worker = os.environ.get("PYTEST_XDIST_WORKER")
    if worker:
        os.environ["MOLT_SESSION_ID"] = f"pytest-xdist-{worker}"
    else:
        os.environ.setdefault("MOLT_SESSION_ID", f"pytest-{os.getpid()}")


def pytest_configure() -> None:
    _ensure_src_on_path()
    _ensure_pytest_process_scope()


def _is_xdist_run(session) -> bool:  # type: ignore[no-untyped-def]
    """True when this pytest invocation runs under pytest-xdist (parallel).

    Detected in workers via ``PYTEST_XDIST_WORKER`` and in the controller via
    the resolved ``-n`` value (``numprocesses``).
    """
    if os.environ.get("PYTEST_XDIST_WORKER"):
        return True
    try:
        return bool(session.config.option.numprocesses)
    except AttributeError:
        return False


def pytest_sessionstart(session) -> None:  # type: ignore[no-untyped-def]
    _ensure_src_on_path()
    _ensure_pytest_process_scope()
    # The repo memory-guard sentinel drains (SIGTERMs) repo-scoped processes on
    # exit, which is fundamentally incompatible with pytest-xdist: a per-worker
    # sentinel SIGTERMs molt builds still running in OTHER workers when one
    # worker finishes first ("Compilation failed: SIGTERM (-15); no RSS
    # violation observed" → silently SKIPPED tests), and the controller-side
    # drain races xdist's worker-channel teardown ("OSError: cannot send
    # (already closed?)" in pytest_sessionfinish → nonzero exit even when every
    # test passed). Under xdist, skip the session sentinel entirely: each
    # compliance build is already bounded by the per-build memory guard in
    # tests/compliance/process_guard.run_compliance_process, and the worker
    # count caps aggregate concurrency. Serial runs keep the full sentinel.
    if _is_xdist_run(session):
        return
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
