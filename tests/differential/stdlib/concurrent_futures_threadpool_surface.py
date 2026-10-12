# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,thread.spawn,thread.shared
"""Purpose: differential coverage for concurrent.futures threadpool surface."""

from __future__ import annotations

from concurrent.futures import (
    ALL_COMPLETED,
    FIRST_EXCEPTION,
    CancelledError,
    Future,
    ThreadPoolExecutor,
    as_completed,
    wait,
)
from decimal import Decimal
from fractions import Fraction
import threading
import time


def add(x: int, y: int) -> int:
    return x + y


def explode() -> int:
    raise ValueError("boom")


def slow(value: int) -> int:
    time.sleep(0.02)
    return value


def blocker(started: threading.Event, release: threading.Event) -> str:
    started.set()
    release.wait()
    return "released"


class ComparisonLimit:
    def __init__(self, maximum: float) -> None:
        self.maximum = maximum
        self.constructor_checks = 0
        self.growth_counts = []
        self.error = None

    def __le__(self, other: int) -> bool:
        self.constructor_checks += 1
        return self.maximum <= other

    def __gt__(self, count: int) -> bool:
        self.growth_counts.append(count)
        if self.error is not None:
            raise self.error
        return count < self.maximum

    def __int__(self) -> int:
        raise AssertionError("worker limit must retain its comparison protocol")


def numeric_limit_surface() -> None:
    # CPython compares the current integer thread count with the original
    # object only when growing; it neither coerces the limit nor starts workers
    # during construction. Block every accepted job to make growth observable.
    custom = ComparisonLimit(3.5)
    for label, limit, expected in [
        ("fractional_small", 0.5, 1),
        ("fractional_large", 3.5, 4),
        ("bool", True, 1),
        ("fraction", Fraction(7, 2), 4),
        ("decimal", Decimal("3.5"), 4),
        ("comparison", custom, 4),
        ("infinity", float("inf"), 2),
    ]:
        executor = ThreadPoolExecutor(max_workers=limit)
        release = threading.Event()
        started = [threading.Event() for _ in range(expected)]
        futures = []
        try:
            for event in started:
                futures.append(executor.submit(blocker, event, release))
            for event in started:
                if not event.wait(timeout=5):
                    raise RuntimeError("numeric worker limit prevented required growth")
            print("thread_limit", label, executor._max_workers is limit, len(futures))
        finally:
            release.set()
            executor.shutdown(wait=True)
        print("thread_limit_results", label, [future.result() for future in futures])
    print("thread_limit_protocol", custom.constructor_checks, custom.growth_counts)

    for label, limit in [("above_old_cap", 513), ("large_int", 1 << 100)]:
        executor = ThreadPoolExecutor(max_workers=limit)
        print("thread_unused_limit", label, executor._max_workers is limit)
        executor.shutdown(wait=True)

    # NaN passes the constructor's <= 0 check, but count < NaN is false.
    nan = float("nan")
    executor = ThreadPoolExecutor(max_workers=nan)
    pending = executor.submit(add, 1, 2)
    print("thread_nan_pending", executor._max_workers is nan, pending.done())
    executor.shutdown(wait=True, cancel_futures=True)
    print("thread_nan_cancelled", pending.cancelled())

    for limit in [0, -1, False, float("-inf"), "2", 1j]:
        try:
            executor = ThreadPoolExecutor(max_workers=limit)
        except Exception as exc:
            print("thread_invalid_limit", type(exc).__name__)
        else:
            executor.shutdown(wait=True)
            raise AssertionError("invalid worker limit was accepted")

    # A failed growth comparison happens after queue publication in CPython.
    # Its exact exception escapes; a later submit can start the queued work.
    limit = ComparisonLimit(1)
    error = LookupError("original worker-limit comparison")
    limit.error = error
    executor = ThreadPoolExecutor(max_workers=limit)
    seen = []

    def record(value: int) -> int:
        seen.append(value)
        return value

    try:
        executor.submit(record, 1)
    except LookupError as exc:
        print("thread_limit_error_identity", exc is error)
    else:
        raise AssertionError("comparison failure was suppressed")
    limit.error = None
    retry = executor.submit(record, 2)
    executor.shutdown(wait=True)
    print(
        "thread_limit_retry",
        retry.result(),
        seen,
        limit.constructor_checks,
        limit.growth_counts,
    )


def main() -> None:
    numeric_limit_surface()
    fut = Future()
    print("future_state_initial", fut.done(), fut.running(), fut.cancelled())
    print("future_set_running", fut.set_running_or_notify_cancel())
    fut.set_result(41)
    print(
        "future_state_final", fut.result(), fut.done(), fut.running(), fut.cancelled()
    )
    try:
        fut.set_result(99)
    except Exception as exc:
        print("future_set_result_again", type(exc).__name__)

    cancelled = Future()
    print("future_cancel", cancelled.cancel(), cancelled.cancelled(), cancelled.done())
    try:
        cancelled.result(timeout=0)
    except Exception as exc:
        print("future_cancel_result", type(exc).__name__)

    with ThreadPoolExecutor(max_workers=2, thread_name_prefix="molt") as executor:
        submitted = [executor.submit(add, idx, idx + 1) for idx in range(3)]
        done, pending = wait(submitted, return_when=ALL_COMPLETED)
        print(
            "thread_wait_all",
            sorted(item.result() for item in done),
            len(pending),
        )

        mapped = list(executor.map(add, [1, 2, 3], [10, 20, 30]))
        print("thread_map", mapped)

        completed = sorted(
            item.result()
            for item in as_completed(
                [executor.submit(slow, 3), executor.submit(slow, 1)]
            )
        )
        print("thread_as_completed", completed)

        ok = executor.submit(add, 2, 3)
        # Make the successful completion precede the failing submission so the
        # FIRST_EXCEPTION result set has a deterministic independent oracle.
        assert ok.result(timeout=5) == 5
        bad = executor.submit(explode)
        done, pending = wait([ok, bad], return_when=FIRST_EXCEPTION)
        errors = sorted(
            type(item.exception()).__name__
            for item in done
            if (not item.cancelled()) and item.exception() is not None
        )
        print("thread_wait_first_exception", len(done), len(pending), errors)

    started = threading.Event()
    release = threading.Event()
    executor = ThreadPoolExecutor(max_workers=1)
    first = executor.submit(blocker, started, release)
    if not started.wait(timeout=1):
        raise RuntimeError("thread worker did not start")
    second = executor.submit(blocker, threading.Event(), release)
    executor.shutdown(wait=False, cancel_futures=True)
    print("thread_cancel_pending", second.cancelled(), second.done())
    release.set()
    print("thread_first", first.result(timeout=1))
    try:
        second.result(timeout=0.2)
    except CancelledError as exc:
        print("thread_cancel_error", type(exc).__name__)


if __name__ == "__main__":
    main()
