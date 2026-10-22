"""Build-state lock paths over the shared file-lock authority."""

from __future__ import annotations

from collections.abc import Generator
from contextlib import ExitStack, contextmanager
import functools
import os
from pathlib import Path

from molt.cli import progress as _progress
from molt.file_locks import held_file_lock
from molt.cli.runtime_paths import _build_state_root


@functools.lru_cache(maxsize=256)
def _build_lock_dir_cached(project_root_str: str, build_state_root_str: str) -> Path:
    return Path(build_state_root_str) / "build_locks"


class BuildLockAcquisitionError(RuntimeError):
    """Expected acquisition failure; errors inside locked work stay distinct."""


@contextmanager
def held_build_lock(lock_path: Path, *, purpose: str) -> Generator[None]:
    """Hold one build lock by the one wait policy (``molt.file_locks``).

    A waiter waits while the build it waits for holds the lock, prints one
    notice naming the lock and its holder, and fails only past an operator's
    ``MOLT_BUILD_LOCK_TIMEOUT``.
    """
    with ExitStack() as held:
        try:
            held.enter_context(
                held_file_lock(
                    lock_path, purpose=purpose, notice=_progress.blocking_notice
                )
            )
        except (OSError, RuntimeError) as exc:
            raise BuildLockAcquisitionError(str(exc)) from exc
        yield


@contextmanager
def _build_lock(
    project_root: Path,
    name: str,
    *,
    cargo_target: Path | None = None,
):
    """Hold ``name`` in the build control of ``cargo_target`` (the project's)."""
    lock_dir = _build_lock_dir_cached(
        os.fspath(project_root),
        os.fspath(_build_state_root(project_root, cargo_target)),
    )
    # The build-state root already carries target/session isolation. When an
    # operator explicitly shares a target/build-state root, mutable Cargo
    # artifacts must share the same lock regardless of MOLT_SESSION_ID.
    with held_build_lock(lock_dir / f"{name}.lock", purpose=f"build ({name})"):
        yield
