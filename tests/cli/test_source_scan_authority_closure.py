from __future__ import annotations
from molt.python_environment_location import environment_bytecode_arguments

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

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


# Synthetic fingerprints and cache-hit oracles need exclusive test cache state.
pytestmark = pytest.mark.usefixtures("isolated_molt_cache")


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
@pytest.mark.parametrize(
    "helper_source",
    [
        "def invoke():\n    return __import__('leaf')\n",
        "from _intrinsics import require_intrinsic as acquire\n"
        "load = acquire('molt_importlib_import_module')\n"
        "def invoke():\n    return load('leaf', None)\n",
    ],
)
def test_role_receipt_survives_cache_hits_and_protocol_detection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    first_full: bool,
    helper_source: str,
) -> None:
    entry = _source(tmp_path / "entry.py", "pass\n")
    helper = _source(tmp_path / "helper.py", helper_source)
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
                (
                    _ModuleSourceScanAuthority(
                        "entry",
                        entry,
                        "full",
                        body_selection=ModuleBodySelection("complete_source"),
                    ),
                )
            )
        )
        policy = scanner._module_graph_needs_runtime_import_support(
            module_graph=graph,
            scan_authority=authority,
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
                (
                    _ModuleSourceScanAuthority(
                        "helper",
                        other,
                        "full",
                        body_selection=ModuleBodySelection("complete_source"),
                    ),
                )
            )
        )


@pytest.mark.parametrize(
    ("left", "right", "expected"),
    [
        (None, PythonNamespacePremise((), 0, 0, ()), None),
        (
            PythonNamespacePremise(("a",), 0, 0, ()),
            PythonNamespacePremise(
                ("b",),
                int(PythonMember.IMPORT_HOOKS),
                int(PythonMember.IMPORT_HOOKS),
                (),
            ),
            PythonNamespacePremise(
                ("a", "b"),
                int(PythonMember.IMPORT_HOOKS),
                int(PythonMember.IMPORT_HOOKS),
                (),
            ),
        ),
        (
            PythonNamespacePremise(None, 0, 0, ()),
            PythonNamespacePremise(
                ("b",),
                int(PythonMember.IMPORT_HOOKS),
                int(PythonMember.IMPORT_HOOKS),
                (),
            ),
            PythonNamespacePremise(
                None, int(PythonMember.IMPORT_HOOKS), int(PythonMember.IMPORT_HOOKS), ()
            ),
        ),
    ],
)
def test_scan_authority_merge_and_payload_preserve_namespace_proof(
    tmp_path, left, right, expected
) -> None:
    source = _source(tmp_path / "owner.py", "pass\n")
    first = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "owner",
                source,
                "full",
                body_selection=ModuleBodySelection("complete_source"),
                namespace_premise=left,
            ),
        )
    )
    second = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "owner",
                source,
                "module_init",
                body_selection=ModuleBodySelection("complete_source"),
                namespace_premise=right,
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
            "entry_member_writes": expected.entry_member_writes,
            "noncanonical_imports": expected.noncanonical_imports,
            "opaque_activation_sources": expected.opaque_activation_sources,
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
                "pkg.owner",
                source,
                left_mode,
                False,
                left_dynamic,
                body_selection=ModuleBodySelection("complete_source"),
            ),
        )
    )
    right = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "pkg.owner",
                source,
                right_mode,
                False,
                right_dynamic,
                body_selection=ModuleBodySelection("complete_source"),
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
        source,
        module_name="pkg.owner",
        is_package=False,
        import_scan_mode="full",
        scan=expected,
        snapshot=PythonSourceSnapshot.capture(source),
    )

    assert (
        graph_cache._read_persisted_import_scan_record(
            source,
            module_name="pkg.owner",
            is_package=False,
            import_scan_mode="full",
        )
        == expected
    )


def test_runtime_protocol_bit_counts_statements_the_scan_reaches() -> None:
    # A deferred body's import counts only when the scan reaches the body.
    def detect(source: str, mode: str) -> bool:
        return _source_import_scan._collect_imports_for_graph(
            ast.parse(source),
            "entry",
            False,
            import_scan_mode=mode,
            target_python=PYTHON,
        ).uses_runtime_import_protocol

    deferred = "def invoke():\n    import leaf\n"
    assert not detect(deferred, "module_init")
    assert detect(deferred, "full")
    assert not detect("pass\n", "full")


