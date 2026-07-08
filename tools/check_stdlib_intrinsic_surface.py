#!/usr/bin/env python3
"""Fail-closed surface-preservation gate for stdlib intrinsic requirements.

A crate decomposition (leaf-owned rename, runtime policy-crate extraction) moves
the Rust `#[no_mangle] extern "C"` intrinsic and its resolver arm between crates.
If such a move silently DROPS an intrinsic registration that a shipped
`src/molt/stdlib/**.py` module still `_require_intrinsic("X")`s at import, the
public surface breaks: a real build fails CLOSED at module import with
`intrinsic unavailable: X`. The `test_public_intrinsic_surface_batch_*` probes do
NOT catch this class -- they inject FAKE intrinsics, so import always succeeds in
the probe regardless of whether the real runtime still registers the symbol.

This gate is the missing authority: it asserts that every intrinsic NAME required
by a shipped stdlib module is registered in the runtime intrinsic manifest
(`runtime/molt-runtime/src/intrinsics/generated.rs`, the generated authority).
Static, cargo-free, fail-closed. Wire as a tier-1 `--check` alongside
`check_table_drift.py` in `tools/ci_gate.py`.

Exit 0 = every stdlib-required intrinsic is registered.
Exit 1 = at least one required-but-unregistered intrinsic (broken surface).
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"
GENERATED_RS = (
    REPO_ROOT / "runtime" / "molt-runtime" / "src" / "intrinsics" / "generated.rs"
)

_REQUIRE_RE = re.compile(r'_require(?:_callable)?_intrinsic\(\s*"([^"]+)"')
_REGISTERED_RE = re.compile(r'name:\s*"([^"]+)"')


def registered_intrinsics() -> set[str]:
    text = GENERATED_RS.read_text(encoding="utf-8")
    return set(_REGISTERED_RE.findall(text))


def required_intrinsics() -> dict[str, list[str]]:
    required: dict[str, list[str]] = {}
    for py in sorted(STDLIB_ROOT.rglob("*.py")):
        text = py.read_text(encoding="utf-8", errors="replace")
        for match in _REQUIRE_RE.finditer(text):
            required.setdefault(match.group(1), []).append(
                str(py.relative_to(REPO_ROOT)).replace("\\", "/")
            )
    return required


def audit() -> tuple[dict[str, list[str]], int, int]:
    registered = registered_intrinsics()
    required = required_intrinsics()
    missing = {
        name: sorted(set(files))
        for name, files in required.items()
        if name not in registered
    }
    return missing, len(required), len(registered)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail (exit 1) if any stdlib-required intrinsic is unregistered.",
    )
    args = parser.parse_args(argv)

    missing, n_required, n_registered = audit()
    print(
        f"stdlib intrinsic surface: {n_required} distinct required, "
        f"{n_registered} registered in generated.rs"
    )
    if not missing:
        print("OK: every stdlib _require_intrinsic resolves to a registered intrinsic.")
        return 0

    print(
        f"BROKEN SURFACE: {len(missing)} required-but-unregistered intrinsic(s) "
        "-> real build fails CLOSED at module import:"
    )
    for name in sorted(missing):
        files = missing[name]
        shown = ", ".join(files[:4]) + (" ..." if len(files) > 4 else "")
        print(f"  {name}  <- {shown}")
    return 1 if args.check else 0


if __name__ == "__main__":
    raise SystemExit(main())
