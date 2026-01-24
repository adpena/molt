"""Purpose: differential coverage for JSONDecodeError detail fields."""

import json

try:
    json.loads("{\"a\": 1, \"b\": }")
except Exception as exc:
    print("err", type(exc).__name__, getattr(exc, "msg", None), getattr(exc, "lineno", None), getattr(exc, "colno", None), getattr(exc, "pos", None))
