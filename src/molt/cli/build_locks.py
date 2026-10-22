"""Build locks: their paths, their one wait policy and their holder records.

A waiter on a build lock (a checkout's build lock or a product store
identity's lock) waits while the holder holds it (``molt.file_locks``'s
``lock_wait_s``), prints one notice naming the lock and its holder, as Cargo's
"Blocking waiting for file lock" and Bazel's "Another command (pid=...) is
running" do, and fails only past an operator's ``MOLT_BUILD_LOCK_TIMEOUT``.
The holder names itself in ``<lock>.holder`` while it holds the lock.
"""

from __future__ import annotations

from collections.abc import Callable, Generator
from contextlib import contextmanager
from dataclasses import dataclass
import functools
import json
import os
from pathlib import Path
import time

from molt.cli import progress as _progress
from molt.file_locks import (
    LOCK_WAIT_ENV,
    FileLockTimeout,
    _acquire_file_lock,
    _release_file_lock,
    _try_acquire_file_lock,
    lock_wait_s,
    lock_wait_timeout_message,
)
from molt.cli.runtime_paths import _build_state_root


@functools.lru_cache(maxsize=256)
def _build_lock_dir_cached(project_root_str: str, build_state_root_str: str) -> Path:
    return Path(build_state_root_str) / "build_locks"


class BuildLockAcquisitionError(RuntimeError):
    """Expected acquisition failure; errors inside locked work stay distinct."""


@dataclass(frozen=True, slots=True)
class LockHolder:
    """The process a lock's holder record names."""

    pid: int
    started_at_ns: int | None
    # False when this host cannot read another process's start time, so the
    # pid runs but may have been reused since the record was written.
    start_verified: bool = True

    def describe(self) -> str:
        if not self.start_verified:
            return f"pid {self.pid} (its start time is not observable here)"
        if self.started_at_ns is None:
            return f"pid {self.pid}"
        started = time.strftime(
            "%Y-%m-%dT%H:%M:%SZ", time.gmtime(self.started_at_ns / 1_000_000_000)
        )
        return f"pid {self.pid}, started {started}"


_HOLDER_SCHEMA = "molt.lock-holder.v1"
_HOLDER_MAX_BYTES = 512
# pid -> the kernel's start marker of that process, read once per process.
_OWN_START: dict[int, int | None] = {}


def lock_holder_path(lock_path: Path) -> Path:
    """Where the holder of ``lock_path`` records itself for waiters to read.

    The record sits beside the lock, never in it: lock ownership is the OS
    region and the in-process mutex (``molt.file_locks``), and lock files stay
    empty metadata. Like the lock, it persists; it is empty while no one holds
    the lock.
    """
    return lock_path.with_name(f"{lock_path.name}.holder")


def _own_start_marker() -> int | None:
    pid = os.getpid()
    if pid not in _OWN_START:
        from molt.process_model import process_started_at_ns

        _OWN_START[pid] = process_started_at_ns(pid)
    return _OWN_START[pid]


def _record_lock_holder(lock_path: Path) -> int | None:
    """Name this process beside a lock it holds, for people to read.

    The record is process-lifetime state, as Bazel's ``pid=`` line in its
    output-base lock is: no barrier, and a torn or stale record only makes a
    waiter's notice say less. Its descriptor stays open for the hold, so
    clearing it at release is one ``ftruncate``: 25-35 us per acquisition
    on the integrator Mac, against about 400 us to create and unlink a file
    on its external volume. Writing it never fails the lock.
    """
    payload = {
        "schema": _HOLDER_SCHEMA,
        "pid": os.getpid(),
        "started_at_ns": _own_start_marker(),
    }
    data = json.dumps(payload, sort_keys=True).encode("utf-8")
    try:
        descriptor = os.open(
            lock_holder_path(lock_path), os.O_WRONLY | os.O_CREAT, 0o666
        )
    except OSError:
        return None
    try:
        os.write(descriptor, data)
        os.ftruncate(descriptor, len(data))
    except OSError:
        _clear_lock_holder(descriptor)
        return None
    return descriptor


def _clear_lock_holder(descriptor: int | None) -> None:
    """Leave the record empty, which names no holder, and close it."""
    if descriptor is None:
        return
    try:
        try:
            os.ftruncate(descriptor, 0)
        except OSError:
            pass
    finally:
        os.close(descriptor)


def read_lock_holder(lock_path: Path) -> LockHolder | None:
    """The live holder ``lock_path``'s record names, or None when unknown.

    A record whose process has exited, or whose pid another process now has,
    names no holder: its writer died before it could clear it. Where the start
    time is not observable (another process on Windows), a record names its
    pid only while that pid runs, and the holder says the pid may be reused.
    """
    try:
        with open(lock_holder_path(lock_path), "rb") as record:
            raw = record.read(_HOLDER_MAX_BYTES + 1)
        payload = json.loads(raw) if len(raw) <= _HOLDER_MAX_BYTES else None
    except (OSError, ValueError):
        return None
    if not isinstance(payload, dict) or payload.get("schema") != _HOLDER_SCHEMA:
        return None
    pid, started = payload.get("pid"), payload.get("started_at_ns")
    if type(pid) is not int or pid <= 0:
        return None
    if started is not None and type(started) is not int:
        return None
    from molt.process_model import pid_alive, process_started_at_ns

    observed = process_started_at_ns(pid)
    if observed is None:
        # An exited process has no start time either; only a running pid
        # can be the holder.
        try:
            running = pid_alive(pid)
        except OSError:
            return None
        return LockHolder(pid, started, start_verified=False) if running else None
    if observed != started:
        return None
    return LockHolder(pid, started)


def _holder_text(lock_path: Path) -> str:
    holder = read_lock_holder(lock_path)
    return (
        f"held by {holder.describe()}"
        if holder is not None
        else "its holder has not recorded itself"
    )


@contextmanager
def held_build_lock(
    lock_path: Path,
    *,
    purpose: str,
    notice: Callable[[str], None] | None = None,
) -> Generator[None]:
    """Hold one build lock exclusively, by the one wait policy.

    A waiter prints one notice naming the lock and its holder, then waits as
    ``lock_wait_s`` says; the holder records itself beside the lock while it
    holds it.
    """
    try:
        handle = _try_acquire_file_lock(lock_path)
        if handle is None:
            wait = lock_wait_s()
            bound = (
                "until it is released"
                if wait is None
                else f"at most {wait:.1f}s ({LOCK_WAIT_ENV})"
            )
            (notice or _progress.blocking_notice)(
                f"Blocking waiting for the {purpose} lock {lock_path}, "
                f"{_holder_text(lock_path)}; waiting {bound}"
            )
            try:
                handle = _acquire_file_lock(
                    lock_path, timeout_s=wait, timeout_message="lock wait timed out"
                )
            except FileLockTimeout:
                assert wait is not None
                raise BuildLockAcquisitionError(
                    lock_wait_timeout_message(
                        wait,
                        f"the {purpose} lock {lock_path}, {_holder_text(lock_path)}",
                    )
                ) from None
    except BuildLockAcquisitionError:
        raise
    except (OSError, RuntimeError) as exc:
        raise BuildLockAcquisitionError(str(exc)) from exc
    record = None
    try:
        record = _record_lock_holder(lock_path)
        yield
    finally:
        _clear_lock_holder(record)
        _release_file_lock(handle)


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
