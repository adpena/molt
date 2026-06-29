from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from molt.cli.c_api_symbols import C_API_TOKEN as _C_API_TOKEN
from molt.cli.c_api_symbols import C_API_TOKEN_RE as _C_API_TOKEN_RE
_C_BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", flags=re.DOTALL)
_C_LINE_COMMENT_RE = re.compile(r"//.*?$", flags=re.MULTILINE)
_CYTHON_PREPROCESSOR_DIRECTIVE_RE = re.compile(
    r"^\s*#\s*(?:include|define|if|ifdef|ifndef|elif|elifdef|elifndef|else|endif|pragma|error|warning)\b"
)
_PY_TRIPLE_STRING_RE = re.compile(
    r'"""(?:\\.|[\s\S])*?"""|\'\'\'(?:\\.|[\s\S])*?\'\'\''
)
_C_STRING_LITERAL_RE = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'')
_C_PREPROCESSOR_CONDITION_RE = re.compile(
    r"(?m)^\s*#\s*(?:if|ifdef|ifndef|elif|elifdef|elifndef)\b.*$"
)
_NUMPY_FAIL_FAST_SYMBOL_RE = re.compile(
    rf"_molt_numpy_unavailable_[A-Za-z0-9_]+\(\s*\"(?P<symbol>{_C_API_TOKEN})\""
)
_C_DEFINE_SYMBOL_RE = re.compile(rf"^\s*#\s*define\s+(?P<symbol>{_C_API_TOKEN})\b")
_C_TAG_SYMBOL_RE = re.compile(
    rf"\b(?:struct|enum|union)\s+(?P<symbol>{_C_API_TOKEN})\b"
)
_C_TOKEN_BEFORE_PAREN_RE = re.compile(rf"\b(?P<symbol>{_C_API_TOKEN})\s*\(")
_CYTHON_INLINE_SYMBOL_RE = re.compile(
    rf"^\s*cdef\s+inline\b.*\b(?P<symbol>{_C_API_TOKEN})\s*\("
)
_C_SINGLE_LINE_FUNCTION_SYMBOL_RE = re.compile(
    rf"(?m)^\s*(?:static\s+inline|static|inline|NPY_NO_EXPORT|PyMODINIT_FUNC)"
    rf"[^\n;{{}}]*\b(?P<symbol>{_C_API_TOKEN})\s*\([^;\n{{}}]*\)\s*\{{"
)
_C_SPLIT_LINE_FUNCTION_SYMBOL_RE = re.compile(
    rf"(?m)^\s*(?:static\s+inline|static|inline|NPY_NO_EXPORT|PyMODINIT_FUNC)"
    rf"[^\n;{{}}]*\n\s*(?P<symbol>{_C_API_TOKEN})\s*"
    rf"\([^;\n{{}}]*\)\s*\n?\s*\{{"
)
_C_PLAIN_SPLIT_LINE_FUNCTION_SYMBOL_RE = re.compile(
    rf"(?m)^\s*[A-Za-z_][A-Za-z0-9_\s\*]*\*?\s*\n"
    rf"\s*(?P<symbol>{_C_API_TOKEN})\s*\([^;\n{{}}]*\)\s*\n?\s*\{{"
)
_C_CONTROL_PREFIXES = (
    "if ",
    "if(",
    "for ",
    "for(",
    "while ",
    "while(",
    "switch ",
    "switch(",
    "return ",
    "else",
    "do ",
)


def _add_py_parameter_names(header: str, symbols: set[str]) -> None:
    if "(" not in header or ")" not in header:
        return
    params = header.rsplit("(", 1)[1].rsplit(")", 1)[0]
    for param in params.split(","):
        tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(param)]
        if len(tokens) >= 2 and tokens[-1] != tokens[0]:
            symbols.add(tokens[-1])


def _strip_cython_hash_comments(text: str) -> str:
    lines: list[str] = []
    for line in text.splitlines(keepends=True):
        body = line.rstrip("\r\n")
        newline = line[len(body) :]
        stripped = body.lstrip()
        if stripped.startswith("#"):
            lines.append(line if _CYTHON_PREPROCESSOR_DIRECTIVE_RE.match(body) else newline)
            continue
        comment_start = body.find("#")
        if comment_start >= 0:
            lines.append(body[:comment_start].rstrip() + newline)
        else:
            lines.append(line)
    return "".join(lines)


def _strip_preprocessor_macro_definitions(text: str) -> str:
    lines: list[str] = []
    in_macro = False
    for line in text.splitlines(keepends=True):
        body = line.rstrip("\r\n")
        newline = line[len(body) :]
        if in_macro:
            in_macro = body.rstrip().endswith("\\")
            lines.append(newline)
            continue
        if body.lstrip().startswith("#"):
            in_macro = body.rstrip().endswith("\\")
            lines.append(newline)
            continue
        lines.append(line)
    return "".join(lines)


