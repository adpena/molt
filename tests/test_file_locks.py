from __future__ import annotations

from dataclasses import dataclass
import errno
import os
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import file_locks as build_locks
from molt.cli import build_locks as cli_build_locks
from tests.process_guard_common import run_custody_subject_process


@dataclass(frozen=True)
class _HeldLocks:
    """The process-wide lock state at one moment."""

    registry: frozenset[str]
    handles: frozenset[int]

    @classmethod
    def now(cls) -> _HeldLocks:
        return cls(
            frozenset(build_locks._IN_PROCESS_LOCK_REGISTRY),
            frozenset(build_locks._LIVE_FILE_LOCK_HANDLES),
        )


@pytest.fixture
def held_before() -> _HeldLocks:
    """Locks held when the test starts.

    A session can hold locks for its whole life (the backend-daemon suite
    lease, for one), so a test proves it leaves no lock of its own by
    comparing against this snapshot, not by expecting empty registries.
    """
    return _HeldLocks.now()


@pytest.mark.parametrize("contents", [None, b"", b"old advisory PID\n"])
def test_file_lock_ownership_never_mutates_file_contents(
    tmp_path, contents, held_before
):
    lock_path = tmp_path / "shared.lock"
    if contents is not None:
        lock_path.write_bytes(contents)
    handle = build_locks._try_acquire_file_lock(lock_path)
    assert handle is not None
    build_locks._release_file_lock(handle)
    assert lock_path.read_bytes() == (contents or b"")
    assert _HeldLocks.now().registry == held_before.registry


def test_empty_file_interprocess_contention_reaches_os_lock_without_writing(
    tmp_path, held_before
):
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
            assert _HeldLocks.now().registry == held_before.registry
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
def test_windows_lock_only_classifies_contention_errors(tmp_path, error_number):
    calls = []
    error = OSError(error_number, "fixture lock failure")

    def lock(fd, mode, length):
        calls.append((fd, mode, length))
        raise error

    windows = build_locks._windows_handle_lock(
        SimpleNamespace(LK_NBLCK=1, LK_UNLCK=0, locking=lock)
    )
    with (tmp_path / "shared.lock").open("w+b") as handle:
        if error_number in (errno.EACCES, errno.EAGAIN):
            assert build_locks._try_lock_handle_with(handle, windows) is False
        else:
            with pytest.raises(OSError) as raised:
                build_locks._try_lock_handle_with(handle, windows)
            assert raised.value is error
    assert len(calls) == 1


@pytest.mark.parametrize("phase", ["open", "lock"])
def test_file_lock_failures_propagate_and_release_process_reservation(
    tmp_path, monkeypatch, phase, held_before
):
    calls = []
    error = OSError(errno.EACCES if phase == "open" else errno.EIO, "fixture failure")

    def fail(_value, **_kwargs):
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
    assert _HeldLocks.now().registry == held_before.registry
    handle = build_locks._try_acquire_file_lock(tmp_path / "shared.lock")
    assert handle is not None
    build_locks._release_file_lock(handle)


def test_file_lock_serializes_when_platform_lock_is_process_reentrant(
    tmp_path: Path,
    monkeypatch,
    held_before,
) -> None:
    """The in-process authority must not depend on OS same-process semantics."""
    unrelated = build_locks._try_acquire_file_lock(tmp_path / "unrelated.lock")
    assert unrelated is not None
    try:
        monkeypatch.setattr(build_locks, "_try_lock_file_handle", lambda _handle: True)
        lock_path = tmp_path / "shared.lock"
        first = build_locks._try_acquire_file_lock(lock_path)
        assert first is not None
        assert build_locks._try_acquire_file_lock(lock_path) is None
        build_locks._release_file_lock(first)
        second = build_locks._try_acquire_file_lock(lock_path)
        assert second is not None
        build_locks._release_file_lock(second)
        assert build_locks._file_lock_is_owned(unrelated)
        assert build_locks._try_acquire_file_lock(tmp_path / "unrelated.lock") is None
        assert str(lock_path.resolve()) not in _HeldLocks.now().registry
    finally:
        build_locks._release_file_lock(unrelated)
    # Other process-owned operations (the suite sentinel among them) may end
    # concurrently. They cannot explain a newly leaked lock from this test.
    assert _HeldLocks.now().registry <= held_before.registry


def test_file_lock_releases_registry_reservation_when_platform_is_contended(
    tmp_path: Path,
    monkeypatch,
    held_before,
) -> None:
    monkeypatch.setattr(build_locks, "_try_lock_file_handle", lambda _handle: False)

    assert build_locks._try_acquire_file_lock(tmp_path / "shared.lock") is None
    assert _HeldLocks.now().registry == held_before.registry


def test_file_lock_registry_key_canonicalizes_path_aliases(tmp_path: Path) -> None:
    nested = tmp_path / "nested"
    nested.mkdir()

    assert build_locks._in_process_lock_key(nested / ".." / "shared.lock") == (
        build_locks._in_process_lock_key(tmp_path / "shared.lock")
    )


