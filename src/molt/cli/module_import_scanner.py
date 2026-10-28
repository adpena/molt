from __future__ import annotations
from dataclasses import replace
from molt.compiler_analysis.python_imports import (
    SourceAllExports,
    normalize_stdlib_module_name,
)

import ntpath
import os
import posixpath
import stat
from collections.abc import Mapping, Sequence
from pathlib import Path

from molt.cli import module_resolution as _module_resolution
from molt.cli.python_import_topology import PythonImportTopology
from molt.compiler_analysis import python_import_scan as _source_import_scan
from molt.cli.models import (
    _CompleteImportScan,
    _ModuleGraphScanAuthority,
    _RuntimeImportSupportPolicy,
)
from molt.compiler_analysis.python_imports import _ImportScanRequests, _StaticSourcePath
from molt.target_python import (
    TargetPythonVersion,
)


# Modules the runtime import protocol acquires through its support loader.
# The meta-path walk calls `_warnings.warn("sys.meta_path is empty",
# ImportWarning)`, as CPython's `importlib._bootstrap._find_spec` does.
_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES = (
    "importlib",
    "importlib.util",
    "importlib.machinery",
    "_warnings",
)


_RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES = frozenset(
    {
        "builtins",
        "_intrinsics",
        *_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES,
        "importlib.abc",
    }
)


def _expand_static_package_all_star_children(
    star_modules: tuple[str, ...],
    *,
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolution_cache: _module_resolution._ModuleResolutionCache,
    target_python: TargetPythonVersion,
    module_graph: Mapping[str, Path] | None = None,
    source_requests: Mapping[str, _ImportScanRequests] | None = None,
) -> tuple[dict[str, tuple[str, ...]], dict[str, SourceAllExports]]:
    """Complete source export facts against this operation's actual topology."""
    children: dict[str, tuple[str, ...]] = {}
    exports_by_module: dict[str, SourceAllExports] = {}

    def resolve(name: str) -> Path | None:
        if module_graph is not None:
            return module_graph.get(normalize_stdlib_module_name(name) or name)
        return resolution_cache.resolve_module(
            name, list(roots), stdlib_root, stdlib_allowlist
        )

    for module in dict.fromkeys(star_modules):
        path = resolve(module)
        known = (source_requests or {}).get(
            normalize_stdlib_module_name(module) or module
        )
        if path is None:
            exports = SourceAllExports(
                "incomplete", reason="package source is unavailable"
            )
        elif path.name != "__init__.py":
            exports = SourceAllExports("absent")
        elif known is not None:
            exports = known.all_exports
        else:
            try:
                source = resolution_cache.read_module_source(path, retain=False)
                parsed = resolution_cache.parse_module_ast(
                    path,
                    source,
                    filename=str(path),
                    target_python=target_python,
                    retain=False,
                )
                exports = _source_import_scan._static_module_all_exports(
                    parsed.tree,
                    target_python=target_python,
                    ast_digest_admission=parsed.admission,
                )
            except (OSError, SyntaxError, UnicodeError) as exc:
                exports = SourceAllExports("incomplete", reason=type(exc).__name__)
        exports_by_module[module] = exports
        children[module] = tuple(
            dict.fromkeys(
                f"{module}.{name}"
                for name in exports.names
                if exports.status == "known" and resolve(f"{module}.{name}") is not None
            )
        )
    return children, exports_by_module


def _resolve_static_source_path(
    request: str | _StaticSourcePath,
    source_path: Path,
    import_topology: PythonImportTopology,
) -> str | Path:
    if isinstance(request, str):
        return request
    parts = tuple(
        _resolve_static_source_path(part, source_path, import_topology)
        for part in request.parts
    )
    head, *tail = parts
    if request.operation == "join":
        return Path(head).joinpath(*tail)
    joiner = {
        "os_join": os.path.join,
        "posix_join": posixpath.join,
        "nt_join": ntpath.join,
    }.get(request.operation)
    if joiner is not None:
        return joiner(*(os.fspath(part) for part in parts))
    head = Path(head)
    if request.operation in {"resolve", "absolute"}:
        if not head.is_absolute():
            head = source_path.parent / head
        if request.operation == "absolute":
            return head.absolute()
        # An existing path resolves through this operation's topology; a
        # missing one is recorded as a miss and keeps its lexical form.
        found = import_topology.probe(head)
        return found.path if found is not None else head.resolve()
    return head


