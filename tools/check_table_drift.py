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
  #2 builtin-exception ordinals    (frontend `BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS`
                                     <-> runtime `builtin_exception_name_for_tag`)
  #3 builtin-exception name set    (frontend `BUILTIN_EXCEPTION_NAMES`
                                     <-> runtime `exception_base_spec` + roots)
  #4 supported target-Python vers  (cli authority <-> stdlib-union baseline
                                     <-> gen_stdlib_module_union generator)
  #5 PyModuleDef_Slot tokens        (inline `include/molt/Python.h`
                                     <-> standalone `molt-cpython-abi/include/Python.h`
                                     both vs CPython 3.12 moduleobject.h)

Authority: the Rust runtime is the constructor-of-truth for #1/#2/#3; the CLI
`TargetPythonVersion` tuple is the single source for #4; CPython 3.12
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
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path


def _find_repo_root() -> Path:
    try:
        out = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"],
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
        if out:
            return Path(out)
    except (subprocess.CalledProcessError, FileNotFoundError):
        pass
    return Path(__file__).resolve().parents[1]


ROOT = _find_repo_root()

# --- Source paths ---------------------------------------------------------
FRONTEND_TYPES_PY = ROOT / "src" / "molt" / "frontend" / "_types.py"
TARGET_PYTHON_PY = ROOT / "src" / "molt" / "cli" / "target_python.py"
TYPE_IDS_RS = ROOT / "runtime" / "molt-runtime" / "src" / "object" / "type_ids.rs"
EXCEPTIONS_RS = ROOT / "runtime" / "molt-runtime" / "src" / "builtins" / "exceptions.rs"
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
    for m in pat.finditer(text):
        out[m.group(1)] = int(m.group(2))
    return out


