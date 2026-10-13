from __future__ import annotations

import ast
import sys
from pathlib import Path
from typing import Any

import pytest

from molt.cli import (
    module_cache,
    module_graph_cache,
    module_graph_discovery,
    module_resolution,
)
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import PythonSourceSnapshot
from molt.target_python import TargetPythonVersion


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

    monkeypatch.setattr(module_cache, "_write_artifact_sync_payload", forbidden)
    module_cache._write_persisted_module_analysis(
        tmp_path,
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
