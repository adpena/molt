from __future__ import annotations

import ast
import hashlib
import json
import weakref
from pathlib import Path
from types import SimpleNamespace
from typing import Any, NoReturn

import pytest

from molt.cli import binary_image_analysis as diagnostics
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease


class _NativePlan:
    def __init__(self) -> None:
        self.name_queries = 0

    def native_module_names(self) -> frozenset[str]:
        self.name_queries += 1
        return frozenset()

    def digest_payload(self) -> dict[str, Any]:
        return {"artifacts": []}


class _ImageScope:
    def diagnostic_payload(self) -> dict[str, str]:
        return {"kind": "binary", "entry_module": "app"}


def _inputs(
    tmp_path: Path, sources: dict[str, str]
) -> tuple[SimpleNamespace, SimpleNamespace]:
    paths: dict[str, Path] = {}
    for name, source in sources.items():
        paths[name] = tmp_path / f"{name}.py"
        paths[name].write_text(source, encoding="utf-8")
    names = frozenset(paths)
    plan = SimpleNamespace(
        known_modules=names,
        compile_modules=names,
        declared_root_modules=frozenset({"app"}),
        entry_reachable_modules=names,
        runtime_support_modules=frozenset(),
        stdlib_support_modules=frozenset(),
        package_parent_modules=frozenset(),
        namespace_module_names=frozenset(),
        module_graph=paths,
        module_graph_metadata=SimpleNamespace(
            logical_source_path_by_module={name: f"<{name}>" for name in paths}
        ),
        module_resolution_cache=_ModuleResolutionCache(),
        native_artifact_plan=_NativePlan(),
        image_scope=_ImageScope(),
    )
    analysis = SimpleNamespace(
        module_sources={},
        module_trees={},
        module_source_catalog=_ModuleSourceCatalog(
            {name: _ModuleSourceLease.path_backed(path) for name, path in paths.items()}
        ),
        module_order=sorted(names),
        module_layers=[sorted(names)],
        module_deps={name: set() for name in names},
        module_dep_closures={name: frozenset() for name in names},
        has_back_edges=False,
    )
    return plan, analysis


def _payload(
    plan: SimpleNamespace, analysis: SimpleNamespace, *, full: bool = True
) -> dict[str, Any]:
    return diagnostics._frontend_binary_image_analysis_payload(
        import_plan=plan,
        frontend_analysis=analysis,
        frontend_module_costs={},
        known_classes={},
        enable_phi=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        type_facts_present=False,
        compile_module_order=analysis.module_order,
        compile_module_layers=analysis.module_layers,
        target_python=SimpleNamespace(tag="3.12"),
        include_source_details=full,
    )


def _count_capture(
    monkeypatch: pytest.MonkeyPatch, *, require_release: bool = False
) -> dict[str, int]:
    counts = {"reads": 0, "parses": 0}
    trees: list[weakref.ReferenceType[ast.AST]] = []
    read = _ModuleSourceLease.read
    parse = ast.parse

    def read_lease(self: _ModuleSourceLease, resolution_cache: Any = None) -> str:
        if require_release:
            assert all(tree() is None for tree in trees)
        counts["reads"] += 1
        return read(self, resolution_cache)

    def parse_source(
        source: str, filename: str = "<unknown>", **kwargs: Any
    ) -> ast.AST:
        counts["parses"] += 1
        tree = parse(source, filename=filename, **kwargs)
        trees.append(weakref.ref(tree))
        return tree

    monkeypatch.setattr(_ModuleSourceLease, "read", read_lease)
    monkeypatch.setattr(diagnostics.ast, "parse", parse_source)
    return counts


def test_full_projection_reads_parses_and_releases_each_module_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plan, analysis = _inputs(tmp_path, {"app": "x = 1\n", "helper": "y = 2\n"})
    counts = _count_capture(monkeypatch, require_release=True)
    payload = _payload(plan, analysis)
    assert counts == {"reads": 2, "parses": 2}
    assert payload["source_ast"]["known"]["ast_nodes"] == 10
    assert payload["source_identity"]["site_count"] == 6
    assert plan.native_artifact_plan.name_queries == 1
    assert analysis.module_sources == {} and analysis.module_trees == {}
    assert plan.module_resolution_cache.source_cache == {}
    assert plan.module_resolution_cache.ast_cache == {}
    # A new diagnostics operation must recapture; no source/AST memo escapes.
    assert _payload(plan, analysis) == payload
    assert counts == {"reads": 4, "parses": 4}


