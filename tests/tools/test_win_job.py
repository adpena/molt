"""Prove tools/win_job: closing the sole job handle reaps the child AND a
grandchild spawned only after resume, so KILL_ON_JOB_CLOSE captures the whole
build subtree with no race. This is the source-side guarantee behind the
orphan-leak prevention (complement to tools/orphan_reaper.py's sweep net)."""

from __future__ import annotations

import ctypes
import os
import subprocess
import sys
import tempfile
import time

import pytest

from tools import win_job

pytestmark = pytest.mark.skipif(
    not sys.platform.startswith("win"),
    reason="Job Objects and the orphan-leak class are Windows-only",
)

_STILL_ACTIVE = 259
_PROCESS_QUERY_LIMITED_INFORMATION = 0x1000

# Child: only AFTER it starts running (i.e. after resume) does it spawn a
# grandchild and record its pid, then idle. If the grandchild inherits the job,
# closing the job kills both — proving race-free capture.
_CHILD = (
    "import subprocess,sys,time;"
    "gc=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)']);"
    "open(sys.argv[1],'w').write(str(gc.pid));sys.stdout.flush();"
    "time.sleep(120)"
)


def _alive(pid: int) -> bool:
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    h = k32.OpenProcess(_PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not h:
        return False
    try:
        code = ctypes.c_ulong()
        if not k32.GetExitCodeProcess(h, ctypes.byref(code)):
            return False
        return code.value == _STILL_ACTIVE
    finally:
        k32.CloseHandle(h)


def test_kill_on_job_close_reaps_child_and_grandchild() -> None:
    pidfile = tempfile.NamedTemporaryFile(delete=False, suffix=".pid")
    pidfile.close()
    proc = None
    gc_pid = None
    try:
        job = win_job.create_kill_on_close_job()
        assert job, "create_kill_on_close_job returned None on Windows"

        proc = subprocess.Popen(
            [sys.executable, "-c", _CHILD, pidfile.name],
            creationflags=(
                win_job.suspended_creationflag() | subprocess.CREATE_NO_WINDOW
            ),
        )
        assert win_job.assign_and_resume(job, proc), "child was not placed in job"

        for _ in range(100):
            raw = open(pidfile.name).read().strip()
            if raw:
                gc_pid = int(raw)
                break
            time.sleep(0.1)
        assert gc_pid is not None, "child never spawned grandchild (resume failed)"

        assert _alive(proc.pid), "child not alive after resume"
        assert _alive(gc_pid), "grandchild not alive after resume"

        # Simulate guard death: drop the sole job handle -> KILL_ON_JOB_CLOSE.
        win_job.close_job(job)

        child_dead = gc_dead = False
        for _ in range(100):
            child_dead = not _alive(proc.pid)
            gc_dead = not _alive(gc_pid)
            if child_dead and gc_dead:
                break
            time.sleep(0.1)
        assert child_dead, "child survived job close (KILL_ON_JOB_CLOSE failed)"
        assert gc_dead, "grandchild survived job close (no-race capture failed)"
    finally:
        if proc is not None and proc.poll() is None:
            proc.kill()
        if gc_pid is not None and _alive(gc_pid):
            subprocess.run(
                ["taskkill", "/PID", str(gc_pid), "/F"], capture_output=True
            )
        with __import__("contextlib").suppress(OSError):
            os.unlink(pidfile.name)
