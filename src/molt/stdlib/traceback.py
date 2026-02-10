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
_MOLT_TRACEBACK_EXTRACT_TB = _require_intrinsic("molt_traceback_extract_tb", globals())
_MOLT_TRACEBACK_EXCEPTION_CHAIN_PAYLOAD = _require_intrinsic(
    "molt_traceback_exception_chain_payload", globals()
)
_MOLT_TRACEBACK_EXCEPTION_SUPPRESS_CONTEXT = _require_intrinsic(
    "molt_traceback_exception_suppress_context", globals()
)
_MOLT_GETFRAME = _require_intrinsic("molt_getframe", globals())


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


def _exception_chain_payload(
    exc: BaseException, limit: int | None
) -> list[tuple[BaseException, list[Any], bool, int | None, int | None]]:
    payload = _MOLT_TRACEBACK_EXCEPTION_CHAIN_PAYLOAD(exc, limit)
    if not isinstance(payload, list):
        raise RuntimeError(
            "traceback exception chain payload intrinsic returned invalid value"
        )
    count = len(payload)
    out: list[tuple[BaseException, list[Any], bool, int | None, int | None]] = []
    for entry in payload:
        if not isinstance(entry, (tuple, list)) or len(entry) != 5:
            raise RuntimeError(
                "traceback exception chain payload intrinsic returned invalid entry"
            )
        value, frames_payload, suppress_context, cause_index, context_index = entry
        if not isinstance(value, BaseException):
            raise RuntimeError(
                "traceback exception chain payload intrinsic returned invalid exception"
            )
        if not isinstance(frames_payload, list):
            raise RuntimeError(
                "traceback exception chain payload intrinsic returned invalid frames"
            )
        if not isinstance(suppress_context, bool):
            raise RuntimeError(
                "traceback exception chain payload intrinsic returned invalid suppress flag"
            )
        if cause_index is not None:
            if (
                not isinstance(cause_index, int)
                or cause_index < 0
                or cause_index >= count
            ):
                raise RuntimeError(
                    "traceback exception chain payload intrinsic returned invalid cause index"
                )
        if context_index is not None:
            if (
                not isinstance(context_index, int)
                or context_index < 0
                or context_index >= count
            ):
                raise RuntimeError(
                    "traceback exception chain payload intrinsic returned invalid context index"
                )
        out.append(
            (
                value,
                frames_payload,
                suppress_context,
                cause_index,
                context_index,
            )
        )
    return out


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
        frames: list[FrameSummary] = []
        for entry in extracted_list:
            if isinstance(entry, FrameSummary):
                frames.append(entry)
                continue
            # Match CPython's unpack semantics for tuple/list inputs:
            # only (filename, lineno, name, line) is accepted here.
            filename, lineno, name, line = entry
            lineno_i = int(lineno)
            frames.append(
                FrameSummary(
                    filename=str(filename),
                    lineno=lineno_i,
                    end_lineno=lineno_i,
                    colno=0,
                    end_colno=0,
                    name=str(name),
                    line="" if line is None else str(line),
                )
            )
        return cls(frames, source=_frame_payload_entries(frames), limit=None)

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
        if exc is None:
            self.__suppress_context__ = False
        else:
            suppress = _MOLT_TRACEBACK_EXCEPTION_SUPPRESS_CONTEXT(exc)
            if not isinstance(suppress, bool):
                raise RuntimeError(
                    "traceback suppress-context intrinsic returned invalid value"
                )
            self.__suppress_context__ = suppress

    @classmethod
    def from_exception(
        cls,
        exc: BaseException,
        limit: int | None = None,
        lookup_lines: bool = True,
        capture_locals: bool = False,
    ) -> "TracebackException":
        del lookup_lines, capture_locals
        chain = _exception_chain_payload(exc, limit)
        if not chain:
            return cls(exc, StackSummary([]))
        nodes: list[TracebackException] = []
        links: list[tuple[int | None, int | None]] = []
        for (
            current,
            frames_payload,
            suppress_context,
            cause_index,
            context_index,
        ) in chain:
            stack = StackSummary(
                _payload_to_frames(
                    frames_payload, "traceback exception chain payload intrinsic"
                ),
                source=frames_payload,
                limit=None,
            )
            current_exc = cls(current, stack)
            current_exc.__suppress_context__ = suppress_context
            nodes.append(current_exc)
            links.append((cause_index, context_index))
        for current_exc, (cause_index, context_index) in zip(nodes, links):
            if cause_index is not None:
                current_exc.__cause__ = nodes[cause_index]
            if context_index is not None:
                current_exc.__context__ = nodes[context_index]
        return nodes[0]

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
    payload = _MOLT_TRACEBACK_EXTRACT_TB(tb, limit)
    return StackSummary(
        _payload_to_frames(payload, "traceback extract tb intrinsic"),
        source=payload,
        limit=None,
    )


def format_list(extracted_list: list[Any]) -> list[str]:
    if isinstance(extracted_list, StackSummary):
        return extracted_list.format()
    return StackSummary.from_list(extracted_list).format()


def extract_stack(f: Any | None = None, limit: int | None = None) -> StackSummary:
    if f is None:
        f = _MOLT_GETFRAME(1)
        if f is None:
            raise RuntimeError("sys._getframe is unavailable")
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
        f = _MOLT_GETFRAME(1)
        if f is None:
            raise RuntimeError("sys._getframe is unavailable")
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
