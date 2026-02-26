"""Purpose: verify raw_decode out-of-range idx line/column diagnostics."""

import json


decoder = json.JSONDecoder()

try:
    decoder.raw_decode("1\n2", 10)
except Exception as exc:  # noqa: BLE001
    print(
        type(exc).__name__,
        getattr(exc, "msg", str(exc)),
        getattr(exc, "pos", None),
        getattr(exc, "lineno", None),
        getattr(exc, "colno", None),
    )
