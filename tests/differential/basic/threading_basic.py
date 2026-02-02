"""Purpose: differential coverage for threading basic."""

import threading
import time


results: list[str] = []


def worker() -> None:
    results.append("start")
    time.sleep(0.01)
    results.append("end")


t = threading.Thread(target=worker, name="worker")
print("name", t.name)
print("daemon", t.daemon)
print("alive0", t.is_alive())

t.start()
print("alive1", t.is_alive())

t.join()
print("alive2", t.is_alive())
print("ident", isinstance(t.ident, int))
print(results)
