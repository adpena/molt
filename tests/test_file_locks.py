from __future__ import annotations

import errno
from pathlib import Path
import sys
import threading
from types import SimpleNamespace

import pytest

from molt import file_locks as build_locks
from tests.process_guard_common import run_custody_subject_process


@pytest.mark.parametrize("contents", [None, b"", b"old advisory PID\n"])
def test_file_lock_ownership_never_mutates_file_contents(tmp_path, contents):
    lock_path = tmp_path / "shared.lock"
    if contents is not None:
        lock_path.write_bytes(contents)
    handle = build_locks._try_acquire_file_lock(lock_path)
    assert handle is not None
    build_locks._release_file_lock(handle)
    assert lock_path.read_bytes() == (contents or b"")
    assert not build_locks._IN_PROCESS_LOCK_REGISTRY


def test_empty_file_interprocess_contention_reaches_os_lock_without_writing(tmp_path):
    lock_path = tmp_path / "shared.lock"
    # Hold byte zero beyond EOF, exactly as in the retired holder-PID truncate
    # window. A separate interpreter cannot be protected by our process mutex.
    with lock_path.open("w+b", buffering=0) as holder:
        assert build_locks._try_lock_file_handle(holder)
        try:
            completed = run_custody_subject_process(
                [
                    sys.executable,
                    "-c",
                    "import sys; from pathlib import Path; "
                    "from molt import file_locks; path = Path(sys.argv[1]); "
                    "assert file_locks._try_acquire_file_lock(path) is None; "
                    "assert path.stat().st_size == 0; "
                    "assert not file_locks._IN_PROCESS_LOCK_REGISTRY",
                    str(lock_path),
                ],
                check=False,
                capture_output=True,
                text=True,
                timeout=15,
            )
            assert completed.returncode == 0, completed.stdout + completed.stderr
            assert lock_path.stat().st_size == 0
            assert not build_locks._IN_PROCESS_LOCK_REGISTRY
        finally:
            build_locks._unlock_file_handle(holder)
    handle = build_locks._try_acquire_file_lock(lock_path)
    assert handle is not None
    build_locks._release_file_lock(handle)
    assert lock_path.read_bytes() == b""


@pytest.mark.parametrize(
    "error_number",
    [errno.EACCES, errno.EAGAIN, errno.EBADF, errno.EINVAL, errno.ENOSPC],
)
def test_windows_lock_only_classifies_contention_errors(
    tmp_path, monkeypatch, error_number
):
    calls = []
    error = OSError(error_number, "fixture lock failure")

    def lock(fd, mode, length):
        calls.append((fd, mode, length))
        raise error

    monkeypatch.setattr(build_locks, "os", SimpleNamespace(name="nt"))
    monkeypatch.setitem(
        sys.modules, "msvcrt", SimpleNamespace(LK_NBLCK=1, locking=lock)
    )
    with (tmp_path / "shared.lock").open("w+b") as handle:
        if error_number in (errno.EACCES, errno.EAGAIN):
            assert build_locks._try_lock_file_handle(handle) is False
        else:
            with pytest.raises(OSError) as raised:
                build_locks._try_lock_file_handle(handle)
            assert raised.value is error
    assert len(calls) == 1


@pytest.mark.parametrize("phase", ["open", "lock"])
def test_file_lock_failures_propagate_and_release_process_reservation(
    tmp_path, monkeypatch, phase
):
    calls = []
    error = OSError(errno.EACCES if phase == "open" else errno.EIO, "fixture failure")

    def fail(_value):
        calls.append(phase)
        raise error

    with monkeypatch.context() as patch:
        patch.setattr(
            build_locks,
            "_open_file_lock_handle" if phase == "open" else "_try_lock_file_handle",
            fail,
        )
        with pytest.raises(OSError) as raised:
            build_locks._try_acquire_file_lock(tmp_path / "shared.lock")
        assert raised.value is error
    assert calls == [phase]
    assert not build_locks._IN_PROCESS_LOCK_REGISTRY
    handle = build_locks._try_acquire_file_lock(tmp_path / "shared.lock")
    assert handle is not None
    build_locks._release_file_lock(handle)


def test_file_lock_serializes_when_platform_lock_is_process_reentrant(
    tmp_path: Path,
    monkeypatch,
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
    tmp_path: Path,
    monkeypatch,
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


def test_file_lock_and_proof_cache_imports_do_not_load_cli_or_frontend():
    completed = run_custody_subject_process(
        [
            sys.executable,
            "-c",
            "import sys; from molt import file_locks; "
            "from tools.proof_queue_pkg import cargo_cache_custody; "
            "assert not any(name == 'molt.cli' or name.startswith('molt.frontend') "
            "for name in sys.modules), sorted(sys.modules)",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert completed.returncode == 0, completed.stderr
