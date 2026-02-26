"""Purpose: differential coverage for json.JSONDecoder.raw_decode idx semantics."""

import json


decoder = json.JSONDecoder()


def show(label, thunk):
    try:
        print(label, thunk())
    except Exception as exc:  # noqa: BLE001
        print(
            label,
            type(exc).__name__,
            getattr(exc, "msg", str(exc)),
            getattr(exc, "pos", None),
            getattr(exc, "colno", None),
        )


show("idx_1_whitespace", lambda: decoder.raw_decode(" 1", 1))
show("idx_0_whitespace", lambda: decoder.raw_decode(" 1", 0))
show("idx_negative", lambda: decoder.raw_decode('{"a": 1}', -1))
show("idx_large", lambda: decoder.raw_decode('{"a": 1}', 100))
show("idx_float", lambda: decoder.raw_decode('{"a": 1}', 0.5))
