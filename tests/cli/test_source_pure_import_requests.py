"""Warm source-request reuse must never retain filesystem-derived edges."""

from __future__ import annotations

import ast
import ntpath
import os
import posixpath
import sys
from pathlib import Path

import pytest

from molt.cli import module_graph_cache as records
from molt.cli import module_graph_discovery as discovery
from molt.cli import module_import_scanner as scanner
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import PythonSourceSnapshot
from molt.target_python import TargetPythonVersion


@pytest.fixture(autouse=True)
def tooling_identity(monkeypatch):
    monkeypatch.setattr(
        records, "_frontend_semantic_tooling_fingerprint", lambda: "requests"
    )


def write(path: Path, text: str = "pass\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def load(path: Path, **kwargs):
    options = dict(
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=_ModuleResolutionCache(),
        project_root=path.parent,
        roots=[path.parent],
        stdlib_root=path.parent / "stdlib",
        stdlib_allowlist=set(),
    )
    options.update(kwargs)
    return discovery._load_module_import_scan(path, **options)


def test_warm_star_exports_and_child_presence_are_live(tmp_path):
    source = write(tmp_path / "entry.py", "from pkg import *\n")
    package = write(tmp_path / "pkg/__init__.py", "__all__ = ['a']\n")
    a = write(tmp_path / "pkg/a.py")
    b = tmp_path / "pkg/b.py"
    assert "pkg.a" in load(source).scan.imports
    write(package, "__all__ = ['b']\n")
    absent = load(source)
    assert absent.cache_hit and "pkg.a" not in absent.scan.imports
    assert "pkg.b" not in absent.scan.imports
    write(b)
    present = load(source)
    assert present.cache_hit and "pkg.b" in present.scan.imports
    b.unlink()
    assert "pkg.b" not in load(source).scan.imports
    assert a.is_file()


@pytest.mark.parametrize(
    "expression",
    ["'loaded.py'", "Path('loaded.py').resolve()", "Path('loaded.py').absolute()"],
)
def test_loader_existence_is_live(tmp_path, expression):
    source = write(
        tmp_path / "entry.py",
        "from pathlib import Path\nfrom importlib.util import spec_from_file_location\n"
        f"spec_from_file_location('loaded', {expression})\n",
    )
    target = tmp_path / "loaded.py"
    assert load(source).scan.source_executions == ()
    write(target)
    present = load(source)
    assert present.cache_hit
    assert present.scan.source_executions == (("loaded", target.resolve()),)
    target.unlink()
    absent = load(source)
    assert absent.cache_hit and absent.scan.source_executions == ()


def test_run_path_directory_main_creation_and_deletion(tmp_path):
    source = write(tmp_path / "entry.py", "import runpy\nrunpy.run_path('site')\n")
    assert load(source).scan.source_executions == ()
    main = write(tmp_path / "site/__main__.py")
    assert load(source).scan.source_executions == ((None, main.resolve()),)
    main.unlink()
    assert load(source).scan.source_executions == ()


def test_path_request_extraction_never_reads_filesystem(tmp_path, monkeypatch):
    source = tmp_path / "entry.py"
    tree = ast.parse(
        "from pathlib import Path\nimport runpy\n"
        "ROOT = Path('link').resolve() / '..'\nrunpy.run_path(ROOT / 'loaded.py')\n"
    )

    def forbidden(*args, **kwargs):
        raise AssertionError("source-only extraction consulted the filesystem")

    with monkeypatch.context() as local:
        for operation in ("resolve", "absolute", "is_file", "is_dir"):
            local.setattr(Path, operation, forbidden)
        requests = scanner._collect_static_source_execution_requests(
            tree, source_path=source
        )
    assert requests


def test_resolution_before_join_is_replayed_after_symlink_change(tmp_path):
    left = tmp_path / "left/nested"
    right = tmp_path / "right/nested"
    left.mkdir(parents=True)
    right.mkdir(parents=True)
    first = write(left.parent / "loaded.py", "FIRST = True\n")
    second = write(right.parent / "loaded.py", "SECOND = True\n")
    link = tmp_path / "link"
    try:
        link.symlink_to(left, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"host lacks directory symlink capability: {exc}")
    source = write(
        tmp_path / "entry.py",
        "from pathlib import Path\nimport runpy\n"
        "runpy.run_path(Path('link').resolve() / '..' / 'loaded.py')\n",
    )
    assert load(source).scan.source_executions == ((None, first.resolve()),)
    link.unlink()
    link.symlink_to(right, target_is_directory=True)
    result = load(source)
    assert result.cache_hit
    assert result.scan.source_executions == ((None, second.resolve()),)


@pytest.mark.parametrize("operation", ["absolute", "resolve"])
def test_path_operation_preserves_host_symlink_and_parent_semantics(
    tmp_path, operation
):
    target = tmp_path / "target/nested"
    target.mkdir(parents=True)
    write(tmp_path / "loaded.py")
    write(target.parent / "loaded.py")
    link = tmp_path / "link"
    try:
        link.symlink_to(target, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"host lacks directory symlink capability: {exc}")
    source = write(
        tmp_path / "entry.py",
        "from pathlib import Path\nimport runpy\n"
        f"runpy.run_path(Path({str(link)!r}).{operation}() / '..' / 'loaded.py')\n",
    )
    expected = (getattr(link, operation)() / ".." / "loaded.py").resolve()
    assert load(source).scan.source_executions == ((None, expected),)
    warm = load(source)
    assert warm.cache_hit and warm.scan.source_executions == ((None, expected),)


def test_whole_graph_rediscovers_new_higher_priority_candidate(tmp_path):
    source = write(tmp_path / "entry.py", "import helper\n")
    high, low = tmp_path / "high", tmp_path / "low"
    high.mkdir()
    original = write(low / "helper.py")

    def graph():
        return discovery._discover_module_graph(
            source,
            [high, low, tmp_path],
            [high, low, tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
        ).graph

    assert graph()["helper"] == original
    shadow = write(high / "helper.py")
    assert graph()["helper"] == shadow
    shadow.unlink()
    assert graph()["helper"] == original
    assert not hasattr(records, "_read_persisted_module_graph")


def test_resolution_memo_keys_roots_and_allowlist(tmp_path):
    app, stdlib = tmp_path / "app", tmp_path / "stdlib"
    app_module = write(app / "example.py")
    standard_module = write(stdlib / "example.py")
    resolver = _ModuleResolutionCache()
    assert resolver.resolve_module("example", [app], stdlib, set()) == app_module
    assert (
        resolver.resolve_module("example", [stdlib], stdlib, set()) == standard_module
    )
    # An allowlisted stdlib module is deliberately resolved from stdlib first.
    assert (
        resolver.resolve_module("example", [app], stdlib, {"example"})
        == standard_module
    )


def test_scan_publication_never_pairs_new_bytes_with_old_requests(
    tmp_path, monkeypatch
):
    source = write(tmp_path / "entry.py", "import before\n")
    original = records._write_persisted_import_scan

    def mutate(project, path, **kwargs):
        write(path, "import after\n")
        original(project, path, **kwargs)

    monkeypatch.setattr(records, "_write_persisted_import_scan", mutate)
    assert load(source).scan.imports == ("before",)
    monkeypatch.setattr(records, "_write_persisted_import_scan", original)
    result = load(source)
    assert not result.cache_hit and result.scan.imports == ("after",)


def test_supplied_tree_cannot_be_overridden_or_persisted(tmp_path):
    source = write(tmp_path / "entry.py", "import disk\n")
    assert load(source).scan.imports == ("disk",)
    supplied = load(source, tree=ast.parse("import supplied\n"))
    assert not supplied.cache_hit
    assert supplied.scan.imports == ("supplied",)
    assert load(source).scan.imports == ("disk",)


def test_supplied_source_does_not_require_a_file(tmp_path):
    source = tmp_path / "not_materialized.py"
    result = load(source, source="import inline\n")
    assert result.scan.imports == ("inline",)
    assert result.snapshot is None and not result.cache_hit
    assert not source.exists()


@pytest.mark.parametrize(
    ("module", "joiner"),
    [("os.path", os.path.join), ("posixpath", posixpath.join), ("ntpath", ntpath.join)],
)
@pytest.mark.parametrize("parts", [("a/b", "/reset", "leaf.py"), ("a", "b", "leaf.py")])
def test_explicit_join_flavor_matches_host_python(tmp_path, module, joiner, parts):
    tree = ast.parse(
        f"import {module}\nimport runpy\n"
        f"runpy.run_path({module}.join({', '.join(repr(p) for p in parts)}))\n"
    )
    source = tmp_path / "entry.py"
    requests = scanner._collect_static_source_execution_requests(
        tree, source_path=source
    )
    assert len(requests) == 1
    value = scanner._resolve_static_source_path(requests[0].path, source)
    assert value == joiner(*parts)


def test_live_completion_deduplicates_equivalent_paths(tmp_path):
    source = write(
        tmp_path / "entry.py",
        "from pathlib import Path\nimport runpy\n"
        "runpy.run_path('loaded.py')\n"
        "runpy.run_path(Path('loaded.py').resolve())\n",
    )
    target = write(tmp_path / "loaded.py")
    for _ in range(2):
        assert load(source).scan.source_executions == ((None, target.resolve()),)


@pytest.mark.parametrize("minor", [12, 13, 14])
def test_static_loader_requests_preserve_target_and_conservative_branches(
    tmp_path, monkeypatch, minor
):
    source = write(
        tmp_path / "entry.py",
        "import sys\nimport runpy\n"
        "if sys.version_info >= (3, 13):\n    runpy.run_path('new.py')\n"
        "else:\n    runpy.run_path('old.py')\n",
    )
    new = write(tmp_path / "new.py")
    old = write(tmp_path / "old.py")
    target = TargetPythonVersion(3, minor, 0)
    # Mutable sys metadata is not presently a sealed compile-time version fact.
    # Preserve both possible edges, while proving every extractor uses the
    # selected target (including when it differs from the default).
    expected = ((None, new.resolve()), (None, old.resolve()))
    seen = []
    original = scanner._collect_static_source_execution_requests

    def collect(*args, **kwargs):
        seen.append(kwargs["target_python"])
        return original(*args, **kwargs)

    monkeypatch.setattr(scanner, "_collect_static_source_execution_requests", collect)
    result = load(source, target_python=target, tree=ast.parse(source.read_text()))
    assert result.scan.source_executions == expected
    assert seen == [target]
    if minor <= sys.version_info.minor:
        assert load(source, target_python=target).scan.source_executions == expected
        warm = load(source, target_python=target)
        assert warm.cache_hit and warm.scan.source_executions == expected
    else:
        with pytest.raises(SyntaxError, match="requires a Python"):
            load(source, target_python=target)


def test_resolver_ast_memo_distinguishes_source_generations(tmp_path):
    path = tmp_path / "entry.py"
    cache = _ModuleResolutionCache()
    first = cache.parse_module_ast(path, "a = 1", filename=str(path))
    second = cache.parse_module_ast(path, "a = 2", filename=str(path))
    assert ast.dump(first) != ast.dump(second)


@pytest.mark.parametrize(
    "bad_path",
    [None, [], {"operation": [], "parts": ["x"]}, {"operation": "join", "parts": []}],
)
def test_malformed_path_request_misses_without_crashing(
    tmp_path, monkeypatch, bad_path
):
    source = write(tmp_path / "entry.py")
    load(source)
    original = records._read_artifact_sync_state

    def corrupt(path):
        payload = original(path)
        assert payload is not None
        return dict(payload, source_executions=[{"module": "bad", "path": bad_path}])

    monkeypatch.setattr(records, "_read_artifact_sync_state", corrupt)
    assert (
        records._read_persisted_import_scan_record(
            tmp_path,
            source,
            module_name="entry",
            is_package=False,
            import_scan_mode="full",
            snapshot=PythonSourceSnapshot.capture(source),
        )
        is None
    )
