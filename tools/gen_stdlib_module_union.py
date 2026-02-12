#!/usr/bin/env python3
"""
Generate the versioned CPython stdlib top-level union baseline.

This script is the single writer for `tools/stdlib_module_union.py`.
It queries `sys.stdlib_module_names` for each selected Python version using
`uv run --python <version>` and records:
1) per-version module names,
2) per-version package names, and
3) union sets used by stdlib coverage gates.

The output is intentionally deterministic and sorted so diffs are reviewable.
"""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT_PATH = ROOT / "tools" / "stdlib_module_union.py"
DEFAULT_PYTHONS = ("3.12", "3.13", "3.14")

_QUERY = """
import importlib.util
import json
import sys
import sysconfig
from pathlib import Path

modules = sorted(sys.stdlib_module_names)
packages = []
for name in modules:
    try:
        spec = importlib.util.find_spec(name)
    except Exception:
        spec = None
    if spec is not None and spec.submodule_search_locations is not None:
        packages.append(name)

stdlib = Path(sysconfig.get_path("stdlib"))
py_modules = set()
py_packages = set()
for path in stdlib.rglob("*.py"):
    rel = path.relative_to(stdlib)
    if any(part == "__pycache__" for part in rel.parts):
        continue
    if any(part in {"site-packages", "dist-packages"} for part in rel.parts):
        continue
    if rel.parts and rel.parts[0].startswith("config-"):
        continue
    if path.name == "__init__.py":
        if len(rel.parts) == 1:
            continue
        name = ".".join(rel.parts[:-1])
        if name.split(".", 1)[0] not in modules:
            continue
        py_packages.add(name)
        py_modules.add(name)
    else:
        name = ".".join((*rel.parts[:-1], path.stem))
        if name.split(".", 1)[0] not in modules:
            continue
        py_modules.add(name)

print(json.dumps({
    "modules": modules,
    "packages": sorted(set(packages)),
    "py_modules": sorted(py_modules),
    "py_packages": sorted(py_packages),
}))
""".strip()


def _capture_version(
    version: str,
) -> tuple[tuple[str, ...], tuple[str, ...], tuple[str, ...], tuple[str, ...]]:
    cmd = [
        "uv",
        "run",
        "--python",
        version,
        "python3",
        "-c",
        _QUERY,
    ]
    output = subprocess.check_output(cmd, cwd=ROOT, text=True)
    payload = json.loads(output)
    modules = tuple(payload.get("modules", ()))
    packages = tuple(payload.get("packages", ()))
    py_modules = tuple(payload.get("py_modules", ()))
    py_packages = tuple(payload.get("py_packages", ()))
    if not modules:
        raise RuntimeError(f"empty stdlib module list for Python {version}")
    if not py_modules:
        raise RuntimeError(f"empty stdlib python-module list for Python {version}")
    return modules, packages, py_modules, py_packages


def _format_tuple(items: tuple[str, ...], *, indent: int) -> str:
    spaces = " " * indent
    lines = ["("]
    for item in items:
        lines.append(f'{spaces}"{item}",')
    lines.append(" " * (indent - 4) + ")")
    return "\n".join(lines)


def _format_mapping(values: dict[str, tuple[str, ...]], *, indent: int) -> str:
    spaces = " " * indent
    lines = ["{"]
    for version in sorted(values):
        lines.append(
            f'{spaces}"{version}": {_format_tuple(values[version], indent=indent + 4)},'
        )
    lines.append(" " * (indent - 4) + "}")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Generate tools/stdlib_module_union.py from sys.stdlib_module_names "
            "across selected CPython versions."
        )
    )
    parser.add_argument(
        "--python",
        dest="pythons",
        action="append",
        help="CPython version for union baseline (repeatable). Defaults to 3.12/3.13/3.14.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=OUT_PATH,
        help="Output path for generated baseline module.",
    )
    args = parser.parse_args()

    versions = tuple(dict.fromkeys(args.pythons or DEFAULT_PYTHONS))
    by_version_modules: dict[str, tuple[str, ...]] = {}
    by_version_packages: dict[str, tuple[str, ...]] = {}
    by_version_py_modules: dict[str, tuple[str, ...]] = {}
    by_version_py_packages: dict[str, tuple[str, ...]] = {}

    for version in versions:
        modules, packages, py_modules, py_packages = _capture_version(version)
        by_version_modules[version] = modules
        by_version_packages[version] = packages
        by_version_py_modules[version] = py_modules
        by_version_py_packages[version] = py_packages

    union_modules = tuple(
        sorted({name for names in by_version_modules.values() for name in names})
    )
    union_packages = tuple(
        sorted({name for names in by_version_packages.values() for name in names})
    )
    union_py_modules = tuple(
        sorted({name for names in by_version_py_modules.values() for name in names})
    )
    union_py_packages = tuple(
        sorted({name for names in by_version_py_packages.values() for name in names})
    )
    union_py_submodules = tuple(name for name in union_py_modules if "." in name)
    union_py_subpackages = tuple(name for name in union_py_packages if "." in name)

    rendered = "\n".join(
        [
            '"""',
            "Autogenerated CPython stdlib top-level union baseline.",
            "",
            "Update workflow:",
            "1. Install/enable target CPython versions with `uv`.",
            "2. Run `python3 tools/gen_stdlib_module_union.py`.",
            "3. Run `python3 tools/sync_stdlib_top_level_stubs.py --write`.",
            "4. Re-run `python3 tools/check_stdlib_intrinsics.py --update-doc`.",
            "",
            "This file is consumed by tools/check_stdlib_intrinsics.py and is the",
            "hard gate baseline for top-level + submodule stdlib coverage.",
            '"""',
            "",
            f"BASELINE_PYTHON_VERSIONS = {versions!r}",
            "",
            "STDLIB_MODULES_BY_VERSION = "
            + _format_mapping(by_version_modules, indent=4),
            "",
            "STDLIB_PACKAGES_BY_VERSION = "
            + _format_mapping(by_version_packages, indent=4),
            "",
            "STDLIB_PY_MODULES_BY_VERSION = "
            + _format_mapping(by_version_py_modules, indent=4),
            "",
            "STDLIB_PY_PACKAGES_BY_VERSION = "
            + _format_mapping(by_version_py_packages, indent=4),
            "",
            f"STDLIB_MODULE_UNION = {_format_tuple(union_modules, indent=4)}",
            "",
            f"STDLIB_PACKAGE_UNION = {_format_tuple(union_packages, indent=4)}",
            "",
            f"STDLIB_PY_MODULE_UNION = {_format_tuple(union_py_modules, indent=4)}",
            "",
            f"STDLIB_PY_PACKAGE_UNION = {_format_tuple(union_py_packages, indent=4)}",
            "",
            f"STDLIB_PY_SUBMODULE_UNION = {_format_tuple(union_py_submodules, indent=4)}",
            "",
            f"STDLIB_PY_SUBPACKAGE_UNION = {_format_tuple(union_py_subpackages, indent=4)}",
            "",
        ]
    )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(rendered, encoding="utf-8")
    print(
        "generated stdlib union baseline: "
        f"{args.output} "
        f"({len(union_modules)} top-level modules, {len(union_packages)} top-level packages, "
        f"{len(union_py_modules)} py modules, {len(union_py_packages)} py packages)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