def _parse_rust_match_ordinals(text: str, fn_name: str) -> dict[int, str]:
    """Parse `<int> => Some("<Name>")` arms inside a named Rust fn body.

    Returns {ordinal: name}. Handles multi-line arms (the string literal may be
    on a following line inside `Some(...)`).
    """
    # Isolate the function body by brace matching from the fn signature.
    sig = re.search(rf"fn\s+{re.escape(fn_name)}\b[^{{]*\{{", text)
    if sig is None:
        return {}
    start = sig.end() - 1  # position of the opening brace
    depth = 0
    end = start
    for i in range(start, len(text)):
        ch = text[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                end = i
                break
    body = text[start : end + 1]
    out: dict[int, str] = {}
    # `1 => Some("BaseException")` possibly wrapped across lines.
    for m in re.finditer(
        r"(\d+)\s*=>\s*Some\(\s*\"([A-Za-z_][A-Za-z0-9_]*)\"",
        body,
    ):
        out[int(m.group(1))] = m.group(2)
    return out


def _parse_rust_string_literals_in_fn(text: str, fn_name: str) -> set[str]:
    """Collect every double-quoted string literal in a named Rust fn body."""
    sig = re.search(rf"fn\s+{re.escape(fn_name)}\b[^{{]*\{{", text)
    if sig is None:
        return set()
    start = sig.end() - 1
    depth = 0
    end = start
    for i in range(start, len(text)):
        ch = text[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                end = i
                break
    body = text[start : end + 1]
    return set(re.findall(r'"([A-Za-z_][A-Za-z0-9_]*)"', body))


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


# --- Category #2: builtin-exception constructor ordinals -------------------
def check_exception_ordinals() -> CategoryResult:
    result = CategoryResult(
        "exception-ordinals",
        "#2 builtin-exception constructor ordinals (frontend BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS <-> runtime builtin_exception_name_for_tag)",
    )
    py_text = _read(FRONTEND_TYPES_PY)
    rs_text = _read(EXCEPTIONS_RS)
    if not py_text or not rs_text:
        result.items.append(
            CheckItem("source", False, "missing frontend or runtime source")
        )
        return result

    rhs = _parse_python_assign(py_text, "BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS")
    py_ordinals: dict[int, str] = {}
    # The frontend builds this via `enumerate((<names...>), start=1)`. Recover
    # the ordered name tuple from the enumerate() call so we compare the actual
    # positional intent, not just the literal dict.
    names_tuple: tuple[str, ...] | None = None
    if isinstance(rhs, ast.DictComp):
        for call in ast.walk(rhs):
            if (
                isinstance(call, ast.Call)
                and isinstance(call.func, ast.Name)
                and call.func.id == "enumerate"
                and call.args
            ):
                names_tuple = _literal_tuple_of_str(call.args[0])
                break
    if names_tuple is not None:
        py_ordinals = {i: n for i, n in enumerate(names_tuple, start=1)}
    if not py_ordinals:
        result.items.append(
            CheckItem(
                "BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS",
                False,
                "could not parse frontend enumerate() ordinal tuple",
            )
        )
        return result

    rust_ordinals = _parse_rust_match_ordinals(
        rs_text, "builtin_exception_name_for_tag"
    )
    if not rust_ordinals:
        result.items.append(
            CheckItem(
                "builtin_exception_name_for_tag",
                False,
                "could not parse runtime tag->name match arms",
            )
        )
        return result

    result.items.append(
        CheckItem(
            "sources",
            True,
            f"frontend: {len(py_ordinals)} ordinals; runtime: {len(rust_ordinals)} match arms",
        )
    )

    all_ordinals = sorted(set(py_ordinals) | set(rust_ordinals))
    for ordinal in all_ordinals:
        py_name = py_ordinals.get(ordinal)
        rust_name = rust_ordinals.get(ordinal)
        ok = py_name is not None and py_name == rust_name
        if ok:
            detail = f"{ordinal} -> {py_name}"
        elif py_name is None:
            detail = f"DRIFT: ordinal {ordinal} only in runtime (={rust_name})"
        elif rust_name is None:
            detail = f"DRIFT: ordinal {ordinal} only in frontend (={py_name})"
        else:
            detail = f"DRIFT: ordinal {ordinal} frontend={py_name} runtime={rust_name}"
        result.items.append(CheckItem(f"ordinal {ordinal}", ok, detail))

    # The count must also agree (a trailing name added on one side only).
    result.items.append(
        CheckItem(
            "ordinal-count",
            len(py_ordinals) == len(rust_ordinals),
            f"frontend={len(py_ordinals)} runtime={len(rust_ordinals)}",
        )
    )
    return result


# --- Category #3: builtin-exception name set ------------------------------
def check_exception_names() -> CategoryResult:
    result = CategoryResult(
        "exception-names",
        "#3 builtin-exception name set (frontend BUILTIN_EXCEPTION_NAMES <-> runtime exception_base_spec)",
    )
    py_text = _read(FRONTEND_TYPES_PY)
    rs_text = _read(EXCEPTIONS_RS)
    if not py_text or not rs_text:
        result.items.append(
            CheckItem("source", False, "missing frontend or runtime source")
        )
        return result

    rhs = _parse_python_assign(py_text, "BUILTIN_EXCEPTION_NAMES")
    py_names: set[str] | None = None
    if rhs is not None:
        try:
            evaluated = ast.literal_eval(rhs)
            if isinstance(evaluated, (set, frozenset, list, tuple)):
                py_names = {str(v) for v in evaluated}
        except (ValueError, SyntaxError):
            py_names = None
    if not py_names:
        result.items.append(
            CheckItem("BUILTIN_EXCEPTION_NAMES", False, "could not parse frontend set")
        )
        return result

    # Runtime authority: `exception_base_spec` names every non-root exception
    # (as the match subject and as its base names) plus `exception_alias_name`
    # aliases. The roots BaseException / Exception / Warning are handled
    # directly in exception_type_bits_from_name and have no base spec, so we add
    # them as known roots.
    spec_subjects = _parse_rust_string_literals_in_fn(rs_text, "exception_base_spec")
    alias_subjects = _parse_rust_string_literals_in_fn(rs_text, "exception_alias_name")
    ROOTS = {"BaseException", "Exception", "Warning"}
    # Runtime-constructible names = every name the runtime can name as a class:
    # spec subjects/bases + aliases + roots.
    rust_names = spec_subjects | alias_subjects | ROOTS

    result.items.append(
        CheckItem(
            "sources",
            True,
            f"frontend: {len(py_names)} names; runtime spec/alias/roots: {len(rust_names)} names",
        )
    )

    # Every frontend-accepted exception name MUST be runtime-constructible,
    # otherwise the frontend accepts a raise the runtime cannot build/parent.
    missing_in_runtime = sorted(py_names - rust_names)
    result.items.append(
        CheckItem(
            "frontend-subset-of-runtime",
            not missing_in_runtime,
            "every frontend exception name is runtime-constructible"
            if not missing_in_runtime
            else f"DRIFT: frontend names not runtime-constructible: {missing_in_runtime}",
        )
    )

    # Every runtime non-root/non-alias exception (a real hierarchy leaf the
    # runtime can raise) MUST be a frontend-known name, else the runtime can
    # construct/parent a class the frontend will never accept as a builtin
    # exception. Aliases (EnvironmentError/IOError/WindowsError) and internal
    # helper names that are not builtins are intentionally excluded.
    KNOWN_NON_FRONTEND = {
        # asyncio.CancelledError and io.UnsupportedOperation are stdlib
        # exceptions the runtime models for parenting, not frontend builtins.
        "CancelledError",
        "UnsupportedOperation",
    }
    # Base-name-only tokens (parents referenced but not subjects) are covered
    # because subjects include them; restrict the reverse check to spec subjects
    # to avoid flagging parent-only tokens that are themselves roots.
    runtime_leaves = (spec_subjects | ROOTS) - alias_subjects - KNOWN_NON_FRONTEND
    missing_in_frontend = sorted(runtime_leaves - py_names)
    result.items.append(
        CheckItem(
            "runtime-subset-of-frontend",
            not missing_in_frontend,
            "every runtime builtin exception is frontend-known"
            if not missing_in_frontend
            else f"DRIFT: runtime exceptions unknown to frontend: {missing_in_frontend}",
        )
    )
    return result


# --- Category #4: supported target-Python versions ------------------------
def check_target_python_versions() -> CategoryResult:
    result = CategoryResult(
        "target-python",
        "#4 supported target-Python versions (cli authority <-> stdlib-union baseline <-> generator)",
    )
    cli_text = _read(TARGET_PYTHON_PY)
    union_text = _read(STDLIB_UNION_PY)
    gen_text = _read(GEN_STDLIB_UNION_PY)
    if not cli_text or not union_text or not gen_text:
        result.items.append(
            CheckItem("source", False, "missing one of the three sources")
        )
        return result

    # Authority: parse the TargetPythonVersion(major, minor, ...) tuple from the
    # CLI so we do not depend on importing packaging in the gate env.
    authority: list[str] = []
    for m in re.finditer(
        r"TargetPythonVersion\(\s*(\d+)\s*,\s*(\d+)\s*,",
        cli_text,
    ):
        authority.append(f"{m.group(1)}.{m.group(2)}")
    # De-dup preserving order (the tuple is the authoritative sequence).
    seen: dict[str, None] = {}
    for v in authority:
        seen.setdefault(v, None)
    authority_versions = tuple(seen)

    if not authority_versions:
        result.items.append(
            CheckItem(
                "authority",
                False,
                "could not parse _SUPPORTED_TARGET_PYTHON_VERSIONS tuple",
            )
        )
        return result
    result.items.append(
        CheckItem(
            "authority",
            True,
            f"cli target_python authority: {authority_versions}",
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
    "exception-ordinals": check_exception_ordinals,
    "exception-names": check_exception_names,
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