def _strip_c_like_comments_and_literals(text: str) -> str:
    without_blocks = _C_BLOCK_COMMENT_RE.sub(" ", text)
    without_lines = _C_LINE_COMMENT_RE.sub(" ", without_blocks)
    without_triple_strings = _PY_TRIPLE_STRING_RE.sub(" ", without_lines)
    without_string_literals = _C_STRING_LITERAL_RE.sub(" ", without_triple_strings)
    return _strip_cython_hash_comments(without_string_literals)


def _extract_c_api_tokens(
    text: str, *, strip_py_condition_blocks: bool = True
) -> set[str]:
    del strip_py_condition_blocks
    sanitized = _strip_c_like_comments_and_literals(text)
    sanitized = _C_PREPROCESSOR_CONDITION_RE.sub(" ", sanitized)
    return {
        match.group("symbol")
        for match in _C_API_TOKEN_RE.finditer(sanitized)
        if not match.group("symbol").endswith("_")
    }


def _extract_file_local_c_api_symbols(text: str) -> set[str]:
    """Return C/API identifiers that are local to the source file.

    These names are not external C/API requirements, but they also must not be
    promoted to project-wide definitions. Filtering them per file prevents a
    local variable or parameter named like an ABI symbol from hiding a real
    missing symbol in another translation unit.
    """
    local: set[str] = set()
    sanitized = _strip_preprocessor_macro_definitions(
        _strip_c_like_comments_and_literals(text)
    )
    signature_parts: list[str] = []
    brace_depth = 0

    for raw_line in sanitized.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        lowered = line.lower()
        is_control = lowered.startswith(_C_CONTROL_PREFIXES)

        if brace_depth == 0 and not is_control:
            if line.lstrip().startswith("static ") and "=" in line:
                lhs = line.split("=", 1)[0]
                if "(" not in lhs:
                    tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(lhs)]
                    if tokens:
                        local.add(tokens[-1])
            signature_parts.append(line)
            if "{" in line:
                header = " ".join(signature_parts).split("{", 1)[0]
                _add_py_parameter_names(header, local)
                if header.lstrip().startswith("static"):
                    matches = list(_C_TOKEN_BEFORE_PAREN_RE.finditer(header))
                    if matches:
                        local.add(matches[-1].group("symbol"))
                signature_parts = []
            elif ";" in line or "}" in line:
                signature_parts = []
        elif brace_depth > 0 and not is_control:
            if "=" in line:
                lhs = line.split("=", 1)[0]
                if "(" not in lhs:
                    tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(lhs)]
                    if tokens:
                        local.add(tokens[-1])
            elif line.endswith(";") and "(" not in line:
                statement = line[:-1]
                tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(statement)]
                if tokens:
                    local.add(tokens[-1])

        brace_depth += line.count("{") - line.count("}")
        if brace_depth < 0:
            brace_depth = 0

    return local


