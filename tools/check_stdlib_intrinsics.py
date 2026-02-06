#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import io
import json
import re
import tokenize
from collections import deque
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = ROOT / "src" / "molt" / "stdlib"
MANIFEST = ROOT / "runtime" / "molt-runtime" / "src" / "intrinsics" / "manifest.pyi"
AUDIT_DOC = (
    ROOT / "docs" / "spec" / "areas" / "compat" / "0016_STDLIB_INTRINSICS_AUDIT.md"
)

TEXT_TOKENS = (
    "load_intrinsic",
    "require_intrinsic",
    "require_optional_intrinsic",
    "_intrinsic_load",
    "_require_intrinsic",
    "_intrinsics_require",
    "_intrinsic_require",
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

INTRINSIC_NAME_RE = re.compile(r"['\"](molt_[a-zA-Z0-9_]+)['\"]")
MANIFEST_INTRINSIC_RE = re.compile(r"^def\s+(molt_[a-zA-Z0-9_]+)\(")

PROBE_INTRINSIC = "molt_stdlib_probe"
STATUS_INTRINSIC = "intrinsic-backed"
STATUS_INTRINSIC_PARTIAL = "intrinsic-partial"
STATUS_PROBE_ONLY = "probe-only"
STATUS_PYTHON_ONLY = "python-only"

STDLIB_TODO_RE = re.compile(
    r"TODO\(stdlib-compat,[^)]*status:(?:missing|partial|planned|divergent)\)"
)

BOOTSTRAP_MODULES = {
    "__future__",
    "_abc",
    "_collections_abc",
    "_weakrefset",
    "abc",
    "collections.abc",
    "copy",
    "copyreg",
    "dataclasses",
    "keyword",
    "linecache",
    "re",
    "reprlib",
    "types",
    "typing",
    "warnings",
    "weakref",
}
PRIORITY_LOWERING_QUEUES: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "P0 queue (Phase 2: concurrency substrate)",
        ("socket", "select", "selectors", "threading", "asyncio"),
    ),
    (
        "P1 queue (Phase 3: core-adjacent stdlib)",
        (
            "builtins",
            "types",
            "weakref",
            "math",
            "re",
            "struct",
            "time",
            "inspect",
            "functools",
            "itertools",
            "operator",
            "contextlib",
        ),
    ),
    (
        "P2 queue (Phase 4: import/data/network long tail)",
        (
            "pathlib",
            "importlib",
            "importlib.util",
            "importlib.machinery",
            "pkgutil",
            "glob",
            "shutil",
            "py_compile",
            "compileall",
            "json",
            "csv",
            "pickle",
            "enum",
            "ipaddress",
            "encodings",
            "ssl",
            "subprocess",
            "concurrent.futures",
            "http.client",
            "http.server",
        ),
    ),
)
INTRINSIC_CALL_NAMES = {
    "load_intrinsic",
    "require_intrinsic",
    "require_optional_intrinsic",
    "_load_intrinsic",
    "_intrinsic_load",
    "_intrinsics_require",
    "_intrinsic_require",
    "_require_intrinsic",
    "_require_callable_intrinsic",
}


@dataclass(frozen=True)
class ModuleAudit:
    module: str
    path: Path
    intrinsic_names: tuple[str, ...]
    status: str


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


def _module_name(path: Path) -> str:
    rel = path.relative_to(STDLIB_ROOT)
    if rel.name == "__init__.py":
        if len(rel.parts) == 1:
            return "molt.stdlib"
        return ".".join(rel.parts[:-1])
    return ".".join((*rel.parts[:-1], rel.stem))


def _canonical_module(name: str, known: set[str]) -> str | None:
    if name in known:
        return name
    root = name.split(".", 1)[0]
    if root in known:
        return root
    return None


def _module_package_parts(module: str, path: Path) -> list[str]:
    if module == "molt.stdlib":
        return []
    parts = module.split(".")
    if path.name == "__init__.py":
        return parts
    return parts[:-1]


def _resolve_from_target(
    *,
    current_module: str,
    current_path: Path,
    level: int,
    module: str | None,
    name: str | None,
) -> str | None:
    package_parts = _module_package_parts(current_module, current_path)
    if level <= 0:
        if module is None:
            return name
        if name is None:
            return module
        return f"{module}.{name}"
    if level > len(package_parts) + 1:
        return None
    anchor = package_parts[: len(package_parts) - level + 1]
    suffix: list[str] = []
    if module:
        suffix.extend(module.split("."))
    if name:
        suffix.append(name)
    parts = anchor + suffix
    if not parts:
        return None
    return ".".join(parts)


