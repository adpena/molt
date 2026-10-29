#!/usr/bin/env python3
"""Cross-language / cross-file duplicate-authority drift gate.

Molt has several tables that are hand-mirrored across the Python frontend and
the Rust runtime (or across sibling Python tools). A one-value drift in any of
them is a SILENT MISCOMPILE — the frontend emits an integer/ordinal/name that
the runtime reads back to construct the WRONG builtin type or exception, or a
supported-version bump lands coverage on one side only. Before this gate, no
tool bound these copies (`tools/check_correspondence.py` matches the Lean/Python
type-tag NAME but discards the integer).

This is the enforcement authority for that class. It parses ALL copies from
source (nothing hardcoded except the authoritative name<->name transforms) and
asserts equality, failing on any mismatch. The bound tables are:

  #1 builtin-type numeric tags     (frontend `BUILTIN_TYPE_TAGS`
                                     <-> runtime `TYPE_TAG_*` / `BUILTIN_TAG_*`)
  #4 supported target-Python vers  (cli authority <-> stdlib-union baseline
                                     <-> gen_stdlib_module_union generator)
  #5 PyModuleDef_Slot tokens        (inline `include/molt/Python.h`
                                     <-> standalone `molt-cpython-abi/include/Python.h`
                                     both vs CPython 3.12 moduleobject.h)

Builtin exception codes and names are not mirrored: `runtime/exception_kinds.toml`
generates both the runtime types and the frontend tables (HF-207).

Authority: the Rust runtime owns #1; the CLI
`target_python_policy` module is the single source for #4; CPython 3.12
`Include/moduleobject.h` is the authority for #5. This gate binds the mirrors to
those authorities.

Usage:
    uv run --python 3.12 python3 tools/check_table_drift.py
    uv run --python 3.12 python3 tools/check_table_drift.py --check
    uv run --python 3.12 python3 tools/check_table_drift.py --json
    uv run --python 3.12 python3 tools/check_table_drift.py --category type-tags
    uv run --python 3.12 python3 tools/check_table_drift.py --verbose
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]


from molt import target_python_policy  # noqa: E402
from molt.rust_source_scan import mask_rust_comments_and_strings  # noqa: E402

# --- Source paths ---------------------------------------------------------
FRONTEND_TYPES_PY = ROOT / "src" / "molt" / "frontend" / "_types.py"
TYPE_IDS_RS = ROOT / "runtime" / "molt-runtime" / "src" / "object" / "type_ids.rs"
STDLIB_UNION_PY = ROOT / "tools" / "stdlib_module_union.py"
GEN_STDLIB_UNION_PY = ROOT / "tools" / "gen_stdlib_module_union.py"
# CPython-ABI PyModuleDef_Slot token headers (two ABI homes: inline + standalone lib).
CPYTHON_ABI_HEADER = ROOT / "include" / "molt" / "Python.h"
CPYTHON_ABI_STANDALONE_HEADER = (
    ROOT / "runtime" / "molt-cpython-abi" / "include" / "Python.h"
)


# --- Result model ---------------------------------------------------------
@dataclass
class CheckItem:
    name: str
    ok: bool
    detail: str = ""


@dataclass
class CategoryResult:
    key: str
    title: str
    items: list[CheckItem] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return all(item.ok for item in self.items)


# --- Terminal colors ------------------------------------------------------
IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(t: str) -> str:
    return _c("32", t)


def red(t: str) -> str:
    return _c("31", t)


def bold(t: str) -> str:
    return _c("1", t)


def _read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


# --- Shared parsers -------------------------------------------------------
def _parse_python_assign(text: str, name: str) -> ast.expr | None:
    """Return the RHS AST of a module-level ``name = <expr>`` assignment."""
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return None
    for node in tree.body:
        if isinstance(node, ast.Assign):
            for tgt in node.targets:
                if isinstance(tgt, ast.Name) and tgt.id == name:
                    return node.value
        elif isinstance(node, ast.AnnAssign):
            if (
                isinstance(node.target, ast.Name)
                and node.target.id == name
                and node.value is not None
            ):
                return node.value
    return None


def _camel_to_upper_snake(name: str) -> str:
    """`BaseException` -> `BASE_EXCEPTION`, `int` -> `INT`."""
    snake = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", name)
    return snake.upper()


def _parse_rust_i64_consts(text: str) -> dict[str, int]:
    """Map bare Rust `const <NAME>: i64 = <int>;` declarations to values."""
    out: dict[str, int] = {}
    pat = re.compile(
        r"const\s+([A-Z][A-Z0-9_]*)\s*:\s*i64\s*=\s*(-?\d+)\s*;",
    )
    for m in pat.finditer(mask_rust_comments_and_strings(text)):
        out[m.group(1)] = int(m.group(2))
    return out


def _literal_tuple_of_str(node: ast.expr | None) -> tuple[str, ...] | None:
    if node is None:
        return None
    try:
        value = ast.literal_eval(node)
    except (ValueError, SyntaxError):
        return None
    if isinstance(value, (tuple, list)) and all(isinstance(v, str) for v in value):
        return tuple(value)
    return None


# --- Category #1: builtin-type numeric tags -------------------------------
def check_type_tags() -> CategoryResult:
    result = CategoryResult(
        "type-tags",
        "#1 builtin-type numeric tags (frontend BUILTIN_TYPE_TAGS <-> runtime TYPE_TAG_*/BUILTIN_TAG_*)",
    )
    py_text = _read(FRONTEND_TYPES_PY)
    rs_text = _read(TYPE_IDS_RS)
    if not py_text:
        result.items.append(CheckItem("source", False, f"missing {FRONTEND_TYPES_PY}"))
        return result
    if not rs_text:
        result.items.append(CheckItem("source", False, f"missing {TYPE_IDS_RS}"))
        return result

    rhs = _parse_python_assign(py_text, "BUILTIN_TYPE_TAGS")
    py_tags: dict[str, int] | None = None
    if rhs is not None:
        try:
            evaluated = ast.literal_eval(rhs)
            if isinstance(evaluated, dict):
                py_tags = {str(k): int(v) for k, v in evaluated.items()}
        except (ValueError, SyntaxError):
            py_tags = None
    if not py_tags:
        result.items.append(
            CheckItem("BUILTIN_TYPE_TAGS", False, "could not parse frontend dict")
        )
        return result

    rust_consts = _parse_rust_i64_consts(rs_text)
    result.items.append(
        CheckItem(
            "sources",
            True,
            f"frontend: {len(py_tags)} entries; runtime: {len(rust_consts)} i64 tag consts",
        )
    )

    for name, py_value in sorted(py_tags.items()):
        suffix = _camel_to_upper_snake(name)
        # The runtime uses TYPE_TAG_<X> for value-carrying types and
        # BUILTIN_TAG_<X> for the object/type/exception/descriptor family.
        candidates = [f"TYPE_TAG_{suffix}", f"BUILTIN_TAG_{suffix}"]
        found = [c for c in candidates if c in rust_consts]
        if not found:
            result.items.append(
                CheckItem(
                    name,
                    False,
                    f"frontend tag {py_value} has no runtime const "
                    f"({' or '.join(candidates)})",
                )
            )
            continue
        rust_name = found[0]
        rust_value = rust_consts[rust_name]
        ok = rust_value == py_value
        result.items.append(
            CheckItem(
                name,
                ok,
                f"{name}={py_value} <-> {rust_name}={rust_value}"
                if ok
                else f"DRIFT: frontend {name}={py_value} but {rust_name}={rust_value}",
            )
        )
    return result


# --- Category #4: supported target-Python versions ------------------------
def check_target_python_versions() -> CategoryResult:
    result = CategoryResult(
        "target-python",
        "#4 supported target-Python versions (cli authority <-> stdlib-union baseline <-> generator)",
    )
    union_text = _read(STDLIB_UNION_PY)
    gen_text = _read(GEN_STDLIB_UNION_PY)
    if not union_text or not gen_text:
        result.items.append(
            CheckItem("source", False, "missing baseline or generator source")
        )
        return result

    # Import the pure admission authority directly. Release metadata and its
    # generated source projection are not prerequisites for this policy check.
    authority_versions = target_python_policy.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
    result.items.append(
        CheckItem(
            "authority",
            True,
            f"target_python_policy authority: {authority_versions}",
        )
    )

    baseline = _literal_tuple_of_str(
        _parse_python_assign(union_text, "BASELINE_PYTHON_VERSIONS")
    )
    result.items.append(
        CheckItem(
            "stdlib-union-baseline",
            baseline is not None and tuple(baseline) == authority_versions,
            f"BASELINE_PYTHON_VERSIONS={baseline} vs authority {authority_versions}"
            if baseline is not None
            else "could not parse BASELINE_PYTHON_VERSIONS",
        )
    )

    width_text = _read(ROOT / "runtime/molt-obj-model/src/unicode_generated.rs")
    width_versions = tuple(
        f"{major}.{minor}"
        for major, minor in re.findall(
            r"^\s*\((\d+),\s*(\d+)\) => Some\(&DATA_", width_text, re.MULTILINE
        )
    )
    from tools.cpython_regrtest import load_cpython_sources

    source_versions = tuple(
        load_cpython_sources(ROOT / "config/cpython_regrtest_sources.toml")
    )
    result.items.append(
        CheckItem(
            "unicode-facts-source-versions",
            width_versions == source_versions,
            f"Unicode facts sources={width_versions} vs pinned cohort {source_versions}; regenerate with tools/gen_unicode_width.py",
        )
    )

    # The generator now derives DEFAULT_PYTHONS from the authority import; assert
    # it no longer re-declares an independent literal tuple (a re-declared literal
    # would be silent drift). A literal-string tuple RHS is the drift signal.
    gen_rhs = _parse_python_assign(gen_text, "DEFAULT_PYTHONS")
    gen_literal = _literal_tuple_of_str(gen_rhs)
    derives_from_authority = (
        gen_rhs is not None
        and gen_literal is None
        and "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS" in gen_text
    )
    result.items.append(
        CheckItem(
            "generator-derives-from-authority",
            derives_from_authority,
            "DEFAULT_PYTHONS derives from SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS"
            if derives_from_authority
            else f"DRIFT: DEFAULT_PYTHONS re-declares a literal tuple {gen_literal}",
        )
    )
    return result


# PyModuleDef_Slot tokens per CPython 3.12 Include/moduleobject.h.
#   Slot IDs (int, assigned to PyModuleDef_Slot.slot):
_PYMOD_SLOT_IDS = {
    "Py_mod_create": "1",
    "Py_mod_exec": "2",
    "Py_mod_multiple_interpreters": "3",
    "Py_mod_gil": "4",
}
#   Slot VALUES (void*, assigned to PyModuleDef_Slot.value) — CPython casts to
#   (void *) so they slot directly into the pointer field without -Wint-conversion.
_PYMOD_SLOT_VALUES = {
    "Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED": "((void *)0)",
    "Py_MOD_MULTIPLE_INTERPRETERS_SUPPORTED": "((void *)1)",
    "Py_MOD_PER_INTERPRETER_GIL_SUPPORTED": "((void *)2)",
    "Py_MOD_GIL_USED": "((void *)0)",
    "Py_MOD_GIL_NOT_USED": "((void *)1)",
}


def _parse_c_defines(path: Path, names: set[str]) -> dict[str, str]:
    """Return {macro: whitespace-normalized body} for the requested #define names."""
    found: dict[str, str] = {}
    text = _read(path)
    if not text:
        return found
    for name in names:
        m = re.search(
            r"^[ \t]*#[ \t]*define[ \t]+" + re.escape(name) + r"[ \t]+(.+?)[ \t]*$",
            text,
            re.MULTILINE,
        )
        if m:
            # Collapse whitespace so ((void *)2) and ((void  *) 2) compare equal.
            found[name] = re.sub(r"\s+", "", m.group(1).strip())
    return found


