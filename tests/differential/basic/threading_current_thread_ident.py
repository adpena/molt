"""Purpose: differential coverage for current_thread/ident/native_id."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    results: list[tuple] = []

    def worker() -> None:
        cur = threading.current_thread()
        results.append(
            (cur.name, isinstance(cur.ident, int), getattr(cur, "native_id", None) is not None)
        )

    main = threading.current_thread()
    print(main.name, isinstance(main.ident, int), getattr(main, "native_id", None) is not None)

    t = threading.Thread(target=worker, name="worker-ident")
    t.start()
    t.join(timeout=1.0)
    print(results)
