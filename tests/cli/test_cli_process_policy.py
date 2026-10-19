"""A compiler process ends as interpreter shutdown ends it, minus heap teardown.

``molt.compiler_process.exit_process`` skips freeing the heap (1.3 s of a
one-file rebuild) and nothing else. Each test runs one script twice, once
ending through ``raise SystemExit`` (CPython's own shutdown, the oracle) and
once through ``exit_process``, and requires the same status and output.
"""

from __future__ import annotations

import json
import sys
import textwrap

import pytest

from tests.process_guard_common import run_guarded_test_process

_PRELUDE = textwrap.dedent(
    """
    import atexit, sys, threading, time
    from molt import compiler_process

    # Shutdown work the interpreter performs: a buffered stdout write, an
    # atexit callback and a non-daemon thread that outlives the main thread.
    print("buffered before exit")
    atexit.register(print, "atexit callback ran")

    def late():
        time.sleep(0.2)
        print("non-daemon thread finished")

    threading.Thread(target=late).start()
    """
)


def _ending(code: str, *, policy: bool) -> str:
    if policy:
        return f"compiler_process.exit_process({code})\n"
    return f"raise SystemExit({code})\n"


def _run(source: str) -> tuple[int, str, str]:
    result = run_guarded_test_process([sys.executable, "-c", source], timeout=120)
    return result.returncode, result.stdout, result.stderr


@pytest.mark.parametrize("code", ["0", "3", "None", "True", "'molt: stopped'", "-1"])
def test_exit_matches_interpreter_shutdown(code: str) -> None:
    oracle = _run(_PRELUDE + _ending(code, policy=False))
    observed = _run(_PRELUDE + _ending(code, policy=True))
    assert observed == oracle
    assert "non-daemon thread finished" in observed[1]
    assert "atexit callback ran" in observed[1]


def test_failed_stdout_flush_matches_interpreter_shutdown() -> None:
    # Python's shutdown flushes sys.stdout once more; a failing flush is
    # reported through the unraisable hook and the process exits with 120.
    prelude = textwrap.dedent(
        """
        import io, sys
        from molt import compiler_process

        class BrokenPipeRaw(io.RawIOBase):
            def writable(self):
                return True

            def write(self, data):
                raise BrokenPipeError(32, "Broken pipe")

        sys.stdout = io.TextIOWrapper(BrokenPipeRaw(), encoding="utf-8")
        print("lost in the buffer")
        """
    )
    oracle = _run(prelude + _ending("0", policy=False))
    observed = _run(prelude + _ending("0", policy=True))
    assert oracle[0] == 120
    assert observed == oracle


def test_cli_entry_matches_interpreter_shutdown() -> None:
    # The process entry flushes piped output and keeps argparse's statuses.
    for argv in (["--version"], ["build"], ["no-such-command"]):
        normal = (
            "import sys\n"
            f"sys.argv = ['molt', *{argv!r}]\n"
            "from molt.cli import main\n"
            "raise SystemExit(main())\n"
        )
        entry = (
            "import runpy, sys\n"
            f"sys.argv = ['molt', *{argv!r}]\n"
            "runpy.run_module('molt.cli', run_name='__main__', alter_sys=True)\n"
        )
        oracle = _run(normal)
        assert _run(entry) == oracle


def _worker_process_observation() -> dict[str, object]:
    import gc
    import multiprocessing
    import os

    from molt import compiler_process

    lock = getattr(compiler_process, "_test_parent_lock", None)
    lock_inherited = lock is not None
    lock_acquired = lock.acquire(blocking=False) if lock_inherited else None
    if lock_acquired:
        lock.release()
    parent = multiprocessing.parent_process()
    assert parent is not None
    return {
        "parent_marker": getattr(compiler_process, "_test_parent_marker", None),
        "lock_inherited": lock_inherited,
        "lock_acquired": lock_acquired,
        "thresholds": gc.get_threshold(),
        "method": multiprocessing.get_start_method(),
        "creating_parent": parent.pid,
        "parent_alive": parent.is_alive(),
        "environment": os.environ.get("PARENT_PROCESS_CONTEXT_TEST_VALUE"),
    }


def test_workers_use_the_compiler_collector_pacing() -> None:
    # The actual worker must not inherit a marker or locked mutex added only
    # to the parent. Raw fork fails both independent observations.
    probe = textwrap.dedent(
        """
        import json, os, threading
        from concurrent.futures import ProcessPoolExecutor
        from molt import compiler_process
        from tests.cli.test_cli_process_policy import _worker_process_observation

        if __name__ == "__main__":
            compiler_process._test_parent_marker = "parent-only"
            compiler_process._test_parent_lock = threading.Lock()
            compiler_process._test_parent_lock.acquire()
            os.environ["PARENT_PROCESS_CONTEXT_TEST_VALUE"] = "first"
            with ProcessPoolExecutor(
                max_workers=1,
                mp_context=compiler_process.worker_context(),
                initializer=compiler_process.worker_initializer,
                initargs=(None,),
            ) as pool:
                worker = pool.submit(_worker_process_observation).result()
            os.environ["PARENT_PROCESS_CONTEXT_TEST_VALUE"] = "second"
            with ProcessPoolExecutor(
                max_workers=1,
                mp_context=compiler_process.worker_context(),
                initializer=compiler_process.worker_initializer,
                initargs=(None,),
            ) as pool:
                later = pool.submit(_worker_process_observation).result()
            print(json.dumps([worker, later, list(compiler_process.GC_THRESHOLDS), os.getpid()]))
        """
    )
    status, stdout, stderr = _run(probe)
    assert status == 0, stderr
    worker, later, expected, parent_pid = json.loads(stdout)
    for observed in (worker, later):
        assert observed["thresholds"] == expected != [700, 10, 10]
        assert observed["parent_marker"] is None
        assert observed["lock_inherited"] is False and observed["lock_acquired"] is None
        assert observed["method"] == "spawn"
        assert observed["creating_parent"] == parent_pid
        assert observed["parent_alive"] is True
    assert worker["environment"] == "first"
    assert later["environment"] == "second"


def test_a_profiler_hosting_the_cli_keeps_control_after_it(tmp_path) -> None:
    # `python -m cProfile -m molt.cli` writes its stats after the module ends;
    # the entry hands such a host the SystemExit instead of ending the process.
    stats = tmp_path / "cli.prof"
    profiled = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "cProfile",
            "-o",
            str(stats),
            "-m",
            "molt.cli",
            "--version",
        ],
        timeout=120,
    )
    plain = run_guarded_test_process(
        [sys.executable, "-m", "molt.cli", "--version"], timeout=120
    )
    assert (profiled.returncode, profiled.stdout) == (plain.returncode, plain.stdout)
    assert plain.returncode == 0 and plain.stdout
    assert stats.stat().st_size > 0
