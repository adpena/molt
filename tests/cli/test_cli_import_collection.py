from __future__ import annotations

import ast
import json
import os
from pathlib import Path
import subprocess
import sys

import molt.cli as cli
import pytest


ROOT = Path(__file__).resolve().parents[2]


def test_resolve_module_path_prefers_package_over_module(tmp_path: Path) -> None:
    module = tmp_path / "shadowed.py"
    module.write_text("value = 'module'\n")
    package_dir = tmp_path / "shadowed"
    package_dir.mkdir()
    package_init = package_dir / "__init__.py"
    package_init.write_text("value = 'package'\n")
    assert cli._resolve_module_path("shadowed", [tmp_path]) == package_init


def test_stdlib_test_support_layout_resolves_like_cpython() -> None:
    stdlib_root = cli._stdlib_root_path()
    support_pkg = cli._resolve_module_path("test.support", [stdlib_root])
    import_helper = cli._resolve_module_path(
        "test.support.import_helper", [stdlib_root]
    )
    os_helper = cli._resolve_module_path("test.support.os_helper", [stdlib_root])
    warnings_helper = cli._resolve_module_path(
        "test.support.warnings_helper", [stdlib_root]
    )

    assert support_pkg == stdlib_root / "test" / "support" / "__init__.py"
    assert import_helper == stdlib_root / "test" / "support" / "import_helper.py"
    assert os_helper == stdlib_root / "test" / "support" / "os_helper.py"
    assert warnings_helper == stdlib_root / "test" / "support" / "warnings_helper.py"


def test_write_importer_module_uses_constant_time_membership(tmp_path: Path) -> None:
    importer = cli._write_importer_module(
        ["pkg.alpha", "pkg.beta", "solo"],
        tmp_path,
    )
    text = importer.read_text()
    assert "_KNOWN_MODULES = frozenset(" in text
    assert "_TOP_LEVEL_BY_MODULE = {'pkg.alpha': 'pkg', 'pkg.beta': 'pkg'}" in text
    assert "_TOP_LEVEL_BY_MODULE.get(resolved, resolved)" in text


def test_find_project_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._find_project_root_cached.cache_clear()
    start = tmp_path / "a" / "b" / "c.py"
    calls = 0

    def fake_has_project_markers(path: Path) -> bool:
        nonlocal calls
        calls += 1
        return path == tmp_path

    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)
    monkeypatch.setattr(cli, "_has_project_markers", fake_has_project_markers)
    first = cli._find_project_root(start)
    first_calls = calls
    second = cli._find_project_root(start)
    assert first == tmp_path
    assert second == first
    assert calls == first_calls
    cli._find_project_root_cached.cache_clear()


def test_find_molt_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._find_molt_root_cached.cache_clear()
    candidate = tmp_path / "repo" / "src"
    repo_root = tmp_path / "repo"
    calls = 0

    def fake_has_molt_repo_markers(path: Path) -> bool:
        nonlocal calls
        calls += 1
        return path == repo_root

    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)
    monkeypatch.setattr(cli, "_has_molt_repo_markers", fake_has_molt_repo_markers)
    first = cli._find_molt_root(candidate)
    first_calls = calls
    second = cli._find_molt_root(candidate)
    assert first == repo_root
    assert second == first
    assert calls == first_calls
    cli._find_molt_root_cached.cache_clear()


def test_stdlib_allowlist_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._stdlib_allowlist_cached.cache_clear()
    spec_path = (
        tmp_path
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
    )
    spec_path.parent.mkdir(parents=True, exist_ok=True)
    spec_path.write_text("| Module |\n| --- |\n| json / pathlib |\n")
    calls = 0
    original_read_text = Path.read_text
    expected_spec_path = spec_path.resolve()

    def wrapped(self: Path, *args: object, **kwargs: object) -> str:
        nonlocal calls
        if self.resolve() == expected_spec_path:
            calls += 1
        return original_read_text(self, *args, **kwargs)

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(tmp_path))
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(Path, "read_text", wrapped)
    first = cli._stdlib_allowlist()
    second = cli._stdlib_allowlist()
    assert {"json", "pathlib"} <= first
    assert second == first
    assert calls == 1
    cli._stdlib_allowlist_cached.cache_clear()


