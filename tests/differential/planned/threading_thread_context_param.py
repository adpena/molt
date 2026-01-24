"""Purpose: differential coverage for Thread context parameter."""

try:
    import contextvars
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    var = contextvars.ContextVar("var", default="missing")
    ctx = contextvars.copy_context()
    ctx.run(var.set, "from-context")

    results: list[str] = []

    def worker() -> None:
        results.append(var.get())

    try:
        t = threading.Thread(target=worker, context=ctx)
    except TypeError as exc:
        print(type(exc).__name__, exc)
    else:
        t.start()
        t.join(timeout=1.0)
        print(results)
