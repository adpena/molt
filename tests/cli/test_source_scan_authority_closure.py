from __future__ import annotations

from molt.compiler_analysis import python_import_scan as _source_import_scan

import ast
import hashlib
import json
import sys
from dataclasses import replace
from pathlib import Path
from unittest.mock import Mock

from tests.process_guard_common import run_guarded_test_process

import pytest

from molt.cli import module_graph as graphs
from molt.cli import module_graph_cache as graph_cache
from molt.cli import module_graph_discovery as discovery
from molt.cli import module_import_scanner as scanner
from molt.cli.module_source import PythonSourceSnapshot
from molt.cli.models import (
    _BinaryImageScope,
    _CompleteImportScan,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ExternalPackageNativeArtifactPlan,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _PreparedEntryModuleGraph,
    _RuntimeImportSupportPolicy,
)
from molt.compiler_analysis.python_imports import (
    _ImportScanRequests,
    _ImportDiscoveryProjection,
)
from molt.compiler_analysis.python_namespace_writes import python_namespace_summary
from molt.compiler_analysis.python_source_keys import python_node_source_key
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_identity_catalog_generated import PythonMember
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION as PYTHON,
)


def _source(path: Path, source: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8")
    return path.resolve()


def _discover(
    path: Path,
    root: Path,
    *,
    full: bool,
    project: Path | None = None,
    state: discovery._ModuleGraphDiscovery | None = None,
):
    return discovery._discover_module_graph_from_paths(
        (path,),
        [root, root / "stdlib"],
        [root],
        root / "stdlib",
        project,
        {"runtime_owner"},
        full_scan_roots=full,
        discovery=state,
        resolver_cache=state.resolver_cache if state is not None else None,
    )


@pytest.mark.parametrize("first_full", [False, True])
def test_role_receipt_survives_cache_hits_and_protocol_detection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    first_full: bool,
) -> None:
    entry = _source(tmp_path / "entry.py", "pass\n")
    helper = _source(
        tmp_path / "helper.py", "def invoke():\n    return __import__('leaf')\n"
    )
    _source(tmp_path / "leaf.py", "pass\n")
    monkeypatch.setattr(
        graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "roles"
    )
    expected = {}
    for full in (first_full, not first_full):
        expected[full] = _discover(helper, tmp_path, full=full, project=tmp_path)

    def forbidden(*args, **kwargs):
        raise AssertionError("warm graph receipt must not repeat source scans")

    monkeypatch.setattr(_source_import_scan, "_collect_import_scan_requests", forbidden)
    for full in (first_full, not first_full):
        result = _discover(helper, tmp_path, full=full, project=tmp_path)
        assert result.scan_authority == expected[full].scan_authority
        graph = {**result.graph, "entry": entry}
        authority = result.scan_authority.merged(
            _ModuleGraphScanAuthority(
                (_ModuleSourceScanAuthority("entry", entry, "full"),)
            )
        )
        policy = scanner._module_graph_needs_runtime_import_support(
            module_graph=graph,
            scan_authority=authority,
            module_resolution_cache=_ModuleResolutionCache(),
            explicit_imports=result.explicit_imports,
            entry_module="entry",
            entry_path=entry,
            entry_tree=ast.parse("pass"),
            target_python=PYTHON,
        )
        assert policy.needs_runtime_import_support is full


def test_full_role_promotes_existing_source_without_downgrade(tmp_path: Path) -> None:
    helper = _source(tmp_path / "helper.py", "def invoke():\n    import leaf\n")
    _source(tmp_path / "leaf.py", "pass\n")
    graph = {}
    roles = {}
    for full in (False, True, False):
        result = _discover(helper, tmp_path, full=full)
        discovery._merge_discovered_module_graph(graph, roles, result)
    assert set(graph) == {"helper", "leaf"}
    assert roles["helper"].mode == "full"
    other = _source(tmp_path / "other.py", "pass\n")
    with pytest.raises(ValueError, match="source authority"):
        _ModuleGraphScanAuthority(tuple(roles.values())).merged(
            _ModuleGraphScanAuthority(
                (_ModuleSourceScanAuthority("helper", other, "full"),)
            )
        )


@pytest.mark.parametrize(
    ("left", "right", "expected"),
    [
        (None, PythonNamespacePremise((), 0), None),
        (
            PythonNamespacePremise(("a",), 0),
            PythonNamespacePremise(("b",), int(PythonMember.IMPORT_HOOKS)),
            PythonNamespacePremise(("a", "b"), int(PythonMember.IMPORT_HOOKS)),
        ),
        (
            PythonNamespacePremise(None, 0),
            PythonNamespacePremise(("b",), int(PythonMember.IMPORT_HOOKS)),
            PythonNamespacePremise(None, int(PythonMember.IMPORT_HOOKS)),
        ),
    ],
)
def test_scan_authority_merge_and_payload_preserve_namespace_proof(
    tmp_path, left, right, expected
) -> None:
    source = _source(tmp_path / "owner.py", "pass\n")
    first = _ModuleGraphScanAuthority(
        (_ModuleSourceScanAuthority("owner", source, "full", namespace_premise=left),)
    )
    second = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "owner", source, "module_init", namespace_premise=right
            ),
        )
    )
    merged = first.merged(second)
    assert merged == second.merged(first)
    assert merged.by_module["owner"].namespace_premise == expected
    row = merged.payload()[0]
    assert row["namespace_premise"] == (
        None
        if expected is None
        else {
            "foreign_writes": expected.foreign_writes,
            "member_writes": expected.member_writes,
        }
    )


@pytest.mark.parametrize("left_mode", ["module_init", "full"])
@pytest.mark.parametrize("right_mode", ["module_init", "full"])
@pytest.mark.parametrize(
    "left_dynamic,right_dynamic",
    [(False, False), (False, True), (True, False), (True, True)],
)
def test_scan_merge_preserves_depth_and_dynamic_package_requirement(
    tmp_path: Path,
    left_mode,
    right_mode,
    left_dynamic,
    right_dynamic,
) -> None:
    source = _source(tmp_path / "pkg" / "owner.py", "pass\n")
    left = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "pkg.owner", source, left_mode, False, left_dynamic
            ),
        )
    )
    right = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "pkg.owner", source, right_mode, False, right_dynamic
            ),
        )
    )
    merged = left.merged(right)
    assert merged == right.merged(left)
    assert merged.merged(left).merged(right) == merged
    modes = ["module_init", "full"]
    assert merged.by_module["pkg.owner"].mode == max(
        (left_mode, right_mode), key=modes.index
    )
    assert merged.by_module["pkg.owner"].requires_runtime_package_anchor is (
        left_dynamic or right_dynamic
    )