def _discover_with_core_modules(entry: Path) -> dict[str, Path]:
    stdlib_root = cli._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    module_graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )
    cli._collect_package_parents(module_graph, roots, stdlib_root, stdlib_allowlist)
    cli._ensure_core_stdlib_modules(module_graph, stdlib_root)
    core_paths = [
        path
        for name in (
            "builtins",
            "sys",
            "types",
            "importlib",
            "importlib.util",
            "importlib.machinery",
        )
        if (path := module_graph.get(name)) is not None
    ]
    for core_path in core_paths:
        core_graph, _ = cli._discover_module_graph(
            core_path,
            roots,
            module_roots,
            stdlib_root,
            stdlib_allowlist,
            skip_modules=cli.STUB_MODULES,
            stub_parents=cli.STUB_PARENT_MODULES,
            nested_stdlib_scan_modules=set(),
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    return module_graph


def test_collect_imports_can_skip_nested_imports() -> None:
    tree = ast.parse(
        "import os\ndef f() -> None:\n    import warnings\nclass C:\n    import re\n"
    )
    nested = cli._collect_imports(tree)
    top_level_only = cli._collect_imports(tree, include_nested=False)
    assert "warnings" in nested
    assert "re" in nested
    assert "warnings" not in top_level_only
    assert "re" not in top_level_only
    assert "os" in top_level_only


def test_collect_imports_resolves_module_constant_via_helper_call() -> None:
    tree = ast.parse(
        "import importlib\n"
        "MODULE_NAME = '_socket'\n"
        "def _probe(module_name):\n"
        "    return importlib.import_module(module_name)\n"
        "_probe(MODULE_NAME)\n"
    )
    imports = cli._collect_imports(tree)
    assert "_socket" in imports


def test_collect_imports_resolves_helper_call_nested_in_expression() -> None:
    tree = ast.parse(
        "import importlib\n"
        "MODULE_NAME = '_socket'\n"
        "def _probe(module_name):\n"
        "    return importlib.import_module(module_name)\n"
        "print(_probe(MODULE_NAME))\n"
    )
    imports = cli._collect_imports(tree)
    assert "_socket" in imports


def test_collect_imports_resolves_name_argument_for_import_module() -> None:
    tree = ast.parse(
        "import importlib\nTARGET = 'pathlib'\nimportlib.import_module(TARGET)\n"
    )
    imports = cli._collect_imports(tree)
    assert "pathlib" in imports


def test_collect_imports_resolves_helper_join_dynamic_module_name() -> None:
    tree = ast.parse(
        "import importlib\n"
        "def _module_name(parts):\n"
        "    return ''.join(parts)\n"
        "def _load(parts):\n"
        "    return importlib.import_module(_module_name(parts))\n"
        "_load(('ma', 'th'))\n"
        "_load(('sy', 's'))\n"
    )
    imports = cli._collect_imports(tree)
    assert "math" in imports
    assert "sys" in imports


def test_backend_ir_text_is_compact() -> None:
    text = cli._backend_ir_text(
        {
            "functions": [{"name": "main", "ops": [{"kind": "ret", "args": []}]}],
            "profile": {"hash": "abc"},
        }
    )
    assert "\n" not in text
    assert ": " not in text
    assert '"functions"' in text


def test_cache_payloads_for_ir_share_sorted_function_order() -> None:
    ir = {
        "functions": [
            {"name": "zeta", "ops": []},
            {"name": "alpha", "ops": []},
        ],
        "profile": {"hash": "abc"},
        "runtime_feedback": {"hot_functions": ["alpha"]},
    }
    module_payload, backend_payload = cli._cache_payloads_for_ir(ir)
    module_text = module_payload.decode("utf-8")
    backend_text = backend_payload.decode("utf-8")
    assert module_text.index('"name":"alpha"') < module_text.index('"name":"zeta"')
    assert backend_text.index('"name":"alpha"') < backend_text.index('"name":"zeta"')
    assert '"top_level_extras_digest"' in backend_text


def test_shared_module_resolution_cache_reduces_repeated_resolution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    pkg = tmp_path / "pkg"
    subpkg = pkg / "subpkg"
    subpkg.mkdir(parents=True)
    (pkg / "__init__.py").write_text("from .subpkg import mod\n")
    (subpkg / "__init__.py").write_text("from . import mod\n")
    entry = subpkg / "mod.py"
    entry.write_text("import pkg.subpkg.helper\n")
    helper = subpkg / "helper.py"
    helper.write_text("VALUE = 1\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()

    resolve_calls = 0
    original = cli._resolve_module_path_parts

    def wrapped(parts: tuple[str, ...], roots_arg: list[Path]) -> Path | None:
        nonlocal resolve_calls
        resolve_calls += 1
        return original(parts, roots_arg)

    monkeypatch.setattr(cli, "_resolve_module_path_parts", wrapped)

    shared_cache = cli._ModuleResolutionCache()
    cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        resolver_cache=shared_cache,
    )
    shared_first = resolve_calls
    cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        resolver_cache=shared_cache,
    )
    shared_second = resolve_calls - shared_first

    resolve_calls = 0
    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
    )
    unshared_first = resolve_calls
    cli._collect_package_parents(graph, roots, stdlib_root, stdlib_allowlist)
    cli._collect_namespace_parents(
        graph,
        roots,
        stdlib_root,
        stdlib_allowlist,
        explicit_imports,
    )
    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
    )
    cli._collect_package_parents(graph, roots, stdlib_root, stdlib_allowlist)
    cli._collect_namespace_parents(
        graph,
        roots,
        stdlib_root,
        stdlib_allowlist,
        explicit_imports,
    )
    unshared_second = resolve_calls - unshared_first

    assert shared_second == 0
    assert unshared_second > 0


def test_shared_module_resolution_cache_reuses_source_and_ast_across_passes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("from . import helper\n")
    entry = pkg / "helper.py"
    entry.write_text("VALUE = 1\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()

    read_calls = 0
    parse_calls = 0
    original_read = cli._read_module_source
    original_parse = cli.ast.parse

    def wrapped_read(path: Path) -> str:
        nonlocal read_calls
        read_calls += 1
        return original_read(path)

    def wrapped_parse(
        source: str,
        filename: str = "<unknown>",
        mode: str = "exec",
        *,
        type_comments: bool = False,
        feature_version: int | None = None,
    ) -> ast.AST:
        nonlocal parse_calls
        parse_calls += 1
        return original_parse(
            source,
            filename=filename,
            mode=mode,
            type_comments=type_comments,
            feature_version=feature_version,
        )

    monkeypatch.setattr(cli, "_read_module_source", wrapped_read)
    monkeypatch.setattr(cli.ast, "parse", wrapped_parse)

    shared_cache = cli._ModuleResolutionCache()
    graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        resolver_cache=shared_cache,
    )
    first_read_calls = read_calls
    first_parse_calls = parse_calls
    for module_path in graph.values():
        source = shared_cache.read_module_source(module_path)
        shared_cache.parse_module_ast(
            module_path, source, filename=str(module_path)
        )
    assert read_calls == first_read_calls
    assert parse_calls == first_parse_calls

    read_calls = 0
    parse_calls = 0
    unshared_graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
    )
    for module_path in unshared_graph.values():
        source = cli._read_module_source(module_path)
        cli.ast.parse(source, filename=str(module_path))
    assert read_calls > 0
    assert parse_calls > 0