# The child-side fork reset rewrites process-wide lock state and closes every
# live handle's stream. In the test process it would also close the locks the
# session holds, and a guard thread that locks while it runs would register in
# the reset state; these cases therefore run in a fresh interpreter.
_CHILD_RESET_TIMEOUT_S = 30


def _run_child_reset_script(script: str, *args: str) -> str:
    completed = run_custody_subject_process(
        [sys.executable, "-c", script, *args],
        check=False,
        capture_output=True,
        text=True,
        timeout=_CHILD_RESET_TIMEOUT_S,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    return completed.stdout


def test_file_lock_registry_is_reinitialized_after_fork() -> None:
    script = """
import threading
from molt import file_locks as locks
prior_registry = locks._IN_PROCESS_LOCK_REGISTRY
prior_guard = locks._IN_PROCESS_LOCK_REGISTRY_GUARD
prior_registry["inherited"] = locks._InProcessLockEntry(mutex=threading.Lock(), users=1)
locks._reset_in_process_lock_registry_after_fork()
assert locks._IN_PROCESS_LOCK_REGISTRY == {}
assert locks._IN_PROCESS_LOCK_REGISTRY is not prior_registry
assert locks._IN_PROCESS_LOCK_REGISTRY_GUARD is not prior_guard
print("REINITIALIZED")
"""
    assert "REINITIALIZED" in _run_child_reset_script(script)


def test_file_lock_and_proof_cache_imports_do_not_load_cli_or_frontend():
    completed = run_custody_subject_process(
        [
            sys.executable,
            "-c",
            "import sys; from molt import file_locks; "
            "from tools.proof_queue_pkg import cargo_cache_custody; "
            "leaked = sorted(name for name in sys.modules "
            "if name == 'molt.cli' or name.startswith('molt.frontend')); "
            "assert not leaked, "
            "f'proof-cache import loaded {len(leaked)} CLI/frontend modules: {leaked[:12]}'",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert completed.returncode == 0, completed.stderr


def test_live_lock_owner_rejects_copied_handle_and_release_is_idempotent(
    tmp_path, held_before
):
    from dataclasses import replace

    handle = build_locks._try_acquire_file_lock(tmp_path / "owned.lock")
    assert handle is not None
    copied = replace(handle)
    assert build_locks._file_lock_is_owned(handle)
    assert not build_locks._file_lock_is_owned(copied)
    build_locks._release_file_lock(copied)
    assert build_locks._file_lock_is_owned(handle)
    build_locks._release_file_lock(handle)
    build_locks._release_file_lock(handle)
    assert not build_locks._file_lock_is_owned(handle)
    assert _HeldLocks.now().handles == held_before.handles
    assert _HeldLocks.now().registry == held_before.registry


def test_inherited_owner_cannot_unlock_or_drop_parent_reservation(
    tmp_path, monkeypatch
):
    handle = build_locks._try_acquire_file_lock(tmp_path / "owned.lock")
    assert handle is not None
    parent_pid = handle.owner_process_id
    with monkeypatch.context() as patch:
        patch.setattr(build_locks.os, "getpid", lambda: parent_pid + 1)
        patch.setattr(
            build_locks,
            "_unlock_file_handle",
            lambda _: pytest.fail("child unlocked parent"),
        )
        assert not build_locks._file_lock_is_owned(handle)
        build_locks._release_file_lock(handle)
        assert not handle.file.closed
        assert handle.entry.users == 1
    build_locks._release_file_lock(handle)


def test_child_fork_cleanup_closes_stream_without_unlock(tmp_path) -> None:
    script = """
import sys
from pathlib import Path
from molt import file_locks as locks
handle = locks._try_acquire_file_lock(Path(sys.argv[1]) / "owned.lock")
assert handle is not None
def forbidden(_file):
    raise AssertionError("child unlocked parent")
locks._unlock_file_handle = forbidden
locks._before_file_lock_fork()
locks._reset_in_process_lock_registry_after_fork()
assert handle.file.closed
assert handle.released
assert not locks._LIVE_FILE_LOCK_HANDLES
assert not locks._IN_PROCESS_LOCK_REGISTRY
locks._release_file_lock(handle)
locks._before_file_lock_fork()
locks._after_file_lock_fork_parent()
print("CHILD_CLEANUP_CLOSED_WITHOUT_UNLOCK")
"""
    stdout = _run_child_reset_script(script, str(tmp_path))
    assert "CHILD_CLEANUP_CLOSED_WITHOUT_UNLOCK" in stdout


def test_fork_guard_serializes_descriptor_birth_and_registration(tmp_path, monkeypatch):
    opened = threading.Event()
    proceed = threading.Event()
    fork_ready = threading.Event()
    original = build_locks._open_file_lock_handle
    result = []

    def delayed_open(path, **kwargs):
        stream = original(path, **kwargs)
        opened.set()
        assert proceed.wait(5)
        return stream

    monkeypatch.setattr(build_locks, "_open_file_lock_handle", delayed_open)
    owned_key = build_locks._in_process_lock_key(tmp_path / "owned.lock")
    acquire = threading.Thread(
        target=lambda: result.append(
            build_locks._try_acquire_file_lock(tmp_path / "owned.lock")
        )
    )
    acquire.start()
    assert opened.wait(5)

    def fork_protocol():
        build_locks._before_file_lock_fork()
        try:
            # Other threads may hold locks of their own; this lock must be
            # registered once the fork gate has drained descriptor births.
            assert owned_key in {
                handle.registry_key
                for handle in build_locks._LIVE_FILE_LOCK_HANDLES.values()
            }
            fork_ready.set()
        finally:
            build_locks._after_file_lock_fork_parent()

    waiter = threading.Thread(target=fork_protocol)
    waiter.start()
    try:
        assert not fork_ready.wait(0.05)
    finally:
        proceed.set()
        acquire.join(5)
        waiter.join(5)
    assert not acquire.is_alive() and not waiter.is_alive()
    assert fork_ready.is_set()
    build_locks._release_file_lock(result[0])


@pytest.mark.skipif(
    not hasattr(__import__("os"), "fork"),
    reason="actual POSIX fork unavailable on Windows",
)
def test_actual_fork_child_release_cannot_unlock_parent(tmp_path):
    import os

    path = tmp_path / "owned.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    pid = os.fork()
    if pid == 0:
        try:
            assert handle.file.closed
            build_locks._release_file_lock(handle)
            assert build_locks._try_acquire_file_lock(path) is None
        except BaseException:
            os._exit(1)
        os._exit(0)
    try:
        _, status = os.waitpid(pid, 0)
        assert os.waitstatus_to_exitcode(status) == 0
        # A fresh open-file-description must still contend with the parent.
        with path.open("r+b", buffering=0) as contender:
            assert not build_locks._try_lock_file_handle(contender)
    finally:
        build_locks._release_file_lock(handle)


def test_duplicate_release_returns_only_after_the_lock_is_free(tmp_path, monkeypatch):
    path = tmp_path / "owned.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    unlocking = threading.Event()
    finish_unlock = threading.Event()
    original = build_locks._unlock_file_handle

    def held_unlock(file):
        unlocking.set()
        assert finish_unlock.wait(5)
        original(file)

    monkeypatch.setattr(build_locks, "_unlock_file_handle", held_unlock)
    first = threading.Thread(target=build_locks._release_file_lock, args=(handle,))
    first.start()
    assert unlocking.wait(5)
    second_returned = threading.Event()

    def second_release():
        build_locks._release_file_lock(handle)
        second_returned.set()

    second = threading.Thread(target=second_release)
    second.start()
    try:
        # The first release has marked the handle released but still holds
        # the lock; a caller whose release returns may acquire it next.
        assert not second_returned.wait(0.2)
    finally:
        finish_unlock.set()
        first.join(5)
        second.join(5)
    assert second_returned.is_set()
    contender = build_locks._try_acquire_file_lock(path)
    assert contender is not None
    build_locks._release_file_lock(contender)


def test_concurrent_duplicate_release_drops_reservation_once(tmp_path, held_before):
    handle = build_locks._try_acquire_file_lock(tmp_path / "owned.lock")
    assert handle is not None
    ready = threading.Barrier(5)
    errors = []

    def release():
        try:
            ready.wait(5)
            build_locks._release_file_lock(handle)
        except BaseException as exc:
            errors.append(exc)

    threads = [threading.Thread(target=release) for _ in range(4)]
    for thread in threads:
        thread.start()
    ready.wait(5)
    for thread in threads:
        thread.join(5)
    assert all(not thread.is_alive() for thread in threads)
    assert not errors
    assert handle.entry.users == 0
    assert _HeldLocks.now().handles == held_before.handles
    assert _HeldLocks.now().registry == held_before.registry


def test_owned_operation_blocks_other_thread_release_without_global_mutex(tmp_path):
    path = tmp_path / "pinned.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    attempted = threading.Event()
    released = threading.Event()
    thread = None
    try:
        with build_locks._file_lock_owned_operation(handle, expected_lock_path=path):

            def revoke():
                attempted.set()
                build_locks._release_file_lock(handle)
                released.set()

            thread = threading.Thread(target=revoke)
            thread.start()
            assert attempted.wait(5)
            assert not released.wait(0.05)
            assert build_locks._file_lock_is_owned(handle)
            other = build_locks._try_acquire_file_lock(tmp_path / "independent.lock")
            assert other is not None
            build_locks._release_file_lock(other)
        assert released.wait(5)
    finally:
        build_locks._release_file_lock(handle)
        if thread is not None:
            thread.join(5)
    assert not thread.is_alive()
    assert not handle.operation_owners


def test_owned_operation_rejects_same_thread_revocation_and_unpins_failure(tmp_path):
    path = tmp_path / "pinned.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    try:
        with pytest.raises(ValueError, match="forced operation failure"):
            with build_locks._file_lock_owned_operation(
                handle, expected_lock_path=path
            ):
                with pytest.raises(RuntimeError, match="owned operation"):
                    build_locks._release_file_lock(handle)
                assert build_locks._file_lock_is_owned(handle)
                assert not handle.release_requested
                raise ValueError("forced operation failure")
        assert not handle.operation_owners
        assert build_locks._file_lock_is_owned(handle)
    finally:
        build_locks._release_file_lock(handle)


def test_current_operation_can_nest_while_external_release_waits(tmp_path):
    path = tmp_path / "nested.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    attempted = threading.Event()
    done = threading.Event()
    thread = None
    try:
        with build_locks._file_lock_owned_operation(handle, expected_lock_path=path):

            def release():
                attempted.set()
                build_locks._release_file_lock(handle)
                done.set()

            thread = threading.Thread(target=release)
            thread.start()
            assert attempted.wait(5)
            with build_locks._FILE_LOCK_LIFECYCLE_CONDITION:
                assert build_locks._FILE_LOCK_LIFECYCLE_CONDITION.wait_for(
                    lambda: handle.release_requested, timeout=5
                )
            with build_locks._file_lock_owned_operation(
                handle, expected_lock_path=path
            ):
                assert sum(handle.operation_owners.values()) == 2
            assert not done.is_set()
        assert done.wait(5)
    finally:
        build_locks._release_file_lock(handle)
        if thread is not None:
            thread.join(5)
    assert not thread.is_alive()


@pytest.mark.parametrize("event", ["os.fork", "os.forkpty"])
def test_atomic_mutation_rejects_fork_audit_and_restores_scope(event):
    import sys

    with build_locks._file_lock_atomic_mutation("test descriptor transition"):
        with pytest.raises(RuntimeError, match="must be deferred"):
            sys.audit(event)
        sys.audit("molt.unrelated.operation")
    sys.audit(event)


def test_fork_protocol_nested_admission_remains_closed_until_all_callbacks_end():
    import sys

    build_locks._enter_file_lock_fork_protocol()
    build_locks._enter_file_lock_fork_protocol()
    try:
        with pytest.raises(RuntimeError, match="fork lifecycle callback"):
            sys.audit("os.fork")
        build_locks._leave_file_lock_fork_protocol()
        with pytest.raises(RuntimeError, match="fork lifecycle callback"):
            sys.audit("os.fork")
    finally:
        build_locks._leave_file_lock_fork_protocol()
    sys.audit("os.fork")


def test_pinned_operation_fork_audit_blocked_but_quiescent_handle_allowed(tmp_path):
    import sys

    path = tmp_path / "pinned.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    try:
        sys.audit("os.fork")
        with build_locks._file_lock_owned_operation(handle, expected_lock_path=path):
            with pytest.raises(RuntimeError, match="must be deferred"):
                sys.audit("os.fork")
        sys.audit("os.fork")
    finally:
        build_locks._release_file_lock(handle)


def test_blocked_audit_registration_fails_import_closed():
    import sys

    code = """import sys
import runpy

def block(event, args):
    if event == "sys.addaudithook":
        raise RuntimeError("registration denied")
sys.addaudithook(block)
runpy.run_path(sys.argv[1], run_name="blocked_file_lock_registration")
"""
    result = run_custody_subject_process(
        [sys.executable, "-c", code, build_locks.__file__],
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert result.returncode != 0
    assert "pre-fork admission audit hook registration was blocked" in result.stderr


@pytest.mark.skipif(
    not hasattr(__import__("os"), "fork"),
    reason="actual POSIX fork unavailable on Windows",
)
def test_actual_python_fork_rejected_before_syscall_during_pin(tmp_path):
    import os

    path = tmp_path / "fork-pin.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    try:
        with build_locks._file_lock_owned_operation(handle, expected_lock_path=path):
            with pytest.raises(RuntimeError, match="must be deferred"):
                pid = os.fork()
                if pid == 0:
                    os._exit(99)
                os.waitpid(pid, 0)
                raise AssertionError("fork syscall unexpectedly admitted during pin")
    finally:
        build_locks._release_file_lock(handle)


@pytest.mark.parametrize("transition", ["birth", "close"])
def test_descriptor_io_does_not_hold_global_mutex_and_fork_waits(
    tmp_path, monkeypatch, transition
):
    path = tmp_path / "transition.lock"
    entered, proceed, fork_ready = (
        threading.Event(),
        threading.Event(),
        threading.Event(),
    )
    result, errors = [], []
    handle = build_locks._try_acquire_file_lock(path) if transition == "close" else None
    if transition == "birth":
        original = build_locks._open_file_lock_handle

        def delayed(candidate, **kwargs):
            stream = original(candidate, **kwargs)
            if candidate == path:
                entered.set()
                assert proceed.wait(5)
            return stream

        monkeypatch.setattr(build_locks, "_open_file_lock_handle", delayed)
    else:
        original = build_locks._unlock_file_handle

        def delayed(stream):
            if stream is handle.file:
                entered.set()
                assert proceed.wait(5)
            original(stream)

        monkeypatch.setattr(build_locks, "_unlock_file_handle", delayed)

    def mutate():
        try:
            if transition == "birth":
                result.append(build_locks._try_acquire_file_lock(path))
            else:
                build_locks._release_file_lock(handle)
        except BaseException as exc:
            errors.append(exc)

    def fork_protocol():
        build_locks._before_file_lock_fork()
        try:
            fork_ready.set()
        finally:
            build_locks._after_file_lock_fork_parent()

    worker = threading.Thread(target=mutate)
    waiter = threading.Thread(target=fork_protocol)
    worker.start()
    try:
        assert entered.wait(5)
        other = build_locks._try_acquire_file_lock(tmp_path / "independent.lock")
        assert other is not None
        build_locks._release_file_lock(other)
        waiter.start()
        assert not fork_ready.wait(0.05)
    finally:
        proceed.set()
        worker.join(5)
        if waiter.ident is not None:
            waiter.join(5)
        if result:
            build_locks._release_file_lock(result[0])
        if handle is not None:
            build_locks._release_file_lock(handle)
    assert not worker.is_alive() and not waiter.is_alive()
    assert not errors
    assert fork_ready.is_set()
    assert not build_locks._FILE_LOCK_DESCRIPTOR_ACTIONS


@pytest.mark.parametrize("operation", ["acquire", "release", "owned", "check"])
def test_fork_callback_reentrant_lock_operations_fail_before_mutex(tmp_path, operation):
    path = tmp_path / "callback.lock"
    handle = build_locks._try_acquire_file_lock(path)
    assert handle is not None
    build_locks._before_file_lock_fork()
    try:
        with pytest.raises(RuntimeError, match="fork lifecycle callbacks end"):
            if operation == "acquire":
                build_locks._try_acquire_file_lock(tmp_path / "other.lock")
            elif operation == "release":
                build_locks._release_file_lock(handle)
            elif operation == "check":
                build_locks._file_lock_is_owned(handle)
            else:
                with build_locks._file_lock_owned_operation(
                    handle, expected_lock_path=path
                ):
                    pytest.fail("reentrant operation admitted")
    finally:
        build_locks._after_file_lock_fork_parent()
        build_locks._release_file_lock(handle)


def test_profile_close_callback_cannot_deadlock_child_lock_reset():

    code = """import sys,tempfile
from pathlib import Path
from molt import file_locks as locks
handle=locks._try_acquire_file_lock(Path(tempfile.mkdtemp())/'owned.lock')
locks._before_file_lock_fork()
rejected=[]
def profile(frame,event,arg):
    if event=='c_call' and getattr(arg,'__name__','')=='close':
        try:
            locks._try_acquire_file_lock(Path(tempfile.mkdtemp())/'another.lock')
        except RuntimeError as exc:
            rejected.append(str(exc))
sys.setprofile(profile)
locks._reset_in_process_lock_registry_after_fork()
sys.setprofile(None)
assert rejected and all('fork lifecycle callbacks end' in x for x in rejected)
assert locks._FILE_LOCK_ATOMIC_LOCAL.__dict__ == {}
print('CALLBACK_REENTRY_REJECTED')
"""
    result = run_custody_subject_process(
        [sys.executable, "-c", code], capture_output=True, text=True, timeout=15
    )
    assert result.returncode == 0, result.stderr
    assert "CALLBACK_REENTRY_REJECTED" in result.stdout


@pytest.mark.parametrize("catch_callback_error", [False, True])
def test_child_reset_callback_failure_is_complete_or_explicitly_fail_closed(
    catch_callback_error,
):

    code = """import sys,tempfile
from pathlib import Path
from molt import file_locks as locks
root=Path(tempfile.mkdtemp())
handles=[locks._try_acquire_file_lock(root/f'{i}.lock') for i in range(2)]
locks._before_file_lock_fork()
def profile(frame,event,arg):
    if event=='c_call' and getattr(arg,'__name__','')=='close':
        if CATCH:
            try: locks._try_acquire_file_lock(root/'reentry.lock')
            except RuntimeError: pass
        else:
            locks._try_acquire_file_lock(root/'reentry.lock')
sys.setprofile(profile)
locks._reset_in_process_lock_registry_after_fork()
sys.setprofile(None)
assert all(h.released for h in handles)
assert not locks._LIVE_FILE_LOCK_HANDLES
assert not locks._IN_PROCESS_LOCK_REGISTRY
assert not locks._FILE_LOCK_DESCRIPTOR_ACTIONS
assert locks._FILE_LOCK_ATOMIC_LOCAL.__dict__ == {}
if CATCH:
    assert all(h.file.closed for h in handles)
    new=locks._try_acquire_file_lock(root/'next.lock')
    assert new is not None
    locks._release_file_lock(new)
else:
    assert locks._FILE_LOCK_FORK_CLEANUP_ERROR is not None
    prior_failure = locks._FILE_LOCK_FORK_CLEANUP_ERROR
    locks._reset_in_process_lock_registry_after_fork()
    assert locks._FILE_LOCK_FORK_CLEANUP_ERROR == prior_failure
    try: locks._try_acquire_file_lock(root/'next.lock')
    except RuntimeError as exc: assert 'child custody is unavailable' in str(exc)
    else: raise AssertionError('partial cleanup admitted child custody')
    for h in handles: h.file.close()
print('RESET_COMPLETE_OR_CLOSED')
""".replace("CATCH", repr(catch_callback_error))
    result = run_custody_subject_process(
        [sys.executable, "-c", code], capture_output=True, text=True, timeout=15
    )
    assert result.returncode == 0, result.stderr
    assert "RESET_COMPLETE_OR_CLOSED" in result.stdout


def test_observing_absent_lock_creates_no_directory_or_registry_entry(tmp_path):
    path = tmp_path / "absent-control" / "owner.lock"
    baseline = dict(build_locks._IN_PROCESS_LOCK_REGISTRY)
    assert build_locks._in_process_lock_key(path) not in baseline
    with pytest.raises(FileNotFoundError):
        build_locks._try_acquire_file_lock(path, create=False)
    assert not path.parent.exists()
    assert build_locks._IN_PROCESS_LOCK_REGISTRY == baseline


def test_observing_existing_lock_preserves_bytes_inode_and_mtime(tmp_path):
    path = tmp_path / "owner.lock"
    path.write_bytes(b"existing ownership state")
    baseline = dict(build_locks._IN_PROCESS_LOCK_REGISTRY)
    assert build_locks._in_process_lock_key(path) not in baseline
    before = path.stat()
    handle = build_locks._try_acquire_file_lock(path, create=False)
    assert handle is not None
    try:
        assert build_locks._try_acquire_file_lock(path, create=False) is None
    finally:
        build_locks._release_file_lock(handle)
    after = path.stat()
    assert (after.st_dev, after.st_ino, after.st_mtime_ns) == (
        before.st_dev,
        before.st_ino,
        before.st_mtime_ns,
    )
    assert path.read_bytes() == b"existing ownership state"
    assert build_locks._IN_PROCESS_LOCK_REGISTRY == baseline


def test_shared_readers_contend_with_exclusive_in_one_process(tmp_path, held_before):
    path = tmp_path / "readers.lock"
    first = build_locks._try_acquire_file_lock(path, shared=True)
    second = build_locks._try_acquire_file_lock(path, shared=True)
    assert first is not None and second is not None
    try:
        assert build_locks._try_acquire_file_lock(path) is None
        build_locks._release_file_lock(first)
        assert build_locks._try_acquire_file_lock(path) is None
    finally:
        build_locks._release_file_lock(first)
        build_locks._release_file_lock(second)
    writer = build_locks._try_acquire_file_lock(path)
    assert writer is not None
    try:
        assert build_locks._try_acquire_file_lock(path, shared=True) is None
    finally:
        build_locks._release_file_lock(writer)
    assert path.read_bytes() == b""
    assert _HeldLocks.now() == held_before


def test_shared_handle_does_not_authorize_exclusive_owned_operation(
    tmp_path, held_before
):
    path = tmp_path / "readers.lock"
    handle = build_locks._try_acquire_file_lock(path, shared=True)
    assert handle is not None
    try:
        with pytest.raises(ValueError, match="exact live ownership"):
            with build_locks._file_lock_owned_operation(
                handle, expected_lock_path=path
            ):
                pytest.fail("a reader must never authorize a cache mutation")
        with build_locks._file_lock_owned_operation(
            handle, expected_lock_path=path, shared=True
        ):
            assert build_locks._file_lock_is_owned(handle)
    finally:
        build_locks._release_file_lock(handle)
    assert _HeldLocks.now() == held_before


def test_shared_readers_and_writer_contend_across_processes(tmp_path, held_before):
    path = tmp_path / "readers.lock"
    holder = build_locks._try_acquire_file_lock(path, shared=True)
    assert holder is not None
    try:
        completed = run_custody_subject_process(
            [
                sys.executable,
                "-c",
                "from pathlib import Path; import sys; "
                "from molt import file_locks as f; p=Path(sys.argv[1]); "
                "reader=f._try_acquire_file_lock(p, shared=True); assert reader is not None; "
                "assert f._try_acquire_file_lock(p) is None; "
                "f._release_file_lock(reader); print('reader-admitted-writer-refused')",
                str(path),
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=15,
        )
        assert completed.returncode == 0, completed.stderr
        assert completed.stdout.strip() == "reader-admitted-writer-refused"
    finally:
        build_locks._release_file_lock(holder)
    assert _HeldLocks.now() == held_before


_HOLD_SCRIPT = """
import sys, time
from pathlib import Path
from molt.cli.build_locks import held_build_lock

path, release = Path(sys.argv[1]), Path(sys.argv[2])
with held_build_lock(path, purpose="test"):
    print("held", flush=True)
    deadline = time.monotonic() + 30
    while not release.exists() and time.monotonic() < deadline:
        time.sleep(0.01)
"""


def _start_holder(lock_path: Path, release: Path):
    import subprocess

    from tests.process_guard_common import start_owned_test_process

    holder = start_owned_test_process(
        [sys.executable, "-c", _HOLD_SCRIPT, str(lock_path), str(release)],
        stdout=subprocess.PIPE,
        text=True,
    )
    assert holder.stdout is not None
    assert holder.stdout.readline().strip() == "held"
    return holder


def test_a_waiter_names_the_holding_process_and_waits_while_it_holds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, held_before
) -> None:
    """Cargo's "Blocking waiting for file lock", with the holder Bazel names."""
    from tests.process_guard_common import close_owned_test_process

    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    lock_path = tmp_path / "build.lock"
    release = tmp_path / "release"
    holder = _start_holder(lock_path, release)
    notices: list[str] = []
    try:
        recorded = cli_build_locks.read_lock_holder(lock_path)
        assert recorded is not None and recorded.pid == holder.pid
        timer = threading.Timer(1.0, release.touch)
        timer.start()
        started = time.monotonic()
        with cli_build_locks.held_build_lock(
            lock_path, purpose="test", notice=notices.append
        ):
            waited = time.monotonic() - started
            # The waiter now holds the lock and names itself for the next one.
            assert cli_build_locks.read_lock_holder(lock_path).pid == os.getpid()
        timer.join()
        assert holder.wait(timeout=30) == 0
    finally:
        release.touch()
        close_owned_test_process(holder)
    assert waited >= 0.5
    assert len(notices) == 1
    assert str(lock_path) in notices[0] and f"pid {holder.pid}," in notices[0]
    assert "until it is released" in notices[0]
    # A released lock names no holder, and the lock file stays empty metadata.
    assert cli_build_locks.read_lock_holder(lock_path) is None
    assert lock_path.read_bytes() == b""
    assert _HeldLocks.now().registry == held_before.registry


def test_an_operator_bound_fails_the_wait_naming_the_holder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tests.process_guard_common import close_owned_test_process

    monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", "0.2")
    lock_path = tmp_path / "build.lock"
    release = tmp_path / "release"
    holder = _start_holder(lock_path, release)
    notices: list[str] = []
    try:
        with pytest.raises(cli_build_locks.BuildLockAcquisitionError) as raised:
            with cli_build_locks.held_build_lock(
                lock_path, purpose="test", notice=notices.append
            ):
                pytest.fail("a held lock admitted a second holder")
    finally:
        release.touch()
        close_owned_test_process(holder)
    assert "0.2s" in notices[0] and "MOLT_BUILD_LOCK_TIMEOUT" in notices[0]
    message = str(raised.value)
    assert str(lock_path) in message and f"pid {holder.pid}," in message


def test_a_record_naming_an_exited_process_names_no_holder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An exited process has no observable start time either (review v2, m2).

    Its record must not name it as the live holder. Where no other process's
    start time is observable (Windows), a running pid is named with that
    caveat, because the pid may have been reused.
    """
    import json

    from molt import process_model
    from tests.process_guard_common import (
        close_owned_test_process,
        start_owned_test_process,
    )

    child = start_owned_test_process([sys.executable, "-c", "pass"])
    child.wait(timeout=60)
    close_owned_test_process(child)
    lock_path = tmp_path / "build.lock"
    record = cli_build_locks.lock_holder_path(lock_path)

    def write(pid: int, started: int | None) -> None:
        record.write_text(
            json.dumps(
                {"schema": "molt.lock-holder.v1", "pid": pid, "started_at_ns": started}
            ),
            encoding="utf-8",
        )

    write(child.pid, 1)
    assert cli_build_locks.read_lock_holder(lock_path) is None

    monkeypatch.setattr(process_model, "process_started_at_ns", lambda pid: None)
    write(os.getpid(), None)
    holder = cli_build_locks.read_lock_holder(lock_path)
    assert holder == cli_build_locks.LockHolder(os.getpid(), None, start_verified=False)
    assert "not observable" in holder.describe()
    write(child.pid, None)
    assert cli_build_locks.read_lock_holder(lock_path) is None


def test_only_a_timeout_reads_as_a_lock_wait_timeout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The waiting notice names the knob too (review v2, m3).

    A build that waited and then failed for another reason is not a lock
    timeout; only the line a wait that gave up writes is.
    """
    from molt import file_locks
    from tests.process_guard_common import close_owned_test_process

    monkeypatch.setenv(file_locks.LOCK_WAIT_ENV, "0.2")
    lock_path = tmp_path / "build.lock"
    release = tmp_path / "release"
    holder = _start_holder(lock_path, release)
    notices: list[str] = []
    try:
        with pytest.raises(cli_build_locks.BuildLockAcquisitionError) as raised:
            with cli_build_locks.held_build_lock(
                lock_path, purpose="test", notice=notices.append
            ):
                pass
    finally:
        release.touch()
        close_owned_test_process(holder)
    (notice,) = notices
    assert file_locks.LOCK_WAIT_ENV in notice
    assert not file_locks.is_lock_wait_timeout(notice + "\nerror: link failed")
    assert file_locks.is_lock_wait_timeout(f"notice\nmolt: {raised.value}\n")
    # The native runtime build prints the timeout inside its stage line, with
    # its evidence path after it (review v3, A-M1): compile_progress retries it.
    from molt.cli import runtime_native_build
    from tools import compile_progress

    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "build-state"))
    runtime_native_build._record_native_runtime_failure(
        None,
        project_root=tmp_path,
        stage="build-lock",
        summary=f"Native runtime build lock acquisition failed: {raised.value}",
        emit_diagnostic=True,
    )
    printed = capsys.readouterr().err
    assert "Evidence: " in printed and printed.rstrip().endswith(".json"), printed
    assert compile_progress._is_retryable_failure(2, False, printed) == (
        "build_lock_timeout"
    )
    assert compile_progress._is_retryable_failure(2, False, notice) is None


def test_a_record_its_process_no_longer_backs_names_no_holder(tmp_path: Path) -> None:
    """A holder that died before clearing its record must not be named."""
    import json

    lock_path = tmp_path / "build.lock"
    record = cli_build_locks.lock_holder_path(lock_path)
    live = cli_build_locks._own_start_marker()
    for pid, started in ((os.getpid(), live), (os.getpid(), (live or 0) + 1)):
        record.write_text(
            json.dumps(
                {"schema": "molt.lock-holder.v1", "pid": pid, "started_at_ns": started}
            ),
            encoding="utf-8",
        )
        holder = cli_build_locks.read_lock_holder(lock_path)
        if started == live:
            assert holder == cli_build_locks.LockHolder(pid, started)
        elif live is not None:
            # Another process now has the pid: the record is stale.
            assert holder is None
    for garbage in (b"", b"{", b'{"schema": "other", "pid": 1}', b"x" * 600):
        record.write_bytes(garbage)
        assert cli_build_locks.read_lock_holder(lock_path) is None
    # A dead holder's longer record never leaks into the next holder's.
    record.write_bytes(b"x" * 400)
    with cli_build_locks.held_build_lock(lock_path, purpose="test"):
        holder = cli_build_locks.read_lock_holder(lock_path)
        assert holder is not None and holder.pid == os.getpid()
    assert record.read_bytes() == b""


def test_every_build_lock_waits_by_one_policy(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    assert build_locks.lock_wait_s() is None
    for raw, expected in (("", None), ("junk", None), ("0", None), ("-1", None)):
        monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", raw)
        assert build_locks.lock_wait_s() is expected
    monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", "2.5")
    assert build_locks.lock_wait_s() == 2.5


def test_a_shared_cache_lock_waiter_waits_while_its_holder_holds_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys
) -> None:
    """The shared cache lock follows the one wait policy: no 300 s default."""
    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    held, release = threading.Event(), threading.Event()

    def holder() -> None:
        with build_locks._shared_cache_lock("compile.test", cache_root=tmp_path):
            held.set()
            release.wait(timeout=30)

    thread = threading.Thread(target=holder)
    thread.start()
    assert held.wait(timeout=10)
    threading.Timer(0.5, release.set).start()
    started = time.monotonic()
    try:
        with build_locks._shared_cache_lock("compile.test", cache_root=tmp_path):
            waited = time.monotonic() - started
    finally:
        release.set()
        thread.join(timeout=30)
    assert waited >= 0.3
    assert "Blocking waiting for the shared cache lock" in capsys.readouterr().err
    monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", "0.1")
    release.clear()
    held.clear()
    thread = threading.Thread(target=holder)
    thread.start()
    assert held.wait(timeout=10)
    try:
        with pytest.raises(
            build_locks.FileLockTimeout, match="MOLT_BUILD_LOCK_TIMEOUT"
        ):
            with build_locks._shared_cache_lock("compile.test", cache_root=tmp_path):
                pytest.fail("a held cache lock admitted a second holder")
    finally:
        release.set()
        thread.join(timeout=30)