@pytest.mark.parametrize("retain_source", [False, True])
@pytest.mark.parametrize("retain_tree", [False, True])
def test_projection_reuses_existing_source_and_tree_independently(
    retain_source: bool,
    retain_tree: bool,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = "x = 1\n"
    plan, analysis = _inputs(tmp_path, {"app": source})
    if retain_source:
        analysis.module_sources["app"] = source
    if retain_tree:
        analysis.module_trees["app"] = ast.parse(source)
    counts = _count_capture(monkeypatch)
    payload = _payload(plan, analysis)
    assert counts == {"reads": int(not retain_source), "parses": int(not retain_tree)}
    assert payload["source_ast"]["known"]["ast_nodes"] == 5
    assert payload["source_identity"]["site_count"] == 3


def test_omitted_details_never_read_parse_or_project_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plan, analysis = _inputs(tmp_path, {"app": "x = 1\n"})

    def forbidden(*_args: object, **_kwargs: object) -> NoReturn:
        raise AssertionError("non-full diagnostics must not inspect source")

    monkeypatch.setattr(_ModuleSourceLease, "read", forbidden)
    monkeypatch.setattr(diagnostics.ast, "parse", forbidden)
    monkeypatch.setattr(diagnostics, "_module_source_projection", forbidden)
    monkeypatch.setattr(plan.native_artifact_plan, "native_module_names", forbidden)
    payload = _payload(plan, analysis, full=False)
    assert payload["source_identity"]["detail"] == "omitted"
    assert payload["source_ast"]["detail"] == "omitted"


def test_projection_keeps_lease_validation_and_recaptures_new_lease(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plan, analysis = _inputs(tmp_path, {"app": "x = 1\n"})
    path = plan.module_graph["app"]
    changed = "longer_name = 2\n"
    path.write_text(changed, encoding="utf-8")
    counts = _count_capture(monkeypatch)
    rejected = _payload(plan, analysis)
    assert counts == {"reads": 1, "parses": 0}
    assert rejected["source_identity"]["modules"][0]["source_sha256"] == (
        hashlib.sha256(b"").hexdigest()
    )
    assert rejected["source_ast"]["known"]["ast_nodes"] == 0
    analysis.module_source_catalog = _ModuleSourceCatalog(
        {"app": _ModuleSourceLease.path_backed(path)}
    )
    accepted = _payload(plan, analysis)
    assert counts == {"reads": 2, "parses": 1}
    assert accepted["source_identity"]["modules"][0]["source_sha256"] == (
        hashlib.sha256(changed.encode("utf-8")).hexdigest()
    )


def test_invalid_source_is_parsed_once_for_both_projections(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = "def broken(:\n"
    plan, analysis = _inputs(tmp_path, {"app": source})
    counts = _count_capture(monkeypatch)
    payload = _payload(plan, analysis)
    assert counts == {"reads": 1, "parses": 1}
    assert payload["source_ast"]["known"]["ast_nodes"] == 0
    identity = payload["source_identity"]["modules"][0]
    assert identity["site_count"] == 0
    assert (
        identity["source_sha256"] == hashlib.sha256(source.encode("utf-8")).hexdigest()
    )


def _legacy_hash(payload: Any) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


@pytest.mark.parametrize(
    ("source", "qualname", "sites", "function_defs"),
    [
        (
            "x = 1\n",
            "<module>",
            [
                ("Assign", [0], 1, 0, 1, 5),
                ("Name", [0, 0], 1, 0, 1, 1),
                ("Constant", [0, 1], 1, 4, 1, 5),
            ],
            0,
        ),
        (
            "def outer():\n    return 1\n",
            "outer",
            [
                ("FunctionDef", [0], 1, 0, 2, 12),
                ("Return", [0, 1], 2, 4, 2, 12),
                ("Constant", [0, 1, 0], 2, 11, 2, 12),
            ],
            1,
        ),
    ],
)
def test_projection_preserves_v1_identity_paths_spans_qualnames_and_metrics(
    source: str,
    qualname: str,
    sites: list[tuple[str, list[int], int, int, int, int]],
    function_defs: int,
    tmp_path: Path,
) -> None:
    plan, analysis = _inputs(tmp_path, {"app": source})
    source_hash = hashlib.sha256(source.encode("utf-8")).hexdigest()
    site_ids = [
        _legacy_hash(
            {
                "schema_version": 1,
                "module": "app",
                "source_sha256": source_hash,
                "target_python": "3.12",
                "node_kind": kind,
                "qualname": qualname,
                "ast_path": path,
                "span": {
                    "line": line,
                    "col": col,
                    "end_line": end,
                    "end_col": end_col,
                },
            }
        )
        for kind, path, line, col, end, end_col in sites
    ]
    roles = ["declared_root", "entry_reachable", "compile", "known"]
    identity = {
        "module": "app",
        "logical_path": "<app>",
        "source_sha256": source_hash,
        "roles": roles,
        "site_count": 3,
        "site_digest": _legacy_hash(sorted(site_ids)),
        "node_kind_counts": {site[0]: 1 for site in sites},
    }
    module_digest = {
        key: identity[key]
        for key in ("module", "source_sha256", "site_digest", "roles")
    }
    expected = {
        "schema_version": 1,
        "target_python": "3.12",
        "module_count": 1,
        "compile_module_count": 1,
        "site_count": 3,
        "compile_site_count": 3,
        "modules": [identity],
        "site_digest": _legacy_hash([identity["site_digest"]]),
        "semantic_identity_digest": _legacy_hash(
            {
                "schema_version": 1,
                "image": plan.image_scope.diagnostic_payload(),
                "compile_modules": ["app"],
                "modules": [module_digest],
                "native_artifact_plan": {"artifacts": []},
            }
        ),
    }
    payload = _payload(plan, analysis)
    assert payload["source_identity"] == expected
    metrics = payload["source_ast"]["known"]
    assert metrics == {
        "ast_nodes": 5,
        "function_defs": function_defs,
        "class_defs": 0,
        "import_statements": 0,
        "loops": 0,
        "branches": 0,
        "calls": 0,
    }
    assert payload["source_ast"]["source_bytes_known"] == len(source.encode("utf-8"))
