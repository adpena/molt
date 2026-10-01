from __future__ import annotations
import threading
import time
from types import SimpleNamespace
from tools.memory_guard_core import process_custody


def test_reserved_reap_cannot_signal_reused_pid_before_publication(monkeypatch):
    reaped = threading.Event()
    publish = threading.Event()
    attempted_signal = threading.Event()
    kills = []

    def wait4(pid, flags):
        reaped.set()
        assert publish.wait(2)
        return pid, 0, SimpleNamespace(ru_maxrss=123)

    os_api = SimpleNamespace(
        name="posix",
        waitid=lambda *args: None,
        wait4=wait4,
        WNOWAIT=1,
        WEXITED=2,
        P_PID=3,
        WNOHANG=4,
        waitstatus_to_exitcode=lambda status: 0,
        kill=lambda *args: kills.append(args),
    )
    monkeypatch.setattr(process_custody, "os", os_api)
    proc = SimpleNamespace(
        pid=123,
        args=["owned"],
        returncode=None,
        wait=lambda: None,
        _waitpid_lock=threading.Lock(),
    )
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert reaped.wait(2)

    def signal():
        attempted_signal.set()
        proc.send_signal(15)

    worker = threading.Thread(target=signal)
    worker.start()
    assert attempted_signal.wait(2)
    assert not kills and proc.poll() is None
    publish.set()
    worker.join(2)
    assert not worker.is_alive() and proc.wait(timeout=2) == 0
    assert not kills and clock.done.is_set()


def test_blocking_reserved_exit_wait_does_not_block_owned_signal(monkeypatch):
    awaiting = threading.Event()
    killed = threading.Event()
    calls = []

    def waitid(*args):
        awaiting.set()
        assert killed.wait(2)

    def kill(pid, sig):
        calls.append((pid, sig))
        killed.set()

    os_api = SimpleNamespace(
        name="posix",
        waitid=waitid,
        wait4=lambda pid, flags: (pid, 0, SimpleNamespace(ru_maxrss=123)),
        WNOWAIT=1,
        WEXITED=2,
        P_PID=3,
        WNOHANG=4,
        waitstatus_to_exitcode=lambda status: 0,
        kill=kill,
    )
    monkeypatch.setattr(process_custody, "os", os_api)
    proc = SimpleNamespace(
        pid=123,
        args=["owned"],
        returncode=None,
        wait=lambda: None,
        _waitpid_lock=threading.Lock(),
    )
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert awaiting.wait(2)
    proc.send_signal(15)
    assert proc.wait(timeout=2) == 0 and calls == [(123, 15)]
    proc.send_signal(9)
    assert calls == [(123, 15)] and clock.done.is_set()


def test_kqueue_registration_race_only_reaps_owned_reserved_child(monkeypatch):
    import errno

    closed = []
    kills = []
    waits = []

    class Queue:
        def control(self, *args):
            raise ProcessLookupError(errno.ESRCH, "child already exited")

        def close(self):
            closed.append(True)

    def wait4(pid, flags):
        waits.append(flags)
        return (
            (0, 0, None) if len(waits) == 1 else (pid, 0, SimpleNamespace(ru_maxrss=64))
        )

    monkeypatch.setattr(
        process_custody,
        "os",
        SimpleNamespace(
            name="posix",
            wait4=wait4,
            WNOHANG=1,
            waitstatus_to_exitcode=lambda status: 0,
            kill=lambda *args: kills.append(args),
        ),
    )
    monkeypatch.setattr(
        process_custody,
        "select",
        SimpleNamespace(
            kqueue=Queue,
            kevent=lambda *args, **kwargs: kwargs,
            KQ_FILTER_PROC=1,
            KQ_EV_ADD=2,
            KQ_EV_ONESHOT=4,
            KQ_NOTE_EXIT=8,
            KQ_EV_ERROR=16,
        ),
    )
    proc = SimpleNamespace(
        pid=123,
        args=["owned"],
        returncode=None,
        wait=lambda: None,
        _waitpid_lock=threading.Lock(),
    )
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert proc.wait(timeout=2) == 0 and clock.posix_kqueue
    proc.send_signal(15)
    assert waits == [1, 1] and closed == [True] and not kills


def test_kqueue_unknown_registration_failure_disables_signaling(monkeypatch):
    import errno

    closed = []
    kills = []

    class Queue:
        def control(self, *args):
            raise PermissionError(errno.EPERM, "unknown")

        def close(self):
            closed.append(True)

    monkeypatch.setattr(
        process_custody,
        "os",
        SimpleNamespace(
            name="posix",
            wait4=lambda *args: (0, 0, None),
            WNOHANG=1,
            waitstatus_to_exitcode=lambda status: 0,
            kill=lambda *args: kills.append(args),
        ),
    )
    monkeypatch.setattr(
        process_custody,
        "select",
        SimpleNamespace(
            kqueue=Queue,
            kevent=lambda *args, **kwargs: kwargs,
            KQ_FILTER_PROC=1,
            KQ_EV_ADD=2,
            KQ_EV_ONESHOT=4,
            KQ_NOTE_EXIT=8,
            KQ_EV_ERROR=16,
        ),
    )
    proc = SimpleNamespace(
        pid=123,
        args=["owned"],
        returncode=None,
        wait=lambda: None,
        _waitpid_lock=threading.Lock(),
    )
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert clock.done.wait(2)
    import pytest

    with pytest.raises(PermissionError):
        proc.wait(timeout=1)
    with pytest.raises(PermissionError):
        proc.send_signal(15)
    assert closed == [True] and not kills and clock.finished is None


