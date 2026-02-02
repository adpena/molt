"""Purpose: differential coverage for Condition wait_for."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    cond = threading.Condition()
    state = {"ready": False}
    results: list[object] = []

    def waiter() -> None:
        with cond:
            ok = cond.wait_for(lambda: state["ready"], timeout=0.05)
            results.append(ok)

    t = threading.Thread(target=waiter)
    t.start()
    t.join(timeout=1.0)
    results.append("first_done")

    with cond:
        state["ready"] = True
        cond.notify_all()

    t2 = threading.Thread(target=waiter)
    t2.start()
    t2.join(timeout=1.0)
    results.append("second_done")
    print(results)
