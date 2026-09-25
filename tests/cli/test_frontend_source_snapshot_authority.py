from __future__ import annotations

import ast
import os
from pathlib import Path

from molt.cli.models import _DiscoveredModuleGraph
from typing import Any, Mapping, NoReturn, TypedDict
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

import pytest

from molt.cli import cache_fingerprints as fingerprints
from molt.cli import module_graph_cache as scans
from molt.cli import module_cache
from molt.cli import module_import_scanner
from molt.cli import module_graph_discovery as discovery
from molt.cli import module_resolution
from molt.cli import module_graph
from molt.cli import python_source_closure as source_closure
from molt.cli.module_source import PythonSourceSnapshot
from molt.cli.models import (
    _ImportScanRequests,
    ImportScanMode,
    _CompleteImportScan,
    _ImportAdmissionPolicy,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _RuntimeImportScanCustody,
)
from molt.compiler_analysis import python_binding_flow
from molt.compiler_analysis.python_binding_flow import python_ast_digest


@pytest.fixture
def compiler_sources(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    root = tmp_path / "compiler"
    driver = root / "src" / "molt" / "cli" / "module_source.py"
    driver.parent.mkdir(parents=True)
    driver.write_text("import molt.cli.helper\n", encoding="utf-8")

    def compiler_root() -> Path:
        return root

    def no_clean_signature(_root: Path, _path_keys: tuple[str, ...]) -> None:
        return None

    monkeypatch.setattr(fingerprints, "_compiler_root", compiler_root)
    # Exercise the content authority, independent of the enclosing checkout.
    monkeypatch.setattr(
        fingerprints, "_source_tree_clean_pathspec_signature", no_clean_signature
    )
    return driver


def _count_semantic_work(monkeypatch: pytest.MonkeyPatch) -> dict[str, int]:
    counts = {"paths": 0, "bytes": 0}
    sources = fingerprints._frontend_semantic_tooling_sources
    content = fingerprints._source_tree_content_signature

    def source_inputs(root: Path) -> fingerprints._SourceFingerprintInputs:
        counts["paths"] += 1
        return sources(root)

    def content_signature(
        root: Path,
        path_keys: tuple[str, ...],
        source_sha256: Mapping[Path, str],
    ) -> tuple[str, ...]:
        counts["bytes"] += 1
        return content(root, path_keys, source_sha256)

    monkeypatch.setattr(
        fingerprints, "_frontend_semantic_tooling_sources", source_inputs
    )
    monkeypatch.setattr(
        fingerprints, "_source_tree_content_signature", content_signature
    )
    return counts


def test_semantic_snapshot_precedes_closure_and_digest_work(
    monkeypatch: pytest.MonkeyPatch, compiler_sources: Path
) -> None:
    counts = _count_semantic_work(monkeypatch)
    with fingerprints._source_tree_fingerprint_transaction():
        snapshot = fingerprints._frontend_semantic_tooling_snapshot()
        with fingerprints._source_tree_fingerprint_transaction():
            for _ in range(5):
                assert fingerprints._frontend_semantic_tooling_snapshot() is snapshot
                assert (
                    fingerprints._frontend_semantic_tooling_fingerprint()
                    == snapshot.fingerprint
                )
    assert compiler_sources in snapshot.source_paths
    assert counts == {"paths": 1, "bytes": 1}
    assert fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get() is None


@pytest.mark.parametrize("change", ["same_stat_bytes", "missing_module"])
def test_next_operation_recaptures_semantic_bytes_and_topology(
    change: str, compiler_sources: Path
) -> None:
    helper = compiler_sources.with_name("helper.py")
    if change == "same_stat_bytes":
        helper.write_text("VALUE = 1\n", encoding="utf-8")
    with fingerprints._source_tree_fingerprint_transaction():
        before = fingerprints._frontend_semantic_tooling_snapshot()
    if change == "same_stat_bytes":
        stat = helper.stat()
        helper.write_text("VALUE = 2\n", encoding="utf-8")
        os.utime(helper, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        assert helper.stat().st_size == stat.st_size
        assert helper.stat().st_mtime_ns == stat.st_mtime_ns
    else:
        assert helper not in before.source_paths
        helper.write_text("VALUE = 1\n", encoding="utf-8")
    with fingerprints._source_tree_fingerprint_transaction():
        after = fingerprints._frontend_semantic_tooling_snapshot()
    assert after is not before
    assert helper in after.source_paths
    assert after.fingerprint != before.fingerprint


def test_next_operation_rejects_seed_ownership_retarget(
    compiler_sources: Path, tmp_path: Path
) -> None:
    owned = compiler_sources.with_name("owned.py")
    owned.write_text("VALUE = 1\n", encoding="utf-8")
    outside = tmp_path / "outside.py"
    outside.write_text("VALUE = 1\n", encoding="utf-8")
    alias = compiler_sources.with_name("module_alias.py")
    try:
        alias.symlink_to(owned)
    except OSError as exc:
        pytest.skip(f"file symlinks unavailable: {exc}")
    with fingerprints._source_tree_fingerprint_transaction():
        before = fingerprints._frontend_semantic_tooling_snapshot()
    assert owned.resolve() in before.source_paths
    alias.unlink()
    alias.symlink_to(outside)
    with pytest.raises(ValueError, match="outside project root"):
        with fingerprints._source_tree_fingerprint_transaction():
            fingerprints._frontend_semantic_tooling_snapshot()
    assert fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get() is None
    alias.unlink()
    alias.symlink_to(owned)
    with fingerprints._source_tree_fingerprint_transaction():
        assert (
            fingerprints._frontend_semantic_tooling_fingerprint() == before.fingerprint
        )


class _ImportScanOptions(TypedDict):
    module_name: str
    is_package: bool
    import_scan_mode: ImportScanMode


def _scan_options(
    module_name: str, mode: ImportScanMode = "full"
) -> _ImportScanOptions:
    return {
        "module_name": module_name,
        "is_package": False,
        "import_scan_mode": mode,
    }


def test_direct_scan_operations_share_identity_but_not_application_bytes(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, compiler_sources: Path
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    counts = _count_semantic_work(monkeypatch)
    options = _scan_options("entry")
    scans._write_persisted_import_scan(
        tmp_path,
        source,
        scan=_ImportScanRequests((), ()),
        snapshot=PythonSourceSnapshot.capture(source),
        **options,
    )
    assert counts == {"paths": 1, "bytes": 1}
    counts.update(paths=0, bytes=0)
    assert (
        scans._read_persisted_import_scan_record(tmp_path, source, **options)
        is not None
    )
    assert counts == {"paths": 1, "bytes": 1}
    with fingerprints._source_tree_fingerprint_transaction():
        assert (
            scans._read_persisted_import_scan_record(tmp_path, source, **options)
            is not None
        )
        stat = source.stat()
        source.write_text("VALUE = 2\n", encoding="utf-8")
        os.utime(source, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        # Only compiler tooling is frozen, never the scanned application.
        assert (
            scans._read_persisted_import_scan_record(tmp_path, source, **options)
            is None
        )


@pytest.mark.parametrize("warm_operation", ["scan", "native_support"])
def test_imports_only_consumers_warm_complete_scan_before_graph_discovery(
    warm_operation: str,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    compiler_sources: Path,
) -> None:
    app = tmp_path / "app"
    app.mkdir()
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    entry = app / "entry.py"
    entry.write_text("import support\n", encoding="utf-8")
    support = app / "support.py"
    target = app / "runtime_site" / "target.py"
    target.parent.mkdir()
    target.write_text("VALUE = 1\n", encoding="utf-8")
    source = (
        "import importlib.util\n"
        f"path = {str(target)!r}\n"
        "importlib.util.spec_from_file_location('loaded_name', path)\n"
    )
    support.write_text(source, encoding="utf-8")
    reads: list[Path] = []
    writes: list[Path] = []
    read_record = scans._read_persisted_import_scan_record
    write_record = scans._write_persisted_import_scan

    def record_read(
        project_root: Path, path: Path, **kwargs: Any
    ) -> _ImportScanRequests | None:
        reads.append(path)
        return read_record(project_root, path, **kwargs)

    def record_write(project_root: Path, path: Path, **kwargs: Any) -> None:
        writes.append(path)
        write_record(project_root, path, **kwargs)

    monkeypatch.setattr(scans, "_read_persisted_import_scan_record", record_read)
    monkeypatch.setattr(scans, "_write_persisted_import_scan", record_write)
    counts = _count_semantic_work(monkeypatch)

    if warm_operation == "scan":
        imports = discovery._load_module_import_scan(
            support,
            resolution_cache=module_resolution._ModuleResolutionCache(),
            project_root=app,
            **_scan_options("support", "module_init"),
        ).scan.imports
    else:

        class NativeSupportPlan:
            def support_source_paths_by_module(self) -> dict[str, Path]:
                return {"support": support}

        native_counts = {
            "native_support_slice_requests": 0,
            "native_support_legacy_equivalent_source_parses": 0,
            "native_support_slice_cache_hits": 0,
            "native_support_slice_cache_misses": 0,
            "native_support_persisted_import_scan_hits": 0,
            "native_support_persisted_import_scan_misses": 0,
            "native_support_source_parses": 0,
            "native_support_source_prunes": 0,
        }
        slices = module_graph._native_support_source_slices(
            native_artifact_plan=NativeSupportPlan(),
            roots_by_module={},
            artifacts_root=app,
            slice_cache={},
            operation_counts=native_counts,
        )
        imports = slices[support].imports
        assert native_counts["native_support_persisted_import_scan_misses"] == 1
        assert native_counts["native_support_source_parses"] == 1
    assert "importlib.util" in imports
    assert reads == [support] and writes == [support]
    assert counts == {"paths": 1, "bytes": 1}

    def discover() -> _DiscoveredModuleGraph:
        return discovery._discover_module_graph_from_paths(
            (entry,),
            [app, stdlib],
            [app],
            stdlib,
            app,
            set(),
            full_scan_roots=True,
        )

    reads.clear()
    writes.clear()
    counts.update(paths=0, bytes=0)
    discovery_result = discover()
    graph = discovery_result.graph
    explicit = discovery_result.explicit_imports
    assert graph["loaded_name"] == target.resolve()
    assert "loaded_name" in explicit
    # The warmed support source is a complete hit; only the entry and newly
    # reached source are produced. No repeated miss lookup or partial write.
    assert reads == [entry, support, target.resolve()]
    assert writes == [entry, target.resolve()]
    assert counts == {"paths": 1, "bytes": 1}

    reads.clear()
    writes.clear()
    discovery_result = discover()
    graph = discovery_result.graph
    assert graph["loaded_name"] == target.resolve()
    assert reads == [entry, support, target.resolve()]
    assert writes == []

    # A new source generation needs exactly one replacement publication, even
    # when size/mtime match. Unchanged entry/support sources are not rewritten.
    stat = target.stat()
    target.write_text("VALUE = 2\n", encoding="utf-8")
    os.utime(target, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    reads.clear()
    writes.clear()
    discovery_result = discover()
    graph = discovery_result.graph
    assert graph["loaded_name"] == target.resolve()
    assert reads == [entry, support, target.resolve()]
    assert writes == [target.resolve()]


def test_cold_discovery_produces_complete_scan_once(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, compiler_sources: Path
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    counts = {"reads": 0, "writes": 0}
    read_record = scans._read_persisted_import_scan_record
    write_record = scans._write_persisted_import_scan

    def read(
        project_root: Path, path: Path, **kwargs: Any
    ) -> _ImportScanRequests | None:
        counts["reads"] += 1
        return read_record(project_root, path, **kwargs)

    def write(project_root: Path, path: Path, **kwargs: Any) -> None:
        counts["writes"] += 1
        write_record(project_root, path, **kwargs)

    monkeypatch.setattr(scans, "_read_persisted_import_scan_record", read)
    monkeypatch.setattr(scans, "_write_persisted_import_scan", write)
    discovery_result = discovery._discover_module_graph_from_paths(
        (source,),
        [tmp_path, stdlib],
        [tmp_path],
        stdlib,
        tmp_path,
        set(),
        full_scan_roots=True,
    )
    graph = discovery_result.graph
    assert graph == {"entry": source}
    assert counts == {"reads": 1, "writes": 1}


def test_complete_cold_scan_hashes_one_exact_ast_generation(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, compiler_sources: Path
) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    (package / "__init__.py").write_text("__all__ = ('child',)\n", encoding="utf-8")
    (package / "child.py").write_text("VALUE = 1\n", encoding="utf-8")
    executed = tmp_path / "loaded.py"
    executed.write_text("VALUE = 2\n", encoding="utf-8")
    source = tmp_path / "entry.py"
    source.write_text(
        "from pkg import *\n"
        "from importlib.util import spec_from_file_location\n"
        "spec_from_file_location('loaded', 'loaded.py')\n",
        encoding="utf-8",
    )
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    digest = python_binding_flow.python_ast_digest
    digested_trees: list[ast.AST] = []

    def record_digest(tree: ast.AST) -> str:
        digested_trees.append(tree)
        return digest(tree)

    monkeypatch.setattr(python_binding_flow, "python_ast_digest", record_digest)
    loaded = discovery._load_module_import_scan(
        source,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=module_resolution._ModuleResolutionCache(),
        project_root=None,
        roots=[tmp_path],
        stdlib_root=stdlib,
        stdlib_allowlist=set(),
    )

    assert "pkg.child" in loaded.scan.imports
    assert loaded.scan.source_executions == (("loaded", executed.resolve()),)
    assert len(digested_trees) == 1


def test_malformed_execution_projection_rejects_entire_scan(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, compiler_sources: Path
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    options = _scan_options("entry")
    with fingerprints._source_tree_fingerprint_transaction():
        scans._write_persisted_import_scan(
            tmp_path,
            source,
            snapshot=PythonSourceSnapshot.capture(source),
            scan=_ImportScanRequests(("helper",), ()),
            **options,
        )
        path = scans._import_scan_cache_path(tmp_path, source, **options)
        payload = scans._read_artifact_sync_state(path)
        assert payload is not None
        malformed = dict(payload, source_executions=[{"module": "child", "path": 17}])

        def read_malformed(_path: Path) -> dict[str, Any]:
            return malformed

        monkeypatch.setattr(scans, "_read_artifact_sync_state", read_malformed)
        assert (
            scans._read_persisted_import_scan_record(tmp_path, source, **options)
            is None
        )
    assert not hasattr(scans, "_read_persisted_source_executions")
    assert not hasattr(scans, "_read_persisted_import_scan")
    assert not hasattr(scans, "_read_persisted_import_scan_payload")


def test_runtime_custody_never_enters_persisted_snapshot_scan_lane(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    owner, target = tmp_path / "entry.py", tmp_path / "target.py"
    source = "__package__ = choose_package()\nfrom . import child\n"
    owner.write_text(source, encoding="utf-8")
    target.write_text("", encoding="utf-8")
    custody = _RuntimeImportScanCustody(
        owners=(("entry", owner),),
        catalog=(("entry", owner), ("target", target)),
        owner_ast_digests=(("entry", python_ast_digest(ast.parse(source))),),
    )
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()

    def forbidden(*_args: object, **_kwargs: object) -> NoReturn:
        raise AssertionError(
            "runtime custody cannot read or publish strict persisted scans"
        )

    for name in (
        "_read_persisted_import_scan_record",
        "_write_persisted_import_scan",
    ):
        monkeypatch.setattr(scans, name, forbidden)
    monkeypatch.setattr(fingerprints, "_frontend_semantic_tooling_sources", forbidden)
    discovery_result = discovery._discover_module_graph_from_paths(
        (owner,),
        [tmp_path, stdlib],
        [tmp_path],
        stdlib,
        tmp_path,
        set(),
        full_scan_roots=True,
        runtime_import_custody=custody,
        enclosing_scan_authority=_ModuleGraphScanAuthority(
            (_ModuleSourceScanAuthority("target", target, "full"),)
        ),
    )
    graph = discovery_result.graph
    assert graph == dict(custody.catalog)


def test_direct_operation_failure_preserves_outer_and_releases_owned_transaction(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, compiler_sources: Path
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    counts = _count_semantic_work(monkeypatch)
    published_fingerprints: list[str] = []

    def fail_publication(_path: Path, payload: dict[str, Any]) -> NoReturn:
        fingerprint = payload["compiler_fingerprint"]
        assert isinstance(fingerprint, str)
        published_fingerprints.append(fingerprint)
        raise OSError("publication interrupted")

    monkeypatch.setattr(scans, "_write_artifact_sync_payload", fail_publication)

    def write() -> None:
        scans._write_persisted_import_scan(
            tmp_path,
            source,
            snapshot=PythonSourceSnapshot.capture(source),
            scan=_ImportScanRequests((), ()),
            **_scan_options("entry"),
        )

    with fingerprints._source_tree_fingerprint_transaction():
        outer = fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
        graph_outer = source_closure._GRAPH_TRANSACTION.get()
        assert outer is not None and graph_outer is not None
        before = fingerprints._frontend_semantic_tooling_snapshot()
        with pytest.raises(OSError, match="publication interrupted"):
            write()
        assert fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get() is outer
        assert source_closure._GRAPH_TRANSACTION.get() is graph_outer
        assert fingerprints._frontend_semantic_tooling_snapshot() is before
    assert fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get() is None
    assert source_closure._GRAPH_TRANSACTION.get() is None

    # A standalone decorated operation owns and releases both contexts even
    # when publication fails. A subsequent operation must see changed topology.
    with pytest.raises(OSError, match="publication interrupted"):
        write()
    assert fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get() is None
    assert source_closure._GRAPH_TRANSACTION.get() is None
    compiler_sources.with_name("helper.py").write_text("VALUE = 1\n", encoding="utf-8")
    with pytest.raises(OSError, match="publication interrupted"):
        write()
    assert fingerprints._SOURCE_TREE_FINGERPRINT_TRANSACTION.get() is None
    assert source_closure._GRAPH_TRANSACTION.get() is None
    assert published_fingerprints[0] == published_fingerprints[1]
    assert published_fingerprints[2] != published_fingerprints[1]
    assert counts == {"paths": 3, "bytes": 3}


def test_artifact_owned_source_retains_precomputed_execution_root_without_scanning(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    owner, target = tmp_path / "owner.py", tmp_path / "target.py"
    owner.write_text("this source must not be interpreted\n", encoding="utf-8")
    target.write_text("VALUE = 1\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    cache = module_resolution._ModuleResolutionCache()
    policy = _ImportAdmissionPolicy(
        native_artifact_source_packages=frozenset({"owner"})
    )

    def forbidden(*_args: object, **_kwargs: object) -> NoReturn:
        raise AssertionError("explicit artifact-owned execution custody was rescanned")

    for name in (
        "_read_persisted_import_scan_record",
        "_write_persisted_import_scan",
    ):
        monkeypatch.setattr(scans, name, forbidden)
    for name in ("_collect_imports", "_collect_static_source_execution_requests"):
        monkeypatch.setattr(module_import_scanner, name, forbidden)
    monkeypatch.setattr(cache, "read_module_source", forbidden)
    monkeypatch.setattr(cache, "parse_module_ast", forbidden)
    load_scan = discovery._load_module_import_scan

    def load_target_only(
        path: Path, **kwargs: Any
    ) -> discovery._LoadedModuleImportScan:
        assert path != owner, (
            "artifact-owned source must not enter strict scan production"
        )
        return load_scan(path, **kwargs)

    monkeypatch.setattr(discovery, "_load_module_import_scan", load_target_only)
    discovery_result = discovery._discover_module_graph_from_paths(
        (owner,),
        [tmp_path, stdlib],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
        resolver_cache=cache,
        import_admission_policy=policy,
        precomputed_scans_by_path={
            owner: discovery._bind_precomputed_module_import_scan(
                owner,
                snapshot=PythonSourceSnapshot.capture(owner),
                module_name="owner",
                import_scan_mode="full",
                scan=_CompleteImportScan((), (("target", target),)),
                target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            ),
            target: discovery._bind_precomputed_module_import_scan(
                target,
                snapshot=PythonSourceSnapshot.capture(target),
                module_name="target",
                import_scan_mode="module_init",
                scan=_CompleteImportScan((), ()),
                target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            ),
        },
    )
    graph = discovery_result.graph
    explicit = discovery_result.explicit_imports
    assert graph == {"owner": owner, "target": target}
    assert explicit == {"target"}


@pytest.mark.parametrize("retain", [False, True])
def test_cold_scan_with_cached_analysis_facts_preserves_retention_and_miss(
    retain: bool,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    compiler_sources: Path,
) -> None:
    source = tmp_path / "entry.py"
    text = "VALUE = 1\n"
    source.write_text(text, encoding="utf-8")

    def cached_facts_without_imports(
        *_args: object, **_kwargs: object
    ) -> tuple[dict[str, dict[str, Any]], dict[str, str]]:
        return {}, {}

    monkeypatch.setattr(
        module_cache, "_read_persisted_module_analysis", cached_facts_without_imports
    )
    result = module_cache._load_module_analysis(
        source,
        source=None,
        logical_source_path="<logical-entry>",
        resolution_cache=module_resolution._ModuleResolutionCache(),
        project_root=tmp_path,
        retain_source=retain,
        retain_tree=retain,
        **_scan_options("entry"),
    )
    tree, imports, defaults, kinds, retained_source, hit, _stat = result
    assert imports == () and defaults == {} and kinds == {}
    assert hit is False
    assert (tree is not None) is retain
    assert retained_source == (text if retain else None)
