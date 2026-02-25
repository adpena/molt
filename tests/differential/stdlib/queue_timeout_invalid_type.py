"""Purpose: differential coverage for queue timeout invalid-type semantics."""

import queue


unbounded: queue.Queue[str] = queue.Queue()
unbounded.put("payload", timeout="bad")
print("unbounded_put_bad_timeout", unbounded.get_nowait())

empty_queue: queue.Queue[str] = queue.Queue()
try:
    empty_queue.get(timeout="bad")
except Exception as exc:  # noqa: BLE001
    print("queue_get_bad_timeout", type(exc).__name__)
    assert isinstance(exc, TypeError), type(exc).__name__
else:
    raise AssertionError("queue.Queue.get(timeout='bad') should raise TypeError")

bounded: queue.Queue[str] = queue.Queue(maxsize=1)
bounded.put("first")
try:
    bounded.put("second", timeout="bad")
except Exception as exc:  # noqa: BLE001
    print("queue_put_full_bad_timeout", type(exc).__name__)
    assert isinstance(exc, TypeError), type(exc).__name__
else:
    raise AssertionError("full queue.Queue.put(timeout='bad') should raise TypeError")

simple = queue.SimpleQueue()
try:
    simple.get(timeout="bad")
except Exception as exc:  # noqa: BLE001
    print("simplequeue_get_bad_timeout", type(exc).__name__)
    assert isinstance(exc, TypeError), type(exc).__name__
else:
    raise AssertionError("queue.SimpleQueue.get(timeout='bad') should raise TypeError")
