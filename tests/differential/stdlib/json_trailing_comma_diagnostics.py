"""Purpose: verify json trailing-comma diagnostics align with CPython."""

import json


def show(label: str, payload: str) -> None:
    try:
        json.loads(payload)
    except Exception as exc:  # noqa: BLE001
        print(
            label,
            type(exc).__name__,
            getattr(exc, "msg", str(exc)),
            getattr(exc, "pos", None),
            getattr(exc, "colno", None),
        )


show("array_compact", "[1,2,]")
show("array_spaced", "[1, 2,]")
show("object_compact", '{"a":1,}')
show("object_spaced", '{"a": 1, }')