def test_complete_precomputed_scan_rejects_mode_path_and_content_drift(
    tmp_path: Path,
) -> None:
    source = _source(tmp_path / "entry.py", "pass\n")
    expected_scan = _CompleteImportScan(
        _ImportScanRequests(
            (),
            (),
            dynamic_relative_import_candidates=("pkg.child",),
            requires_runtime_package_anchor=True,
        ),
        (),
        (),
        ("pkg.child",),
        True,
    )
    record = discovery._bind_precomputed_module_import_scan(
        source,
        snapshot=PythonSourceSnapshot.capture(source),
        module_name="entry",
        import_scan_mode="full",
        scan=expected_scan,
        target_python=PYTHON,
    )

    def load(path=source, mode="full"):
        return discovery._load_module_import_scan(
            path,
            module_name="entry",
            is_package=False,
            import_scan_mode=mode,
            resolution_cache=_ModuleResolutionCache(),
            project_root=None,
            precomputed_scan=record,
        )

    assert load().scan == expected_scan
    assert record.authority.requires_runtime_package_anchor
    with pytest.raises(ValueError, match="custody"):
        load(mode="module_init")
    with pytest.raises(ValueError, match="source authority"):
        load(path=_source(tmp_path / "other.py", "pass\n"))
    source.write_text("VALUE = 1\n", encoding="utf-8")
    with pytest.raises(ValueError, match="custody"):
        load()


def test_persisted_import_scan_roundtrip_preserves_dynamic_discovery_fact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = _source(tmp_path / "pkg" / "owner.py", "pass\n")
    monkeypatch.setattr(
        graph_cache,
        "_frontend_semantic_tooling_fingerprint",
        lambda: "dynamic-relative-imports",
    )
    expected = _ImportScanRequests(
        ("ssl",),
        (),
        (),
        ("pkg.child",),
        True,
        namespace_summary=python_namespace_summary(ast.parse(source.read_bytes())),
    )

    graph_cache._write_persisted_import_scan(
        tmp_path,
        source,
        module_name="pkg.owner",
        is_package=False,
        import_scan_mode="full",
        scan=expected,
        snapshot=PythonSourceSnapshot.capture(source),
    )

    assert (
        graph_cache._read_persisted_import_scan_record(
            tmp_path,
            source,
            module_name="pkg.owner",
            is_package=False,
            import_scan_mode="full",
        )
        == expected
    )


def test_runtime_policy_memo_covers_statements_and_binds_source_and_mode(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = _source(tmp_path / "entry.py", "def invoke():\n    import leaf\n")
    cache = _ModuleResolutionCache()
    reads = []
    read = cache.read_module_source

    def counted(path, **kwargs):
        reads.append(path)
        return read(path, **kwargs)

    monkeypatch.setattr(cache, "read_module_source", counted)

    def detect(mode):
        return scanner._module_uses_runtime_import_protocol(
            module_name="entry",
            module_path=path,
            module_resolution_cache=cache,
            target_python=PYTHON,
            import_scan_mode=mode,
        )

    assert not detect("module_init")
    assert detect("full")
    assert detect("full")
    assert len(reads) == 2
    path.write_text("pass\n", encoding="utf-8")
    assert not detect("full")
    assert len(reads) == 3
    assert len(cache.ast_cache) == 1
    assert (
        next(iter(cache.ast_cache.values())).admission.tree
        is next(iter(cache.ast_cache.values())).tree
    )


@pytest.mark.parametrize("target", ["native", "wasm", "rust", "luau", "mlir"])
def test_late_native_root_refreshes_policy_catalog_and_generated_importer(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    target: str,
) -> None:
    entry = _source(tmp_path / "entry.py", "pass\n")
    late = _source(
        tmp_path / "late.py", "def invoke():\n    return __import__('_molt_importer')\n"
    )
    owner = _source(tmp_path / "stdlib" / "runtime_owner.py", "pass\n")
    monkeypatch.setattr(
        scanner, "_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES", ("runtime_owner",)
    )
    initial = _discover(entry, tmp_path, full=True)
    assert initial.discovery is not None
    prepared = _PreparedEntryModuleGraph(
        discovery=initial.discovery,
        image_scope=_BinaryImageScope.from_entry(
            kind="entry_script",
            selector_source="test:entry",
            entry_module="entry",
            source_path=entry,
            project_root=tmp_path,
            module_roots=[tmp_path],
        ),
        declared_root_modules=frozenset({"entry"}),
        stdlib_allowlist={"runtime_owner"},
        roots=[tmp_path, tmp_path / "stdlib"],
        module_resolution_cache=initial.discovery.resolver_cache,
        module_graph=initial.graph,
        scan_authority=initial.scan_authority,
        target=target,
        explicit_imports=initial.explicit_imports,
        runtime_import_dispatch_roots=frozenset(),
        stub_parents=set(),
        spawn_enabled=False,
        runtime_import_support_policy=_RuntimeImportSupportPolicy(False, False),
        native_artifact_plan=_EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
        target_python=PYTHON,
    )

    def add_native_root(*, module_graph, scan_authorities, **kwargs):
        result = _discover(late, tmp_path, full=True, state=kwargs["discovery"])
        discovery._merge_discovered_module_graph(module_graph, scan_authorities, result)
        return frozenset(result.explicit_imports | {"late"})

    monkeypatch.setattr(
        graphs, "_extend_native_support_source_closure", add_native_root
    )
    plan = graphs._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons={},
        stdlib_root=owner.parent,
        artifacts_root=tmp_path / "artifacts",
        entry_module="entry",
        diagnostics_enabled=False,
    )
    assert plan.runtime_import_support_policy.needs_generated_importer
    assert plan.runtime_import_support_policy.needs_runtime_import_support
    assert "_molt_importer" in plan.module_graph
    assert plan.runtime_import_scan_custody is not None
    assert plan.runtime_import_scan_custody.catalog_by_module["late"] == late
    assert (
        plan.runtime_import_scan_custody.catalog_by_module["_molt_importer"]
        == plan.module_graph["_molt_importer"].resolve()
    )
    assert plan.module_graph_operation_counts["native_support_iterations"] < 6
    assert "late" in plan.runtime_import_dispatch_roots
    assert plan.scan_authority.mode_for("late", late) == "full"
    narrowed = plan.with_compile_modules({"entry"})
    assert narrowed.scan_authority is plan.scan_authority
    assert narrowed.runtime_import_scan_custody is plan.runtime_import_scan_custody


