"""Warm source-request reuse must never retain filesystem-derived edges."""

from __future__ import annotations

from molt.compiler_analysis.python_import_scan import SourceImportPurpose
from molt.compiler_analysis.python_string_facts import SourceStringArguments

from molt.compiler_analysis import python_import_scan as _source_import_scan

import ast
import copy
import ntpath
import os
import posixpath
import sys
from pathlib import Path
from types import ModuleType
from typing import Callable, cast

import pytest

from molt.compiler_analysis.python_binding_facts import PythonSourceCall
from molt.cli import module_graph_cache as records
from molt.cli import module_graph_discovery as discovery
from molt.cli import module_import_scanner as scanner
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import PythonSourceSnapshot
from molt.cli.models import ImportScanAdmission
from molt.target_python import TargetPythonVersion, _DEFAULT_TARGET_PYTHON_VERSION
from tests.source_record_test_support import replace_source_record, source_records

from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

# The scanned module is the whole closed program, so no code holds a PEP 667
# frame-locals writer (HF-901) and callbacks leave its locals in SSA.
WHOLE_PROGRAM = PythonNamespacePremise((), 0, 0, ())


@pytest.fixture(autouse=True)
def tooling_identity(monkeypatch, isolated_molt_cache):
    monkeypatch.setattr(records, "_import_scan_tooling_fingerprint", lambda: "requests")


def write(path: Path, text: str = "pass\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def load(path: Path, **kwargs):
    options = dict(
        purpose=SourceImportPurpose.EXECUTION,
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


@pytest.mark.parametrize(
    ("setup", "target"),
    [
        ("name, *options = '  spark ,opt=true'.split(',')", "name.strip()"),
        (
            "name, *options = '  spark '.split(sep=',', maxsplit=0)",
            "name.lstrip().rstrip()",
        ),
        (
            "name, *options = b' presto ,opt=true'.split(b',')\n"
            "if name == b' presto ':\n"
            "    kind = 'presto'\n"
            "else:\n"
            "    kind = 'other'",
            "kind",
        ),
    ],
)
def test_exact_text_operations_preserve_import_names(
    monkeypatch: pytest.MonkeyPatch, setup: str, target: str
) -> None:
    import importlib

    source = (
        f"import importlib\n{setup}\n"
        f"importlib.import_module(f'dialects.{{{target}}}')\n"
    )
    imports = _source_import_scan._collect_imports_for_graph(
        ast.parse(source), "entry", import_scan_mode="module_init"
    ).imports
    observed = []
    with monkeypatch.context() as runtime:

        def import_module(name: str) -> ModuleType:
            observed.append(name)
            return ModuleType(name)

        runtime.setattr(importlib, "import_module", import_module)
        exec(compile(source, "<text-import-oracle>", "exec"), {})
    assert set(imports) == {"importlib", *observed}


def test_source_demand_rows_preserve_admitted_and_pending_slots_across_storage() -> (
    None
):
    from molt.compiler_analysis.python_source_demands import (
        SourceDemand,
        _SourceDemandIncidences,
    )

    rows = _SourceDemandIncidences()
    value = ("module", None)
    seed = SourceDemand("escape", (), None, "seed")
    assert rows.add(value, seed)
    assert tuple(rows.take_pending(value, include_existing=False)[1]) == (seed,)

    for index in range(rows._DENSE_SLOTS):
        name = f"peer_{index}"
        rows.add((name, None), SourceDemand("escape", (), None, name))

    delayed = SourceDemand("call", ((0, "1"),), 1, "consumer")
    earlier = rows.demands(value)
    assert rows.add(value, delayed)
    assert not rows.add(value, delayed)
    assert tuple(earlier) == (seed,)
    known, pending = rows.take_pending(value, include_existing=True)
    assert tuple(known) == (seed,)
    assert tuple(pending) == (delayed,)
    assert tuple(rows.demands(value)) == (seed, delayed)


def test_deferred_writer_refines_only_the_selected_namespace_key(
    tmp_path: Path,
) -> None:
    from molt.compiler_analysis.python_source_keys import python_node_source_key
    from molt.compiler_analysis.python_string_facts import PythonStringBound

    path = write(
        tmp_path / "entry.py",
        "import sys\n"
        "def publish(name, value):\n"
        "    key = f'plugins.{name}'\n"
        "    sys.modules[key] = value\n"
        "def unused(name, value):\n"
        "    key = f'other.{name}'\n"
        "    sys.modules[key] = value\n",
    )
    _, publish, unused = ast.parse(path.read_text(encoding="utf-8")).body
    owner = python_node_source_key(publish)
    options = dict(import_scan_mode="module_init", namespace_premise=WHOLE_PROGRAM)
    initializer = load(path, **options).scan.source_requests.namespace_summary
    assert initializer is not None
    assert tuple(row.source_owner for row in initializer.foreign_writes) == (
        owner,
        python_node_source_key(unused),
    )
    assert all(row.needs_binding_refinement for row in initializer.foreign_writes)

    selected = load(path, source_owner=owner, **options)
    summary = selected.scan.source_requests.namespace_summary
    assert summary is not None
    (write_fact,) = summary.foreign_writes
    assert write_fact.source_owner == owner
    assert write_fact.key_bound == PythonStringBound("plugins.", "", 8)
    assert not write_fact.needs_binding_refinement
    warm = load(path, source_owner=owner, **options)
    assert warm.query is not None and warm.query.source_owner == owner
    assert warm.scan == selected.scan
    assert warm.cache_hit and not warm.source_parsed


def test_execution_scan_publishes_only_the_activated_body_result(
    tmp_path: Path,
) -> None:
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    text = (
        "def returned(): pass\n"
        "def factory(): return returned\n"
        "def unrelated(): return factory\n"
    )
    source = write(tmp_path / "entry.py", text)
    returned, factory, unrelated = map(python_node_source_key, ast.parse(text).body)

    initialization = load(source).scan.source_values
    assert {entry.owner for entry in initialization.declarations} == {
        returned,
        factory,
        unrelated,
    }
    assert initialization.functions == ()

    active = load(source, source_owner=factory, import_scan_mode="module_init")
    (result,) = active.scan.source_values.functions
    assert result.owner == factory
    assert result.returned == frozenset({returned})
    assert tuple(row.owner for row in active.scan.source_values.declarations) == (
        factory,
    )
    warm = load(source, source_owner=factory, import_scan_mode="module_init")
    assert warm.cache_hit
    assert warm.scan.source_values == active.scan.source_values


def test_import_query_records_preserve_python_string_identity(tmp_path: Path) -> None:
    path = write(
        tmp_path / "entry.py",
        "def resolve(name): return __import__(name)\n"
        "resolve('\\ud83d\\ude00')\n"
        "resolve('\\U0001f600')\n",
    )
    pair, scalar = "\ud83d\ude00", "\U0001f600"
    resolver = _ModuleResolutionCache()
    cold = load(path, resolution_cache=resolver)
    warm = load(path, resolution_cache=resolver)
    assert not cold.cache_hit and warm.cache_hit
    assert cold.scan == warm.scan
    found = {
        item.argument_strings[0]
        for item in warm.scan.source_requests.source_values.calls
        if item.argument_strings
    }
    assert {pair, scalar} <= found
    assert len(pair) == 2 and len(scalar) == 1

    options = dict(module_name="entry", is_package=False, import_scan_mode="full")
    assert records._import_scan_cache_key(
        path, **options, source_arguments=((0, pair),)
    ) != records._import_scan_cache_key(
        path, **options, source_arguments=((0, scalar),)
    )

    from molt.cli.import_graph_record import ImportResultDigests

    assert cold.query is not None
    requests = cold.scan.source_requests
    pair_scan = cold.scan._replace(source_requests=requests._replace(imports=(pair,)))
    scalar_scan = cold.scan._replace(
        source_requests=requests._replace(imports=(scalar,))
    )
    digests = ImportResultDigests()
    assert digests.digest(cold.query, pair_scan) != digests.digest(
        cold.query, scalar_scan
    )


def test_import_query_capture_unifies_source_persisted_and_precomputed(
    tmp_path: Path,
) -> None:
    path = write(tmp_path / "entry.py", "import leaf\n")
    write(tmp_path / "leaf.py")
    resolver = _ModuleResolutionCache()
    cold = load(path, resolution_cache=resolver)
    warm = load(path, resolution_cache=resolver)
    assert not cold.cache_hit and warm.cache_hit
    assert cold.snapshot is not None
    # Use the boundary's actual default version, just as the producer did.
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    bound = discovery._bind_precomputed_module_import_scan(
        path,
        module_name="entry",
        import_scan_mode="full",
        purpose=SourceImportPurpose.EXECUTION,
        scan=cold.scan,
        snapshot=cold.snapshot,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        target_sys_platform=None,
    )
    precomputed = load(path, resolution_cache=resolver, precomputed_scan=bound)
    closure = resolver.import_query_closure()
    assert len(closure.observations) == 1
    observation = closure.observations[0]
    assert observation.result == cold.scan == warm.scan == precomputed.scan
    assert observation.admissions == frozenset(
        (
            ImportScanAdmission.SOURCE,
            ImportScanAdmission.PERSISTED,
            ImportScanAdmission.PRECOMPUTED,
        )
    )
    closure.validate()


def test_import_query_closure_fences_negative_resolution_and_source(
    tmp_path: Path,
) -> None:
    path = write(tmp_path / "entry.py", "import runpy\nrunpy.run_path('loaded.py')\n")
    resolver = _ModuleResolutionCache()
    assert load(path, resolution_cache=resolver).scan.source_executions == ()
    closure = resolver.import_query_closure()
    closure.validate()
    write(tmp_path / "loaded.py")
    with pytest.raises(ValueError, match="topology changed"):
        closure.validate()

    resolver = _ModuleResolutionCache()
    load(path, resolution_cache=resolver)
    closure = resolver.import_query_closure()
    write(path, "import runpy\nrunpy.run_path('loaded.py')\nVALUE = 2\n")
    with pytest.raises(ValueError, match="source generation changed"):
        closure.validate()


@pytest.mark.parametrize("inventory_first", [False, True])
def test_warm_inventory_and_execution_requests_stay_disjoint(tmp_path, inventory_first):
    source = write(tmp_path / "entry.py", "def available():\n    import leaf\n")
    write(tmp_path / "leaf.py")
    purposes = (
        (SourceImportPurpose.INVENTORY, SourceImportPurpose.EXECUTION)
        if inventory_first
        else (SourceImportPurpose.EXECUTION, SourceImportPurpose.INVENTORY)
    )
    for index, purpose in enumerate((*purposes, *purposes)):
        # Each load has a fresh resolver: a hit proves persisted request reuse,
        # rather than the in-memory AST/projection cache hiding a wrong record.
        result = load(source, purpose=purpose)
        assert set(result.scan.imports) == (
            {"leaf"} if purpose is SourceImportPurpose.INVENTORY else set()
        )
        assert result.cache_hit is (index >= len(purposes))
    payloads = tuple(source_records("import-scan").values())
    assert {payload["purpose"] for payload in payloads} == {
        "inventory",
        "execution",
    }


def test_linked_runtime_prunes_host_fallback_import_on_cold_and_warm_scan(
    tmp_path: Path,
) -> None:
    entry = write(
        tmp_path / "entry.py",
        "from _intrinsics import runtime_active as available\n"
        "if available():\n"
        "    import guest_runtime\n"
        "else:\n"
        "    import host_fallback\n",
    )
    write(tmp_path / "guest_runtime.py")
    write(tmp_path / "host_fallback.py")
    cold = load(entry)
    warm = load(entry)
    assert not cold.cache_hit and warm.cache_hit
    assert cold.scan == warm.scan
    assert "guest_runtime" in warm.scan.imports
    assert "host_fallback" not in warm.scan.imports


def test_intrinsic_evidence_only_warms_inventory_requests(tmp_path: Path) -> None:
    source = write(tmp_path / "entry.py", "def deferred():\n    import leaf\n")
    write(tmp_path / "leaf.py")
    target = TargetPythonVersion(3, 12, 0)
    records._stdlib_intrinsic_source_facts("entry", source, target_python=target)

    inventory = load(
        source, purpose=SourceImportPurpose.INVENTORY, target_python=target
    )
    assert inventory.cache_hit and inventory.scan.imports == ("leaf",)

    execution = load(source, target_python=target)
    assert not execution.cache_hit
    assert execution.scan.imports == ()


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


def test_relocated_source_reuses_requests_and_resolves_its_own_edges(
    tmp_path: Path,
) -> None:
    text = "import runpy\nrunpy.run_path('loaded.py')\n"
    original = write(tmp_path / "original/entry.py", text)
    original_child = write(original.with_name("loaded.py"), "VALUE = 1\n")
    cold = load(original)
    assert not cold.cache_hit
    assert cold.scan.source_executions == ((None, original_child),)

    relocated = write(tmp_path / "relocated/entry.py", text)
    relocated_child = write(relocated.with_name("loaded.py"), "VALUE = 2\n")
    warm = load(relocated)
    assert warm.cache_hit
    assert warm.scan.source_requests == cold.scan.source_requests
    assert warm.scan.source_executions == ((None, relocated_child),)

    write(relocated, "import runpy\nrunpy.run_path('absent.py')\n")
    changed = load(relocated)
    assert not changed.cache_hit
    assert changed.scan.source_executions == ()


@pytest.mark.parametrize(
    "source",
    [
        "import runpy\nrunpy.ｒｕｎ＿ｐａｔｈ('loaded.py')\n",
        "from runpy import ｒｕｎ＿ｐａｔｈ as execute\nexecute('loaded.py')\n",
    ],
)
def test_source_execution_uses_parsed_identifiers(tmp_path: Path, source: str) -> None:
    entry = write(tmp_path / "entry.py", source)
    child = write(tmp_path / "loaded.py")
    assert load(entry).scan.source_executions == ((None, child),)


def test_run_path_directory_main_creation_and_deletion(tmp_path):
    source = write(tmp_path / "entry.py", "import runpy\nrunpy.run_path('site')\n")
    assert load(source).scan.source_executions == ()
    main = write(tmp_path / "site/__main__.py")
    assert load(source).scan.source_executions == ((None, main.resolve()),)
    main.unlink()
    assert load(source).scan.source_executions == ()


def test_path_request_extraction_never_reads_filesystem(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tree = ast.parse(
        "from pathlib import Path\nimport runpy\n"
        "ROOT = Path('link').resolve() / '..'\nrunpy.run_path(ROOT / 'loaded.py')\n"
    )

    def forbidden(*args, **kwargs):
        raise AssertionError("source-only extraction consulted the filesystem")

    with monkeypatch.context() as local:
        for operation in ("resolve", "absolute", "is_file", "is_dir"):
            local.setattr(Path, operation, forbidden)
        requests = _source_import_scan._collect_static_source_execution_requests(
            tree, is_package=False
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

    def mutate(path, **kwargs):
        write(path, "import after\n")
        original(path, **kwargs)

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
    requests = _source_import_scan._collect_static_source_execution_requests(
        tree, is_package=source.name == "__init__.py"
    )
    assert len(requests) == 1
    from molt.cli.python_import_topology import PythonImportTopology

    value = scanner._resolve_static_source_path(
        requests[0].path, source, PythonImportTopology()
    )
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


@pytest.mark.parametrize("minor", [12, 13, 14, 15])
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
    original = _source_import_scan._collect_static_source_execution_requests

    def collect(*args, **kwargs):
        seen.append(kwargs["target_python"])
        return original(*args, **kwargs)

    monkeypatch.setattr(
        _source_import_scan, "_collect_static_source_execution_requests", collect
    )
    result = load(
        source, target_python=target, tree=ast.parse(source.read_text(encoding="utf-8"))
    )
    assert result.scan.source_executions == expected
    assert seen == [target]
    # An older frontend compiles source it can parse for a newer target.
    assert load(source, target_python=target).scan.source_executions == expected
    warm = load(source, target_python=target)
    assert warm.cache_hit and warm.scan.source_executions == expected


def test_resolver_ast_memo_distinguishes_source_generations(tmp_path):
    path = tmp_path / "entry.py"
    cache = _ModuleResolutionCache()
    first = cache.parse_module_ast(path, "a = 1", filename=str(path)).tree
    second = cache.parse_module_ast(path, "a = 2", filename=str(path)).tree
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
    key = records._import_scan_cache_key(
        source, module_name="entry", is_package=False, import_scan_mode="full"
    )
    payload = source_records("import-scan")[key]
    payload["source_executions"] = [{"module": "bad", "path": bad_path}]
    replace_source_record("import-scan", key, payload)
    assert (
        records._read_persisted_import_scan_record(
            source,
            module_name="entry",
            is_package=False,
            import_scan_mode="full",
            snapshot=PythonSourceSnapshot.capture(source),
        )
        is None
    )


@pytest.mark.parametrize("invalid", [{}, ""])
@pytest.mark.parametrize(
    "container",
    [
        "sites",
        "namespaces",
        "attributes",
        "namespace_names",
        "namespace_name_origins",
        "namespace_exposed",
        "bases",
        "base_origins",
        "attribute_origins",
        "calls",
        "functions",
        "call_origins",
        "function_returned",
        "function_returned_exposed",
        "function_yielded",
        "function_yielded_exposed",
        "completions",
        "completion_origins",
        "elements",
        "element_origins",
    ],
)
def test_malformed_source_fact_containers_force_cold_regeneration(
    tmp_path, monkeypatch, container, invalid
):
    source = write(
        tmp_path / "entry.py",
        "try:\n    import provider\nexcept ImportError:\n    pass\n"
        "def callback():\n    _MOLT_OP()\n"
        "def make():\n    return callback\n"
        "def generator():\n    yield callback\n    return callback\n"
        "async def consume():\n    return await make()\n"
        "item = make()[0]\n"
        "class Base:\n    method = callback\n"
        "class Derived(Base):\n    pass\n"
        "Derived.callback = callback\n"
        "try:\n    Derived.callback()\nexcept Exception:\n    pass\n",
    )
    cold = load(source, purpose=SourceImportPurpose.INVENTORY)
    assert cold.scan.source_sites
    assert load(source, purpose=SourceImportPurpose.INVENTORY).cache_hit
    key = records._import_scan_cache_key(
        source,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        purpose=SourceImportPurpose.INVENTORY,
    )
    payload = source_records("import-scan")[key]
    values = payload["source_values"]
    if container == "sites":
        payload["source_sites"] = invalid
    elif container in {
        "namespaces",
        "attributes",
        "elements",
        "calls",
        "functions",
        "bases",
        "completions",
    }:
        assert values[container]
        values[container] = invalid
    elif container == "namespace_names":
        row = next(row for row in values["namespaces"] if row[1])
        row[1] = invalid
    elif container == "namespace_name_origins":
        name = next(name for row in values["namespaces"] for name in row[1] if name[1])
        name[1] = invalid
    elif container == "namespace_exposed":
        row = next(row for row in values["namespaces"] if row[2])
        row[2] = invalid
    elif container == "base_origins":
        row = next(row for row in values["bases"] if row[1])
        row[1] = invalid
    elif container == "attribute_origins":
        row = next(row for row in values["attributes"] if row[2])
        row[2] = invalid
    elif container in {"call_origins", "element_origins", "completion_origins"}:
        field = {
            "call_origins": "calls",
            "element_origins": "elements",
            "completion_origins": "completions",
        }[container]
        index = 2 if container == "element_origins" else 1
        row = next(row for row in values[field] if row[index])
        row[index] = invalid
    else:
        index = {
            "function_returned": 1,
            "function_returned_exposed": 2,
            "function_yielded": 3,
            "function_yielded_exposed": 4,
        }[container]
        row = next(row for row in values["functions"] if row[index])
        row[index] = invalid
    replace_source_record("import-scan", key, payload)
    restored = records._read_persisted_import_scan_record(
        source,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        snapshot=PythonSourceSnapshot.capture(source),
        purpose=SourceImportPurpose.INVENTORY,
    )
    assert restored is None
    recovered = load(source, purpose=SourceImportPurpose.INVENTORY)
    assert not recovered.cache_hit
    assert recovered.scan.source_sites == cold.scan.source_sites
    assert recovered.scan.source_values == cold.scan.source_values
    warm = load(source, purpose=SourceImportPurpose.INVENTORY)
    assert warm.cache_hit
    assert warm.scan.source_sites == cold.scan.source_sites
    assert warm.scan.source_values == cold.scan.source_values


@pytest.mark.parametrize("dynamic", [False, True])
@pytest.mark.parametrize(
    "package_source", ["def broken(:\n", "# coding: utf-8\nVALUE = '\udcff'\n"]
)
def test_package_export_parse_failure_is_an_incomplete_obligation(
    tmp_path, dynamic, package_source
):
    statement = "__import__('pkg', fromlist=('*',))" if dynamic else "from pkg import *"
    source = write(tmp_path / "entry.py", statement + "\n")
    package = tmp_path / "pkg/__init__.py"
    package.parent.mkdir()
    package.write_bytes(package_source.encode("utf-8", errors="surrogateescape"))
    result = load(source)
    assert dict(result.scan.star_exports)["pkg"].status == "incomplete"
    assert result.scan.requires_runtime_package_anchor
    [site] = result.scan.source_sites
    assert site.unresolved


def test_package_export_read_failure_is_not_an_empty_export_set(tmp_path, monkeypatch):
    source = write(tmp_path / "entry.py", "from pkg import *\n")
    package = write(tmp_path / "pkg/__init__.py", "__all__ = ['child']\n")
    write(tmp_path / "pkg/child.py")
    original = _ModuleResolutionCache.read_module_source

    def deny_package(self, path, *args, **kwargs):
        if path == package:
            raise PermissionError("source denied")
        return original(self, path, *args, **kwargs)

    monkeypatch.setattr(_ModuleResolutionCache, "read_module_source", deny_package)
    result = load(source)
    [site] = result.scan.source_sites
    assert site.unresolved
    assert dict(result.scan.star_exports)["pkg"].reason == "PermissionError"


def test_imported_factory_result_reaches_actual_compiler_body_discovery(tmp_path):
    source = write(tmp_path / "entry.py", "from factory import make\nmake()()\n")
    write(
        tmp_path / "factory.py",
        "def make():\n    def returned():\n        import reached\n"
        "    return returned\n",
    )
    child = write(tmp_path / "reached.py", "VALUE = 7\n")
    for _ in range(2):
        graph = discovery._discover_module_graph(
            source, [tmp_path], [tmp_path], tmp_path / "stdlib", tmp_path, set()
        ).graph
        assert graph["reached"] == child


def test_source_demand_activation_join_emits_only_new_incidences(tmp_path, monkeypatch):
    from molt.compiler_analysis.python_source_demands import PythonSourceDemands
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = write(
        tmp_path / "entry.py",
        "def left(): pass\ndef right(): pass\n"
        "def make(flag):\n    if flag: return left\n    return right\n"
        "value = make(unknown)\n",
    )
    scan = load(source).scan.source_requests
    tree = ast.parse(source.read_text(encoding="utf-8"))
    value = ("entry", python_node_source_key(tree.body[-1].value))
    queued_bodies = []
    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda _module: None,
        admit_imports=lambda *_: None,
        queue_body=lambda module, owner, arguments: queued_bodies.append(
            (module, owner, arguments)
        ),
    )
    engine.accept("entry", scan)
    engine.demand(value, "read")
    engine.drain()
    make_owner = python_node_source_key(tree.body[2])
    assert any(
        module == "entry" and owner == make_owner
        for module, owner, _arguments in queued_bodies
    )
    assert not engine.value_candidates[value]
    engine.demand(value, "read", consumer="borrower")
    engine.demand(value, "factory", consumer="factory_reader")
    assert value not in engine.value_jobs
    assert len(tuple(engine._incidences.demands(value))) == 1
    body = load(source, import_scan_mode="module_init", source_owner=make_owner)
    engine.accept("entry", body.scan.source_requests)
    engine.drain()
    expected = {python_node_source_key(node) for node in tree.body[:2]}
    assert expected <= engine.read_callables["entry"]
    assert {
        candidate.source for candidate in engine.value_candidates[value]
    } == expected
    observed = []
    original = engine.activate

    def record(candidate, phase, arguments=(), *, argument_count=None, consumer=None):
        if candidate.source in expected and arguments:
            observed.append((candidate, phase, arguments, argument_count, consumer))
        original(
            candidate,
            phase,
            arguments,
            argument_count=argument_count,
            consumer=consumer,
        )

    monkeypatch.setattr(engine, "activate", record)
    for line in range(1, 65):
        engine.demand(
            value,
            "call",
            (f"argument_{line}",),
            argument_count=1,
            consumer="entry",
        )
        engine.drain()
    assert len(observed) == 2 * 64
    assert len(set(observed)) == len(observed)
    assert {item[3:] for item in observed} == {(1, "entry")}
    for _ in range(64):
        engine.queue_value(value)
        engine.drain()
    assert len(observed) == 2 * 64


@pytest.mark.parametrize("values,receipts", [(8, 5), (32, 20)])
def test_unchanged_body_receipts_do_not_revisit_existing_values(
    monkeypatch, values, receipts
):
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_source_demands import PythonSourceDemands
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda _module: None,
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
    )
    empty = _ImportScanRequests((), ())
    engine.accept("module", empty)
    # Settle initialization/cache publication before observing body receipts.
    engine.drain()
    tree = ast.parse("\n".join(f"def value_{index}(): pass" for index in range(values)))
    for function in tree.body:
        engine.demand(("module", python_node_source_key(function)), "read")
    visits = []
    process = engine.process_value

    def observe(value):
        visits.append(value)
        process(value)

    monkeypatch.setattr(engine, "process_value", observe)
    engine.drain()
    assert len(visits) == values
    for _ in range(receipts):
        engine.accept("module", empty)
        engine.drain()
    assert len(visits) == values


def test_worklist_waits_for_factory_declaration_before_demanding_its_body() -> None:
    from molt.compiler_analysis.python_binding_flow import (
        PythonBindingPolicy,
        analyze_python_source_bindings,
    )
    from molt.compiler_analysis.python_imports import (
        _ImportScanRequests,
        _SourceValueSummary,
    )
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = "def generator(): yield 1\nvalue = generator()\n"
    definition, call_statement = ast.parse(source).body
    owner = python_node_source_key(definition)
    call = python_node_source_key(call_statement.value)
    value = ("entry", call)
    initial = _ImportScanRequests(
        (),
        (),
        source_values=_SourceValueSummary(
            calls=(PythonSourceCall(call, frozenset({owner}), 0, ()),)
        ),
    )
    queued = []
    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda _: None,
        admit_imports=lambda *_: None,
        queue_body=lambda *job: queued.append(job),
    )
    engine.accept("entry", initial)
    engine.demand(value, "read")
    engine.drain()
    assert not engine.value_candidates[value]
    assert not queued

    headers = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(analyze_deferred_bodies=False)
    )
    assert not headers.source_function_results
    engine.accept(
        "entry",
        _ImportScanRequests(
            (),
            (),
            source_values=_SourceValueSummary(
                declarations=headers.source_callable_declarations
            ),
        ),
    )
    engine.drain()
    assert engine.value_candidates[value] == {
        SourceCandidate("entry", owner, "resumable")
    }
    assert not queued

    engine.demand(value, "next")
    engine.drain()
    assert queued == [("entry", owner, ())]