def test_shared_module_resolution_cache_reuses_resolved_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "pkg" / "__init__.py"
    entry.parent.mkdir()
    entry.write_text("VALUE = 1\n")
    stdlib_root = cli._stdlib_root_path()

    resolve_calls = 0
    original_resolve = Path.resolve

    def wrapped_resolve(self: Path, *args: object, **kwargs: object) -> Path:
        nonlocal resolve_calls
        resolve_calls += 1
        return original_resolve(self, *args, **kwargs)

    monkeypatch.setattr(Path, "resolve", wrapped_resolve)

    cache = cli._ModuleResolutionCache()
    module_roots = [tmp_path]
    first_name = cache.module_name_from_path(entry, module_roots, stdlib_root)
    first_is_stdlib = cache.is_stdlib_path(entry, stdlib_root)
    first_resolve_calls = resolve_calls

    second_name = cache.module_name_from_path(entry, module_roots, stdlib_root)
    second_is_stdlib = cache.is_stdlib_path(entry, stdlib_root)

    assert first_name == "pkg"
    assert second_name == first_name
    assert not first_is_stdlib
    assert second_is_stdlib is first_is_stdlib
    assert resolve_calls == first_resolve_calls


def test_shared_module_resolution_cache_skips_resolve_for_normalized_absolute_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache = cli._ModuleResolutionCache()
    path = (tmp_path / "module.py").resolve()

    def fail_resolve(self: Path, *args: object, **kwargs: object) -> Path:
        raise AssertionError(f"resolve() should not run for {self}")

    monkeypatch.setattr(Path, "resolve", fail_resolve)
    assert cache.resolved_path(path) == path


def test_shared_module_resolution_cache_resolves_relative_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache = cli._ModuleResolutionCache()
    rel_path = Path("pkg") / "module.py"
    resolved_path = (tmp_path / rel_path).resolve()
    calls = 0
    original_resolve = Path.resolve

    def wrapped_resolve(self: Path, *args: object, **kwargs: object) -> Path:
        nonlocal calls
        calls += 1
        if self == rel_path:
            return resolved_path
        return original_resolve(self, *args, **kwargs)

    monkeypatch.setattr(Path, "resolve", wrapped_resolve)
    assert cache.resolved_path(rel_path) == resolved_path
    assert calls == 1


