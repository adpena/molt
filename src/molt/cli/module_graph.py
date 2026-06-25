from __future__ import annotations

import ast
from collections import deque
import contextlib
import functools
import os
import re
from collections.abc import Collection, Iterable, Mapping, MutableMapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol
from molt.cli.atomic_io import _write_text_if_changed
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.config_resolution import STATIC_IMPORT_MODULES_ENV
from molt.cli import module_graph_cache as _module_graph_cache
from molt.cli import module_import_scanner as _module_import_scanner
from molt.cli import module_resolution as _module_resolution
from molt.cli import module_source as _module_source
from molt.cli.models import (
    ImportScanMode,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ImportAdmissionPolicy,
    _ImportPlan,
    _ModuleGraphAugmentation,
    _ModuleGraphMetadata,
    _PreparedEntryModuleGraph,
    _SupportModuleAugmentation,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.runtime_features import _runtime_builtin_features_for_profile
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)
STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}


STUB_PARENT_MODULES = {"molt"}


# Submodule prefixes excluded from the module graph because they target
# platforms that Molt does not support (e.g. Emscripten/Pyodide).  The import
# scanner still discovers them but the graph walker skips any candidate whose
# dotted name starts with one of these prefixes.
PLATFORM_EXCLUDED_SUBMODULES = ("urllib3.contrib.emscripten",)


ENTRY_OVERRIDE_SPAWN = "multiprocessing.spawn"


@functools.lru_cache(maxsize=4096)
def _expand_module_chain_cached(name: str) -> tuple[str, ...]:
    name = name.strip()
    if not name:
        return ()
    parts = name.split(".")
    if any(not part or not part.isidentifier() for part in parts):
        return ()
    return tuple(".".join(parts[:idx]) for idx in range(1, len(parts) + 1))


def _expand_module_chain(name: str) -> list[str]:
    return list(_expand_module_chain_cached(name))


def _module_dependencies_from_imports(
    module_name: str,
    module_graph: Mapping[str, Path],
    imports: Iterable[str],
) -> set[str]:
    deps: set[str] = set()
    for name in imports:
        for candidate in _expand_module_chain_cached(name):
            if candidate == "molt" and module_name.startswith("molt."):
                continue
            if candidate in module_graph and candidate != module_name:
                deps.add(candidate)
            if candidate.startswith("molt.stdlib."):
                stdlib_candidate = candidate[len("molt.stdlib.") :]
                if stdlib_candidate in module_graph and stdlib_candidate != module_name:
                    deps.add(stdlib_candidate)
    return deps


def _module_dependencies(
    tree: ast.AST,
    module_name: str,
    module_graph: dict[str, Path],
    *,
    imports: list[str] | None = None,
) -> set[str]:
    path = module_graph.get(module_name)
    is_package = path is not None and path.name == "__init__.py"
    collected_imports = (
        imports
        if imports is not None
        else _module_import_scanner._collect_imports(tree, module_name, is_package)
    )
    return _module_dependencies_from_imports(
        module_name,
        module_graph,
        collected_imports,
    )


def _module_dependency_layers(
    module_order: list[str],
    module_deps: dict[str, set[str]],
) -> list[list[str]]:
    if not module_order:
        return []
    depth_by_module: dict[str, int] = {}
    for name in module_order:
        deps = [
            dep
            for dep in module_deps.get(name, set())
            if dep in depth_by_module and dep != name
        ]
        if not deps:
            depth_by_module[name] = 0
            continue
        depth_by_module[name] = max(depth_by_module[dep] for dep in deps) + 1
    grouped: dict[int, list[str]] = {}
    for name in module_order:
        grouped.setdefault(depth_by_module.get(name, 0), []).append(name)
    return [grouped[level] for level in sorted(grouped)]


def _module_order_has_back_edges(
    module_order: list[str],
    module_deps: dict[str, set[str]],
) -> bool:
    seen: set[str] = set()
    module_set = set(module_order)
    for name in module_order:
        deps = {dep for dep in module_deps.get(name, set()) if dep in module_set}
        if not deps.issubset(seen):
            return True
        seen.add(name)
    return False


def _topo_sort_modules(
    module_graph: dict[str, Path], module_deps: dict[str, set[str]]
) -> list[str]:
    in_degree = {name: 0 for name in module_graph}
    dependents = _reverse_module_dependencies(module_deps, module_graph)
    for name, deps in module_deps.items():
        for dep in deps:
            in_degree[name] += 1
    ready = deque(sorted(name for name, degree in in_degree.items() if degree == 0))
    order: list[str] = []
    while ready:
        name = ready.popleft()
        order.append(name)
        for child in sorted(dependents[name]):
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    if len(order) != len(module_graph):
        remaining = sorted(name for name in module_graph if name not in order)
        order.extend(remaining)
    return order


def _analyze_module_schedule(
    module_graph: Mapping[str, Path],
    module_deps: Mapping[str, set[str]],
) -> tuple[
    list[str],
    dict[str, set[str]],
    bool,
    list[list[str]],
    dict[str, frozenset[str]],
]:
    module_names = set(module_graph)
    in_degree = {name: 0 for name in module_names}
    reverse_module_deps = _reverse_module_dependencies(dict(module_deps), module_names)
    for name, deps in module_deps.items():
        for dep in deps:
            if dep in module_names and name in in_degree:
                in_degree[name] += 1
    ready = deque(sorted(name for name, degree in in_degree.items() if degree == 0))
    order: list[str] = []
    while ready:
        name = ready.popleft()
        order.append(name)
        for child in sorted(reverse_module_deps.get(name, ())):
            if child not in in_degree:
                continue
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    has_back_edges = len(order) != len(module_names)
    if has_back_edges:
        remaining = sorted(name for name in module_names if name not in order)
        order.extend(remaining)
    layers = _module_dependency_layers(order, dict(module_deps))
    module_dep_closures = _module_dependency_closures(
        dict(module_deps),
        module_names,
        module_order=order,
        has_back_edges=has_back_edges,
    )
    return order, reverse_module_deps, has_back_edges, layers, module_dep_closures


def _reverse_module_dependencies(
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
) -> dict[str, set[str]]:
    dependents: dict[str, set[str]] = {name: set() for name in module_names}
    for name, deps in module_deps.items():
        if name not in dependents:
            dependents[name] = set()
        for dep in deps:
            dependents.setdefault(dep, set()).add(name)
    return dependents


def _dependent_module_closure(
    dirty_modules: Collection[str],
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    reverse_module_deps: Mapping[str, set[str]] | None = None,
) -> set[str]:
    dependents = (
        reverse_module_deps
        if reverse_module_deps is not None
        else _reverse_module_dependencies(module_deps, module_names)
    )
    closure: set[str] = {name for name in dirty_modules if name in dependents}
    queue = deque(sorted(closure))
    while queue:
        module_name = queue.popleft()
        for dependent in sorted(dependents.get(module_name, ())):
            if dependent not in closure:
                closure.add(dependent)
                queue.append(dependent)
    return closure


