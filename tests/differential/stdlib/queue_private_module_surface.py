"""Purpose: differential sanity coverage for `_queue` import surface."""

import _queue


required_names = ("Empty", "SimpleQueue")
for name in required_names:
    assert hasattr(_queue, name), name
    value = getattr(_queue, name)
    print(name, type(value).__name__, callable(value))

simple = _queue.SimpleQueue()
simple.put("value")
print("simplequeue_get", simple.get())

try:
    simple.get(block=False)
except Exception as exc:  # noqa: BLE001
    print("simplequeue_empty_exc", type(exc).__name__)
    assert isinstance(exc, _queue.Empty), type(exc)
else:
    raise AssertionError("_queue.SimpleQueue.get(block=False) should raise _queue.Empty")
