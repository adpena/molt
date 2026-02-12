"""Purpose: differential coverage for threading.enumerate/main_thread."""

try:
    import threading
    import time
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    ready = threading.Event()
    finish = threading.Event()
    names: list[str] = []

    def worker() -> None:
        ready.set()
        finish.wait(timeout=1.0)

    main = threading.main_thread()
    names.append(main.name)
    t = threading.Thread(target=worker, name="worker-enum")
    t.start()
    ready.wait(timeout=1.0)

    threads = threading.enumerate()
    names.extend(sorted(th.name for th in threads))
    print("active", threading.active_count())
    print("names", names)

    finish.set()
    t.join(timeout=1.0)
