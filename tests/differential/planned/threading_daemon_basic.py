"""Purpose: differential coverage for threading daemon basic."""

import threading


results: list[bool] = []


def worker() -> None:
    results.append(threading.current_thread().daemon)


t = threading.Thread(target=worker)
t.daemon = True
print("daemon", t.daemon)

t.start()
t.join()
print(results)
