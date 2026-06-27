#!/usr/bin/env python3
"""Keep Tk imports explicit and prevent ambient module authority."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
import sys


REPO_ROOT = Path(__file__).resolve().parents[1]
TK_ROOT = REPO_ROOT / "runtime" / "molt-runtime-tk" / "src" / "tk.rs"
TK_DIR = REPO_ROOT / "runtime" / "molt-runtime-tk" / "src" / "tk"

PRIVATE_ROOT_USE_RE = re.compile(r"^use\s+", re.MULTILINE)
ROOT_PRELUDE_MOD_RE = re.compile(r"^mod\s+prelude\s*;", re.MULTILINE)

FORBIDDEN_IMPORT_PATTERNS: tuple[tuple[re.Pattern[str], str], ...] = (
    (
        re.compile(r"^use\s+super::\*\s*;", re.MULTILINE),
        "Tk child modules must not import the root as wildcard authority",
    ),
    (
        re.compile(r"^use\s+super::super::\*\s*;", re.MULTILINE),
        "Tk nested modules must not import the root as wildcard authority",
    ),
    (
        re.compile(r"^use\s+super::prelude::\*\s*;", re.MULTILINE),
        "Tk child modules must not import a Tk prelude authority",
    ),
    (
        re.compile(r"^use\s+super::super::prelude::\*\s*;", re.MULTILINE),
        "Tk nested modules must not import a Tk prelude authority",
    ),
    (
        re.compile(r"^use\s+crate::tk::prelude::\*\s*;", re.MULTILINE),
        "Tk modules must not import a Tk prelude authority",
    ),
    (
        re.compile(r"^pub\(super\)\s+use\s+self::common::\*\s*;", re.MULTILINE),
        "Tk widget modules must not reexport common as wildcard authority",
    ),
    (
        re.compile(r"^pub\(super\)\s+use\s+super(?:::[A-Za-z0-9_]+)*::\*\s*;", re.MULTILINE),
        "Tk modules must not reexport parent wildcard authority",
    ),
)


@dataclass(frozen=True, slots=True)
class TkImportAuthorityViolation:
    path: str
    line: int
    reason: str


def _rel(path: Path) -> str:
    try:
        return path.relative_to(REPO_ROOT).as_posix()
    except ValueError:
        parts = path.parts
        if "runtime" in parts:
            return Path(*parts[parts.index("runtime") :]).as_posix()
        return path.as_posix()


def _line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def _violation(
    path: Path,
    text: str,
    offset: int,
    reason: str,
) -> TkImportAuthorityViolation:
    return TkImportAuthorityViolation(_rel(path), _line_number(text, offset), reason)


def find_tk_import_authority_violations(
    *,
    tk_root: Path = TK_ROOT,
    tk_dir: Path = TK_DIR,
) -> list[TkImportAuthorityViolation]:
    violations: list[TkImportAuthorityViolation] = []
    root_text = tk_root.read_text(encoding="utf-8")
    prelude_path = tk_dir / "prelude.rs"

    for match in ROOT_PRELUDE_MOD_RE.finditer(root_text):
        violations.append(
            _violation(
                tk_root,
                root_text,
                match.start(),
                "tk.rs must not declare a Tk prelude module",
            )
        )
    if prelude_path.exists():
        violations.append(
            TkImportAuthorityViolation(
                _rel(prelude_path),
                1,
                "Tk must not have a prelude.rs ambient import authority",
            )
        )
    for match in PRIVATE_ROOT_USE_RE.finditer(root_text):
        violations.append(
            _violation(
                tk_root,
                root_text,
                match.start(),
                "tk.rs must not be a private import authority",
            )
        )

    for path in sorted(tk_dir.rglob("*.rs")):
        if path == prelude_path:
            continue
        text = path.read_text(encoding="utf-8")
        for pattern, reason in FORBIDDEN_IMPORT_PATTERNS:
            for match in pattern.finditer(text):
                violations.append(_violation(path, text, match.start(), reason))
    return violations


def _json_payload(violations: list[TkImportAuthorityViolation]) -> dict[str, object]:
    return {
        "ok": not violations,
        "violations": [
            {"path": v.path, "line": v.line, "reason": v.reason} for v in violations
        ],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    violations = find_tk_import_authority_violations()
    if args.json:
        print(json.dumps(_json_payload(violations), indent=2, sort_keys=True))
    if not violations:
        if not args.json:
            print("tk import authority OK")
        return 0

    if not args.json:
        print("tk import authority violation(s) found:", file=sys.stderr)
        for violation in violations:
            print(
                f"- {violation.path}:{violation.line}: {violation.reason}",
                file=sys.stderr,
            )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
