from __future__ import annotations
from molt.compiler_analysis.python_imports import (
    _ImportScanRequests,
    _StaticSourceExecutionRequest,
)
from molt.compiler_analysis import python_import_scan as _source_import_scan

import ast
import json
import os
from pathlib import Path

from molt.cli.module_graph_discovery import _DiscoveredModuleGraph
from typing import Any, Mapping, NoReturn, TypedDict
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

import pytest

from molt.cli import cache_fingerprints as fingerprints
from molt.cli import module_graph_cache as scans
from molt.cli import module_cache
from molt.cli import module_graph_discovery as discovery
from molt.cli import module_resolution
from molt.cli import module_source
from molt.cli import module_graph
from molt.cli import python_source_closure as source_closure
from molt.cli.module_source import PythonSourceSnapshot
from molt.cli.source_record_store import SourceRecordStore
from tests.source_record_test_support import replace_source_record, source_records
from tests.cli.test_installed_compiler import installation as installation
from molt.cli.models import (
    ImportScanMode,
    _CompleteImportScan,
    _ImportAdmissionPolicy,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _RuntimeImportScanCustody,
)
from molt.compiler_analysis import python_source_keys
from molt.compiler_analysis.python_source_keys import python_ast_digest
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_identity_catalog_generated import ALL_INVALID_MEMBERS


pytestmark = pytest.mark.usefixtures("isolated_molt_cache")


def test_discovery_authority_retains_raw_source_generation(
    tmp_path: Path, compiler_sources: Path
) -> None:
    import hashlib

    content = b"# coding: latin-1\r\nVALUE = '\xe9'\r\n"
    source = tmp_path / "entry.py"
    source.write_bytes(content)
    result = discovery._discover_module_graph_from_paths(
        (source,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        tmp_path,
        set(),
        full_scan_roots=True,
    )
    captured = result.scan_authority.by_module["entry"]
    expected = hashlib.sha256(content).hexdigest()
    assert captured.source_sha256 == expected
    assert (
        expected
        != hashlib.sha256(
            PythonSourceSnapshot.capture(source).text.encode()
        ).hexdigest()
    )
    result.scan_authority.validate_source_generations()
    assert result.scan_authority.payload()[0]["source_sha256"] == expected
    for other in (
        _ModuleSourceScanAuthority("entry", source, "module_init"),
        _ModuleSourceScanAuthority("entry", source, "full"),
    ):
        joined = result.scan_authority.merged(_ModuleGraphScanAuthority((other,)))
        assert joined.by_module["entry"].source_sha256 == expected
        reverse = _ModuleGraphScanAuthority((other,)).merged(result.scan_authority)
        assert reverse.by_module["entry"].source_sha256 == expected
    source.write_bytes(content.replace(b"VALUE", b"OTHER"))
    changed = discovery._discover_module_graph_from_paths(
        (source,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        tmp_path,
        set(),
        full_scan_roots=True,
    )
    with pytest.raises(
        module_source.PythonSourceChangedError, match="generation changed"
    ):
        result.scan_authority.merged(changed.scan_authority)


@pytest.mark.parametrize("changed_module", ["entry", "writer"])
@pytest.mark.parametrize("phase", ["before", "generated-importer"])
def test_runtime_import_policy_fences_complete_source_generation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    compiler_sources: Path,
    changed_module: str,
    phase: str,
) -> None:
    import os
    from molt.cli import module_import_scanner

    entry, writer = tmp_path / "entry.py", tmp_path / "writer.py"
    entry.write_text("import writer\nVALUE = 1\n", encoding="utf-8")
    writer.write_text("VALUE = 1\n", encoding="utf-8")
    cache = module_resolution._ModuleResolutionCache()
    graph = discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        tmp_path,
        set(),
        resolver_cache=cache,
        full_scan_roots=True,
    )
    options = dict(
        module_graph=graph.graph,
        scan_authority=graph.scan_authority,
        explicit_imports=(),
    )
    # The decision reads the scans' bits: the entry imports the writer.
    baseline = module_import_scanner._module_graph_needs_runtime_import_support(
        **options
    )
    assert baseline.needs_runtime_import_support
    changed = graph.graph[changed_module]

    def mutate() -> None:
        before = changed.stat()
        content = changed.read_bytes()
        changed.write_bytes(content.replace(b"1", b"2"))
        os.utime(changed, ns=(before.st_atime_ns, before.st_mtime_ns))
        assert changed.stat().st_size == before.st_size
        assert changed.stat().st_mtime_ns == before.st_mtime_ns
        assert changed.read_bytes() != content

    mutate()
    if phase == "generated-importer":
        options["explicit_imports"] = (module_import_scanner.IMPORTER_MODULE_NAME,)
    with pytest.raises(
        module_source.PythonSourceChangedError, match="generation changed"
    ):
        module_import_scanner._module_graph_needs_runtime_import_support(**options)


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


