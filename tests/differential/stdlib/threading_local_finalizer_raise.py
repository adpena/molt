# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,thread.spawn,thread.shared
"""A local finalizer's raised error is unraisable and cannot leak custody."""

import sys
import threading

local = threading.local()
events: list[tuple[str, str]] = []


def unraisable(args: object) -> None:
    events.append((args.exc_type.__name__, str(args.exc_value)))


class Value:
    def __del__(self) -> None:
        raise ValueError("local finalizer")


def worker() -> None:
    local.value = Value()


old_hook = sys.unraisablehook
sys.unraisablehook = unraisable
try:
    for _ in range(2):
        thread = threading.Thread(target=worker)
        thread.start()
        thread.join()
        print(events)
finally:
    sys.unraisablehook = old_hook
print("joined")