def test_changed_source_facts_wake_only_actual_dependencies():
    from molt.compiler_analysis.python_binding_facts import (
        PythonSourceCallableDeclaration,
    )
    from molt.compiler_analysis.python_function_kind import FunctionKind
    from molt.compiler_analysis.python_binding_facts import PythonFunctionResult
    from molt.compiler_analysis.python_imports import (
        _ImportScanRequests,
        _SourceImportBinding,
        _SourceValueSummary,
    )
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
        SourceMember,
        SourceReturn,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    tree = ast.parse(
        "def first(): pass\ndef second(): pass\nvalue = first()\nimport other\n"
    )
    first, second, call, binding = (
        python_node_source_key(tree.body[0]),
        python_node_source_key(tree.body[1]),
        python_node_source_key(tree.body[2].value),
        python_node_source_key(tree.body[3].names[0]),
    )
    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda _module: None,
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
    )
    member = ("module", SourceMember(SourceCandidate("module", None), "selected"))
    returned = ("module", SourceReturn(("module", first)))
    for value in (member, returned, ("module", call), ("module", binding)):
        engine.demand(value, "read")
    engine.drain()
    engine.accept("module", _ImportScanRequests((), ()))
    engine.drain()
    assert all(
        not engine.value_candidates[value]
        for value in (member, returned, ("module", call), ("module", binding))
    )
    update = _ImportScanRequests(
        (),
        (),
        source_values=_SourceValueSummary(
            namespaces=(
                (
                    python_node_source_key(tree),
                    (("selected", frozenset({first})),),
                    frozenset(),
                ),
            ),
            calls=(PythonSourceCall(call, frozenset({first}), 0, ()),),
            declarations=(
                PythonSourceCallableDeclaration(first, FunctionKind.SYNC),
                PythonSourceCallableDeclaration(second, FunctionKind.SYNC),
            ),
            functions=(
                PythonFunctionResult(
                    first,
                    frozenset({second}),
                    frozenset(),
                    frozenset(),
                    frozenset(),
                ),
            ),
        ),
        source_bindings=(_SourceImportBinding(binding, ("other",), None),),
    )
    engine.accept("module", update)
    engine.drain()
    assert engine.value_candidates[member] == {SourceCandidate("module", first)}
    assert (
        engine.value_candidates[returned]
        == engine.value_candidates[("module", call)]
        == {SourceCandidate("module", second)}
    )
    assert not engine.value_candidates[("module", binding)]
    engine.accept("other", _ImportScanRequests((), ()))
    engine.drain()
    assert engine.value_candidates[("module", binding)] == {
        SourceCandidate("other", None)
    }
    third = python_node_source_key(ast.parse("\n\n\n\ndef third(): pass").body[0])
    changed = update._replace(
        source_values=update.source_values._replace(
            namespaces=(
                (
                    python_node_source_key(tree),
                    (("selected", frozenset({second})),),
                    frozenset(),
                ),
            ),
            declarations=(PythonSourceCallableDeclaration(third, FunctionKind.SYNC),),
            functions=(
                PythonFunctionResult(
                    first,
                    frozenset({third}),
                    frozenset(),
                    frozenset(),
                    frozenset(),
                ),
            ),
        ),
        source_bindings=(_SourceImportBinding(binding, ("later",), None),),
    )
    engine.accept("module", changed)
    engine.drain()
    assert engine.value_candidates[member] == {
        SourceCandidate("module", first),
        SourceCandidate("module", second),
    }
    assert (
        engine.value_candidates[returned]
        == engine.value_candidates[("module", call)]
        == {SourceCandidate("module", second), SourceCandidate("module", third)}
    )
    engine.accept("later", _ImportScanRequests((), ()))
    engine.drain()
    assert engine.value_candidates[("module", binding)] == {
        SourceCandidate("other", None),
        SourceCandidate("later", None),
    }


@pytest.mark.parametrize("publication", ["named", "exposed"])
def test_late_descriptor_namespace_publication_wakes_only_descriptor_hooks(publication):
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    tree = ast.parse(
        "class Descriptor:\n    def __get__(self, instance, owner): pass\n"
        "class Host:\n    def unrelated(self): pass\n"
        "field = Descriptor()\n"
    )
    projection = _source_import_scan._collect_imports_for_graph(
        tree, "module", import_scan_mode="module_init"
    )
    host = python_node_source_key(tree.body[1])
    descriptor_hook = python_node_source_key(tree.body[0].body[0])
    field = python_node_source_key(tree.body[2].value)
    bodies = []
    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda _module: None,
        admit_imports=lambda *_: None,
        queue_body=lambda *job: bodies.append(job),
    )
    engine.accept(
        "module", _ImportScanRequests((), (), source_values=projection.source_values)
    )
    engine.activate(SourceCandidate("module", host, "instance"), "descriptor")
    engine.drain()
    assert not bodies
    update = _ImportScanRequests(
        (),
        (),
        source_values=projection.source_values._replace(
            namespaces=(
                (
                    host,
                    (("field", frozenset({field})),) if publication == "named" else (),
                    frozenset({field}) if publication == "exposed" else frozenset(),
                ),
            ),
        ),
    )
    engine.accept("module", update)
    engine.drain()
    assert bodies == [("module", descriptor_hook, ())]
    engine.accept("module", update)
    engine.drain()
    assert bodies == [("module", descriptor_hook, ())]


@pytest.mark.parametrize(
    "operation",
    [
        "pass",
        "globals()[key] = None",
        "del globals()[key]",
        "globals()[key] += 1",
        "globals().__setitem__(key, None)",
        "globals().__delitem__(key)",
        "globals().get(key)",
        "globals()[key]",
        "globals().pop(key, None)",
        "globals().setdefault(key, None)",
    ],
)
@pytest.mark.parametrize("imported", [False, True])
def test_native_globals_key_protocol_matches_actual_cpython_imports(
    tmp_path, monkeypatch, operation, imported
):
    import builtins

    leaf = "_molt_namespace_key_hash_leaf"
    key_source = f"class Key:\n    def __hash__(self):\n        import {leaf}\n        return 42\n"
    provider = "_molt_namespace_key_provider"
    source = (
        (f"from {provider} import Key\n" if imported else key_source)
        + "key = Key()\ntry:\n    "
        + operation
        + "\nexcept KeyError:\n    pass\n"
    )
    entry = write(tmp_path / "entry.py", source)
    leaf_path = write(tmp_path / f"{leaf}.py")
    paths = {"entry": entry, leaf: leaf_path}
    if imported:
        paths[provider] = write(tmp_path / f"{provider}.py", key_source)
    observed = []
    original_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name == leaf:
            observed.append(name)
        return original_import(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            sys.modules.pop(leaf, None)
            sys.modules.pop(provider, None)
    assert bool(observed) is (operation != "pass")
    imports = closed_source_imports(paths)
    assert (leaf in imports) is bool(observed)


def closed_source_imports(paths, *, protocol_modules=None, scanned=None):
    """Run the real source worklist with a premise derived from fixture sources.

    ``scanned`` collects every body job (module, owner, arguments) it ran.
    """
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )
    from molt.compiler_analysis.python_namespace_writes import python_namespace_summary
    from molt.compiler_analysis.python_parameter_classes import representative_arguments
    from molt.compiler_analysis.python_source_demands import PythonSourceDemands
    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission

    trees = {
        name: ast.parse(path.read_text(encoding="utf-8"))
        for name, path in paths.items()
    }
    admissions = {
        name: _PythonAstDigestAdmission.for_tree(tree) for name, tree in trees.items()
    }
    writes = graph_python_namespace_writes(
        (
            write
            for tree in trees.values()
            for write in python_namespace_summary(tree).foreign_writes
        ),
        paths.get,
    )
    imports = set()
    bodies = []
    if protocol_modules is None:
        protocol_modules = set()
    engine = PythonSourceDemands(
        require_runtime_import_protocol=protocol_modules.add,
        admit_imports=lambda _module, names: imports.update(names),
        queue_body=lambda *job: bodies.append(job),
    )

    def scan(module, owner=None, arguments=()):
        tree, path = trees[module], paths[module]
        admission = admissions[module]
        projection = _source_import_scan._collect_imports_for_graph(
            tree,
            module,
            source_owner=owner,
            source_arguments=arguments,
            import_scan_mode="module_init",
            namespace_premise=writes.premise(path, frozenset()),
            ast_digest_admission=admission,
        )
        imports.update(projection.imports)
        engine.accept(
            module,
            _ImportScanRequests((), ())._replace(**projection._asdict()),
        )

    for module in paths:
        scan(module)
    completed = set()
    classes_by_owner = {}
    while True:
        engine.drain()
        if not bodies:
            return imports
        module, owner, arguments = bodies.pop()
        if arguments:
            key = (module, owner)
            if key not in classes_by_owner:
                classes_by_owner[key] = (
                    _source_import_scan.source_body_argument_classes(
                        trees[module],
                        admissions[module],
                        owner,
                        module_name=module,
                        is_package=False,
                        namespace_premise=writes.premise(paths[module], frozenset()),
                        target_sys_platform=None,
                        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
                    )
                )
            if (classes := classes_by_owner[key]) is not None:
                arguments = representative_arguments(arguments, classes)
        job = (module, owner, arguments)
        if job in completed:
            continue
        completed.add(job)
        if scanned is not None:
            scanned.append(job)
        scan(*job)


