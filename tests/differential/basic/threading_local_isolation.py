"""Purpose: differential coverage for threading.local isolation."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    local = threading.local()
    local.value = "main"
    results: list[str] = []

    def worker() -> None:
        results.append(getattr(local, "value", "missing"))
        local.value = "worker"
        results.append(local.value)

    t = threading.Thread(target=worker)
    t.start()
    t.join(timeout=1.0)
    results.append(local.value)
    print(results)
