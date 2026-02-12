"""Purpose: differential coverage for threading.stack_size."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    try:
        current = threading.stack_size()
        print("current", current)
        prev = threading.stack_size(0)
        print("prev", prev, "now", threading.stack_size())
    except Exception as exc:
        print(type(exc).__name__, exc)