@pytest.mark.parametrize(
    "route", ["generator", "iterator", "async-generator", "async-iterator"]
)
@pytest.mark.parametrize("consume", [False, True])
def test_iteration_discovers_returned_carrier_and_only_consumed_callback(
    tmp_path, monkeypatch, route, consume
):
    import builtins

    provider = "_molt_iteration_provider"
    reached = "_molt_iteration_reached"
    unused = "_molt_iteration_unused"
    # A local callback isolates returned-value laziness from conservative
    # availability of module globals under supported callback writes.
    callback = f"        def unused():\n            import {unused}\n"
    if route == "generator":
        implementation = f"class Items:\n    def __iter__(self):\n{callback}        import {reached}\n        yield unused\n"
    elif route == "async-generator":
        implementation = f"class Items:\n    async def __aiter__(self):\n{callback}        import {reached}\n        yield unused\n"
    else:
        asynchronous = route == "async-iterator"
        acquire, advance, stopped = (
            ("__aiter__", "__anext__", "StopAsyncIteration")
            if asynchronous
            else ("__iter__", "__next__", "StopIteration")
        )
        implementation = (
            f"class Iterator:\n    spent = False\n    def {acquire}(self): return self\n"
            f"    {'async ' if asynchronous else ''}def {advance}(self):\n{callback}        import {reached}\n"
            f"        if self.spent: raise {stopped}\n        self.spent = True\n        return unused\n"
            f"class Items:\n    def {acquire}(self): return Iterator()\n"
        )
    provider_path = write(tmp_path / f"{provider}.py", implementation)
    reached_path = write(tmp_path / f"{reached}.py")
    write(tmp_path / f"{unused}.py")
    source = f"from {provider} import Items\n"
    action = "item()" if consume else "pass"
    if route.startswith("async"):
        source += f"import asyncio\nasync def consume():\n    async for item in Items(): {action}\nasyncio.run(consume())\n"
    else:
        source += f"for item in Items(): {action}\n"
    entry = write(tmp_path / "entry.py", source)
    attempted = []
    import_module = builtins.__import__

    def observe(name, *args, **kwargs):
        if name in {reached, unused}:
            attempted.append(name)
        return import_module(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            for name in (provider, reached, unused):
                sys.modules.pop(name, None)
    assert set(attempted) == ({reached, unused} if consume else {reached})
    precise = closed_source_imports(
        {
            "entry": entry,
            provider: provider_path,
            reached: reached_path,
            unused: tmp_path / f"{unused}.py",
        }
    )
    assert reached in precise
    assert (unused in precise) is consume
    for _ in range(2):
        result = discovery._discover_module_graph(
            entry, [tmp_path], [tmp_path], tmp_path / "stdlib", tmp_path, set()
        )
        assert result.graph[reached] == reached_path
        assert result.scan_authority.by_module[provider].mode == "module_init"
        assert result.graph[provider] == provider_path


@pytest.mark.parametrize("asynchronous", [False, True])
@pytest.mark.parametrize("consume", [False, True])
def test_generator_expression_creation_acquires_without_advancing(
    tmp_path, monkeypatch, asynchronous, consume
):
    import builtins

    acquired, advanced = "_molt_iter_acquired", "_molt_iter_advanced"
    acquire, advance, stop = (
        ("__aiter__", "__anext__", "StopAsyncIteration")
        if asynchronous
        else ("__iter__", "__next__", "StopIteration")
    )
    provider = write(
        tmp_path / "carrier.py",
        f"class Iterator:\n    def {acquire}(self): return self\n"
        f"    {'async ' if asynchronous else ''}def {advance}(self):\n"
        f"        import {advanced}\n        raise {stop}\n"
        f"class Items:\n    def {acquire}(self):\n"
        f"        import {acquired}\n        return Iterator()\n",
    )
    loop = "async for" if asynchronous else "for"
    body = f"values = (item {loop} item in Items())\n"
    if consume:
        body += f"{loop} item in values: pass\n"
    source = "from carrier import Items\n"
    if asynchronous:
        source += "import asyncio\nasync def build():\n"
        source += "".join(f"    {line}\n" for line in body.splitlines())
        source += "asyncio.run(build())\n"
    else:
        source += body
    entry = write(tmp_path / "entry.py", source)
    paths = {"entry": entry, "carrier": provider}
    paths.update(
        (name, write(tmp_path / f"{name}.py")) for name in (acquired, advanced)
    )
    attempts = set()
    real_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name in {acquired, advanced}:
            attempts.add(name)
        return real_import(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            for name in paths:
                if name != "entry":
                    sys.modules.pop(name, None)
    assert attempts == ({acquired, advanced} if consume else {acquired})
    discovered = closed_source_imports(paths)
    assert acquired in discovered
    assert (advanced in discovered) is consume


@pytest.mark.parametrize("base", ["Parent", "make_base()", "*(Parent,)"])
def test_actual_class_bases_supply_inherited_protocols(tmp_path, monkeypatch, base):
    import builtins

    leaf = "_molt_inherited_iteration"
    provider = write(
        tmp_path / "carrier.py",
        f"class Parent:\n    def __iter__(self):\n        import {leaf}\n        return iter(())\n"
        "def make_base(): return Parent\n"
        f"class Items({base}): pass\n",
    )
    source = "from carrier import Items\nfor item in Items(): pass\n"
    entry = write(tmp_path / "entry.py", source)
    leaf_path = write(tmp_path / f"{leaf}.py")
    attempts = []
    actual_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name == leaf:
            attempts.append(name)
        return actual_import(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            for name in ("carrier", leaf):
                sys.modules.pop(name, None)
    assert attempts == [leaf]
    assert leaf in closed_source_imports(
        {"entry": entry, "carrier": provider, leaf: leaf_path}
    )


def test_inherited_member_search_visits_shared_ancestors_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
        SourceValue,
        _SourceNamespace,
    )

    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    root = SourceCandidate("provider", (1, 0, 1, 1, "ClassDef"))
    implementation = SourceCandidate("provider", (2, 0, 2, 1, "FunctionDef"))
    engine.namespaces[root.value] = _SourceNamespace(
        {"method": frozenset({implementation.source})}
    )
    engine.value_candidates[implementation.value] = {implementation}
    engine.value_candidates[root.value] = {root}
    leaf = root
    for line in range(3, 27, 3):
        left, right, child = (
            SourceCandidate("provider", (index, 0, index, 1, "ClassDef"))
            for index in range(line, line + 3)
        )
        for node, parents in (
            (left, (leaf,)),
            (right, (leaf,)),
            (child, (left, right)),
        ):
            engine.bases[node.value] = frozenset(parent.source for parent in parents)
            engine.value_candidates[node.value] = {node}
        leaf = child

    watch = engine.watch
    reads: list[SourceValue] = []

    def record(origin: SourceValue, user: SourceValue) -> set[SourceCandidate]:
        reads.append(origin)
        return watch(origin, user)

    monkeypatch.setattr(engine, "watch", record)
    assert engine.select_member(leaf, "method", leaf.value) == {implementation}
    assert reads.count(implementation.value) == 1
    assert set(reads) == engine.value_candidates.keys()


def test_inherited_source_projection_wakes_only_for_new_owners(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from unittest.mock import Mock

    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
        _SourceNamespace,
    )

    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    module = "provider"
    engine.modules.add(module)
    base = (1, 0, 1, 1, "arg")
    receiver = SourceCandidate(module, (2, 0, 2, 1, "ClassDef"), "instance")
    parent = SourceCandidate(module, (3, 0, 3, 1, "ClassDef"))
    method = SourceCandidate(module, (4, 0, 4, 1, "FunctionDef"))
    user = ("consumer", None)
    engine.bases[receiver.value] = frozenset({base})
    engine.namespaces[parent.value] = _SourceNamespace(
        {"method": frozenset({method.source})}
    )
    engine.value_candidates[method.value] = {method}
    engine.value_candidates[parent.value] = {parent}

    assert not engine._select_class_members(receiver, "method", user, strict=False)
    engine._queue_arguments((module, base), {parent})
    engine.process_value((module, base))
    assert engine._select_class_members(receiver, "method", user, strict=False) == {
        method
    }

    # Binding and containment widen the operand without adding a base identity.
    select = Mock(wraps=engine.select_member)
    monkeypatch.setattr(engine, "select_member", select)
    engine._queue_arguments(
        (module, base), {parent._replace(contained=True, receiver=receiver.value)}
    )
    engine.process_value((module, base))
    assert engine._select_class_members(receiver, "method", user, strict=False) == {
        method
    }
    select.assert_not_called()


@pytest.mark.parametrize("cyclic_bases", [False, True])
def test_inherited_providers_share_search_and_rebind_after_shadowing(
    monkeypatch: pytest.MonkeyPatch, cyclic_bases: bool
) -> None:
    from unittest.mock import Mock

    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
        _SourceNamespace,
    )

    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    module = "provider"
    parent, intermediate, left, right = (
        SourceCandidate(module, (line, 0, line, 1, "ClassDef")) for line in range(1, 5)
    )
    original = SourceCandidate(module, (5, 0, 5, 1, "FunctionDef"), "class_descriptor")
    shadow = SourceCandidate(module, (6, 0, 6, 1, "FunctionDef"), "class_descriptor")
    for candidate in (parent, intermediate, left, right, original, shadow):
        engine.value_candidates[candidate.value] = {candidate}
    for child, base in (
        (intermediate, parent),
        (left, intermediate),
        (right, intermediate),
    ):
        engine.bases[child.value] = frozenset({base.source})
    if cyclic_bases:
        # A may-provenance edge can refer back to its consumer's class.
        engine.bases[intermediate.value] |= {left.source}
    engine.namespaces[parent.value] = _SourceNamespace(
        {"factory": frozenset({original.source})}
    )

    follow = Mock(wraps=engine._watch_inherited_sources)
    monkeypatch.setattr(engine, "_watch_inherited_sources", follow)
    for receiver in (left, right):
        assert engine._select_class_members(
            receiver, "factory", ("consumer", None), strict=False
        ) == {original._replace(kind="bound_method", receiver=receiver.value)}
    assert sum(call.args[0] == parent.value for call in follow.call_args_list) == 1

    engine._publish(
        engine.namespaces,
        intermediate.value,
        _SourceNamespace({"factory": frozenset({shadow.source})}),
    )
    for receiver in (left, right):
        assert engine._select_class_members(
            receiver, "factory", ("consumer", None), strict=False
        ) == {shadow._replace(kind="bound_method", receiver=receiver.value)}


def test_class_member_query_shares_selection_and_replays_property_origins(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
        SourceReturn,
        _SourceNamespace,
    )

    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    owner = SourceCandidate("provider", (1, 0, 1, 1, "ClassDef"), "instance")
    member_source = (2, 0, 2, 1, "FunctionDef")
    getter_source = (3, 0, 3, 1, "FunctionDef")
    member = SourceCandidate("provider", member_source)
    getter = SourceCandidate("provider", getter_source, "property_descriptor")
    engine.value_candidates[member.value] = {member}
    engine.value_candidates[getter.value] = {getter}
    engine.value_candidates[("provider", SourceReturn(getter.value))] = {member}
    first, second, third = ("first", None), ("second", None), ("third", None)

    from unittest.mock import Mock

    select = Mock(wraps=engine.select_member)
    monkeypatch.setattr(engine, "select_member", select)
    assert not engine._select_class_members(owner, "selected", first, strict=False)
    assert not engine._select_class_members(owner, "selected", second, strict=False)
    assert select.call_count == 1

    engine._publish(
        engine.namespaces,
        owner.value,
        _SourceNamespace({"selected": frozenset({member_source})}),
    )
    assert engine._select_class_members(owner, "selected", third, strict=False) == {
        member
    }
    assert select.call_count == 2
    assert first in engine.value_jobs and third not in engine.value_jobs
    assert third not in engine._property_results

    # The value is unchanged, but its getter provenance changes binding.
    engine._publish(
        engine.namespaces,
        owner.value,
        _SourceNamespace({"selected": frozenset({getter_source})}),
    )
    assert engine._select_class_members(owner, "selected", second, strict=False) == {
        member
    }
    assert select.call_count == 3
    assert third in engine.value_jobs
    assert engine._property_results[second] == {member}


def test_call_forwards_results_until_its_transfer_changes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from unittest.mock import Mock

    from molt.compiler_analysis.python_binding_facts import (
        PythonFunctionResult,
        PythonSourceCall,
        PythonSourceCallableDeclaration,
    )
    from molt.compiler_analysis.python_function_kind import FunctionKind
    from molt.compiler_analysis.python_imports import (
        _ImportScanRequests,
        _SourceImportBinding,
        _SourceValueSummary,
    )
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
    )

    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    module = "provider"
    function = (1, 0, 1, 1, "FunctionDef")
    formal = (1, 2, 1, 3, "arg")
    site = (2, 0, 2, 1, "Call")
    call = (module, site)
    other_site = (2, 2, 2, 3, "Call")
    other_call = (module, other_site)
    first, second, third = (
        SourceCandidate(module, (line, 0, line, 1, "ClassDef")) for line in (3, 4, 5)
    )
    engine.accept(
        module,
        _ImportScanRequests(
            (),
            (),
            source_values=_SourceValueSummary(
                calls=tuple(
                    PythonSourceCall(owner, frozenset({function}), 0, ())
                    for owner in (site, other_site)
                ),
                declarations=(
                    PythonSourceCallableDeclaration(
                        function, FunctionKind.SYNC, (formal,)
                    ),
                ),
                functions=(
                    PythonFunctionResult(
                        function,
                        frozenset({formal}),
                        frozenset(),
                        frozenset(),
                        frozenset(),
                    ),
                ),
            ),
        ),
    )
    engine._queue_arguments((module, formal), {first})
    engine.queue_value(call)
    engine.queue_value(other_call)
    engine.drain()
    assert engine.value_candidates[call] == {first}

    transfer = Mock(wraps=engine._returned_values)
    monkeypatch.setattr(engine, "_returned_values", transfer)
    engine._queue_arguments((module, formal), {second})
    engine.drain()
    assert engine.value_candidates[call] == {first, second}
    assert engine.value_candidates[other_call] == {first, second}
    transfer.assert_not_called()

    engine.accept("alternate", _ImportScanRequests((), ()))
    engine.accept(
        module,
        _ImportScanRequests(
            ("alternate",),
            (),
            source_bindings=(_SourceImportBinding(site, ("alternate",), None),),
        ),
    )
    engine.drain()
    engine._queue_arguments((module, formal), {third})
    engine.drain()
    assert engine.value_candidates[call] == {
        first,
        second,
        SourceCandidate("alternate", None),
    }
    assert engine.value_candidates[other_call] == {first, second, third}


def test_returned_cache_values_propagate_only_new_candidates(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from unittest.mock import Mock

    from molt.compiler_analysis.python_binding_facts import PythonFunctionResult
    from molt.compiler_analysis.python_imports import (
        _ImportScanRequests,
        _SourceValueSummary,
    )
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCacheItem,
        SourceCallableOutcome,
        SourceCandidate,
    )

    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    module = "provider"
    argument = (1, 0, 1, 1, "arg")
    function = (2, 0, 2, 1, "FunctionDef")
    read = (3, 0, 3, 1, "Call")
    first = SourceCandidate(module, (4, 0, 4, 1, "ClassDef"))
    second = SourceCandidate(module, (5, 0, 5, 1, "ClassDef"))
    new_route = (6, 0, 6, 1, "TypeAlias")
    key = SourceCacheItem(None, True)
    outcome = (module, SourceCallableOutcome(function, yielded=False))
    engine.accept(
        module,
        _ImportScanRequests(
            (),
            (),
            source_values=_SourceValueSummary(
                functions=(
                    PythonFunctionResult(
                        function,
                        frozenset({argument}),
                        frozenset(),
                        frozenset(),
                        frozenset(),
                    ),
                ),
                cache_reads=((read, ("selected",), frozenset(), frozenset(), False),),
            ),
        ),
    )
    engine.hold_cache_origins(key, {(outcome, False)})
    engine._queue_arguments((module, argument), {first})
    engine.queue_value((module, read))
    engine.drain()
    assert engine.value_candidates[(module, read)] == {first}

    read_operands = Mock(wraps=engine._operand_candidates)
    monkeypatch.setattr(engine, "_operand_candidates", read_operands)
    engine._queue_arguments((module, argument), {first, second})
    engine.drain()
    assert engine.value_candidates[(module, read)] == {first, second}
    assert {
        row.args[1] for row in read_operands.call_args_list if row.args[2] is not None
    } == {outcome, ("", key), (module, read)}

    engine.hold_cache_origins(key, {((module, new_route), True)})
    engine.drain()
    assert engine.value_candidates[(module, read)] == {
        first,
        second,
        SourceCandidate(module, new_route, contained=True),
    }


def test_attribute_receiver_delta_and_structural_wake(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from unittest.mock import Mock

    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
        _SourceNamespace,
    )

    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    module = "provider"
    argument = (1, 0, 1, 1, "arg")
    operand = (module, argument)
    user = (module, (2, 0, 2, 1, "Attribute"))
    engine.attributes[user] = {"member": frozenset({argument})}
    first = SourceCandidate(module, (3, 0, 3, 1, "ClassDef"))
    second = SourceCandidate(module, (4, 0, 4, 1, "ClassDef"))
    first_origin = (5, 0, 5, 1, "FunctionDef")
    second_origin = (6, 0, 6, 1, "FunctionDef")
    first_member = SourceCandidate(module, first_origin)
    second_member = SourceCandidate(module, second_origin)
    for owner, origin, member in (
        (first, first_origin, first_member),
        (second, second_origin, second_member),
    ):
        engine.value_candidates[owner.value] = {owner}
        engine.value_candidates[member.value] = {member}
        engine.namespaces[owner.value] = _SourceNamespace(
            {"member": frozenset({origin})}
        )

    select = Mock(wraps=engine._select_class_members)
    monkeypatch.setattr(engine, "_select_class_members", select)
    engine._queue_arguments(operand, {first})
    engine.queue_value(user)
    engine.drain()
    assert not engine.value_candidates.get(operand)
    assert not select.call_args_list

    engine.accept(module, _ImportScanRequests((), ()))
    engine.drain()
    assert engine.value_candidates[user] == {first_member}
    assert [row.args[0] for row in select.call_args_list] == [first]

    select.reset_mock()
    engine._queue_arguments(operand, {first, second})
    engine._queue_arguments(operand, {second})
    engine.drain()
    assert engine.value_candidates[user] == {first_member, second_member}
    assert [row.args[0] for row in select.call_args_list] == [second]
    assert operand not in engine.pending_arguments

    select.reset_mock()
    engine._invalidate_value(operand)
    engine.drain()
    assert {row.args[0] for row in select.call_args_list} == {first, second}