def test_runtime_owner_catalog_refresh_rescans_only_under_new_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    extend = Mock(wraps=discovery._extend_module_graph_with_closure)
    monkeypatch.setattr(discovery, "_extend_module_graph_with_closure", extend)
    entry = _source(tmp_path / "entry.py", "import leaf\n")
    leaf = _source(tmp_path / "leaf.py", "pass\n")
    owner = _source(tmp_path / "stdlib" / "runtime_owner.py", "pass\n")
    monkeypatch.setattr(
        scanner, "_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES", ("runtime_owner",)
    )
    initial = _discover(entry, tmp_path, full=True)
    graph, roles = initial.graph, dict(initial.scan_authority.by_module)
    assert graph["leaf"] == leaf
    assert initial.discovery is not None
    common = dict(
        discovery=initial.discovery,
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        module_resolution_cache=initial.discovery.resolver_cache,
        roots=[tmp_path, owner.parent],
        stdlib_root=owner.parent,
        stdlib_allowlist={"runtime_owner"},
        entry_module="entry",
        target_python=PYTHON,
        import_admission_policy=None,
    )
    first = graphs._finalize_runtime_import_closure(
        **common,
        explicit_imports={"leaf"},
        dispatch_roots={"leaf"},
    )
    assert first.custody is not None
    extensions = extend.call_count
    stable = graphs._finalize_runtime_import_closure(
        **common,
        explicit_imports={"leaf"},
        dispatch_roots=first.dispatch_roots,
        previous_custody=first.custody,
    )
    assert stable.custody is first.custody
    assert extend.call_count == extensions
    late = _source(tmp_path / "late.py", "pass\n")
    discovery._merge_discovered_module_graph(
        graph, roles, _discover(late, tmp_path, full=True, state=initial.discovery)
    )
    second = graphs._finalize_runtime_import_closure(
        **common,
        explicit_imports={"leaf", "late"},
        dispatch_roots=first.dispatch_roots | {"late"},
        previous_custody=first.custody,
    )
    assert second.custody is not None and second.custody != first.custody
    assert extend.call_count > extensions
    assert any(
        call.kwargs["runtime_import_custody"] is second.custody
        for call in extend.call_args_list
    )
    assert second.custody.catalog_by_module["late"] == late
    second.custody.validate_graph(graph)
    assert set(second.custody.modules) <= second.dispatch_roots
    extensions = extend.call_count
    stable = graphs._finalize_runtime_import_closure(
        **common,
        explicit_imports={"leaf", "late"},
        dispatch_roots=second.dispatch_roots,
        previous_custody=second.custody,
    )
    assert stable.custody is second.custody
    assert extend.call_count == extensions

    # Source/AST retention is operation-scoped: changing an owner after discovery
    # must not combine the new custody generation with the old retained parse.
    owner.write_text("OWNER_REVISION = 2\n", encoding="utf-8")
    from molt.cli.module_source import PythonSourceChangedError

    with pytest.raises(PythonSourceChangedError, match="generation changed"):
        graphs._finalize_runtime_import_closure(
            **common,
            explicit_imports={"leaf", "late"},
            dispatch_roots=second.dispatch_roots,
            previous_custody=second.custody,
        )
    # A new byte generation starts a complete operation. Earlier namespace and
    # source-value summaries cannot be transplanted into that new graph.
    fresh_custodies = []
    for _ in range(2):
        cache = _ModuleResolutionCache()
        state = discovery._ModuleGraphDiscovery(cache, PYTHON)
        rebuilt = discovery._discover_module_graph_from_paths(
            (entry, late),
            [tmp_path, owner.parent],
            [tmp_path],
            owner.parent,
            None,
            {"runtime_owner"},
            full_scan_roots=True,
            resolver_cache=cache,
            discovery=state,
            entry_imports={"runtime_owner"},
        )
        rebuilt_graph = dict(rebuilt.graph)
        rebuilt_roles = dict(rebuilt.scan_authority.by_module)
        assert set(rebuilt_graph) == {"entry", "leaf", "late", "runtime_owner"}
        refreshed = graphs._finalize_runtime_import_closure(
            discovery=state,
            module_graph=rebuilt_graph,
            scan_authorities=rebuilt_roles,
            module_reasons={},
            module_resolution_cache=cache,
            roots=[tmp_path, owner.parent],
            stdlib_root=owner.parent,
            stdlib_allowlist={"runtime_owner"},
            entry_module="entry",
            target_python=PYTHON,
            import_admission_policy=None,
            explicit_imports={"leaf", "late"},
            dispatch_roots=second.dispatch_roots,
            previous_custody=second.custody,
        )
        assert refreshed.custody is not None and refreshed.custody != second.custody
        assert refreshed.custody.owners == second.custody.owners
        assert refreshed.custody.catalog == second.custody.catalog
        assert refreshed.custody.owner_ast_digests != second.custody.owner_ast_digests
        fresh_custodies.append(refreshed.custody)
    assert fresh_custodies[0] == fresh_custodies[1]
    assert extend.call_count > extensions


def test_dynamic_owner_fixed_point_uses_custody_names_and_admitted_roots(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = _source(tmp_path / "app" / "entry.py", "import support.owner\n")
    stdlib = tmp_path / "stdlib"
    _source(stdlib / "support" / "__init__.py", "pass\n")
    owner = _source(
        stdlib / "support" / "owner.py",
        "__package__ = choose_package()\nfrom . import child\n",
    )
    _source(tmp_path / "vendor" / "support" / "__init__.py", "pass\n")
    child = _source(
        tmp_path / "vendor" / "support" / "child.py",
        "__package__ = choose_package()\nfrom . import grandchild\n",
    )
    grandchild = _source(
        tmp_path / "vendor" / "support" / "grandchild.py",
        "VALUE = 1\n",
    )
    monkeypatch.setattr(
        scanner,
        "_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES",
        ("support.owner",),
    )
    cache = _ModuleResolutionCache()
    state = discovery._ModuleGraphDiscovery(cache, PYTHON)
    initial = discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path / "app", tmp_path / "vendor", stdlib],
        [tmp_path],
        stdlib,
        None,
        {"support.owner"},
        full_scan_roots=True,
        resolver_cache=cache,
        discovery=state,
    )
    graph, roles = initial.graph, dict(initial.scan_authority.by_module)
    common = dict(
        discovery=state,
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        explicit_imports={"support.owner"},
        dispatch_roots={"support.owner"},
        module_resolution_cache=cache,
        roots=[tmp_path / "app", tmp_path / "vendor", stdlib],
        stdlib_root=stdlib,
        stdlib_allowlist={"support.owner"},
        entry_module="app.entry",
        target_python=PYTHON,
        import_admission_policy=None,
    )

    closure = graphs._finalize_runtime_import_closure(**common)

    assert closure.policy.needs_runtime_import_support
    assert closure.custody is not None
    assert closure.custody.owners_by_module["support.owner"] == owner
    assert closure.custody.owners_by_module["support.child"] == child
    assert closure.custody.catalog_by_module["support.grandchild"] == grandchild
    assert roles["support.owner"].mode == "full"
    assert roles["support.child"].mode == "full"
    assert roles["support.owner"].requires_runtime_package_anchor
    assert roles["support.child"].requires_runtime_package_anchor