def test_installed_semantic_snapshot_uses_admitted_release_without_graph(
    monkeypatch: pytest.MonkeyPatch, installation: Path
) -> None:
    from molt import compiler_distribution
    from molt.cli import compiler_identity

    calls = {"sources": 0, "executing": 0, "binary": 0}
    installed_type = compiler_distribution.InstalledCompiler
    verify_sources = installed_type.verify_sources
    verify_executing = installed_type.verify_executing_package
    verify_binary = installed_type.verify_binary

    # Keep strict release-manifest and live file admission real. The shared
    # fixture's synthetic executable bytes are never execution evidence.
    def sources(compiler):
        calls["sources"] += 1
        return verify_sources(compiler)

    def executing(compiler):
        calls["executing"] += 1
        return verify_executing(compiler)

    def binary(compiler, features, cargo_profile):
        calls["binary"] += 1
        return verify_binary(compiler, features, cargo_profile)

    def forbidden(root: Path) -> NoReturn:
        raise AssertionError(
            "admitted installed source must not walk the developer graph"
        )

    monkeypatch.setattr(installed_type, "verify_sources", sources)
    monkeypatch.setattr(installed_type, "verify_executing_package", executing)
    monkeypatch.setattr(installed_type, "verify_binary", binary)
    monkeypatch.setattr(fingerprints, "_compiler_root", lambda: installation)
    monkeypatch.setattr(fingerprints, "_frontend_semantic_tooling_sources", forbidden)
    with fingerprints._source_tree_fingerprint_transaction():
        first = fingerprints._frontend_semantic_tooling_snapshot()
        assert fingerprints._frontend_semantic_tooling_snapshot() is first
        broad = fingerprints._cache_tooling_fingerprint()
    assert first.source_paths == ()
    assert first.fingerprint != broad  # Explicit consumer namespace separation.
    assert calls == {"sources": 1, "executing": 1, "binary": 2}

    manifest = installation / compiler_distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    payload["git"]["commit"] = "b" * 40
    payload["runtime"]["source"]["commit"] = "b" * 40
    manifest.write_text(json.dumps(payload), encoding="utf-8")
    with fingerprints._source_tree_fingerprint_transaction():
        second = fingerprints._frontend_semantic_tooling_snapshot()
    assert second.fingerprint != first.fingerprint

    (installation / "Cargo.toml").write_bytes(b"changed source generation")
    with fingerprints._source_tree_fingerprint_transaction():
        with pytest.raises(
            compiler_identity.CompilerIdentityError, match="content changed"
        ):
            fingerprints._frontend_semantic_tooling_snapshot()


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
        source,
        scan=_ImportScanRequests(
            **_source_import_scan._collect_imports_for_graph(
                ast.parse(source.read_text(encoding="utf-8")),
                "entry",
                import_scan_mode="full",
            )._asdict(),
            source_executions=(),
        ),
        snapshot=PythonSourceSnapshot.capture(source),
        **options,
    )
    assert counts == {"paths": 1, "bytes": 1}
    counts.update(paths=0, bytes=0)
    assert scans._read_persisted_import_scan_record(source, **options) is not None
    assert counts == {"paths": 1, "bytes": 1}
    with fingerprints._source_tree_fingerprint_transaction():
        assert scans._read_persisted_import_scan_record(source, **options) is not None
        stat = source.stat()
        source.write_text("VALUE = 2\n", encoding="utf-8")
        os.utime(source, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        # Only compiler tooling is frozen, never the scanned application.
        assert scans._read_persisted_import_scan_record(source, **options) is None


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

    def record_read(path: Path, **kwargs: Any) -> _ImportScanRequests | None:
        reads.append(path)
        return read_record(path, **kwargs)

    def record_write(path: Path, **kwargs: Any) -> None:
        writes.append(path)
        write_record(path, **kwargs)

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
            roots_by_module={"support": ()},
            resolution_cache=module_resolution._ModuleResolutionCache(),
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
    # The imports-only warmup has no closed-graph premise. Discovery projects
    # it once under its actual premise; it cannot reuse a different policy's
    # completed record. The next matching discovery rewrites nothing.
    assert reads == [entry, support, target.resolve()]
    assert writes == [entry, support, target.resolve()]
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

    def read(path: Path, **kwargs: Any) -> _ImportScanRequests | None:
        counts["reads"] += 1
        return read_record(path, **kwargs)

    def write(path: Path, **kwargs: Any) -> None:
        counts["writes"] += 1
        write_record(path, **kwargs)

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


def test_complete_cold_scan_hashes_each_source_generation_once(
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
    digest = python_source_keys.python_ast_digest
    digested_trees: list[ast.AST] = []

    def record_digest(tree: ast.AST) -> str:
        digested_trees.append(tree)
        return digest(tree)

    monkeypatch.setattr(python_source_keys, "python_ast_digest", record_digest)
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
    # Completion proves current exports from a distinct package generation.
    # Both actual inputs are hashed once; re-hashing either must fail this test.
    from collections import Counter

    assert Counter(ast.dump(tree) for tree in digested_trees) == Counter(
        ast.dump(ast.parse(path.read_text(encoding="utf-8")))
        for path in (source, package / "__init__.py")
    )


def test_malformed_execution_projection_rejects_entire_scan(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, compiler_sources: Path
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    options = _scan_options("entry")
    snapshot = PythonSourceSnapshot.capture(source)
    request = discovery._load_module_import_scan(
        source,
        **options,
        resolution_cache=module_resolution._ModuleResolutionCache(),
        project_root=None,
        source_snapshot=snapshot,
    ).scan.source_requests
    with fingerprints._source_tree_fingerprint_transaction():
        scans._write_persisted_import_scan(
            source,
            snapshot=snapshot,
            scan=request,
            **options,
        )
        assert (
            scans._read_persisted_import_scan_record(
                source, **options, snapshot=snapshot
            )
            == request
        )
        key = scans._import_scan_cache_key(source, **options)
        payload = source_records("import-scan")[key]
        malformed = dict(payload, source_executions=[{"module": "child", "path": 17}])

        replace_source_record("import-scan", key, malformed)
        assert scans._read_persisted_import_scan_record(source, **options) is None
    assert not hasattr(scans, "_read_persisted_source_executions")
    assert not hasattr(scans, "_read_persisted_import_scan")
    assert not hasattr(scans, "_read_persisted_import_scan_payload")


def test_runtime_custody_persists_only_source_pure_owner_scans(
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

    cache = module_resolution._ModuleResolutionCache()
    operation = discovery._ModuleGraphDiscovery(cache, _DEFAULT_TARGET_PYTHON_VERSION)
    discovery._discover_module_graph_from_paths(
        (target,),
        [tmp_path, stdlib],
        [tmp_path],
        stdlib,
        tmp_path,
        set(),
        resolver_cache=cache,
        discovery=operation,
        full_scan_roots=True,
    )

    calls: list[tuple[str, Path, bool, Any]] = []
    read_record = scans._read_persisted_import_scan_record
    write_record = scans._write_persisted_import_scan

    def read(path: Path, **kwargs: Any) -> Any:
        record = read_record(path, **kwargs)
        owner_mode = kwargs.get("runtime_catalog_owner", False)
        calls.append(("read", path.resolve(), owner_mode, record))
        return record

    def write(path: Path, **kwargs: Any) -> None:
        owner_mode = kwargs.get("runtime_catalog_owner", False)
        calls.append(("write", path.resolve(), owner_mode, kwargs["scan"]))
        write_record(path, **kwargs)

    monkeypatch.setattr(scans, "_read_persisted_import_scan_record", read)
    monkeypatch.setattr(scans, "_write_persisted_import_scan", write)
    discovery_result = discovery._discover_module_graph_from_paths(
        (owner,),
        [tmp_path, stdlib],
        [tmp_path],
        stdlib,
        tmp_path,
        set(),
        full_scan_roots=True,
        runtime_import_custody=custody,
        resolver_cache=cache,
        discovery=operation,
    )
    assert discovery_result.discovery is operation
    assert operation.graph == dict(custody.catalog)
    owner_calls = [call for call in calls if call[1] == owner.resolve()]
    assert owner_calls and all(call[2] for call in owner_calls), (
        "an owner never reads or publishes a strict record"
    )
    written = [call[3] for call in owner_calls if call[0] == "write"]
    assert written
    for scan in written:
        # The record defers to the catalog; it never names a catalog row.
        assert scan.runtime_catalog is not None and scan.runtime_catalog.imports
        assert "target" not in scan.imports


def test_direct_operation_failure_preserves_outer_and_releases_owned_transaction(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, compiler_sources: Path
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    snapshot = PythonSourceSnapshot.capture(source)
    request = discovery._load_module_import_scan(
        source,
        **_scan_options("entry"),
        resolution_cache=module_resolution._ModuleResolutionCache(),
        project_root=None,
        source_snapshot=snapshot,
    ).scan.source_requests
    counts = _count_semantic_work(monkeypatch)
    published_fingerprints: list[str] = []

    def fail_publication(
        _store: SourceRecordStore,
        _kind: str,
        _key: str,
        payload: bytes,
        *,
        generation: str,
    ) -> NoReturn:
        fingerprint = json.loads(payload)["compiler_fingerprint"]
        assert isinstance(fingerprint, str) and fingerprint == generation
        published_fingerprints.append(fingerprint)
        raise OSError("publication interrupted")

    monkeypatch.setattr(SourceRecordStore, "put", fail_publication)

    def write() -> None:
        scans._write_persisted_import_scan(
            source,
            snapshot=snapshot,
            scan=request,
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
    unknown = PythonNamespacePremise(None, ALL_INVALID_MEMBERS)
    target_scan = _CompleteImportScan(
        _ImportScanRequests(
            **_source_import_scan._collect_imports_for_graph(
                ast.parse(target.read_text(encoding="utf-8")),
                "target",
                import_scan_mode="module_init",
                namespace_premise=unknown,
            )._asdict(),
            source_executions=(),
        ),
        (),
        (),
    )

    def forbidden(*_args: object, **_kwargs: object) -> NoReturn:
        raise AssertionError("explicit artifact-owned execution custody was rescanned")

    for name in (
        "_read_persisted_import_scan_record",
        "_write_persisted_import_scan",
    ):
        monkeypatch.setattr(scans, name, forbidden)
    for name in ("_collect_imports", "_collect_static_source_execution_requests"):
        monkeypatch.setattr(_source_import_scan, name, forbidden)
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
                scan=_CompleteImportScan(
                    _ImportScanRequests(
                        (), (_StaticSourceExecutionRequest("target", str(target)),)
                    ),
                    (),
                    (("target", target),),
                ),
                target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            ),
            target: discovery._bind_precomputed_module_import_scan(
                target,
                snapshot=PythonSourceSnapshot.capture(target),
                module_name="target",
                import_scan_mode="module_init",
                scan=target_scan,
                namespace_premise=unknown,
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


@pytest.mark.parametrize("newline", [b"\n", b"\r\n"])
def test_gpu_reference_capture_is_lazy_and_generation_bound(
    monkeypatch: pytest.MonkeyPatch,
    compiler_sources: Path,
    newline: bytes,
) -> None:
    root = compiler_sources.parents[3]
    reference = root / "src/molt/gpu/__init__.py"
    reference.parent.mkdir(parents=True)
    first = b"VALUE = 1" + newline
    second = b"VALUE = 2" + newline
    reference.write_bytes(first)
    with fingerprints._source_tree_fingerprint_transaction():
        snapshot = fingerprints._frontend_semantic_tooling_snapshot()
        assert snapshot.reference_sources == {}
        captured = snapshot.reference_source("gpu/__init__.py")
        reference.write_bytes(second)
        assert snapshot.reference_source("gpu/__init__.py") is captured
        assert captured.content == first
    with fingerprints._source_tree_fingerprint_transaction():
        next_generation = fingerprints._frontend_semantic_tooling_snapshot()
        assert next_generation.fingerprint != snapshot.fingerprint
        assert next_generation.reference_source("gpu/__init__.py").content == second
    with fingerprints._source_tree_fingerprint_transaction():
        changed_before_capture = fingerprints._frontend_semantic_tooling_snapshot()
        reference.write_bytes(b"VALUE = 3" + newline)
        with pytest.raises(ValueError, match="changed after semantic generation"):
            changed_before_capture.reference_source("gpu/__init__.py")
        assert changed_before_capture.reference_sources == {}


@pytest.mark.parametrize("syntax_stub", [False, True])
def test_frontend_analysis_keeps_the_snapshot_that_supplied_its_facts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    compiler_sources: Path,
    syntax_stub: bool,
) -> None:
    from molt.cli import frontend_pipeline

    source = tmp_path / "dependency.py"
    content = b"VALUE = 1\n"
    source.write_bytes(content)
    snapshot = PythonSourceSnapshot.capture(source)
    graph = {"dependency": source}
    authority = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "dependency", source, "full", source_sha256=snapshot.sha256
            ),
        )
    )
    metadata = module_graph._build_module_graph_metadata(
        graph,
        generated_module_source_paths={},
        entry_module="entry",
        entry_execution_kind="script",
        namespace_module_names=set(),
    )
    seen: list[PythonSourceSnapshot] = []

    def analyze(path: Path, **kwargs: Any) -> tuple[Any, ...]:
        captured = kwargs["source_snapshot"]
        assert captured.content == content
        seen.append(captured)
        before = path.stat()
        path.write_bytes(b"VALUE = 2\n")
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        if syntax_stub:
            raise SyntaxError("invalid captured source")
        return None, (), {}, {}, None, False, before

    monkeypatch.setattr(frontend_pipeline, "_load_module_analysis", analyze)
    resolver = module_resolution._ModuleResolutionCache()
    analysis, error = frontend_pipeline._prepare_frontend_analysis(
        module_graph=graph,
        scan_authority=authority,
        module_graph_metadata=metadata,
        module_resolution_cache=resolver,
        roots=[tmp_path],
        stdlib_root=tmp_path / "stdlib",
        stdlib_allowlist=set(),
        project_root=tmp_path,
        entry_module="entry",
        json_output=False,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert error is None and analysis is not None
    assert len(seen) == 1
    lease = analysis.module_source_catalog.lease_for("dependency", source)
    assert lease.source_sha256 == snapshot.sha256
    assert analysis.module_source_catalog.import_topology is resolver.import_topology
    assert bool(analysis.syntax_error_modules) is syntax_stub
    with pytest.raises(module_source.PythonSourceChangedError):
        analysis.module_source_catalog.validate()


@pytest.mark.parametrize("change", ["same-stat", "remove"])
def test_lowering_cache_cannot_publish_or_serve_a_different_source_generation(
    tmp_path, monkeypatch, compiler_sources, change
):
    import contextlib
    from molt.cli.source_record_store import shared_source_records

    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    path = tmp_path / "owner.py"
    path.write_bytes(b"VALUE = 1\n")
    metadata = path.stat()
    lease = module_source._ModuleSourceLease.path_backed(path)
    digest = "a" * 64
    result = {"functions": [], "function_metadata": {}}
    MC = module_cache
    MC._write_persisted_module_lowering(
        path, source_lease=lease, context_digest=digest, result=result
    )

    def observation():
        record = shared_source_records().get(
            "module-lowering",
            digest,
            generation=MC._frontend_semantic_tooling_fingerprint(),
            max_bytes=None,
        )
        assert record is not None
        return record

    before = observation()
    assert (
        source_records("module-lowering")[digest]["source_sha256"]
        == lease.source_sha256
    )
    loaded = MC._read_persisted_module_lowering(
        path, source_lease=lease, module_name="owner", context_digest=digest
    )
    assert loaded is not None and loaded.pop("function_digest_seeds") == ()
    assert loaded == result
    if change == "same-stat":
        path.write_bytes(b"VALUE = 2\n")
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    else:
        path.unlink()
    with pytest.raises(module_source.PythonSourceChangedError):
        with contextlib.suppress(OSError):
            MC._write_persisted_module_lowering(
                path,
                source_lease=lease,
                context_digest=digest,
                result={"functions": [{"name": "stale"}], "function_metadata": {}},
            )
    assert observation().payload == before.payload
    assert observation().revision == before.revision
    with pytest.raises(module_source.PythonSourceChangedError):
        MC._read_persisted_module_lowering(
            path, source_lease=lease, module_name="owner", context_digest=digest
        )
    assert observation().payload == before.payload
    assert observation().revision == before.revision
