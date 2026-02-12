#!/usr/bin/env python3
from __future__ import annotations

"""
Synchronize stdlib submodule/subpackage coverage stubs with the CPython union.

This script compares `tools/stdlib_module_union.py` submodule baselines to
`src/molt/stdlib` and reports missing entries.

`--write` creates intrinsic-first placeholder stubs for missing submodules and
subpackages. Without `--write`, this script is a dry-run check.
"""

import argparse
import runpy
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = ROOT / "src" / "molt" / "stdlib"
BASELINE = ROOT / "tools" / "stdlib_module_union.py"


def _load_baseline() -> tuple[frozenset[str], frozenset[str]]:
    if not BASELINE.exists():
        raise RuntimeError(f"baseline missing: {BASELINE}")
    namespace = runpy.run_path(str(BASELINE))
    modules = namespace.get("STDLIB_PY_SUBMODULE_UNION")
    packages = namespace.get("STDLIB_PY_SUBPACKAGE_UNION")
    if not isinstance(modules, tuple) or not isinstance(packages, tuple):
        raise RuntimeError("baseline missing required submodule tuple constants")
    return frozenset(modules), frozenset(packages)


def _present_modules() -> set[str]:
    out: set[str] = set()
    for path in STDLIB_ROOT.rglob("*.py"):
        if path.name.startswith("."):
            continue
        rel = path.relative_to(STDLIB_ROOT)
        if path.name == "__init__.py":
            if len(rel.parts) <= 1:
                continue
            name = ".".join(rel.parts[:-1])
        else:
            name = ".".join((*rel.parts[:-1], path.stem))
        if "." in name:
            out.add(name)
    return out


def _stub_text(name: str, *, kind: str) -> str:
    return (
        f'"""Intrinsic-first stdlib {kind} stub for `{name}`."""\n'
        "\n"
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "\n"
        '_require_intrinsic("molt_capabilities_has", globals())\n'
        "\n"
        f"# TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): "
        f"replace `{name}` {kind} stub with full intrinsic-backed lowering.\n"
        "def __getattr__(attr: str):\n"
        "    raise RuntimeError(\n"
        f'        "stdlib {kind} \\"{name}\\" is not fully lowered yet; only an intrinsic-first stub is available."\n'
        "    )\n"
    )


def _ensure_package_chain(name: str) -> None:
    parts = name.split(".")
    for index in range(1, len(parts) + 1):
        package_name = ".".join(parts[:index])
        target = STDLIB_ROOT.joinpath(*parts[:index], "__init__.py")
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            continue
        target.write_text(_stub_text(package_name, kind="package"), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Create intrinsic-first stdlib submodule stubs required by "
            "tools/stdlib_module_union.py when entries are missing."
        )
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="Write missing stubs. Without this flag the command is a dry run.",
    )
    args = parser.parse_args()

    required_modules, required_packages = _load_baseline()
    present = _present_modules()
    missing = sorted(required_modules - present)
    if not missing:
        print("no missing stdlib submodule entries")
        return 0

    print(f"missing stdlib submodule entries: {len(missing)}")
    for name in missing:
        is_package = name in required_packages
        parts = name.split(".")
        if is_package:
            target = STDLIB_ROOT.joinpath(*parts, "__init__.py")
        else:
            target = STDLIB_ROOT.joinpath(*parts[:-1], f"{parts[-1]}.py")
        print(f"- {'package' if is_package else 'module'} {target.relative_to(ROOT)}")
        if not args.write:
            continue
        if len(parts) > 1:
            _ensure_package_chain(".".join(parts[:-1]))
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            continue
        target.write_text(
            _stub_text(name, kind="package" if is_package else "module"),
            encoding="utf-8",
        )

    if not args.write:
        print("dry run only; rerun with --write to create stubs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
