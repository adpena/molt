"""Purpose: differential coverage for threading exception propagation."""

import threading


seen: list[str] = []


def worker() -> None:
    try:
        raise ValueError("boom")
    except Exception as exc:
        seen.append(type(exc).__name__)


t = threading.Thread(target=worker)
t.start()
t.join()
print(seen)
