from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

_PY_C_API_TOKEN_RE = re.compile(r"\bPy[A-Za-z_][A-Za-z0-9_]*\b")
_C_BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", flags=re.DOTALL)
_C_LINE_COMMENT_RE = re.compile(r"//.*?$", flags=re.MULTILINE)
_C_STRING_LITERAL_RE = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'')
_NUMPY_FAIL_FAST_SYMBOL_RE = re.compile(
    r"_molt_numpy_unavailable_[A-Za-z0-9_]+\(\s*\"(?P<symbol>Py[A-Za-z0-9_]*)\""
)


def _strip_c_like_comments_and_literals(text: str) -> str:
    without_blocks = _C_BLOCK_COMMENT_RE.sub(" ", text)
    without_lines = _C_LINE_COMMENT_RE.sub(" ", without_blocks)
    return _C_STRING_LITERAL_RE.sub(" ", without_lines)


def _extract_py_c_api_tokens(text: str) -> set[str]:
    sanitized = _strip_c_like_comments_and_literals(text)
    return {match.group(0) for match in _PY_C_API_TOKEN_RE.finditer(sanitized)}


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


def _load_py_c_api_scan_surface(
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
    runtime_tokens.update(_extract_py_c_api_tokens(header_text))
    datetime_header = molt_root / "include" / "datetime.h"
    if datetime_header.exists():
        try:
            datetime_text = datetime_header.read_text()
        except OSError:
            datetime_text = ""
        if datetime_text:
            runtime_tokens.update(_extract_py_c_api_tokens(datetime_text))
    numpy_include_root = molt_root / "include" / "numpy"
    if numpy_include_root.exists():
        for numpy_header in sorted(numpy_include_root.rglob("*.h")):
            try:
                numpy_text = numpy_header.read_text()
            except OSError:
                continue
            numpy_tokens.update(_extract_py_c_api_tokens(numpy_text))
            fail_fast_tokens.update(_extract_numpy_fail_fast_symbols(numpy_text))
    source_compile_only = numpy_tokens - runtime_tokens - fail_fast_tokens
    surface = _ExtensionScanSurface(
        runtime_backed=frozenset(runtime_tokens - fail_fast_tokens),
        source_compile_only=frozenset(source_compile_only),
        fail_fast=frozenset(fail_fast_tokens),
        header_path=header_path,
    )
    return surface, header_path, None