def _complete_import_scan(
    requests: _ImportScanRequests,
    *,
    source_path: Path,
    roots: Sequence[Path] | None,
    stdlib_root: Path | None,
    stdlib_allowlist: set[str] | None,
    resolution_cache: _module_resolution._ModuleResolutionCache,
    target_python: TargetPythonVersion,
    module_graph: Mapping[str, Path] | None = None,
    source_requests: Mapping[str, _ImportScanRequests] | None = None,
) -> _CompleteImportScan:
    """Resolve every filesystem-derived decision in this operation, including misses."""
    imports = requests.imports
    candidates = requests.dynamic_relative_import_candidates
    star_children: dict[str, tuple[str, ...]] = {}
    star_exports: dict[str, SourceAllExports] = {}
    stars = tuple(
        dict.fromkeys((*requests.star_modules, *requests.dynamic_star_modules))
    )
    if stars:
        if module_graph is None and (
            roots is None or stdlib_root is None or stdlib_allowlist is None
        ):
            raise ValueError(
                "star-import completion requires current resolution context"
            )
        star_children, star_exports = _expand_static_package_all_star_children(
            stars,
            roots=roots or (),
            stdlib_root=stdlib_root or source_path.parent,
            stdlib_allowlist=stdlib_allowlist or set(),
            resolution_cache=resolution_cache,
            target_python=target_python,
            module_graph=module_graph,
            source_requests=source_requests,
        )
        imports = tuple(
            dict.fromkeys(
                (
                    *imports,
                    *(
                        child
                        for module in requests.star_modules
                        for child in star_children[module]
                    ),
                )
            )
        )
        candidates = tuple(
            dict.fromkeys(
                (
                    *candidates,
                    *(
                        child
                        for module in requests.dynamic_star_modules
                        for child in star_children[module]
                    ),
                )
            )
        )
    sites = []
    for site in requests.source_sites:
        if site.request is not None and "*" in site.request.fromlist:
            owners = site.owner_plan.modules
            children = tuple(
                child for owner in owners for child in star_children.get(owner, ())
            )
            unknown_exports = tuple(
                (owner, fact)
                for owner in owners
                if (
                    fact := star_exports.get(owner, SourceAllExports("incomplete"))
                ).status
                in {"unknown", "incomplete"}
            )
            site = replace(
                site,
                plan=replace(
                    site.plan,
                    modules=tuple(dict.fromkeys((*site.plan.modules, *children))),
                    requires_runtime=site.plan.requires_runtime
                    or bool(unknown_exports),
                    errors=tuple(
                        dict.fromkeys(
                            (
                                *site.plan.errors,
                                *(
                                    f"unresolved star exports for {owner!r}: "
                                    f"{fact.reason or fact.status}"
                                    for owner, fact in unknown_exports
                                ),
                            )
                        )
                    ),
                ),
            )
        sites.append(site)
    executions: list[tuple[str | None, Path]] = []
    seen: set[tuple[str | None, Path]] = set()
    # Each target decision is an import fact of this operation, so a target
    # created, removed or retargeted during the build ends it.
    topology = resolution_cache.import_topology
    for request in requests.source_executions:
        candidate = Path(
            _resolve_static_source_path(request.path, source_path, topology)
        )
        if not candidate.is_absolute():
            candidate = source_path.parent / candidate
        target = topology.probe(candidate)
        if target is not None and stat.S_ISDIR(target.mode):
            target = topology.probe(target.path / "__main__.py")
        if (
            target is None
            or not stat.S_ISREG(target.mode)
            or target.path.suffix not in {".py", ".pyi"}
        ):
            continue
        execution = (request.module_name, target.path)
        if execution not in seen:
            seen.add(execution)
            executions.append(execution)
    return _CompleteImportScan(
        requests,
        imports,
        tuple(executions),
        candidates,
        requests.requires_runtime_package_anchor
        or any(
            fact.status in {"unknown", "incomplete"} for fact in star_exports.values()
        ),
        requests.source_demands,
        requests.source_bindings,
        requests.source_values,
        tuple(sites),
        tuple(star_exports.items()),
    )


def _module_graph_needs_runtime_import_support(
    *,
    module_graph: Mapping[str, Path],
    scan_authority: _ModuleGraphScanAuthority,
) -> _RuntimeImportSupportPolicy:
    """Whether the closed graph needs the runtime import protocol.

    Each scan records whether its reached code imports or may call an import
    entry point in its execution-region visitor, and discovery ORs that bit
    into the module's scan authority; a source that was not scanned claims
    it. The decision reads those bits and no source.
    """
    scan_authority.validate_graph(module_graph)
    scan_authority.validate_source_generations()
    return _RuntimeImportSupportPolicy(
        needs_runtime_import_support=any(
            source.uses_runtime_import_protocol
            for source in scan_authority.sources
            if source.module_name not in _RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES
        ),
    )
