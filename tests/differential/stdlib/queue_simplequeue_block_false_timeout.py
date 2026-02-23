"""Purpose: differential coverage for SimpleQueue block=False timeout semantics."""

import queue


simple = queue.SimpleQueue()

for timeout in (0.0, 0.01, 1.0):
    try:
        simple.get(block=False, timeout=timeout)
    except Exception as exc:  # noqa: BLE001
        print("empty_on_nonblocking_timeout", timeout, type(exc).__name__)
        assert isinstance(exc, queue.Empty), (timeout, type(exc).__name__)
    else:
        raise AssertionError("SimpleQueue.get(block=False, timeout=...) should raise Empty")

simple.put("payload")
print("get_with_nonblocking_timeout", simple.get(block=False, timeout=0.5))
