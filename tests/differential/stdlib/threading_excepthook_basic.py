"""Purpose: differential coverage for threading.excepthook."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    events: list[str] = []
    old_hook = threading.excepthook

    def hook(args) -> None:
        events.append(args.exc_type.__name__)

    threading.excepthook = hook

    def boom() -> None:
        raise ValueError("boom")

    t = threading.Thread(target=boom)
    t.start()
    t.join(timeout=1.0)
    threading.excepthook = old_hook
    print(events)
