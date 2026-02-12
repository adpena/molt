#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import io
import json
import re
import runpy
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
STDLIB_UNION_BASELINE = ROOT / "tools" / "stdlib_module_union.py"
INTRINSIC_PARTIAL_RATCHET = ROOT / "tools" / "stdlib_intrinsics_ratchet.json"
STDLIB_FULL_COVERAGE_MANIFEST = ROOT / "tools" / "stdlib_full_coverage_manifest.py"

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
    r"TODO\(stdlib[^,]*,[^)]*status:(?:missing|partial|planned|divergent)\)"
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
BOOTSTRAP_STRICT_ROOTS: tuple[str, ...] = (
    "builtins",
    "sys",
    "types",
    "importlib",
    "importlib.machinery",
    "importlib.util",
)
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
CRITICAL_STRICT_IMPORT_ROOTS: tuple[str, ...] = (
    "socket",
    "threading",
    "asyncio",
    "pathlib",
    "time",
    "traceback",
    "sys",
    "os",
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
STRICT_OPTIONAL_INTRINSIC_CALL_NAMES = {
    "load_intrinsic",
    "_load_intrinsic",
    "require_optional_intrinsic",
}
STRICT_IMPORT_FALLBACK_EXCEPTIONS = {
    "ImportError",
    "ModuleNotFoundError",
    "Exception",
    "BaseException",
}
INTRINSIC_RUNTIME_FALLBACK_EXEMPT_PREFIXES = ("test", "test.")
INTRINSIC_PASS_FALLBACK_STRICT_MODULES: tuple[str, ...] = ("json",)


@dataclass(frozen=True)
class ModuleAudit:
    module: str
    path: Path
    intrinsic_names: tuple[str, ...]
    status: str


def _display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def _load_required_top_level_stdlib() -> tuple[frozenset[str], frozenset[str]]:
    if not STDLIB_UNION_BASELINE.exists():
        raise RuntimeError(
            f"stdlib module baseline missing: {_display_path(STDLIB_UNION_BASELINE)}"
        )
    namespace = runpy.run_path(str(STDLIB_UNION_BASELINE))
    raw_union = namespace.get("STDLIB_MODULE_UNION")
    raw_packages = namespace.get("STDLIB_PACKAGE_UNION")
    if not isinstance(raw_union, tuple):
        raise RuntimeError(
            "stdlib module baseline is invalid: STDLIB_MODULE_UNION tuple missing"
        )
    if not isinstance(raw_packages, tuple):
        raise RuntimeError(
            "stdlib module baseline is invalid: STDLIB_PACKAGE_UNION tuple missing"
        )
    union = frozenset(name for name in raw_union if isinstance(name, str))
    packages = frozenset(name for name in raw_packages if isinstance(name, str))
    if not union:
        raise RuntimeError("stdlib module baseline is invalid: union is empty")
    return union, packages


def _load_required_stdlib_submodules() -> tuple[frozenset[str], frozenset[str]]:
    if not STDLIB_UNION_BASELINE.exists():
        raise RuntimeError(
            f"stdlib module baseline missing: {_display_path(STDLIB_UNION_BASELINE)}"
        )
    namespace = runpy.run_path(str(STDLIB_UNION_BASELINE))
    raw_union = namespace.get("STDLIB_PY_SUBMODULE_UNION")
    raw_packages = namespace.get("STDLIB_PY_SUBPACKAGE_UNION")
    if not isinstance(raw_union, tuple):
        raise RuntimeError(
            "stdlib module baseline is invalid: STDLIB_PY_SUBMODULE_UNION tuple missing"
        )
    if not isinstance(raw_packages, tuple):
        raise RuntimeError(
            "stdlib module baseline is invalid: STDLIB_PY_SUBPACKAGE_UNION tuple missing"
        )
    union = frozenset(name for name in raw_union if isinstance(name, str))
    packages = frozenset(name for name in raw_packages if isinstance(name, str))
    return union, packages


def _top_level_entry(path: Path) -> tuple[str, str] | None:
    rel = path.relative_to(STDLIB_ROOT)
    if path.name == "__init__.py":
        if len(rel.parts) != 2:
            return None
        return rel.parts[0], "package"
    if len(rel.parts) != 1:
        return None
    return path.stem, "module"


def _module_entry(path: Path) -> tuple[str, str]:
    rel = path.relative_to(STDLIB_ROOT)
    if path.name == "__init__.py":
        if len(rel.parts) == 1:
            return "molt.stdlib", "package"
        return ".".join(rel.parts[:-1]), "package"
    return ".".join((*rel.parts[:-1], path.stem)), "module"


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


def _exception_type_names(exc: ast.expr | None) -> set[str]:
    if exc is None:
        return {"BaseException"}
    if isinstance(exc, ast.Name):
        return {exc.id}
    if isinstance(exc, ast.Attribute):
        return {exc.attr}
    if isinstance(exc, ast.Tuple):
        names: set[str] = set()
        for item in exc.elts:
            names.update(_exception_type_names(item))
        return names
    return set()


def _stmt_block_has_import(statements: list[ast.stmt]) -> bool:
    for stmt in statements:
        for node in ast.walk(stmt):
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                return True
    return False


def _scan_strict_module_fallback_patterns(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    try:
        tree = ast.parse(text, filename=str(path))
    except SyntaxError:
        return []

    errors: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        name = _call_name(node.func)
        if name in STRICT_OPTIONAL_INTRINSIC_CALL_NAMES:
            errors.append(
                "Strict module cannot use optional intrinsic loaders; use require_intrinsic."
            )
            break

    for node in ast.walk(tree):
        if not isinstance(node, ast.Try):
            continue
        if not _stmt_block_has_import(node.body):
            continue
        for handler in node.handlers:
            names = _exception_type_names(handler.type)
            if names & STRICT_IMPORT_FALLBACK_EXCEPTIONS:
                errors.append(
                    "Strict module cannot use try/except import fallback paths."
                )
                break

    return errors


def _scan_intrinsic_runtime_fallback_patterns(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    try:
        tree = ast.parse(text, filename=str(path))
    except SyntaxError:
        return []

    intrinsic_callables: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, (ast.Assign, ast.AnnAssign)):
            continue
        value = node.value
        if not isinstance(value, ast.Call):
            continue
        call_name = _call_name(value.func)
        if call_name not in INTRINSIC_CALL_NAMES:
            continue
        if not value.args:
            continue
        first = value.args[0]
        if not (isinstance(first, ast.Constant) and isinstance(first.value, str)):
            continue
        if not first.value.startswith("molt_"):
            continue
        targets: list[ast.expr] = []
        if isinstance(node, ast.Assign):
            targets = list(node.targets)
        else:
            targets = [node.target]
        for target in targets:
            if isinstance(target, ast.Name):
                intrinsic_callables.add(target.id)

    def _stmt_block_has_intrinsic_call(statements: list[ast.stmt]) -> bool:
        for stmt in statements:
            for inner in ast.walk(stmt):
                if not isinstance(inner, ast.Call):
                    continue
                func = inner.func
                if isinstance(func, ast.Name):
                    if func.id in intrinsic_callables or func.id.startswith("_MOLT_"):
                        return True
                elif isinstance(func, ast.Attribute):
                    if func.attr.startswith("molt_"):
                        return True
        return False

    errors: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Try):
            continue
        if not node.handlers:
            continue
        if not _stmt_block_has_intrinsic_call(node.body):
            continue
        for handler in node.handlers:
            if not handler.body:
                continue
            if not all(isinstance(stmt, ast.Pass) for stmt in handler.body):
                continue
            errors.append(
                "Intrinsic call is wrapped in try/except with a pass-only fallback path."
            )
            break
    return errors


def _scan_host_fallback_module_patterns(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    try:
        tree = ast.parse(text, filename=str(path))
    except SyntaxError:
        return []

    def _is_forbidden_module_name(name: str) -> bool:
        return name.startswith("_py_") or "._py_" in name

    errors: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if _is_forbidden_module_name(alias.name):
                    errors.append(
                        "Host fallback imports (`_py_*`) are forbidden in stdlib modules."
                    )
                    return errors
        if isinstance(node, ast.ImportFrom):
            module_name = node.module
            if isinstance(module_name, str) and _is_forbidden_module_name(module_name):
                errors.append(
                    "Host fallback imports (`_py_*`) are forbidden in stdlib modules."
                )
                return errors
        if not isinstance(node, ast.Call):
            continue
        if not node.args:
            continue
        first = node.args[0]
        if not (isinstance(first, ast.Constant) and isinstance(first.value, str)):
            continue
        target = first.value
        if not _is_forbidden_module_name(target):
            continue
        func = node.func
        if isinstance(func, ast.Name) and func.id == "__import__":
            errors.append(
                "Dynamic host fallback imports (`__import__` on `_py_*`) are forbidden."
            )
            return errors
        if isinstance(func, ast.Attribute) and func.attr == "import_module":
            errors.append(
                "Dynamic host fallback imports (`import_module` on `_py_*`) are forbidden."
            )
            return errors
    return errors


def _load_intrinsic_partial_ratchet(path: Path) -> int:
    if not path.exists():
        raise RuntimeError(
            "intrinsic-partial ratchet file missing: " + _display_path(path)
        )
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:  # pragma: no cover - defensive parse guard
        raise RuntimeError(
            "intrinsic-partial ratchet file is not valid JSON: " + _display_path(path)
        ) from exc
    if not isinstance(payload, dict):
        raise RuntimeError("intrinsic-partial ratchet file must be a JSON object")
    raw = payload.get("max_intrinsic_partial")
    if not isinstance(raw, int) or raw < 0:
        raise RuntimeError(
            "intrinsic-partial ratchet file must define non-negative "
            "`max_intrinsic_partial`"
        )
    return raw


def _load_fully_covered_stdlib_modules(path: Path) -> frozenset[str]:
    if not path.exists():
        raise RuntimeError(
            f"stdlib full-coverage manifest missing: {_display_path(path)}"
        )
    namespace = runpy.run_path(str(path))
    raw = namespace.get("STDLIB_FULLY_COVERED_MODULES")
    if not isinstance(raw, tuple):
        raise RuntimeError(
            "stdlib full-coverage manifest is invalid: "
            "STDLIB_FULLY_COVERED_MODULES tuple missing"
        )
    out = frozenset(name for name in raw if isinstance(name, str))
    if "molt.stdlib" in out:
        raise RuntimeError("stdlib full-coverage manifest must not include molt.stdlib")
    return out


def _load_full_coverage_required_intrinsics(
    path: Path,
) -> dict[str, tuple[str, ...]]:
    if not path.exists():
        raise RuntimeError(
            f"stdlib full-coverage manifest missing: {_display_path(path)}"
        )
    namespace = runpy.run_path(str(path))
    raw = namespace.get("STDLIB_REQUIRED_INTRINSICS_BY_MODULE", {})
    if not isinstance(raw, dict):
        raise RuntimeError(
            "stdlib full-coverage manifest is invalid: "
            "STDLIB_REQUIRED_INTRINSICS_BY_MODULE dict missing"
        )

    out: dict[str, tuple[str, ...]] = {}
    for module_name, intrinsic_names in raw.items():
        if not isinstance(module_name, str):
            raise RuntimeError(
                "stdlib full-coverage manifest is invalid: "
                "STDLIB_REQUIRED_INTRINSICS_BY_MODULE keys must be strings"
            )
        if module_name == "molt.stdlib":
            raise RuntimeError(
                "stdlib full-coverage manifest must not include molt.stdlib "
                "in STDLIB_REQUIRED_INTRINSICS_BY_MODULE"
            )
        if not isinstance(intrinsic_names, tuple) or not all(
            isinstance(name, str) for name in intrinsic_names
        ):
            raise RuntimeError(
                "stdlib full-coverage manifest is invalid: "
                "STDLIB_REQUIRED_INTRINSICS_BY_MODULE values must be tuples[str, ...]"
            )
        if any(not name.startswith("molt_") for name in intrinsic_names):
            raise RuntimeError(
                "stdlib full-coverage manifest is invalid: "
                "required intrinsic names must start with `molt_`"
            )
        if PROBE_INTRINSIC in intrinsic_names:
            raise RuntimeError(
                "stdlib full-coverage manifest is invalid: "
                "`molt_stdlib_probe` cannot satisfy full coverage contracts"
            )
        out[module_name] = tuple(dict.fromkeys(intrinsic_names))
    return out


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
        tree = ast.parse(
            audit.path.read_text(encoding="utf-8"), filename=str(audit.path)
        )
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
            "- Strict roots: "
            + ", ".join(f"`{name}`" for name in BOOTSTRAP_STRICT_ROOTS),
            "- Gate rule: when strict roots are present, each strict root and its full transitive stdlib import closure must be `intrinsic-backed` (no `intrinsic-partial`, `probe-only`, or `python-only`).",
            "- Required modules: "
            + ", ".join(f"`{name}`" for name in sorted(BOOTSTRAP_MODULES)),
            "- Gate rule: required bootstrap modules that are present must be `intrinsic-backed`.",
            "",
            "## Critical Strict-Import Gate",
            "- Optional strict mode: `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`.",
            "- Critical roots: "
            + ", ".join(f"`{name}`" for name in CRITICAL_STRICT_IMPORT_ROOTS),
            "- Gate rule: for each listed root currently `intrinsic-backed`, every transitive stdlib import in its closure must also be `intrinsic-backed`.",
            "- Strict root rule: no optional intrinsic loaders and no try/except import fallback paths (applies to all listed roots, including `intrinsic-partial`).",
            "",
            "## Intrinsic-Backed Fallback Gate",
            "- Global rule: every `intrinsic-backed` module must avoid optional intrinsic loaders and try/except import fallback paths.",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`.",
            "",
            "## All-Stdlib Fallback Gate",
            "- Global rule: every stdlib module must avoid optional intrinsic loaders and try/except import fallback paths.",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## Intrinsic Pass-Fallback Gate",
            "- Rule: selected modules must not swallow intrinsic call failures via try/except pass-only fallback paths.",
            "- Enforced modules: "
            + ", ".join(f"`{name}`" for name in INTRINSIC_PASS_FALLBACK_STRICT_MODULES),
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## Zero Non-Intrinsic Gate",
            "- Global rule: stdlib classification must have zero `probe-only` modules and zero `python-only` modules.",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## Intrinsic-Partial Ratchet Gate",
            "- Global rule: `intrinsic-partial` count must be less than or equal to the ratchet budget and trend to zero.",
            "- Ratchet source: `tools/stdlib_intrinsics_ratchet.json` (`max_intrinsic_partial`).",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## Full-Coverage Attestation Rule",
            "- Global rule: any module/submodule not explicitly attested as full CPython 3.12+ API/PEP coverage is classified as `intrinsic-partial`.",
            "- Attestation source: `tools/stdlib_full_coverage_manifest.py` (`STDLIB_FULLY_COVERED_MODULES`).",
            "- Full-coverage intrinsic contract source: `tools/stdlib_full_coverage_manifest.py` (`STDLIB_REQUIRED_INTRINSICS_BY_MODULE`).",
            "- Gate rule: each attested full-coverage module must stay `intrinsic-backed`, declare its required intrinsic set, and wire every declared intrinsic in-module.",
            "- This rule applies to all stdlib modules and submodules.",
            "",
            "## CPython Top-Level Union Gate",
            "- Global rule: Molt must expose one top-level stdlib module or package for every CPython stdlib entry in the 3.12/3.13/3.14 union baseline.",
            "- Global rule: required package names must be implemented as packages (not single-file modules).",
            "- Global rule: do not provide both `name.py` and `name/__init__.py` for the same top-level entry.",
            "- Baseline source: `tools/stdlib_module_union.py` (regenerate with `python3 tools/gen_stdlib_module_union.py`).",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## CPython Submodule Union Gate",
            "- Global rule: Molt must expose one stdlib submodule/subpackage for every CPython stdlib `.py` module in the 3.12/3.13/3.14 union baseline.",
            "- Global rule: required subpackage names must be implemented as packages (`pkg/subpkg/__init__.py`), not single-file modules.",
            "- Global rule: do not provide both `pkg/name.py` and `pkg/name/__init__.py` for the same submodule entry.",
            "- Baseline source: `tools/stdlib_module_union.py` (regenerate with `python3 tools/gen_stdlib_module_union.py`).",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## TODO",
            "- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.",
            "",
        ]
    )
    return "\n".join(lines)


def _parse_module_list(raw: str) -> tuple[str, ...]:
    out = [item.strip() for item in raw.split(",") if item.strip()]
    if not out:
        raise ValueError("module list cannot be empty")
    return tuple(dict.fromkeys(out))


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
    parser.add_argument(
        "--allowlist-modules",
        help=(
            "Comma-separated stdlib modules to enforce strict transitive import closure "
            "(only roots currently intrinsic-backed are checked)."
        ),
    )
    parser.add_argument(
        "--critical-allowlist",
        action="store_true",
        help=(
            "Apply strict transitive import closure checks for critical modules: "
            + ", ".join(CRITICAL_STRICT_IMPORT_ROOTS)
            + " (and require those roots to be intrinsic-backed)."
        ),
    )
    parser.add_argument(
        "--fallback-intrinsic-backed-only",
        action="store_true",
        help=(
            "Limit fallback-pattern enforcement to intrinsic-backed modules "
            "(default enforces this gate across all stdlib modules)."
        ),
    )
    parser.add_argument(
        "--intrinsic-partial-ratchet-file",
        type=Path,
        default=INTRINSIC_PARTIAL_RATCHET,
        help=(
            "JSON file with `max_intrinsic_partial` budget. "
            "Default: tools/stdlib_intrinsics_ratchet.json."
        ),
    )
    parser.add_argument(
        "--full-coverage-manifest",
        type=Path,
        default=STDLIB_FULL_COVERAGE_MANIFEST,
        help=(
            "Python manifest declaring fully covered stdlib modules via "
            "STDLIB_FULLY_COVERED_MODULES tuple."
        ),
    )
    args = parser.parse_args()

    if not STDLIB_ROOT.is_dir():
        print(f"stdlib root missing: {STDLIB_ROOT}")
        return 1

    try:
        required_top_level, required_top_level_packages = (
            _load_required_top_level_stdlib()
        )
    except RuntimeError as exc:
        print(f"stdlib intrinsics lint failed: {exc}")
        return 1
    try:
        required_submodules, required_subpackages = _load_required_stdlib_submodules()
    except RuntimeError as exc:
        print(f"stdlib intrinsics lint failed: {exc}")
        return 1
    try:
        intrinsic_partial_budget = _load_intrinsic_partial_ratchet(
            args.intrinsic_partial_ratchet_file
        )
    except RuntimeError as exc:
        print(f"stdlib intrinsics lint failed: {exc}")
        return 1
    try:
        fully_covered_modules = _load_fully_covered_stdlib_modules(
            args.full_coverage_manifest
        )
    except RuntimeError as exc:
        print(f"stdlib intrinsics lint failed: {exc}")
        return 1
    try:
        full_coverage_required_intrinsics = _load_full_coverage_required_intrinsics(
            args.full_coverage_manifest
        )
    except RuntimeError as exc:
        print(f"stdlib intrinsics lint failed: {exc}")
        return 1

    manifest_intrinsics = _load_manifest_intrinsics()
    failures: list[tuple[Path, list[str]]] = []
    missing_intrinsics: list[tuple[str, str]] = []
    audits: list[ModuleAudit] = []
    top_level_files: dict[str, Path] = {}
    top_level_packages: dict[str, Path] = {}
    module_files: dict[str, Path] = {}
    module_packages: dict[str, Path] = {}

    for path in sorted(STDLIB_ROOT.rglob("*.py")):
        if path.name.startswith("."):
            continue
        errors, intrinsic_names, status, has_stdlib_todo = _scan_file(path)
        errors.extend(_scan_host_fallback_module_patterns(path))
        module = _module_name(path)
        if status == STATUS_INTRINSIC and has_stdlib_todo:
            status = STATUS_INTRINSIC_PARTIAL
        if status == STATUS_INTRINSIC and module not in fully_covered_modules:
            status = STATUS_INTRINSIC_PARTIAL
        if errors:
            failures.append((path, errors))
        for name in intrinsic_names:
            if name not in manifest_intrinsics:
                missing_intrinsics.append((_display_path(path), name))
        top_level = _top_level_entry(path)
        if top_level is not None:
            name, kind = top_level
            if kind == "module":
                top_level_files.setdefault(name, path)
            else:
                top_level_packages.setdefault(name, path)
        module_name, module_kind = _module_entry(path)
        if module_kind == "module":
            module_files.setdefault(module_name, path)
        else:
            module_packages.setdefault(module_name, path)
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
        if audit.module in BOOTSTRAP_MODULES and audit.status != STATUS_INTRINSIC
    ]
    modules_by_name = {audit.module: audit for audit in audits}
    dep_graph = _build_stdlib_dep_graph(modules_by_name)
    unknown_fully_covered_modules = tuple(
        sorted(fully_covered_modules - set(modules_by_name))
    )
    uncovered_contract_entries = tuple(
        sorted(set(full_coverage_required_intrinsics) - fully_covered_modules)
    )
    missing_full_coverage_contract_entries = tuple(
        sorted(fully_covered_modules - set(full_coverage_required_intrinsics))
    )
    full_coverage_status_violations = tuple(
        sorted(
            (
                module_name,
                modules_by_name[module_name].status,
            )
            for module_name in fully_covered_modules
            if module_name in modules_by_name
            and modules_by_name[module_name].status != STATUS_INTRINSIC
        )
    )
    full_coverage_unknown_intrinsics: list[tuple[str, tuple[str, ...]]] = []
    full_coverage_missing_intrinsic_wiring: list[tuple[str, tuple[str, ...]]] = []
    for module_name in sorted(fully_covered_modules):
        required = full_coverage_required_intrinsics.get(module_name)
        if required is None:
            continue
        unknown = tuple(
            sorted(name for name in required if name not in manifest_intrinsics)
        )
        if unknown:
            full_coverage_unknown_intrinsics.append((module_name, unknown))
            continue
        audit = modules_by_name.get(module_name)
        if audit is None:
            continue
        seen = set(audit.intrinsic_names)
        missing = tuple(sorted(name for name in required if name not in seen))
        if missing:
            full_coverage_missing_intrinsic_wiring.append((module_name, missing))
    bootstrap_roots_present = tuple(
        root for root in BOOTSTRAP_STRICT_ROOTS if root in modules_by_name
    )
    bootstrap_roots_missing = tuple(
        root for root in BOOTSTRAP_STRICT_ROOTS if root not in modules_by_name
    )
    bootstrap_closure_violations: list[tuple[str, str]] = []
    if bootstrap_roots_present and not bootstrap_roots_missing:
        bootstrap_closure = _closure(set(BOOTSTRAP_STRICT_ROOTS), dep_graph) | set(
            BOOTSTRAP_STRICT_ROOTS
        )
        for module_name in sorted(bootstrap_closure):
            audit = modules_by_name.get(module_name)
            if audit is None:
                continue
            if audit.status != STATUS_INTRINSIC:
                bootstrap_closure_violations.append((module_name, audit.status))
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

    strict_roots: set[str] = set()
    if args.allowlist_modules:
        try:
            strict_roots.update(_parse_module_list(args.allowlist_modules))
        except ValueError as exc:
            print(f"stdlib intrinsics lint failed: {exc}")
            return 1
    if args.critical_allowlist:
        strict_roots.update(CRITICAL_STRICT_IMPORT_ROOTS)
    unknown_strict_roots = sorted(
        root for root in strict_roots if root not in modules_by_name
    )
    if unknown_strict_roots:
        print("stdlib intrinsics lint failed: unknown strict-import modules requested")
        for module in unknown_strict_roots:
            print(f"- {module}")
        return 1
    strict_root_status_violations = [
        (root, modules_by_name[root].status)
        for root in sorted(strict_roots)
        if modules_by_name[root].status != STATUS_INTRINSIC
    ]
    if strict_root_status_violations:
        print(
            "stdlib intrinsics lint failed: strict-import roots must be intrinsic-backed"
        )
        for root, status in strict_root_status_violations:
            print(f"- {root}: {status}")
        return 1
    non_intrinsic_backed = {
        audit.module for audit in audits if audit.status != STATUS_INTRINSIC
    }
    strict_import_violations: list[tuple[str, tuple[str, ...]]] = []
    for root in sorted(strict_roots):
        imported_closure = _closure({root}, dep_graph)
        imported_closure.discard(root)
        bad = tuple(sorted(imported_closure & non_intrinsic_backed))
        if bad:
            strict_import_violations.append((root, bad))
    fallback_errors_by_module: dict[str, tuple[str, ...]] = {}
    for audit in sorted(audits, key=lambda item: item.module):
        errors = _scan_strict_module_fallback_patterns(audit.path)
        if errors:
            fallback_errors_by_module[audit.module] = tuple(errors)
    strict_fallback_violations = [
        (root, fallback_errors_by_module[root])
        for root in sorted(strict_roots)
        if root in fallback_errors_by_module
    ]
    intrinsic_backed_fallback_violations: list[tuple[str, tuple[str, ...]]] = []
    for audit in sorted(audits, key=lambda item: item.module):
        if audit.status != STATUS_INTRINSIC:
            continue
        errors = fallback_errors_by_module.get(audit.module)
        if errors:
            intrinsic_backed_fallback_violations.append((audit.module, errors))
    all_fallback_violations = sorted(fallback_errors_by_module.items())
    intrinsic_runtime_fallback_violations: list[tuple[str, tuple[str, ...]]] = []
    for audit in sorted(audits, key=lambda item: item.module):
        if audit.module not in INTRINSIC_PASS_FALLBACK_STRICT_MODULES:
            continue
        if audit.status not in {STATUS_INTRINSIC, STATUS_INTRINSIC_PARTIAL}:
            continue
        if audit.module.startswith(INTRINSIC_RUNTIME_FALLBACK_EXEMPT_PREFIXES):
            continue
        errors = _scan_intrinsic_runtime_fallback_patterns(audit.path)
        if errors:
            intrinsic_runtime_fallback_violations.append((audit.module, tuple(errors)))
    probe_only_modules = tuple(
        sorted(audit.module for audit in audits if audit.status == STATUS_PROBE_ONLY)
    )
    intrinsic_partial_modules = tuple(
        sorted(
            audit.module for audit in audits if audit.status == STATUS_INTRINSIC_PARTIAL
        )
    )
    python_only_modules_sorted = tuple(
        sorted(audit.module for audit in audits if audit.status == STATUS_PYTHON_ONLY)
    )
    top_level_collisions = tuple(
        sorted(set(top_level_files).intersection(top_level_packages))
    )
    submodule_collisions = tuple(
        sorted(
            name
            for name in set(module_files).intersection(module_packages)
            if "." in name
        )
    )
    present_top_level = set(top_level_files) | set(top_level_packages)
    missing_top_level = tuple(sorted(required_top_level - present_top_level))
    package_kind_mismatches = tuple(
        sorted(
            name
            for name in required_top_level_packages
            if name in top_level_files and name not in top_level_packages
        )
    )
    present_submodules = {
        name
        for name in set(module_files).union(module_packages)
        if "." in name and name != "molt.stdlib"
    }
    missing_submodules = tuple(sorted(required_submodules - present_submodules))
    subpackage_kind_mismatches = tuple(
        sorted(
            name
            for name in required_subpackages
            if name in module_files and name not in module_packages
        )
    )

    if failures:
        print("stdlib intrinsics lint failed:")
        for path, errors in failures:
            rel = _display_path(path)
            print(f"- {rel}")
            for msg in errors:
                print(f"  {msg}")
        return 1

    if missing_intrinsics:
        print("stdlib intrinsics lint failed: unknown intrinsic names")
        for rel, name in sorted(set(missing_intrinsics)):
            print(f"- {rel}: `{name}` is not present in {_display_path(MANIFEST)}")
        return 1

    if unknown_fully_covered_modules:
        print(
            "stdlib intrinsics lint failed: full-coverage attestation references unknown modules"
        )
        for module in unknown_fully_covered_modules:
            print(f"- {module}")
        return 1

    if uncovered_contract_entries:
        print(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract has non-attested modules"
        )
        for module in uncovered_contract_entries:
            print(f"- {module}")
        return 1

    if missing_full_coverage_contract_entries:
        print(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract missing modules"
        )
        for module in missing_full_coverage_contract_entries:
            print(f"- {module}")
        return 1

    if full_coverage_status_violations:
        print(
            "stdlib intrinsics lint failed: full-coverage modules must remain intrinsic-backed"
        )
        for module, status in full_coverage_status_violations:
            print(f"- {module}: {status}")
        return 1

    if full_coverage_unknown_intrinsics:
        print(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract references unknown intrinsics"
        )
        for module, names in full_coverage_unknown_intrinsics:
            joined = ", ".join(f"`{name}`" for name in names)
            print(f"- {module}: {joined}")
        return 1

    if full_coverage_missing_intrinsic_wiring:
        print(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract violated"
        )
        for module, names in full_coverage_missing_intrinsic_wiring:
            joined = ", ".join(f"`{name}`" for name in names)
            print(f"- {module}: missing {joined}")
        return 1

    if top_level_collisions:
        print(
            "stdlib intrinsics lint failed: top-level module/package duplicate mapping"
        )
        for name in top_level_collisions:
            print(
                f"- {name}: {_display_path(top_level_files[name])} and "
                f"{_display_path(top_level_packages[name])}"
            )
        return 1

    if missing_top_level:
        print("stdlib intrinsics lint failed: stdlib top-level coverage gate violated")
        print("- missing top-level modules/packages:")
        for name in missing_top_level:
            print(f"  - {name}")
        return 1

    if package_kind_mismatches:
        print("stdlib intrinsics lint failed: stdlib package kind gate violated")
        print("- required packages implemented as single-file modules:")
        for name in package_kind_mismatches:
            print(f"  - {name}: {_display_path(top_level_files[name])}")
        return 1

    if submodule_collisions:
        print("stdlib intrinsics lint failed: submodule/package duplicate mapping")
        for name in submodule_collisions:
            print(
                f"- {name}: {_display_path(module_files[name])} and "
                f"{_display_path(module_packages[name])}"
            )
        return 1

    if missing_submodules:
        print("stdlib intrinsics lint failed: stdlib submodule coverage gate violated")
        print("- missing submodules/packages:")
        for name in missing_submodules:
            print(f"  - {name}")
        return 1

    if subpackage_kind_mismatches:
        print("stdlib intrinsics lint failed: stdlib subpackage kind gate violated")
        print("- required subpackages implemented as single-file modules:")
        for name in subpackage_kind_mismatches:
            print(f"  - {name}: {_display_path(module_files[name])}")
        return 1

    if bootstrap_roots_present and bootstrap_roots_missing:
        print("stdlib intrinsics lint failed: bootstrap strict roots are incomplete")
        for module in bootstrap_roots_missing:
            print(f"- {module}")
        return 1

    if bootstrap_closure_violations:
        print(
            "stdlib intrinsics lint failed: bootstrap strict closure must be intrinsic-backed"
        )
        for module, status in bootstrap_closure_violations:
            print(f"- {module}: {status}")
        return 1

    if bootstrap_failures:
        print(
            "stdlib intrinsics lint failed: bootstrap modules must be intrinsic-backed"
        )
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

    if strict_import_violations:
        print(
            "stdlib intrinsics lint failed: strict-import allowlist violated "
            "(intrinsic-backed roots imported non-intrinsic-backed stdlib modules)"
        )
        for root, bad in strict_import_violations:
            joined = ", ".join(f"`{name}`" for name in bad)
            print(f"- {root} imports {joined}")
        return 1

    if strict_fallback_violations:
        print(
            "stdlib intrinsics lint failed: strict-import roots used forbidden fallback patterns"
        )
        for root, errors in strict_fallback_violations:
            print(f"- {root}")
            for msg in errors:
                print(f"  {msg}")
        return 1

    if intrinsic_backed_fallback_violations:
        print(
            "stdlib intrinsics lint failed: intrinsic-backed modules used forbidden "
            "fallback patterns"
        )
        for module, errors in intrinsic_backed_fallback_violations:
            print(f"- {module}")
            for msg in errors:
                print(f"  {msg}")
        return 1

    if not args.fallback_intrinsic_backed_only and all_fallback_violations:
        print("stdlib intrinsics lint failed: all-stdlib fallback gate violated")
        for module, errors in all_fallback_violations:
            print(f"- {module}")
            for msg in errors:
                print(f"  {msg}")
        return 1

    if intrinsic_runtime_fallback_violations:
        print("stdlib intrinsics lint failed: intrinsic runtime fallback gate violated")
        for module, errors in intrinsic_runtime_fallback_violations:
            print(f"- {module}")
            for msg in errors:
                print(f"  {msg}")
        return 1

    if len(intrinsic_partial_modules) > intrinsic_partial_budget:
        print("stdlib intrinsics lint failed: intrinsic-partial ratchet gate violated")
        print(
            f"- intrinsic-partial count: {len(intrinsic_partial_modules)} "
            f"(budget: {intrinsic_partial_budget})"
        )
        print("- lower intrinsic-partial count or tighten ratchet intentionally.")
        return 1

    if probe_only_modules or python_only_modules_sorted:
        print("stdlib intrinsics lint failed: zero non-intrinsic gate violated")
        if probe_only_modules:
            print("- probe-only modules:")
            for module in probe_only_modules:
                print(f"  - {module}")
        if python_only_modules_sorted:
            print("- python-only modules:")
            for module in python_only_modules_sorted:
                print(f"  - {module}")
        return 1

    generated_doc = _build_audit_doc(audits)
    if args.update_doc:
        AUDIT_DOC.write_text(generated_doc, encoding="utf-8")
    else:
        if not AUDIT_DOC.exists():
            print(f"stdlib intrinsic audit doc missing: {_display_path(AUDIT_DOC)}")
            return 1
        existing = AUDIT_DOC.read_text(encoding="utf-8")
        if existing != generated_doc:
            print(
                "stdlib intrinsic audit doc is out of date. "
                "Run: python3 tools/check_stdlib_intrinsics.py --update-doc"
            )
            return 1

    if args.json_out is not None:
        status_counts = {
            STATUS_INTRINSIC: 0,
            STATUS_INTRINSIC_PARTIAL: 0,
            STATUS_PROBE_ONLY: 0,
            STATUS_PYTHON_ONLY: 0,
        }
        for audit in audits:
            status_counts[audit.status] += 1
        report = {
            "status_counts": status_counts,
            "modules": [
                {
                    "module": audit.module,
                    "path": _display_path(audit.path),
                    "status": audit.status,
                    "intrinsics": list(audit.intrinsic_names),
                }
                for audit in sorted(audits, key=lambda a: a.module)
            ],
            "strict_import_violations": [
                {"module": root, "imports": list(bad)}
                for root, bad in strict_import_violations
            ],
            "strict_fallback_violations": [
                {"module": root, "errors": list(errors)}
                for root, errors in strict_fallback_violations
            ],
            "intrinsic_backed_fallback_violations": [
                {"module": module, "errors": list(errors)}
                for module, errors in intrinsic_backed_fallback_violations
            ],
            "all_fallback_violations": [
                {"module": module, "errors": list(errors)}
                for module, errors in all_fallback_violations
            ],
            "intrinsic_runtime_fallback_violations": [
                {"module": module, "errors": list(errors)}
                for module, errors in intrinsic_runtime_fallback_violations
            ],
            "intrinsic_pass_fallback_modules": list(
                INTRINSIC_PASS_FALLBACK_STRICT_MODULES
            ),
            "dependency_violations": [
                {"module": module, "status": status, "imports": list(bad)}
                for module, status, bad in dependency_violations
            ],
            "bootstrap_roots_present": list(bootstrap_roots_present),
            "bootstrap_roots_missing": list(bootstrap_roots_missing),
            "bootstrap_closure_violations": [
                {"module": module, "status": status}
                for module, status in bootstrap_closure_violations
            ],
            "required_top_level_modules": sorted(required_top_level),
            "required_top_level_packages": sorted(required_top_level_packages),
            "missing_top_level_modules": list(missing_top_level),
            "top_level_package_kind_mismatches": list(package_kind_mismatches),
            "top_level_collisions": list(top_level_collisions),
            "required_submodules": sorted(required_submodules),
            "required_subpackages": sorted(required_subpackages),
            "missing_submodules": list(missing_submodules),
            "subpackage_kind_mismatches": list(subpackage_kind_mismatches),
            "submodule_collisions": list(submodule_collisions),
            "probe_only_modules": list(probe_only_modules),
            "intrinsic_partial_modules": list(intrinsic_partial_modules),
            "intrinsic_partial_budget": intrinsic_partial_budget,
            "fully_covered_modules": sorted(fully_covered_modules),
            "full_coverage_required_intrinsics": {
                module: list(intrinsics)
                for module, intrinsics in sorted(
                    full_coverage_required_intrinsics.items()
                )
            },
            "python_only_modules": list(python_only_modules_sorted),
        }
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

    print("stdlib intrinsics lint: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
