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
    "TracebackException",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): add full traceback extraction + chaining details.
# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full FrameSummary/TracebackException fields and rich formatting per PEP 657.


def _exc_name(exc_type: Any, value: Any) -> str:
    if exc_type is None and value is not None:
        exc_type = type(value)
    if exc_type is None:
        return "Exception"
    return getattr(exc_type, "__name__", str(exc_type))


def format_exception_only(exc_type: Any, value: Any) -> list[str]:
    name = _exc_name(exc_type, value)
    if value is None:
        return [f"{name}\n"]
    text = str(value)
    if text == "":
        return [f"{name}\n"]
    return [f"{name}: {text}\n"]


def _split_molt_symbol(name: str) -> tuple[str, str]:
    if "__" in name:
        module_hint, func = name.split("__", 1)
        if module_hint:
            return f"<molt:{module_hint}>", func or name
    return "<molt>", name


def _extract_tb_info(entry: Any) -> tuple[str, int, str] | None:
    frame = getattr(entry, "tb_frame", None)
    lineno = getattr(entry, "tb_lineno", None)
    if frame is not None and lineno is not None:
        code = getattr(frame, "f_code", None)
        filename = getattr(code, "co_filename", "<unknown>") if code else "<unknown>"
        name = getattr(code, "co_name", "<module>") if code else "<module>"
        return str(filename), int(lineno), str(name)
    filename = getattr(entry, "filename", None)
    lineno = getattr(entry, "lineno", None)
    if filename is not None and lineno is not None:
        name = getattr(entry, "name", "<module>")
        return str(filename), int(lineno), str(name)
    if isinstance(entry, dict):
        if "filename" in entry and "lineno" in entry:
            name = entry.get("name", "<module>")
            return str(entry["filename"]), int(entry["lineno"]), str(name)
    if isinstance(entry, (tuple, list)):
        if not entry:
            return None
        if len(entry) == 1:
            return _extract_tb_info(entry[0])
        if len(entry) == 2:
            first, second = entry
            if isinstance(first, str) and isinstance(second, int):
                return first, int(second), "<module>"
            if isinstance(second, str) and isinstance(first, int):
                return second, int(first), "<module>"
            if isinstance(first, str) and isinstance(second, str):
                filename, name = _split_molt_symbol(first)
                return filename, 0, name
        if len(entry) >= 3:
            first, second, third = entry[0], entry[1], entry[2]
            if isinstance(second, int):
                return str(first), int(second), str(third)
            if isinstance(third, int):
                return str(second), int(third), str(first)
    if isinstance(entry, str):
        filename, name = _split_molt_symbol(entry)
        return filename, 0, name
    return None


def _format_tb_entry(tb: Any) -> str:
    info = _extract_tb_info(tb)
    if info is None:
        return "<traceback>\n"
    filename, lineno, name = info
    return f'  File "{filename}", line {lineno}, in {name}\n'


def _get_source_line(filename: str, lineno: int) -> str:
    try:
        with open(filename, "r") as handle:  # noqa: PTH123 - trusted mode in diff tests
            for idx, line in enumerate(handle, 1):
                if idx == lineno:
                    return line.rstrip("\n")
    except Exception:
        return ""
    return ""


def _infer_col_offsets(line: str) -> tuple[int, int]:
    if not line:
        return 0, 0
    stripped = line.lstrip()
    indent = len(line) - len(stripped)
    if stripped.startswith("return "):
        col = indent + len("return ")
    else:
        col = indent
    end = len(line)
    return col, end


class FrameSummary:
    def __init__(
        self,
        *,
        filename: str,
        lineno: int,
        end_lineno: int,
        colno: int,
        end_colno: int,
        name: str,
        line: str,
    ) -> None:
        self.filename = filename
        self.lineno = lineno
        self.end_lineno = end_lineno
        self.colno = colno
        self.end_colno = end_colno
        self.name = name
        self.line = line


def _frame_summary_from_tb(tb: Any) -> FrameSummary:
    frame = getattr(tb, "tb_frame", None)
    lineno = getattr(tb, "tb_lineno", None)
    if lineno is None:
        lineno = 0
    filename = "<unknown>"
    name = "<module>"
    if frame is not None:
        code = getattr(frame, "f_code", None)
        if code is not None:
            filename = getattr(code, "co_filename", filename)
            name = getattr(code, "co_name", name)
    line = _get_source_line(str(filename), int(lineno)) if lineno else ""
    colno, end_colno = _infer_col_offsets(line)
    return FrameSummary(
        filename=str(filename),
        lineno=int(lineno),
        end_lineno=int(lineno),
        colno=colno,
        end_colno=end_colno,
        name=str(name),
        line=line,
    )


class TracebackException:
    def __init__(self, exc: BaseException | None, stack: list[FrameSummary]) -> None:
        self.stack = stack
        self.exc_type = type(exc) if exc is not None else None
        self._exc = exc

    @classmethod
    def from_exception(cls, exc: BaseException) -> "TracebackException":
        tb = getattr(exc, "__traceback__", None)
        stack: list[FrameSummary] = []
        while tb is not None:
            stack.append(_frame_summary_from_tb(tb))
            tb = getattr(tb, "tb_next", None)
        stack.reverse()
        return cls(exc, stack)


def format_tb(tb: Any, limit: int | None = None) -> list[str]:
    lines: list[str] = []
    count = 0
    if isinstance(tb, (tuple, list)):
        for entry in tb:
            lines.append(_format_tb_entry(entry))
            count += 1
            if limit is not None and count >= limit:
                break
    else:
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
        lines.extend(format_tb(tb, limit))
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
    out = "".join(format_exception(exc_type, value, tb, limit, chain))
    if file is not None and hasattr(file, "write"):
        file.write(out)
        return
    print(out)


def print_tb(tb: Any, limit: int | None = None, file: Any | None = None) -> None:
    out = "".join(format_tb(tb, limit))
    if file is not None and hasattr(file, "write"):
        file.write(out)
        return
    print(out)


def format_exc(limit: int | None = None) -> str:
    try:
        import sys

        exc_type, value, tb = sys.exc_info()
    except Exception:
        return ""
    return "".join(format_exception(exc_type, value, tb, limit))


def print_exc(limit: int | None = None, file: Any | None = None) -> None:
    try:
        import sys

        exc_type, value, tb = sys.exc_info()
    except Exception:
        return None
    print_exception(exc_type, value, tb, limit, file)
