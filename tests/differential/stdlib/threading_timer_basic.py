"""Purpose: differential coverage for threading.Timer."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    events: list[str] = []

    def fired() -> None:
        events.append("fired")

    timer = threading.Timer(0.01, fired)
    timer.start()
    timer.join(timeout=1.0)
    print(events)