def _module_dependency_closure(
    module_name: str,
    module_deps: dict[str, set[str]],
) -> set[str]:
    closure: set[str] = {module_name}
    queue = deque([module_name])
    while queue:
        current = queue.popleft()
        for dep in sorted(module_deps.get(current, ())):
            if dep not in closure:
                closure.add(dep)
                queue.append(dep)
    return closure


_DEAD_MODULE_ELIMINATION_SAFELIST: frozenset[str] = frozenset(
    {
        "builtins",
        "sys",
        "os",
        "os.path",
        "_collections_abc",
        "abc",
        "io",
        "typing",
        "types",
        "functools",
        "collections",
        "collections.abc",
        "enum",
        "dataclasses",
        "warnings",
        "importlib",
        "importlib.util",
        "importlib.machinery",
        "importlib.abc",
        "_thread",
        "threading",
        "copyreg",
        "keyword",
        "operator",
        "reprlib",
        "itertools",
        _module_import_scanner.IMPORTER_MODULE_NAME,
        "molt.stdlib",
    }
)


def _compute_reachable_modules(
    entry_module: str,
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    *,
    extra_roots: Collection[str] = (),
) -> set[str]:
    reachable: set[str] = set()
    queue: deque[str] = deque()
    module_name_set = set(module_names)

    def _seed(name: str) -> None:
        if name in reachable:
            return
        reachable.add(name)
        queue.append(name)

    _seed(entry_module)
    for safe in _DEAD_MODULE_ELIMINATION_SAFELIST:
        if safe in module_name_set:
            _seed(safe)
    for root in extra_roots:
        if root in module_name_set:
            _seed(root)
    while queue:
        current = queue.popleft()
        for dep in module_deps.get(current, ()):
            _seed(dep)
    parents_to_add: set[str] = set()
    for name in list(reachable):
        parts = name.split(".")
        for i in range(1, len(parts)):
            parent = ".".join(parts[:i])
            if parent in module_name_set:
                parents_to_add.add(parent)
    reachable.update(parents_to_add)
    return reachable


def _apply_dead_module_elimination(
    module_order: list[str],
    module_layers: list[list[str]],
    entry_module: str,
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    *,
    extra_roots: Collection[str] = (),
) -> tuple[list[str], list[list[str]], int]:
    reachable = _compute_reachable_modules(
        entry_module,
        module_deps,
        module_names,
        extra_roots=extra_roots,
    )
    filtered_order = [m for m in module_order if m in reachable]
    filtered_layers = [[m for m in layer if m in reachable] for layer in module_layers]
    filtered_layers = [layer for layer in filtered_layers if layer]
    eliminated_count = len(module_order) - len(filtered_order)
    return filtered_order, filtered_layers, eliminated_count


def _module_dependency_closures(
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    *,
    module_order: Sequence[str] | None = None,
    has_back_edges: bool = False,
) -> dict[str, frozenset[str]]:
    if module_order is not None and not has_back_edges:
        closures: dict[str, frozenset[str]] = {}
        for module_name in tuple(module_order):
            closure: set[str] = {module_name}
            for dep in module_deps.get(module_name, ()):
                closure.update(closures.get(dep, frozenset({dep})))
            closures[module_name] = frozenset(closure)
        for module_name in module_names:
            closures.setdefault(module_name, frozenset({module_name}))
        return closures
    closures: dict[str, frozenset[str]] = {}
    for module_name in sorted(module_names):
        closures[module_name] = frozenset(
            _module_dependency_closure(module_name, module_deps)
        )
    return closures


@dataclass(frozen=True)
class ModuleSyntaxErrorInfo:
    message: str
    filename: str
    lineno: int | None
    offset: int | None
    text: str | None


def _write_importer_module(output_dir: Path) -> Path:
    lines = [
        '"""Auto-generated import dispatcher for Molt-compiled modules."""',
        "",
        "from __future__ import annotations",
        "",
    ]
    lines.extend(
        [
            "from _intrinsics import require_intrinsic as _require_intrinsic",
            "",
            "_IMPORT_TRANSACTION = _require_intrinsic(",
            "    'molt_importlib_import_transaction', globals()",
            ")",
            "",
            "def _molt_import(name, globals=None, locals=None, fromlist=(), level=0):",
            "    return _IMPORT_TRANSACTION(name, globals, locals, fromlist, level)",
        ]
    )
    path = output_dir / f"{_module_import_scanner.IMPORTER_MODULE_NAME}.py"
    _write_text_if_changed(path, "\n".join(lines) + "\n")
    return path


def _parse_static_import_modules(raw: str) -> tuple[frozenset[str], str | None]:
    modules: set[str] = set()
    for part in re.split(r"[\s,]+", raw.strip()):
        if not part:
            continue
        if not re.fullmatch(
            r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*",
            part,
        ):
            return frozenset(), (
                f"{STATIC_IMPORT_MODULES_ENV} must contain comma/space-separated "
                f"Python module names; invalid entry: {part!r}"
            )
        modules.add(part)
    return frozenset(modules), None


