"""Optional, invocation-owned RSS observations for the existing midend stats.

Peaks are sampled compiler-process RSS, including retained inputs, not allocation
counts or process-lifetime high-water marks. Nested pass observations overlap.
"""

from __future__ import annotations

from dataclasses import dataclass
from functools import wraps
import os
import threading
import time
from typing import Any, Callable, ParamSpec, TypeVar, cast

P = ParamSpec("P")
R = TypeVar("R")
RSS_SAMPLE_INTERVAL_S = 0.01


def _resident_bytes() -> int | None:
    from molt import process_model

    pid = os.getpid()
    rows = process_model.sample_processes_custody(
        process_model.CustodyInterest(frozenset(), frozenset({pid}))
    )
    row = rows.get(pid)
    return row.rss_kb * 1024 if row is not None and row.rss_kb > 0 else None


@dataclass(slots=True)
class PassMemorySample:
    start_rss_bytes: int | None = None
    peak_rss_bytes: int | None = None
    samples: int = 0
    missing_samples: int = 0

    def observe(self, resident: int | None) -> None:
        if resident is None:
            self.missing_samples += 1
            return
        if self.start_rss_bytes is None:
            self.start_rss_bytes = resident
        self.peak_rss_bytes = max(self.peak_rss_bytes or 0, resident)
        self.samples += 1


class MidendMemoryProfiler:
    """One observer per active function; explicit tokens fence nested scopes."""

    def __init__(self) -> None:
        self._scopes: list[PassMemorySample] = []
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._poll, daemon=True)
        self._thread.start()

    def _observe(self) -> None:
        try:
            resident = _resident_bytes()
        except (OSError, RuntimeError):
            resident = None
        with self._lock:
            for scope in self._scopes:
                scope.observe(resident)

    def _poll(self) -> None:
        while not self._stop.wait(RSS_SAMPLE_INTERVAL_S):
            self._observe()

    def begin(self) -> PassMemorySample:
        sample = PassMemorySample()
        with self._lock:
            self._scopes.append(sample)
        self._observe()
        return sample

    def finish(self, sample: PassMemorySample) -> None:
        self._observe()
        with self._lock:
            if not self._scopes or self._scopes[-1] is not sample:
                raise RuntimeError("midend memory scopes must finish in nesting order")
            self._scopes.pop()

    def close(self) -> None:
        self._stop.set()
        self._thread.join()
        self._scopes.clear()


def profile_midend_function(function: Callable[P, R]) -> Callable[P, R]:
    @wraps(function)
    def measured(*args: P.args, **kwargs: P.kwargs) -> R:
        owner = cast(Any, args[0])
        if (
            os.environ.get("MOLT_MIDEND_STATS") is None
            or owner._source_is_stdlib_module
        ):
            return function(*args, **kwargs)
        previous = owner._midend_memory_profiler
        profiler = MidendMemoryProfiler()
        owner._midend_memory_profiler = profiler
        try:
            return function(*args, **kwargs)
        finally:
            owner._midend_memory_profiler = previous
            profiler.close()

    return measured


def profile_midend_solver(name: str) -> Callable[[Callable[P, R]], Callable[P, R]]:
    def decorate(function: Callable[P, R]) -> Callable[P, R]:
        @wraps(function)
        def measured(*args: P.args, **kwargs: P.kwargs) -> R:
            owner = cast(Any, args[0])
            if (
                os.environ.get("MOLT_MIDEND_STATS") is None
                or owner._midend_memory_profiler is None
            ):
                return function(*args, **kwargs)
            memory = owner._start_midend_pass_memory()
            started = time.perf_counter()
            try:
                return function(*args, **kwargs)
            finally:
                owner._record_midend_pass_sample(
                    name,
                    elapsed_ms=(time.perf_counter() - started) * 1000.0,
                    accepted=None,
                    memory=memory,
                )

        return measured

    return decorate
