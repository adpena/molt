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


def main() -> None:
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
