"""Fail fast when Rust FFI declaration blocks omit `unsafe`."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ROOTS = (ROOT / "runtime", ROOT / "src", ROOT / "tools", ROOT / "tests")
SKIPPED_DIR_NAMES = {
    ".git",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    "__pycache__",
    "node_modules",
    "target",
}

_EXTERN_BLOCK = re.compile(r'\bextern\s+"[^"]+"\s*\{')
_UNSAFE_EXTERN_BLOCK = re.compile(r'\bunsafe\s+extern\s+"[^"]+"\s*\{')


@dataclass(frozen=True, slots=True)
class Finding:
    path: Path
    line: int
    text: str


def _strip_rust_strings_and_comments(source: str) -> str:
    out: list[str] = []
    i = 0
    n = len(source)
    while i < n:
        ch = source[i]
        nxt = source[i + 1] if i + 1 < n else ""

        if ch == "/" and nxt == "/":
            out.extend("  ")
            i += 2
            while i < n and source[i] != "\n":
                out.append(" ")
                i += 1
            continue

        if ch == "/" and nxt == "*":
            out.extend("  ")
            i += 2
            depth = 1
            while i < n and depth:
                if source[i] == "/" and i + 1 < n and source[i + 1] == "*":
                    out.extend("  ")
                    i += 2
                    depth += 1
                elif source[i] == "*" and i + 1 < n and source[i + 1] == "/":
                    out.extend("  ")
                    i += 2
                    depth -= 1
                else:
                    out.append("\n" if source[i] == "\n" else " ")
                    i += 1
            continue

        raw_end = _raw_string_extent(source, i)
        if raw_end is not None:
            while i < raw_end:
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
            continue

        if ch == '"':
            preserve_abi_literal = re.search(
                r"\bextern\s*$", source[max(0, i - 24) : i]
            )
            out.append(ch if preserve_abi_literal else " ")
            i += 1
            escaped = False
            while i < n:
                current = source[i]
                if preserve_abi_literal:
                    out.append(current)
                else:
                    out.append("\n" if current == "\n" else " ")
                i += 1
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == '"':
                    break
            continue

        char_end = _char_literal_extent(source, i)
        if char_end is not None:
            while i < char_end:
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
            continue

        out.append(ch)
        i += 1
    return "".join(out)


def _raw_string_extent(source: str, start: int) -> int | None:
    if source[start : start + 1] != "r":
        return None
    i = start + 1
    hashes = 0
    n = len(source)
    while i < n and source[i] == "#":
        hashes += 1
        i += 1
    if i >= n or source[i] != '"':
        return None
    terminator = '"' + ("#" * hashes)
    end = source.find(terminator, i + 1)
    if end == -1:
        return n
    return end + len(terminator)


def _char_literal_extent(source: str, start: int) -> int | None:
    if source[start : start + 1] != "'":
        return None
    i = start + 1
    n = len(source)
    escaped = False
    while i < n and source[i] != "\n":
        current = source[i]
        i += 1
        if escaped:
            escaped = False
        elif current == "\\":
            escaped = True
        elif current == "'":
            return i
    return None


def _line_for_offset(source: str, offset: int) -> int:
    return source.count("\n", 0, offset) + 1


def find_missing_unsafe_extern_blocks(path: Path) -> list[Finding]:
    source = path.read_text(encoding="utf-8")
    stripped = _strip_rust_strings_and_comments(source)
    findings: list[Finding] = []
    for match in _EXTERN_BLOCK.finditer(stripped):
        prefix_start = max(0, match.start() - 16)
        prefix = stripped[prefix_start : match.end()]
        if _UNSAFE_EXTERN_BLOCK.search(prefix):
            continue
        line = _line_for_offset(stripped, match.start())
        findings.append(
            Finding(
                path=path,
                line=line,
                text=source.splitlines()[line - 1].strip(),
            )
        )
    return findings


def _rust_files(paths: list[Path]) -> list[Path]:
    files: list[Path] = []
    for path in paths:
        if path.is_file() and path.suffix == ".rs":
            files.append(path)
        elif path.is_dir():
            for dirpath, dirnames, filenames in os.walk(path):
                dirnames[:] = [
                    name for name in dirnames if name not in SKIPPED_DIR_NAMES
                ]
                base = Path(dirpath)
                files.extend(
                    base / filename
                    for filename in filenames
                    if filename.endswith(".rs")
                )
    return sorted(files)


def _default_rust_files() -> list[Path]:
    if (ROOT / ".git").exists():
        try:
            result = subprocess.run(
                ["git", "-C", str(ROOT), "grep", "-l", 'extern "', "--", "*.rs"],
                capture_output=True,
                text=True,
            )
        except OSError:
            pass
        else:
            if result.returncode == 1:
                return []
            if result.returncode != 0:
                return _rust_files(list(DEFAULT_ROOTS))
            roots = tuple(path.resolve() for path in DEFAULT_ROOTS)
            files = []
            for line in result.stdout.splitlines():
                path = (ROOT / line).resolve()
                if not any(path.is_relative_to(root) for root in roots):
                    continue
                if any(part in SKIPPED_DIR_NAMES for part in path.parts):
                    continue
                files.append(path)
            return sorted(files)
    return _rust_files(list(DEFAULT_ROOTS))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("paths", nargs="*", type=Path)
    args = parser.parse_args(argv)

    rust_files = _rust_files(args.paths) if args.paths else _default_rust_files()
    findings: list[Finding] = []
    for path in rust_files:
        findings.extend(find_missing_unsafe_extern_blocks(path))

    if findings:
        for finding in findings:
            rel = (
                finding.path.relative_to(ROOT)
                if finding.path.is_relative_to(ROOT)
                else finding.path
            )
            print(
                f"{rel}:{finding.line}: Rust FFI declaration blocks must use "
                f"unsafe extern: {finding.text}",
                file=sys.stderr,
            )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