def _collect_namespace_parents(
    module_graph: Mapping[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: Collection[str] | None = None,
    *,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
) -> set[str]:
    namespace_parents: set[str] = set()
    resolution_cache = resolver_cache or _module_resolution._ModuleResolutionCache()

    def maybe_add(name: str) -> None:
        if name in module_graph:
            return
        if (
            resolution_cache.resolve_module(name, roots, stdlib_root, stdlib_allowlist)
            is not None
        ):
            return
        if resolution_cache.has_namespace_dir(
            name, roots, stdlib_root, stdlib_allowlist
        ):
            namespace_parents.add(name)

    for module_name in module_graph:
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            maybe_add(".".join(parts[:idx]))

    if explicit_imports:
        for name in explicit_imports:
            for candidate in _expand_module_chain_cached(name):
                maybe_add(candidate)
    return namespace_parents


def _namespace_paths(name: str, roots: list[Path]) -> list[str]:
    rel = Path(*name.split("."))
    paths: list[str] = []
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            paths.append(str(candidate))
    return list(dict.fromkeys(paths))


def _write_namespace_module(name: str, paths: list[str], output_dir: Path) -> Path:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", name.replace(".", "_")).strip("_")
    if not safe:
        safe = "root"
    stub_path = output_dir / f"namespace_{safe}.py"
    lines = [
        '"""Auto-generated namespace package stub for Molt."""',
        "",
        f"__package__ = {name!r}",
        f"__path__ = {paths!r}",
        "try:",
        "    spec = __spec__",
        "except NameError:",
        "    spec = None",
        "if spec is not None:",
        "    try:",
        "        spec.submodule_search_locations = list(__path__)",
        "    except Exception:",
        "        pass",
        "",
    ]
    stub_path.parent.mkdir(parents=True, exist_ok=True)
    _write_text_if_changed(stub_path, "\n".join(lines))
    return stub_path


def _logical_generated_module_path(module_name: str) -> str:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", module_name).strip("_")
    if not safe:
        safe = "module"
    return f"/__molt_generated__/{safe}.py"


def _collect_package_parents(
    module_graph: dict[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    *,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
) -> set[str]:
    resolution_cache = resolver_cache or _module_resolution._ModuleResolutionCache()
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    pending: set[str] = set()
    added: set[str] = set()
    for module_name in list(module_graph):
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            pending.add(".".join(parts[:idx]))

    while pending:
        parent = pending.pop()
        if parent in module_graph:
            continue
        resolved = resolution_cache.resolve_module(
            parent, roots, stdlib_root, stdlib_allowlist
        )
        if resolved is None or resolved.name != "__init__.py":
            continue
        if not import_admission_policy.admits_package_parent(
            parent,
            resolved,
            existing_modules=module_graph.keys(),
        ):
            continue
        module_graph[parent] = resolved
        added.add(parent)
        parent_parts = parent.split(".")
        for idx in range(1, len(parent_parts)):
            ancestor = ".".join(parent_parts[:idx])
            if ancestor not in module_graph:
                pending.add(ancestor)
    return added


def _build_module_lowering_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
) -> tuple[
    dict[str, str],
    dict[str, str | None],
    dict[str, bool],
    dict[str, bool],
]:
    logical_source_path_by_module: dict[str, str] = {}
    entry_override_by_module: dict[str, str | None] = {}
    module_is_namespace_by_module: dict[str, bool] = {}
    module_is_package_by_module: dict[str, bool] = {}
    namespace_modules = set(namespace_module_names)
    for module_name in sorted(module_graph):
        module_path = module_graph[module_name]
        logical_source_path_by_module[module_name] = generated_module_source_paths.get(
            module_name, str(module_path)
        )
        # Every lowered module needs to know the canonical entry module name.
        # The frontend uses `entry_module` together with `module_name` to
        # recognize the real entry module and emit __main__ cache/name
        # semantics for it. Passing `None` for the entry module itself causes
        # `__name__` to lower as the ordinary module name instead of "__main__".
        entry_override_by_module[module_name] = entry_module
        module_is_namespace_by_module[module_name] = module_name in namespace_modules
        module_is_package_by_module[module_name] = module_path.name == "__init__.py"
    return (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    )


@functools.lru_cache(maxsize=8)
def _stdlib_allowlist_cached(project_root_text: str | None) -> frozenset[str]:
    allowlist: set[str] = set()
    spec_path = Path("docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md")
    if not spec_path.exists():
        if project_root_text:
            spec_path = (
                Path(project_root_text)
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
        else:
            spec_path = (
                _compiler_root()
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
    if not spec_path.exists():
        return frozenset(allowlist)
    for line in spec_path.read_text().splitlines():
        if not line.startswith("|"):
            continue
        if line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return frozenset(allowlist)


def _stdlib_allowlist() -> set[str]:
    project_root = os.environ.get("MOLT_PROJECT_ROOT")
    return set(_stdlib_allowlist_cached(project_root))


_INTRINSIC_CALL_NAMES = {
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


_STDLIB_PROBE_INTRINSIC = "molt_stdlib_probe"


_STDLIB_POLICY_GATE_STATUS = "policy-gate"


def _is_fail_closed_import_policy_gate(text: str) -> bool:
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return False
    body = list(tree.body)
    if (
        body
        and isinstance(body[0], ast.Expr)
        and isinstance(body[0].value, ast.Constant)
        and isinstance(body[0].value.value, str)
    ):
        body = body[1:]
    while (
        body and isinstance(body[0], ast.ImportFrom) and body[0].module == "__future__"
    ):
        body = body[1:]
    if len(body) != 1 or not isinstance(body[0], ast.Raise):
        return False
    exc = body[0].exc
    if isinstance(exc, ast.Call):
        exc = exc.func
    if isinstance(exc, ast.Name):
        return exc.id == "ImportError"
    if isinstance(exc, ast.Attribute):
        return exc.attr == "ImportError"
    return False


def _module_required_intrinsic_names(path: Path) -> frozenset[str]:
    """Return every ``molt_*`` intrinsic a module statically requires.

    Walks the module source for ``require_intrinsic`` / ``load_intrinsic`` /
    ``_lazy_intrinsic`` style calls (see ``_INTRINSIC_CALL_NAMES``) and collects
    the string-literal first argument when it names an intrinsic. This is the
    same extraction used to classify a module's intrinsic status *and* to decide
    whether a feature-gated module is buildable on the selected profile, so the
    two views never disagree. Returns an empty set on read/parse failure (a
    module that cannot be parsed requires no intrinsics from this analysis).
    """
    try:
        source = path.read_text(encoding="utf-8")
    except Exception:
        return frozenset()
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return frozenset()

    intrinsic_names: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        call_name: str | None = None
        if isinstance(node.func, ast.Name):
            call_name = node.func.id
        elif isinstance(node.func, ast.Attribute):
            call_name = node.func.attr
        if call_name not in _INTRINSIC_CALL_NAMES and call_name != "_lazy_intrinsic":
            continue
        first: ast.expr | None = None
        if node.args:
            first = node.args[0]
        else:
            for keyword in node.keywords:
                if keyword.arg == "name":
                    first = keyword.value
                    break
        if not isinstance(first, ast.Constant) or not isinstance(first.value, str):
            continue
        name = first.value
        if name.startswith("molt_"):
            intrinsic_names.add(name)
    return frozenset(intrinsic_names)


def _stdlib_module_intrinsic_status(path: Path) -> str:
    if path.name == "_intrinsics.py":
        return "intrinsic-backed"
    try:
        text = path.read_text(encoding="utf-8")
    except Exception:
        return "python-only"

    intrinsic_names = _module_required_intrinsic_names(path)
    if not intrinsic_names:
        if _is_fail_closed_import_policy_gate(text):
            return _STDLIB_POLICY_GATE_STATUS
        return "python-only"
    if intrinsic_names == {_STDLIB_PROBE_INTRINSIC}:
        return "probe-only"
    return "intrinsic-backed"


def _enforce_intrinsic_stdlib(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    json_output: bool,
) -> int | None:
    missing: list[str] = []
    probe_only: list[str] = []
    stdlib_root = stdlib_root.resolve()
    for name, path in module_graph.items():
        if not path or not path.suffix == ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        status = _stdlib_module_intrinsic_status(path)
        if status == "python-only":
            missing.append(name)
        elif status == "probe-only":
            probe_only.append(name)
    if not missing:
        return None
    missing.sort()
    probe_only.sort()
    message = (
        "Intrinsic-only stdlib enforcement failed. These modules are Python-only "
        "and must be lowered to Rust intrinsics (or become thin intrinsic wrappers):\n"
        + "\n".join(f"  - {name}" for name in missing)
    )
    if probe_only:
        message += (
            "\n\nProbe-only modules in this build (thin wrappers + policy gate only):\n"
            + "\n".join(f"  - {name}" for name in probe_only)
        )
    return _fail(message, json_output, command="build")


def _profile_feature_gap_for_module(
    path: Path,
    enabled_features: frozenset[str],
) -> dict[str, list[str]]:
    """Map each excluded gating feature this module needs to its intrinsics.

    For every ``molt_*`` intrinsic *path* statically requires, resolve the
    Cargo feature whose absence would remove the runtime *symbol definition*
    from the linked archive (``None`` â‡’ always linked: an ungated symbol such as
    the deliberately-ungated ``molt_ssl_*`` ABI, OR a resolver-only feature
    whose ``#[unsafe(no_mangle)]`` definition is compiled unconditionally â€” see
    ``LINK_AFFECTING_FEATURES``). A link-affecting feature that is NOT in
    *enabled_features* would leave the intrinsic undefined at link. The result
    maps each such excluded feature to the sorted intrinsics that need it
    (empty â‡’ buildable on this profile).
    """
    gap: dict[str, set[str]] = {}
    for symbol in _module_required_intrinsic_names(path):
        feature = link_affecting_feature_gate_for_symbol(symbol)
        if feature is None or feature in enabled_features:
            continue
        gap.setdefault(feature, set()).add(symbol)
    return {feature: sorted(symbols) for feature, symbols in gap.items()}


def _enforce_profile_feature_availability(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    stdlib_profile: str | None,
    target: str,
    json_output: bool,
) -> int | None:
    """Refuse, loudly and at compile time, builds whose import graph needs a
    runtime feature the selected profile excludes.

    Domain-feature-gated stdlib modules (``ast`` â†’ ``stdlib_ast``, ``sqlite3``
    â†’ ``sqlite``, â€¦) call runtime intrinsics that are ``#[cfg(feature = ...)]``
    -gated. Feature selection is **profile-driven, not import-driven**
    (``_runtime_builtin_features_for_profile``): the native ``micro`` profile
    excludes the heavy domains to keep tiny binaries small. Without this gate,
    importing such a module on a profile that excludes its feature surfaces only
    as an opaque undefined-symbol **linker error** late in the build. This pass
    makes the loud-refusal doctrine executable: it detects the gap from the
    static import graph and refuses with an actionable remedy *before any link
    is attempted*.

    The enabled-feature set is computed with the exact function the runtime
    staticlib build uses, so the refusal can never disagree with what actually
    links.
    """
    # The wasm staticlib excludes a slightly different set on the micro profile;
    # derive the effective triple from the target the same way the build does so
    # the enabled-feature computation matches the linked runtime exactly.
    # `_runtime_builtin_features_for_profile` keys the wasm distinction on a
    # `wasm32`-prefixed triple, so map the symbolic wasm targets (and an explicit
    # wasm32 triple, if ever passed) to one.
    is_wasm = target in {"wasm", "wasm-freestanding"} or target.startswith("wasm32")
    effective_triple = "wasm32-wasip1" if is_wasm else None
    enabled_features = frozenset(
        _runtime_builtin_features_for_profile(
            stdlib_profile,
            target_triple=effective_triple,
        )
    )
    profile_name = stdlib_profile or "micro"
    stdlib_root = stdlib_root.resolve()

    # feature -> {module -> [intrinsics]} so one message can group every module
    # blocked by the same excluded feature.
    blocked: dict[str, dict[str, list[str]]] = {}
    for name, path in module_graph.items():
        if not path or path.suffix != ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        gap = _profile_feature_gap_for_module(path, enabled_features)
        for feature, symbols in gap.items():
            blocked.setdefault(feature, {})[name] = symbols
    if not blocked:
        return None

    lines: list[str] = []
    for feature in sorted(blocked):
        modules = blocked[feature]
        module_list = ", ".join(repr(m) for m in sorted(modules))
        plural = "module" if len(modules) == 1 else "modules"
        lines.append(f"  {feature}: required by {plural} {module_list}")
        for module_name in sorted(modules):
            sample = ", ".join(modules[module_name][:4])
            more = len(modules[module_name]) - 4
            if more > 0:
                sample += f", â€¦ (+{more} more)"
            lines.append(f"      {module_name} â†’ {sample}")

    excluded_features = sorted(blocked)
    feature_phrase = (
        f"the {excluded_features[0]!r} runtime feature"
        if len(excluded_features) == 1
        else "runtime features " + ", ".join(repr(f) for f in excluded_features)
    )
    message = (
        f"Profile '{profile_name}' excludes {feature_phrase} that this program's "
        f"import graph requires.\n"
        f"These statically-imported stdlib modules need a feature profile "
        f"'{profile_name}' does not build, so their runtime intrinsics would be "
        f"undefined at link:\n"
        + "\n".join(lines)
        + "\n\nFeature selection is profile-driven, not import-driven: the "
        "native 'micro' profile omits heavy domains (ast, crypto, "
        "compression, â€¦) to keep small binaries small.\n"
        "Rebuild with the full stdlib profile, which includes these features:\n"
        "    --stdlib-profile full\n"
        "or set the environment knob the build reads as its canonical profile:\n"
        "    MOLT_STDLIB_PROFILE=full"
    )
    return _fail(message, json_output, command="build")


# Core modules always included in the module graph.  The micro profile
# restricts this to the absolute minimum needed to run pure-computation
# benchmarks (builtins + sys).  Everything else is still available via lazy
# initialisation if user code actually imports it.
_CORE_STDLIB_MODULES_FULL = (
    "builtins",
    "sys",
    "types",
    "importlib",
    "importlib.util",
    "importlib.machinery",
)


_CORE_STDLIB_MODULES_MICRO = (
    "builtins",
    "sys",
)


def _ensure_core_stdlib_modules(
    module_graph: dict[str, Path], stdlib_root: Path
) -> None:
    stdlib_profile = os.environ.get("MOLT_STDLIB_PROFILE", "micro")
    if stdlib_profile == "micro":
        core_modules = _CORE_STDLIB_MODULES_MICRO
    else:
        core_modules = _CORE_STDLIB_MODULES_FULL
    for name in core_modules:
        path = _module_resolution._resolve_module_path(name, [stdlib_root])
        if path is not None:
            module_graph.setdefault(name, path)


def _record_module_reason(
    module_reasons: MutableMapping[str, set[str]],
    module_name: str,
    reason: str,
) -> None:
    module_reasons.setdefault(module_name, set()).add(reason)


def _extend_module_graph_with_closure(
    module_graph: MutableMapping[str, Path],
    *,
    entry_paths: Sequence[Path],
    roots: Sequence[Path],
    module_roots: Sequence[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    reason: str,
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> frozenset[str]:
    if not entry_paths:
        return frozenset()
    closure_graph, _ = _discover_module_graph_from_paths(
        entry_paths,
        list(roots),
        list(module_roots),
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=allow_entry_external_imports,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if diagnostics_enabled:
        for name, path in closure_graph.items():
            _record_module_reason(module_reasons, name, reason)
            module_graph.setdefault(name, path)
        return frozenset(closure_graph)
    for name, path in closure_graph.items():
        module_graph.setdefault(name, path)
    return frozenset(closure_graph)


def _resolve_static_import_module_paths(
    *,
    module_names: Collection[str],
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    import_admission_policy: _ImportAdmissionPolicy | None,
) -> tuple[dict[str, Path], list[str]]:
    resolved: dict[str, Path] = {}
    errors: list[str] = []
    for module_name in sorted(module_names):
        path = resolver_cache.resolve_module(
            module_name,
            list(roots),
            stdlib_root,
            stdlib_allowlist,
        )
        if path is None:
            errors.append(
                f"{STATIC_IMPORT_MODULES_ENV} module {module_name!r} was not found"
            )
            continue
        if import_admission_policy is not None and not (
            import_admission_policy.admits_import(
                module_name,
                path,
                from_entry_path=False,
            )
        ):
            errors.append(
                f"{STATIC_IMPORT_MODULES_ENV} module {module_name!r} resolves under "
                "an external root but is not within an admitted external static package"
            )
            continue
        resolved[module_name] = path
    return resolved, errors


def _extend_module_graph_with_static_import_modules(
    *,
    module_graph: MutableMapping[str, Path],
    explicit_imports: set[str],
    module_names: Collection[str],
    roots: Sequence[Path],
    module_roots: Sequence[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    import_admission_policy: _ImportAdmissionPolicy | None,
    target_python: TargetPythonVersion,
    capability_config_digest: str = "",
) -> list[str]:
    if not module_names:
        return []
    resolved, errors = _resolve_static_import_module_paths(
        module_names=module_names,
        roots=roots,
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
    )
    if errors:
        return errors
    explicit_imports.update(module_names)
    _extend_module_graph_with_closure(
        module_graph,
        entry_paths=tuple(resolved.values()),
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=resolver_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        reason="explicit_static_import",
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=False,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    return []


def _record_new_module_reasons(
    module_graph: Mapping[str, Path],
    before_names: set[str],
    module_reasons: MutableMapping[str, set[str]],
    reason: str,
) -> None:
    for name in module_graph:
        if name in before_names:
            continue
        _record_module_reason(module_reasons, name, reason)


def _looks_like_stdlib_module_name(module_name: str) -> bool:
    if module_name == "molt.stdlib" or module_name.startswith("molt.stdlib."):
        return True
    root = module_name.split(".", 1)[0]
    return root in {
        "__future__",
        "_collections_abc",
        "abc",
        "builtins",
        "collections",
        "dataclasses",
        "importlib",
        "os",
        "pathlib",
        "runpy",
        "signal",
        "sys",
        "test",
        "typing",
        "warnings",
        "zipfile",
        "zipimport",
    }


def _build_frontend_module_costs(
    module_names: Collection[str],
    *,
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _module_source._ModuleSourceCatalog | None = None,
    module_graph: Mapping[str, Path] | None = None,
    module_deps: Mapping[str, set[str]],
) -> dict[str, float]:
    module_costs: dict[str, float] = {}
    for module_name in sorted(module_names):
        source_size = 0
        if module_source_catalog is not None:
            source_size = module_source_catalog.source_size(
                module_name,
                module_graph.get(module_name) if module_graph is not None else None,
            )
        elif module_sources is not None:
            source_size = len(module_sources.get(module_name, ""))
        source_cost = max(1.0, float(source_size))
        dep_cost = float(max(0, len(module_deps.get(module_name, set()))) * 512)
        module_costs[module_name] = source_cost + dep_cost
    return module_costs


def _build_stdlib_like_module_flags(
    module_graph: Mapping[str, Path],
) -> dict[str, bool]:
    return {
        module_name: (
            _module_resolution._is_runtime_owned_module_path(module_path)
            or _looks_like_stdlib_module_name(module_name)
        )
        for module_name, module_path in sorted(module_graph.items())
    }


def _build_module_graph_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _module_source._ModuleSourceCatalog | None = None,
    module_deps: Mapping[str, set[str]] | None = None,
) -> _ModuleGraphMetadata:
    (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    ) = _build_module_lowering_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=namespace_module_names,
    )
    frontend_module_costs = None
    if module_deps is not None and (
        module_sources is not None or module_source_catalog is not None
    ):
        frontend_module_costs = _build_frontend_module_costs(
            module_graph,
            module_sources=module_sources,
            module_source_catalog=module_source_catalog,
            module_graph=module_graph,
            module_deps=module_deps,
        )
    stdlib_like_by_module = (
        _build_stdlib_like_module_flags(module_graph)
        if module_deps is not None
        else None
    )
    return _ModuleGraphMetadata(
        logical_source_path_by_module=MappingProxyType(logical_source_path_by_module),
        entry_override_by_module=MappingProxyType(entry_override_by_module),
        module_is_namespace_by_module=MappingProxyType(module_is_namespace_by_module),
        module_is_package_by_module=MappingProxyType(module_is_package_by_module),
        frontend_module_costs=(
            MappingProxyType(frontend_module_costs)
            if frontend_module_costs is not None
            else None
        ),
        stdlib_like_by_module=(
            MappingProxyType(stdlib_like_by_module)
            if stdlib_like_by_module is not None
            else None
        ),
    )


def _requires_spawn_entry_override(
    module_graph: Mapping[str, Path], explicit_imports: Collection[str]
) -> bool:
    names: set[str] = set(module_graph)
    names.update(explicit_imports)
    for name in names:
        if name == ENTRY_OVERRIDE_SPAWN or name.startswith("multiprocessing."):
            return True
        if name == "multiprocessing":
            return True
    return False


def _module_graph_import_scan_mode(
    *,
    path: Path,
    module_name: str,
    entry_paths: frozenset[Path],
    nested_scan_modules: Collection[str],
    resolution_cache: _module_resolution._ModuleResolutionCache,
) -> ImportScanMode:
    resolved_path = resolution_cache.resolved_path(path)
    if resolved_path in entry_paths or module_name in nested_scan_modules:
        return "full"
    return "module_init"


def _discover_module_graph_from_paths(
    entry_paths: Sequence[Path],
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_imports_by_path: Mapping[Path, Collection[str]] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[dict[str, Path], set[str]]:
    entry_paths = tuple(entry_paths)
    if not entry_paths:
        return {}, set()
    graph: dict[str, Path] = {}
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    nested_stdlib_scan_modules = (
        _module_import_scanner.STDLIB_NESTED_IMPORT_SCAN_MODULES
        if nested_stdlib_scan_modules is None
        else nested_stdlib_scan_modules
    )
    explicit_imports: set[str] = set()
    seen_import_names: set[str] = set()
    queue = list(reversed(entry_paths))
    queued_paths = set(entry_paths)
    resolution_cache = resolver_cache or _module_resolution._ModuleResolutionCache()
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    resolved_entry_paths = frozenset(
        resolution_cache.resolved_path(path) for path in entry_paths
    )

    persisted_graph_paths: dict[str, Path] = {}
    dirty_persisted_modules: set[str] = set()
    use_persisted_graph_cache = project_root is not None and len(entry_paths) == 1
    if use_persisted_graph_cache:
        cache_project_root = project_root
        assert cache_project_root is not None
        entry_path = entry_paths[0]
        persisted_graph = _module_graph_cache._read_persisted_module_graph(
            cache_project_root,
            entry_path,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            skip_modules=skip_modules,
            stub_parents=stub_parents,
            nested_stdlib_scan_modules=nested_stdlib_scan_modules,
            stdlib_allowlist=stdlib_allowlist,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=allow_entry_external_imports,
            resolution_cache=resolution_cache,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted_graph is not None:
            if not persisted_graph.dirty_modules:
                return persisted_graph.graph, persisted_graph.explicit_imports
            persisted_graph_paths = dict(persisted_graph.graph)
            dirty_persisted_modules = set(persisted_graph.dirty_modules)

    def resolve_candidate(candidate: str) -> Path | None:
        persisted_path = persisted_graph_paths.get(candidate)
        if persisted_path is not None and candidate not in dirty_persisted_modules:
            return persisted_path
        return resolution_cache.resolve_module(
            candidate, roots, stdlib_root, stdlib_allowlist
        )

    while queue:
        path = queue.pop()
        queued_paths.discard(path)
        module_name = resolution_cache.module_name_from_path(
            path, module_roots, stdlib_root
        )
        if module_name in graph:
            continue
        graph[module_name] = path
        is_package = path.name == "__init__.py"
        import_scan_mode = _module_graph_import_scan_mode(
            path=path,
            module_name=module_name,
            entry_paths=resolved_entry_paths,
            nested_scan_modules=nested_stdlib_scan_modules,
            resolution_cache=resolution_cache,
        )
        precomputed_imports = (
            precomputed_imports_by_path.get(path)
            if precomputed_imports_by_path is not None
            else None
        )
        if precomputed_imports is not None:
            imports = precomputed_imports
            if use_persisted_graph_cache:
                with contextlib.suppress(OSError):
                    _module_graph_cache._write_persisted_import_scan(
                        cache_project_root,
                        path,
                        module_name=module_name,
                        is_package=is_package,
                        import_scan_mode=import_scan_mode,
                        imports=imports,
                        target_python=target_python,
                        capability_config_digest=capability_config_digest,
                    )
        else:
            persisted_imports = None
            if project_root is not None:
                persisted_imports = _module_graph_cache._read_persisted_import_scan(
                    project_root,
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    import_scan_mode=import_scan_mode,
                    target_python=target_python,
                    capability_config_digest=capability_config_digest,
                )
            if persisted_imports is None:
                try:
                    source = resolution_cache.read_module_source(path)
                except (OSError, SyntaxError, UnicodeDecodeError):
                    continue
                try:
                    tree = resolution_cache.parse_module_ast(
                        path,
                        source,
                        filename=str(path),
                        target_python=target_python,
                    )
                except SyntaxError:
                    continue
                imports = _load_module_imports(
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    import_scan_mode=import_scan_mode,
                    tree=tree,
                    resolution_cache=resolution_cache,
                    project_root=project_root,
                    roots=roots,
                    stdlib_root=stdlib_root,
                    stdlib_allowlist=stdlib_allowlist,
                    target_python=target_python,
                    capability_config_digest=capability_config_digest,
                )
            else:
                imports = persisted_imports
        for name in imports:
            if name in seen_import_names:
                continue
            seen_import_names.add(name)
            explicit_imports.add(name)
            for candidate in _expand_module_chain_cached(name):
                if candidate in stub_parents:
                    continue
                if candidate.split(".", 1)[0] in skip_modules:
                    continue
                if any(
                    candidate == prefix or candidate.startswith(prefix + ".")
                    for prefix in PLATFORM_EXCLUDED_SUBMODULES
                ):
                    continue
                resolved = resolve_candidate(candidate)
                if resolved is None or resolved in queued_paths:
                    continue
                from_entry_path = (
                    allow_entry_external_imports
                    and resolution_cache.resolved_path(path) in resolved_entry_paths
                )
                if not import_admission_policy.admits_import(
                    candidate,
                    resolved,
                    from_entry_path=from_entry_path,
                ):
                    continue
                if resolved not in queued_paths:
                    queued_paths.add(resolved)
                    queue.append(resolved)
    if use_persisted_graph_cache:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_module_graph(
                cache_project_root,
                entry_paths[0],
                roots=roots,
                module_roots=module_roots,
                stdlib_root=stdlib_root,
                skip_modules=skip_modules,
                stub_parents=stub_parents,
                nested_stdlib_scan_modules=nested_stdlib_scan_modules,
                stdlib_allowlist=stdlib_allowlist,
                import_admission_policy=import_admission_policy,
                allow_entry_external_imports=allow_entry_external_imports,
                graph=graph,
                explicit_imports=explicit_imports,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return graph, explicit_imports


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    resolver_cache: _module_resolution._ModuleResolutionCache | None = None,
    precomputed_imports: Collection[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[dict[str, Path], set[str]]:
    precomputed_imports_by_path = (
        {entry_path: precomputed_imports} if precomputed_imports is not None else None
    )
    return _discover_module_graph_from_paths(
        (entry_path,),
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        resolver_cache=resolver_cache,
        precomputed_imports_by_path=precomputed_imports_by_path,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )


def _load_module_imports(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    tree: ast.AST,
    resolution_cache: _module_resolution._ModuleResolutionCache,
    project_root: Path | None,
    roots: Sequence[Path] | None = None,
    stdlib_root: Path | None = None,
    stdlib_allowlist: set[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[str, ...]:
    if project_root is not None:
        persisted_imports = _module_graph_cache._read_persisted_import_scan(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted_imports is not None:
            return persisted_imports
    imports = resolution_cache.collect_imports(
        path,
        tree,
        collector=_module_import_scanner._collect_imports,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
    )
    if roots is not None and stdlib_root is not None and stdlib_allowlist is not None:
        imports = _module_import_scanner._expand_imports_with_static_package_all_star_children(
            imports,
            tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolution_cache=resolution_cache,
            target_python=target_python,
        )
    if project_root is not None:
        with contextlib.suppress(OSError):
            _module_graph_cache._write_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                import_scan_mode=import_scan_mode,
                imports=imports,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return imports


def _augment_support_modules(
    *,
    module_graph: MutableMapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: Collection[str],
    resolver_cache: "_module_resolution._ModuleResolutionCache",
    artifacts_root: Path,
    stub_parents: Collection[str],
    entry_module: str,
    needs_generated_importer: bool,
    diagnostics_enabled: bool,
) -> _SupportModuleAugmentation:
    namespace_parents = _collect_namespace_parents(
        module_graph,
        roots,
        stdlib_root,
        stdlib_allowlist,
        explicit_imports,
        resolver_cache=resolver_cache,
    )
    namespace_modules: dict[str, Path] = {}
    if namespace_parents:
        for name in sorted(namespace_parents):
            paths = _namespace_paths(
                name,
                _module_resolution._roots_for_module(
                    name,
                    roots,
                    stdlib_root,
                    stdlib_allowlist,
                ),
            )
            if not paths:
                continue
            stub_path = _write_namespace_module(name, paths, artifacts_root)
            namespace_modules[name] = stub_path
        if namespace_modules:
            module_graph.update(namespace_modules)
            if diagnostics_enabled:
                for name in namespace_modules:
                    _record_module_reason(module_reasons, name, "namespace_stub")
    generated_module_source_paths: dict[str, str] = {
        name: _logical_generated_module_path(name) for name in namespace_modules
    }
    for stub in stub_parents:
        if stub != entry_module and stub in namespace_modules:
            module_graph.pop(stub, None)
    if (
        needs_generated_importer
        and _module_import_scanner.IMPORTER_MODULE_NAME not in module_graph
    ):
        importer_path = _write_importer_module(artifacts_root)
        module_graph[_module_import_scanner.IMPORTER_MODULE_NAME] = importer_path
        if diagnostics_enabled:
            _record_module_reason(
                module_reasons,
                _module_import_scanner.IMPORTER_MODULE_NAME,
                "importer_generated",
            )
    if (
        needs_generated_importer
        and _module_import_scanner.IMPORTER_MODULE_NAME in module_graph
    ):
        generated_module_source_paths.setdefault(
            _module_import_scanner.IMPORTER_MODULE_NAME,
            _logical_generated_module_path(
                _module_import_scanner.IMPORTER_MODULE_NAME
            ),
        )
    return _SupportModuleAugmentation(
        namespace_module_names=frozenset(namespace_modules),
        generated_module_source_paths=generated_module_source_paths,
    )


def _materialize_import_plan(
    *,
    prepared_module_graph: _PreparedEntryModuleGraph,
    module_reasons: MutableMapping[str, set[str]],
    stdlib_root: Path,
    artifacts_root: Path,
    entry_module: str,
    diagnostics_enabled: bool,
) -> _ImportPlan:
    module_graph = dict(prepared_module_graph.module_graph)
    stdlib_allowlist = set(prepared_module_graph.stdlib_allowlist)
    stub_parents = set(prepared_module_graph.stub_parents)
    support_modules = _augment_support_modules(
        module_graph=module_graph,
        module_reasons=module_reasons,
        roots=list(prepared_module_graph.roots),
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        explicit_imports=prepared_module_graph.explicit_imports,
        resolver_cache=prepared_module_graph.module_resolution_cache,
        artifacts_root=artifacts_root,
        stub_parents=stub_parents,
        entry_module=entry_module,
        needs_generated_importer=(
            prepared_module_graph.runtime_import_support_policy.needs_generated_importer
        ),
        diagnostics_enabled=diagnostics_enabled,
    )
    namespace_module_names = support_modules.namespace_module_names
    generated_module_source_paths = dict(support_modules.generated_module_source_paths)
    known_modules = frozenset(module_graph)
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    module_graph_metadata = _build_module_graph_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=set(namespace_module_names),
    )
    return _ImportPlan(
        stdlib_allowlist=frozenset(stdlib_allowlist),
        roots=tuple(prepared_module_graph.roots),
        stdlib_root=stdlib_root,
        module_resolution_cache=prepared_module_graph.module_resolution_cache,
        module_graph=MappingProxyType(dict(module_graph)),
        explicit_imports=frozenset(prepared_module_graph.explicit_imports),
        runtime_import_dispatch_roots=frozenset(
            prepared_module_graph.runtime_import_dispatch_roots
        ),
        stub_parents=frozenset(stub_parents),
        spawn_enabled=prepared_module_graph.spawn_enabled,
        runtime_import_support_policy=prepared_module_graph.runtime_import_support_policy,
        namespace_module_names=namespace_module_names,
        generated_module_source_paths=MappingProxyType(generated_module_source_paths),
        known_modules=known_modules,
        known_modules_sorted=tuple(sorted(known_modules)),
        stdlib_allowlist_sorted=tuple(sorted(stdlib_allowlist)),
        module_graph_metadata=module_graph_metadata,
        native_artifact_plan=prepared_module_graph.native_artifact_plan,
    )


def _augment_module_graph_for_entry_and_runtime(
    *,
    source_path: Path,
    entry_module: str,
    module_roots: Sequence[Path],
    stdlib_root: Path,
    roots: Sequence[Path],
    project_root: Path | None,
    stdlib_allowlist: set[str],
    entry_imports: Collection[str],
    module_resolution_cache: "_module_resolution._ModuleResolutionCache",
    module_graph: MutableMapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    diagnostics_enabled: bool,
    json_output: bool,
    target: str,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[_ModuleGraphAugmentation, _CliFailure | None]:
    roots = list(roots)
    module_roots = list(module_roots)
    entry_imports = set(entry_imports)
    explicit_imports = set(entry_imports)
    stub_skip_modules = STUB_MODULES - entry_imports
    stub_parents = STUB_PARENT_MODULES - entry_imports
    stdlib_profile = os.environ.get("MOLT_STDLIB_PROFILE", "micro")
    if stdlib_profile == "micro":
        core_module_names = _CORE_STDLIB_MODULES_MICRO
    else:
        core_module_names = _CORE_STDLIB_MODULES_FULL
    core_paths = [
        path
        for name in core_module_names
        if (path := module_graph.get(name)) is not None
    ]
    _extend_module_graph_with_closure(
        module_graph,
        entry_paths=core_paths,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=module_resolution_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        reason="core_closure",
        skip_modules=stub_skip_modules,
        stub_parents=stub_parents,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    spawn_enabled = False
    spawn_required = target != "wasm" and _requires_spawn_entry_override(
        module_graph, explicit_imports
    )
    if spawn_required:
        spawn_path = module_resolution_cache.resolve_module(
            ENTRY_OVERRIDE_SPAWN,
            roots,
            stdlib_root,
            stdlib_allowlist,
        )
        if spawn_path is None:
            return _ModuleGraphAugmentation(
                spawn_enabled=False,
                explicit_imports=explicit_imports,
                stub_parents=stub_parents,
            ), _fail(
                (
                    f"Missing required stdlib module: {ENTRY_OVERRIDE_SPAWN}. "
                    "multiprocessing spawn entry override cannot be lowered."
                ),
                json_output,
                command="build",
            )
        spawn_enabled = True
        _extend_module_graph_with_closure(
            module_graph,
            entry_paths=[spawn_path],
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            project_root=project_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="spawn_closure",
            skip_modules=stub_skip_modules,
            stub_parents=stub_parents,
            import_admission_policy=import_admission_policy,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
    return _ModuleGraphAugmentation(
        spawn_enabled=spawn_enabled,
        explicit_imports=explicit_imports,
        stub_parents=stub_parents,
    ), None


def _prepare_entry_module_graph(
    *,
    source_path: Path,
    entry_module: str,
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    entry_tree: ast.AST,
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    json_output: bool,
    target: str,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[_PreparedEntryModuleGraph | None, _CliFailure | None]:
    stdlib_allowlist = _stdlib_allowlist()
    roots = module_roots + [stdlib_root]
    module_resolution_cache = _module_resolution._ModuleResolutionCache()
    entry_is_package = source_path.name == "__init__.py"
    entry_imports = (
        _module_import_scanner._expand_imports_with_static_package_all_star_children(
            tuple(
                _module_import_scanner._collect_imports(
                    entry_tree,
                    entry_module,
                    entry_is_package,
                )
            ),
            entry_tree,
            module_name=entry_module,
            is_package=entry_is_package,
            import_scan_mode="full",
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolution_cache=module_resolution_cache,
            target_python=target_python,
        )
    )
    module_graph, explicit_imports = _discover_module_graph(
        source_path,
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=STUB_MODULES,
        stub_parents=STUB_PARENT_MODULES,
        resolver_cache=module_resolution_cache,
        precomputed_imports=entry_imports,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if diagnostics_enabled:
        for name in module_graph:
            _record_module_reason(module_reasons, name, "entry_closure")
    static_import_modules, static_import_error = _parse_static_import_modules(
        os.environ.get(STATIC_IMPORT_MODULES_ENV, "")
    )
    if static_import_error is not None:
        return None, _fail(static_import_error, json_output, command="build")
    static_import_errors = _extend_module_graph_with_static_import_modules(
        module_graph=module_graph,
        explicit_imports=explicit_imports,
        module_names=static_import_modules,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=module_resolution_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if static_import_errors:
        return None, _fail(
            "; ".join(static_import_errors),
            json_output,
            command="build",
        )
    while True:
        package_before = set(module_graph)
        added_package_parents = _collect_package_parents(
            module_graph,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            import_admission_policy=import_admission_policy,
        )
        if diagnostics_enabled:
            _record_new_module_reasons(
                module_graph,
                package_before,
                module_reasons,
                "package_parent",
            )
        if not added_package_parents:
            break
        package_parent_paths = [
            module_graph[name]
            for name in sorted(added_package_parents)
            if name in module_graph
        ]
        before_parent_closure = set(module_graph)
        _extend_module_graph_with_closure(
            module_graph,
            entry_paths=package_parent_paths,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            project_root=project_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="package_parent_closure",
            skip_modules=STUB_MODULES,
            stub_parents=STUB_PARENT_MODULES,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=False,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if diagnostics_enabled:
            _record_new_module_reasons(
                module_graph,
                before_parent_closure,
                module_reasons,
                "package_parent_closure",
            )
    core_before = set(module_graph)
    _ensure_core_stdlib_modules(module_graph, stdlib_root)
    if diagnostics_enabled:
        _record_new_module_reasons(
            module_graph,
            core_before,
            module_reasons,
            "core_required",
        )
    intrinsic_enforced = _enforce_intrinsic_stdlib(
        module_graph, stdlib_root, json_output
    )
    if intrinsic_enforced is not None:
        return None, intrinsic_enforced
    # MOLT_STDLIB_PROFILE is the single canonical profile signal the module-graph
    # construction and the runtime staticlib build both read (see the
    # `--stdlib-profile` propagation note); use the same source here so the
    # feature-availability refusal matches the profile that will actually link.
    feature_availability_enforced = _enforce_profile_feature_availability(
        module_graph,
        stdlib_root,
        os.environ.get("MOLT_STDLIB_PROFILE", "micro"),
        target,
        json_output,
    )
    if feature_availability_enforced is not None:
        return None, feature_availability_enforced
    augmentation, augmentation_error = _augment_module_graph_for_entry_and_runtime(
        source_path=source_path,
        entry_module=entry_module,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        roots=roots,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        entry_imports=explicit_imports,
        module_resolution_cache=module_resolution_cache,
        module_graph=module_graph,
        module_reasons=module_reasons,
        diagnostics_enabled=diagnostics_enabled,
        json_output=json_output,
        target=target,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if augmentation_error is not None:
        return None, augmentation_error
    runtime_import_dispatch_roots: set[str] = set(augmentation.explicit_imports)
    runtime_import_support_policy = (
        _module_import_scanner._module_graph_needs_runtime_import_support(
            module_graph=module_graph,
            module_resolution_cache=module_resolution_cache,
            explicit_imports=augmentation.explicit_imports,
            entry_module=entry_module,
            entry_path=source_path,
            entry_tree=entry_tree,
            target_python=target_python,
        )
    )
    if runtime_import_support_policy.needs_runtime_import_support:
        import_support_paths: list[Path] = []
        for module_name in _module_import_scanner._RUNTIME_IMPORT_SUPPORT_ROOT_MODULES:
            module_path = _module_resolution._resolve_module_path(
                module_name,
                [stdlib_root],
            )
            if module_path is None:
                return None, _fail(
                    f"Missing required stdlib support module: {module_name}",
                    json_output,
                    command="build",
                )
            import_support_paths.append(module_path)
        before_support = set(module_graph)
        support_closure_modules = _extend_module_graph_with_closure(
            module_graph,
            entry_paths=import_support_paths,
            roots=[stdlib_root],
            module_roots=[stdlib_root],
            stdlib_root=stdlib_root,
            project_root=None,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="runtime_import_support",
            import_admission_policy=import_admission_policy,
            target_python=target_python,
        )
        runtime_import_dispatch_roots.update(support_closure_modules)
        if diagnostics_enabled:
            _record_new_module_reasons(
                module_graph,
                before_support,
                module_reasons,
                "runtime_import_support",
            )
    return _PreparedEntryModuleGraph(
        stdlib_allowlist=stdlib_allowlist,
        roots=roots,
        module_resolution_cache=module_resolution_cache,
        module_graph=dict(module_graph),
        explicit_imports=augmentation.explicit_imports,
        runtime_import_dispatch_roots=frozenset(runtime_import_dispatch_roots),
        stub_parents=augmentation.stub_parents,
        spawn_enabled=augmentation.spawn_enabled,
        runtime_import_support_policy=runtime_import_support_policy,
        native_artifact_plan=(
            import_admission_policy.native_artifact_plan
            if import_admission_policy is not None
            else _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
        ),
    ), None
