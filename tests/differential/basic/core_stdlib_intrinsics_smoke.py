"""Purpose: intrinsic-backed smoke coverage for core stdlib wrappers."""

import os
import sys
import threading
import time
import traceback

print("os_name", isinstance(os.name, str))
print("os_path", os.path.basename("/tmp/molt.txt"), os.path.dirname("/tmp/molt.txt"))

print("sys", isinstance(sys.argv, list), isinstance(sys.modules, dict))
print("sys_ver", isinstance(sys.version, str), isinstance(sys.platform, str))

t0 = time.monotonic()
t1 = time.monotonic()
clock = time.get_clock_info("monotonic")
print("time", t1 >= t0, clock.monotonic, isinstance(clock.resolution, float))

try:
    1 / 0
except Exception as exc:
    lines = traceback.format_exception(type(exc), exc, exc.__traceback__)
    print(
        "traceback",
        any("ZeroDivisionError" in line for line in lines),
        any("core_stdlib_intrinsics_smoke.py" in line for line in lines),
    )

results = []


def _worker() -> None:
    results.append(threading.current_thread().name)


worker = threading.Thread(target=_worker, name="core-smoke-worker")
worker.start()
worker.join()
print("thread", worker.ident is not None, not worker.is_alive(), results == ["core-smoke-worker"])