def check_pymod_slot_tokens() -> CategoryResult:
    """Bind PyModuleDef_Slot token values across both CPython-ABI header homes.

    The inline header (include/molt/Python.h) and the standalone-lib header
    (runtime/molt-cpython-abi/include/Python.h) both declare the Py_mod_* slot
    IDs and Py_MOD_* slot values. A value-token drift (e.g. a plain ``2`` instead
    of ``((void *)2)``) breaks any extension whose PyModuleDef_Slot array assigns
    the token into the void* value field — the SciPy _nd_image build hit exactly
    this. Authority is CPython 3.12 Include/moduleobject.h; this gate asserts both
    headers match it and thus each other.
    """
    result = CategoryResult(
        key="pymod-slot-tokens",
        title="CPython-ABI PyModuleDef_Slot tokens (both header homes vs CPython 3.12)",
    )
    expected = {
        k: re.sub(r"\s+", "", v)
        for k, v in {**_PYMOD_SLOT_IDS, **_PYMOD_SLOT_VALUES}.items()
    }
    headers = {
        "inline (include/molt/Python.h)": CPYTHON_ABI_HEADER,
        "standalone (molt-cpython-abi/include/Python.h)": CPYTHON_ABI_STANDALONE_HEADER,
    }
    all_names = set(expected)
    for label, path in headers.items():
        if not path.exists():
            result.items.append(CheckItem(label, False, f"missing header {path}"))
            continue
        got = _parse_c_defines(path, all_names)
        for name, want in expected.items():
            have = got.get(name)
            if have is None:
                result.items.append(
                    CheckItem(f"{label}:{name}", False, "define not found")
                )
            else:
                ok = have == want
                result.items.append(
                    CheckItem(
                        f"{label}:{name}",
                        ok,
                        f"{name} = {have}"
                        if ok
                        else f"DRIFT: expected {want}, got {have}",
                    )
                )
    return result


