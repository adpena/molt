"""Purpose: differential coverage for PEP 680 tomllib basics."""

import tomllib


payload = "title = 'Molt'\\ncount = 3\\nflags = [true, false]\\n"
print(tomllib.loads(payload))

try:
    tomllib.loads("a = 1\\na = 2\\n")
except Exception as exc:
    print(type(exc).__name__, exc)