@pytest.mark.parametrize("escape", [False, True])
def test_unrelated_member_class_is_exposure_not_an_inherited_base(
    tmp_path, monkeypatch, escape
):
    import builtins

    leaf = "_molt_unrelated_member"
    provider = write(
        tmp_path / "carrier.py",
        "class Items:\n    class Unrelated:\n"
        f"        def __init__(self):\n            import {leaf}\n"
        f"        def __getitem__(self, index):\n            import {leaf}\n            raise IndexError\n"
        "    def __iter__(self): return iter(())\n",
    )
    foreign = write(
        tmp_path / "foreign.py",
        "def consume(cls):\n    for item in cls.Unrelated(): pass\n",
    )
    source = "from carrier import Items\nfor item in Items(): pass\n"
    if escape:
        source += "from foreign import consume\nconsume(Items)\n"
    entry = write(tmp_path / "entry.py", source)
    leaf_path = write(tmp_path / f"{leaf}.py")
    attempts = []
    actual_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name == leaf:
            attempts.append(name)
        return actual_import(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            for name in ("carrier", "foreign", leaf):
                sys.modules.pop(name, None)
    assert bool(attempts) is escape
    if escape:
        assert leaf in closed_source_imports(
            {"entry": entry, "carrier": provider, "foreign": foreign, leaf: leaf_path}
        )
    else:
        from molt.compiler_analysis.python_imports import _ImportScanRequests
        from molt.compiler_analysis.python_source_demands import (
            PythonSourceDemands,
            SourceCandidate,
            SourceMember,
        )
        from molt.compiler_analysis.python_source_keys import python_node_source_key

        tree = ast.parse(provider.read_text(encoding="utf-8"))
        projection = _source_import_scan._collect_imports_for_graph(
            tree, "carrier", import_scan_mode="module_init"
        )
        bodies = []
        engine = PythonSourceDemands(
            require_runtime_import_protocol=lambda _module: None,
            admit_imports=lambda *_: None,
            queue_body=lambda *job: bodies.append(job),
        )
        # Isolate member selection from conservative descriptor callbacks at
        # class creation: those are separate source availability demands.
        engine.accept(
            "carrier",
            _ImportScanRequests((), (), source_values=projection.source_values),
        )
        owner = SourceCandidate(
            "carrier", python_node_source_key(tree.body[0]), "instance"
        )
        members = {
            name: ("carrier", SourceMember(owner, name))
            for name in ("__init__", "__getitem__", "__iter__")
        }
        for value in members.values():
            engine.demand(value, "read")
        engine.drain()
        assert not engine.value_candidates[members["__init__"]]
        assert not engine.value_candidates[members["__getitem__"]]
        assert engine.value_candidates[members["__iter__"]]
        assert not bodies


@pytest.mark.parametrize("factory", [False, True])
def test_opaque_callable_escape_discovers_returned_or_yielded_callback(
    tmp_path, monkeypatch, factory
):
    import builtins

    leaf = "_molt_escaped_result"
    provider = write(
        tmp_path / "carrier.py",
        f"def produce():\n    def callback():\n        import {leaf}\n"
        f"    {'yield' if factory else 'return'} callback\n",
    )
    foreign = write(
        tmp_path / "foreign.py",
        "def consume(fn):\n"
        + ("    for callback in fn(): callback()\n" if factory else "    fn()()\n"),
    )
    source = (
        "from carrier import produce\nfrom foreign import consume\nconsume(produce)\n"
    )
    entry = write(tmp_path / "entry.py", source)
    leaf_path = write(tmp_path / f"{leaf}.py")
    attempts = []
    actual_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name == leaf:
            attempts.append(name)
        return actual_import(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            for name in ("carrier", "foreign", leaf):
                sys.modules.pop(name, None)
    assert attempts == [leaf]
    assert leaf in closed_source_imports(
        {"entry": entry, "carrier": provider, "foreign": foreign, leaf: leaf_path}
    )


@pytest.mark.parametrize(
    "defect",
    [
        "old-schema",
        "retired-effects",
        "retired-site-handlers",
        "retired-site-owner",
        "retired-demand-owner",
        "unknown-phase",
        "missing-purpose",
        "unknown-purpose",
        "wrong-purpose",
    ],
)
def test_source_cache_rejects_retired_or_incoherent_protocol(
    tmp_path, monkeypatch, defect
):
    source = write(
        tmp_path / "entry.py",
        "def callback():\n    import provider\n"
        "try:\n    callback()\n    import provider\nexcept ImportError:\n    pass\n",
    )
    cold = load(source)
    assert load(source).cache_hit
    key = records._import_scan_cache_key(
        source, module_name="entry", is_package=False, import_scan_mode="full"
    )
    payload = source_records("import-scan")[key]
    if defect == "old-schema":
        payload["version"] -= 1
    elif defect == "retired-effects":
        payload["source_effects"] = []
    elif defect == "retired-site-handlers":
        payload["source_sites"][0]["handlers"] = []
    elif defect == "retired-site-owner":
        payload["source_sites"][0]["owner"] = None
    elif defect == "retired-demand-owner":
        payload["source_demands"][0].append(None)
    elif defect == "unknown-phase":
        payload["source_demands"][0][1] = "invented"
    elif defect == "missing-purpose":
        del payload["purpose"]
    elif defect == "unknown-purpose":
        payload["purpose"] = "invented"
    elif defect == "wrong-purpose":
        payload["purpose"] = "inventory"
    replace_source_record("import-scan", key, payload)
    decoded = records._read_persisted_import_scan_record(
        source,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        snapshot=PythonSourceSnapshot.capture(source),
    )
    assert decoded is None
    regenerated = load(source)
    assert not regenerated.cache_hit
    assert regenerated.scan.source_requests == cold.scan.source_requests


def test_exception_paths_retain_import_requests_across_source_cache(tmp_path):
    source = write(
        tmp_path / "entry.py",
        "try:\n    import provider\nexcept Exception:\n    import recovery\n    raise\n"
        "finally:\n    import cleanup\n",
    )
    cold = load(source)
    warm = load(source)
    assert warm.cache_hit and warm.scan.source_requests == cold.scan.source_requests
    assert {site.request.name for site in warm.scan.source_sites} == {
        "provider",
        "recovery",
        "cleanup",
    }
    assert not hasattr(warm.scan, "source_effects")


@pytest.mark.parametrize("member", [False, True])
def test_operation_local_source_memo_binds_namespace_premise(tmp_path, member):
    from molt.compiler_analysis.python_identity_catalog_generated import PythonMember
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    binding = "import typing" if member else "from typing import TYPE_CHECKING"
    condition = "typing.TYPE_CHECKING" if member else "TYPE_CHECKING"
    source = write(
        tmp_path / "entry.py",
        f"{binding}\ncallback()\nif {condition}:\n    __import__('leaf')\n",
    )
    tree = ast.parse(source.read_text(encoding="utf-8"))
    resolver = _ModuleResolutionCache()
    stable = PythonNamespacePremise((), 0, 0, ())
    rewritten = PythonNamespacePremise(
        () if member else ("TYPE_CHECKING",),
        int(PythonMember.TYPING_TYPE_CHECKING) if member else 0,
        int(PythonMember.TYPING_TYPE_CHECKING) if member else 0,
        (),
    )
    source_calls = []

    def collect(*args, **kwargs):
        source_calls.append(kwargs["namespace_premise"])
        return _source_import_scan._collect_imports_for_graph(*args, **kwargs)

    def graph(premise):
        return resolver.collect_graph_imports(
            source,
            tree,
            collector=collect,
            module_name="entry",
            namespace_premise=premise,
        )

    for premise, expected in ((None, True), (stable, False), (rewritten, True)):
        for _ in range(2):
            assert ("leaf" in graph(premise).imports) is expected
    # An equal immutable premise reuses the earlier precise result after a
    # different graph premise has been consulted; the memo may not cross them.
    assert "leaf" not in graph(PythonNamespacePremise((), 0, 0, ())).imports
    assert source_calls == [None, stable, rewritten]


def test_runtime_import_protocol_follows_the_namespace_premise() -> None:
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    # No import statement: only the guarded call can need the protocol, and the
    # premise decides whether the callback can rebind its guard.
    tree = ast.parse("flag = 0\ncallback()\nif flag:\n    __import__('leaf')\n")

    def uses(premise):
        return _source_import_scan._collect_imports_for_graph(
            tree, "entry", False, namespace_premise=premise
        ).uses_runtime_import_protocol

    assert uses(None)
    assert not uses(PythonNamespacePremise((), 0, 0, ()))
    assert uses(PythonNamespacePremise(("flag",), 0, 0, ()))


@pytest.mark.parametrize(
    "intrinsic,arguments",
    [
        ("molt_module_import", "'leaf'"),
        ("molt_importlib_import_module", "'leaf', None"),
        ("molt_importlib_import_optional", "'leaf'"),
        ("molt_importlib_import_or_fallback", "'leaf', None"),
        ("molt_importlib_import_required", "'leaf'"),
        ("molt_importlib_import_transaction", "'leaf', None, None, (), 0"),
        ("molt_importlib_find_spec", "'leaf', None, None"),
    ],
)
def test_acquired_import_intrinsics_demand_protocol_by_callable_identity(
    intrinsic, arguments
):
    prefix = (
        "from _intrinsics import require_intrinsic as acquire\n"
        f"load = acquire({intrinsic!r})\n"
    )
    for suffix, expected in (
        (f"alias = load\nalias({arguments})\n", True),
        ("", False),
        (f"if False:\n    load({arguments})\n", False),
        (f"load = lambda *args: None\nload({arguments})\n", False),
    ):
        result = _source_import_scan._collect_imports_for_graph(
            ast.parse(prefix + suffix), "entry", False
        )
        assert result.uses_runtime_import_protocol is expected


@pytest.mark.parametrize(
    "source",
    [
        "from _intrinsics import load_intrinsic as acquire\n"
        "load = acquire('molt_module_import')\nload('leaf')\n",
        "from _intrinsics import _require_intrinsic as acquire\n"
        "load = acquire('molt_module_import')\nload('leaf')\n",
        "from _intrinsics import _load_intrinsic as acquire\n"
        "load = acquire('molt_module_import')\nload('leaf')\n",
        "from package._intrinsics import require_intrinsic as acquire\n"
        "load = acquire('molt_module_import')\nload('leaf')\n",
        "from _intrinsics import require_intrinsic as acquire\n"
        "load = acquire(input())\nload('leaf')\n",
        "from _intrinsics import require_intrinsic as acquire\n"
        "load = acquire(*arguments)\nload('leaf')\n",
        "from _intrinsics import require_intrinsic as acquire\n"
        "load = acquire(**keywords)\nload('leaf')\n",
        "from _intrinsics import require_intrinsic as acquire\n"
        "resolve = acquire('molt_require_intrinsic_runtime')\n"
        "resolve('molt_module_import')('leaf')\n",
        "from _intrinsics import require_intrinsic as acquire\n"
        "resolve = acquire('molt_load_intrinsic_runtime')\n"
        "resolve('molt_module_import')('leaf')\n",
    ],
)
def test_intrinsic_acquisition_family_preserves_possible_import_demand(source):
    assert _source_import_scan._collect_imports_for_graph(
        ast.parse(source), "entry", False
    ).uses_runtime_import_protocol


@pytest.mark.parametrize(
    "source,mode,expected",
    [
        ("values = [i for i in range(2)]\n", "full", False),
        ("import leaf\n", "full", False),
        ("import leaf\n__import__('leaf')\n", "full", True),
        ("raise RuntimeError()\nimport leaf\n", "full", False),
        ("from __future__ import annotations\n", "full", False),
        ("import _intrinsics\n", "full", False),
        ("from pkg._intrinsics import item\n", "full", False),
        ("from ._intrinsics import item\n", "full", False),
        ("import pkg._intrinsics\n", "full", False),
        ("loader = __import__ if flag else None\nloader('leaf')\n", "full", True),
        ("def __import__(*args):\n    pass\n__import__('leaf')\n", "full", False),
        ("def invoke():\n    import leaf\n", "module_init", False),
        ("def invoke():\n    __import__('leaf')\n", "module_init", False),
        ("def invoke():\n    __import__('leaf')\n", "full", False),
    ],
)
def test_protocol_and_import_demands_share_execution_regions(
    monkeypatch, source, mode, expected
):
    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission

    tree = ast.parse(source)
    admission = _PythonAstDigestAdmission(tree)
    original = _source_import_scan._collect_imports_for_graph(
        tree,
        "pkg.entry",
        import_scan_mode=mode,
        ast_digest_admission=admission,
    )
    assert original.uses_runtime_import_protocol is expected
    # Keep immutable binding/export facts warm. Count real execution traversal,
    # not a helper name, so moving or renaming a second walk cannot pass.
    walks = 0
    regions = _source_import_scan.python_execution_regions

    def observe(*args, **kwargs):
        nonlocal walks
        walks += 1
        return regions(*args, **kwargs)

    monkeypatch.setattr(_source_import_scan, "python_execution_regions", observe)
    assert (
        _source_import_scan._collect_imports_for_graph(
            tree,
            "pkg.entry",
            import_scan_mode=mode,
            ast_digest_admission=admission,
        )
        == original
    )
    assert walks == 1


def test_persisted_source_requests_bind_complete_namespace_premise(
    tmp_path, monkeypatch
):
    from molt.compiler_analysis.python_identity_catalog_generated import (
        ALL_INVALID_MEMBERS,
    )
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.compiler_analysis.python_namespace_writes import python_namespace_summary

    path = write(tmp_path / "entry.py", "pass\n")
    snapshot = PythonSourceSnapshot.capture(path)
    premises = (
        None,
        PythonNamespacePremise((), 0, 0, ()),
        PythonNamespacePremise(("callback",), 0, 0, ()),
        PythonNamespacePremise(None, 0, 0, ()),
        PythonNamespacePremise((), ALL_INVALID_MEMBERS, ALL_INVALID_MEMBERS, ()),
    )
    options = dict(module_name="entry", is_package=False, import_scan_mode="full")
    requests = []
    payloads = []
    paths = []
    for index, premise in enumerate(premises):
        request = _ImportScanRequests(
            (f"generation_{index}",),
            (),
            namespace_summary=python_namespace_summary(ast.parse(snapshot.content)),
        )
        records._write_persisted_import_scan(
            path,
            **options,
            namespace_premise=premise,
            scan=request,
            snapshot=snapshot,
        )
        requests.append(request)
        cache_key = records._import_scan_cache_key(
            path, **options, namespace_premise=premise
        )
        paths.append(cache_key)
        payloads.append(source_records("import-scan")[cache_key])
    assert len(set(paths)) == len(premises)
    for premise, request in zip(premises, requests):
        assert (
            records._read_persisted_import_scan_record(
                path, **options, namespace_premise=premise, snapshot=snapshot
            )
            == request
        )
    # The key alone names the premise view (missing and unknown included):
    # a row stores no copy of it (schema 51).
    assert all("namespace_premise" not in payload for payload in payloads)


@pytest.mark.parametrize("import_count", [1, 512])
def test_persisted_import_blobs_reuse_decode_only_under_matching_admission(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, import_count: int
) -> None:
    from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction

    imports = tuple(f"leaf_{number:04d}" for number in range(import_count))
    path = write(tmp_path / "entry.py", "\n".join(f"import {name}" for name in imports))
    snapshot = PythonSourceSnapshot.capture(path)
    request = load(
        path, project_root=None, source_snapshot=snapshot
    ).scan.source_requests
    assert request.imports == imports

    key = records._import_scan_cache_key(
        path,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        snapshot=snapshot,
    )
    alias = records._import_scan_cache_key(
        path,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        snapshot=snapshot,
        namespace_premise=WHOLE_PROGRAM,
    )
    foreign = records._import_scan_cache_key(
        path,
        module_name="other",
        is_package=False,
        import_scan_mode="full",
        snapshot=snapshot,
    )
    assert len({key, alias, foreign}) == 3
    records._write_persisted_import_scan(
        path,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        snapshot=snapshot,
        scan=request,
    )
    store = records.frontend_record_files()
    assert store.link("import-scan", alias, key)
    assert store.link("import-scan", foreign, key)
    stored = store.get(
        "import-scan", key, max_bytes=records.SOURCE_ANALYSIS_CACHE_MAX_BYTES
    )
    assert stored is not None
    assert (len(stored.payload) > 32 * 1024) == (import_count > 1)

    decode_count = 0
    original_decode = records._decode_import_scan_payload

    def count_decode(
        raw: bytes, admission: records._ImportScanAdmission
    ) -> records._ImportScanRequests | None:
        nonlocal decode_count
        decode_count += 1
        return original_decode(raw, admission)

    monkeypatch.setattr(records, "_decode_import_scan_payload", count_decode)

    def read(
        name: str, premise: PythonNamespacePremise | None = None
    ) -> records._ImportScanRequests | None:
        return records._read_persisted_import_scan_record(
            path,
            module_name=name,
            namespace_premise=premise,
            is_package=False,
            import_scan_mode="full",
            snapshot=snapshot,
        )

    with _source_tree_fingerprint_transaction():
        assert read("entry") == request
        assert read("entry", WHOLE_PROGRAM) == request
        assert decode_count == 1
        assert read("other") is None
        assert decode_count == 2
    assert read("entry") == request
    assert decode_count == 3

    # Only one admission fits. Reusing the same bytes under a different
    # admission displaces it, even though the record itself is the same.
    monkeypatch.setattr(
        records,
        "_IMPORT_SCAN_DECODE_BYTES",
        len(stored.payload) + records._IMPORT_SCAN_DECODE_ENTRY_BYTES,
    )
    with _source_tree_fingerprint_transaction():
        assert read("entry") == request
        assert read("other") is None
        assert read("entry", WHOLE_PROGRAM) == request
        assert decode_count == 6


@pytest.mark.parametrize(
    "retired",
    [
        None,
        {},
        {"foreign_writes": [], "member_writes": False},
        {"foreign_writes": [], "member_writes": -1},
        {"foreign_writes": [], "member_writes": 1 << 60},
        {"foreign_writes": {}, "member_writes": 0},
        {"foreign_writes": ["name", "name"], "member_writes": 0},
    ],
)
def test_persisted_scan_with_a_retired_premise_copy_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, retired: object
) -> None:
    """Source records are a closed protocol: a row that still carries the
    retired premise copy, valid or malformed, is never admitted."""
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    path = write(tmp_path / "entry.py")
    premise = PythonNamespacePremise((), 0, 0, ())
    snapshot = PythonSourceSnapshot.capture(path)
    options = dict(module_name="entry", is_package=False, import_scan_mode="full")
    request = load(
        path, project_root=None, namespace_premise=premise, source_snapshot=snapshot
    ).scan.source_requests
    records._write_persisted_import_scan(
        path,
        **options,
        namespace_premise=premise,
        scan=request,
        snapshot=snapshot,
    )
    assert (
        records._read_persisted_import_scan_record(
            path, **options, namespace_premise=premise, snapshot=snapshot
        )
        == request
    )
    cache_key = records._import_scan_cache_key(
        path, **options, namespace_premise=premise
    )
    payload = source_records("import-scan")[cache_key]
    payload["namespace_premise"] = retired
    replace_source_record("import-scan", cache_key, payload)
    assert (
        records._read_persisted_import_scan_record(
            path, **options, namespace_premise=premise, snapshot=snapshot
        )
        is None
    )


@pytest.mark.parametrize(
    "defect",
    [
        "nonclass-owner",
        "duplicate-owner",
        "invalid-origin",
        "missing-bases",
        "unknown-field",
    ],
)
def test_source_base_cache_rejects_incoherent_provenance(tmp_path, monkeypatch, defect):
    source = write(tmp_path / "entry.py", "class Base: pass\nclass Child(Base): pass\n")
    cold = load(source)
    assert cold.scan.source_values.bases
    assert load(source).cache_hit
    key = records._import_scan_cache_key(
        source, module_name="entry", is_package=False, import_scan_mode="full"
    )
    payload = source_records("import-scan")[key]
    values = payload["source_values"]
    if defect == "missing-bases":
        del values["bases"]
    elif defect == "unknown-field":
        values["invented"] = []
    elif defect == "duplicate-owner":
        values["bases"].append(copy.deepcopy(values["bases"][0]))
    elif defect == "nonclass-owner":
        values["bases"][0][0][-1] = "FunctionDef"
    else:
        values["bases"][0][1] = [[1, 0, 1, 1, 42]]
    replace_source_record("import-scan", key, payload)
    recovered = load(source)
    assert not recovered.cache_hit
    assert recovered.scan.source_values.bases == cold.scan.source_values.bases
    assert load(source).cache_hit


@pytest.mark.parametrize("started", [False, True])
def test_resumable_release_never_starts_body_or_finally(tmp_path, monkeypatch, started):
    import builtins

    body, final = "_molt_started_body", "_molt_started_finally"
    provider = write(
        tmp_path / "carrier.py",
        f"def produce():\n    try:\n        import {body}\n        yield 1\n"
        f"    finally:\n        import {final}\n",
    )
    source = "from carrier import produce\nitems = produce()\n"
    if started:
        source += "for item in items: break\n"
    source += "del items\n"
    entry = write(tmp_path / "entry.py", source)
    paths = {"entry": entry, "carrier": provider}
    paths.update((name, write(tmp_path / f"{name}.py")) for name in (body, final))
    attempts = []
    real_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name in {body, final}:
            attempts.append(name)
        return real_import(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            for name in paths:
                if name != "entry":
                    sys.modules.pop(name, None)
    assert attempts == ([body, final] if started else [])
    found = closed_source_imports(paths)
    assert (body in found) is started
    assert (final in found) is started


@pytest.mark.parametrize(
    "kind,asynchronous",
    [
        ("generator", True),
        ("async-generator", False),
        ("coroutine", False),
        ("coroutine", True),
        ("uncalled-generator", False),
        ("uncalled-async-generator", True),
    ],
)
def test_invalid_iteration_protocol_does_not_resume_known_carrier(
    tmp_path, monkeypatch, kind, asynchronous
):
    import builtins
    import inspect

    leaf = "_molt_invalid_iteration"
    is_async = "async-generator" in kind or kind == "coroutine"
    is_generator = "generator" in kind
    provider = write(
        tmp_path / "carrier.py",
        f"{'async ' if is_async else ''}def produce():\n    import {leaf}\n"
        + ("    yield 1\n" if is_generator else "    return 1\n"),
    )
    source = "from carrier import produce\nitems = produce" + (
        "\n" if kind.startswith("uncalled") else "()\n"
    )
    if asynchronous:
        source += (
            "import asyncio\nasync def consume():\n    async for item in items: pass\n"
        )
        operation = "asyncio.run(consume())"
    else:
        operation = "for item in items: pass"
    source += f"try:\n    {operation}\nexcept TypeError:\n    rejected = True\n"
    entry = write(tmp_path / "entry.py", source)
    leaf_path = write(tmp_path / f"{leaf}.py")
    attempts = []
    real_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name == leaf:
            attempts.append(name)
        return real_import(name, *args, **kwargs)

    namespace = {}
    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), namespace)
        finally:
            if inspect.iscoroutine(namespace.get("items")):
                namespace["items"].close()
            for name in ("carrier", leaf):
                sys.modules.pop(name, None)
    assert namespace.get("rejected") is True
    assert not attempts
    assert leaf not in closed_source_imports(
        {"entry": entry, "carrier": provider, leaf: leaf_path}
    )