CATEGORIES = {
    "type-tags": check_type_tags,
    "target-python": check_target_python_versions,
    "pymod-slot-tokens": check_pymod_slot_tokens,
}


def run(selected: str | None) -> list[CategoryResult]:
    if selected:
        fn = CATEGORIES.get(selected)
        if fn is None:
            raise SystemExit(
                f"unknown category {selected!r}; choose from {', '.join(CATEGORIES)}"
            )
        return [fn()]
    return [fn() for fn in CATEGORIES.values()]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Exit non-zero on any drift (default behavior; kept for gate symmetry).",
    )
    parser.add_argument(
        "--json", action="store_true", help="Emit machine-readable JSON."
    )
    parser.add_argument(
        "--verbose", action="store_true", help="Print every item, not just failures."
    )
    parser.add_argument(
        "--category",
        choices=sorted(CATEGORIES),
        help="Run only one category.",
    )
    args = parser.parse_args()

    results = run(args.category)
    all_ok = all(r.ok for r in results)

    if args.json:
        payload = {
            "ok": all_ok,
            "categories": [
                {
                    "key": r.key,
                    "title": r.title,
                    "ok": r.ok,
                    "items": [
                        {"name": it.name, "ok": it.ok, "detail": it.detail}
                        for it in r.items
                    ],
                }
                for r in results
            ],
        }
        print(json.dumps(payload, indent=2))
        return 0 if all_ok else 1

    for r in results:
        status = green("PASS") if r.ok else red("FAIL")
        print(f"{bold(r.title)}: {status}")
        for it in r.items:
            if it.ok and not args.verbose:
                continue
            mark = green("  ok ") if it.ok else red(" DRIFT")
            print(f"  {mark} {it.name}: {it.detail}")
        print()

    if all_ok:
        print(green("All duplicate-authority tables are in sync."))
    else:
        print(
            red(
                "Duplicate-authority DRIFT detected — a mismatch is a silent miscompile."
            )
        )
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
