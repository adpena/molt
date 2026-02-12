"""Purpose: differential coverage for select timeout edge cases."""

import select

ready = select.select([], [], [], 0)
print([len(item) for item in ready])

try:
    select.select([], [], [], -1)
except Exception as exc:
    print(type(exc).__name__)
