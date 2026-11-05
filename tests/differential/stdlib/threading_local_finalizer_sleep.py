# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,thread.spawn,thread.shared
"""Thread exit releases local values while finalizers may suspend the GIL."""

import threading
import time

local = threading.local()
local.value = "main"
events: list[str] = []


class Value:
    def __del__(self) -> None:
        events.append("enter")
        time.sleep(0.001)
        events.append("exit")


def worker() -> None:
    local.value = Value()


for _ in range(2):
    thread = threading.Thread(target=worker)
    thread.start()
    thread.join()
    print(events)
    print(local.value)
