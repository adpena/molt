from __future__ import annotations

import ast
from collections import deque
import functools
from collections.abc import Collection, Iterable, Mapping, Sequence
from pathlib import Path

from molt.cli import module_import_scanner as _module_import_scanner


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