def _call_name(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def _extract_intrinsic_names(text: str) -> tuple[str, ...]:
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return tuple(sorted(set(INTRINSIC_NAME_RE.findall(text))))

    names: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        call_name = _call_name(node.func)
        if call_name not in INTRINSIC_CALL_NAMES:
            continue
        if not node.args:
            continue
        first = node.args[0]
        if isinstance(first, ast.Constant) and isinstance(first.value, str):
            value = first.value
            if value.startswith("molt_"):
                names.add(value)
    return tuple(sorted(names))


def _imports_from_ast(
    *,
    tree: ast.AST,
    current_module: str,
    current_path: Path,
    known_modules: set[str],
) -> set[str]:
    out: set[str] = set()

    def _static_bool(expr: ast.expr) -> bool | None:
        if isinstance(expr, ast.Constant) and isinstance(expr.value, bool):
            return expr.value
        if isinstance(expr, ast.Name) and expr.id == "TYPE_CHECKING":
            return False
        if isinstance(expr, ast.Attribute) and expr.attr == "TYPE_CHECKING":
            return False
        if isinstance(expr, ast.UnaryOp) and isinstance(expr.op, ast.Not):
            val = _static_bool(expr.operand)
            return None if val is None else not val
        if isinstance(expr, ast.BoolOp):
            values = [_static_bool(value) for value in expr.values]
            if isinstance(expr.op, ast.And):
                if any(value is False for value in values):
                    return False
                if all(value is True for value in values):
                    return True
                return None
            if isinstance(expr.op, ast.Or):
                if any(value is True for value in values):
                    return True
                if all(value is False for value in values):
                    return False
                return None
        return None

    def _collect_stmt_list(statements: list[ast.stmt]) -> None:
        for stmt in statements:
            if isinstance(stmt, ast.Import):
                for alias in stmt.names:
                    resolved = _canonical_module(alias.name, known_modules)
                    if resolved:
                        out.add(resolved)
                continue
            if isinstance(stmt, ast.ImportFrom):
                module_name = _resolve_from_target(
                    current_module=current_module,
                    current_path=current_path,
                    level=stmt.level,
                    module=stmt.module,
                    name=None,
                )
                if module_name:
                    resolved = _canonical_module(module_name, known_modules)
                    if resolved:
                        out.add(resolved)
                for alias in stmt.names:
                    if alias.name == "*":
                        continue
                    target = _resolve_from_target(
                        current_module=current_module,
                        current_path=current_path,
                        level=stmt.level,
                        module=stmt.module,
                        name=alias.name,
                    )
                    if not target:
                        continue
                    resolved = _canonical_module(target, known_modules)
                    if resolved:
                        out.add(resolved)
                continue
            if isinstance(stmt, ast.If):
                truth = _static_bool(stmt.test)
                if truth is True:
                    _collect_stmt_list(stmt.body)
                elif truth is False:
                    _collect_stmt_list(stmt.orelse)
                else:
                    _collect_stmt_list(stmt.body)
                    _collect_stmt_list(stmt.orelse)
                continue
            if isinstance(stmt, ast.Try):
                _collect_stmt_list(stmt.body)
                for handler in stmt.handlers:
                    _collect_stmt_list(handler.body)
                _collect_stmt_list(stmt.orelse)
                _collect_stmt_list(stmt.finalbody)
                continue
            if isinstance(
                stmt,
                (
                    ast.FunctionDef,
                    ast.AsyncFunctionDef,
                    ast.ClassDef,
                ),
            ):
                continue
            child_bodies: list[list[ast.stmt]] = []
            for field_name in ("body", "orelse", "finalbody"):
                child = getattr(stmt, field_name, None)
                if (
                    isinstance(child, list)
                    and child
                    and all(isinstance(item, ast.stmt) for item in child)
                ):
                    child_bodies.append(child)
            for child_body in child_bodies:
                _collect_stmt_list(child_body)

    if isinstance(tree, ast.Module):
        _collect_stmt_list(tree.body)
    return out


def _build_stdlib_dep_graph(
    modules: dict[str, ModuleAudit],
) -> dict[str, set[str]]:
    known_modules = set(modules)
    deps: dict[str, set[str]] = {}
    for module, audit in modules.items():
        tree = ast.parse(audit.path.read_text(encoding="utf-8"), filename=str(audit.path))
        deps[module] = _imports_from_ast(
            tree=tree,
            current_module=module,
            current_path=audit.path,
            known_modules=known_modules,
        )
    return deps


def _closure(seeds: set[str], deps: dict[str, set[str]]) -> set[str]:
    seen: set[str] = set()
    queue: deque[str] = deque(sorted(seeds))
    while queue:
        module = queue.popleft()
        if module in seen:
            continue
        seen.add(module)
        for dep in sorted(deps.get(module, ())):
            if dep not in seen:
                queue.append(dep)
    return seen


def _scan_file(path: Path) -> tuple[list[str], tuple[str, ...], str, bool]:
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

    intrinsic_names = _extract_intrinsic_names(text)
    has_stdlib_todo = bool(STDLIB_TODO_RE.search(text))
    if is_registry_file:
        status = STATUS_INTRINSIC
    elif not intrinsic_names:
        status = STATUS_PYTHON_ONLY
    elif set(intrinsic_names) == {PROBE_INTRINSIC}:
        status = STATUS_PROBE_ONLY
    else:
        status = STATUS_INTRINSIC

    return errors, intrinsic_names, status, has_stdlib_todo


def _load_manifest_intrinsics() -> set[str]:
    if not MANIFEST.exists():
        raise RuntimeError(f"intrinsics manifest missing: {MANIFEST}")
    out: set[str] = set()
    for line in MANIFEST.read_text(encoding="utf-8").splitlines():
        match = MANIFEST_INTRINSIC_RE.match(line.strip())
        if match:
            out.add(match.group(1))
    return out


def _build_audit_doc(audits: list[ModuleAudit]) -> str:
    intrinsic = sorted(a.module for a in audits if a.status == STATUS_INTRINSIC)
    intrinsic_partial = sorted(
        a.module for a in audits if a.status == STATUS_INTRINSIC_PARTIAL
    )
    probe_only = sorted(a.module for a in audits if a.status == STATUS_PROBE_ONLY)
    python_only = sorted(a.module for a in audits if a.status == STATUS_PYTHON_ONLY)
    status_by_module = {audit.module: audit.status for audit in audits}
    total_modules = len(audits)

    lines = [
        "# Stdlib Intrinsics Audit",
        "**Spec ID:** 0016",
        "**Status:** Draft (enforcement + audit)",
        "**Owner:** stdlib + runtime",
        "",
        "## Policy",
        "- Compiled binaries must not execute Python stdlib implementations.",
        "- Every stdlib module must be backed by Rust intrinsics (Python files are allowed only as thin, intrinsic-forwarding wrappers).",
        "- Modules without intrinsic usage are forbidden in compiled builds and must raise immediately until fully lowered.",
        "",
        "## Progress Summary (Generated)",
        f"- Total audited modules: `{total_modules}`",
        f"- `intrinsic-backed`: `{len(intrinsic)}`",
        f"- `intrinsic-partial`: `{len(intrinsic_partial)}`",
        f"- `probe-only`: `{len(probe_only)}`",
        f"- `python-only`: `{len(python_only)}`",
        "",
        "## Priority Lowering Queue (Generated)",
    ]
    for title, modules in PRIORITY_LOWERING_QUEUES:
        lines.append(f"### {title}")
        for module in modules:
            status = status_by_module.get(module)
            if status is None:
                lines.append(f"- `{module}`: `not-audited`")
            else:
                lines.append(f"- `{module}`: `{status}`")
        lines.append("")

    lines.extend(
        [
            "## Audit (Generated)",
            "### Intrinsic-backed modules (lowering complete)",
        ]
    )
    lines.extend(f"- `{name}`" for name in intrinsic)
    lines.extend(["", "### Intrinsic-backed modules (partial lowering pending)"])
    lines.extend(f"- `{name}`" for name in intrinsic_partial)
    lines.extend(["", "### Probe-only modules (thin wrappers + policy gate only)"])
    lines.extend(f"- `{name}`" for name in probe_only)
    lines.extend(["", "### Python-only modules (intrinsic missing)"])
    lines.extend(f"- `{name}`" for name in python_only)
    lines.extend(
        [
            "",
            "## Core Lane Gate",
            "- Required lane: `tests/differential/core/TESTS.txt` (import closure).",
            "- Gate rule: core-lane imports must be `intrinsic-backed` only (no `intrinsic-partial`, `probe-only`, or `python-only`).",
            "- Enforced by: `python3 tools/check_core_lane_lowering.py`.",
            "",
            "## Bootstrap Gate",
            "- Required modules: "
            + ", ".join(f"`{name}`" for name in sorted(BOOTSTRAP_MODULES)),
            "- Gate rule: bootstrap modules must not be `python-only`.",
            "",
            "## TODO",
            "- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Stdlib intrinsics lint + audit gate")
    parser.add_argument(
        "--update-doc",
        action="store_true",
        help=(
            "Rewrite docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md "
            "with generated audit output."
        ),
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        help="Write machine-readable audit report JSON to this path.",
    )
    args = parser.parse_args()

    if not STDLIB_ROOT.is_dir():
        print(f"stdlib root missing: {STDLIB_ROOT}")
        return 1

    manifest_intrinsics = _load_manifest_intrinsics()
    failures: list[tuple[Path, list[str]]] = []
    missing_intrinsics: list[tuple[str, str]] = []
    audits: list[ModuleAudit] = []

    for path in sorted(STDLIB_ROOT.rglob("*.py")):
        if path.name.startswith("."):
            continue
        errors, intrinsic_names, status, has_stdlib_todo = _scan_file(path)
        module = _module_name(path)
        if status == STATUS_INTRINSIC and has_stdlib_todo:
            status = STATUS_INTRINSIC_PARTIAL
        if errors:
            failures.append((path, errors))
        for name in intrinsic_names:
            if name not in manifest_intrinsics:
                missing_intrinsics.append((str(path.relative_to(ROOT)), name))
        audits.append(
            ModuleAudit(
                module=module,
                path=path,
                intrinsic_names=intrinsic_names,
                status=status,
            )
        )

    bootstrap_failures = [
        audit.module
        for audit in audits
        if audit.module in BOOTSTRAP_MODULES and audit.status == STATUS_PYTHON_ONLY
    ]
    modules_by_name = {audit.module: audit for audit in audits}
    dep_graph = _build_stdlib_dep_graph(modules_by_name)
    python_only_modules = {
        audit.module for audit in audits if audit.status == STATUS_PYTHON_ONLY
    }
    dependency_violations: list[tuple[str, str, tuple[str, ...]]] = []
    for audit in sorted(audits, key=lambda item: item.module):
        if audit.status == STATUS_PYTHON_ONLY:
            continue
        imported_closure = _closure({audit.module}, dep_graph)
        imported_closure.discard(audit.module)
        bad = tuple(sorted(imported_closure & python_only_modules))
        if bad:
            dependency_violations.append((audit.module, audit.status, bad))

    if failures:
        print("stdlib intrinsics lint failed:")
        for path, errors in failures:
            rel = path.relative_to(ROOT)
            print(f"- {rel}")
            for msg in errors:
                print(f"  {msg}")
        return 1

    if missing_intrinsics:
        print("stdlib intrinsics lint failed: unknown intrinsic names")
        for rel, name in sorted(set(missing_intrinsics)):
            print(f"- {rel}: `{name}` is not present in {MANIFEST.relative_to(ROOT)}")
        return 1

    if bootstrap_failures:
        print("stdlib intrinsics lint failed: bootstrap modules cannot be python-only")
        for module in sorted(set(bootstrap_failures)):
            print(f"- {module}")
        return 1

    if dependency_violations:
        print(
            "stdlib intrinsics lint failed: non-python-only modules cannot depend "
            "on python-only stdlib modules"
        )
        for module, status, bad in dependency_violations:
            joined = ", ".join(f"`{name}`" for name in bad)
            print(f"- {module} ({status}) depends on {joined}")
        return 1

    generated_doc = _build_audit_doc(audits)
    if args.update_doc:
        AUDIT_DOC.write_text(generated_doc, encoding="utf-8")
    else:
        if not AUDIT_DOC.exists():
            print(f"stdlib intrinsic audit doc missing: {AUDIT_DOC.relative_to(ROOT)}")
            return 1
        existing = AUDIT_DOC.read_text(encoding="utf-8")
        if existing != generated_doc:
            print(
                "stdlib intrinsic audit doc is out of date. "
                "Run: python3 tools/check_stdlib_intrinsics.py --update-doc"
            )
            return 1

    if args.json_out is not None:
        report = {
            "modules": [
                {
                    "module": audit.module,
                    "path": str(audit.path.relative_to(ROOT)),
                    "status": audit.status,
                    "intrinsics": list(audit.intrinsic_names),
                }
                for audit in sorted(audits, key=lambda a: a.module)
            ]
        }
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

    print("stdlib intrinsics lint: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
