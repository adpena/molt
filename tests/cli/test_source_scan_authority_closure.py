from __future__ import annotations

import ast
from pathlib import Path
from unittest.mock import Mock

import pytest

from molt.cli import module_graph as graphs
from molt.cli import module_graph_cache as graph_cache
from molt.cli import module_graph_discovery as discovery
from molt.cli import module_import_scanner as scanner
from molt.cli.module_source import PythonSourceSnapshot
from molt.cli.models import (
    _ImportScanRequests,
    _BinaryImageScope,
    _CompleteImportScan,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ExternalPackageNativeArtifactPlan,
    _ImportDiscoveryProjection,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _PreparedEntryModuleGraph,
    _RuntimeImportSupportPolicy,
)
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION as PYTHON,
)


def _source(path: Path, source: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8")
    return path.resolve()


def _discover(path: Path, root: Path, *, full: bool, project: Path | None = None):
    return discovery._discover_module_graph_from_paths(
        (path,),
        [root, root / "stdlib"],
        [root],
        root / "stdlib",
        project,
        {"runtime_owner"},
        full_scan_roots=full,
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

    monkeypatch.setattr(scanner, "_collect_import_scan_requests", forbidden)
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
    "left_mode", ["module_init", "module_init_static_helpers", "full"]
)
@pytest.mark.parametrize(
    "right_mode", ["module_init", "module_init_static_helpers", "full"]
)
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
    modes = ["module_init", "module_init_static_helpers", "full"]
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
    assert not cache.ast_cache


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
    prepared = _PreparedEntryModuleGraph(
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
        module_resolution_cache=_ModuleResolutionCache(),
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
        result = _discover(late, tmp_path, full=True)
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
    common = dict(
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        module_resolution_cache=_ModuleResolutionCache(),
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
        graph, roles, _discover(late, tmp_path, full=True)
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
    refreshed = graphs._finalize_runtime_import_closure(
        **common,
        explicit_imports={"leaf", "late"},
        dispatch_roots=second.dispatch_roots,
        previous_custody=second.custody,
    )
    assert refreshed.custody is not None
    assert refreshed.custody.owner_ast_digests != second.custody.owner_ast_digests
    # Source-keyed AST memoization also makes a fresh operation agree.
    common["module_resolution_cache"] = _ModuleResolutionCache()
    changed = graphs._finalize_runtime_import_closure(
        **common,
        explicit_imports={"leaf", "late"},
        dispatch_roots=second.dispatch_roots,
        previous_custody=second.custody,
    )
    assert changed.custody is not None and changed.custody != second.custody
    assert changed.custody.owners == second.custody.owners
    assert changed.custody.catalog == second.custody.catalog
    assert changed.custody.owner_ast_digests != second.custody.owner_ast_digests
    assert changed.custody == refreshed.custody
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
    graph = {"app.entry": entry}
    roles = {
        "app.entry": _ModuleSourceScanAuthority("app.entry", entry, "full", False),
    }
    common = dict(
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        explicit_imports={"support.owner"},
        dispatch_roots={"support.owner"},
        module_resolution_cache=_ModuleResolutionCache(),
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
    graph = {"entry": entry, "pkg.nested.owner": owner}
    roles = {
        "entry": _ModuleSourceScanAuthority("entry", entry, "full", False),
        "pkg.nested.owner": _ModuleSourceScanAuthority(
            "pkg.nested.owner",
            owner,
            "module_init",
            False,
            True,
        ),
    }

    closure = graphs._finalize_runtime_import_closure(
        module_graph=graph,
        scan_authorities=roles,
        module_reasons={},
        explicit_imports={"pkg.nested.owner"},
        dispatch_roots={"pkg.nested.owner"},
        module_resolution_cache=_ModuleResolutionCache(),
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


def test_generated_native_slice_carries_one_source_for_both_scan_projections(
    tmp_path: Path,
) -> None:
    source = _source(
        tmp_path / "support.py",
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
        artifacts_root=artifacts,
        slice_cache={},
        operation_counts=counts,
    )[source]
    assert result.generated_path is not None
    assert result.scan.authority.source_path == result.generated_path.resolve()
    assert result.scan.authority.mode == "full"
    assert "leaf" in result.scan.scan.imports and "dead" not in result.scan.scan.imports
    assert result.scan.scan.source_executions == ()
    assert "unused" not in result.generated_path.read_text(encoding="utf-8")


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


def test_native_runtime_import_generations_share_resolver_without_stale_source(
    tmp_path: Path,
) -> None:
    first = _source(tmp_path / "first.py", "pass\n")
    second = _source(tmp_path / "second.py", "pass\n")

    class NativePlan(_ExternalPackageNativeArtifactPlan):
        def runtime_python_imports(self):
            return frozenset(requested)

    cache = _ModuleResolutionCache()
    graph, roles = {}, {}
    artifacts = tmp_path / "generated"
    for requested in ({"first"}, {"first", "second"}):
        graphs._extend_native_runtime_python_import_closure(
            module_graph=graph,
            scan_authorities=roles,
            module_reasons={},
            native_artifact_plan=NativePlan(),
            artifacts_root=artifacts,
            roots=[tmp_path],
            stdlib_root=tmp_path / "stdlib",
            stdlib_allowlist=set(),
            resolver_cache=cache,
            target_python=PYTHON,
            capability_config_digest="",
        )
    assert graph == {"first": first, "second": second}
    assert set(roles) == {"first", "second"}
    assert all(source.mode == "module_init" for source in roles.values())
    generations = tuple(artifacts.glob("_molt_generated_*.py"))
    assert len(generations) == 2
    assert {path.read_text(encoding="utf-8") for path in generations} == {
        "import first\n",
        "import second\n",
    }


def test_native_slice_generation_transfer_retains_roots_and_dependency_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    support = _source(
        tmp_path / "support.py",
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
        capability_config_digest="",
        slice_cache=slices,
        operation_counts=counts,
    )
    graphs._extend_native_support_source_closure(**common)
    first_generation = graph["support"]
    selected = ("first", "second")
    graphs._extend_native_support_source_closure(**common)
    second_generation = graph["support"]
    assert second_generation != first_generation
    assert "def second" not in first_generation.read_text(encoding="utf-8")
    assert "def second" in second_generation.read_text(encoding="utf-8")
    assert {"leaf1", "leaf2"} <= set(graph)
    graphs._extend_native_support_source_closure(**common)
    assert graph["support"] == second_generation
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
        enclosing_scan_authority=_ModuleGraphScanAuthority(tuple(roles.values())),
    )
    assert result.graph["support"] == second_generation
    assert set(result.graph) == {"consumer", "support"}
    discovery._merge_discovered_module_graph(graph, roles, result)
    assert roles["support"].mode == "full"


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

    monkeypatch.setattr(scanner, "_collect_import_scan_requests", forbidden)
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

    from molt.compiler_analysis.python_imports import _PythonAstDigestAdmission

    def collector(
        tree: ast.AST,
        module_name: str | None,
        is_package: bool,
        *,
        import_scan_mode: str,
        target_python: TargetPythonVersion,
        ast_digest_admission: _PythonAstDigestAdmission,
    ) -> _ImportDiscoveryProjection:
        assert ast_digest_admission.tree is tree
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

    monkeypatch.setattr(scanner, "_collect_imports_for_graph", collector)
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
    authority = _ModuleSourceScanAuthority("pkg", generated, "full", True)
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
                scan=_CompleteImportScan(("pkg.child",), ()),
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

    monkeypatch.setattr(scanner, "_collect_import_scan_requests", cold_scan)
    warm = discover()
    assert warm.graph == first.graph
    assert warm.scan_authority == first.scan_authority
    assert not hasattr(graph_cache, "_read_persisted_module_graph")
    assert not hasattr(graph_cache, "_write_persisted_module_graph")
