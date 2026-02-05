#!/usr/bin/env python3
from __future__ import annotations

import io
import re
import tokenize
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = ROOT / "src" / "molt" / "stdlib"

TEXT_TOKENS = (
    "load_intrinsic",
    "require_intrinsic",
    "_intrinsic_load",
    "_require_intrinsic",
)

INTRINSICS_IMPORT_RE = re.compile(
    r"^\s*from\s+(\.+)?_intrinsics\s+import\s+|"
    r"^\s*from\s+molt\.stdlib\._intrinsics\s+import\s+|"
    r"^\s*import\s+_intrinsics(\s|$)|"
    r"^\s*import\s+molt\.stdlib\._intrinsics(\s|$)",
    re.MULTILINE,
)

FORBIDDEN_MOLT_INTRINSICS_RE = re.compile(
    r"^\s*import\s+molt\.intrinsics\b|"
    r"^\s*from\s+molt\s+import\s+intrinsics\b|"
    r"^\s*from\s+molt\.intrinsics\s+import\b",
    re.MULTILINE,
)


def _code_text(text: str) -> str:
    try:
        tokens = tokenize.generate_tokens(io.StringIO(text).readline)
    except Exception:
        return text
    parts: list[str] = []
    try:
        for tok_type, tok_str, *_ in tokens:
            if tok_type in {tokenize.COMMENT, tokenize.STRING}:
                continue
            parts.append(tok_str)
    except Exception:
        return text
    return " ".join(parts)


def _scan_file(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    code_text = _code_text(text)
    errors: list[str] = []
    is_registry_file = path.name == "_intrinsics.py"

    if not is_registry_file and "_molt_intrinsics" in code_text:
        errors.append(
            "Direct access to _molt_intrinsics is forbidden; use stdlib/_intrinsics.py."
        )

    has_intrinsics_import = bool(INTRINSICS_IMPORT_RE.search(text))
    if FORBIDDEN_MOLT_INTRINSICS_RE.search(text):
        errors.append(
            "Importing molt.intrinsics in stdlib is forbidden; use stdlib/_intrinsics.py."
        )

    if not is_registry_file:
        if (
            any(token in code_text for token in TEXT_TOKENS)
            and not has_intrinsics_import
        ):
            errors.append("Intrinsic loader usage requires importing from _intrinsics.")

    return errors


def main() -> int:
    if not STDLIB_ROOT.is_dir():
        print(f"stdlib root missing: {STDLIB_ROOT}")
        return 1

    failures: list[tuple[Path, list[str]]] = []
    for path in sorted(STDLIB_ROOT.rglob("*.py")):
        if path.name.startswith("."):
            continue
        errors = _scan_file(path)
        if errors:
            failures.append((path, errors))

    if failures:
        print("stdlib intrinsics lint failed:")
        for path, errors in failures:
            rel = path.relative_to(ROOT)
            print(f"- {rel}")
            for msg in errors:
                print(f"  {msg}")
        return 1

    print("stdlib intrinsics lint: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