def _extract_project_defined_c_api_symbols(text: str) -> set[str]:
    """Return C/API symbols defined by the scanned package itself.

    The extension scanner gates unresolved external C/API requirements. Large
    packages such as NumPy define many Py-prefixed, Npy-prefixed, npy-prefixed,
    and NPY-prefixed C functions, macros, type objects, and typedefs in their
    own source tree; counting those as missing Molt C/API symbols makes the scan
    both noisy and wrong. This pass is intentionally line-oriented and
    conservative so it stays fast on generated megabyte-scale C files.
    """
    defined: set[str] = set()
    for raw_line in text.splitlines():
        line = raw_line.strip()
        cython_inline_match = _CYTHON_INLINE_SYMBOL_RE.match(line)
        if cython_inline_match is not None:
            defined.add(cython_inline_match.group("symbol"))
        elif line.startswith("cdef inline "):
            matches = list(_C_TOKEN_BEFORE_PAREN_RE.finditer(line))
            if matches:
                defined.add(matches[-1].group("symbol"))

    sanitized_with_macros = _strip_c_like_comments_and_literals(text)
    for macro_line in sanitized_with_macros.splitlines():
        define_match = _C_DEFINE_SYMBOL_RE.match(macro_line)
        if define_match is not None:
            defined.add(define_match.group("symbol"))
    sanitized = _strip_preprocessor_macro_definitions(sanitized_with_macros)
    for function_re in (
        _C_SINGLE_LINE_FUNCTION_SYMBOL_RE,
        _C_SPLIT_LINE_FUNCTION_SYMBOL_RE,
        _C_PLAIN_SPLIT_LINE_FUNCTION_SYMBOL_RE,
    ):
        for match in function_re.finditer(sanitized):
            header = match.group(0).lstrip()
            if not header.startswith("static"):
                defined.add(match.group("symbol"))
    typedef_parts: list[str] | None = None
    signature_parts: list[str] = []
    brace_depth = 0

    for raw_line in sanitized.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        lowered = line.lower()

        for tag_match in _C_TAG_SYMBOL_RE.finditer(line):
            defined.add(tag_match.group("symbol"))

        if typedef_parts is not None:
            typedef_parts.append(line)
            if ";" in line:
                statement = " ".join(typedef_parts).split(";", 1)[0]
                tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(statement)]
                if tokens:
                    defined.add(tokens[-1])
                typedef_parts = None
        elif line.startswith("typedef "):
            typedef_parts = [line]
            if ";" in line:
                statement = line.split(";", 1)[0]
                tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(statement)]
                if tokens:
                    defined.add(tokens[-1])
                typedef_parts = None

        if brace_depth == 0:
            is_control = lowered.startswith(_C_CONTROL_PREFIXES)
            if (
                not is_control
                and "=" in line
                and not lowered.startswith(("extern ", "static "))
            ):
                lhs = line.split("=", 1)[0]
                tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(lhs)]
                if tokens:
                    defined.add(tokens[-1])
            elif (
                not is_control
                and line.endswith(";")
                and "(" not in line
                and not lowered.startswith(("extern ", "static ", "typedef ", "#"))
            ):
                statement = line[:-1]
                tokens = [match.group("symbol") for match in _C_API_TOKEN_RE.finditer(statement)]
                if tokens:
                    defined.add(tokens[-1])

            if not is_control:
                signature_parts.append(line)
                if "{" in line:
                    header = " ".join(signature_parts).split("{", 1)[0]
                    if not header.lstrip().startswith("static"):
                        matches = list(_C_TOKEN_BEFORE_PAREN_RE.finditer(header))
                        if matches:
                            defined.add(matches[-1].group("symbol"))
                    signature_parts = []
                elif ";" in line or "}" in line:
                    signature_parts = []

        brace_depth += line.count("{") - line.count("}")
        if brace_depth < 0:
            brace_depth = 0

    return defined


def _load_c_api_scan_surface(
    molt_root: Path,
) -> tuple[_ExtensionScanSurface | None, Path, str | None]:
    header_path = molt_root / "include" / "molt" / "Python.h"
    runtime_tokens: set[str] = set()
    numpy_tokens: set[str] = set()
    fail_fast_tokens: set[str] = set()
    try:
        header_text = header_path.read_text()
    except OSError as exc:
        return None, header_path, str(exc)
    runtime_tokens.update(_extract_c_api_tokens(header_text, strip_py_condition_blocks=False))
    datetime_header = molt_root / "include" / "datetime.h"
    if datetime_header.exists():
        try:
            datetime_text = datetime_header.read_text()
        except OSError:
            datetime_text = ""
        if datetime_text:
            runtime_tokens.update(
                _extract_c_api_tokens(datetime_text, strip_py_condition_blocks=False)
            )
    numpy_include_root = molt_root / "include" / "numpy"
    if numpy_include_root.exists():
        for numpy_header in sorted(numpy_include_root.rglob("*.h")):
            try:
                numpy_text = numpy_header.read_text()
            except OSError:
                continue
            numpy_tokens.update(
                _extract_c_api_tokens(numpy_text, strip_py_condition_blocks=False)
            )
            fail_fast_tokens.update(_extract_numpy_fail_fast_symbols(numpy_text))
    source_compile_only = numpy_tokens - runtime_tokens - fail_fast_tokens
    surface = _ExtensionScanSurface(
        runtime_backed=frozenset(runtime_tokens - fail_fast_tokens),
        source_compile_only=frozenset(source_compile_only),
        fail_fast=frozenset(fail_fast_tokens),
        header_path=header_path,
    )
    return surface, header_path, None


@dataclass(frozen=True)
class _ExtensionScanSurface:
    runtime_backed: frozenset[str]
    source_compile_only: frozenset[str]
    fail_fast: frozenset[str]
    header_path: Path

    @property
    def accepted(self) -> frozenset[str]:
        return self.runtime_backed | self.source_compile_only

    @property
    def known(self) -> frozenset[str]:
        return self.accepted | self.fail_fast

    def status_for(self, symbol: str) -> str:
        if symbol in self.fail_fast:
            return "fail_fast"
        if symbol in self.runtime_backed:
            return "runtime_backed"
        if symbol in self.source_compile_only:
            return "source_compile_only"
        return "missing"


def _extract_numpy_fail_fast_symbols(text: str) -> set[str]:
    return {
        match.group("symbol") for match in _NUMPY_FAIL_FAST_SYMBOL_RE.finditer(text)
    }
