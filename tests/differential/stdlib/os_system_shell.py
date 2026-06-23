# MOLT_ENV: MOLT_CAPABILITIES=process.exec
"""Purpose: differential coverage for os.system shell execution."""

import os


print("empty", os.system(""))
print("exit7", os.system("exit 7"))

for value in (None, 1):
    try:
        os.system(value)
    except Exception as exc:
        print("type", type(value).__name__, type(exc).__name__, str(exc))
