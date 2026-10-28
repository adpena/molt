#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import functools
import json
import re
import runpy
import sys
from dataclasses import dataclass
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"


from molt.compiler_analysis.python_imports import (  # noqa: E402
    UnresolvedStaticImportError,
)
from molt.cli import module_graph_cache  # noqa: E402
from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction  # noqa: E402
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise  # noqa: E402
from molt.stdlib_intrinsic_policy import (  # noqa: E402
    INTRINSIC_CALL_NAMES,
    STATUS_INTRINSIC,
    STATUS_INTRINSIC_PARTIAL,
    STATUS_INTRINSIC_SUPPORT,
    STATUS_POLICY_GATE,
    STATUS_PYTHON_COMPILED,
    STATUS_STUB,
    classify_stdlib_module_statuses,
    stdlib_namespace_inventory,
    intrinsic_names_from_source,
)
from molt.stdlib_intrinsic_policy import StdlibIntrinsicClassification  # noqa: E402
from molt.stdlib_intrinsic_policy import StdlibModuleIntrinsicFacts  # noqa: E402
from molt.target_python_policy import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS  # noqa: E402
from molt.target_python import (  # noqa: E402
    _DEFAULT_TARGET_PYTHON_VERSION,
    TargetPythonVersion,
    _parse_target_python_version,
)

STDLIB_ROOT = ROOT / "src" / "molt" / "stdlib"
MOLTLIB_ROOT = ROOT / "src" / "moltlib"
MANIFEST = ROOT / "runtime" / "molt-runtime" / "src" / "intrinsics" / "manifest.pyi"
AUDIT_DOC = (
    ROOT
    / "docs"
    / "spec"
    / "areas"
    / "compat"
    / "surfaces"
    / "stdlib"
    / "stdlib_intrinsics_audit.generated.md"
)
STDLIB_UNION_BASELINE = ROOT / "tools" / "stdlib_module_union.py"
INTRINSIC_PARTIAL_RATCHET = ROOT / "tools" / "stdlib_intrinsics_ratchet.json"
STDLIB_FULL_COVERAGE_MANIFEST = ROOT / "tools" / "stdlib_full_coverage_manifest.py"

MANIFEST_INTRINSIC_RE = re.compile(r"^def\s+(molt_[a-zA-Z0-9_]+)\(")

STDLIB_PROGRESS_MARKER_RE = re.compile(
    r"(?:TODO|STDLIB_GAP)\(stdlib[^,]*,[^)]*status:(?:missing|partial|planned|divergent)\)"
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
    "re",
    "socket",
    "threading",
    "asyncio",
    "pathlib",
    "time",
    "traceback",
    "sys",
    "os",
)
# tinygrad.dflash was the sole policy-gate module here. It moved out of the
# stdlib substrate into demos/tinygrad/ (app/demo code, not a compiler
# primitive), so it is no longer discovered by this stdlib scan and no longer
# needs a policy-gate exemption.
ALLOWED_POLICY_GATE_MODULES: frozenset[str] = frozenset()


@dataclass(frozen=True)
class ModuleAudit:
    module: str
    path: Path
    # The intrinsics the module reads (classification ``used_intrinsics``).
    intrinsic_names: tuple[str, ...]
    status: str


def _is_implemented(status: str) -> bool:
    """Intrinsic-backed, intrinsic support, or compiled pure Python.

    A stub and a fail-closed policy gate implement nothing.
    """
    return status in {
        STATUS_INTRINSIC,
        STATUS_INTRINSIC_PARTIAL,
        STATUS_INTRINSIC_SUPPORT,
        STATUS_PYTHON_COMPILED,
    }


def _display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def _run_manifest(path: Path, label: str) -> dict[str, object]:
    try:
        return runpy.run_path(str(path))
    except (Exception, SystemExit) as exc:
        raise RuntimeError(
            f"{label} failed to evaluate ({_display_path(path)}): "
            f"{type(exc).__name__}: {exc}"
        ) from exc


def _load_required_top_level_stdlib() -> tuple[frozenset[str], frozenset[str]]:
    if not STDLIB_UNION_BASELINE.exists():
        raise RuntimeError(
            f"stdlib module baseline missing: {_display_path(STDLIB_UNION_BASELINE)}"
        )
    namespace = _run_manifest(STDLIB_UNION_BASELINE, "stdlib module baseline")
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
    namespace = _run_manifest(STDLIB_UNION_BASELINE, "stdlib module baseline")
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


def _module_name(path: Path) -> str:
    rel = path.relative_to(STDLIB_ROOT)
    if rel.name == "__init__.py":
        if len(rel.parts) == 1:
            return "molt.stdlib"
        return ".".join(rel.parts[:-1])
    return ".".join((*rel.parts[:-1], rel.stem))


RATCHET_FIELDS = ("max_intrinsic_partial", "max_stub")


