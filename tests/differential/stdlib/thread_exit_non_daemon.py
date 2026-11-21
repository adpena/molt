# MOLT_ENV: MOLT_CAPABILITIES=thread.spawn,thread.shared
# MOLT_META: stderr=exception_signature
"""Wait for non-daemon worker cleanup after an uncaught main exception."""

import atexit
import sys
import threading


def worker():
    released.wait()
    print("worker finished")


def report(exc_type, exc, tb):
    sys.__excepthook__(exc_type, exc, tb)
    released.set()


released = threading.Event()
atexit.register(print, "atexit after worker")
threading.Thread(target=worker).start()
sys.excepthook = report
raise ValueError("uncaught main exception")
