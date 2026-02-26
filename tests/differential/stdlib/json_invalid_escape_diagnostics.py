"""Purpose: verify json invalid-escape diagnostics align with CPython."""

import json


def show(payload: str) -> None:
    try:
        json.loads(payload)
    except Exception as exc:  # noqa: BLE001
        print(
            type(exc).__name__,
            getattr(exc, "msg", str(exc)),
            getattr(exc, "pos", None),
            getattr(exc, "colno", None),
        )


show('"\\x"')
show('"\\u12"')
show('"\\uZZZZ"')