def source_protocol_oracle(tmp_path, monkeypatch, source, provider, leaves):
    """CPython import observations and real closed-source worklist requests."""
    import builtins

    entry = write(tmp_path / "entry.py", source)
    paths = {"entry": entry, "carrier": write(tmp_path / "carrier.py", provider)}
    paths.update((name, write(tmp_path / f"{name}.py")) for name in leaves)
    previous = {name: sys.modules.get(name) for name in paths if name != "entry"}
    observed = []
    actual_import = builtins.__import__

    def observe(name, *args, **kwargs):
        if name in leaves:
            observed.append(name)
        return actual_import(name, *args, **kwargs)

    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        runtime.setattr(builtins, "__import__", observe)
        try:
            exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        finally:
            for name, value in previous.items():
                if value is None:
                    sys.modules.pop(name, None)
                else:
                    sys.modules[name] = value
    cold = closed_source_imports(paths)
    assert closed_source_imports(paths) == cold
    return observed, cold


@pytest.mark.parametrize("shadowed", [False, True])
def test_iter_acquisition_uses_only_the_receiver_protocol(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, shadowed: bool
) -> None:
    live, dormant = "_molt_iter_protocol_live", "_molt_iter_protocol_dormant"
    provider = (
        "class Cursor:\n"
        f"    def __iter__(self):\n        import {live}\n        return iter(())\n"
        f"    def ordinary(self): import {dormant}\n"
    )
    source = "from carrier import Cursor\n"
    if shadowed:
        source += (
            "import builtins\n"
            "def iter(value):\n"
            "    value.ordinary()\n"
            "    return builtins.iter(value)\n"
        )
    source += "iter(Cursor())\n"
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, source, provider, {live, dormant}
    )
    assert observed == ([dormant, live] if shadowed else [live])
    assert live in found
    assert (dormant in found) is shadowed


def test_iter_returns_the_user_iterators_other_methods(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    live, dormant = "_molt_iter_result_live", "_molt_iter_result_dormant"
    provider = (
        "class Iterator:\n"
        "    def __iter__(self): return self\n"
        "    def __next__(self): raise StopIteration\n"
        f"    def consume(self): import {live}\n"
        f"    def unrelated(self): import {dormant}\n"
        "class Iterable:\n"
        "    def __iter__(self): return Iterator()\n"
    )
    source = "from carrier import Iterable\niter(Iterable()).consume()\n"
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, source, provider, {live, dormant}
    )
    assert observed == [live]
    assert live in found
    assert dormant not in found


@pytest.mark.parametrize(
    "hook", ["absent", "declared", "rebound", "body_write", "type_descriptor"]
)
def test_class_creation_preserves_only_reachable_subclass_effects(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, hook: str
) -> None:
    name = "_molt_subclass_hook_leaf"
    provider = f"print(end='')\nclass Base:\n    def ordinary(self): import {name}\n"
    if hook == "declared":
        provider += "    def __init_subclass__(cls): cls().ordinary()\n"
    if hook in {"rebound", "body_write", "type_descriptor"}:
        provider += "def hook(cls): cls().ordinary()\n"
    if hook == "rebound":
        provider += "Base.__init_subclass__ = classmethod(hook)\n"
    if hook == "type_descriptor":
        provider += (
            "set_slot = type.__setattr__\n"
            "set_slot(Base, '__init_subclass__', classmethod(hook))\n"
        )
    provider += "class Child(Base):\n"
    provider += (
        "    Base.__init_subclass__ = classmethod(hook)\n"
        if hook == "body_write"
        else "    pass\n"
    )
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, "import carrier\n", provider, {name}
    )
    assert observed == ([] if hook == "absent" else [name])
    assert (name in found) is (hook != "absent")


def test_await_and_yield_from_keep_distinct_results_of_the_same_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    awaited, delegated, dormant = (
        "_molt_completion_awaited",
        "_molt_completion_delegated",
        "_molt_completion_dormant",
    )
    provider = (
        f"def async_callback(): import {awaited}\n"
        f"def sync_callback(): import {delegated}\n"
        f"def unused(): import {dormant}\n"
        "async def coroutine(): return async_callback\n"
        "def generator():\n    yield 0\n    return sync_callback\n"
        "def choose(want_async):\n"
        "    if want_async: return coroutine()\n"
        "    return generator()\n"
    )
    source = (
        "from carrier import choose\n"
        "async def via_await():\n    callback = await selected\n    callback()\n"
        "def via_delegate():\n    callback = yield from selected\n    callback()\n"
        "for mode in (True, False):\n"
        "    selected = choose(mode)\n"
        "    if mode:\n"
        "        try: via_await().send(None)\n"
        "        except StopIteration: pass\n"
        "    else:\n"
        "        for item in via_delegate(): pass\n"
    )
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, source, provider, {awaited, delegated, dormant}
    )
    assert observed == [awaited, delegated]
    assert {awaited, delegated} <= found
    assert dormant not in found


@pytest.mark.parametrize("throwing_iter", [False, True])
def test_await_drives_returned_iterator_without_reacquiring_iter(
    tmp_path, monkeypatch, throwing_iter
):
    leaf, wrong = "_molt_await_next", "_molt_await_wrong_iter"
    provider = (
        f"class Driver:\n    def __iter__(self):\n        import {wrong}\n"
        + (
            "        raise AssertionError('await must not call iter')\n"
            if throwing_iter
            else "        return self\n"
        )
        + f"    def __next__(self):\n        import {leaf}\n        raise StopIteration\n"
        "class Awaitable:\n    def __await__(self): return Driver()\n"
    )
    source = "from carrier import Awaitable\nimport asyncio\nasync def consume():\n    await Awaitable()\nasyncio.run(consume())\n"
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, source, provider, {leaf, wrong}
    )
    assert observed == [leaf]
    assert leaf in found
    assert wrong not in found


@pytest.mark.parametrize("asynchronous", [False, True])
@pytest.mark.parametrize("unpack", [False, True])
def test_async_context_calls_then_awaits_real_result_before_unpack(
    tmp_path, monkeypatch, asynchronous, unpack
):
    enter, leave, payload = (
        "_molt_context_enter_await",
        "_molt_context_exit_await",
        "_molt_context_payload_iter",
    )
    provider = (
        f"class Payload:\n    def __iter__(self):\n        import {payload}\n        yield 1\n        yield 2\n"
        f"class EnterAwaitable:\n    def __await__(self):\n        import {enter}\n        if False: yield\n        return Payload()\n"
        f"class ExitAwaitable:\n    def __await__(self):\n        import {leave}\n        if False: yield\n        return False\n"
        f"class Context:\n    {'async ' if asynchronous else ''}def __aenter__(self):\n"
        f"        return {'await ' if asynchronous else ''}EnterAwaitable()\n"
        f"    {'async ' if asynchronous else ''}def __aexit__(self, *args):\n"
        f"        return {'await ' if asynchronous else ''}ExitAwaitable()\n"
    )
    target = "(left, right)" if unpack else "value"
    source = f"from carrier import Context\nimport asyncio\nasync def consume():\n    async with Context() as {target}: pass\nasyncio.run(consume())\n"
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, source, provider, {enter, leave, payload}
    )
    assert observed == ([enter, payload, leave] if unpack else [enter, leave])
    assert {enter, leave} <= found
    assert (payload in found) is unpack


def test_custom_await_iterator_stopiteration_payload_retains_import_availability(
    tmp_path, monkeypatch
):
    next_leaf, unpack_leaf, wrong = (
        "_molt_driver_next",
        "_molt_driver_payload",
        "_molt_driver_wrong_iter",
    )
    provider = (
        f"class Payload:\n    def __iter__(self):\n        import {unpack_leaf}\n        yield 1\n        yield 2\n"
        f"class Driver:\n    def __iter__(self):\n        import {wrong}\n        raise AssertionError\n"
        f"    def __next__(self):\n        import {next_leaf}\n        raise StopIteration(Payload())\n"
        "class Awaitable:\n    def __await__(self): return Driver()\n"
        "class Context:\n    def __aenter__(self): return Awaitable()\n"
        "    async def __aexit__(self, *args): return False\n"
    )
    source = "from carrier import Context\nimport asyncio\nasync def consume():\n    async with Context() as (left, right): pass\nasyncio.run(consume())\n"
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, source, provider, {next_leaf, unpack_leaf, wrong}
    )
    assert observed == [next_leaf, unpack_leaf]
    assert {next_leaf, unpack_leaf} <= found
    assert wrong not in found


@pytest.mark.parametrize("consume_yielded", [False, True])
@pytest.mark.parametrize("delegates_yield", [False, True])
def test_yield_from_completion_is_distinct_from_forwarded_yielded_callback(
    tmp_path, monkeypatch, consume_yielded, delegates_yield
):
    returned_leaf, yielded_leaf = "_molt_delegate_returned", "_molt_delegate_yielded"
    provider = (
        f"def returned():\n    import {returned_leaf}\n"
        f"def yielded():\n    import {yielded_leaf}\n"
        "def inner():\n"
        + (
            "    yield yielded\n"
            if delegates_yield
            else "    if False: yield yielded\n"
        )
        + "    return returned\n"
        "def outer():\n    callback = yield from inner()\n    callback()\n"
    )
    source = "from carrier import outer\nfor item in outer():\n" + (
        "    item()\n" if consume_yielded else "    pass\n"
    )
    observed, found = source_protocol_oracle(
        tmp_path, monkeypatch, source, provider, {returned_leaf, yielded_leaf}
    )
    assert observed == (
        [yielded_leaf, returned_leaf]
        if consume_yielded and delegates_yield
        else [returned_leaf]
    )
    assert returned_leaf in found
    assert (yielded_leaf in found) is (consume_yielded and delegates_yield)


@pytest.mark.parametrize(
    "defect",
    [
        "short-function",
        "nonfunction-owner",
        "duplicate-function",
        "noncompletion-owner",
        "duplicate-completion",
        "retired-returns",
        "retired-awaits",
    ],
)
def test_function_completion_cache_rejects_retired_or_incoherent_records(
    tmp_path, monkeypatch, defect
):
    source = write(
        tmp_path / "entry.py",
        "def values():\n    yield 0\n    return 1\n"
        "def delegate():\n    return (yield from values())\n"
        "async def ready(): return 1\nasync def resumed(): return await ready()\n",
    )
    cold = load(source, purpose=SourceImportPurpose.INVENTORY)
    assert cold.scan.source_values.functions
    assert {owner[-1] for owner, _ in cold.scan.source_values.completions} == {
        "Await",
        "YieldFrom",
    }
    assert load(source, purpose=SourceImportPurpose.INVENTORY).cache_hit
    key = records._import_scan_cache_key(
        source,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        purpose=SourceImportPurpose.INVENTORY,
    )
    payload = source_records("import-scan")[key]
    values = payload["source_values"]
    if defect == "short-function":
        values["functions"][0] = values["functions"][0][:4]
    elif defect == "nonfunction-owner":
        values["functions"][0][0][-1] = "ClassDef"
    elif defect == "duplicate-function":
        values["functions"].append(copy.deepcopy(values["functions"][0]))
    elif defect == "noncompletion-owner":
        values["completions"][0][0][-1] = "Call"
    elif defect == "duplicate-completion":
        values["completions"].append(copy.deepcopy(values["completions"][0]))
    elif defect == "retired-returns":
        values["returns"] = values.pop("functions")
    else:
        values["awaits"] = values.pop("completions")
    replace_source_record("import-scan", key, payload)
    recovered = load(source, purpose=SourceImportPurpose.INVENTORY)
    assert not recovered.cache_hit
    assert recovered.scan.source_values == cold.scan.source_values
    warm = load(source, purpose=SourceImportPurpose.INVENTORY)
    assert warm.cache_hit
    assert warm.scan.source_values == cold.scan.source_values


@pytest.mark.parametrize(
    "operation", ["subscript", "iterate", "sequence", "async_iterate"]
)
def test_typed_element_value_matches_actual_cpython_protocol_selection(
    tmp_path, operation
):
    import asyncio
    from molt.compiler_analysis.python_source_keys import PythonElementProtocol
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_source_demands import PythonSourceDemands

    provider_source = (
        "class Items:\n"
        "    def __getitem__(self, index):\n        def subscription(): pass\n        return subscription\n"
        "    def __iter__(self):\n        def iteration(): pass\n        yield iteration\n"
        "    def __aiter__(self): return self\n"
        "    async def __anext__(self):\n        def asynchronous(): pass\n        return asynchronous\n"
    )
    if operation == "sequence":
        provider_source = provider_source.replace(
            "    def __iter__(self):\n        def iteration(): pass\n        yield iteration\n",
            "",
        )
    namespace = {}
    exec(
        compile(
            provider_source, "<element-protocol-oracle>", "exec", dont_inherit=True
        ),
        namespace,
    )
    instance = namespace["Items"]()
    if operation == "subscript":
        actual = instance[0]
        source = "from carrier import Items\nvalue = Items()[0]\n"
    elif operation in {"iterate", "sequence"}:
        actual = next(iter(instance))
        source = "from carrier import Items\nfor value in Items(): pass\n"
    else:

        async def first():
            async for value in instance:
                return value

        actual = asyncio.run(first())
        source = "from carrier import Items\nasync def consume():\n    async for value in Items(): pass\n"
    entry_tree = ast.parse(source)
    owner = (
        _source_import_scan.python_node_source_key(entry_tree.body[-1])
        if operation == "async_iterate"
        else None
    )
    projections = {
        "carrier": _source_import_scan._collect_imports_for_graph(
            ast.parse(provider_source),
            "carrier",
            import_scan_mode="module_init",
            purpose=SourceImportPurpose.INVENTORY,
        ),
        "entry": _source_import_scan._collect_imports_for_graph(
            entry_tree, "entry", import_scan_mode="module_init"
        ),
    }
    protocol = (
        PythonElementProtocol.ITERATE
        if operation == "sequence"
        else PythonElementProtocol(operation)
    )
    active = projections["entry"]
    if owner is not None:
        assert all(item.protocol != protocol for item in active.source_values.elements)
        active = _source_import_scan._collect_imports_for_graph(
            entry_tree, "entry", source_owner=owner, import_scan_mode="module_init"
        )
    element = next(
        item for item in active.source_values.elements if item.protocol == protocol
    )
    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda _module: None,
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
    )
    publications = list(projections.items())
    if owner is not None:
        publications.append(("entry", active))
    for module, projection in publications:
        engine.accept(
            module,
            _ImportScanRequests(
                (),
                (),
                source_values=projection.source_values,
                source_bindings=projection.source_bindings,
            ),
        )
    value = ("entry", element.owner)
    engine.demand(value, "read")
    engine.drain()
    expected = {actual.__code__.co_firstlineno}
    if operation == "iterate":
        # Published method candidates do not prove exact slot presence after
        # callbacks. Sync iteration retains CPython's sequence fallback, while
        # subscription and async iteration never receive that alternative.
        expected.add(instance[0].__code__.co_firstlineno)
    assert {
        candidate.source[0] for candidate in engine.value_candidates[value]
    } == expected


@pytest.mark.parametrize(
    "defect", ["old-width", "unknown-protocol", "duplicate-owner", "invalid-origin"]
)
def test_element_cache_rejects_untyped_or_incoherent_operation(
    tmp_path, monkeypatch, defect
):
    source = write(
        tmp_path / "entry.py",
        "class Items:\n    def __iter__(self): yield 1\nfor value in Items(): pass\n",
    )
    cold = load(source)
    assert cold.scan.source_values.elements
    assert load(source).cache_hit
    key = records._import_scan_cache_key(
        source, module_name="entry", is_package=False, import_scan_mode="full"
    )
    payload = source_records("import-scan")[key]
    elements = payload["source_values"]["elements"]
    if defect == "old-width":
        elements[0] = [elements[0][0], elements[0][2]]
    elif defect == "unknown-protocol":
        elements[0][1] = "maybe_iterate"
    elif defect == "duplicate-owner":
        elements.append(copy.deepcopy(elements[0]))
    else:
        elements[0][2] = [[0, 0, 0, 0, 42]]
    replace_source_record("import-scan", key, payload)
    recovered = load(source)
    assert not recovered.cache_hit
    assert recovered.scan.source_values == cold.scan.source_values
    assert load(source).cache_hit


