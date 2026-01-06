"""Traceback formatting helpers for Molt."""

from __future__ import annotations

from typing import Any

__all__ = [
    "format_exception",
    "format_exception_only",
    "format_tb",
    "format_exc",
    "print_exception",
    "print_tb",
    "print_exc",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): add full traceback extraction + chaining details.


def _exc_name(exc_type: Any, value: Any) -> str:
    if exc_type is None and value is not None:
        exc_type = type(value)
    if exc_type is None:
        return "Exception"
    return getattr(exc_type, "__name__", str(exc_type))


def format_exception_only(exc_type: Any, value: Any) -> list[str]:
    name = _exc_name(exc_type, value)
    if value is None or value == "":
        return [f"{name}\n"]
    return [f"{name}: {value}\n"]


def _format_tb_entry(tb: Any) -> str:
    frame = getattr(tb, "tb_frame", None)
    lineno = getattr(tb, "tb_lineno", None)
    if frame is None or lineno is None:
        return "<traceback>\n"
    code = getattr(frame, "f_code", None)
    filename = getattr(code, "co_filename", "<unknown>") if code else "<unknown>"
    name = getattr(code, "co_name", "<module>") if code else "<module>"
    return f'  File "{filename}", line {lineno}, in {name}\n'


def format_tb(tb: Any, limit: int | None = None) -> list[str]:
    lines: list[str] = []
    count = 0
    while tb is not None:
        lines.append(_format_tb_entry(tb))
        tb = getattr(tb, "tb_next", None)
        count += 1
        if limit is not None and count >= limit:
            break
    return lines


def format_exception(
    exc_type: Any,
    value: Any,
    tb: Any,
    limit: int | None = None,
    chain: bool = True,
) -> list[str]:
    _ = chain
    lines: list[str] = []
    if tb is not None:
        lines.append("Traceback (most recent call last):\n")
        lines.extend(format_tb(tb, limit=limit))
    lines.extend(format_exception_only(exc_type, value))
    return lines


def print_exception(
    exc_type: Any,
    value: Any,
    tb: Any,
    limit: int | None = None,
    file: Any | None = None,
    chain: bool = True,
) -> None:
    out = "".join(format_exception(exc_type, value, tb, limit=limit, chain=chain))
    if file is not None and hasattr(file, "write"):
        file.write(out)
        return
    print(out, end="")


def print_tb(tb: Any, limit: int | None = None, file: Any | None = None) -> None:
    out = "".join(format_tb(tb, limit=limit))
    if file is not None and hasattr(file, "write"):
        file.write(out)
        return
    print(out, end="")


def format_exc(limit: int | None = None) -> str:
    try:
        import sys

        exc_type, value, tb = sys.exc_info()
    except Exception:
        return ""
    return "".join(format_exception(exc_type, value, tb, limit=limit))


def print_exc(limit: int | None = None, file: Any | None = None) -> None:
    try:
        import sys

        exc_type, value, tb = sys.exc_info()
    except Exception:
        return None
    print_exception(exc_type, value, tb, limit=limit, file=file)