def test_ordinary_nested_package_owner_rescans_under_original_project_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    entry = _source(project / "entry.py", "pass\n")
    _source(project / "pkg" / "__init__.py", "pass\n")
    _source(project / "pkg" / "nested" / "__init__.py", "pass\n")
    owner = _source(
        project / "pkg" / "nested" / "owner.py",
        "__package__ = choose_package()\nfrom . import child\n",
    )
    child = _source(project / "pkg" / "nested" / "child.py", "VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    support = _source(stdlib / "runtime_owner.py", "pass\n")
    monkeypatch.setattr(
        scanner,
        "_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES",
        ("runtime_owner",),
    )
    cache = _ModuleResolutionCache()
    state = discovery._ModuleGraphDiscovery(cache, PYTHON)
    initial = discovery._discover_module_graph_from_paths(
        (entry, owner),
        [project, stdlib],
        [project],
        stdlib,
        None,
        {"runtime_owner"},
        full_scan_roots=False,
        resolver_cache=cache,
        discovery=state,
    )
    graph, roles = initial.graph, dict(initial.scan_authority.by_module)
    closure = graphs._finalize_runtime_import_closure(
        discovery=state,
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        explicit_imports={"pkg.nested.owner"},
        dispatch_roots={"pkg.nested.owner"},
        module_resolution_cache=cache,
        roots=[project, stdlib],
        stdlib_root=stdlib,
        stdlib_allowlist={"runtime_owner"},
        entry_module="entry",
        target_python=PYTHON,
        import_admission_policy=None,
    )

    assert closure.policy.needs_runtime_import_support
    assert closure.custody is not None
    assert closure.custody.owners_by_module["pkg.nested.owner"] == owner
    assert closure.custody.owners_by_module["runtime_owner"] == support
    assert graph["pkg.nested.child"] == child
    assert roles["pkg.nested.owner"].mode == "full"


@pytest.mark.parametrize("source_name", ["support.py", "support/__init__.py"])
def test_native_slice_preserves_original_source_for_both_scan_projections(
    tmp_path: Path,
    source_name: str,
) -> None:
    source = _source(
        tmp_path / source_name,
        "def used():\n    return __import__('leaf')\n\ndef unused():\n    return __import__('dead')\n",
    )

    class NativePlan:
        def support_source_paths_by_module(self):
            return {"support": source}

    counts = {
        name: 0
        for name in (
            "native_support_slice_requests",
            "native_support_legacy_equivalent_source_parses",
            "native_support_slice_cache_hits",
            "native_support_slice_cache_misses",
            "native_support_persisted_import_scan_hits",
            "native_support_persisted_import_scan_misses",
            "native_support_source_parses",
            "native_support_source_prunes",
        )
    }
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    result = graphs._native_support_source_slices(
        native_artifact_plan=NativePlan(),
        roots_by_module={"support": ("used",)},
        resolution_cache=_ModuleResolutionCache(),
        artifacts_root=artifacts,
        slice_cache={},
        operation_counts=counts,
    )[source]
    assert result.scan.authority.source_path == source
    assert (
        result.scan.authority.source_sha256
        == hashlib.sha256(source.read_bytes()).hexdigest()
    )
    assert result.scan.authority.native_support_function_roots == ("used",)
    assert result.scan.authority.mode == "full"
    assert result.scan.authority.package_paths == (
        (source.parent,) if source.name == "__init__.py" else ()
    )
    assert "leaf" in result.scan.scan.imports and "dead" not in result.scan.scan.imports
    assert result.scan.scan.source_executions == ()
    assert "def unused" in source.read_text(encoding="utf-8")
    assert not tuple(artifacts.glob("_molt_generated_*.py"))
    assert counts["native_support_source_parses"] == 1
    assert counts["native_support_source_prunes"] == 1


def test_generated_sources_are_content_addressed_without_self_referential_identity(
    tmp_path: Path,
) -> None:
    first = graphs._write_generated_module_source("generated", "VALUE = 1\n", tmp_path)
    assert (
        graphs._write_generated_module_source("generated", "VALUE = 1\n", tmp_path)
        == first
    )
    second = graphs._write_generated_module_source("generated", "VALUE = 2\n", tmp_path)
    assert first != second
    assert first.read_text(encoding="utf-8") == "VALUE = 1\n"
    importer = graphs._write_importer_module(tmp_path)
    assert graphs._write_importer_module(tmp_path) == importer
    namespace = graphs._write_namespace_module("pkg", ["/source/pkg"], tmp_path)
    assert graphs._write_namespace_module("pkg", ["/source/pkg"], tmp_path) == namespace
    for path in (first, second, importer, namespace):
        assert path.name not in path.read_text(encoding="utf-8")


@pytest.mark.parametrize("source_name", ["support.py", "support/__init__.py"])
def test_native_view_growth_reuses_one_live_solver_and_preserves_later_source_key(
    tmp_path: Path,
    source_name: str,
) -> None:
    text = (
        "# Original coordinates are part of source custody.\n\n"
        "def first():\n    import leaf1\n\n\n"
        "def later():\n    import leaf2\n\n"
        "def unused():\n    import dead\n"
    )
    source = _source(tmp_path / source_name, text)
    leaf1 = _source(tmp_path / "leaf1.py", "pass\n")
    leaf2 = _source(tmp_path / "leaf2.py", "pass\n")
    _source(tmp_path / "dead.py", "pass\n")
    snapshot = PythonSourceSnapshot.capture(source)
    expected = ast.parse(text, filename=str(source))
    expected_later = next(
        node
        for node in expected.body
        if isinstance(node, ast.FunctionDef) and node.name == "later"
    )
    expected_key = python_node_source_key(expected_later)
    original_namespace = {}
    exec(compile(expected, str(source), "exec"), original_namespace)
    cache = _ModuleResolutionCache()
    state = discovery._ModuleGraphDiscovery(cache, PYTHON)
    graph, roles = {}, {}
    loaded = []
    solver = None
    for roots in (("later",), ("first", "later"), ("first", "later")):
        result = discovery._load_module_import_scan(
            source,
            module_name="support",
            is_package=source.name == "__init__.py",
            import_scan_mode="full",
            resolution_cache=cache,
            project_root=None,
            source_snapshot=snapshot,
            native_support_function_roots=roots,
            target_python=PYTHON,
        )
        loaded.append(result)
        record = discovery._bind_precomputed_module_import_scan(
            source,
            module_name="support",
            import_scan_mode="full",
            is_package=source.name == "__init__.py",
            package_paths=(source.parent,) if source.name == "__init__.py" else (),
            scan=result.scan,
            snapshot=snapshot,
            native_support_function_roots=roots,
            target_python=PYTHON,
        )
        closure = discovery._discover_module_graph_from_paths(
            (source,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            None,
            set(),
            full_scan_roots=True,
            resolver_cache=cache,
            discovery=state,
            precomputed_scans_by_path={source: record},
            enclosing_scan_authority=_ModuleGraphScanAuthority(tuple(roles.values())),
        )
        discovery._merge_discovered_module_graph(graph, roles, closure)
        assert closure.discovery is state
        if solver is None:
            solver = state.demands
            assert solver is not None
        else:
            assert state.demands is solver
        assert graph == (
            {"support": source, "leaf2": leaf2}
            if roots == ("later",)
            else {"support": source, "leaf1": leaf1, "leaf2": leaf2}
        )
        assert roles["support"].native_support_function_roots == roots
        assert roles["support"].source_sha256 == snapshot.sha256
        assert expected_key in {
            row.owner for row in result.scan.source_values.functions
        }
        assert isinstance(result.tree, ast.Module)
        later = next(
            node
            for node in result.tree.body
            if isinstance(node, ast.FunctionDef) and node.name == "later"
        )
        assert python_node_source_key(later) == expected_key
        selected_namespace = {}
        exec(compile(result.tree, str(source), "exec"), selected_namespace)
        assert (
            selected_namespace["later"].__code__.co_firstlineno
            == original_namespace["later"].__code__.co_firstlineno
        )
        assert "unused" not in selected_namespace
        assert result.snapshot is snapshot
    assert [result.source_parsed for result in loaded] == [True, False, False]
    assert [result.source_pruned for result in loaded] == [True, True, False]
    # The two dependency modules each need one additional original-source parse.
    assert cache.source_parse_count == 3
    assert len(cache.native_support_view_cache) == 1
    assert [set(result.scan.imports) for result in loaded] == [
        {"leaf2"},
        {"leaf1", "leaf2"},
        {"leaf1", "leaf2"},
    ]
    assert source.read_text(encoding="utf-8") == text
    assert snapshot.sha256 == hashlib.sha256(text.encode()).hexdigest()
    assert tuple(tmp_path.glob("_molt_generated_*.py")) == ()


@pytest.mark.parametrize("selected_first", [False, True])
@pytest.mark.parametrize("acquisition", ["import", "execute"])
def test_native_selected_source_cycle_preserves_raw_acquisition_in_both_seed_orders(
    tmp_path: Path,
    selected_first: bool,
    acquisition: str,
) -> None:
    support = _source(
        tmp_path / "support.py",
        "import initializer_leaf\nINITIALIZED = True\n"
        "def later():\n    import selected_leaf\n    import consumer\n",
    )
    initializer = _source(tmp_path / "initializer_leaf.py", "pass\n")
    selected = _source(tmp_path / "selected_leaf.py", "pass\n")
    consumer = _source(
        tmp_path / "consumer.py",
        "import support\n"
        if acquisition == "import"
        else "import importlib.util\n"
        + f"spec = importlib.util.spec_from_file_location('support', {str(support)!r})\n"
        + "support = importlib.util.module_from_spec(spec)\n"
        + "spec.loader.exec_module(support)\n",
    )
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-c",
            "import sys,json; sys.path.insert(0,sys.argv[1]); import consumer; "
            "consumer.support.later(); print(json.dumps(["
            "consumer.support.INITIALIZED, 'initializer_leaf' in sys.modules, "
            "'selected_leaf' in sys.modules]))",
            str(tmp_path),
        ],
        timeout=10,
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(observed.stdout) == [True, True, True]
    snapshot = PythonSourceSnapshot.capture(support)
    cache = _ModuleResolutionCache()
    state = discovery._ModuleGraphDiscovery(cache, PYTHON)
    loaded = discovery._load_module_import_scan(
        support,
        module_name="support",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=cache,
        project_root=None,
        source_snapshot=snapshot,
        native_support_function_roots=("later",),
        target_python=PYTHON,
    )
    assert "initializer_leaf" not in loaded.scan.imports
    record = discovery._bind_precomputed_module_import_scan(
        support,
        module_name="support",
        import_scan_mode="full",
        scan=loaded.scan,
        snapshot=snapshot,
        native_support_function_roots=("later",),
        target_python=PYTHON,
    )
    result = discovery._discover_module_graph_from_paths(
        (support, consumer) if selected_first else (consumer, support),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=True,
        resolver_cache=cache,
        discovery=state,
        precomputed_scans_by_path={support: record},
    )
    expected = {
        "support": support,
        "consumer": consumer,
        "initializer_leaf": initializer,
        "selected_leaf": selected,
    }
    assert result.graph == state.graph == expected
    owner = result.scan_authority.by_module["support"]
    assert owner.native_support_function_roots == ()
    assert owner.mode == "full"
    assert owner.source_sha256 == snapshot.sha256
    solver = state.demands
    assert solver is not None
    repeated = discovery._discover_module_graph_from_paths(
        (support,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=True,
        resolver_cache=cache,
        discovery=state,
        precomputed_scans_by_path={support: record},
    )
    assert state.demands is solver
    assert state.graph == expected
    assert repeated.scan_authority.by_module["support"] == owner
    assert snapshot.content == support.read_bytes()
    assert tuple(tmp_path.glob("_molt_generated_*.py")) == ()


def test_native_slice_absent_demand_does_not_read_manifest_helpers(
    tmp_path: Path,
) -> None:
    class NativePlan:
        def support_source_paths_by_module(self):
            return {"unused": tmp_path / "missing.py"}

    cache = _ModuleResolutionCache()
    counts = {}
    assert (
        graphs._native_support_source_slices(
            native_artifact_plan=NativePlan(),
            roots_by_module={},
            resolution_cache=cache,
            artifacts_root=tmp_path / "artifacts",
            slice_cache={},
            operation_counts=counts,
        )
        == {}
    )
    assert counts == {}
    assert cache.source_parse_count == 0
    assert cache.source_cache == {}


def test_native_runtime_import_requests_share_live_discovery_without_generated_source(
    tmp_path: Path,
) -> None:
    first = _source(tmp_path / "first.py", "pass\n")
    second = _source(tmp_path / "second.py", "pass\n")

    class NativePlan(_ExternalPackageNativeArtifactPlan):
        def runtime_python_imports(self):
            return frozenset(requested)

    cache = _ModuleResolutionCache()
    state = discovery._ModuleGraphDiscovery(cache, PYTHON)
    graph, roles = {}, {}
    artifacts = tmp_path / "generated"
    for requested in ({"first"}, {"first", "second"}):
        graphs._extend_native_runtime_python_import_closure(
            discovery=state,
            module_graph=graph,
            scan_authorities=roles,
            module_reasons={},
            native_artifact_plan=NativePlan(),
            roots=[tmp_path],
            stdlib_root=tmp_path / "stdlib",
            stdlib_allowlist=set(),
            resolver_cache=cache,
            target_python=PYTHON,
        )
    assert graph == {"first": first, "second": second}
    assert set(roles) == {"first", "second"}
    assert all(source.mode == "module_init" for source in roles.values())
    assert state.graph == graph
    assert cache.source_parse_count == 2
    generations = tuple(artifacts.glob("_molt_generated_*.py"))
    assert generations == ()


@pytest.mark.parametrize("source_name", ["support.py", "support/__init__.py"])
def test_native_slice_root_growth_retains_source_and_dependency_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    source_name: str,
) -> None:
    support = _source(
        tmp_path / source_name,
        "def first():\n    import leaf1\n\ndef second():\n    import leaf2\n",
    )
    _source(tmp_path / "leaf1.py", "pass\n")
    _source(tmp_path / "leaf2.py", "pass\n")

    class NativePlan(_ExternalPackageNativeArtifactPlan):
        def support_source_paths_by_module(self):
            return {"support": support}

    selected = ("first",)
    monkeypatch.setattr(
        graphs,
        "_native_support_function_roots_by_module",
        lambda *args, **kwargs: {"support": selected},
    )
    counts = {
        name: 0
        for name in (
            "native_support_slice_requests",
            "native_support_legacy_equivalent_source_parses",
            "native_support_slice_cache_hits",
            "native_support_slice_cache_misses",
            "native_support_persisted_import_scan_hits",
            "native_support_persisted_import_scan_misses",
            "native_support_source_parses",
            "native_support_source_prunes",
        )
    }
    graph, roles, slices = {}, {}, {}
    cache = _ModuleResolutionCache()
    common = dict(
        discovery=discovery._ModuleGraphDiscovery(cache, PYTHON),
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        native_artifact_plan=NativePlan(),
        artifacts_root=tmp_path / "generated",
        roots=[tmp_path],
        stdlib_root=tmp_path / "stdlib",
        stdlib_allowlist=set(),
        resolver_cache=cache,
        target_python=PYTHON,
        slice_cache=slices,
        operation_counts=counts,
    )
    graphs._extend_native_support_source_closure(**common)
    assert graph["support"] == support
    first_sha = roles["support"].source_sha256
    assert roles["support"].native_support_function_roots == ("first",)
    assert "leaf1" in graph and "leaf2" not in graph
    selected = ("first", "second")
    graphs._extend_native_support_source_closure(**common)
    assert graph["support"] == support
    assert roles["support"].source_sha256 == first_sha
    assert roles["support"].native_support_function_roots == ("first", "second")
    assert {"leaf1", "leaf2"} <= set(graph)
    counts_after_growth = dict(counts)
    graphs._extend_native_support_source_closure(**common)
    assert graph["support"] == support
    assert (
        counts["native_support_source_parses"]
        == counts_after_growth["native_support_source_parses"]
    )
    assert (
        counts["native_support_source_prunes"]
        == counts_after_growth["native_support_source_prunes"]
    )
    assert not tuple((tmp_path / "generated").glob("_molt_generated_*.py"))
    consumer = _source(tmp_path / "consumer.py", "import support\n")
    result = discovery._discover_module_graph_from_paths(
        (consumer,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=True,
        resolver_cache=cache,
        discovery=common["discovery"],
        enclosing_scan_authority=_ModuleGraphScanAuthority(tuple(roles.values())),
    )
    assert result.graph["support"] == support
    assert set(result.graph) == {"consumer", "support", "leaf1", "leaf2"}
    discovery._merge_discovered_module_graph(graph, roles, result)
    assert roles["support"].mode == "full"
    assert roles["support"].native_support_function_roots == ()
    assert roles["support"].source_sha256 == first_sha
    assert roles["support"].package_paths == (
        (support.parent,) if support.name == "__init__.py" else ()
    )


@pytest.mark.parametrize("raw_first", [False, True])
def test_native_raw_source_dominates_selected_roots_without_losing_depth(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    raw_first: bool,
) -> None:
    support = _source(
        tmp_path / "support.py",
        "import initializer\n\ndef used():\n    import selected\n\ndef unused():\n    import other\n",
    )
    for name in ("initializer", "selected", "other"):
        _source(tmp_path / f"{name}.py", "pass\n")
    raw = raw_first

    class NativePlan(_ExternalPackageNativeArtifactPlan):
        def support_source_paths_by_module(self):
            return {"support": support}

        def runtime_python_import_module_names(self):
            return frozenset({"support"}) if raw else frozenset()

    monkeypatch.setattr(
        graphs,
        "_native_support_function_roots_by_module",
        lambda *args, **kwargs: {"support": ("used",)},
    )
    counts = {
        name: 0
        for name in (
            "native_support_slice_requests",
            "native_support_legacy_equivalent_source_parses",
            "native_support_slice_cache_hits",
            "native_support_slice_cache_misses",
            "native_support_persisted_import_scan_hits",
            "native_support_persisted_import_scan_misses",
            "native_support_source_parses",
            "native_support_source_prunes",
        )
    }
    graph, roles = {}, {}
    cache = _ModuleResolutionCache()
    common = dict(
        discovery=discovery._ModuleGraphDiscovery(cache, PYTHON),
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        native_artifact_plan=NativePlan(),
        artifacts_root=tmp_path / "artifacts",
        roots=[tmp_path],
        stdlib_root=tmp_path / "stdlib",
        stdlib_allowlist=set(),
        resolver_cache=cache,
        target_python=PYTHON,
        slice_cache={},
        operation_counts=counts,
    )
    graphs._extend_native_support_source_closure(**common)
    assert roles["support"].native_support_function_roots == (
        () if raw_first else ("used",)
    )
    assert set(graph) == (
        {"support", "initializer"} if raw_first else {"support", "selected"}
    )
    raw = not raw
    graphs._extend_native_support_source_closure(**common)
    assert graph["support"] == support
    assert (
        roles["support"].source_sha256
        == hashlib.sha256(support.read_bytes()).hexdigest()
    )
    assert roles["support"].native_support_function_roots == ()
    assert roles["support"].mode == ("module_init" if raw_first else "full")
    assert set(graph) == (
        {"support", "initializer"}
        if raw_first
        else {"support", "initializer", "selected", "other"}
    )
    assert tuple((tmp_path / "artifacts").glob("_molt_generated_*.py")) == ()


def test_graph_cache_binds_admitted_source_generation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = _source(tmp_path / "entry.py", "import support\n")
    original = _source(tmp_path / "support.py", "SHOULD_NOT_BE_SELECTED = True\n")
    first = graphs._write_generated_module_source(
        "support", "VALUE = 1\n", tmp_path / "generated"
    )
    second = graphs._write_generated_module_source(
        "support", "VALUE = 2\n", tmp_path / "generated"
    )
    monkeypatch.setattr(
        graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "generation"
    )

    def discover(generation):
        return discovery._discover_module_graph_from_paths(
            (entry,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
            enclosing_scan_authority=_ModuleGraphScanAuthority(
                (_ModuleSourceScanAuthority("support", generation, "full"),)
            ),
        )

    for generation in (first, second):
        assert discover(generation).graph["support"] == generation

    def forbidden(*args, **kwargs):
        raise AssertionError("generation-specific graph receipt should be warm")

    monkeypatch.setattr(_source_import_scan, "_collect_import_scan_requests", forbidden)
    for generation in (first, second):
        assert discover(generation).graph["support"] == generation != original


def test_complete_scan_collector_forwards_and_keys_target_python(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = _source(tmp_path / "entry.py", "pass\n")
    tree = ast.parse("pass\n")
    cache = _ModuleResolutionCache()
    calls = []

    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission

    def collector(
        tree: ast.AST,
        module_name: str | None,
        is_package: bool,
        *,
        import_scan_mode: str,
        source_owner=None,
        source_arguments=(),
        target_python: TargetPythonVersion,
        ast_digest_admission: _PythonAstDigestAdmission,
        namespace_premise,
    ) -> _ImportDiscoveryProjection:
        assert source_owner is None
        assert source_arguments == ()
        assert ast_digest_admission.tree is tree
        assert namespace_premise is None
        calls.append(target_python.tag)
        return _ImportDiscoveryProjection((target_python.tag,))

    for target in (
        TargetPythonVersion(3, 12, 0),
        TargetPythonVersion(3, 13, 0),
        TargetPythonVersion(3, 12, 0),
    ):
        assert cache.collect_graph_imports(
            source,
            tree,
            collector=collector,
            module_name="entry",
            target_python=target,
        ).imports == (target.tag,)
    assert calls == ["py312", "py313"]

    monkeypatch.setattr(_source_import_scan, "_collect_imports_for_graph", collector)
    target = TargetPythonVersion(3, 13, 0)
    loaded = discovery._load_module_import_scan(
        source,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=_ModuleResolutionCache(),
        project_root=None,
        tree=tree,
        source="pass\n",
        target_python=target,
    )
    assert loaded.scan.imports == (target.tag,)
    assert calls[-1] == target.tag


@pytest.mark.parametrize("source_kind", ["admitted", "precomputed"])
def test_generated_package_graph_cache_validates_source_role_authority(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    source_kind: str,
) -> None:
    entry = _source(tmp_path / "entry.py", "import pkg\n")
    _source(tmp_path / "pkg" / "__init__.py", "ORIGINAL = True\n")
    child = _source(tmp_path / "pkg" / "child.py", "pass\n")
    generated = graphs._write_generated_module_source(
        "pkg",
        "from . import child\n",
        tmp_path / "generated",
    )
    authority = _ModuleSourceScanAuthority(
        "pkg",
        generated,
        "full",
        True,
        package_paths=(child.parent,),
    )
    source_inputs = {}
    root = entry
    if source_kind == "admitted":
        source_inputs["enclosing_scan_authority"] = _ModuleGraphScanAuthority(
            (authority,)
        )
    else:
        root = generated
        source_inputs["precomputed_scans_by_path"] = {
            generated.resolve(): discovery._bind_precomputed_module_import_scan(
                generated,
                snapshot=PythonSourceSnapshot.capture(generated),
                module_name="pkg",
                import_scan_mode="full",
                is_package=True,
                package_paths=(child.parent,),
                scan=_CompleteImportScan(
                    _ImportScanRequests(("pkg.child",), ()), ("pkg.child",), ()
                ),
                target_python=PYTHON,
            ),
        }
    monkeypatch.setattr(
        graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "package-role"
    )

    def discover():
        return discovery._discover_module_graph_from_paths(
            (root,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
            **source_inputs,
        )

    first = discover()
    assert first.graph["pkg.child"] == child
    assert first.scan_authority.by_module["pkg"].is_package is True

    def cold_scan(*args, **kwargs):
        raise AssertionError("cold source scan")

    monkeypatch.setattr(_source_import_scan, "_collect_import_scan_requests", cold_scan)
    warm = discover()
    assert warm.graph == first.graph
    assert warm.scan_authority == first.scan_authority
    assert not hasattr(graph_cache, "_read_persisted_module_graph")
    assert not hasattr(graph_cache, "_write_persisted_module_graph")


def test_precomputed_star_scan_recompletes_package_exports_without_reparsing_entry(
    tmp_path, monkeypatch
):
    entry = _source(tmp_path / "entry.py", "from pkg import *\n")
    package = _source(tmp_path / "pkg" / "__init__.py", "__all__ = ['first']\n")
    _source(tmp_path / "pkg" / "first.py", "pass\n")
    _source(tmp_path / "pkg" / "second.py", "pass\n")
    first = discovery._load_module_import_scan(
        entry,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=_ModuleResolutionCache(),
        project_root=None,
        roots=[tmp_path],
        stdlib_root=tmp_path / "stdlib",
        stdlib_allowlist=set(),
        target_python=PYTHON,
    )
    record = discovery._bind_precomputed_module_import_scan(
        entry,
        module_name="entry",
        import_scan_mode="full",
        scan=first.scan,
        snapshot=first.snapshot,
        target_python=PYTHON,
    )
    package.write_text("__all__ = ['second']\n", encoding="utf-8")

    def forbidden(*args, **kwargs):
        raise AssertionError("precomputed entry source was rescanned")

    monkeypatch.setattr(_source_import_scan, "_collect_import_scan_requests", forbidden)
    second = discovery._load_module_import_scan(
        entry,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=_ModuleResolutionCache(),
        project_root=None,
        precomputed_scan=record,
        roots=[tmp_path],
        stdlib_root=tmp_path / "stdlib",
        stdlib_allowlist=set(),
        target_python=PYTHON,
    )
    assert "pkg.first" in first.scan.imports and "pkg.second" not in first.scan.imports
    assert (
        "pkg.second" in second.scan.imports and "pkg.first" not in second.scan.imports
    )


def test_package_search_paths_are_canonical_ordered_source_authority(tmp_path):
    source = _source(tmp_path / "pkg" / "__init__.py", "pass\n")
    generated = _source(tmp_path / "generated.py", "pass\n")
    second_root = tmp_path / "second" / "pkg"
    second_root.mkdir(parents=True)
    regular = _ModuleSourceScanAuthority("pkg", source, "full")
    assert regular.package_paths == (source.parent,)
    roots = (source.parent, second_root.resolve())
    generated_owner = _ModuleSourceScanAuthority(
        "pkg",
        generated,
        "full",
        is_package=True,
        package_paths=(source.parent / ".", second_root, source.parent),
    )
    assert generated_owner.package_paths == roots
    assert (
        replace(generated_owner, mode="module_init").package_paths
        is generated_owner.package_paths
    )
    assert _ModuleGraphScanAuthority((generated_owner,)).payload()[0][
        "package_paths"
    ] == [str(path) for path in roots]
    for changed_roots in (roots[::-1], (source.parent,)):
        changed = _ModuleSourceScanAuthority(
            "pkg",
            generated,
            "full",
            is_package=True,
            package_paths=changed_roots,
        )
        with pytest.raises(ValueError, match="package search paths changed"):
            _ModuleGraphScanAuthority((generated_owner,)).merged(
                _ModuleGraphScanAuthority((changed,))
            )
        with pytest.raises(ValueError, match="package search paths changed"):
            discovery._merge_discovered_module_graph(
                {"pkg": generated},
                {"pkg": generated_owner},
                discovery._DiscoveredModuleGraph(
                    {"pkg": generated},
                    set(),
                    _ModuleGraphScanAuthority((changed,)),
                ),
            )
    with pytest.raises(ValueError, match="lacks original search paths"):
        _ModuleSourceScanAuthority("pkg", generated, "full", is_package=True)
    with pytest.raises(ValueError, match="non-package"):
        _ModuleSourceScanAuthority("plain", generated, "full", package_paths=roots)


def test_generated_package_discovery_uses_original_paths_and_reuses_child_inventory(
    tmp_path,
    monkeypatch,
):

    roots = (tmp_path / "first" / "pkg", tmp_path / "second" / "pkg")
    _source(roots[0] / "alpha.py", "pass\n")
    _source(roots[1] / "beta.py", "pass\n")
    generated = _source(tmp_path / "artifacts" / "package.digest.py", "pass\n")
    _source(generated.parent / "artifact_only.py", "pass\n")
    snapshot = PythonSourceSnapshot.capture(generated)
    loaded = discovery._load_module_import_scan(
        generated,
        module_name="pkg",
        is_package=True,
        import_scan_mode="full",
        resolution_cache=_ModuleResolutionCache(),
        project_root=None,
        source_snapshot=snapshot,
        target_python=PYTHON,
    )
    bound = discovery._bind_precomputed_module_import_scan(
        generated,
        module_name="pkg",
        is_package=True,
        package_paths=roots,
        import_scan_mode="full",
        scan=loaded.scan,
        snapshot=snapshot,
        target_python=PYTHON,
    )
    visits = []
    child_names = discovery.python_package_child_names

    def counted(paths, *, import_topology):
        visits.append(tuple(paths))
        return child_names(paths, import_topology=import_topology)

    monkeypatch.setattr(discovery, "python_package_child_names", counted)
    monkeypatch.setattr(
        graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "package-paths"
    )
    expected = None
    for _ in range(2):
        visits.clear()
        result = discovery._discover_module_graph_from_paths(
            (generated,),
            [tmp_path / "first", tmp_path / "second"],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            precomputed_scans_by_path={generated: bound},
            full_scan_roots=True,
        )
        owner = result.scan_authority.by_module["pkg"]
        assert owner.package_paths == roots
        assert owner.namespace_premise == PythonNamespacePremise(("alpha", "beta"), 0)
        assert set(result.graph) == {"pkg"}
        assert visits == [roots]
        if expected is None:
            expected = result.scan_authority

            def forbidden(*args, **kwargs):
                raise AssertionError("warm package discovery repeated source analysis")

            monkeypatch.setattr(
                _source_import_scan, "_collect_import_scan_requests", forbidden
            )
        else:
            assert result.scan_authority == expected


@pytest.mark.parametrize("namespace_writer", [False, True])
def test_materialized_namespace_package_identity_matches_cpython(
    tmp_path, namespace_writer
):
    for name in (
        "importlib/__init__.py",
        "importlib/util.py",
        "importlib/machinery.py",
    ):
        _source(tmp_path / "stdlib" / name, "pass\n")
    roots = [tmp_path / "first", tmp_path / "second"]
    _source(roots[0] / "hf226_namespace" / "alpha.py", "pass\n")
    _source(roots[1] / "hf226_namespace" / "beta.py", "pass\n")
    if namespace_writer:
        _source(
            tmp_path / "writer.py",
            "import hf226_namespace\nhf226_namespace.marker = True\n",
        )
    entry = _source(
        tmp_path / "entry.py",
        ("import writer\n" if namespace_writer else "")
        + "import hf226_namespace.alpha\nimport hf226_namespace.beta\n",
    )
    initial = discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path, *roots],
        [tmp_path, *roots],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=True,
    )
    assert initial.discovery is not None
    # The writer has no source path for its namespace receiver until the real
    # package materialization joins this same discovery operation.
    assert "hf226_namespace" not in initial.graph
    prepared = _PreparedEntryModuleGraph(
        discovery=initial.discovery,
        image_scope=_BinaryImageScope.from_entry(
            kind="entry_script",
            selector_source="test:namespace",
            entry_module="entry",
            source_path=entry,
            project_root=tmp_path,
            module_roots=[tmp_path, *roots],
        ),
        declared_root_modules=frozenset({"entry"}),
        stdlib_allowlist={"importlib"},
        roots=[tmp_path, *roots],
        module_resolution_cache=initial.discovery.resolver_cache,
        module_graph=initial.graph,
        scan_authority=initial.scan_authority,
        target="native",
        explicit_imports=initial.explicit_imports,
        runtime_import_dispatch_roots=frozenset(),
        stub_parents=set(),
        spawn_enabled=False,
        runtime_import_support_policy=_RuntimeImportSupportPolicy(False, False),
        native_artifact_plan=_EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
        target_python=PYTHON,
    )
    plan = graphs._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons={},
        stdlib_root=tmp_path / "stdlib",
        artifacts_root=tmp_path / "artifacts",
        entry_module="entry",
        diagnostics_enabled=False,
    )
    reference = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-c",
            "import sys,json; sys.path[:0] = json.loads(sys.argv[1]); "
            "import entry,hf226_namespace; "
            "print(json.dumps([list(hf226_namespace.__path__), "
            "list(hf226_namespace.__spec__.submodule_search_locations), "
            "getattr(hf226_namespace, 'marker', False)]))",
            json.dumps([str(tmp_path), *(str(p) for p in roots)]),
        ],
        timeout=30,
        check=True,
    )
    paths, spec_paths, marker = json.loads(reference.stdout)
    assert marker is namespace_writer
    owner = plan.scan_authority.by_module["hf226_namespace"]
    assert owner.is_package
    assert [str(path) for path in owner.package_paths] == paths == spec_paths
    assert (
        owner.source_sha256
        == hashlib.sha256(owner.source_path.read_bytes()).hexdigest()
    )
    assert owner.namespace_premise == PythonNamespacePremise(
        ("alpha", "beta", "marker") if namespace_writer else ("alpha", "beta"),
        0,
    )
    metadata = plan.module_graph_metadata
    assert metadata.module_is_package_by_module["hf226_namespace"]
    assert metadata.module_is_namespace_by_module["hf226_namespace"]
    namespace = {}
    exec(owner.source_path.read_bytes(), namespace)
    assert namespace["__path__"] == paths
    assert (
        metadata.logical_source_path_by_module["hf226_namespace"]
        == "/__molt_generated__/hf226_namespace.py"
    )
