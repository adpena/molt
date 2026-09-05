"""Known eager Python-import facts from custodied C extension sources.

This is a lexical projection, not a C evaluator or a dynamic-import admission
policy. Unknown names, object expressions and relative package contexts remain
runtime concerns. A completely scanned source set does not prove that arbitrary
dynamic imports are statically closed.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Literal

from molt.cli.python_module_names import canonical_python_module_name


@dataclass(frozen=True, slots=True)
class _ImportCall:
    context: Literal["eager", "init"]
    name_kind: Literal["c_string", "python_object"] = "c_string"
    level_argument: int | None = None


_C_IMPORT = _ImportCall("init")
_OBJECT_IMPORT = _ImportCall("init", "python_object")
_EAGER_IMPORT = _ImportCall("eager")
_PYTHON_IMPORT_CALLS = {
    "PyImport_ImportModule": _C_IMPORT,
    "PyImport_ImportModuleNoBlock": _C_IMPORT,
    "PyImport_ImportFrozenModule": _C_IMPORT,
    "PyImport_ImportModuleLevel": _ImportCall("init", level_argument=4),
    "PyImport_Import": _OBJECT_IMPORT,
    "PyImport_ImportFrozenModuleObject": _OBJECT_IMPORT,
    "PyImport_ImportModuleLevelObject": _ImportCall(
        "init", "python_object", level_argument=4
    ),
}
_SOURCE_EXTENSION_EAGER_IMPORT_CALLEES = frozenset({"IMPORT_GLOBAL", "IMPORT_NAME"})
_SOURCE_EXTENSION_GENERIC_IMPORT_CALLEES = frozenset(
    {"npy_cache_import", "npy_cache_import_runtime", "npy_import"}
)
_SOURCE_EXTENSION_CYTHON_MODINIT_PREFIX = "__Pyx_modinit_"
_ZERO_C_INTEGER = re.compile(r"(?:0+|0[xX]0+|0[bB]0+)[uUlL]*\Z")
_C_SIMPLE_STRING_ESCAPES = {
    "'": b"'",
    '"': b'"',
    "?": b"?",
    "\\": b"\\",
    "a": b"\a",
    "b": b"\b",
    "f": b"\f",
    "n": b"\n",
    "r": b"\r",
    "t": b"\t",
    "v": b"\v",
}


def _source_extension_runtime_import_callee(callee: str) -> _ImportCall | None:
    if callee in _SOURCE_EXTENSION_EAGER_IMPORT_CALLEES:
        return _EAGER_IMPORT
    if callee.startswith("PyImport_"):
        # AddModule creates/looks up entries, ExecCodeModule executes supplied
        # code, and GetImporter queries a path finder. None imports a dependency
        # just because it shares this C API namespace.
        return _PYTHON_IMPORT_CALLS.get(callee)
    if callee in _SOURCE_EXTENSION_GENERIC_IMPORT_CALLEES:
        return _C_IMPORT
    lowered = callee.lower()
    if (
        lowered == "import"
        or lowered.startswith("import_")
        or lowered.endswith("_import")
        or "_import_" in lowered
    ):
        return _C_IMPORT
    return None


def _source_extension_cython_eager_modinit_function_name(function_name: str) -> bool:
    return function_name.startswith(_SOURCE_EXTENSION_CYTHON_MODINIT_PREFIX)


def _source_extension_eager_import_function_name(function_name: str | None) -> bool:
    if function_name is None:
        return False
    lowered = function_name.lower()
    return (
        function_name == "exec"
        or function_name == "module_exec"
        or function_name.startswith("PyInit_")
        or "pymod_exec" in lowered
        or lowered.endswith("_exec")
        or _source_extension_cython_eager_modinit_function_name(function_name)
    )


def _skip_c_ws_comments(source: str, pos: int) -> int:
    while pos < len(source):
        if source[pos].isspace():
            pos += 1
        elif source.startswith("//", pos):
            end = source.find("\n", pos + 2)
            while end >= 0 and source[max(0, end - 2) : end].endswith(("\\", "\\\r")):
                end = source.find("\n", end + 1)
            pos = len(source) if end < 0 else end + 1
        elif source.startswith("/*", pos):
            end = source.find("*/", pos + 2)
            pos = len(source) if end < 0 else end + 2
        else:
            break
    return pos


def _skip_c_string_or_char(source: str, pos: int) -> int:
    quote = source[pos]
    if quote == '"' and pos > 0 and source[pos - 1] == "R":
        prefix_start = pos - 1
        while prefix_start > 0 and (
            source[prefix_start - 1].isalnum() or source[prefix_start - 1] == "_"
        ):
            prefix_start -= 1
        if source[prefix_start:pos] in {"R", "u8R", "uR", "UR", "LR"}:
            opening = source.find("(", pos + 1, pos + 18)
            if opening >= 0:
                delimiter = source[pos + 1 : opening]
                if not any(ch.isspace() or ch in "\\)" for ch in delimiter):
                    closing = ")" + delimiter + '"'
                    end = source.find(closing, opening + 1)
                    return len(source) if end < 0 else end + len(closing)
    pos += 1
    while pos < len(source):
        ch = source[pos]
        pos += 1
        if ch == "\\":
            pos = min(pos + 1, len(source))
        elif ch == quote:
            break
    return pos


def _call_arguments(
    source: str, opening: int
) -> tuple[tuple[tuple[int, int], ...], int] | None:
    """Locate argument spans without interpreting their C expressions."""
    pos = opening + 1
    start = pos
    closers = [")"]
    arguments: list[tuple[int, int]] = []
    while pos < len(source):
        pos = _skip_c_ws_comments(source, pos)
        if pos >= len(source):
            break
        ch = source[pos]
        if ch in {'"', "'"}:
            pos = _skip_c_string_or_char(source, pos)
            continue
        if ch in "([{":
            closers.append({"(": ")", "[": "]", "{": "}"}[ch])
        elif ch in ")]}":
            if ch != closers[-1]:
                return None
            closers.pop()
            if not closers:
                arguments.append((start, pos))
                return tuple(arguments), pos + 1
        elif ch == "," and len(closers) == 1:
            arguments.append((start, pos))
            start = pos + 1
        pos += 1
    return None


def _identifier_end(source: str, start: int) -> int:
    pos = start + 1
    while pos < len(source) and (source[pos] == "_" or source[pos].isalnum()):
        pos += 1
    return pos


def _source_extension_function_name_before_brace(
    source: str, declaration_start: int, brace_pos: int
) -> str | None:
    # Only this declaration may name the opening scope. A whole-file rfind('(')
    # would leak a previously closed module_exec into a later extern-C block.
    pos = declaration_start
    candidate: str | None = None
    while pos < brace_pos:
        pos = _skip_c_ws_comments(source, pos)
        if pos >= brace_pos:
            break
        if source[pos] in {'"', "'"}:
            pos = _skip_c_string_or_char(source, pos)
            continue
        if source[pos] == "_" or source[pos].isalpha():
            end = _identifier_end(source, pos)
            name = source[pos:end]
            opening = _skip_c_ws_comments(source, end)
            if opening < brace_pos and source[opening] == "(":
                parsed = _call_arguments(source, opening)
                if parsed is None or parsed[1] > brace_pos:
                    return None
                if name not in {"__attribute__", "__declspec", "noexcept"}:
                    candidate = name
                pos = parsed[1]
                continue
            pos = end
            continue
        pos += 1
    return candidate


def _parse_c_string_component(source: str, pos: int) -> tuple[bytes | None, int]:
    if source.startswith('u8"', pos):
        pos += 2
    if pos >= len(source) or source[pos] != '"':
        return None, pos
    pos += 1
    value = bytearray()
    while pos < len(source):
        ch = source[pos]
        pos += 1
        if ch == '"':
            return bytes(value), pos
        if ch != "\\":
            if ch in "\r\n":
                return None, pos
            try:
                value.extend(ch.encode("utf-8"))
            except UnicodeEncodeError:
                return None, pos
            continue
        if pos >= len(source):
            return None, pos
        escape = source[pos]
        pos += 1
        if escape in _C_SIMPLE_STRING_ESCAPES:
            value.extend(_C_SIMPLE_STRING_ESCAPES[escape])
        elif escape == "\n":
            continue
        elif escape == "\r" and pos < len(source) and source[pos] == "\n":
            pos += 1
        elif escape in "01234567":
            digits = escape
            while pos < len(source) and len(digits) < 3 and source[pos] in "01234567":
                digits += source[pos]
                pos += 1
            number = int(digits, 8)
            if number > 255:
                return None, pos
            value.append(number)
        elif escape == "x":
            start = pos
            while pos < len(source) and source[pos] in "0123456789abcdefABCDEF":
                pos += 1
            if pos == start:
                return None, pos
            number = int(source[start:pos], 16)
            if number > 255:
                return None, pos
            value.append(number)
        elif escape in {"u", "U"}:
            width = 4 if escape == "u" else 8
            digits = source[pos : pos + width]
            pos += width
            if len(digits) != width or any(
                ch not in "0123456789abcdefABCDEF" for ch in digits
            ):
                return None, pos
            try:
                value.extend(chr(int(digits, 16)).encode("utf-8"))
            except (ValueError, UnicodeEncodeError):
                return None, pos
        else:
            return None, pos
    return None, pos


def _literal_module_argument(source: str, span: tuple[int, int]) -> str | None:
    pos, end = span
    parts: list[bytes] = []
    while (pos := _skip_c_ws_comments(source, pos)) < end:
        part, next_pos = _parse_c_string_component(source, pos)
        if part is None:
            return None
        parts.append(part)
        pos = next_pos
    if not parts or pos != end:
        return None
    # These APIs receive NUL-terminated UTF-8 char*, not Python string objects.
    try:
        name = b"".join(parts).split(b"\0", 1)[0].decode("utf-8")
        return canonical_python_module_name(name, field="C eager-import literal")
    except (UnicodeDecodeError, ValueError):
        return None


def _absolute_level_argument(source: str, span: tuple[int, int]) -> bool:
    pos, end = span
    tokens: list[str] = []
    while (pos := _skip_c_ws_comments(source, pos)) < end:
        start = pos
        while (
            pos < end
            and not source[pos].isspace()
            and not source.startswith(("//", "/*"), pos)
        ):
            pos += 1
        tokens.append(source[start:pos])
    # Do not concatenate distinct tokens (0 /*...*/ 1) into a constant.
    return len(tokens) == 1 and _ZERO_C_INTEGER.fullmatch(tokens[0]) is not None


def source_extension_runtime_python_imports(source_text: str) -> tuple[str, ...]:
    """Return canonical known eager roots, preserving runtime dynamic policy."""
    names: set[str] = set()
    pos = 0
    declaration_start = 0
    paren_depth = 0
    bracket_depth = 0
    eager_context_stack: list[bool] = []
    while pos < len(source_text):
        pos = _skip_c_ws_comments(source_text, pos)
        if pos >= len(source_text):
            break
        ch = source_text[pos]
        if ch in {'"', "'"}:
            pos = _skip_c_string_or_char(source_text, pos)
            continue
        if ch == "{":
            function_name = _source_extension_function_name_before_brace(
                source_text, declaration_start, pos
            )
            eager_context_stack.append(
                bool(eager_context_stack and eager_context_stack[-1])
                or _source_extension_eager_import_function_name(function_name)
            )
            declaration_start = pos + 1
        elif ch == "}":
            if eager_context_stack:
                eager_context_stack.pop()
            declaration_start = pos + 1
        elif ch == "(":
            paren_depth += 1
        elif ch == ")":
            paren_depth = max(0, paren_depth - 1)
        elif ch == "[":
            bracket_depth += 1
        elif ch == "]":
            bracket_depth = max(0, bracket_depth - 1)
        elif ch == ";" and paren_depth == 0 and bracket_depth == 0:
            declaration_start = pos + 1
        elif ch == "_" or ch.isalpha():
            end = _identifier_end(source_text, pos)
            contract = _source_extension_runtime_import_callee(source_text[pos:end])
            pos = end
            if contract is None or contract.name_kind != "c_string":
                continue
            if contract.context == "init" and not (
                eager_context_stack and eager_context_stack[-1]
            ):
                continue
            opening = _skip_c_ws_comments(source_text, pos)
            if opening >= len(source_text) or source_text[opening] != "(":
                continue
            parsed = _call_arguments(source_text, opening)
            if parsed is None:
                continue
            arguments, _ = parsed
            if contract.level_argument is not None and (
                len(arguments) <= contract.level_argument
                or not _absolute_level_argument(
                    source_text, arguments[contract.level_argument]
                )
            ):
                continue
            module_name = _literal_module_argument(source_text, arguments[0])
            if module_name is not None:
                names.add(module_name)
            continue
        pos += 1
    return tuple(sorted(names))
