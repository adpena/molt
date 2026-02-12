#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[1]
DIFF_ROOT = ROOT / "tests" / "differential"
COVERAGE_INDEX = DIFF_ROOT / "COVERAGE_INDEX.yaml"
ALLOWED_LANES = {"basic", "stdlib", "moltlib"}

# Core Python 3.12+ PEP coverage tracked in COVERAGE_INDEX.yaml.
REQUIRED_CORE_PEPS = {
    "448",
    "526",
    "563",
    "570",
    "572",
    "584",
    "585-604",
    "618",
    "634",
    "649",
    "657",
    "695",
    "701",
}

# Tests that intentionally live in basic while still being listed under stdlib
# coverage entries because they assert builtin/core behavior boundaries.
STDLIB_SECTION_BASIC_ALLOWLIST = {
    "tests/differential/basic/builtins_basic.py",
    "tests/differential/basic/builtins_api_surface_312_plus.py",
    "tests/differential/basic/bytes_translate_maketrans.py",
    "tests/differential/basic/context_return_unwind_scope.py",
    "tests/differential/basic/oserror_errno.py",
    "tests/differential/basic/unicode_surrogate_handling.py",
}

STDLIB_SECTION_BASIC_ALLOW_PREFIXES = ("tests/differential/basic/builtins_",)

STDLIB_PREFIXES = {
    "abc",
    "argparse",
    "ast",
    "asyncio",
    "bisect",
    "cmd",
    "code",
    "codecs",
    "codeop",
    "collections",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "csv",
    "ctypes",
    "datetime",
    "decimal",
    "dis",
    "doctest",
    "email",
    "encodings",
    "enum",
    "errno",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "http",
    "httpclient",
    "imaplib",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "linecache",
    "locale",
    "logging",
    "mailbox",
    "marshal",
    "math",
    "multiprocessing",
    "netrc",
    "opcode",
    "operator",
    "os",
    "pathlib",
    "pdb",
    "pickle",
    "pkgutil",
    "plistlib",
    "poplib",
    "posix",
    "pprint",
    "py",
    "queue",
    "random",
    "re",
    "runpy",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "smtplib",
    "socket",
    "sre",
    "ssl",
    "stat",
    "statistics",
    "string",
    "struct",
    "subprocess",
    "symtable",
    "sys",
    "tabnanny",
    "tempfile",
    "test",
    "textwrap",
    "threading",
    "time",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "urllib",
    "uuid",
    "warnings",
    "weakref",
    "wsgi",
    "wsgiref",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zoneinfo",
    "stdlib",
    "windows",
    "zlib",
}


def _collect_lane_files() -> tuple[list[str], list[str]]:
    wrong_lane: list[str] = []
    basic_stdlib_prefix: list[str] = []
    for path in sorted(DIFF_ROOT.rglob("*.py")):
        rel = path.relative_to(ROOT).as_posix()
        lane = path.relative_to(DIFF_ROOT).parts[0]
        if lane not in ALLOWED_LANES:
            wrong_lane.append(rel)
        if lane == "basic" and path.parent == DIFF_ROOT / "basic":
            prefix = path.stem.split("_", 1)[0]
            if prefix in STDLIB_PREFIXES:
                basic_stdlib_prefix.append(rel)
    return wrong_lane, basic_stdlib_prefix


def _parse_coverage_index() -> tuple[set[str], set[str], set[str]]:
    text = COVERAGE_INDEX.read_text(encoding="utf-8").splitlines()
    all_paths: set[str] = set()
    stdlib_paths: set[str] = set()
    peps: set[str] = set()
    section: str | None = None
    for raw in text:
        if raw.startswith("core:"):
            section = "core"
            continue
        if raw.startswith("stdlib:"):
            section = "stdlib"
            continue
        pep_match = re.match(r'\s+"([^"]+)":\s*$', raw)
        if section == "core" and pep_match:
            candidate = pep_match.group(1)
            if candidate and candidate[0].isdigit():
                peps.add(candidate)
        stripped = raw.lstrip()
        if not stripped.startswith("- "):
            continue
        maybe_path = stripped[2:].strip()
        if not maybe_path.endswith(".py"):
            continue
        all_paths.add(maybe_path)
        if section == "stdlib":
            stdlib_paths.add(maybe_path)
    return all_paths, stdlib_paths, peps


def main() -> int:
    errors: list[str] = []

    wrong_lane, basic_stdlib_prefix = _collect_lane_files()
    if wrong_lane:
        errors.append(
            "unexpected differential lane directories:\n- " + "\n- ".join(wrong_lane)
        )
    if basic_stdlib_prefix:
        errors.append(
            "basic lane has stdlib-prefixed tests (move to stdlib lane):\n- "
            + "\n- ".join(basic_stdlib_prefix)
        )

    all_paths, stdlib_paths, peps = _parse_coverage_index()
    missing_paths = sorted(path for path in all_paths if not (ROOT / path).exists())
    if missing_paths:
        errors.append(
            "COVERAGE_INDEX has missing test paths:\n- " + "\n- ".join(missing_paths)
        )

    retired_refs = sorted(
        path
        for path in all_paths
        if "/planned/" in path or "/core/" in path or "/scoping/" in path
    )
    if retired_refs:
        errors.append(
            "COVERAGE_INDEX still references retired lanes:\n- "
            + "\n- ".join(retired_refs)
        )

    stdlib_lane_violations = sorted(
        path
        for path in stdlib_paths
        if not path.startswith("tests/differential/stdlib/")
        and path not in STDLIB_SECTION_BASIC_ALLOWLIST
        and not any(
            path.startswith(prefix) for prefix in STDLIB_SECTION_BASIC_ALLOW_PREFIXES
        )
    )
    if stdlib_lane_violations:
        errors.append(
            "stdlib coverage section points outside stdlib lane:\n- "
            + "\n- ".join(stdlib_lane_violations)
        )

    missing_peps = sorted(REQUIRED_CORE_PEPS - peps)
    if missing_peps:
        errors.append(
            "core PEP coverage entries missing from COVERAGE_INDEX:\n- "
            + "\n- ".join(missing_peps)
        )

    if errors:
        print("[FAIL] differential suite layout check failed")
        for err in errors:
            print(err)
        return 1

    print("[OK] differential suite layout check passed")
    print(f"[OK] lanes: {', '.join(sorted(ALLOWED_LANES))}")
    print(f"[OK] indexed tests: {len(all_paths)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
