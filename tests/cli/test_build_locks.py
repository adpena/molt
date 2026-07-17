from __future__ import annotations

from pathlib import Path
import threading

from molt.cli import build_locks


def test_file_lock_serializes_when_platform_lock_is_process_reentrant(
    tmp_path: Path, monkeypatch,
) -> None:
    """The in-process authority must not depend on OS same-process semantics."""
    monkeypatch.setattr(build_locks, "_try_lock_file_handle", lambda _handle: True)
    lock_path = tmp_path / "shared.lock"

    first = build_locks._try_acquire_file_lock(lock_path)
    assert first is not None
    assert build_locks._try_acquire_file_lock(lock_path) is None

    build_locks._release_file_lock(first)
    second = build_locks._try_acquire_file_lock(lock_path)
    assert second is not None
    build_locks._release_file_lock(second)

    assert not build_locks._IN_PROCESS_LOCK_REGISTRY


def test_file_lock_releases_registry_reservation_when_platform_is_contended(
    tmp_path: Path, monkeypatch,
) -> None:
    monkeypatch.setattr(build_locks, "_try_lock_file_handle", lambda _handle: False)

    assert build_locks._try_acquire_file_lock(tmp_path / "shared.lock") is None
    assert not build_locks._IN_PROCESS_LOCK_REGISTRY


def test_file_lock_registry_key_canonicalizes_path_aliases(tmp_path: Path) -> None:
    nested = tmp_path / "nested"
    nested.mkdir()

    assert build_locks._in_process_lock_key(nested / ".." / "shared.lock") == (
        build_locks._in_process_lock_key(tmp_path / "shared.lock")
    )


def test_file_lock_registry_is_reinitialized_after_fork() -> None:
    prior_registry = build_locks._IN_PROCESS_LOCK_REGISTRY
    prior_guard = build_locks._IN_PROCESS_LOCK_REGISTRY_GUARD
    prior_registry["inherited"] = build_locks._InProcessLockEntry(
        mutex=threading.Lock(),
        users=1,
    )

    build_locks._reset_in_process_lock_registry_after_fork()

    assert build_locks._IN_PROCESS_LOCK_REGISTRY == {}
    assert build_locks._IN_PROCESS_LOCK_REGISTRY is not prior_registry
    assert build_locks._IN_PROCESS_LOCK_REGISTRY_GUARD is not prior_guard