@pytest.mark.parametrize(
    ("expression", "change"),
    [
        ("'loaded.py'", "create"),
        ("'loaded.py'", "remove"),
        ("Path('loaded.py').resolve()", "create"),
        ("'site'", "create-main"),
    ],
)
def test_a_source_execution_target_is_a_fact_of_the_operation(
    tmp_path, expression, change
):
    """The target decision belongs to this build's import topology."""
    from molt.cli.python_import_topology import PythonImportTopologyChangedError

    source = write(
        tmp_path / "entry.py",
        f"from pathlib import Path\nimport runpy\nrunpy.run_path({expression})\n",
    )
    target = tmp_path / "loaded.py"
    if change == "remove":
        write(target)
    cache = _ModuleResolutionCache()
    first = load(source, resolution_cache=cache)
    assert bool(first.scan.source_executions) is (change == "remove")
    metadata = tmp_path.stat()
    if change == "create":
        write(target)
    elif change == "remove":
        target.unlink()
    else:
        write(tmp_path / "site/__main__.py")
    os.utime(tmp_path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    with pytest.raises(PythonImportTopologyChangedError):
        cache.import_topology.validate()


@pytest.mark.parametrize("source_kind", ["user", "stdlib"])
def test_source_records_are_shared_by_every_project(
    tmp_path, isolated_molt_cache, source_kind
):
    """Both user and toolchain sources reuse one row across project sessions."""
    from molt.cli.source_record_store import shared_source_records
    from molt.cli.module_resolution import _stdlib_root_path

    stdlib = _stdlib_root_path()
    source = (
        stdlib / "keyword.py"
        if source_kind == "stdlib"
        else write(tmp_path / "shared/keyword.py")
    )
    first, second = tmp_path / "first", tmp_path / "second"
    for project in (first, second):
        project.mkdir()

    def scan(project: Path):
        return load(
            source,
            module_name="keyword",
            project_root=project,
            roots=[stdlib],
            stdlib_root=stdlib,
        )

    assert not scan(first).cache_hit
    assert scan(second).cache_hit
    store = shared_source_records()
    assert store.path.is_relative_to(isolated_molt_cache)
    assert len(source_records("import-scan")) == 1


def test_shared_requests_complete_against_each_projects_roots(tmp_path):
    """One source row cannot preserve another project's completed star edges."""
    source = write(tmp_path / "shared/entry.py", "from pkg import *\n")
    first, second = tmp_path / "first", tmp_path / "second"
    for project, child in ((first, "left"), (second, "right")):
        write(project / "pkg/__init__.py", f"__all__ = [{child!r}]\n")
        write(project / f"pkg/{child}.py")

    def scan(project):
        return load(source, project_root=project, roots=[project])

    cold = scan(first)
    warm = scan(second)
    assert not cold.cache_hit and warm.cache_hit
    assert "pkg.left" in cold.scan.imports and "pkg.right" not in cold.scan.imports
    assert "pkg.right" in warm.scan.imports and "pkg.left" not in warm.scan.imports
    assert cold.scan.source_requests == warm.scan.source_requests
    assert "pkg.left" not in warm.scan.source_requests.imports
    assert "pkg.right" not in warm.scan.source_requests.imports
    assert scan(first).scan.imports == cold.scan.imports


def test_source_request_recency_requires_complete_admission(tmp_path: Path) -> None:
    """An import scan is a record file (cache frontier step 2): only a record
    its consumer admits renews its blob; a refused one keeps its age."""
    import os
    import time

    from molt.cli.frontend_record_files import (
        FrontendRecordFiles,
        frontend_record_files,
    )

    source = write(tmp_path / "entry.py", "import math\n")
    load(source)
    options = dict(module_name="entry", is_package=False, import_scan_mode="full")
    key = records._import_scan_cache_key(source, **options)
    payload = source_records("import-scan")[key]
    files = frontend_record_files()

    def aged_blob() -> tuple[Path, float]:
        assert files.save()
        record = FrontendRecordFiles(files.cache_root).get(
            "import-scan", key, max_bytes=2**31
        )
        assert record is not None and record.path is not None
        blob = record.path
        stale = time.time() - 2 * 3600
        os.utime(blob, (stale, stale))
        return blob, stale

    malformed = copy.deepcopy(payload)
    # This failure is reached only after identity and source admission.
    malformed["source_values"]["invented"] = []
    replace_source_record("import-scan", key, malformed)
    blob, stale = aged_blob()
    assert records._read_persisted_import_scan_record(source, **options) is None
    assert blob.stat().st_mtime == pytest.approx(stale, abs=1)

    replace_source_record("import-scan", key, payload)
    blob, stale = aged_blob()
    assert records._read_persisted_import_scan_record(source, **options) is not None
    assert blob.stat().st_mtime > stale + 3600


@pytest.mark.parametrize(
    ("function", "argument"),
    [("select", "left"), ("select", "right"), ("other", "left")],
)
def test_shared_deferred_requests_bind_owner_and_arguments(
    tmp_path, monkeypatch, function, argument
):
    import builtins

    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = write(
        tmp_path / "shared/entry.py",
        "def select(choice):\n"
        "    if choice == 'left':\n        import left_leaf\n"
        "    else:\n        import right_leaf\n"
        "def other(choice):\n    import other_leaf\n",
    )
    namespace = {}
    text = source.read_text(encoding="utf-8")
    exec(compile(text, str(source), "exec", dont_inherit=True), namespace)
    expected = []
    with monkeypatch.context() as runtime:
        runtime.setattr(
            builtins,
            "__import__",
            lambda name, *_args, **_kwargs: expected.append(name),
        )
        namespace[function](argument)
    owners = {node.name: python_node_source_key(node) for node in ast.parse(text).body}
    first, second = tmp_path / "first", tmp_path / "second"
    for project in (first, second):
        project.mkdir()
    options = dict(
        import_scan_mode="module_init",
        source_owner=owners[function],
        source_arguments=((0, argument),),
    )
    cold = load(source, project_root=first, **options)
    warm = load(source, project_root=second, **options)
    assert not cold.cache_hit and warm.cache_hit
    assert list(cold.scan.imports) == expected == list(warm.scan.imports)

    key = records._import_scan_cache_key(
        source, module_name="entry", is_package=False, **options
    )
    payload = source_records("import-scan")[key]
    payload["source_arguments"] = [argument + "_different"]
    replace_source_record("import-scan", key, payload)
    recovered = load(source, project_root=second, **options)
    assert not recovered.cache_hit
    assert list(recovered.scan.imports) == expected


def test_one_operation_resolves_the_shared_record_root_once(tmp_path, monkeypatch):
    """Thousands of record requests share one root admission per operation."""
    from molt.cli import source_record_store as storage
    from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction

    resolved = []
    default_cache = storage._default_molt_cache

    def cache_root():
        root = default_cache()
        resolved.append(root)
        return root

    monkeypatch.setattr(storage, "_default_molt_cache", cache_root)
    sources = [write(tmp_path / f"m{index}.py") for index in range(3)]
    with _source_tree_fingerprint_transaction():
        assert not any(load(path, module_name=path.stem).cache_hit for path in sources)
        assert all(load(path, module_name=path.stem).cache_hit for path in sources)
        first = storage.shared_source_records()
    assert len(resolved) == 1
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "later-cache"))
    with _source_tree_fingerprint_transaction():
        second = storage.shared_source_records()
    assert len(resolved) == 2
    assert second.path != first.path
    assert second.path.is_relative_to(tmp_path / "later-cache")


def test_module_export_fact_is_shared_by_owner_arguments_and_star_consumer(
    tmp_path, monkeypatch
):
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = (
        "__all__ = ['visible']\n"
        "def first(name):\n    return name\n"
        "def second(name):\n    return name\n"
    )
    package = write(tmp_path / "pkg/__init__.py", source)
    visible = write(tmp_path / "pkg/visible.py")
    namespace = {}
    exec(compile(source, str(package), "exec", dont_inherit=True), namespace)
    expected = tuple(namespace["__all__"])
    target = TargetPythonVersion(3, 12, 0)
    cache = _ModuleResolutionCache()
    parsed = cache.parse_module_ast(
        package, source, filename=str(package), target_python=target
    )
    original = _source_import_scan.python_scope_declarations
    declarations = []

    def observe(*args, **kwargs):
        declarations.append(args[0])
        return original(*args, **kwargs)

    monkeypatch.setattr(_source_import_scan, "python_scope_declarations", observe)
    owners = [python_node_source_key(node) for node in parsed.tree.body[1:]]
    projections = [
        _source_import_scan._collect_imports_for_graph(
            parsed.tree,
            "pkg",
            is_package=True,
            import_scan_mode="module_init",
            source_owner=owner,
            source_arguments=arguments,
            target_python=target,
            ast_digest_admission=parsed.admission,
        )
        for owner, arguments in (
            (None, ()),
            (owners[0], ((0, "one"),)),
            (owners[0], ((0, "two"),)),
            (owners[1], ((0, "one"),)),
        )
    ]
    children, exports = scanner._expand_static_package_all_star_children(
        ("pkg",),
        roots=[tmp_path],
        stdlib_root=tmp_path / "stdlib",
        stdlib_allowlist=set(),
        resolution_cache=cache,
        target_python=target,
        module_graph={"pkg": package, "pkg.visible": visible},
    )
    assert all(projection.all_exports.names == expected for projection in projections)
    assert exports["pkg"].names == expected
    assert children == {"pkg": ("pkg.visible",)}
    # Only the two function declarations need the module-level export check;
    # changing an activated owner or its arguments must not repeat that walk.
    assert len(declarations) == 2


def test_namespace_syntax_is_shared_by_binding_and_graph_projections(
    tmp_path, monkeypatch
):
    from molt.compiler_analysis import python_binding_flow, python_namespace_writes
    from molt.compiler_analysis.python_namespace_graph import (
        PythonNamespacePremise,
        python_namespace_binding_policy,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = (
        "from . import target\n"
        "__all__ = ['namespace_visible']\n"
        "def namespace_callback(name):\n"
        "    global namespace_marker\n"
        "    namespace_marker = name\n"
        "    target.flag = name\n"
    )
    path = write(tmp_path / "pkg/entry.py", source)
    target = TargetPythonVersion(3, 12, 0)
    parsed = _ModuleResolutionCache().parse_module_ast(
        path, source, filename=str(path), target_python=target
    )
    original = python_namespace_writes._NamespaceWriteCollector.collect
    collected = []

    def collect(collector):
        collected.append(collector.package)
        return original(collector)

    monkeypatch.setattr(
        python_namespace_writes._NamespaceWriteCollector, "collect", collect
    )
    owner = python_node_source_key(parsed.tree.body[-1])
    for premise in (
        PythonNamespacePremise((), 0, 0, ()),
        PythonNamespacePremise(("namespace_marker",), 0, 0, ()),
        PythonNamespacePremise(None, 0, 0, ()),
    ):
        for source_owner, arguments in (
            (None, ()),
            (owner, ((0, "one"),)),
            (owner, ((0, "two"),)),
        ):
            projection = _source_import_scan._collect_imports_for_graph(
                parsed.tree,
                "pkg.entry",
                source_owner=source_owner,
                source_arguments=arguments,
                namespace_premise=premise,
                target_python=target,
                ast_digest_admission=parsed.admission,
            )
            if source_owner is None:
                assert projection.namespace_summary.own_names == {"namespace_marker"}
                assert {
                    write.module
                    for write in projection.namespace_summary.foreign_writes
                } == {"pkg.target"}
            else:
                assert projection.namespace_summary is None
        # The facts-only API uses the same core/source generation as the index
        # API exercised by graph discovery; it must retain the syntax owner.
        python_binding_flow.analyze_python_binding_facts(
            parsed.tree,
            source_digest=parsed.admission.digest,
            ast_digest_admission=parsed.admission,
            policy=python_namespace_binding_policy(
                python_binding_flow.PythonBindingFlowPolicy(
                    target_python=target.feature_version
                ),
                premise,
            ),
        )
    # Resolving foreign relative imports needs a package; own callback names do
    # not. Neither syntax fact depends on owners, arguments or graph premises.
    assert collected == ["pkg"]
    relocated = _source_import_scan._collect_imports_for_graph(
        parsed.tree,
        "other.entry",
        namespace_premise=PythonNamespacePremise((), 0, 0, ()),
        target_python=target,
        ast_digest_admission=parsed.admission,
    )
    assert relocated.namespace_summary.own_names == {"namespace_marker"}
    assert {write.module for write in relocated.namespace_summary.foreign_writes} == {
        "other.target"
    }
    assert collected == ["pkg", "other"]


def test_module_export_fact_qualifies_target_and_namespace_premise():
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission

    source = "__all__ = ['visible']\nmarker: __all__\n"
    tree = ast.parse(source)
    admission = _PythonAstDigestAdmission(tree)
    closed = PythonNamespacePremise((), 0, 0, ())

    def exports(minor, premise):
        return _source_import_scan._static_module_all_exports(
            tree,
            target_python=TargetPythonVersion(3, minor, 0),
            namespace_premise=premise,
            ast_digest_admission=admission,
        )

    # Before PEP 649's deferred annotations the mutable export list escapes
    # through __annotations__. From 3.14 that annotation has not executed.
    namespace = {}
    exec(compile(source, "exports.py", "exec", dont_inherit=True), namespace)
    if sys.version_info < (3, 14):
        assert namespace["__annotations__"]["marker"] is namespace["__all__"]
    assert exports(13, closed).status == "unknown"
    assert exports(14, closed).names == ("visible",)
    assert (
        exports(14, PythonNamespacePremise(("__all__",), 0, 0, ())).status == "unknown"
    )
    assert exports(14, PythonNamespacePremise(None, 0, 0, ())).status == "unknown"
    assert exports(14, closed).names == ("visible",)
    with pytest.raises(ValueError, match="different tree"):
        _source_import_scan._static_module_all_exports(
            ast.parse(source),
            target_python=TargetPythonVersion(3, 14, 0),
            ast_digest_admission=admission,
        )


def test_opaque_module_getattr_still_blocks_proof_of_absent_exports(
    tmp_path: Path,
) -> None:
    from types import ModuleType

    source = "public = 1\n__getattr__ = tuple\n"
    module = ModuleType("entry")
    exec(compile(source, "entry.py", "exec"), vars(module))
    assert getattr(module, "__all__") == tuple("__all__")

    path = write(tmp_path / "entry.py", source)
    cold = load(path)
    assert cold.scan.source_requests.all_exports.status == "unknown"
    assert not any(
        name == "__getattr__"
        for owner, names, _ in cold.scan.source_values.namespaces
        if owner[-1] == "Module"
        for name, _ in names
    )

    key = records._import_scan_cache_key(
        path, module_name="entry", is_package=False, import_scan_mode="full"
    )
    payload = source_records("import-scan")[key]
    module_row = next(
        row for row in payload["source_values"]["namespaces"] if row[0][-1] == "Module"
    )
    module_row[1].append(["storage_only", []])
    replace_source_record("import-scan", key, payload)
    rebuilt = load(path)
    assert not rebuilt.cache_hit
    assert rebuilt.scan.source_requests == cold.scan.source_requests
    assert load(path).cache_hit


def test_module_export_fact_does_not_outlive_source_generation(tmp_path):
    source = write(tmp_path / "entry.py", "__all__ = ['before']\n")
    cache = _ModuleResolutionCache()
    before = load(source, resolution_cache=cache)
    write(source, "__all__ = ['after_']\n")
    namespace = {}
    exec(
        compile(
            source.read_text(encoding="utf-8"), str(source), "exec", dont_inherit=True
        ),
        namespace,
    )
    after = load(source, resolution_cache=cache)
    assert before.scan.source_requests.all_exports.names == ("before",)
    assert after.scan.source_requests.all_exports.names == tuple(namespace["__all__"])
    assert not after.cache_hit


@pytest.mark.parametrize(
    "body,expected",
    [
        (
            "def factory(): return take('molt_module_import')\nloader = factory()\n",
            False,
        ),
        ("def factory(): return take('molt_module_import')\nfactory()('leaf')\n", True),
        (
            "def replacement(name): return take('molt_module_import')\n"
            "acquire = take if flag else replacement\n"
            "acquire('molt_sys_platform')('leaf')\n",
            True,
        ),
        ("def factory(): return take\nfactory()('molt_module_import')('leaf')\n", True),
        (
            "def factory(): return [take('molt_module_import')]\n"
            "alias = factory()\nalias[0]('leaf')\n",
            True,
        ),
        (
            "def factory(reader=take('molt_module_import')): return reader\n"
            "factory()('leaf')\n",
            True,
        ),
        (
            "def identity(reader): return reader\n"
            "identity(take('molt_module_import'))('leaf')\n",
            True,
        ),
        (
            "def identity(*, reader): return reader\n"
            "identity(reader=take('molt_module_import'))('leaf')\n",
            True,
        ),
        (
            "def factory(): yield take('molt_module_import')\n"
            "for loader in factory(): loader('leaf')\n",
            True,
        ),
        ("def factory(): yield take('molt_module_import')\nvalue = factory()\n", False),
        (
            "async def factory(): return take('molt_module_import')\n"
            "async def main():\n    loader = await factory()\n    loader('leaf')\n"
            "run(main())\n",
            True,
        ),
        ("take(selector)('leaf')\n", True),
        ("take(*values)('leaf')\n", True),
        ("take(**keywords)('leaf')\n", True),
        ("take('molt_sys_platform')()\n", False),
    ],
)
def test_canonical_callable_results_follow_actual_consumers(tmp_path, body, expected):
    import asyncio

    observed = []

    def importer(name):
        observed.append(name)

    def acquire(name, namespace=None):
        return importer if name == "molt_module_import" else lambda: "linux"

    # This independent runtime oracle distinguishes obtaining a callable from
    # invoking it; the source solver must preserve the same execution boundary.
    exec(
        compile(body, "<canonical-callable-oracle>", "exec"),
        {
            "take": acquire,
            "flag": False,
            "selector": "molt_module_import",
            "values": ("molt_module_import",),
            "keywords": {"name": "molt_module_import"},
            "run": asyncio.run,
        },
    )
    assert bool(observed) is expected
    entry = write(
        tmp_path / "entry.py",
        "from _intrinsics import require_intrinsic as take\n" + body,
    )
    protocol = set()
    closed_source_imports({"entry": entry}, protocol_modules=protocol)
    assert ("entry" in protocol) is expected


@pytest.mark.parametrize("factory", [False, True])
def test_zero_argument_importer_cannot_execute_import_protocol(tmp_path, factory):
    body = (
        "def factory(): return take('molt_module_import')\nfactory()()\n"
        if factory
        else "take('molt_module_import')()\n"
    )
    observed = []

    def importer(name):
        observed.append(name)

    with pytest.raises(TypeError):
        exec(body, {"take": lambda name: importer})
    assert observed == []
    entry = write(
        tmp_path / "entry.py",
        "from _intrinsics import require_intrinsic as take\n" + body,
    )
    protocol = set()
    closed_source_imports({"entry": entry}, protocol_modules=protocol)
    assert protocol == set()


@pytest.mark.parametrize("invoke", [False, True])
def test_returned_public_importer_admits_actual_user_leaf(tmp_path, invoke):
    leaf = write(tmp_path / "_molt_returned_import_leaf.py", "sentinel = 42\n")
    entry = write(
        tmp_path / "entry.py",
        "import importlib\ndef factory(): return importlib.import_module\n"
        "loader = factory()\n"
        + ("loader('_molt_returned_import_leaf')\n" if invoke else ""),
    )
    for _ in range(2):
        graph = discovery._discover_module_graph(
            entry, [tmp_path], [tmp_path], tmp_path / "stdlib", tmp_path, set()
        )
        assert (graph.graph.get("_molt_returned_import_leaf") == leaf) is invoke


def test_canonical_callable_cache_preserves_strict_new_rows(tmp_path):
    entry = write(
        tmp_path / "entry.py",
        "from _intrinsics import require_intrinsic as take\n"
        "def factory(): return take\n"
        "factory()('molt_module_import')('leaf')\n",
    )
    cold = load(entry)
    warm = load(entry)
    assert warm.cache_hit
    assert warm.scan.source_values == cold.scan.source_values
    factory = _source_import_scan.python_node_source_key(
        ast.parse(entry.read_text(encoding="utf-8")).body[1]
    )
    active = load(entry, source_owner=factory, import_scan_mode="module_init")
    assert active.scan.source_values.callables
    assert cold.scan.source_values.call_imports
    leaf = write(tmp_path / "leaf.py")
    assert "leaf" in closed_source_imports({"entry": entry, "leaf": leaf})
    key = records._import_scan_cache_key(
        entry, module_name="entry", is_package=False, import_scan_mode="full"
    )
    payload = source_records("import-scan")[key]
    payload["source_values"]["calls"][0] = payload["source_values"]["calls"][0][:2]
    replace_source_record("import-scan", key, payload)
    assert not load(entry).cache_hit


def test_call_argument_transfer_observes_actuals_without_replaying_return_wakes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCallArguments,
        SourceCandidate,
        SourceValue,
        _OperandDeltas,
    )
    from molt.compiler_analysis.python_source_keys import (
        PythonSourceKey,
        python_node_source_key,
    )

    tree = ast.parse(
        "def payload(): pass\n"
        "def result(): pass\n"
        "def make(): return payload\n"
        "def receive(callback): return result\n"
        "receive(callback=make())\n"
    )
    premise = PythonNamespacePremise((), 0, 0, ())
    projection = _source_import_scan._collect_imports_for_graph(
        tree,
        "entry",
        namespace_premise=premise,
        purpose=SourceImportPurpose.INVENTORY,
    )
    inventory = _ImportScanRequests((), ())._replace(**projection._asdict())
    initial = inventory._replace(
        source_values=inventory.source_values._replace(functions=())
    )
    payload, result, make, receive, call = tree.body
    assert isinstance(make, ast.FunctionDef)
    assert isinstance(receive, ast.FunctionDef)
    assert isinstance(call, ast.Expr)
    site = python_node_source_key(call.value)
    formal = ("entry", python_node_source_key(receive.args.args[0]))
    outcomes = {row.owner: row for row in inventory.source_values.functions}
    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    engine.accept("entry", initial)
    engine.demand(("entry", site), "read")
    engine.drain()
    assert not engine.value_candidates.get(formal)

    def publish_outcome(definition: ast.FunctionDef) -> None:
        body = _source_import_scan._collect_imports_for_graph(
            tree,
            "entry",
            source_owner=python_node_source_key(definition),
            namespace_premise=premise,
        )
        assert body.source_values.functions == (
            outcomes[python_node_source_key(definition)],
        )
        engine.accept("entry", _ImportScanRequests((), ())._replace(**body._asdict()))
        engine.drain()

    publish_outcome(make)
    assert engine.value_candidates[formal] == {
        SourceCandidate("entry", python_node_source_key(payload))
    }

    routed: list[PythonSourceKey] = []
    route = engine._route_call_arguments

    def observe(
        module: str,
        inputs: SourceCallArguments,
        user: SourceValue,
        deltas: _OperandDeltas | None,
    ) -> None:
        routed.append(inputs.site)
        route(module, inputs, user, deltas)

    monkeypatch.setattr(engine, "_route_call_arguments", observe)
    publish_outcome(receive)
    assert engine.value_candidates[("entry", site)] == {
        SourceCandidate("entry", python_node_source_key(result))
    }
    assert site not in routed