@pytest.mark.parametrize("target", ["native", "wasm", "rust", "luau", "mlir"])
def test_late_native_root_refreshes_policy_and_catalog(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    target: str,
) -> None:
    entry = _source(tmp_path / "entry.py", "pass\n")
    late = _source(
        tmp_path / "late.py", "def invoke():\n    return __import__('late_leaf')\n"
    )
    leaf = _source(tmp_path / "late_leaf.py", "pass\n")
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
        runtime_import_support_policy=_RuntimeImportSupportPolicy(False),
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
    assert plan.runtime_import_support_policy.needs_runtime_import_support
    assert plan.runtime_import_scan_custody is not None
    assert plan.runtime_import_scan_custody.catalog_by_module["late"] == late
    assert plan.runtime_import_scan_custody.catalog_by_module["late_leaf"] == leaf
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
    assert result.scan.authority.body_selection.roots == ("used",)
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
    physical_source = source.read_bytes()
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
        assert roles["support"].body_selection.roots == roots
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
    assert source.read_bytes() == physical_source
    assert snapshot.sha256 == hashlib.sha256(physical_source).hexdigest()
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
            *environment_bytecode_arguments(Path(sys.prefix)),
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
    assert owner.body_selection.roots == ()
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
    assert roles["support"].body_selection.roots == ("first",)
    assert "leaf1" in graph and "leaf2" not in graph
    selected = ("first", "second")
    graphs._extend_native_support_source_closure(**common)
    assert graph["support"] == support
    assert roles["support"].source_sha256 == first_sha
    assert roles["support"].body_selection.roots == ("first", "second")
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
    assert roles["support"].body_selection.roots == ()
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
    assert roles["support"].body_selection.roots == (() if raw_first else ("used",))
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
    assert roles["support"].body_selection.roots == ()
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
    first = _source(tmp_path / "generated-a" / "support.py", "VALUE = 1\n")
    second = _source(tmp_path / "generated-b" / "support.py", "VALUE = 2\n")
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
                (
                    _ModuleSourceScanAuthority(
                        "support",
                        generation,
                        "full",
                        body_selection=ModuleBodySelection("complete_source"),
                    ),
                )
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
        target_sys_platform,
    ) -> _ImportDiscoveryProjection:
        assert source_owner is None
        assert source_arguments == ()
        assert ast_digest_admission.tree is tree
        assert namespace_premise is None
        assert target_sys_platform is None
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
    generated = _source(tmp_path / "generated" / "pkg.py", "from . import child\n")
    authority = _ModuleSourceScanAuthority(
        "pkg",
        generated,
        "full",
        True,
        body_selection=ModuleBodySelection("complete_source"),
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
    regular = _ModuleSourceScanAuthority(
        "pkg", source, "full", body_selection=ModuleBodySelection("complete_source")
    )
    assert regular.package_paths == (source.parent,)
    roots = (source.parent, second_root.resolve())
    generated_owner = _ModuleSourceScanAuthority(
        "pkg",
        generated,
        "full",
        body_selection=ModuleBodySelection("complete_source"),
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
            body_selection=ModuleBodySelection("complete_source"),
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
        _ModuleSourceScanAuthority(
            "pkg",
            generated,
            "full",
            body_selection=ModuleBodySelection("complete_source"),
            is_package=True,
        )
    with pytest.raises(ValueError, match="non-package"):
        _ModuleSourceScanAuthority(
            "plain",
            generated,
            "full",
            body_selection=ModuleBodySelection("complete_source"),
            package_paths=roots,
        )


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
        assert owner.namespace_premise == PythonNamespacePremise(
            ("alpha", "beta"), 0, 0, (), ()
        )
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
        "_warnings.py",
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
    # Namespace packages are metadata-only catalog entries, with no source body.
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
        runtime_import_support_policy=_RuntimeImportSupportPolicy(False),
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
            *environment_bytecode_arguments(Path(sys.prefix)),
            "-I",
            "-c",
            "import sys,json; sys.path[:0] = json.loads(sys.argv[1]); "
            "import entry,hf226_namespace; "
            "print(json.dumps([list(hf226_namespace.__path__), "
            "list(hf226_namespace.__spec__.submodule_search_locations), "
            "hf226_namespace.__path__ is "
            "hf226_namespace.__spec__.submodule_search_locations, "
            "getattr(hf226_namespace, 'marker', False)]))",
            json.dumps([str(tmp_path), *(str(p) for p in roots)]),
        ],
        timeout=30,
        check=True,
    )
    paths, spec_paths, shared_locations, marker = json.loads(reference.stdout)
    assert marker is namespace_writer
    portions = plan.namespace_portions_by_module["hf226_namespace"]
    assert list(portions) == paths == spec_paths
    assert shared_locations is True
    assert plan.namespace_module_names == {"hf226_namespace"}
    assert "hf226_namespace" in plan.known_modules
    assert "hf226_namespace" not in plan.module_graph
    assert "hf226_namespace" not in plan.compile_modules
    assert "hf226_namespace" not in plan.direct_call_modules
    assert "hf226_namespace" not in plan.scan_authority.by_module
    metadata = plan.module_graph_metadata
    assert metadata.namespace_portions_by_module == plan.namespace_portions_by_module
    assert "hf226_namespace" not in metadata.logical_source_path_by_module
    assert not tuple((tmp_path / "artifacts").glob("_molt_generated_*.py"))


