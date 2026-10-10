from __future__ import annotations
import threading
import time
from types import SimpleNamespace
from tools.memory_guard_core import process_custody


def _kqueue_api(queue_type):
    return SimpleNamespace(
        kqueue=queue_type,
        kevent=lambda *args, **kwargs: kwargs,
        KQ_FILTER_PROC=1,
        KQ_EV_ADD=2,
        KQ_EV_ONESHOT=4,
        KQ_NOTE_EXIT=8,
        KQ_EV_ERROR=16,
    )


def _exit_event(pid, *, flags=0, data=0):
    # A kevent record: ident, flags (EV_ERROR=16 here), NOTE_EXIT fflags, data.
    return SimpleNamespace(ident=pid, flags=flags, fflags=8, data=data)


def _owned_handle():
    return SimpleNamespace(
        pid=123,
        args=["owned"],
        returncode=None,
        wait=lambda: None,
        _waitpid_lock=threading.Lock(),
    )


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
        # ESRCH at attach means exiting; WNOHANG cannot reap until the child
        # is a zombie, so only the blocking reap (flags 0) succeeds.
        waits.append(flags)
        return (pid, 0, SimpleNamespace(ru_maxrss=64)) if flags == 0 else (0, 0, None)

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
    monkeypatch.setattr(process_custody, "select", _kqueue_api(Queue))
    proc = _owned_handle()
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert proc.wait(timeout=2) == 0 and clock.posix_kqueue
    proc.send_signal(15)
    assert waits == [0] and closed == [True] and not kills


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
            return [_exit_event(123)]

        def close(self):
            pass

    def wait4(pid, flags):
        waits.append(flags)
        return (pid, 0, SimpleNamespace(ru_maxrss=64)) if flags == 0 else (0, 0, None)

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
    assert waits == [0]


def test_kqueue_exit_notification_before_zombie_waits_for_reapable_child(
    monkeypatch,
):
    # XNU posts NOTE_EXIT from proc_exit() before the child is a zombie; under
    # load wait4(WNOHANG) still returns 0 then. The reap must wait for the
    # zombie, never report the certain exit as unreapable.
    waits = []

    class Queue:
        def control(self, *args):
            return [_exit_event(123)]

        def close(self):
            pass

    def wait4(pid, flags):
        waits.append(flags)
        return (pid, 0, SimpleNamespace(ru_maxrss=64)) if flags == 0 else (0, 0, None)

    monkeypatch.setattr(
        process_custody,
        "os",
        SimpleNamespace(
            name="posix",
            wait4=wait4,
            WNOHANG=1,
            waitstatus_to_exitcode=lambda status: 0,
            kill=lambda *args: None,
        ),
    )
    monkeypatch.setattr(process_custody, "select", _kqueue_api(Queue))
    proc = _owned_handle()
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert proc.wait(timeout=2) == 0
    assert clock.error is None and clock.finished is not None
    assert clock.usage is not None and clock.usage.max_rss_kb > 0
    assert waits == [0]


def test_kqueue_error_event_is_registration_evidence_not_exit(monkeypatch):
    import errno

    import pytest

    for code in (errno.ESRCH, errno.EPERM):
        waits = []

        class Queue:
            def control(self, *args):
                # kevent returns a changelist error as an EV_ERROR record.
                return [_exit_event(123, flags=16, data=code)]

            def close(self):
                pass

        def wait4(pid, flags):
            waits.append(flags)
            return (
                (pid, 0, SimpleNamespace(ru_maxrss=64)) if flags == 0 else (0, 0, None)
            )

        monkeypatch.setattr(
            process_custody,
            "os",
            SimpleNamespace(
                name="posix",
                wait4=wait4,
                WNOHANG=1,
                waitstatus_to_exitcode=lambda status: 0,
                kill=lambda *args: None,
            ),
        )
        monkeypatch.setattr(process_custody, "select", _kqueue_api(Queue))
        proc = _owned_handle()
        clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
        assert clock.done.wait(2)
        if code == errno.ESRCH:
            # The unreaped child keeps its PID: ESRCH is exit evidence.
            assert proc.wait(timeout=1) == 0 and waits == [0]
        else:
            # Unknown registration failure: never block on a possibly live
            # child, and never mistake the record for an exit.
            with pytest.raises(OSError) as caught:
                proc.wait(timeout=1)
            assert caught.value.errno == errno.EPERM
            assert waits == [1] and clock.finished is None