def test_call_argument_routing_joins_both_deltas(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.compiler_analysis.python_binding_facts import (
        PythonSourceCall,
        PythonSourceCallableDeclaration,
    )
    from molt.compiler_analysis.python_function_kind import FunctionKind
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCallArguments,
        SourceCandidate,
        SourceValue,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    tree = ast.parse(
        "def first(callback): pass\n"
        "def second(callback): pass\n"
        "def previous(): pass\n"
        "def added(): pass\n"
        "def invoke(callee, actual): callee(actual)\n"
    )
    first, second, previous, added, invoke = tree.body
    assert isinstance(first, ast.FunctionDef) and isinstance(second, ast.FunctionDef)
    assert isinstance(invoke, ast.FunctionDef) and isinstance(invoke.body[0], ast.Expr)
    module = "entry"
    key = python_node_source_key
    site = key(invoke.body[0].value)
    selection, actual = map(key, invoke.args.args)
    engine = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    engine.modules.add(module)
    for definition in (first, second):
        owner = key(definition)
        engine.declarations[(module, owner)] = PythonSourceCallableDeclaration(
            owner, FunctionKind.SYNC, (key(definition.args.args[0]),)
        )
    engine.calls[(module, site)] = PythonSourceCall(
        site,
        frozenset({selection}),
        0,
        ((0, frozenset({actual}), frozenset({actual})),),
    )
    engine._ensure_value((module, SourceCallArguments(site)))
    old_payload = SourceCandidate(module, key(previous))
    added_payload = SourceCandidate(module, key(added))
    engine._queue_arguments((module, selection), {SourceCandidate(module, key(first))})
    engine._queue_arguments((module, actual), {old_payload})
    engine.drain()

    routed: dict[SourceValue, set[SourceCandidate]] = {}
    enqueue = engine._queue_arguments

    def observe(formal: SourceValue, values: set[SourceCandidate]) -> None:
        routed.setdefault(formal, set()).update(values)
        enqueue(formal, values)

    monkeypatch.setattr(engine, "_queue_arguments", observe)
    enqueue((module, selection), {SourceCandidate(module, key(second))})
    enqueue((module, actual), {added_payload})
    engine.drain()
    first_formal = (module, key(first.args.args[0]))
    second_formal = (module, key(second.args.args[0]))
    assert routed == {
        first_formal: {added_payload},
        second_formal: {old_payload, added_payload},
    }
    assert engine.value_candidates[first_formal] == {old_payload, added_payload}
    assert engine.value_candidates[second_formal] == {old_payload, added_payload}


def test_late_canonical_provider_activation_retains_application_consumer():
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_source_demands import PythonSourceDemands
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    sources = {
        "entry": "from provider import factory\nfactory()('leaf')\n",
        "provider": (
            "from _intrinsics import require_intrinsic as take\n"
            "def factory(): return take('molt_module_import')\n"
        ),
    }
    protocols = set()
    imports = set()
    queued = []
    engine = PythonSourceDemands(
        admit_imports=lambda _module, names: imports.update(names),
        queue_body=lambda *job: queued.append(job),
        require_runtime_import_protocol=protocols.add,
    )
    for module in ("entry", "provider"):
        projection = _source_import_scan._collect_imports_for_graph(
            ast.parse(sources[module]),
            module,
            namespace_premise=PythonNamespacePremise((), 0, 0, ()),
        )
        engine.accept(
            module,
            _ImportScanRequests((), ())._replace(**projection._asdict()),
        )
        engine.drain()
        if module == "entry":
            assert protocols == set()
    assert not protocols
    (job,) = queued
    module, owner, arguments = job
    assert module == "provider"
    body = _source_import_scan._collect_imports_for_graph(
        ast.parse(sources[module]),
        module,
        import_scan_mode="module_init",
        source_owner=owner,
        source_arguments=arguments,
        namespace_premise=PythonNamespacePremise((), 0, 0, ()),
    )
    engine.accept(module, _ImportScanRequests((), ())._replace(**body._asdict()))
    engine.drain()
    assert "entry" in protocols
    assert "leaf" in imports


@pytest.mark.parametrize("route", ["direct", "deferred", "intrinsic"])
@pytest.mark.parametrize("fromlist,selected", [((), "root"), (("child",), "leaf")])
def test_import_result_selects_root_or_base_without_exposing_fromlist_child(
    tmp_path, monkeypatch, route, fromlist, selected
):
    package = "_molt_import_result_package"
    markers = {
        kind: f"_molt_import_result_{kind}" for kind in ("root", "leaf", "child")
    }
    paths = {
        package: write(
            tmp_path / package / "__init__.py",
            f"def chosen(): import {markers['root']}\n",
        ),
        f"{package}.sub": write(
            tmp_path / package / "sub" / "__init__.py",
            f"def chosen(): import {markers['leaf']}\n",
        ),
        f"{package}.sub.child": write(
            tmp_path / package / "sub" / "child.py",
            f"def chosen(): import {markers['child']}\n",
        ),
    }
    paths.update({name: write(tmp_path / f"{name}.py") for name in markers.values()})
    body = (
        "from _intrinsics import require_intrinsic as take\n"
        "loader = take('molt_importlib_import_transaction')\n"
        f"result = loader('{package}.sub', None, None, {fromlist!r}, 0)\nresult.chosen()\n"
        if route == "intrinsic"
        else (
            "def factory(): return __import__\nloader = factory()\n"
            if route == "deferred"
            else "loader = __import__\n"
        )
        + f"result = loader('{package}.sub', fromlist={fromlist!r})\nresult.chosen()\n"
    )
    paths["entry"] = write(tmp_path / "entry.py", body)
    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        namespace = {"take": lambda name: __import__}
        try:
            exec(body.split("\n", 1)[1] if route == "intrinsic" else body, namespace)
            assert namespace["result"].__name__ == (
                package if selected == "root" else f"{package}.sub"
            )
            assert {name for name in markers.values() if name in sys.modules} == {
                markers[selected]
            }
        finally:
            for name in (*paths, *markers.values()):
                sys.modules.pop(name, None)
    admitted = closed_source_imports(paths)
    assert admitted.intersection(markers.values()) == {markers[selected]}


@pytest.mark.parametrize("route", ["direct", "deferred", "intrinsic"])
@pytest.mark.parametrize(
    "name,level,fromlist,imported,returned",
    [
        ("", 1, (), "branch", "branch"),
        ("child", 1, (), "branch.child", "branch.child"),
        ("child.deep", 1, (), "branch.child.deep", "branch.child"),
        ("child.deep", 1, ("chosen",), "branch.child.deep", "branch.child.deep"),
        ("child.deep", 2, (), "child.deep", "child"),
    ],
)
def test_relative_import_result_retains_the_resolved_package_prefix(
    tmp_path, monkeypatch, route, name, level, fromlist, imported, returned
):
    package = "_molt_relative_result_package"
    suffixes = (
        "",
        "branch",
        "branch.child",
        "branch.child.deep",
        "child",
        "child.deep",
    )
    modules = {
        suffix: package + (f".{suffix}" if suffix else "") for suffix in suffixes
    }
    markers = {
        suffix: f"_molt_relative_result_{suffix.replace('.', '_') or 'root'}"
        for suffix in suffixes
    }
    paths = {
        module: write(
            tmp_path.joinpath(*module.split("."), "__init__.py"),
            f"def chosen(): import {markers[suffix]}\n",
        )
        for suffix, module in modules.items()
    }
    paths.update(
        {marker: write(tmp_path / f"{marker}.py") for marker in markers.values()}
    )
    prefix = (
        "from _intrinsics import require_intrinsic as take\n"
        "loader = take('molt_importlib_import_transaction')\n"
        if route == "intrinsic"
        else (
            "def factory(): return __import__\nloader = factory()\n"
            if route == "deferred"
            else "loader = __import__\n"
        )
    )
    body = prefix + (
        f"result = loader({name!r}, {{'__package__': '{package}.branch'}}, "
        f"None, {fromlist!r}, {level})\nresult.chosen()\n"
    )
    paths["entry"] = write(tmp_path / "entry.py", body)
    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        namespace = {"take": lambda intrinsic: __import__}
        try:
            exec(body.split("\n", 1)[1] if route == "intrinsic" else body, namespace)
            assert namespace["result"].__name__ == modules[returned]
            assert modules[imported] in sys.modules
            assert {marker for marker in markers.values() if marker in sys.modules} == {
                markers[returned]
            }
        finally:
            for module in (*modules.values(), *markers.values()):
                sys.modules.pop(module, None)
    admitted = closed_source_imports(paths)
    assert modules[imported] in admitted
    assert admitted.intersection(markers.values()) == {markers[returned]}


@pytest.mark.parametrize("intrinsic", [False, True])
def test_finder_result_does_not_publish_the_leaf_module_namespace(
    tmp_path, monkeypatch, intrinsic
):
    import importlib.util

    package = "_molt_finder_result_package"
    marker = "_molt_finder_result_callback"
    paths = {
        package: write(tmp_path / package / "__init__.py"),
        f"{package}.leaf": write(
            tmp_path / package / "leaf.py", f"def chosen(): import {marker}\n"
        ),
        marker: write(tmp_path / f"{marker}.py"),
    }
    source = (
        "from _intrinsics import require_intrinsic as take\n"
        "def factory(): return take('molt_importlib_find_spec')\n"
        f"result = factory()('{package}.leaf', None, None)\n"
        if intrinsic
        else "import importlib.util\ndef factory(): return importlib.util.find_spec\n"
        f"result = factory()('{package}.leaf')\n"
    ) + "result.chosen()\n"
    paths["entry"] = write(tmp_path / "entry.py", source)
    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        try:
            spec = importlib.util.find_spec(f"{package}.leaf")
            assert spec is not None
            with pytest.raises(AttributeError):
                spec.chosen()
            assert marker not in sys.modules
        finally:
            sys.modules.pop(package, None)
            sys.modules.pop(f"{package}.leaf", None)
    assert marker not in closed_source_imports(paths)


@pytest.mark.parametrize("invoke", [False, True])
def test_import_fallback_result_preserves_the_supplied_callable(
    tmp_path, monkeypatch, invoke
):
    marker = "_molt_import_fallback_callable"
    body = (
        f"def fallback(): import {marker}\n"
        "loader = take('molt_importlib_import_or_fallback')\n"
        "result = loader('_molt_missing_import_for_fallback', fallback)\n"
        + ("result()\n" if invoke else "")
    )
    entry = write(
        tmp_path / "entry.py",
        "from _intrinsics import require_intrinsic as take\n" + body,
    )
    marker_path = write(tmp_path / f"{marker}.py")
    # A missing import returns exactly the supplied object, not a module-shaped
    # proxy. The worklist consumes that object's own existing source origins.
    import importlib

    def fallback_import(module_name, fallback):
        try:
            return importlib.import_module(module_name)
        except ImportError:
            return fallback

    namespace = {"take": lambda name: fallback_import}
    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        try:
            exec(body, namespace)
            assert namespace["result"] is namespace["fallback"]
            assert (marker in sys.modules) is invoke
        finally:
            sys.modules.pop(marker, None)
    assert (
        marker in closed_source_imports({"entry": entry, marker: marker_path})
    ) is invoke


@pytest.mark.parametrize(
    "intrinsic,arguments",
    [
        ("molt_module_import", "'{module}'"),
        ("molt_importlib_import_module", "'{module}', None"),
        ("molt_importlib_import_optional", "'{module}'"),
        ("molt_importlib_import_required", "'{module}'"),
        ("molt_importlib_import_or_fallback", "'{module}', None"),
    ],
)
def test_native_leaf_importers_return_requested_leaf_with_positional_abi(
    tmp_path, monkeypatch, intrinsic, arguments
):
    import importlib

    package = "_molt_native_leaf_package"
    root_marker, leaf_marker = "_molt_native_root_marker", "_molt_native_leaf_marker"
    paths = {
        package: write(
            tmp_path / package / "__init__.py", f"def chosen(): import {root_marker}\n"
        ),
        f"{package}.leaf": write(
            tmp_path / package / "leaf.py", f"def chosen(): import {leaf_marker}\n"
        ),
        root_marker: write(tmp_path / f"{root_marker}.py"),
        leaf_marker: write(tmp_path / f"{leaf_marker}.py"),
    }
    body = (
        "from _intrinsics import require_intrinsic as take\n"
        f"loader = take({intrinsic!r})\n"
        f"result = loader({arguments.format(module=f'{package}.leaf')})\n"
        "result.chosen()\n"
    )
    paths["entry"] = write(tmp_path / "entry.py", body)
    with monkeypatch.context() as runtime:
        runtime.syspath_prepend(str(tmp_path))
        try:
            leaf = importlib.import_module(f"{package}.leaf")
            leaf.chosen()
            assert leaf.__name__ == f"{package}.leaf"
            assert leaf_marker in sys.modules and root_marker not in sys.modules
        finally:
            for name in paths:
                sys.modules.pop(name, None)
    assert closed_source_imports(paths).intersection({root_marker, leaf_marker}) == {
        leaf_marker
    }


@pytest.mark.parametrize("defect", ["retired-row", "finder-module", "finder-fallback"])
def test_conditional_import_cache_rejects_incoherent_return_projections(
    tmp_path, defect
):
    entry = write(
        tmp_path / "entry.py",
        "from _intrinsics import require_intrinsic as take\n"
        "take('molt_importlib_find_spec')('leaf', None, None)\n",
    )
    cold = load(entry)
    assert load(entry).cache_hit
    key = records._import_scan_cache_key(
        entry, module_name="entry", is_package=False, import_scan_mode="full"
    )
    payload = source_records("import-scan")[key]
    rows = payload["source_values"]["call_imports"]
    assert rows
    if defect == "retired-row":
        rows[0] = rows[0][:2]
    elif defect == "finder-module":
        rows[0][2] = ["leaf"]
    else:
        rows[0][4] = [rows[0][1]["source"]]
    replace_source_record("import-scan", key, payload)
    repaired = load(entry)
    assert not repaired.cache_hit
    assert repaired.scan.source_values == cold.scan.source_values


@pytest.fixture
def module_cache_fact_record(tmp_path):
    from molt.compiler_analysis.python_imports import (
        _ImportScanRequests,
        _SourceValueSummary,
    )
    from molt.compiler_analysis.python_namespace_writes import python_namespace_summary
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = write(
        tmp_path / "cache_owner.py",
        "import sys\ndef provider():\n    return 1\n"
        "sys.modules['alias'] = provider\n"
        "loaded = sys.modules.get('alias', provider)\n",
    )
    snapshot = PythonSourceSnapshot.capture(source)
    tree = ast.parse(snapshot.content)
    provider = python_node_source_key(tree.body[1])
    write_source = python_node_source_key(tree.body[2].targets[0])
    read_source = python_node_source_key(tree.body[3].value)
    options = dict(
        module_name="cache_owner",
        is_package=False,
        import_scan_mode="full",
        target_sys_platform="linux",
    )

    def publish(keys=("alias", "other"), contained=False):
        values = _SourceValueSummary(
            cache_writes=(
                (write_source, keys, frozenset({provider}), frozenset({provider})),
            ),
            cache_reads=(
                (
                    read_source,
                    keys,
                    frozenset({provider}),
                    frozenset({provider}),
                    contained,
                ),
            ),
        )
        request = _ImportScanRequests(
            (),
            (),
            source_values=values,
            namespace_summary=python_namespace_summary(tree),
        )
        records._write_persisted_import_scan(
            source, **options, scan=request, snapshot=snapshot
        )
        key = records._import_scan_cache_key(source, **options)
        return source, snapshot, options, key, request

    return publish


@pytest.mark.parametrize("keys", [None, ("",), ("alias", "other")])
@pytest.mark.parametrize("contained", [False, True])
def test_module_cache_source_values_roundtrip_canonical_storage_facts(
    module_cache_fact_record, keys, contained
):
    source, snapshot, options, key, request = module_cache_fact_record(keys, contained)
    payload = source_records("import-scan")[key]
    assert payload["target_sys_platform"] == "linux"
    raw = payload["source_values"]
    assert raw["cache_writes"][0][1] == (None if keys is None else list(keys))
    assert raw["cache_reads"][0][-1] is contained
    assert (
        records._read_persisted_import_scan_record(source, **options, snapshot=snapshot)
        == request
    )
    retired = copy.deepcopy(payload)
    retired["version"] = 36
    replace_source_record("import-scan", key, retired)
    assert (
        records._read_persisted_import_scan_record(source, **options, snapshot=snapshot)
        is None
    )
    replace_source_record("import-scan", key, payload)
    # Equal reads are source-only: the target generation and source-byte
    # admission remain the existing enclosing cache authority.
    assert (
        records._read_persisted_import_scan_record(
            source, **{**options, "target_sys_platform": "win32"}, snapshot=snapshot
        )
        is None
    )
    source.write_text("CHANGED = True\n", encoding="utf-8")
    assert records._read_persisted_import_scan_record(source, **options) is None


@pytest.mark.parametrize("field", ["cache_writes", "cache_reads"])
@pytest.mark.parametrize(
    "defect",
    [
        "missing",
        "container",
        "row-width",
        "source",
        "keys-container",
        "keys-empty",
        "keys-order",
        "keys-duplicate",
        "keys-type",
        "origins-container",
        "origin-source",
        "contained-origins-container",
        "contained-origin-source",
        "retired-width",
    ],
)
def test_module_cache_source_values_reject_malformed_storage_facts(
    module_cache_fact_record, field, defect
):
    source, snapshot, options, key, _request = module_cache_fact_record()
    payload = source_records("import-scan")[key]
    values = payload["source_values"]
    row = values[field][0]
    if defect == "missing":
        del values[field]
    elif defect == "container":
        values[field] = {}
    elif defect == "row-width":
        row.append(None)
    elif defect == "source":
        row[0] = [False, 0, 1, 0, "Subscript"]
    elif defect == "keys-container":
        row[1] = "alias"
    elif defect == "keys-empty":
        row[1] = []
    elif defect == "keys-order":
        row[1] = ["z", "a"]
    elif defect == "keys-duplicate":
        row[1] = ["alias", "alias"]
    elif defect == "keys-type":
        row[1] = [True]
    elif defect == "origins-container":
        row[2] = {}
    elif defect == "origin-source":
        row[2] = [[-1, 0, 1, 0, "FunctionDef"]]
    elif defect == "contained-origins-container":
        row[3] = {}
    elif defect == "contained-origin-source":
        row[3] = [[-1, 0, 1, 0, "FunctionDef"]]
    else:
        row.pop()
    replace_source_record("import-scan", key, payload)
    assert (
        records._read_persisted_import_scan_record(source, **options, snapshot=snapshot)
        is None
    )


@pytest.mark.parametrize("contained", [None, 0, 1, "false"])
def test_module_cache_read_containment_is_a_closed_boolean(
    module_cache_fact_record, contained
):
    source, snapshot, options, key, _request = module_cache_fact_record()
    payload = source_records("import-scan")[key]
    payload["source_values"]["cache_reads"][0][-1] = contained
    replace_source_record("import-scan", key, payload)
    assert (
        records._read_persisted_import_scan_record(source, **options, snapshot=snapshot)
        is None
    )


@pytest.mark.parametrize(
    "constructor,protocol",
    [("classmethod", "class_method"), ("staticmethod", "static_method")],
)
def test_retained_descriptor_source_element_survives_cold_and_warm_scan(
    tmp_path, constructor, protocol
):
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    source = write(
        tmp_path / "entry.py",
        f"class Box:\n    @{constructor}\n    def method(cls):\n        import dormant_leaf\n",
    )
    premise = PythonNamespacePremise(
        (),
        0,
        entry_member_writes=0,
        noncanonical_imports=(),
        opaque_activation_sources=(),
    )
    cold = load(source, namespace_premise=premise)
    assert any(
        item.protocol.value == protocol for item in cold.scan.source_values.elements
    )
    warm = load(source, namespace_premise=premise)
    assert warm.cache_hit
    assert warm.scan.source_values == cold.scan.source_values


_CLASSED_HOOK = (
    "import importlib\n"
    "def __getattr__(name):\n"
    "    if name == 'tasks':\n        import tasks_leaf as mod\n"
    "    elif name in ('futures', 'events'):\n"
    "        mod = importlib.import_module(name + '_leaf')\n"
    "    else:\n"
    "        raise AttributeError(f'module has no attribute {name!r}')\n"
    "    globals()[name] = mod\n"
    "    return importlib.import_module(name)\n"
)
_UNCLASSED_HOOK = (
    "import importlib\n"
    "def __getattr__(name):\n"
    "    name += '_impl'\n"
    "    if name == 'log_impl':\n        return importlib.import_module(name)\n"
    "    raise AttributeError(f'{name}')\n"
)

# A positive test without a terminal else hides nothing: every argument reaches
# the import below it, so no class may collapse them.
_UNTERMINATED_HOOK = (
    "import importlib\n"
    "def __getattr__(name):\n"
    "    if name == 'x':\n        pass\n"
    "    return importlib.import_module(name)\n"
)


@pytest.mark.parametrize(
    "text, literal_class, outside, inside",
    [
        (
            _CLASSED_HOOK,
            frozenset({"tasks", "futures", "events"}),
            ("alpha", "beta", "tasks_"),
            ("tasks", "events"),
        ),
        (_UNCLASSED_HOOK, None, ("alpha",), ("log",)),
        (_UNTERMINATED_HOOK, None, ("alpha",), ("log",)),
    ],
    ids=["classed", "augmented", "unterminated"],
)
def test_argument_class_representative_scans_like_every_member(
    tmp_path, text, literal_class, outside, inside
):
    """Oracle for python_parameter_classes: the whole body scan of the class
    representative equals the exact scan of every argument outside the
    literal set. A refused parameter shows why: its exact scans differ."""
    from molt.compiler_analysis.python_parameter_classes import UNMATCHED_ARGUMENT
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = write(tmp_path / "hooked.py", text)
    hook = python_node_source_key(ast.parse(text).body[-1])
    (parameters,) = [
        item
        for item in load(
            source,
            source_owner=hook,
            import_scan_mode="module_init",
            namespace_premise=WHOLE_PROGRAM,
        ).scan.source_values.activation_partitions
        if item.owner == hook
    ]
    assert parameters.classes == (literal_class,)

    def record(argument: str):
        return load(
            source,
            import_scan_mode="module_init",
            source_owner=hook,
            source_arguments=((0, argument),),
            namespace_premise=WHOLE_PROGRAM,
        )

    def scan(argument: str):
        return record(argument).scan

    representative = scan(UNMATCHED_ARGUMENT)
    # The sentinel is not an identifier; persisted records keep it exactly.
    warm = record(UNMATCHED_ARGUMENT)
    assert warm.cache_hit and warm.scan == representative
    exact = {argument: scan(argument) for argument in (*outside, *inside)}
    if literal_class is None:
        assert exact["log"] != representative
        return
    assert all(exact[argument] == representative for argument in outside)
    for argument in inside:
        assert exact[argument] != representative
        assert any(argument in name for name in exact[argument].imports)


def test_member_reads_share_one_hook_scan_per_argument_class(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Four named reads of a PEP 562 module cost two body scans: one for the
    literal read and one for every other name (exact arguments cost four).
    The argument-free scan is the hook's unknown-name activation, present
    either way. The closure keeps the literal branch's import only."""
    from molt.compiler_analysis.python_parameter_classes import UNMATCHED_ARGUMENT
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    paths = {
        "hooked": write(tmp_path / "hooked.py", _CLASSED_HOOK),
        "entry": write(
            tmp_path / "entry.py",
            "import hooked\nprint(hooked.alpha, hooked.beta, hooked.gamma, hooked.tasks)\n",
        ),
        "tasks_leaf": write(tmp_path / "tasks_leaf.py"),
        "futures_leaf": write(tmp_path / "futures_leaf.py"),
    }
    hook = python_node_source_key(ast.parse(_CLASSED_HOOK).body[-1])
    scanned = []
    imports = closed_source_imports(paths, scanned=scanned)
    hook_jobs = sorted(
        arguments for module, owner, arguments in scanned if owner == hook
    )
    assert hook_jobs == [(), ((0, UNMATCHED_ARGUMENT),), ((0, "tasks"),)]
    assert "tasks_leaf" in imports
    assert "futures_leaf" not in imports
    body_loads = set()
    original = discovery._load_module_import_scan

    def observe(path: Path, **kwargs: object) -> discovery._LoadedModuleImportScan:
        if kwargs["module_name"] == "hooked" and kwargs.get("source_owner") == hook:
            body_loads.add(cast(SourceStringArguments, kwargs["source_arguments"]))
        load_scan = cast(Callable[..., discovery._LoadedModuleImportScan], original)
        return load_scan(path, **kwargs)

    monkeypatch.setattr(discovery, "_load_module_import_scan", observe)
    graph = discovery._discover_module_graph(
        paths["entry"], [tmp_path], [tmp_path], tmp_path / "stdlib", tmp_path, set()
    )
    assert set(body_loads) == set(hook_jobs)
    assert graph.graph["tasks_leaf"] == paths["tasks_leaf"]
    assert "futures_leaf" not in graph.graph


def test_changed_literal_class_remaps_collapsed_activations(tmp_path):
    """A rescan under a new premise can shrink a literal class (a raised
    builtin loses its proof). Activations collapsed under the old class must
    then scan exactly; the solver keeps each original argument tuple."""
    from molt.compiler_analysis.python_imports import _ImportScanRequests
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )
    from molt.compiler_analysis.python_parameter_classes import UNMATCHED_ARGUMENT
    from molt.compiler_analysis.python_source_demands import PythonSourceDemands
    from molt.compiler_analysis.python_source_keys import (
        _PythonAstDigestAdmission,
        python_node_source_key,
    )

    text = "def hook(name):\n    if name == 'a':\n        return 1\n    raise AttributeError(f'{name}')\n"
    path = write(tmp_path / "hooked.py", text)
    tree = ast.parse(text)
    hook = python_node_source_key(tree.body[0])
    writes = graph_python_namespace_writes((), {"hooked": path}.get)
    projection = _source_import_scan._collect_imports_for_graph(
        tree,
        "hooked",
        source_owner=hook,
        import_scan_mode="module_init",
        namespace_premise=writes.premise(path, frozenset()),
        ast_digest_admission=_PythonAstDigestAdmission.for_tree(tree),
    )
    scan = _ImportScanRequests((), ())._replace(**projection._asdict())
    (parameters,) = scan.source_values.activation_partitions
    assert parameters.classes == (frozenset({"a"}),)
    bodies = []
    engine = PythonSourceDemands(
        require_runtime_import_protocol=lambda _module: None,
        admit_imports=lambda *_: None,
        queue_body=lambda *job: bodies.append(job),
    )
    engine.accept("hooked", scan)
    for argument in ("x", "y", "a"):
        engine.demand(("hooked", hook), "call", ((0, argument),))
    engine.drain()
    assert sorted(bodies) == [
        ("hooked", hook, ((0, UNMATCHED_ARGUMENT),)),
        ("hooked", hook, ((0, "a"),)),
    ]
    shrunk = scan._replace(
        source_values=scan.source_values._replace(
            activation_partitions=(parameters._replace(classes=(None,)),)
        )
    )
    engine.accept("hooked", shrunk)
    engine.drain()
    assert sorted(arguments for _, _, arguments in bodies) == [
        ((0, UNMATCHED_ARGUMENT),),
        ((0, "a"),),
        ((0, "x"),),
        ((0, "y"),),
    ]

    # Another context can prove its partition without erasing the context
    # that requires exact arguments. A new argument still gets its own body.
    engine.accept("hooked", scan)
    engine.demand(("hooked", hook), "call", ((0, "z"),))
    engine.drain()
    assert bodies[-1] == ("hooked", hook, ((0, "z"),))


@pytest.mark.parametrize("corruption", [None, "length", "unsorted", "type"])
def test_persisted_literal_classes_round_trip_or_regenerate(tmp_path, corruption):
    source = write(tmp_path / "hooked.py", _CLASSED_HOOK)
    owner = _source_import_scan.python_node_source_key(
        ast.parse(_CLASSED_HOOK).body[-1]
    )
    selection = dict(import_scan_mode="module_init", source_owner=owner)
    cold = load(source, **selection)
    classes = [item.classes for item in cold.scan.source_values.activation_partitions]
    assert (frozenset({"tasks", "futures", "events"}),) in classes
    if corruption is not None:
        key = records._import_scan_cache_key(
            source, module_name="entry", is_package=False, **selection
        )
        payload = source_records("import-scan")[key]
        row = next(
            row
            for row in payload["source_values"]["activation_partitions"]
            if row[1] and row[1] != [None]
        )
        row[1] = {
            "length": row[1] + [None],
            "unsorted": [sorted(row[1][0], reverse=True)],
            "type": [[1]],
        }[corruption]
        replace_source_record("import-scan", key, payload)
    warm = load(source, **selection)
    assert warm.cache_hit is (corruption is None)
    assert warm.scan.source_values == cold.scan.source_values


@pytest.mark.parametrize("opaque", [False, True])
@pytest.mark.parametrize("closed", [False, True])
def test_joined_activation_uses_existing_finite_cache_write_projection(
    monkeypatch: pytest.MonkeyPatch, opaque: bool, closed: bool
) -> None:
    from dataclasses import replace
    from molt.compiler_analysis.python_binding_facts import PythonStringAlternatives
    from molt.compiler_analysis.python_binding_flow import (
        analyze_python_source_bindings,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = (
        "import sys\n"
        "def install(name, create):\n"
        "    alias = name\n"
        "    mod = create(name)\n"
        "    sys.modules[alias] = mod\n"
    )
    tree = ast.parse(source)
    owner = tree.body[1]
    assert isinstance(owner, ast.FunctionDef)
    domain = PythonStringAlternatives(
        frozenset({"asyncio.base_events", "asyncio.tasks"})
    )
    policy = _source_import_scan._source_binding_policy(
        target_python=TargetPythonVersion(3, 15, 0),
        module_name="sample",
        namespace_premise=replace(
            WHOLE_PROGRAM,
            opaque_activation_sources=() if closed else None,
            code_rebinding=not closed,
        ),
        activation_argument_values=(
            (python_node_source_key(owner.args.args[0]), domain),
        ),
    )
    if opaque:
        policy = replace(policy, activation_argument_values=())
    bindings = analyze_python_source_bindings(source, policy=policy)
    # This prerequisite does not infer caller coverage. Supply the admitted
    # binding result at the scanner boundary to exercise its real key consumer.
    monkeypatch.setattr(
        _source_import_scan,
        "analyze_python_bindings",
        lambda *_args, **_kwargs: bindings,
    )
    scan = _source_import_scan._collect_imports_for_graph(
        tree,
        "sample",
        source_owner=python_node_source_key(owner),
        import_scan_mode="module_init",
        namespace_premise=replace(
            WHOLE_PROGRAM,
            opaque_activation_sources=() if closed else None,
            code_rebinding=not closed,
        ),
        target_python=TargetPythonVersion(3, 15, 0),
    )
    if closed:
        assert len(scan.source_values.cache_writes) == 1
        assert scan.source_values.cache_writes[0][1] == (
            None if opaque else tuple(sorted(domain.values))
        )
    else:
        # Finite local names cannot make an escaped activation's receiver exact.
        assert scan.source_values.cache_writes == ()
    # The registered name still participates; the joined argument grants no
    # exact constructor result or fresh-module substitution to either branch.
    creation = owner.body[1].value
    result = bindings.expression_result(creation)
    assert result.kind == "unknown" and not result.fresh_container


@pytest.mark.parametrize(
    "route", ["literal", "unknown", "escape", "empty", "keyword", "starred"]
)
def test_closed_local_literal_callers_remove_only_the_impossible_import(
    monkeypatch: pytest.MonkeyPatch, route: str
) -> None:
    import builtins
    from dataclasses import replace
    from types import ModuleType
    from molt.compiler_analysis import python_binding_flow as flow
    from molt.compiler_analysis.python_source_keys import (
        _PythonAstDigestAdmission,
        python_node_source_key,
    )

    calls = {
        "literal": "    helper('left')\n    helper('right')\n",
        "unknown": "    helper('left')\n    helper(value)\n",
        "escape": "    helper('left')\n    return helper\n",
        "empty": "    helper('left')\n    helper('')\n",
        "keyword": "    helper(name='left')\n    helper(name='right')\n",
        "starred": "    helper(*('left',))\n    helper('right')\n",
    }
    source = (
        "def outer(value=None):\n"
        "    def helper(name):\n"
        "        if name:\n            import chosen_leaf\n"
        "        else:\n            import fallback_leaf\n"
        + calls[route]
        + ("escaped = outer()\nescaped('')\n" if route == "escape" else "outer()\n")
    )
    tree = ast.parse(source)
    owner = tree.body[0].body[0]
    assert isinstance(owner, ast.FunctionDef)
    # This fixture is the complete closed program, with no external callback or
    # reflection. Both legs use the same admitted source and namespace custody.
    premise = replace(WHOLE_PROGRAM, opaque_activation_sources=())
    admission = _PythonAstDigestAdmission.for_tree(tree)

    def scan() -> tuple[str, ...]:
        return _source_import_scan._collect_imports_for_graph(
            tree,
            "entry",
            source_owner=python_node_source_key(owner),
            import_scan_mode="module_init",
            namespace_premise=premise,
            ast_digest_admission=admission,
        ).imports

    # Isolate the existing core cache so each leg executes its real source scan.
    with monkeypatch.context() as baseline:
        baseline.setattr(flow, "_CORE_CACHE", flow._BindingCache())
        baseline.setattr(flow._Analyzer, "_local_literal_actuals", lambda *_args: {})
        before = scan()
    monkeypatch.setattr(flow, "_CORE_CACHE", flow._BindingCache())
    after = scan()
    assert set(before) == {"chosen_leaf", "fallback_leaf"}
    assert set(after) == ({"chosen_leaf"} if route == "literal" else set(before))
    observed: list[str] = []
    with monkeypatch.context() as runtime:
        runtime.setattr(
            builtins,
            "__import__",
            lambda name, *_args, **_kwargs: (observed.append(name), ModuleType(name))[
                1
            ],
        )
        exec(compile(source, "<local-call-import-oracle>", "exec"), {})
    assert set(observed) <= set(after)
