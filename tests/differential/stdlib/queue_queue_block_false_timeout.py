"""Purpose: differential coverage for Queue block=False timeout semantics."""

import queue


def assert_get_nonblocking_empty(timeout: float) -> None:
    q: queue.Queue[str] = queue.Queue(maxsize=1)
    try:
        q.get(block=False, timeout=timeout)
    except Exception as exc:  # noqa: BLE001
        print("get_empty_nonblocking", timeout, type(exc).__name__)
        assert isinstance(exc, queue.Empty), (timeout, type(exc).__name__)
    else:
        raise AssertionError(
            "queue.Queue.get(block=False, timeout=...) should raise Empty when empty"
        )


def assert_put_nonblocking_full(timeout: float) -> None:
    q: queue.Queue[str] = queue.Queue(maxsize=1)
    q.put("first")
    try:
        q.put("second", block=False, timeout=timeout)
    except Exception as exc:  # noqa: BLE001
        print("put_full_nonblocking", timeout, type(exc).__name__)
        assert isinstance(exc, queue.Full), (timeout, type(exc).__name__)
    else:
        raise AssertionError(
            "queue.Queue.put(block=False, timeout=...) should raise Full when full"
        )


for probe in (-1.0, 0.0, 0.25):
    assert_get_nonblocking_empty(probe)
    assert_put_nonblocking_full(probe)

q_ok: queue.Queue[str] = queue.Queue(maxsize=1)
q_ok.put("payload", block=False, timeout=-99.0)
print("put_nonfull_nonblocking", q_ok.get_nowait())
q_ok.put("payload2")
print("get_nonempty_nonblocking", q_ok.get(block=False, timeout=-77.0))
