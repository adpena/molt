"""Purpose: differential coverage for JSON error positions."""

import json


def show(payload):
    try:
        json.loads(payload)
        print("ok", payload)
    except Exception as exc:
        print("err", type(exc).__name__, getattr(exc, "lineno", None), getattr(exc, "colno", None), getattr(exc, "pos", None))


show("[1, 2,]")
show("{\"a\": 1,}")
show("{\"a\": 1 \"b\": 2}")