def test_shared_module_resolution_cache_reuses_import_scans(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "pkg" / "__init__.py"
    entry.parent.mkdir()
    entry.write_text("import pkg.helper\n")
    helper = entry.parent / "helper.py"
    helper.write_text("import warnings\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()

    collect_calls = 0
    original_collect = cli._collect_imports

    def wrapped_collect(*args: object, **kwargs: object) -> list[str]:
        nonlocal collect_calls
        collect_calls += 1
        return original_collect(*args, **kwargs)

    monkeypatch.setattr(cli, "_collect_imports", wrapped_collect)

    cache = cli._ModuleResolutionCache()
    graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        resolver_cache=cache,
    )
    first_collect_calls = collect_calls

    for module_name, module_path in graph.items():
        source = cache.read_module_source(module_path)
        tree = cache.parse_module_ast(module_path, source, filename=str(module_path))
        include_nested = (
            not cache.is_stdlib_path(module_path, stdlib_root)
            or module_name in cli.STDLIB_NESTED_IMPORT_SCAN_MODULES
        )
        cache.collect_imports(
            module_path,
            tree,
            module_name=module_name,
            is_package=module_path.name == "__init__.py",
            include_nested=include_nested,
        )

    assert collect_calls == first_collect_calls


def test_read_module_source_uses_utf8_fast_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source_path = tmp_path / "fast_utf8.py"
    source_path.write_text("value = 'hello'\n", encoding="utf-8")

    def fail_open(path: Path):  # type: ignore[no-untyped-def]
        raise AssertionError(f"tokenize.open should not run for {path}")

    monkeypatch.setattr(cli.tokenize, "open", fail_open)
    assert cli._read_module_source(source_path) == "value = 'hello'\n"


def test_read_module_source_falls_back_for_encoding_cookie(
    tmp_path: Path,
) -> None:
    source_path = tmp_path / "latin1_source.py"
    source_path.write_bytes("# -*- coding: latin-1 -*-\nname = 'caf\xe9'\n".encode("latin-1"))
    assert cli._read_module_source(source_path) == "# -*- coding: latin-1 -*-\nname = 'café'\n"


def test_stdlib_graph_ignores_nested_imports_for_core_scan(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print(1)\n")
    graph = _discover_with_core_modules(entry)
    assert "builtins" in graph
    assert "sys" in graph
    assert "importlib" in graph
    assert "importlib.util" in graph
    assert "importlib.machinery" in graph
    assert "warnings" not in graph
    assert "re" not in graph
    assert "dataclasses" not in graph


def test_typing_enables_nested_import_scan_for_collections_abc(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import typing\n")
    graph = _discover_with_core_modules(entry)
    assert "typing" in graph
    assert "_collections_abc" in graph


def test_spawn_entry_override_not_required_for_plain_script(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    stdlib_root = cli._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    module_graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )
    cli._collect_package_parents(module_graph, roots, stdlib_root, stdlib_allowlist)
    cli._ensure_core_stdlib_modules(module_graph, stdlib_root)
    core_paths = [
        path
        for name in (
            "builtins",
            "sys",
            "types",
            "importlib",
            "importlib.util",
            "importlib.machinery",
        )
        if (path := module_graph.get(name)) is not None
    ]
    for core_path in core_paths:
        core_graph, _ = cli._discover_module_graph(
            core_path,
            roots,
            module_roots,
            stdlib_root,
            stdlib_allowlist,
            skip_modules=cli.STUB_MODULES,
            stub_parents=cli.STUB_PARENT_MODULES,
            nested_stdlib_scan_modules=set(),
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    assert not cli._requires_spawn_entry_override(module_graph, explicit_imports)


def test_spawn_entry_override_required_for_multiprocessing(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import multiprocessing\nprint('ok')\n")
    stdlib_root = cli._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    module_graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )
    assert "multiprocessing" in module_graph
    assert cli._requires_spawn_entry_override(module_graph, explicit_imports)


def test_spawn_entry_override_required_for_spawn_import() -> None:
    graph = {"__main__": ROOT / "script.py"}
    explicit_imports = {"multiprocessing.spawn"}
    assert cli._requires_spawn_entry_override(graph, explicit_imports)


def test_merge_module_graph_with_reason_tracks_sources(tmp_path: Path) -> None:
    module_graph = {"__main__": tmp_path / "main.py"}
    reasons: dict[str, set[str]] = {}
    additions = {
        "__main__": tmp_path / "main.py",
        "multiprocessing.spawn": tmp_path / "spawn.py",
    }
    cli._merge_module_graph_with_reason(
        module_graph,
        additions,
        reasons,
        "spawn_closure",
    )
    assert "multiprocessing.spawn" in module_graph
    assert reasons["__main__"] == {"spawn_closure"}
    assert reasons["multiprocessing.spawn"] == {"spawn_closure"}


def test_build_reason_summary_is_stable() -> None:
    reasons = {
        "a": {"entry_closure"},
        "b": {"entry_closure", "core_closure"},
        "c": {"core_closure"},
    }
    summary = cli._build_reason_summary(reasons)
    assert summary == {"core_closure": 2, "entry_closure": 2}


def test_build_diagnostics_enabled_from_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BUILD_DIAGNOSTICS", "1")
    assert cli._build_diagnostics_enabled()
    monkeypatch.setenv("MOLT_BUILD_DIAGNOSTICS", "0")
    assert not cli._build_diagnostics_enabled()


def test_build_allocation_diagnostics_enabled_from_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BUILD_ALLOCATIONS", "1")
    assert cli._build_allocation_diagnostics_enabled()
    monkeypatch.setenv("MOLT_BUILD_ALLOCATIONS", "0")
    assert not cli._build_allocation_diagnostics_enabled()


def test_resolve_build_diagnostics_verbosity_aliases() -> None:
    assert cli._resolve_build_diagnostics_verbosity(None) == "default"
    assert cli._resolve_build_diagnostics_verbosity("brief") == "summary"
    assert cli._resolve_build_diagnostics_verbosity("verbose") == "full"
    assert cli._resolve_build_diagnostics_verbosity("unknown") == "default"


def test_phase_duration_map_orders_by_start(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli.time, "perf_counter", lambda: 10.0)
    durations = cli._phase_duration_map({"module_graph": 2.0, "resolve_entry": 1.0})
    assert durations["resolve_entry"] == 1.0
    assert durations["module_graph"] == 8.0


def test_resolve_build_diagnostics_path_relative_and_absolute(tmp_path: Path) -> None:
    rel = cli._resolve_build_diagnostics_path("diag.json", tmp_path)
    assert rel == tmp_path / "diag.json"
    abs_path = tmp_path / "absolute_diag.json"
    resolved_abs = cli._resolve_build_diagnostics_path(str(abs_path), tmp_path)
    assert resolved_abs == abs_path


def test_build_midend_diagnostics_payload_summarizes_policy_and_passes() -> None:
    payload = cli._build_midend_diagnostics_payload(
        requested_profile="release",
        policy_outcomes_by_function={
            "pkg.mod::fn_a": {
                "profile": "release",
                "tier": "A",
                "tier_base": "B",
                "tier_effective": "A",
                "tier_source": "default",
                "promoted": True,
                "promotion_source": "pgo_hot_functions",
                "promotion_signal": "pkg.mod::fn_a",
                "budget_ms": 120.0,
                "spent_ms": 140.0,
                "degraded": True,
                "degrade_events": [
                    {
                        "reason": "budget_exceeded",
                        "stage": "round_2_post_dce",
                        "action": "disable_cse",
                        "spent_ms": 140.0,
                    }
                ],
            }
        },
        pass_stats_by_function={
            "pkg.mod::fn_a": {
                "sccp_edge_thread": {
                    "attempted": 2,
                    "accepted": 1,
                    "rejected": 1,
                    "degraded": 0,
                    "ms_total": 9.5,
                    "ms_max": 6.0,
                    "samples_ms": [3.5, 6.0],
                },
                "cse": {
                    "attempted": 1,
                    "accepted": 0,
                    "rejected": 1,
                    "degraded": 1,
                    "ms_total": 4.25,
                    "ms_max": 4.25,
                    "samples_ms": [4.25],
                },
            }
        },
    )
    assert payload is not None
    assert payload["requested_profile"] == "release"
    assert payload["degraded_functions"] == 1
    assert payload["tier_summary"] == {"A": 1}
    assert payload["tier_base_summary"] == {"B": 1}
    assert payload["promoted_functions"] == 1
    assert payload["promotion_source_summary"] == {"pgo_hot_functions": 1}
    assert payload["degrade_reason_summary"] == {"budget_exceeded": 1}
    assert payload["policy_config"]["hot_tier_promotion_enabled"] is True
    assert payload["policy_config"]["budget_alpha"] == 0.03
    assert payload["policy_config"]["budget_beta"] == 0.75
    assert payload["policy_config"]["budget_scale"] == 1.0
    assert payload["function_count"] == 1
    hotspots = payload["pass_hotspots_top"]
    assert hotspots
    assert hotspots[0]["module"] == "pkg.mod"
    assert hotspots[0]["function"] == "fn_a"
    assert hotspots[0]["pass"] == "sccp_edge_thread"
    fn_hotspots = payload["function_hotspots_top"]
    assert fn_hotspots
    assert fn_hotspots[0]["module"] == "pkg.mod"
    assert fn_hotspots[0]["function"] == "fn_a"
    degrade_hotspots = payload["degrade_event_hotspots_top"]
    assert degrade_hotspots
    assert degrade_hotspots[0]["reason"] == "budget_exceeded"
    promotion_hotspots = payload["promotion_hotspots_top"]
    assert promotion_hotspots
    assert promotion_hotspots[0]["module"] == "pkg.mod"
    assert promotion_hotspots[0]["function"] == "fn_a"
    assert promotion_hotspots[0]["tier_base"] == "B"
    assert promotion_hotspots[0]["tier_effective"] == "A"


def test_resolve_frontend_parallel_module_workers_from_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_FRONTEND_PARALLEL_MODULES", raising=False)
    assert cli._resolve_frontend_parallel_module_workers() == 0

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "0")
    assert cli._resolve_frontend_parallel_module_workers() == 0

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "3")
    assert cli._resolve_frontend_parallel_module_workers() == 3

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "auto")
    assert cli._resolve_frontend_parallel_module_workers() >= 2


def test_module_dependency_layers_preserve_topological_determinism() -> None:
    order = ["a", "b", "c", "d", "e"]
    deps = {
        "a": set(),
        "b": {"a"},
        "c": {"a"},
        "d": {"b", "c"},
        "e": {"b"},
    }
    layers = cli._module_dependency_layers(order, deps)
    assert layers == [["a"], ["b", "c"], ["d", "e"]]


def test_choose_frontend_parallel_layer_workers_applies_policy_gates() -> None:
    decision = cli._choose_frontend_parallel_layer_workers(
        candidates=["a", "b"],
        module_sources={"a": "x=1\n", "b": "y=2\n"},
        module_deps={"a": set(), "b": set()},
        max_workers=8,
        min_modules=3,
        min_predicted_cost=1.0,
        target_cost_per_worker=10.0,
    )
    assert decision["enabled"] is False
    assert decision["reason"] == "layer_module_count_below_min"

    decision = cli._choose_frontend_parallel_layer_workers(
        candidates=["a", "b", "c"],
        module_sources={"a": "x=1\n", "b": "y=2\n", "c": "z=3\n"},
        module_deps={"a": set(), "b": set(), "c": set()},
        max_workers=8,
        min_modules=2,
        min_predicted_cost=100_000.0,
        target_cost_per_worker=10.0,
    )
    assert decision["enabled"] is False
    assert decision["reason"] == "layer_predicted_cost_below_min"


def test_choose_frontend_parallel_layer_workers_scales_workers_by_cost() -> None:
    decision = cli._choose_frontend_parallel_layer_workers(
        candidates=["a", "b", "c", "d"],
        module_sources={
            "a": "x" * 40_000,
            "b": "y" * 40_000,
            "c": "z" * 40_000,
            "d": "w" * 40_000,
        },
        module_deps={"a": {"x"}, "b": {"x"}, "c": {"x"}, "d": {"x"}},
        max_workers=6,
        min_modules=2,
        min_predicted_cost=1.0,
        target_cost_per_worker=50_000.0,
    )
    assert decision["enabled"] is True
    assert int(decision["workers"]) == 4


def test_module_order_has_back_edges_detects_cycles() -> None:
    order = ["a", "b"]
    assert cli._module_order_has_back_edges(order, {"a": {"b"}, "b": {"a"}})
    assert not cli._module_order_has_back_edges(order, {"a": set(), "b": {"a"}})


def test_frontend_lower_module_worker_smoke(tmp_path: Path) -> None:
    module_path = tmp_path / "worker_module.py"
    payload = {
        "module_name": "worker_module",
        "module_path": str(module_path),
        "source": "x = 1\ny = x + 2\n",
        "parse_codec": "msgpack",
        "type_hint_policy": "ignore",
        "fallback_policy": "error",
        "module_is_namespace": False,
        "entry_module": None,
        "enable_phi": True,
        "known_modules": ["worker_module"],
        "known_classes": {},
        "stdlib_allowlist": [],
        "known_func_defaults": {},
        "module_chunking": False,
        "module_chunk_max_ops": 0,
        "optimization_profile": "dev",
        "pgo_hot_functions": ["worker_module::molt_main"],
    }
    result = cli._frontend_lower_module_worker(payload)
    assert result["ok"] is True
    assert isinstance(result["functions"], list)
    assert isinstance(result["func_code_ids"], dict)
    assert isinstance(result["timings"]["total_s"], float)
    worker = result["worker"]
    assert isinstance(worker["pid"], int)
    assert worker["started_ns"] > 0
    assert worker["finished_ns"] >= worker["started_ns"]


def test_duration_ms_from_ns_clamps_and_converts() -> None:
    assert cli._duration_ms_from_ns(1_000_000, 2_500_000) == 1.5
    assert cli._duration_ms_from_ns(5, 4) == 0.0
    assert cli._duration_ms_from_ns("bad", 10) == 0.0


def test_emit_build_diagnostics_includes_frontend_parallel_layer_counters(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli._emit_build_diagnostics(
        diagnostics={
            "total_sec": 1.25,
            "frontend_parallel": {
                "enabled": True,
                "workers": 4,
                "mode": "process_pool",
                "reason": "enabled",
                "policy": {
                    "min_modules": 2,
                    "min_predicted_cost": 32768.0,
                    "target_cost_per_worker": 65536.0,
                },
                "layers": [
                    {
                        "index": 0,
                        "mode": "parallel",
                        "policy_reason": "enabled",
                        "module_count": 3,
                        "candidate_count": 3,
                        "workers": 3,
                        "queue_ms_total": 4.5,
                        "wait_ms_total": 2.0,
                        "exec_ms_total": 9.0,
                    }
                ],
                "worker_summary": {
                    "count": 3,
                    "queue_ms_total": 4.5,
                    "queue_ms_max": 2.5,
                    "wait_ms_total": 2.0,
                    "wait_ms_max": 1.0,
                    "exec_ms_total": 9.0,
                    "exec_ms_max": 4.0,
                },
            },
            "midend": {
                "policy_config": {
                    "profile_override": None,
                    "hot_tier_promotion_enabled": True,
                    "budget_override_ms": None,
                    "budget_alpha": 0.03,
                    "budget_beta": 0.75,
                    "budget_scale": 1.0,
                },
                "promoted_functions": 2,
                "promotion_source_summary": {"pgo_hot_functions": 2},
                "promotion_hotspots_top": [
                    {
                        "module": "pkg.mod",
                        "function": "hot_fn",
                        "tier_base": "B",
                        "tier_effective": "A",
                        "source": "pgo_hot_functions",
                        "signal": "pkg.mod::hot_fn",
                        "spent_ms": 12.5,
                    }
                ],
            },
        },
        diagnostics_path=None,
        json_output=False,
    )
    stderr = capsys.readouterr().err
    assert "frontend_parallel.policy: min_modules=2" in stderr
    assert "- frontend_parallel.layers: 1" in stderr
    assert "frontend_parallel.layer.1: mode=parallel" in stderr
    assert "frontend_parallel.worker_ms: count=3" in stderr
    assert "- midend.policy.hot_tier_promotion_enabled: True" in stderr
    assert "- midend.policy.budget_formula: alpha=0.0300 beta=0.7500 scale=1.0000" in stderr
    assert "- midend.promoted_functions: 2" in stderr
    assert "- midend.promotion_source.pgo_hot_functions: 2" in stderr
    assert "midend.promotion_hotspot.1: pkg.mod::hot_fn B->A" in stderr


def test_capture_build_allocation_diagnostics_returns_top_sites() -> None:
    cli.tracemalloc.start(5)
    try:
        payload = cli._capture_build_allocation_diagnostics(top_n=3)
    finally:
        cli.tracemalloc.stop()
    assert payload is not None
    assert isinstance(payload["current_bytes"], int)
    assert isinstance(payload["peak_bytes"], int)
    assert len(payload["top"]) <= 3


def test_emit_build_diagnostics_prints_allocation_summary(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli._emit_build_diagnostics(
        diagnostics={
            "total_sec": 1.0,
            "allocations": {
                "current_bytes": 1024,
                "peak_bytes": 4096,
                "top": [
                    {
                        "file": "src/molt/cli.py",
                        "line": 123,
                        "size_bytes": 2048,
                        "count": 7,
                    }
                ],
            },
        },
        diagnostics_path=None,
        json_output=False,
        verbosity="full",
    )
    stderr = capsys.readouterr().err
    assert "- alloc.current_bytes: 1024" in stderr
    assert "- alloc.peak_bytes: 4096" in stderr
    assert "alloc.top.1: src/molt/cli.py:123 size_bytes=2048 count=7" in stderr


def test_midend_policy_config_snapshot_honors_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_MIDEND_PROFILE", "release")
    monkeypatch.setenv("MOLT_MIDEND_HOT_TIER_PROMOTION", "0")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_MS", "42")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_ALPHA", "0.5")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_BETA", "2.0")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_SCALE", "1.5")

    assert cli._midend_policy_config_snapshot() == {
        "profile_override": "release",
        "hot_tier_promotion_enabled": False,
        "budget_override_ms": 42.0,
        "budget_alpha": 0.5,
        "budget_beta": 2.0,
        "budget_scale": 1.5,
    }


def test_emit_build_diagnostics_summary_omits_hotspot_details(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli._emit_build_diagnostics(
        diagnostics={
            "total_sec": 1.25,
            "frontend_parallel": {
                "enabled": True,
                "workers": 4,
                "mode": "process_pool",
                "reason": "enabled",
                "policy": {
                    "min_modules": 2,
                    "min_predicted_cost": 32768.0,
                    "target_cost_per_worker": 65536.0,
                },
                "layers": [
                    {
                        "index": 0,
                        "mode": "parallel",
                        "module_count": 3,
                        "candidate_count": 3,
                        "workers": 3,
                        "queue_ms_total": 4.5,
                        "wait_ms_total": 2.0,
                        "exec_ms_total": 9.0,
                    }
                ],
                "worker_summary": {
                    "count": 3,
                    "queue_ms_total": 4.5,
                    "queue_ms_max": 2.5,
                    "wait_ms_total": 2.0,
                    "wait_ms_max": 1.0,
                    "exec_ms_total": 9.0,
                    "exec_ms_max": 4.0,
                },
            },
            "midend": {
                "promoted_functions": 2,
                "promotion_source_summary": {"pgo_hot_functions": 2},
                "promotion_hotspots_top": [
                    {
                        "module": "pkg.mod",
                        "function": "hot_fn",
                        "tier_base": "B",
                        "tier_effective": "A",
                        "source": "pgo_hot_functions",
                        "signal": "pkg.mod::hot_fn",
                        "spent_ms": 12.5,
                    }
                ],
            },
        },
        diagnostics_path=None,
        json_output=False,
        verbosity="summary",
    )
    stderr = capsys.readouterr().err
    assert "Build diagnostics:" in stderr
    assert "- frontend_parallel: enabled=True workers=4 mode=process_pool" in stderr
    assert "- midend.promoted_functions: 2" in stderr
    assert "frontend_parallel.layer.1:" not in stderr
    assert "frontend_parallel.worker_ms:" not in stderr
    assert "midend.promotion_hotspot.1:" not in stderr


def test_emit_build_diagnostics_full_prints_extended_hotspots(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli._emit_build_diagnostics(
        diagnostics={
            "frontend_module_timings_top": [
                {
                    "module": f"pkg.mod_{idx}",
                    "total_s": float(idx),
                    "visit_s": 0.1,
                    "lower_s": 0.2,
                }
                for idx in range(12)
            ]
        },
        diagnostics_path=None,
        json_output=False,
        verbosity="full",
    )
    stderr = capsys.readouterr().err
    assert "frontend.hotspot.10: pkg.mod_9" in stderr
    assert "frontend.hotspot.12: pkg.mod_11" in stderr


def test_module_name_from_path_outside_module_roots_uses_stem(tmp_path: Path) -> None:
    script = tmp_path / "outside_script.py"
    script.write_text("print('ok')\n")
    stdlib_root = cli._stdlib_root_path()
    roots = [ROOT.resolve(), (ROOT / "src").resolve()]
    assert cli._module_name_from_path(script, roots, stdlib_root) == "outside_script"


def test_expand_module_chain_ignores_invalid_module_names() -> None:
    assert cli._expand_module_chain("pkg.sub") == ["pkg", "pkg.sub"]
    assert cli._expand_module_chain("") == []
    assert cli._expand_module_chain("/.Volumes.bad.mod") == []


def test_extract_runtime_feedback_hot_functions_sorts_and_dedupes() -> None:
    warnings: list[str] = []
    payload = {
        "hot_functions": [
            {"symbol": "pkg.mod::warm_fn", "count": 3},
            {"symbol": "pkg.mod::hot_fn", "count": 9},
            "pkg.mod::hot_fn",
            ["pkg.mod::cold_fn", 1],
        ]
    }

    assert cli._extract_runtime_feedback_hot_functions(payload, warnings) == [
        "pkg.mod::hot_fn",
        "pkg.mod::warm_fn",
        "pkg.mod::cold_fn",
    ]
    assert warnings == []


def test_resolve_backend_profile_defaults_to_selected_build_profile(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BACKEND_PROFILE", "")
    profile, error = cli._resolve_backend_profile("dev")
    assert profile == "dev"
    assert error is None


def test_resolve_backend_profile_env_override_and_validation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BACKEND_PROFILE", "release")
    profile, error = cli._resolve_backend_profile("dev")
    assert profile == "release"
    assert error is None

    monkeypatch.setenv("MOLT_BACKEND_PROFILE", "invalid")
    profile, error = cli._resolve_backend_profile("dev")
    assert profile == "dev"
    assert error == "Invalid MOLT_BACKEND_PROFILE value: invalid"


def test_resolve_cargo_profile_name_defaults_and_validation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_DEV_CARGO_PROFILE", raising=False)
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "dev-fast"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "my-dev_1")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "my-dev_1"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "bad profile")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "dev"
    assert error == "Invalid MOLT_DEV_CARGO_PROFILE value: bad profile"


def test_backend_daemon_retryable_error_classification() -> None:
    assert cli._backend_daemon_retryable_error("backend daemon returned empty response")
    assert cli._backend_daemon_retryable_error("unsupported protocol version 9")
    assert cli._backend_daemon_retryable_error(
        "backend daemon connection failed: timeout"
    )
    assert not cli._backend_daemon_retryable_error(
        "backend daemon failed to compile job"
    )


def test_backend_daemon_request_payload_bytes_is_unbounded() -> None:
    payload = {"version": 1, "jobs": [{"id": "x", "ir": "x" * 4096}]}
    data, err = cli._backend_daemon_request_payload_bytes(payload)
    assert isinstance(data, bytes)
    assert data.endswith(b"\n")
    assert b": " not in data
    assert b", " not in data
    assert err is None


def test_backend_daemon_start_timeout_is_unbounded() -> None:
    assert cli._backend_daemon_start_timeout() is None


def test_backend_codegen_env_digest_tracks_codegen_knobs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    baseline_native = cli._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_BACKEND_REGALLOC_ALGORITHM", "single_pass")
    native_changed = cli._backend_codegen_env_digest(is_wasm=False)
    assert native_changed != baseline_native

    baseline_wasm = cli._backend_codegen_env_digest(is_wasm=True)
    monkeypatch.setenv("MOLT_WASM_TABLE_BASE", "2048")
    wasm_changed = cli._backend_codegen_env_digest(is_wasm=True)
    assert wasm_changed != baseline_wasm


def test_backend_daemon_config_digest_and_socket_path_include_config(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET", raising=False)
    digest_a = cli._backend_daemon_config_digest(tmp_path, "dev-fast")
    monkeypatch.setenv("MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2", "2")
    digest_b = cli._backend_daemon_config_digest(tmp_path, "dev-fast")
    assert digest_a != digest_b

    socket_a = cli._backend_daemon_socket_path(
        tmp_path, "dev-fast", config_digest=digest_a
    )
    socket_b = cli._backend_daemon_socket_path(
        tmp_path, "dev-fast", config_digest=digest_b
    )
    assert socket_a != socket_b


def test_function_cache_key_tracks_top_level_ir_extras() -> None:
    ir_base = {"functions": [{"name": "f", "ops": []}], "profile": None}
    ir_extra_a = {
        "profile": None,
        "functions": [{"name": "f", "ops": []}],
        "meta": {"x": 1},
    }
    ir_extra_b = {
        "functions": [{"name": "f", "ops": []}],
        "meta": {"x": 1},
        "profile": None,
    }
    key_base = cli._function_cache_key(ir_base, "native", None, "variant")
    key_extra_a = cli._function_cache_key(ir_extra_a, "native", None, "variant")
    key_extra_b = cli._function_cache_key(ir_extra_b, "native", None, "variant")
    assert key_extra_a != key_base
    assert key_extra_a == key_extra_b


def test_compile_with_backend_daemon_surfaces_cache_telemetry(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    captured_payload: dict[str, object] = {}

    def _fake_request(
        socket_path: Path,
        data: bytes,
        *,
        timeout: float | None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, timeout
        captured_payload.update(json.loads(data))
        backend_output.write_bytes(b"\x7fELF")
        return (
            {
                "ok": True,
                "jobs": [
                    {
                        "id": "job0",
                        "ok": True,
                        "cached": True,
                        "cache_tier": "function",
                    }
                ],
                "health": {"pid": 42, "cache_hits": 1, "cache_misses": 0},
            },
            None,
        )

    monkeypatch.setattr(cli, "_backend_daemon_request_bytes", _fake_request)
    result = cli._compile_with_backend_daemon(
        Path("/tmp/fake.sock"),
        ir={"functions": []},
        backend_output=backend_output,
        is_wasm=False,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key="function-cache",
        config_digest="digest123",
        timeout=0.1,
    )
    assert result.ok is True
    assert result.cached is True
    assert result.cache_tier == "function"
    assert captured_payload.get("config_digest") == "digest123"


def test_cached_backend_artifact_validity_guard(tmp_path: Path) -> None:
    wasm_bad = tmp_path / "bad.wasm"
    wasm_bad.write_bytes(b"not-wasm")
    assert not cli._is_valid_cached_backend_artifact(wasm_bad, is_wasm=True)

    wasm_good = tmp_path / "good.wasm"
    wasm_good.write_bytes(b"\x00asm\x01\x00\x00\x00")
    assert cli._is_valid_cached_backend_artifact(wasm_good, is_wasm=True)

    native_empty = tmp_path / "empty.o"
    native_empty.write_bytes(b"")
    assert not cli._is_valid_cached_backend_artifact(native_empty, is_wasm=False)

    native_nonempty = tmp_path / "nonempty.o"
    native_nonempty.write_bytes(b"\x01")
    assert cli._is_valid_cached_backend_artifact(native_nonempty, is_wasm=False)


def test_backend_daemon_health_from_response_parses_int_fields() -> None:
    response = {
        "ok": True,
        "pong": True,
        "health": {
            "pid": 123,
            "uptime_ms": 456,
            "cache_entries": 2,
            "cache_bytes": 100,
            "requests_total": 7,
            "jobs_total": 9,
            "cache_hits": 4,
            "cache_misses": 5,
        },
    }
    health = cli._backend_daemon_health_from_response(response)
    assert isinstance(health, dict)
    assert health["pid"] == 123
    assert health["cache_entries"] == 2


def test_backend_daemon_ping_health_backcompat_without_health(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        cli,
        "_backend_daemon_request",
        lambda socket_path, payload, timeout: ({"ok": True, "pong": True}, None),
    )
    ready, health = cli._backend_daemon_ping_health(Path("/tmp/fake.sock"), timeout=0.1)
    assert ready is True
    assert health is None


def test_internal_batch_build_server_ping_shutdown_roundtrip() -> None:
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    proc = subprocess.Popen(
        [sys.executable, "-m", "molt.cli", "internal-batch-build-server"],
        cwd=str(ROOT),
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert proc.stdin is not None
    assert proc.stdout is not None
    proc.stdin.write(json.dumps({"id": 1, "op": "ping"}) + "\n")
    proc.stdin.flush()
    ping_response = json.loads(proc.stdout.readline())
    assert ping_response["ok"] is True
    assert ping_response["pong"] is True
    proc.stdin.write(json.dumps({"id": 2, "op": "shutdown"}) + "\n")
    proc.stdin.flush()
    shutdown_response = json.loads(proc.stdout.readline())
    assert shutdown_response["ok"] is True
    assert shutdown_response["shutdown"] is True
    proc.wait(timeout=5)
    assert proc.returncode == 0