def test_kqueue_blocking_exit_watch_does_not_hold_signal_lock(monkeypatch):
    awaiting = threading.Event()
    killed = threading.Event()
    waits = []

    class Queue:
        def control(self, *args):
            awaiting.set()
            assert killed.wait(2)
            return [SimpleNamespace(ident=123)]

        def close(self):
            pass

    def wait4(pid, flags):
        waits.append(flags)
        return (
            (0, 0, None) if len(waits) == 1 else (pid, 0, SimpleNamespace(ru_maxrss=64))
        )

    monkeypatch.setattr(
        process_custody,
        "os",
        SimpleNamespace(
            name="posix",
            wait4=wait4,
            WNOHANG=1,
            waitstatus_to_exitcode=lambda status: 0,
            kill=lambda *args: killed.set(),
        ),
    )
    monkeypatch.setattr(
        process_custody,
        "select",
        SimpleNamespace(
            kqueue=Queue,
            kevent=lambda *args, **kwargs: kwargs,
            KQ_FILTER_PROC=1,
            KQ_EV_ADD=2,
            KQ_EV_ONESHOT=4,
            KQ_NOTE_EXIT=8,
            KQ_EV_ERROR=16,
        ),
    )
    proc = SimpleNamespace(
        pid=123,
        args=["owned"],
        returncode=None,
        wait=lambda: None,
        _waitpid_lock=threading.Lock(),
    )
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert awaiting.wait(2)
    proc.send_signal(15)
    assert proc.wait(timeout=2) == 0 and clock.finished is not None


def test_actual_posix_owned_signal_keeps_child_reserved_until_reap():
    import os
    import sys
    import subprocess
    import pytest

    if os.name != "posix":
        pytest.skip("Native POSIX exit/signal coordinate; Windows mocks are not credit")
    proc = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(0.25)"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    try:
        assert clock.posix_reserved_wait, (
            "Missing native unreaped exit observation capability"
        )
        proc.terminate()
        assert proc.wait(timeout=3) is not None
        assert clock.finished is not None and clock.usage is not None
        proc.send_signal(15)  # Closed owned capability must perform no PID signal.
    finally:
        if proc.returncode is None and clock.error is None:
            proc.terminate()
            proc.wait(timeout=3)


def test_locked_reap_failure_published_before_unwind_sender(monkeypatch):
    import pytest

    for path in ("waitid", "commit"):
        released = threading.Event()
        continue_unwind = threading.Event()
        actual_lock = threading.Lock()
        failure = ChildProcessError("owned reap failed")
        kills = []
        sender_errors = []
        waiter_errors = []

        class PausedUnwindLock:
            def __enter__(self):
                actual_lock.acquire()
                return self

            def __exit__(self, exc_type, exc, tb):
                actual_lock.release()
                if exc is failure and threading.current_thread() is worker:
                    released.set()
                    assert continue_unwind.wait(2)

        def wait4(*args):
            raise failure

        monkeypatch.setattr(
            process_custody,
            "os",
            SimpleNamespace(
                waitid=lambda *args: None,
                P_PID=1,
                WEXITED=2,
                WNOWAIT=4,
                WNOHANG=8,
                wait4=wait4,
                kill=lambda *args: kills.append(args),
            ),
        )
        clock = process_custody.ChildExecutionClock.__new__(
            process_custody.ChildExecutionClock
        )
        clock.proc = SimpleNamespace(pid=123, returncode=None)
        clock.reap_signal_lock = PausedUnwindLock()
        clock.error = None
        clock.finished = None
        clock.usage = None
        clock.done = threading.Event()
        clock.posix_kqueue = False
        clock.posix_waitid = True

        def reap():
            try:
                if path == "waitid":
                    clock._reap()
                else:
                    clock._commit_reserved_exit(finished=None)
            except BaseException as exc:
                waiter_errors.append(exc)

        worker = threading.Thread(target=reap)
        worker.start()
        assert released.wait(2)
        try:
            with pytest.raises(ChildProcessError) as caught:
                clock.send_signal(15)
            sender_errors.append(caught.value)
            assert clock.error is failure and not kills
        finally:
            continue_unwind.set()
            worker.join(2)
        assert not worker.is_alive() and sender_errors == [failure]
        assert waiter_errors == ([] if path == "waitid" else [failure])