def test_reaper_failure_never_raises_from_popen_lifecycle(monkeypatch):
    import errno
    import sys

    import pytest

    class Queue:
        def control(self, *args):
            return [_exit_event(123)]

        def close(self):
            pass

    def wait4(pid, flags):
        if flags == 0:
            raise ChildProcessError(errno.ECHILD, "No child processes")
        return (0, 0, None)

    monkeypatch.setattr(
        process_custody,
        "os",
        SimpleNamespace(
            name="posix",
            wait4=wait4,
            WNOHANG=1,
            waitstatus_to_exitcode=lambda status: 0,
            kill=lambda *args: None,
        ),
    )
    monkeypatch.setattr(process_custody, "select", _kqueue_api(Queue))
    proc = _owned_handle()
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert clock.done.wait(2)
    # Popen.__del__ and subprocess._cleanup reach the reaper only here.
    assert proc._internal_poll(_deadstate=sys.maxsize) is None
    with pytest.raises(ChildProcessError, match="reaped outside its"):
        proc.poll()


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
        clock.exit_census = None
        clock.exit_census_samples = None
        clock.exit_census_error = None
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


def _census_os(events, *, waitid=True):
    def wait4(pid, flags):
        # After waitid(WNOWAIT) the child is a zombie, so WNOHANG reaps it.
        # A kqueue NOTE_EXIT can precede the zombie: only flags 0 reaps then.
        events.append(f"wait4:{flags}")
        reaped = waitid or flags == 0
        return (pid, 0, SimpleNamespace(ru_maxrss=64)) if reaped else (0, 0, None)

    api = SimpleNamespace(
        name="posix",
        wait4=wait4,
        WNOHANG=1,
        waitstatus_to_exitcode=lambda status: 0,
        kill=lambda *args: None,
    )
    if waitid:
        api.waitid = lambda *args: events.append("waitid")
        api.WNOWAIT = 2
        api.WEXITED = 4
        api.P_PID = 8
    return api


def test_waitid_exit_census_precedes_the_reap(monkeypatch):
    # Until the reap the child's PID, and the group it leads, stay reserved;
    # a census after the reap could see a reused group ID (HF-146).
    events = []
    census = {7: "member"}
    monkeypatch.setattr(process_custody, "os", _census_os(events))

    def take():
        events.append("census")
        return census

    proc = _owned_handle()
    process_custody.ChildExecutionClock(proc, time.perf_counter(), exit_census=take)
    assert proc.wait(timeout=2) == 0
    assert events == ["waitid", "census", "wait4:1"]
    assert process_custody.take_child_exit_census(proc) == (census, None)
    assert process_custody.take_child_exit_census(proc) == (None, None)


def test_kqueue_exit_census_precedes_the_reap(monkeypatch):
    import errno

    for exit_evidence in ("note_exit", "attach_esrch", "ev_error_esrch"):
        events = []

        class Queue:
            def control(self, *args):
                if exit_evidence == "attach_esrch":
                    raise ProcessLookupError(errno.ESRCH, "child already exited")
                if exit_evidence == "ev_error_esrch":
                    return [_exit_event(123, flags=16, data=errno.ESRCH)]
                return [_exit_event(123)]

            def close(self):
                pass

        monkeypatch.setattr(process_custody, "os", _census_os(events, waitid=False))
        monkeypatch.setattr(process_custody, "select", _kqueue_api(Queue))

        def take():
            events.append("census")
            return {}

        proc = _owned_handle()
        clock = process_custody.ChildExecutionClock(
            proc, time.perf_counter(), exit_census=take
        )
        assert proc.wait(timeout=2) == 0 and clock.posix_kqueue
        # No WNOHANG reap may precede the census, even for a child that had
        # already exited when the watch began.
        assert events == ["census", "wait4:0"], exit_evidence


def test_uncertain_exit_takes_no_census(monkeypatch):
    import errno

    events = []

    class Queue:
        def control(self, *args):
            return [_exit_event(123, flags=16, data=errno.EPERM)]

        def close(self):
            pass

    monkeypatch.setattr(process_custody, "os", _census_os(events, waitid=False))
    monkeypatch.setattr(process_custody, "select", _kqueue_api(Queue))
    proc = _owned_handle()
    clock = process_custody.ChildExecutionClock(
        proc, time.perf_counter(), exit_census=lambda: events.append("census")
    )
    assert clock.done.wait(2)
    assert events == ["wait4:1"]
    assert process_custody.take_child_exit_census(proc) == (None, None)


def test_exit_census_failure_is_recorded_and_the_child_is_still_reaped(monkeypatch):
    events = []
    monkeypatch.setattr(process_custody, "os", _census_os(events))

    def broken():
        raise RuntimeError("process table unavailable")

    proc = _owned_handle()
    clock = process_custody.ChildExecutionClock(
        proc, time.perf_counter(), exit_census=broken
    )
    assert proc.wait(timeout=2) == 0 and clock.error is None
    assert events == ["waitid", "wait4:1"]
    assert process_custody.take_child_exit_census(proc) == (
        None,
        "RuntimeError: process table unavailable",
    )
