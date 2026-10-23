from __future__ import annotations

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

import ast
import sys
from pathlib import Path
from typing import Any

import pytest

from molt.cli.source_record_store import SourceRecordStore
from tests.source_record_test_support import source_records

from molt.cli import (
    frontend_pipeline,
    module_cache,
    module_graph,
    module_graph_cache,
    module_graph_discovery,
    module_resolution,
)
from molt.cli.models import _ModuleGraphScanAuthority, _ModuleSourceScanAuthority
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import PythonSourceSnapshot
from molt.target_python import TargetPythonVersion, _DEFAULT_TARGET_PYTHON_VERSION


@pytest.fixture(autouse=True)
def isolated_caches(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.delenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", raising=False)
    for owner in (module_cache, module_graph_cache):
        monkeypatch.setattr(
            owner, "_frontend_semantic_tooling_fingerprint", lambda: "scan-admission"
        )


def _analyze(path: Path, **overrides: Any) -> tuple[Any, ...]:
    options: dict[str, Any] = {
        "module_name": "app",
        "is_package": False,
        "import_scan_mode": "full",
        "source": None,
        "logical_source_path": str(path),
        "resolution_cache": _ModuleResolutionCache(),
        "project_root": path.parent,
        "roots": (path.parent,),
        "stdlib_root": path.parent / "stdlib",
        "stdlib_allowlist": set(),
    }
    options.update(overrides)
    return module_cache._load_module_analysis(path, **options)


@pytest.fixture
def native_support_source(tmp_path: Path) -> Path:
    path = tmp_path / "app.py"
    path.write_text(
        "import excluded\n"
        "def first(value=11):\n    import alpha\n    return value\n"
        "@ignored_decorator\n"
        "async def later(value=29):\n    import beta\n    return value\n",
        encoding="utf-8",
    )
    return path


@pytest.mark.parametrize("newline", ["\n", "\r\n", "\r"], ids=["lf", "crlf", "cr"])
def test_native_selection_keeps_imports_and_function_facts_in_one_cache_scope(
    native_support_source: Path,
    newline: str,
) -> None:
    path = native_support_source
    # Python's text reader is the independent universal-newline oracle. Keep
    # byte custody separate: analysis must not rewrite the input file.
    source = path.read_text(encoding="utf-8")
    path.write_bytes(source.replace("\n", newline).encode("utf-8"))
    original = path.read_bytes()
    cases = (
        (("later",), ("beta",), {"later": "async"}),
        (("first",), ("alpha",), {"first": "sync"}),
        (("first", "later"), ("alpha", "beta"), {"first": "sync", "later": "async"}),
        ((), ("alpha", "beta", "excluded"), {"first": "sync", "later": "async"}),
    )
    for index, (roots, imports, kinds) in enumerate((*cases, *cases)):
        result = _analyze(path, native_support_function_roots=roots)
        assert set(result[1]) == set(imports)
        assert result[3] == kinds
        assert set(result[2]) == set(kinds)
        if "later" in kinds:
            assert result[2]["later"]["defaults"] == [{"const": True, "value": 29}]
            assert result[2]["later"]["has_decorators"] is (not roots)
        assert result[4] == source
        assert path.read_bytes() == original
        assert result[5] is (index >= len(cases))
    # The wire must retain the distinction even when a support slice includes
    # every definition: complete source also executes imports and decorators.
    for kind in ("module-analysis", "import-scan"):
        selections = [
            record["body_selection"] for record in source_records(kind).values()
        ]
        assert {"kind": "complete_source", "roots": []} in selections
        assert {"kind": "native_support", "roots": ["first", "later"]} in selections


@pytest.mark.parametrize(
    "payload",
    [
        None,
        {},
        [],
        {"kind": "native_support", "roots": []},
        {"kind": "complete_source", "roots": ["f"]},
        {"kind": "native_support", "roots": ["g", "f"]},
        {"kind": "native_support", "roots": ["f", "f"]},
        {"kind": "complete_source", "roots": [], "legacy": True},
    ],
)
def test_producer_selection_requires_explicit_canonical_wire_authority(payload) -> None:
    with pytest.raises(ValueError):
        ModuleBodySelection.from_payload(payload)


def test_native_selection_reuses_cached_scan_for_missing_syntax_facts(
    native_support_source: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = native_support_source
    cache = _ModuleResolutionCache()
    _analyze(
        path,
        resolution_cache=cache,
        native_support_function_roots=("later",),
        retain_tree=False,
    )
    original_trees = {
        key: ast.dump(record.tree, include_attributes=True)
        for key, record in cache.ast_cache.items()
    }
    admissions = []
    load_scan = module_cache._load_module_import_scan

    def record(path: Path, **kwargs: Any) -> Any:
        loaded = load_scan(path, **kwargs)
        admissions.append((loaded.cache_hit, loaded.tree))
        return loaded

    monkeypatch.setattr(module_cache, "_load_module_import_scan", record)
    monkeypatch.setattr(
        module_cache, "_read_persisted_module_analysis", lambda *a, **k: None
    )
    result = _analyze(
        path,
        resolution_cache=cache,
        native_support_function_roots=("later",),
    )
    assert admissions == [(True, None)]
    assert result[1] == ("beta",)
    assert result[3] == {"later": "async"}
    assert isinstance(result[0], ast.Module)
    assert [
        node.name for node in result[0].body if isinstance(node, ast.AsyncFunctionDef)
    ] == ["later"]
    assert original_trees == {
        key: ast.dump(record.tree, include_attributes=True)
        for key, record in cache.ast_cache.items()
    }


@pytest.mark.parametrize("other_roots", [(), ("first",)])
def test_native_selection_payload_rejects_other_root_scope(
    native_support_source: Path, other_roots: tuple[str, ...]
) -> None:
    path = native_support_source
    _analyze(path, native_support_function_roots=("later",))
    key = module_cache._module_analysis_record_key(
        path,
        module_name="app",
        is_package=False,
        import_scan_mode="full",
        native_support_function_roots=("later",),
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    payload = source_records("module-analysis")[key]
    assert payload is not None
    snapshot = PythonSourceSnapshot.capture(path)
    assert (
        module_cache._validate_persisted_module_analysis_payload(
            payload,
            path,
            import_scan_mode="full",
            path_stat=None,
            snapshot=snapshot,
            native_support_function_roots=("later",),
        )
        is not None
    )
    assert (
        module_cache._validate_persisted_module_analysis_payload(
            payload,
            path,
            import_scan_mode="full",
            path_stat=None,
            snapshot=snapshot,
            native_support_function_roots=other_roots,
        )
        is None
    )


def test_frontend_analysis_uses_admitted_native_selection(
    native_support_source: Path,
) -> None:
    path = native_support_source
    graph = {"app": path}
    authority = _ModuleGraphScanAuthority(
        (
            _ModuleSourceScanAuthority(
                "app",
                path,
                "full",
                body_selection=ModuleBodySelection.from_requested_roots(("later",)),
            ),
        )
    )
    analysis, error = frontend_pipeline._prepare_frontend_analysis(
        module_graph=graph,
        scan_authority=authority,
        module_graph_metadata=module_graph._build_module_graph_metadata(
            graph,
            generated_module_source_paths={},
            entry_module="app",
            entry_execution_kind="script",
            namespace_module_names=set(),
        ),
        module_resolution_cache=_ModuleResolutionCache(),
        roots=[path.parent],
        stdlib_root=path.parent / "stdlib",
        stdlib_allowlist=set(),
        project_root=path.parent,
        entry_module="app",
        json_output=False,
        target_python=TargetPythonVersion(3, 12, 0),
    )
    assert error is None and analysis is not None
    assert analysis.known_func_kinds == {"app": {"later": "async"}}
    assert set(analysis.known_func_defaults["app"]) == {"later"}


def test_discovery_tree_is_reused_readonly_by_unretained_analysis(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "app.py"
    helper = tmp_path / "helper.py"
    leaf = tmp_path / "leaf.py"
    entry.write_text("from helper import f\n", encoding="utf-8")
    helper.write_text(
        "def f(value=1):\n    import leaf\n    return value\n"
        "class Private:\n    __value = 2\n",
        encoding="utf-8",
    )
    leaf.write_text("VALUE = 1\n", encoding="utf-8")
    cache = _ModuleResolutionCache()
    parsed: list[str] = []
    parse = module_resolution._parse_source_for_target

    def record(source: str, **kwargs: Any) -> ast.Module:
        parsed.append(kwargs["filename"])
        return parse(source, **kwargs)

    monkeypatch.setattr(module_resolution, "_parse_source_for_target", record)
    discovered = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=True,
        resolver_cache=cache,
    )
    assert discovered.graph == {"app": entry, "helper": helper}
    assert sorted(parsed) == sorted((str(entry), str(helper)))
    retained = {
        key: (record, ast.dump(record.tree, include_attributes=True))
        for key, record in cache.ast_cache.items()
    }
    result = _analyze(
        helper,
        module_name="helper",
        project_root=None,
        resolution_cache=cache,
        retain_source=False,
        retain_tree=False,
    )
    assert sorted(parsed) == sorted((str(entry), str(helper)))
    # Full analysis still admits deferred imports and function facts. Reuse
    # cannot substitute discovery's module-init-only projection for this scan.
    assert result[0] is None and result[4] is None
    assert result[1] == ("leaf",)
    assert result[2]["f"]["params"] == 1
    assert result[3] == {"f": "sync"}
    assert cache.ast_cache.keys() == retained.keys()
    for key, (record, before) in retained.items():
        assert cache.ast_cache[key] is record
        assert ast.dump(record.tree, include_attributes=True) == before


@pytest.mark.parametrize("changed", ["source", "filename", "target", "path"])
def test_unretained_parse_reuses_only_identical_source_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, changed: str
) -> None:
    path = tmp_path / "app.py"
    source = "VALUE = 1\n"
    filename = str(path)
    target = TargetPythonVersion(3, 12, 0)
    cache = _ModuleResolutionCache()
    calls: list[tuple[Path, str, str]] = []
    parse = module_resolution._parse_source_for_target

    def record(text: str, **kwargs: Any) -> ast.Module:
        calls.append((Path(kwargs["filename"]), text, kwargs["target_python"].tag))
        return parse(text, **kwargs)

    monkeypatch.setattr(module_resolution, "_parse_source_for_target", record)
    first = cache.parse_module_ast(
        path, source, filename=filename, target_python=target
    )
    assert (
        cache.parse_module_ast(
            path, source, filename=filename, target_python=target, retain=False
        )
        is first
    )
    assert len(calls) == 1
    if changed == "source":
        source = "VALUE = 2\n"
    elif changed == "filename":
        filename = "display/app.py"
    elif changed == "target":
        target = TargetPythonVersion(3, 13, 0)
    else:
        path = tmp_path / "other.py"
    if target.feature_version > sys.version_info[:2]:
        for _ in range(2):
            with pytest.raises(SyntaxError, match="requires a Python 3.13"):
                cache.parse_module_ast(
                    path, source, filename=filename, target_python=target, retain=False
                )
        assert len(calls) == 3
        assert len(cache.ast_cache) == 1
        assert cache.ast_error_cache == {}
        return
    second = cache.parse_module_ast(
        path, source, filename=filename, target_python=target, retain=False
    )
    third = cache.parse_module_ast(
        path, source, filename=filename, target_python=target, retain=False
    )
    assert second is not first and third is not second
    assert len(calls) == 3
    assert len(cache.ast_cache) == 1
    assert calls[-1] == (Path(filename), source, target.tag)
    assert ast.dump(second.tree) == ast.dump(ast.parse(source))


def test_unretained_cold_parse_and_error_do_not_populate_caches(tmp_path: Path) -> None:
    path = tmp_path / "app.py"
    cache = _ModuleResolutionCache()
    first = cache.parse_module_ast(
        path, "VALUE = 1\n", filename=str(path), retain=False
    )
    second = cache.parse_module_ast(
        path, "VALUE = 1\n", filename=str(path), retain=False
    )
    assert first is not second
    for filename in ("first.py", "second.py"):
        with pytest.raises(SyntaxError) as caught:
            cache.parse_module_ast(
                path, "def invalid(\n", filename=filename, retain=False
            )
        assert caught.value.filename == filename
    assert cache.ast_cache == {} and cache.ast_error_cache == {}


def test_warm_function_facts_still_admit_live_scan_without_parsing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "app.py"
    path.write_text("import helper\ndef f(value=1): return value\n", encoding="utf-8")
    first = _analyze(path)
    admitted: list[Path] = []
    load_scan = module_cache._load_module_import_scan

    def record(path: Path, **kwargs: Any) -> Any:
        admitted.append(path)
        return load_scan(path, **kwargs)

    def forbidden(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("warm pure scan and function facts must not parse")

    cache = _ModuleResolutionCache()
    monkeypatch.setattr(module_cache, "_load_module_import_scan", record)
    monkeypatch.setattr(cache, "parse_module_ast", forbidden)
    second = _analyze(path, resolution_cache=cache)
    assert admitted == [path]
    assert second[1:4] == first[1:4]
    assert second[5] is True


def test_warm_analysis_resolves_changed_package_all_live(tmp_path: Path) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    initializer = package / "__init__.py"
    initializer.write_text("__all__ = ['a']\n", encoding="utf-8")
    for name in ("a", "b"):
        (package / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")
    path = tmp_path / "app.py"
    path.write_text(
        "from pkg import *\ndef f(value=1): return value\n", encoding="utf-8"
    )
    first = _analyze(path)
    assert "pkg.a" in first[1] and "pkg.b" not in first[1]
    initializer.write_text("__all__ = ['b']\n", encoding="utf-8")
    second = _analyze(path)
    assert "pkg.b" in second[1] and "pkg.a" not in second[1]
    assert second[2:4] == first[2:4]
    assert second[5] is True


def test_warm_analysis_recompletes_loader_target_presence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "app.py"
    path.write_text(
        "import importlib.util\n"
        "importlib.util.spec_from_file_location('loaded', 'loaded.py')\n"
        "def f(value=1): return value\n",
        encoding="utf-8",
    )
    target = tmp_path / "loaded.py"
    executions: list[tuple[tuple[str | None, Path], ...]] = []
    load_scan = module_cache._load_module_import_scan

    def record(path: Path, **kwargs: Any) -> Any:
        loaded = load_scan(path, **kwargs)
        executions.append(loaded.scan.source_executions)
        return loaded

    monkeypatch.setattr(module_cache, "_load_module_import_scan", record)
    _analyze(path)
    target.write_text("VALUE = 1\n", encoding="utf-8")
    second = _analyze(path)
    target.unlink()
    third = _analyze(path)
    assert executions == [(), (("loaded", target.resolve()),), ()]
    assert second[5] is True and third[5] is True


def test_explicit_source_never_uses_disk_function_metadata(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "app.py"
    path.write_text("import disk\ndef f(value=1): return value\n", encoding="utf-8")
    _analyze(path)
    supplied = "import supplied\ndef f(value=2): return value\n"

    def forbidden(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("supplied source must remain operation-local")

    monkeypatch.setattr(module_cache, "_read_persisted_module_analysis", forbidden)
    monkeypatch.setattr(module_cache, "_write_persisted_module_analysis", forbidden)
    result = _analyze(path, source=supplied)
    assert result[1] == ("supplied",)
    assert result[2] == module_cache._collect_func_defaults(ast.parse(supplied))
    assert result[4] == supplied
    assert result[5] is False


def test_changed_source_misses_despite_equal_function_facts(
    tmp_path: Path,
) -> None:
    path = tmp_path / "app.py"
    path.write_text("def f(value=1): return 1\n", encoding="utf-8")
    first = _analyze(path)
    path.write_text("def f(value=1): return 2\n", encoding="utf-8")
    second = _analyze(path)
    assert first[2:4] == second[2:4]
    assert second[5] is False


def test_analysis_publication_rejects_a_changed_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "app.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    snapshot = PythonSourceSnapshot.capture(path)
    path.write_text("VALUE = 2\n", encoding="utf-8")

    def forbidden(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("changed source must not publish analysis metadata")

    monkeypatch.setattr(SourceRecordStore, "put", forbidden)
    module_cache._write_persisted_module_analysis(
        path,
        module_name="app",
        is_package=False,
        import_scan_mode="full",
        func_defaults={},
        func_kinds={},
        snapshot=snapshot,
    )


@pytest.mark.parametrize("current_schema", [False, True])
@pytest.mark.parametrize("has_imports", [False, True])
@pytest.mark.parametrize("admitted_snapshot", [False, True])
def test_function_metadata_payload_requires_current_schema_without_imports(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    current_schema: bool,
    has_imports: bool,
    admitted_snapshot: bool,
) -> None:
    path = tmp_path / "app.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    snapshot = PythonSourceSnapshot.capture(path)
    payload = {
        "version": module_cache._MODULE_ANALYSIS_CACHE_SCHEMA_VERSION
        - (0 if current_schema else 1),
        "compiler_fingerprint": "scan-admission",
        "import_scan_mode": "full",
        "body_selection": {"kind": "complete_source", "roots": []},
        "func_defaults": {},
        "func_kinds": {},
        "size": len(snapshot.content),
        "source_sha256": snapshot.sha256,
    }
    options: dict[str, Any] = {
        "import_scan_mode": "full",
        "path_stat": path.stat(),
        "snapshot": snapshot if admitted_snapshot else None,
    }
    if admitted_snapshot:

        def reread(*args, **kwargs):
            raise AssertionError("captured byte admission must not reread the path")

        monkeypatch.setattr(module_cache, "_payload_source_matches", reread)
    if has_imports:
        payload["imports"] = ["stale.child"]
    result = module_cache._validate_persisted_module_analysis_payload(
        payload, path, **options
    )
    assert result == (({}, {}) if current_schema and not has_imports else None)
