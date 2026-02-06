"""Traceback formatting helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


import sys
from typing import Any

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_TRACEBACK_SOURCE_LINE = _require_intrinsic(
    "molt_traceback_source_line", globals()
)
_MOLT_TRACEBACK_FORMAT_EXCEPTION_ONLY = _require_intrinsic(
    "molt_traceback_format_exception_only", globals()
)
_MOLT_TRACEBACK_FORMAT_TB = _require_intrinsic("molt_traceback_format_tb", globals())
_MOLT_TRACEBACK_EXTRACT_TB = _require_intrinsic("molt_traceback_extract_tb", globals())


__all__ = [
    "extract_tb",
    "extract_stack",
    "format_exception",
    "format_exception_only",
    "format_list",
    "format_stack",
    "format_tb",
    "format_exc",
    "print_exception",
    "print_list",
    "print_stack",
    "print_tb",
    "print_exc",
    "FrameSummary",
    "StackSummary",
    "TracebackException",
]

_CHAIN_CAUSE = (
    "The above exception was the direct cause of the following exception:\n\n"
)
_CHAIN_CONTEXT = (
    "During handling of the above exception, another exception occurred:\n\n"
)


def _exc_name(exc_type: Any, value: Any) -> str:
    if exc_type is None and value is not None:
        exc_type = type(value)
    if exc_type is None:
        return "Exception"
    return getattr(exc_type, "__name__", str(exc_type))


def format_exception_only(exc_type: Any, value: Any) -> list[str]:
    lines = _MOLT_TRACEBACK_FORMAT_EXCEPTION_ONLY(exc_type, value)
    if not isinstance(lines, list) or not all(isinstance(line, str) for line in lines):
        raise RuntimeError(
            "traceback format_exception_only intrinsic returned invalid value"
        )
    return list(lines)


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
    if lineno <= 0:
        return ""
    try:
        line = _MOLT_TRACEBACK_SOURCE_LINE(filename, int(lineno))
    except Exception:
        return ""
    if not isinstance(line, str):
        return ""
    return line


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


def _format_caret_line(line: str, colno: int, end_colno: int) -> str:
    if not line:
        return ""
    if colno < 0:
        return ""
    text_len = len(line)
    if text_len <= 0:
        return ""
    if end_colno < colno:
        end_colno = colno
    if colno > text_len:
        colno = text_len
    if end_colno > text_len:
        end_colno = text_len
    width = end_colno - colno
    if width <= 0:
        width = 1
    return "    " + (" " * colno) + ("^" * width) + "\n"


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


class StackSummary:
    def __init__(self, frames: list[FrameSummary]) -> None:
        self._frames = list(frames)

    @classmethod
    def extract(cls, tb: Any, limit: int | None = None) -> "StackSummary":
        frames: list[FrameSummary] = []
        if isinstance(tb, (tuple, list)):
            for entry in tb:
                frame = _frame_summary_from_entry(entry)
                if frame is not None:
                    frames.append(frame)
                if limit is not None and len(frames) >= limit:
                    break
        else:
            extracted = _MOLT_TRACEBACK_EXTRACT_TB(tb, limit)
            if isinstance(extracted, list):
                for entry in extracted:
                    frame = _frame_summary_from_entry(entry)
                    if frame is not None:
                        frames.append(frame)
            else:
                while tb is not None:
                    frames.append(_frame_summary_from_tb(tb))
                    tb = getattr(tb, "tb_next", None)
                if limit is not None:
                    frames = frames[:limit]
        return cls(frames)

    @classmethod
    def from_list(cls, extracted_list: list[Any]) -> "StackSummary":
        frames: list[FrameSummary] = []
        for entry in extracted_list:
            frame = _frame_summary_from_entry(entry)
            if frame is not None:
                frames.append(frame)
        return cls(frames)

    def __iter__(self):
        return iter(self._frames)

    def __len__(self) -> int:
        return len(self._frames)

    def __getitem__(self, index: int) -> FrameSummary:
        return self._frames[index]

    def format(self) -> list[str]:
        lines: list[str] = []
        for frame in self._frames:
            lines.append(
                f'  File "{frame.filename}", line {frame.lineno}, in {frame.name}\n'
            )
            if frame.line:
                lines.append(f"    {frame.line}\n")
                caret = _format_caret_line(frame.line, frame.colno, frame.end_colno)
                if caret:
                    lines.append(caret)
        return lines


def _frame_summary_from_frame(frame: Any) -> FrameSummary:
    code = getattr(frame, "f_code", None)
    filename = getattr(code, "co_filename", "<unknown>") if code else "<unknown>"
    name = getattr(code, "co_name", "<module>") if code else "<module>"
    lineno = getattr(frame, "f_lineno", 0) or 0
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


def _frame_summary_from_entry(entry: Any) -> FrameSummary | None:
    if isinstance(entry, FrameSummary):
        return entry
    if isinstance(entry, (tuple, list)) and len(entry) >= 7:
        filename, lineno = entry[0], entry[1]
        end_lineno, colno, end_colno = entry[2], entry[3], entry[4]
        name, line = entry[5], entry[6]
        text = "" if line is None else str(line)
        return FrameSummary(
            filename=str(filename),
            lineno=int(lineno),
            end_lineno=int(end_lineno),
            colno=int(colno),
            end_colno=int(end_colno),
            name=str(name),
            line=text,
        )
    if isinstance(entry, (tuple, list)) and len(entry) >= 4:
        filename, lineno, name, line = entry[0], entry[1], entry[2], entry[3]
        text = "" if line is None else str(line)
        colno, end_colno = _infer_col_offsets(text)
        return FrameSummary(
            filename=str(filename),
            lineno=int(lineno),
            end_lineno=int(lineno),
            colno=colno,
            end_colno=end_colno,
            name=str(name),
            line=text,
        )
    info = _extract_tb_info(entry)
    if info is None:
        return None
    filename, lineno, name = info
    line = _get_source_line(filename, lineno) if lineno else ""
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
    def __init__(self, exc: BaseException | None, stack: StackSummary) -> None:
        self.stack = stack
        self.exc_type = type(exc) if exc is not None else None
        self._exc = exc
        self.__cause__: TracebackException | None = None
        self.__context__: TracebackException | None = None
        self.__suppress_context__ = bool(getattr(exc, "__suppress_context__", False))

    @classmethod
    def from_exception(
        cls,
        exc: BaseException,
        limit: int | None = None,
        lookup_lines: bool = True,
        capture_locals: bool = False,
    ) -> "TracebackException":
        del lookup_lines, capture_locals
        seen: set[int] = set()

        def _convert(current: BaseException) -> TracebackException:
            key = id(current)
            if key in seen:
                return cls(current, StackSummary([]))
            seen.add(key)
            tb = getattr(current, "__traceback__", None)
            stack = StackSummary.extract(tb, limit)
            current_exc = cls(current, stack)
            cause = getattr(current, "__cause__", None)
            if cause is not None:
                current_exc.__cause__ = _convert(cause)
            context = getattr(current, "__context__", None)
            if context is not None:
                current_exc.__context__ = _convert(context)
            current_exc.__suppress_context__ = bool(
                getattr(current, "__suppress_context__", False)
            )
            return current_exc

        return _convert(exc)

    def format(self, *, chain: bool = True) -> list[str]:
        seen: set[int] = set()

        def _format_one(current: TracebackException) -> list[str]:
            lines: list[str] = []
            if len(current.stack):
                lines.append("Traceback (most recent call last):\n")
                lines.extend(current.stack.format())
            lines.extend(format_exception_only(current.exc_type, current._exc))
            return lines

        def _format_chain(current: TracebackException) -> list[str]:
            key = id(current)
            if key in seen:
                return _format_one(current)
            seen.add(key)
            lines: list[str] = []
            if chain:
                if current.__cause__ is not None:
                    lines.extend(_format_chain(current.__cause__))
                    lines.append(_CHAIN_CAUSE)
                elif (
                    current.__context__ is not None and not current.__suppress_context__
                ):
                    lines.extend(_format_chain(current.__context__))
                    lines.append(_CHAIN_CONTEXT)
            lines.extend(_format_one(current))
            return lines

        return _format_chain(self)


def format_tb(tb: Any, limit: int | None = None) -> list[str]:
    lines = _MOLT_TRACEBACK_FORMAT_TB(tb, limit)
    if not isinstance(lines, list) or not all(isinstance(line, str) for line in lines):
        raise RuntimeError("traceback format_tb intrinsic returned invalid value")
    return list(lines)


def extract_tb(tb: Any, limit: int | None = None) -> StackSummary:
    extracted = _MOLT_TRACEBACK_EXTRACT_TB(tb, limit)
    if not isinstance(extracted, list):
        raise RuntimeError("traceback extract_tb intrinsic returned invalid value")
    return StackSummary.from_list(extracted)


def format_list(extracted_list: list[Any]) -> list[str]:
    return StackSummary.from_list(extracted_list).format()


def extract_stack(f: Any | None = None, limit: int | None = None) -> StackSummary:
    if f is None:
        getter = getattr(sys, "_getframe", None)
        if getter is None:
            return StackSummary([])
        f = getter(1)
    stack: list[FrameSummary] = []
    while f is not None:
        stack.append(_frame_summary_from_frame(f))
        f = getattr(f, "f_back", None)
    stack.reverse()
    if limit is not None:
        stack = stack[-limit:]
    return StackSummary(stack)


def format_stack(f: Any | None = None, limit: int | None = None) -> list[str]:
    return extract_stack(f, limit).format()


def _format_exception_single(
    exc_type: Any, value: Any, tb: Any, limit: int | None
) -> list[str]:
    lines: list[str] = []
    if tb is not None:
        lines.append("Traceback (most recent call last):\n")
        lines.extend(format_tb(tb, limit))
    lines.extend(format_exception_only(exc_type, value))
    return lines


def _format_exception_chain(
    exc_type: Any,
    value: Any,
    tb: Any,
    limit: int | None,
    chain: bool,
    seen: set[int],
) -> list[str]:
    if value is None or not chain:
        return _format_exception_single(exc_type, value, tb, limit)
    key = id(value)
    if key in seen:
        return _format_exception_single(exc_type, value, tb, limit)
    seen.add(key)
    cause = getattr(value, "__cause__", None)
    if cause is not None:
        lines = _format_exception_chain(
            type(cause),
            cause,
            getattr(cause, "__traceback__", None),
            limit,
            chain,
            seen,
        )
        lines.append(_CHAIN_CAUSE)
        lines.extend(_format_exception_single(exc_type, value, tb, limit))
        return lines
    context = getattr(value, "__context__", None)
    suppress = bool(getattr(value, "__suppress_context__", False))
    if context is not None and not suppress:
        lines = _format_exception_chain(
            type(context),
            context,
            getattr(context, "__traceback__", None),
            limit,
            chain,
            seen,
        )
        lines.append(_CHAIN_CONTEXT)
        lines.extend(_format_exception_single(exc_type, value, tb, limit))
        return lines
    return _format_exception_single(exc_type, value, tb, limit)


def format_exception(
    exc_type: Any,
    value: Any,
    tb: Any,
    limit: int | None = None,
    chain: bool = True,
) -> list[str]:
    return _format_exception_chain(exc_type, value, tb, limit, chain, seen=set())


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
    print(out, end="")


def print_list(extracted_list: list[Any], file: Any | None = None) -> None:
    out = "".join(format_list(extracted_list))
    if file is not None and hasattr(file, "write"):
        file.write(out)
        return
    print(out, end="")


def print_stack(
    f: Any | None = None, limit: int | None = None, file: Any | None = None
) -> None:
    if f is None:
        getter = getattr(sys, "_getframe", None)
        f = getter().f_back if callable(getter) else None
    out = "".join(format_stack(f, limit))
    if file is not None and hasattr(file, "write"):
        file.write(out)
        return
    print(out, end="")


def print_tb(tb: Any, limit: int | None = None, file: Any | None = None) -> None:
    out = "".join(format_tb(tb, limit))
    if file is not None and hasattr(file, "write"):
        file.write(out)
        return
    print(out, end="")


def format_exc(limit: int | None = None) -> str:
    exc_type, value, tb = sys.exc_info()
    return "".join(format_exception(exc_type, value, tb, limit))


def print_exc(limit: int | None = None, file: Any | None = None) -> None:
    exc_type, value, tb = sys.exc_info()
    print_exception(exc_type, value, tb, limit, file)