@pytest.mark.parametrize(
    ("publication", "entry_source", "dependency_reached"),
    [
        ("", "from provider import wanted\nwanted()\n", False),
        (
            "import sys\n"
            "sys.modules['provider_alias'] = sys.modules[__name__]\n"
            "globals().pop('_absent',None)\n",
            "from provider import wanted\nwanted()\n",
            False,
        ),
        (
            "import sys\nsys.modules['provider_alias'] = sys.modules[__name__]\n",
            "from provider import wanted\nwanted()\n",
            False,
        ),
        (
            "import sys\nsys.modules['provider_alias'] = sys.modules[__name__]\n",
            "import provider\nimport provider_alias\nprovider_alias.dormant()\n",
            True,
        ),
        (
            "import sys\nsys.modules['provider_alias'] = sys.modules[__name__]\n",
            "import provider,sys\nsys.modules['provider_alias'].dormant()\n",
            True,
        ),
        (
            "import sys\nsys.modules['provider_alias'] = sys.modules[__name__]\n",
            "import provider,sys\nsys.modules.get('provider_alias').dormant()\n",
            True,
        ),
        (
            "import sys\nsys.modules['provider_alias'] = sys.modules[__name__]\n",
            "import provider,sys\nsys.modules.get('provider_alias')\n",
            False,
        ),
        (
            "import sys\nsys.modules['provider_alias'] = sys.modules[__name__]\n",
            "import provider,sys\nunknown(sys.modules)\n",
            True,
        ),
        (
            "import sys\n"
            "sys.modules['provider_alias'] = sys.modules[__name__]\n"
            "sys.modules['provider_alias']=None\n",
            "import provider\nimport provider_alias\nprovider_alias.dormant()\n",
            True,
        ),
        (
            "import sys\nsys.modules.setdefault('provider_alias',sys.modules[__name__])\n",
            "from provider import wanted\nwanted()\n",
            False,
        ),
        (
            "import sys\nsys.modules.setdefault('provider_alias',sys.modules[__name__])\n",
            "import provider,sys\nsys.modules['provider_alias'].dormant()\n",
            True,
        ),
        (
            "import sys\nsys.modules['provider_alias'] = sys.modules[__name__]\n",
            "import provider,sys\nsys.modules[unknown_key].dormant()\n",
            True,
        ),
        (
            "import sys\nsys.modules[unknown_key]=sys.modules[__name__]\n",
            "from provider import wanted\nwanted()\n",
            True,
        ),
        (
            "import sys\nsys.modules[unknown_key]=sys.modules[__name__]\n",
            "import provider,sys\nsys.modules['provider_alias'].dormant()\n",
            True,
        ),
        (
            "import holder,sys\nholder.mapping['provider_alias']=sys.modules[__name__]\n",
            "from provider import wanted\nwanted()\n",
            True,
        ),
    ],
    ids=[
        "plain",
        "held_alias_cleanup",
        "held_alias",
        "alias_import_call",
        "alias_read_call",
        "alias_get_call",
        "alias_get_discard",
        "cache_opaque_argument",
        "cache_none_overwrite",
        "default_hold",
        "default_read_call",
        "unknown_key_read",
        "unknown_key_hold",
        "unknown_key_write",
        "opaque_mapping_store",
    ],
)
def test_import_dictionary_source_values_require_actual_consumption(
    tmp_path: Path,
    publication: str,
    entry_source: str,
    dependency_reached: bool,
) -> None:
    # Cache ownership holds values. Calling an alias/read must discover the
    # provider body even when no physical module exists under the alias name.
    provider = (
        "def wanted():\n    return 7\n\n"
        "def dormant():\n    import dependency\n    return dependency.value\n\n"
    )
    for name, source in {
        "provider": provider + publication,
        "dependency": "value = 8\n",
        "holder": "mapping = {}\n",
        "entry": entry_source,
    }.items():
        _source(tmp_path / f"{name}.py", source)
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert "provider" in result.graph
    assert ("dependency" in result.graph) is dependency_reached
    assert "provider_alias" not in result.graph


