# MOLT_ENV: MOLT_CAPABILITIES=thread.spawn,thread.shared
"""Purpose: deep recursion runs to the recursion limit on every thread.

CPython keeps Python frames off the C stack, so a raised recursion limit
admits hundreds of thousands of nested calls; Molt runs compiled frames on a
reserved program stack (molt-runtime state/program_stack.rs), on the main
thread and on threads the runtime starts.
"""

import sys
import threading

sys.setrecursionlimit(1_000_000)


def down(n):
    return n if n == 0 else 1 + down(n - 1)


print("main", down(300_000))

results = []
worker = threading.Thread(target=lambda: results.append(down(200_000)))
worker.start()
worker.join()
print("thread", results)

sys.setrecursionlimit(5_000)
try:
    down(100_000)
except RecursionError:
    print("limit still raises RecursionError")
