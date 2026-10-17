"""Process policy for Molt compiler processes: collector pacing and exit.

A compiler process holds a large, long-lived heap: a one-file rebuild keeps
about 1.9 M tracked objects. CPython's default pacing (700, 10, 10) then runs
about 5,800 collections in that build, 15 of them full, for about 3 s of pauses
that free almost nothing. Interpreter teardown then spends 1.3 s freeing a heap
that the operating system reclaims at exit. mypy has the same two costs and
makes the same two choices: a first-generation threshold of 200,000
(``mypy/build.py``) and an exit that skips teardown (``mypy.util.hard_exit``).

``configure`` paces the collector at the start of a compiler process, and
``worker_initializer`` does so in each worker. ``exit_process`` ends a command
process as interpreter shutdown does, except that it does not free the heap.
In-process callers of ``molt.cli.main``, such as tests, keep the default policy.
"""

from __future__ import annotations

import atexit
import gc
import os
import sys
import threading
from typing import NoReturn

GC_THRESHOLDS = (200_000, 30, 30)

# CPython's status when it cannot flush a standard stream at exit (Py_Exit).
_FLUSH_FAILURE_STATUS = 120

# PEP 669 tool identifiers: debugger, coverage, profiler, two unnamed, optimizer.
_MONITORING_TOOL_IDS = range(6)


def configure() -> None:
    """Pace CPython's cyclic collector for a compiler-sized heap."""
    if sys.implementation.name == "cpython":
        gc.set_threshold(*GC_THRESHOLDS)


def worker_initializer(log_dir: str | None) -> None:
    """Start one compiler worker: the process policy, then input reporting."""
    from molt import input_report

    configure()
    input_report.worker_initializer(log_dir)


def exit_status(code: object) -> int:
    """The status ``SystemExit(code)`` gives; a non-integer code is printed."""
    if code is None:
        return 0
    if isinstance(code, int):
        return int(code)
    print(code, file=sys.stderr)
    return 1


def exit_process(code: object) -> NoReturn:
    """Run interpreter shutdown up to heap teardown, then end the process.

    A profiler, tracer or debugger that runs this module (``python -m cProfile``,
    ``python -m pdb``) still has work after the module ends, so it receives the
    ``SystemExit`` instead. ``MOLT_CLI_PROFILE`` profiles inside the command.
    """
    if _observed():
        raise SystemExit(code)
    status = exit_status(code)
    # Py_FinalizeEx first waits for non-daemon threads through
    # ``threading._shutdown``, which also runs threading's exit callbacks (the
    # ones concurrent.futures uses to join its workers). It then runs the atexit
    # callbacks and flushes the standard streams. Only the freeing comes later.
    threading._shutdown()  # ty: ignore[unresolved-attribute]
    atexit._run_exitfuncs()
    for stream in (sys.stdout, sys.stderr):
        if stream is None or stream.closed:
            continue
        try:
            stream.flush()
        except OSError as exc:
            status = _FLUSH_FAILURE_STATUS
            _report_flush_failure(stream, exc)
    os._exit(status)


def _observed() -> bool:
    # cProfile and coverage run on sys.monitoring (PEP 669); pdb and older
    # tools install a profile or trace function.
    return (
        sys.getprofile() is not None
        or sys.gettrace() is not None
        or any(
            sys.monitoring.get_tool(tool) is not None for tool in _MONITORING_TOOL_IDS
        )
    )


def _report_flush_failure(stream: object, exc: OSError) -> None:
    import traceback

    # The message sys.unraisablehook prints for a failed exit-time flush. The
    # interpreter flushes from C, so the traceback omits this module's frame.
    frames = exc.__traceback__.tb_next if exc.__traceback__ is not None else None
    message = f"Exception ignored in: {stream!r}\n" + "".join(
        traceback.format_exception(type(exc), exc, frames)
    )
    try:
        os.write(2, message.encode("utf-8", "backslashreplace"))
    except OSError:
        pass
