"""Purpose: differential coverage for tomllib error positions."""

import tomllib


def show_error(payload: str) -> None:
    try:
        tomllib.loads(payload)
    except Exception as exc:
        print(type(exc).__name__, getattr(exc, "lineno", None), getattr(exc, "colno", None))


show_error("a = 1\\n")
show_error("a =\\n")