def _load_ratchet(path: Path) -> dict[str, int]:
    """The status budgets: ``max_intrinsic_partial`` and ``max_stub``."""
    if not path.exists():
        raise RuntimeError("stdlib status ratchet file missing: " + _display_path(path))
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:  # pragma: no cover - defensive parse guard
        raise RuntimeError(
            "stdlib status ratchet file is not valid JSON: " + _display_path(path)
        ) from exc
    if not isinstance(payload, dict) or set(payload) != set(RATCHET_FIELDS):
        raise RuntimeError(
            "stdlib status ratchet file must be a JSON object with exactly "
            + ", ".join(f"`{field}`" for field in RATCHET_FIELDS)
        )
    budgets: dict[str, int] = {}
    for field in RATCHET_FIELDS:
        raw = payload[field]
        if type(raw) is not int or raw < 0:
            raise RuntimeError(
                f"stdlib status ratchet `{field}` must be a non-negative integer"
            )
        budgets[field] = raw
    return budgets


def _load_fully_covered_stdlib_modules(path: Path) -> frozenset[str]:
    if not path.exists():
        raise RuntimeError(
            f"stdlib full-coverage manifest missing: {_display_path(path)}"
        )
    namespace = _run_manifest(path, "stdlib full-coverage manifest")
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
    namespace = _run_manifest(path, "stdlib full-coverage manifest")
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
        if not isinstance(intrinsic_names, tuple):
            raise RuntimeError(
                "stdlib full-coverage manifest is invalid: "
                "STDLIB_REQUIRED_INTRINSICS_BY_MODULE values must be tuples[str, ...]"
            )
        validated: dict[str, None] = {}
        for name in intrinsic_names:
            if not isinstance(name, str):
                raise RuntimeError(
                    "stdlib full-coverage manifest is invalid: "
                    "required intrinsic names must be strings"
                )
            if not name.startswith("molt_"):
                raise RuntimeError(
                    "stdlib full-coverage manifest is invalid: "
                    "required intrinsic names must start with `molt_`"
                )
            validated[name] = None
        out[module_name] = tuple(validated)
    return out


def _is_compiler_import(node: ast.AST) -> bool:
    """Reject host-package imports, except normalized molt.stdlib names."""
    if isinstance(node, ast.Import):
        names = tuple(alias.name for alias in node.names)
    elif isinstance(node, ast.ImportFrom) and node.level == 0:
        names = (node.module or "",)
    else:
        return False
    return any(
        (name == "molt" or name.startswith("molt."))
        and name != "molt.stdlib"
        and not name.startswith("molt.stdlib.")
        for name in names
    )


def _scan_runtime_import_boundary(path: Path, text: str) -> list[str]:
    # Static compilation admits imports even under TYPE_CHECKING or in a lazy
    # function body. Walk the whole AST, not the runtime dependency projection.
    tree = ast.parse(text, filename=str(path))
    return [
        f"Line {node.lineno}: Importing the host molt compiler package in runtime "
        "libraries is forbidden; use _intrinsics or runtime-library modules "
        "(only molt.stdlib normalization is allowed)."
        for node in ast.walk(tree)
        if isinstance(node, (ast.Import, ast.ImportFrom)) and _is_compiler_import(node)
    ]


