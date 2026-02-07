"""Traceback formatting helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import sys

Any = object

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_TRACEBACK_PAYLOAD = _require_intrinsic("molt_traceback_payload", globals())
_MOLT_TRACEBACK_FORMAT_EXCEPTION_ONLY = _require_intrinsic(
    "molt_traceback_format_exception_only", globals()
)
_MOLT_TRACEBACK_FORMAT_EXCEPTION = _require_intrinsic(
    "molt_traceback_format_exception", globals()
)
_MOLT_TRACEBACK_FORMAT_TB = _require_intrinsic("molt_traceback_format_tb", globals())
_MOLT_TRACEBACK_FORMAT_STACK = _require_intrinsic(
    "molt_traceback_format_stack", globals()
)
_MOLT_TRACEBACK_EXCEPTION_COMPONENTS = _require_intrinsic(
    "molt_traceback_exception_components", globals()
)


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


def format_exception_only(exc_type: Any, value: Any) -> list[str]:
    lines = _MOLT_TRACEBACK_FORMAT_EXCEPTION_ONLY(exc_type, value)
    if not isinstance(lines, list) or not all(isinstance(line, str) for line in lines):
        raise RuntimeError(
            "traceback format_exception_only intrinsic returned invalid value"
        )
    return list(lines)


def _validate_string_list(value: Any, label: str) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(line, str) for line in value):
        raise RuntimeError(f"{label} returned invalid value")
    return list(value)


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


def _frame_payload_entries(frames: list[FrameSummary]) -> list[tuple[Any, ...]]:
    return [
        (
            frame.filename,
            frame.lineno,
            frame.end_lineno,
            frame.colno,
            frame.end_colno,
            frame.name,
            frame.line,
        )
        for frame in frames
    ]


def _payload_to_frames(payload: Any, label: str) -> list[FrameSummary]:
    if not isinstance(payload, list):
        raise RuntimeError(f"{label} returned invalid value")
    frames: list[FrameSummary] = []
    for entry in payload:
        if not isinstance(entry, (tuple, list)) or len(entry) < 7:
            raise RuntimeError(f"{label} returned invalid value")
        filename = str(entry[0])
        lineno = int(entry[1])
        end_lineno = int(entry[2])
        colno = int(entry[3])
        end_colno = int(entry[4])
        name = str(entry[5])
        line = "" if entry[6] is None else str(entry[6])
        frames.append(
            FrameSummary(
                filename=filename,
                lineno=lineno,
                end_lineno=end_lineno,
                colno=colno,
                end_colno=end_colno,
                name=name,
                line=line,
            )
        )
    return frames


def _payload_frames(source: Any, limit: int | None) -> list[FrameSummary]:
    payload = _MOLT_TRACEBACK_PAYLOAD(source, limit)
    return _payload_to_frames(payload, "traceback payload intrinsic")


def _exception_components_payload(
    exc: BaseException, limit: int | None
) -> tuple[list[Any], Any, Any, bool]:
    payload = _MOLT_TRACEBACK_EXCEPTION_COMPONENTS(exc, limit)
    if not isinstance(payload, (tuple, list)) or len(payload) != 4:
        raise RuntimeError(
            "traceback exception components intrinsic returned invalid value"
        )
    frames_payload = payload[0]
    cause = payload[1]
    context = payload[2]
    suppress_context = payload[3]
    if not isinstance(frames_payload, list):
        raise RuntimeError(
            "traceback exception components intrinsic returned invalid frames payload"
        )
    if cause is not None and not isinstance(cause, BaseException):
        raise RuntimeError(
            "traceback exception components intrinsic returned invalid cause payload"
        )
    if context is not None and not isinstance(context, BaseException):
        raise RuntimeError(
            "traceback exception components intrinsic returned invalid context payload"
        )
    if not isinstance(suppress_context, bool):
        raise RuntimeError(
            "traceback exception components intrinsic returned invalid suppress flag"
        )
    return frames_payload, cause, context, suppress_context


class StackSummary:
    def __init__(
        self,
        frames: list[FrameSummary],
        source: Any | None = None,
        limit: int | None = None,
    ) -> None:
        self._frames = list(frames)
        self._source = (
            source if source is not None else _frame_payload_entries(self._frames)
        )
        self._limit = limit

    @classmethod
    def extract(cls, source: Any, limit: int | None = None) -> "StackSummary":
        return cls(_payload_frames(source, limit), source=source, limit=limit)

    @classmethod
    def from_list(cls, extracted_list: list[Any]) -> "StackSummary":
        return cls(
            _payload_frames(extracted_list, None), source=extracted_list, limit=None
        )

    def __iter__(self):
        return iter(self._frames)

    def __len__(self) -> int:
        return len(self._frames)

    def __getitem__(self, index: int) -> FrameSummary:
        return self._frames[index]

    def format(self) -> list[str]:
        lines = _MOLT_TRACEBACK_FORMAT_STACK(self._source, self._limit)
        return _validate_string_list(lines, "traceback format stack intrinsic")


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
            frames_payload, cause, context, suppress_context = (
                _exception_components_payload(current, limit)
            )
            stack = StackSummary(
                _payload_to_frames(
                    frames_payload, "traceback exception components intrinsic"
                ),
                source=frames_payload,
                limit=None,
            )
            current_exc = cls(current, stack)
            if cause is not None:
                current_exc.__cause__ = _convert(cause)
            if context is not None:
                current_exc.__context__ = _convert(context)
            current_exc.__suppress_context__ = suppress_context
            return current_exc

        return _convert(exc)

    def format(self, *, chain: bool = True) -> list[str]:
        if self._exc is None:
            lines: list[str] = []
            if len(self.stack):
                lines.append("Traceback (most recent call last):\n")
                lines.extend(self.stack.format())
            lines.extend(format_exception_only(self.exc_type, self._exc))
            return lines
        lines = _MOLT_TRACEBACK_FORMAT_EXCEPTION(
            self.exc_type, self._exc, None, None, bool(chain)
        )
        return _validate_string_list(lines, "traceback format exception intrinsic")


def format_tb(tb: Any, limit: int | None = None) -> list[str]:
    lines = _MOLT_TRACEBACK_FORMAT_TB(tb, limit)
    return _validate_string_list(lines, "traceback format tb intrinsic")


def extract_tb(tb: Any, limit: int | None = None) -> StackSummary:
    payload = _MOLT_TRACEBACK_PAYLOAD(tb, limit)
    return StackSummary(
        _payload_to_frames(payload, "traceback payload intrinsic"),
        source=payload,
        limit=None,
    )


def format_list(extracted_list: list[Any]) -> list[str]:
    if isinstance(extracted_list, StackSummary):
        return extracted_list.format()
    return StackSummary.from_list(extracted_list).format()


def extract_stack(f: Any | None = None, limit: int | None = None) -> StackSummary:
    if f is None:
        getter = getattr(sys, "_getframe", None)
        if not callable(getter):
            raise RuntimeError("sys._getframe is unavailable")
        f = getter(1)
    return StackSummary.extract(f, limit)


def format_stack(f: Any | None = None, limit: int | None = None) -> list[str]:
    return extract_stack(f, limit).format()


def format_exception(
    exc_type: Any,
    value: Any,
    tb: Any,
    limit: int | None = None,
    chain: bool = True,
) -> list[str]:
    lines = _MOLT_TRACEBACK_FORMAT_EXCEPTION(exc_type, value, tb, limit, bool(chain))
    return _validate_string_list(lines, "traceback format exception intrinsic")


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
        if not callable(getter):
            raise RuntimeError("sys._getframe is unavailable")
        f = getter(1)
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