@pytest.mark.parametrize(
    "consumer",
    [
        "sys.modules['provider'].dormant()",
        "sys.modules.get('provider').dormant()",
        "unknown(sys.modules)",
    ],
)
def test_implicit_import_publication_supplies_cache_source_values(
    tmp_path: Path, consumer: str
) -> None:
    _source(tmp_path / "entry.py", "import provider, sys\n" + consumer + "\n")
    _source(tmp_path / "provider.py", "def dormant():\n    import implicit_leaf\n")
    _source(tmp_path / "implicit_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert "implicit_leaf" in result.graph


def test_cache_aliases_join_later_writes_without_erasing_original_source(
    tmp_path: Path,
) -> None:
    _source(tmp_path / "entry.py", "import alias\nalias.call()\nimport replacement\n")
    _source(tmp_path / "alias.py", "def call():\n    import original_leaf\n")
    _source(
        tmp_path / "replacement.py",
        "import sys\ndef call():\n    import replacement_leaf\nsys.modules['alias'] = sys.modules[__name__]\n",
    )
    _source(tmp_path / "original_leaf.py", "pass\n")
    _source(tmp_path / "replacement_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert {"original_leaf", "replacement_leaf"} <= result.graph.keys()


def test_cache_replacement_preserves_key_and_terminal_object_callbacks(
    tmp_path: Path,
) -> None:
    _source(tmp_path / "entry.py", "import provider\n")
    _source(
        tmp_path / "provider.py",
        "import sys\nclass Key:\n    def __hash__(self):\n        import hash_leaf\n        return 1\nclass Retired:\n    def __del__(self):\n        import finalizer_leaf\nsys.modules[Key()] = Retired()\nsys.modules[Key()] = None\n",
    )
    _source(tmp_path / "hash_leaf.py", "pass\n")
    _source(tmp_path / "finalizer_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert {"hash_leaf", "finalizer_leaf"} <= result.graph.keys()


@pytest.mark.parametrize(
    ("storage", "consumer", "reached"),
    [
        ("sys.modules['x'] = dormant", "", False),
        ("sys.modules['x'] = dormant", "sys.modules['x']()", True),
        ("sys.modules['x'] = [dormant]", "", False),
        ("sys.modules['x'] = [dormant]", "sys.modules['x'][0]()", True),
        ("sys.modules['x'] = [[dormant]]", "sys.modules['x'][0][0]()", True),
        ("sys.modules.setdefault('x', [dormant])", "sys.modules['x'][0]()", True),
        ("pass", "sys.modules.get('absent', [dormant])[0]()", True),
        (
            "sys.modules['x'] = dormant if isinstance(opaque, int) else opaque",
            "",
            False,
        ),
        (
            "sys.modules['x'] = dormant if isinstance(opaque, int) else opaque",
            "sys.modules['x']()",
            True,
        ),
        (
            "sys.modules['x'] = [dormant] if isinstance(opaque, int) else opaque",
            "sys.modules['x'][0]()",
            True,
        ),
        ("sys.modules.__setitem__('x', dormant)", "", False),
        ("sys.modules.__setitem__('x', dormant)", "sys.modules['x']()", True),
        ("sys.modules.update({'x': dormant})", "", False),
        ("sys.modules.update({'x': dormant})", "sys.modules['x']()", True),
        ("sys.modules.update(x=dormant)", "sys.modules['x']()", True),
        (
            "sys.modules['x'] = dormant",
            "for item in sys.modules.values():\n    item()",
            True,
        ),
        (
            "sys.modules['x'] = dormant",
            "for key, item in sys.modules.items():\n    item()",
            True,
        ),
        ("sys.modules['x'] = dormant", "sys.modules.popitem()[1]()", True),
    ],
)
def test_native_cache_owns_mixed_and_contained_values_until_consumed(
    tmp_path: Path, storage: str, consumer: str, reached: bool
) -> None:
    _source(tmp_path / "entry.py", "import owner\n")
    _source(
        tmp_path / "owner.py",
        "import sys\ndef dormant():\n    import contained_leaf\n"
        + storage
        + "\n"
        + consumer
        + "\n",
    )
    _source(tmp_path / "contained_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert ("contained_leaf" in result.graph) is reached


def test_native_cache_container_retirement_preserves_finalizer(tmp_path: Path) -> None:
    _source(tmp_path / "entry.py", "import owner\n")
    _source(
        tmp_path / "owner.py",
        "import sys\nclass Retired:\n    def __del__(self):\n        import contained_finalizer_leaf\nsys.modules['x'] = [Retired()]\ndel sys.modules['x']\n",
    )
    _source(tmp_path / "contained_finalizer_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert "contained_finalizer_leaf" in result.graph


@pytest.mark.parametrize(
    ("activation", "writer_activated"),
    [
        ("", False),
        ("provider.write()", True),
        ("provider.invoke(provider.write)", True),
        ("opaque(provider)", True),
        ("from provider import missing", True),
        ("import builtins\nbuiltins.__import__ = opaque\nimport provider", True),
        ("sys.modules['provider'] = opaque\nimport provider", True),
    ],
)
def test_graph_initializer_premise_separates_inventory_from_executed_writers(
    tmp_path: Path, activation: str, writer_activated: bool
) -> None:
    provider = _source(
        tmp_path / "provider.py",
        'import sys\ndef write():\n    sys.platform = "changed"\n'
        "def invoke(callback):\n    callback()\n"
        "def __getattr__(name):\n    write()\n    return None\n",
    )
    entry = _source(
        tmp_path / "entry.py",
        "import provider, sys\n" + activation + "\n"
        "if sys.platform == 'wasi':\n    import selected_leaf\nelse:\n    import alternate_leaf\n",
    )
    _source(tmp_path / "selected_leaf.py", "pass\n")
    _source(tmp_path / "alternate_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph_from_paths(
        (entry, provider),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    premise = result.scan_authority.by_module["entry"].namespace_premise
    assert premise is not None
    # Every raw writer remains inventoried; only its actual activation may
    # taint initializer entry. No source availability is thrown away.
    assert premise.member_writes & int(PythonMember.SYS_PLATFORM)
    # Ordinary import still uses the callback inventory: entry precision is
    # not a certificate for arbitrary cached receiver/spec metadata.
    assert "alternate_leaf" in result.graph
    if not writer_activated:
        assert not premise.entry_member_writes & int(PythonMember.SYS_PLATFORM)
    assert "selected_leaf" in result.graph


def test_cached_module_spec_truth_keeps_import_callback_effects(tmp_path: Path) -> None:
    _source(tmp_path / "provider.py", "marker = 1\n")
    entry_source = (
        "import provider, sys\n"
        "class Flag:\n"
        "    def __bool__(self):\n"
        "        sys.platform = 'changed'\n"
        "        return False\n"
        "class Spec:\n"
        "    _initializing = Flag()\n"
        "provider.__spec__ = Spec()\n"
        "import provider\n"
        "observed = sys.platform\n"
        "if observed == 'wasi':\n    import selected_leaf\nelse:\n    import alternate_leaf\n"
    )
    _source(tmp_path / "entry.py", entry_source)
    for name in ("selected_leaf", "alternate_leaf"):
        _source(tmp_path / (name + ".py"), "pass\n")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-c",
            "import sys;sys.path.insert(0,sys.argv[1]);import entry;print(entry.observed)",
            str(tmp_path),
        ],
        timeout=10,
        check=True,
        capture_output=True,
        text=True,
    )
    assert observed.stdout.strip() == "changed"
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert "alternate_leaf" in result.graph


@pytest.mark.parametrize(
    "consumer,leaf",
    [
        ("", False),
        ("provider.Box.method()", True),
        ("provider.Box().method()", True),
        ("provider.Box.stat()", True),
        ("provider.Box().stat()", True),
        ("opaque(provider.Box)", True),
        ("opaque(provider.Box.method.__self__)", True),
    ],
)
def test_retained_descriptor_constructor_requires_actual_consumption(
    tmp_path, consumer, leaf
):
    _source(tmp_path / "entry.py", "import provider\n" + consumer + "\n")
    _source(
        tmp_path / "provider.py",
        "class Box:\n"
        "    @classmethod\n    def method(cls):\n        import descriptor_leaf\n"
        "    @staticmethod\n    def stat():\n        import descriptor_leaf\n",
    )
    _source(tmp_path / "descriptor_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert ("descriptor_leaf" in result.graph) is leaf


def test_retained_classmethod_uses_actual_inherited_class_argument(tmp_path):
    for name, source in {
        "entry": "import provider\nprovider.Derived.pick().needed()\n",
        "provider": "import first, second\nclass Base:\n    value=first\n    @classmethod\n    def pick(cls):\n        return cls.value\nclass Derived(Base):\n    value=second\n",
        "first": "def needed():\n    import first_leaf\n",
        "second": "def needed():\n    import second_leaf\n",
        "first_leaf": "pass\n",
        "second_leaf": "pass\n",
    }.items():
        _source(tmp_path / (name + ".py"), source)
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert "second_leaf" in result.graph


@pytest.mark.parametrize("decorator", ["unknown", "property"])
def test_retained_descriptor_unproved_decorator_still_exposes_function(
    tmp_path, decorator
):
    _source(tmp_path / "entry.py", "import provider\n")
    _source(
        tmp_path / "provider.py",
        f"@{decorator}\ndef method():\n    import descriptor_leaf\n",
    )
    _source(tmp_path / "descriptor_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert "descriptor_leaf" in result.graph


@pytest.mark.parametrize("parameters", ["*args", ""])
@pytest.mark.parametrize(
    "consumer", ["opaque(provider.Box.method.__self__)", "opaque(provider.Box.method)"]
)
def test_retained_classmethod_receiver_is_independent_of_formal_parameters(
    tmp_path, parameters, consumer
):
    _source(tmp_path / "entry.py", "import provider\n" + consumer + "\n")
    _source(
        tmp_path / "provider.py",
        "class Box:\n    @classmethod\n    def method("
        + parameters
        + "): pass\n    def dormant(self):\n        import descriptor_leaf\n",
    )
    _source(tmp_path / "descriptor_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert "descriptor_leaf" in result.graph


@pytest.mark.parametrize(
    "consumer,reached",
    [("", False), ("provider.Box.method()", True), ("opaque(provider.Box)", True)],
)
def test_retained_descriptor_literal_defaults_activate_only_on_consumption(
    tmp_path, consumer, reached
):
    _source(tmp_path / "entry.py", "import provider\n" + consumer + "\n")
    _source(
        tmp_path / "provider.py",
        "class Box:\n    @classmethod\n    def method(cls,path=None,*,target=None):\n        import descriptor_leaf\n",
    )
    _source(tmp_path / "descriptor_leaf.py", "pass\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = discovery._discover_module_graph(
        tmp_path / "entry.py",
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(target_sys_platform="wasi"),
    )
    assert ("descriptor_leaf" in result.graph) is reached


def test_synthetic_intrinsic_from_has_no_foreign_source_body_or_import_request():
    source = (
        "from pkg._intrinsics import require_intrinsic as take, _require_intrinsic, load_intrinsic, _load_intrinsic, runtime_active, unknown\n"
        "load=take('molt_importlib_import_module')\nload('leaf',None)\n"
    )
    tree = ast.parse(source)
    projection = _source_import_scan._collect_imports_for_graph(
        tree,
        module_name="entry",
        namespace_premise=PythonNamespacePremise((), 0, 0, ()),
    )
    imports = list(projection.imports)
    assert not projection.source_bindings
    assert not any(name.startswith("pkg._intrinsics") for name in imports)
    assert imports == []
    assert projection.uses_runtime_import_protocol
    from molt.compiler_analysis.python_binding_flow import (
        analyze_python_source_bindings,
    )

    index = analyze_python_source_bindings(source)
    from molt.compiler_analysis.python_identity_catalog_generated import PythonIdentity

    # Canonical callables carry local value provenance, even though synthetic
    # imports create no foreign source binding or deferred Python body.
    origins = index.expression_result(tree.body[1].value.func).source_origins
    callables = dict(index.source_callables)
    assert {callables[origin] for origin in origins} == {
        int(PythonIdentity.INTRINSICS_REQUIRE)
    }
    ordinary = _source_import_scan._collect_imports_for_graph(
        ast.parse("import pkg._intrinsics\n"), module_name="entry"
    ).imports
    assert "pkg._intrinsics" in ordinary


def test_ordinary_class_decorator_consumes_class_namespace_methods(tmp_path):
    source = "def decorate(cls):\n    cls.enter()\n    return cls\n@decorate\nclass Box:\n    @staticmethod\n    def enter():\n        import decorator_leaf\n"
    entry = _source(tmp_path / "entry.py", source)
    _source(tmp_path / "decorator_leaf.py", "VALUE = 1\n")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys\nsys.path.insert(0, sys.argv[1])\nexec(compile(sys.argv[2], '<class-decorator-consumer>', 'exec'), {})\nprint('decorator_leaf' in sys.modules)",
            str(tmp_path),
            source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert observed.stdout.strip() == "True"
    for _ in range(2):
        graph = _discover(entry, tmp_path, full=True, project=tmp_path).graph
        assert "decorator_leaf" in graph


@pytest.mark.parametrize(
    "operation,expected",
    [
        ("setattr(provider, 'tag', None)", ()),
        ("getattr(provider, 'chosen')", ()),
        ("getattr(provider, 'chosen')()", ("selected_leaf",)),
        ("hasattr(provider, 'chosen')", ()),
        ("delattr(provider, 'chosen')", ()),
        ("getattr(provider, 'absent', fallback)", ()),
        ("getattr(provider, 'absent', fallback)()", ("fallback_leaf",)),
        ("hold(provider)", ()),
        ("consume(provider)", ("unused_leaf",)),
        ("setattr(box, 'stored', provider)", ("unused_leaf",)),
        ("replacement(provider, 'tag')", ("unused_leaf",)),
        ("opaque = getattr(provider, 'consumer'); opaque(provider)", ("unused_leaf",)),
        ("hold.__code__ = consume.__code__; hold(provider)", ("unused_leaf",)),
        (
            "setattr(hold, '__code__', consume.__code__); hold(provider)",
            ("unused_leaf",),
        ),
    ],
)
def test_precise_call_arguments_preserve_actual_source_closure(
    tmp_path, operation, expected
):
    # CPython establishes actual execution independently of binding/source facts.
    provider = (
        "def chosen():\n    import selected_leaf\n"
        "def unused():\n    import unused_leaf\n"
        "def consumer(module): module.unused()\n"
    )
    _source(tmp_path / "provider.py", provider)
    for name in ("selected_leaf", "unused_leaf", "fallback_leaf"):
        _source(tmp_path / f"{name}.py", "VALUE = 1\n")
    source = (
        "import provider\n"
        "def fallback():\n    import fallback_leaf\n"
        "def hold(module): return None\n"
        "def consume(module): module.unused()\n"
        "def replacement(owner, name): return owner.unused()\n"
        "class Box: pass\nbox = Box()\n" + operation + "\n"
    )
    entry = _source(tmp_path / "entry.py", source)
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys\nsys.path.insert(0, sys.argv[1])\nexec(compile(sys.argv[2], '<call-argument-closure>', 'exec'), {})\nprint(','.join(sorted(name for name in sys.modules if name.endswith('_leaf'))))",
            str(tmp_path),
            source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    actual = tuple(filter(None, observed.stdout.strip().split(",")))
    # Stored module exposure is deliberately conservative until storage uses are
    # represented; it must not lose the existing opaque-publication guarantee.
    if not operation.startswith("setattr(box"):
        assert actual == tuple(sorted(expected))
    else:
        assert actual == ()
    for _ in range(2):
        graph = _discover(entry, tmp_path, full=True, project=tmp_path).graph
        reached = {name for name in graph if name.endswith("_leaf")}
        if operation.startswith(
            ("setattr(box", "opaque =", "hold.__code__", "setattr(hold")
        ):
            assert "unused_leaf" in reached
        else:
            assert reached == set(expected)


@pytest.mark.parametrize("selector", ["getattr", "hasattr"])
def test_literal_attribute_builtin_reaches_lazy_hook_only_selected_branch(
    tmp_path, selector
):
    package = (
        "def __getattr__(name):\n"
        "    if name == 'chosen':\n        import hook_leaf\n        return 1\n"
        "    import dead_hook_leaf\n    raise AttributeError(name)\n"
        "def unused():\n    import unused_leaf\n"
    )
    _source(tmp_path / "provider.py", package)
    for name in ("hook_leaf", "dead_hook_leaf", "unused_leaf"):
        _source(tmp_path / f"{name}.py", "VALUE = 1\n")
    source = f"import provider\nalias = {selector}\nalias(provider, 'chosen')\n"
    entry = _source(tmp_path / "entry.py", source)
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys\nsys.path.insert(0, sys.argv[1])\nexec(compile(sys.argv[2], '<lazy-attribute-closure>', 'exec'), {})\nprint(','.join(sorted(name for name in sys.modules if name.endswith('_leaf'))))",
            str(tmp_path),
            source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert observed.stdout.strip() == "hook_leaf"
    for _ in range(2):
        graph = _discover(entry, tmp_path, full=True, project=tmp_path).graph
        assert {name for name in graph if name.endswith("_leaf")} == {"hook_leaf"}


@pytest.mark.parametrize(
    "action,opaque",
    [
        ("C()", False),
        ("C.__init__ = replacement; C()", True),
        ("setattr(C, '__init__', replacement); C()", True),
        ("install(C); C()", True),
        ("set_install(C); C()", True),
        ("opaque_consumer(C())", True),
    ],
)
def test_class_mutation_activation_follows_foreign_source_and_formal_receiver(
    tmp_path, action, opaque
):
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_source_demands import PythonSourceDemands
    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission

    provider = "class C:\n    def __init__(self): self.value = 'old'\n"
    helper = (
        "def replacement(self):\n    import replacement_leaf\n    self.value = 'new'\n"
    )
    entry_source = (
        "from provider import C\nfrom helper import replacement\n"
        "def install(target): target.__init__ = replacement\n"
        "def set_install(target): setattr(target, '__init__', replacement)\n"
        "def opaque_consumer(value): return unknown(value)\n" + action + "\n"
    )
    paths = {
        "entry": _source(tmp_path / "entry.py", entry_source),
        "provider": _source(tmp_path / "provider.py", provider),
        "helper": _source(tmp_path / "helper.py", helper),
    }
    _source(tmp_path / "replacement_leaf.py", "VALUE = 1\n")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys\nsys.path.insert(0, sys.argv[1])\nnamespace = {'unknown': lambda value: None}\nexec(compile(sys.argv[2], '<foreign-class-mutation>', 'exec'), namespace)\nprint(namespace['C']().value)",
            str(tmp_path),
            entry_source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    changed = "__init__" in action or action.startswith(("install(", "set_install("))
    assert observed.stdout.strip() == ("new" if changed else "old")
    trees = {
        name: ast.parse(path.read_text(encoding="utf-8"))
        for name, path in paths.items()
    }
    bodies = []
    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda module: None,
        admit_imports=lambda *args: None,
        queue_body=lambda *job: bodies.append(job),
    )

    def scan(module, owner=None, arguments=()):
        tree = trees[module]
        projection = _source_import_scan._collect_imports_for_graph(
            tree,
            module,
            source_owner=owner,
            source_arguments=arguments,
            import_scan_mode="module_init",
            namespace_premise=PythonNamespacePremise(
                (), 0, 0, (), engine.opaque_activation_sources(module)
            ),
            ast_digest_admission=_PythonAstDigestAdmission.for_tree(tree),
        )
        engine.accept(
            module, _ImportScanRequests((), ())._replace(**projection._asdict())
        )

    for module in paths:
        scan(module)
    while True:
        engine.drain()
        changed_modules = engine.drain_opaque_activation_changes()
        for module in sorted(changed_modules):
            scan(module)
        if bodies:
            scan(*bodies.pop())
        elif not changed_modules:
            break
    class_source = python_node_source_key(trees["provider"].body[0])
    assert (class_source in engine.opaque_activation_sources("provider")) is opaque


def test_dataclass_exposure_keeps_post_init_source_import(tmp_path):
    source = (
        "from dataclasses import dataclass\n@dataclass\nclass C:\n"
        "    value: int\n    def __post_init__(self):\n        import post_init_leaf\n"
        "C(1)\n"
    )
    entry = _source(tmp_path / "entry.py", source)
    _source(tmp_path / "post_init_leaf.py", "VALUE = 1\n")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys\nsys.path.insert(0, sys.argv[1])\nexec(compile(sys.argv[2], '<dataclass-post-init>', 'exec'), {})\nprint('post_init_leaf' in sys.modules)",
            str(tmp_path),
            source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert observed.stdout.strip() == "True"
    for _ in range(2):
        assert (
            "post_init_leaf"
            in _discover(entry, tmp_path, full=True, project=tmp_path).graph
        )