def _scan_file(path: Path) -> tuple[list[str], tuple[str, ...], bool]:
    """Source rule errors, required intrinsic names and the progress marker."""
    text = path.read_text(encoding="utf-8")
    errors = _scan_runtime_import_boundary(path, text)
    tree = ast.parse(text, filename=str(path))
    # Inspect resolver calls themselves, rather than trying to infer the names
    # passed through arbitrary Python wrappers. Aliases include local imports
    # during bootstrap (for example importlib.machinery's former `_require`).
    resolver_exports = {
        "require_intrinsic",
        "load_intrinsic",
        "require_optional_intrinsic",
    }
    resolvers = {"_require_intrinsic", *resolver_exports}
    resolver_modules: set[str] = set()
    # A local function may validate an already resolved callable under this
    # name (runpy/multiprocessing), or implement the resolver itself. Neither
    # is an imported intrinsic lookup; inspect its actual loader calls below.
    resolvers.difference_update(
        node.name
        for node in ast.walk(tree)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    )
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            resolver_modules.update(
                alias.asname or alias.name
                for alias in node.names
                if alias.name == "_intrinsics"
            )
        if isinstance(node, ast.ImportFrom) and node.module in {
            "_intrinsics",
            "molt.stdlib._intrinsics",
        }:
            resolvers.update(
                alias.asname or alias.name
                for alias in node.names
                if alias.name in resolver_exports
            )
    for node in ast.walk(tree):
        if not (
            isinstance(node, ast.Call)
            and (
                isinstance(node.func, ast.Name)
                and node.func.id in resolvers
                or isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Name)
                and node.func.value.id in resolver_modules
                and node.func.attr in resolver_exports
            )
        ):
            continue
        name = (
            node.args[0]
            if node.args
            else next(
                (keyword.value for keyword in node.keywords if keyword.arg == "name"),
                None,
            )
        )
        if not (
            isinstance(name, ast.Constant)
            and isinstance(name.value, str)
            and name.value.startswith("molt_")
        ):
            errors.append(
                f"Line {node.lineno}: Intrinsic reads must use a literal molt_* "
                "name directly; name wrappers hide reads from the stdlib audit."
            )
        elif (
            isinstance(node.func, ast.Name) and node.func.id not in INTRINSIC_CALL_NAMES
        ):
            errors.append(
                f"Line {node.lineno}: Use _require_intrinsic directly; this "
                "resolver alias hides the literal read from the stdlib audit."
            )
    intrinsic_names = tuple(sorted(intrinsic_names_from_source(text)))
    has_stdlib_progress_marker = bool(STDLIB_PROGRESS_MARKER_RE.search(text))
    return errors, intrinsic_names, has_stdlib_progress_marker


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
    intrinsic_support = sorted(
        a.module for a in audits if a.status == STATUS_INTRINSIC_SUPPORT
    )
    python_compiled = sorted(
        a.module for a in audits if a.status == STATUS_PYTHON_COMPILED
    )
    stubs = sorted(a.module for a in audits if a.status == STATUS_STUB)
    policy_gate = sorted(a.module for a in audits if a.status == STATUS_POLICY_GATE)
    status_by_module = {audit.module: audit.status for audit in audits}
    total_modules = len(audits)

    lines = [
        "# Stdlib Intrinsics Audit",
        "**Spec ID:** 0016",
        "**Status:** Draft (enforcement + audit)",
        "**Owner:** stdlib + runtime",
        "",
        "## Policy",
        "- Every stdlib module and submodule has exactly one status, derived from its own source and its proven imports by `src/molt/stdlib_intrinsic_policy.py`.",
        "- Intrinsic status rests on the intrinsics a module reads, not on the ones it requires. A module reads an intrinsic when it loads or exports the module-level name bound to it, requires it inside a function or class body, consumes it in an expression, or another stdlib module imports that private binding by name.",
        "- A requirement nothing reads is a gate failure, never backing: a discarded `_require_intrinsic(...)` statement or a private binding no module loads. Intrinsics that exist only to make a module count (readiness anchors) are not admitted.",
        "- Intrinsic resolver calls must name a literal `molt_*` intrinsic directly. Name wrappers are rejected because they hide reads from the audit; callable validation may wrap the resolved value.",
        "- `intrinsic-backed`: reads intrinsics and is attested as full CPython 3.12+ coverage. `intrinsic-partial`: reads intrinsics without that attestation, or carries a stdlib progress marker.",
        "- `intrinsic-support`: a private fragment its intrinsic owner imports, or a pure forwarding facade whose every resolved owner is intrinsic-backed. Forwarding cycles alone cannot establish support.",
        "- `python-compiled`: reads no intrinsic. Molt compiles the module's Python source into the binary like application code. This is an admitted implementation, not debt; a module moves to intrinsics when a measured win or a missing host capability requires it.",
        "- `stub`: the generated stand-in for a module Molt has not lowered (`tools/gen_stdlib_stubs.py`). Any attribute raises the canonical gap error. Stubs are debt under a ratchet.",
        "- `policy-gate`: a fail-closed namespace reservation whose only statement raises `ImportError`; it needs an explicit allowlist entry.",
        "- This classification is not runtime conformance evidence.",
        "- Each audit scan reports all failed gates. `--json-out` preserves diagnostics on failure and marks incomplete analysis explicitly; `--update-doc` publishes this document only after all gates pass.",
        "",
        "## Progress Summary (Generated)",
        f"- Total audited modules: `{total_modules}`",
        f"- `intrinsic-backed`: `{len(intrinsic)}`",
        f"- `intrinsic-partial`: `{len(intrinsic_partial)}`",
        f"- `intrinsic-support`: `{len(intrinsic_support)}`",
        f"- `python-compiled`: `{len(python_compiled)}`",
        f"- `stub`: `{len(stubs)}`",
        f"- `policy-gate`: `{len(policy_gate)}`",
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
    lines.extend(["", "### Intrinsic-owned support fragments and forwarding facades"])
    lines.extend(f"- `{name}`" for name in intrinsic_support)
    lines.extend(["", "### Compiled pure-Python modules"])
    lines.extend(f"- `{name}`" for name in python_compiled)
    lines.extend(["", "### Generated stubs (not lowered)"])
    lines.extend(f"- `{name}`" for name in stubs)
    lines.extend(["", "### Fail-closed policy-gate modules"])
    lines.extend(f"- `{name}`" for name in policy_gate)
    lines.extend(
        [
            "",
            "## Core Lane Source Inventory",
            "- Source lane: `tests/differential/basic/CORE_TESTS.txt`.",
            "- The inventory records exact stdlib import candidates and source status; candidate presence, absence and exception handling do not establish runtime success or API parity.",
            "- Unknown dynamic import custody remains explicit. Actual compiler admission, retained-IR requirements, linking and target execution own runtime qualification.",
            "- Enforced by: `python3 tools/check_core_lane_lowering.py`.",
            "",
            "## Bootstrap and Selected Source Roots",
            "- Bootstrap source roots: "
            + ", ".join(f"`{name}`" for name in BOOTSTRAP_STRICT_ROOTS),
            "- When bootstrap roots are present, all those roots must be present and implemented. Candidate dependency closure does not prove which imports must succeed.",
            "- Required bootstrap modules that are present must be implemented; a `stub` or `policy-gate` module is not bootstrap support.",
            "- `--critical-allowlist` and `--allowlist-modules` require implemented source roots, without inferring successful execution of every candidate dependency.",
            "",
            "## Standalone Execution and Python Errors",
            "- Admitted Python implementations may select compiled alternatives or catch Python exceptions. Their semantics and performance require independent qualification.",
            "- Retained intrinsic symbols must satisfy the compiler's target/profile and link requirements. Source catch syntax is not an availability or error-contract proof.",
            "",
            "## Unread Intrinsic Gate",
            "- Global rule: every intrinsic a stdlib module requires must be read (see Policy). A discarded requirement or a private binding no module loads fails the gate; delete it.",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (every mode).",
            "",
            "## Ratchet Gate",
            "- Global rule: the `intrinsic-partial` and `stub` counts must each stay at or below their ratchet budget and trend to zero.",
            "- Ratchet source: `tools/stdlib_intrinsics_ratchet.json` (`max_intrinsic_partial`, `max_stub`).",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (every mode).",
            "",
            "## Full-Coverage Attestation Rule",
            "- Global rule: any intrinsic-reading module/submodule not explicitly attested as full CPython 3.12+ API/PEP coverage is classified as `intrinsic-partial`.",
            "- `intrinsic-support` modules are owned implementation fragments or proven pure forwarding facades of intrinsic implementations; they are not full-coverage attestations.",
            "- Attestation source: `tools/stdlib_full_coverage_manifest.py` (`STDLIB_FULLY_COVERED_MODULES`).",
            "- Full-coverage intrinsic contract source: `tools/stdlib_full_coverage_manifest.py` (`STDLIB_REQUIRED_INTRINSICS_BY_MODULE`).",
            "- Gate rule: each attested full-coverage module must stay `intrinsic-backed`, `intrinsic-support` or `python-compiled` without a progress marker, declare the intrinsic set it reads (empty for a pure facade or `python-compiled`), and read every declared intrinsic.",
            "- This rule applies to all stdlib modules and submodules.",
            "",
            "## CPython Top-Level Union Gate",
            "- Global rule: Molt must expose one top-level stdlib module or package for every CPython stdlib entry in the 3.12/3.13/3.14 union baseline.",
            "- Global rule: required package names must be implemented as packages (not single-file modules).",
            "- Global rule: do not provide both `name.py` and `name/__init__.py` for the same top-level entry.",
            "- Baseline source: `tools/stdlib_module_union.py` (regenerate with `python3 tools/gen_stdlib_module_union.py --write`).",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## CPython Submodule Union Gate",
            "- Global rule: Molt must expose one stdlib submodule/subpackage for every CPython stdlib `.py` module in the 3.12/3.13/3.14 union baseline.",
            "- Global rule: required subpackage names must be implemented as packages (`pkg/subpkg/__init__.py`), not single-file modules.",
            "- Global rule: do not provide both `pkg/name.py` and `pkg/name/__init__.py` for the same submodule entry.",
            "- Baseline source: `tools/stdlib_module_union.py` (regenerate with `python3 tools/gen_stdlib_module_union.py --write`).",
            "- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).",
            "",
            "## Backlog Focus",
            "- Lower each `stub` module, and bring each `intrinsic-partial` module to attested full coverage; see the audit lists above.",
            "- Move a `python-compiled` module to intrinsics only for a measured performance win or a host capability Python cannot reach.",
            "",
        ]
    )
    return "\n".join(lines)


def _parse_module_list(raw: str) -> tuple[str, ...]:
    out = [item.strip() for item in raw.split(",") if item.strip()]
    if not out:
        raise ValueError("module list cannot be empty")
    return tuple(dict.fromkeys(out))


def _finish_audit(
    *,
    report: dict[str, object],
    diagnostics: dict[str, list[str]],
    json_out: Path | None,
    analysis_complete: bool = True,
) -> int:
    """Render one diagnostic authority, including failed or incomplete audits."""
    for messages in diagnostics.values():
        for message in messages:
            print(message)
    report.update(
        schema="molt.stdlib-intrinsics-audit.v4",
        analysis_complete=analysis_complete,
        ok=not diagnostics,
        diagnostics=[
            {"code": code, "messages": messages}
            for code, messages in diagnostics.items()
        ],
    )
    if json_out is not None:
        try:
            json_out.parent.mkdir(parents=True, exist_ok=True)
            json_out.write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
        except OSError as exc:
            print(f"stdlib intrinsics audit report publication failed: {exc}")
            return 1
    if not diagnostics:
        print("stdlib intrinsics lint: ok")
    return int(bool(diagnostics))


def main() -> int:
    parser = argparse.ArgumentParser(description="Stdlib intrinsics lint + audit gate")
    parser.add_argument(
        "--target-python",
        choices=SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS,
        default=_DEFAULT_TARGET_PYTHON_VERSION.short,
        help="Python language target for shared intrinsic dependency classification.",
    )
    parser.add_argument(
        "--update-doc",
        action="store_true",
        help=(
            "Rewrite docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md "
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
            "Comma-separated stdlib source roots to require as implemented "
            "(each root must be implemented)."
        ),
    )
    parser.add_argument(
        "--critical-allowlist",
        action="store_true",
        help=(
            "Require implemented source roots for critical modules: "
            + ", ".join(CRITICAL_STRICT_IMPORT_ROOTS)
            + " (and require those roots to be implemented)."
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

    try:
        return _run_audit(args)
    except (
        OSError,
        UnicodeError,
        SyntaxError,
        RuntimeError,
        UnresolvedStaticImportError,
    ) as exc:
        return _finish_audit(
            report={},
            diagnostics={
                "incomplete-analysis": [f"stdlib intrinsics lint failed: {exc}"]
            },
            json_out=args.json_out,
            analysis_complete=False,
        )


def _intrinsic_source_facts(
    module_name: str,
    path: Path,
    namespace_premise: PythonNamespacePremise,
    *,
    target_python: TargetPythonVersion,
) -> StdlibModuleIntrinsicFacts:
    """The compiler's content-keyed facts cache: builds and this gate share it."""
    return module_graph_cache._stdlib_intrinsic_source_facts(
        module_name,
        path,
        target_python=target_python,
        namespace_premise=namespace_premise,
    )


def _classify_stdlib_modules(
    module_paths: dict[str, Path], target_python: TargetPythonVersion
) -> StdlibIntrinsicClassification:
    with _source_tree_fingerprint_transaction():
        inventory = stdlib_namespace_inventory(
            module_paths,
            target_python=target_python,
            namespace_provider=functools.partial(
                module_graph_cache._stdlib_intrinsic_namespace_summary,
                target_python=target_python,
            ),
        )
        module_graph_cache.warm_stdlib_intrinsic_source_facts(
            module_paths,
            target_python=target_python,
            namespace_premises=inventory.premises,
        )
        return classify_stdlib_module_statuses(
            module_paths,
            target_python=target_python,
            facts_provider=functools.partial(
                _intrinsic_source_facts, target_python=target_python
            ),
            namespace_inventory=inventory,
        )


def _module_audits(
    classification: StdlibIntrinsicClassification,
    module_paths: dict[str, Path],
    *,
    progress_marker_modules: set[str],
    fully_covered_modules: frozenset[str],
) -> list[ModuleAudit]:
    """Final statuses: an intrinsic module is partial until fully attested."""
    audits: list[ModuleAudit] = []
    for module, path in module_paths.items():
        status = classification.statuses[module]
        if status == STATUS_INTRINSIC and (
            module in progress_marker_modules or module not in fully_covered_modules
        ):
            status = STATUS_INTRINSIC_PARTIAL
        audits.append(
            ModuleAudit(
                module=module,
                path=path,
                intrinsic_names=tuple(
                    sorted(classification.used_intrinsics.get(module, ()))
                ),
                status=status,
            )
        )
    return audits


def _stdlib_module_audits(
    target_python: TargetPythonVersion,
) -> tuple[StdlibIntrinsicClassification, list[ModuleAudit]]:
    """One source fact inventory shared by status and dependency consumers."""
    module_paths: dict[str, Path] = {}
    progress_marker_modules: set[str] = set()
    for path in sorted(STDLIB_ROOT.rglob("*.py")):
        if path.name.startswith("."):
            continue
        module = _module_name(path)
        module_paths[module] = path
        if STDLIB_PROGRESS_MARKER_RE.search(path.read_text(encoding="utf-8")):
            progress_marker_modules.add(module)
    classification = _classify_stdlib_modules(module_paths, target_python)
    audits = _module_audits(
        classification,
        module_paths,
        progress_marker_modules=progress_marker_modules,
        fully_covered_modules=_load_fully_covered_stdlib_modules(
            STDLIB_FULL_COVERAGE_MANIFEST
        ),
    )
    return classification, audits


def stdlib_module_statuses(
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
) -> dict[str, tuple[str, Path]]:
    """Every stdlib module's final audit status and path, for other gates."""
    _, audits = _stdlib_module_audits(target_python)
    return {audit.module: (audit.status, audit.path) for audit in audits}


def _run_audit(args: argparse.Namespace) -> int:
    if not STDLIB_ROOT.is_dir():
        raise RuntimeError(f"stdlib root missing: {STDLIB_ROOT}")
    required_top_level, required_top_level_packages = _load_required_top_level_stdlib()
    required_submodules, required_subpackages = _load_required_stdlib_submodules()
    budgets = _load_ratchet(args.intrinsic_partial_ratchet_file)
    fully_covered_modules = _load_fully_covered_stdlib_modules(
        args.full_coverage_manifest
    )
    full_coverage_required_intrinsics = _load_full_coverage_required_intrinsics(
        args.full_coverage_manifest
    )
    strict_roots: set[str] = set()
    if args.allowlist_modules:
        try:
            strict_roots.update(_parse_module_list(args.allowlist_modules))
        except ValueError as exc:
            raise RuntimeError(str(exc)) from exc
    if args.critical_allowlist:
        strict_roots.update(CRITICAL_STRICT_IMPORT_ROOTS)
    diagnostics: dict[str, list[str]] = {}

    manifest_intrinsics = _load_manifest_intrinsics()
    failures: list[tuple[Path, list[str]]] = []
    missing_intrinsics: list[tuple[str, str]] = []
    module_paths: dict[str, Path] = {}
    top_level_files: dict[str, Path] = {}
    top_level_packages: dict[str, Path] = {}
    module_files: dict[str, Path] = {}
    module_packages: dict[str, Path] = {}
    progress_marker_modules: set[str] = set()

    for path in sorted(STDLIB_ROOT.rglob("*.py")):
        if path.name.startswith("."):
            continue
        errors, intrinsic_names, has_stdlib_progress_marker = _scan_file(path)
        module = _module_name(path)
        if has_stdlib_progress_marker:
            progress_marker_modules.add(module)
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
        module_paths[module] = path

    # moltlib shares the runtime/compiler boundary, not the CPython stdlib
    # coverage universe, intrinsic status classification, or generated audit.
    for path in sorted(MOLTLIB_ROOT.rglob("*.py")):
        if path.name.startswith("."):
            continue
        errors = _scan_runtime_import_boundary(path, path.read_text(encoding="utf-8"))
        if errors:
            failures.append((path, errors))

    intrinsic_classification = _classify_stdlib_modules(
        module_paths, _parse_target_python_version(args.target_python)
    )
    unresolved_intrinsic_imports = intrinsic_classification.unresolved_imports_payload()
    if unresolved_intrinsic_imports:
        print("Intrinsic relationship evidence excludes unresolved import sites:")
        for site in unresolved_intrinsic_imports:
            print(
                f"- {site['module']}: {site['path']}:{site['line']} ({site['name']!r})"
            )
    audits = _module_audits(
        intrinsic_classification,
        module_paths,
        progress_marker_modules=progress_marker_modules,
        fully_covered_modules=fully_covered_modules,
    )

    bootstrap_failures = [
        audit.module
        for audit in audits
        if audit.module in BOOTSTRAP_MODULES and not _is_implemented(audit.status)
    ]
    modules_by_name = {audit.module: audit for audit in audits}
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
            and (
                modules_by_name[module_name].status
                not in {
                    STATUS_INTRINSIC,
                    STATUS_INTRINSIC_SUPPORT,
                    STATUS_PYTHON_COMPILED,
                }
                or module_name in progress_marker_modules
            )
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
    bootstrap_root_status_violations = [
        (name, modules_by_name[name].status)
        for name in bootstrap_roots_present
        if not _is_implemented(modules_by_name[name].status)
    ]
    stub_modules = {audit.module for audit in audits if audit.status == STATUS_STUB}

    unknown_strict_roots = sorted(
        root for root in strict_roots if root not in modules_by_name
    )
    if unknown_strict_roots:
        messages = diagnostics.setdefault("unknown-strict-roots", [])
        messages.append(
            "stdlib intrinsics lint failed: unknown strict-import modules requested"
        )
        for module in unknown_strict_roots:
            messages.append(f"- {module}")
    strict_roots.intersection_update(modules_by_name)
    strict_root_status_violations = [
        (root, modules_by_name[root].status)
        for root in sorted(strict_roots)
        if not _is_implemented(modules_by_name[root].status)
    ]
    if strict_root_status_violations:
        messages = diagnostics.setdefault("strict-root-status-violations", [])
        messages.append(
            "stdlib intrinsics lint failed: strict-import roots must be implemented"
        )
        for root, status in strict_root_status_violations:
            messages.append(f"- {root}: {status}")
    policy_gate_modules = tuple(
        sorted(audit.module for audit in audits if audit.status == STATUS_POLICY_GATE)
    )
    unknown_policy_gate_modules = tuple(
        module
        for module in policy_gate_modules
        if module not in ALLOWED_POLICY_GATE_MODULES
    )
    intrinsic_partial_modules = tuple(
        sorted(
            audit.module for audit in audits if audit.status == STATUS_INTRINSIC_PARTIAL
        )
    )
    stub_modules_sorted = tuple(sorted(stub_modules))
    python_compiled_modules = tuple(
        sorted(
            audit.module for audit in audits if audit.status == STATUS_PYTHON_COMPILED
        )
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
        messages = diagnostics.setdefault("failures", [])
        messages.append("stdlib intrinsics lint failed:")
        for path, errors in failures:
            rel = _display_path(path)
            messages.append(f"- {rel}")
            for msg in errors:
                messages.append(f"  {msg}")

    unread_bindings = intrinsic_classification.unused_bindings
    discarded_requirements = intrinsic_classification.discarded_requirements
    if unread_bindings or discarded_requirements:
        messages = diagnostics.setdefault("unread-intrinsics", [])
        messages.append(
            "stdlib intrinsics lint failed: required intrinsics nothing reads "
            "(delete each requirement; an intrinsic must be read to count)"
        )
        for module in sorted(set(unread_bindings) | set(discarded_requirements)):
            rel = _display_path(module_paths[module])
            for binding in unread_bindings.get(module, ()):
                messages.append(
                    f"- {rel}:{binding.line}: `{binding.name}` binds "
                    f"`{binding.intrinsic}` and no module reads it"
                )
            for name, line in discarded_requirements.get(module, ()):
                messages.append(f"- {rel}:{line}: discards `{name}`")

    if missing_intrinsics:
        messages = diagnostics.setdefault("missing-intrinsics", [])
        messages.append("stdlib intrinsics lint failed: unknown intrinsic names")
        for rel, name in sorted(set(missing_intrinsics)):
            messages.append(
                f"- {rel}: `{name}` is not present in {_display_path(MANIFEST)}"
            )

    if unknown_policy_gate_modules:
        messages = diagnostics.setdefault("unknown-policy-gate-modules", [])
        messages.append(
            "stdlib intrinsics lint failed: policy-gate modules require explicit allowlist"
        )
        for module in unknown_policy_gate_modules:
            messages.append(f"- {module}")

    if unknown_fully_covered_modules:
        messages = diagnostics.setdefault("unknown-fully-covered-modules", [])
        messages.append(
            "stdlib intrinsics lint failed: full-coverage attestation references unknown modules"
        )
        for module in unknown_fully_covered_modules:
            messages.append(f"- {module}")

    if uncovered_contract_entries:
        messages = diagnostics.setdefault("uncovered-contract-entries", [])
        messages.append(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract has non-attested modules"
        )
        for module in uncovered_contract_entries:
            messages.append(f"- {module}")

    if missing_full_coverage_contract_entries:
        messages = diagnostics.setdefault("missing-full-coverage-contract-entries", [])
        messages.append(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract missing modules"
        )
        for module in missing_full_coverage_contract_entries:
            messages.append(f"- {module}")

    if full_coverage_status_violations:
        messages = diagnostics.setdefault("full-coverage-status-violations", [])
        messages.append(
            "stdlib intrinsics lint failed: full-coverage modules must stay "
            "implemented (intrinsic-backed, intrinsic-support or python-compiled) "
            "without a progress marker"
        )
        for module, status in full_coverage_status_violations:
            messages.append(f"- {module}: {status}")

    if full_coverage_unknown_intrinsics:
        messages = diagnostics.setdefault("full-coverage-unknown-intrinsics", [])
        messages.append(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract references unknown intrinsics"
        )
        for module, names in full_coverage_unknown_intrinsics:
            joined = ", ".join(f"`{name}`" for name in names)
            messages.append(f"- {module}: {joined}")

    if full_coverage_missing_intrinsic_wiring:
        messages = diagnostics.setdefault("full-coverage-missing-intrinsic-wiring", [])
        messages.append(
            "stdlib intrinsics lint failed: full-coverage intrinsic contract violated"
        )
        for module, names in full_coverage_missing_intrinsic_wiring:
            joined = ", ".join(f"`{name}`" for name in names)
            messages.append(f"- {module}: missing {joined}")

    if top_level_collisions:
        messages = diagnostics.setdefault("top-level-collisions", [])
        messages.append(
            "stdlib intrinsics lint failed: top-level module/package duplicate mapping"
        )
        for name in top_level_collisions:
            messages.append(
                f"- {name}: {_display_path(top_level_files[name])} and "
                f"{_display_path(top_level_packages[name])}"
            )

    if missing_top_level:
        messages = diagnostics.setdefault("missing-top-level", [])
        messages.append(
            "stdlib intrinsics lint failed: stdlib top-level coverage gate violated"
        )
        messages.append("- missing top-level modules/packages:")
        for name in missing_top_level:
            messages.append(f"  - {name}")

    if package_kind_mismatches:
        messages = diagnostics.setdefault("package-kind-mismatches", [])
        messages.append(
            "stdlib intrinsics lint failed: stdlib package kind gate violated"
        )
        messages.append("- required packages implemented as single-file modules:")
        for name in package_kind_mismatches:
            messages.append(f"  - {name}: {_display_path(top_level_files[name])}")

    if submodule_collisions:
        messages = diagnostics.setdefault("submodule-collisions", [])
        messages.append(
            "stdlib intrinsics lint failed: submodule/package duplicate mapping"
        )
        for name in submodule_collisions:
            messages.append(
                f"- {name}: {_display_path(module_files[name])} and "
                f"{_display_path(module_packages[name])}"
            )

    if missing_submodules:
        messages = diagnostics.setdefault("missing-submodules", [])
        messages.append(
            "stdlib intrinsics lint failed: stdlib submodule coverage gate violated"
        )
        messages.append("- missing submodules/packages:")
        for name in missing_submodules:
            messages.append(f"  - {name}")

    if subpackage_kind_mismatches:
        messages = diagnostics.setdefault("subpackage-kind-mismatches", [])
        messages.append(
            "stdlib intrinsics lint failed: stdlib subpackage kind gate violated"
        )
        messages.append("- required subpackages implemented as single-file modules:")
        for name in subpackage_kind_mismatches:
            messages.append(f"  - {name}: {_display_path(module_files[name])}")

    if bootstrap_roots_present and bootstrap_roots_missing:
        messages = diagnostics.setdefault("incomplete-bootstrap-roots", [])
        messages.append(
            "stdlib intrinsics lint failed: bootstrap strict roots are incomplete"
        )
        for module in bootstrap_roots_missing:
            messages.append(f"- {module}")

    if bootstrap_root_status_violations:
        messages = diagnostics.setdefault("bootstrap-root-status-violations", [])
        messages.append(
            "stdlib intrinsics lint failed: bootstrap source roots must be implemented"
        )
        for module, status in bootstrap_root_status_violations:
            messages.append(f"- {module}: {status}")

    if bootstrap_failures:
        messages = diagnostics.setdefault("bootstrap-failures", [])
        messages.append(
            "stdlib intrinsics lint failed: bootstrap modules must be implemented"
        )
        for module in sorted(set(bootstrap_failures)):
            messages.append(f"- {module}")

    for status, field, count in (
        (
            STATUS_INTRINSIC_PARTIAL,
            "max_intrinsic_partial",
            len(intrinsic_partial_modules),
        ),
        (STATUS_STUB, "max_stub", len(stub_modules_sorted)),
    ):
        if count > budgets[field]:
            messages = diagnostics.setdefault(f"{status}-budget", [])
            messages.append(
                f"stdlib intrinsics lint failed: {status} ratchet gate violated"
            )
            messages.append(f"- {status} count: {count} (budget: {budgets[field]})")
            messages.append(
                f"- lower the {status} count or raise `{field}` intentionally."
            )

    generated_doc = _build_audit_doc(audits)
    try:
        if args.update_doc:
            if not diagnostics:
                AUDIT_DOC.write_text(generated_doc, encoding="utf-8")
        elif not AUDIT_DOC.exists():
            diagnostics["audit-document-missing"] = [
                f"stdlib intrinsic audit doc missing: {_display_path(AUDIT_DOC)}"
            ]
        elif AUDIT_DOC.read_text(encoding="utf-8") != generated_doc:
            diagnostics["audit-document-stale"] = [
                "stdlib intrinsic audit doc is out of date. "
                "Run: python3 tools/check_stdlib_intrinsics.py --update-doc"
            ]
    except OSError as exc:
        diagnostics["audit-document-publication"] = [
            f"stdlib intrinsic audit document publication failed: {exc}"
        ]

    status_counts = {
        STATUS_INTRINSIC: 0,
        STATUS_INTRINSIC_PARTIAL: 0,
        STATUS_INTRINSIC_SUPPORT: 0,
        STATUS_PYTHON_COMPILED: 0,
        STATUS_STUB: 0,
        STATUS_POLICY_GATE: 0,
    }
    for audit in audits:
        status_counts[audit.status] += 1
    report = {
        "file_failures": [
            {"path": _display_path(path), "errors": errors} for path, errors in failures
        ],
        "missing_intrinsics": [
            {"path": path, "intrinsic": name}
            for path, name in sorted(set(missing_intrinsics))
        ],
        "bootstrap_failures": sorted(set(bootstrap_failures)),
        "unknown_strict_roots": unknown_strict_roots,
        "strict_root_status_violations": [
            {"module": module, "status": status}
            for module, status in strict_root_status_violations
        ],
        "unknown_fully_covered_modules": list(unknown_fully_covered_modules),
        "uncovered_contract_entries": list(uncovered_contract_entries),
        "missing_full_coverage_contract_entries": list(
            missing_full_coverage_contract_entries
        ),
        "full_coverage_status_violations": [
            {"module": module, "status": status}
            for module, status in full_coverage_status_violations
        ],
        "full_coverage_unknown_intrinsics": [
            {"module": module, "intrinsics": list(names)}
            for module, names in full_coverage_unknown_intrinsics
        ],
        "full_coverage_missing_intrinsic_wiring": [
            {"module": module, "intrinsics": list(names)}
            for module, names in full_coverage_missing_intrinsic_wiring
        ],
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
        "unresolved_intrinsic_imports": unresolved_intrinsic_imports,
        "facades": intrinsic_classification.facades_payload(),
        "allowed_policy_gate_modules": sorted(ALLOWED_POLICY_GATE_MODULES),
        "unknown_policy_gate_modules": list(unknown_policy_gate_modules),
        "bootstrap_roots_present": list(bootstrap_roots_present),
        "bootstrap_roots_missing": list(bootstrap_roots_missing),
        "bootstrap_root_status_violations": [
            {"module": module, "status": status}
            for module, status in bootstrap_root_status_violations
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
        "intrinsic_partial_modules": list(intrinsic_partial_modules),
        "intrinsic_support_modules": [
            audit.module
            for audit in sorted(audits, key=lambda item: item.module)
            if audit.status == STATUS_INTRINSIC_SUPPORT
        ],
        "ratchet_budgets": budgets,
        "fully_covered_modules": sorted(fully_covered_modules),
        "full_coverage_required_intrinsics": {
            module: list(intrinsics)
            for module, intrinsics in sorted(full_coverage_required_intrinsics.items())
        },
        "python_compiled_modules": list(python_compiled_modules),
        "stub_modules": list(stub_modules_sorted),
        "unread_intrinsic_bindings": [
            {
                "module": module,
                "path": _display_path(module_paths[module]),
                "line": binding.line,
                "binding": binding.name,
                "intrinsic": binding.intrinsic,
            }
            for module, bindings in sorted(
                intrinsic_classification.unused_bindings.items()
            )
            for binding in bindings
        ],
        "discarded_intrinsic_requirements": [
            {
                "module": module,
                "path": _display_path(module_paths[module]),
                "line": line,
                "intrinsic": name,
            }
            for module, requirements in sorted(
                intrinsic_classification.discarded_requirements.items()
            )
            for name, line in requirements
        ],
    }
    return _finish_audit(report=report, diagnostics=diagnostics, json_out=args.json_out)


if __name__ == "__main__":
    raise SystemExit(main())
