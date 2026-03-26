from __future__ import annotations

import ast
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import cast

import molt.cli as cli
import pytest
from molt.frontend import MoltValue
from molt.type_facts import Fact, FunctionFacts, ModuleFacts, TypeFacts


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


def test_write_importer_module_avoids_rewriting_identical_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path_type = type(tmp_path)
    original_write_text = path_type.write_text
    writes = 0

    def wrapped_write_text(
        self: Path, data: str, *args: object, **kwargs: object
    ) -> int:
        nonlocal writes
        if self == tmp_path / f"{cli.IMPORTER_MODULE_NAME}.py":
            writes += 1
        return original_write_text(self, data, *args, **kwargs)

    monkeypatch.setattr(path_type, "write_text", wrapped_write_text)

    cli._write_importer_module(["pkg.alpha", "solo"], tmp_path)
    cli._write_importer_module(["pkg.alpha", "solo"], tmp_path)

    assert writes == 1


def test_write_namespace_module_avoids_rewriting_identical_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path_type = type(tmp_path)
    original_write_text = path_type.write_text
    writes = 0
    expected_path = tmp_path / "namespace_demo_pkg.py"

    def wrapped_write_text(
        self: Path, data: str, *args: object, **kwargs: object
    ) -> int:
        nonlocal writes
        if self == expected_path:
            writes += 1
        return original_write_text(self, data, *args, **kwargs)

    monkeypatch.setattr(path_type, "write_text", wrapped_write_text)

    cli._write_namespace_module("demo.pkg", ["/tmp/demo/pkg"], tmp_path)
    cli._write_namespace_module("demo.pkg", ["/tmp/demo/pkg"], tmp_path)

    assert writes == 1


def test_build_module_lowering_metadata_precomputes_module_flags(
    tmp_path: Path,
) -> None:
    module_graph = {
        "app_entry": tmp_path / "app.py",
        "pkg": tmp_path / "pkg" / "__init__.py",
        "pkg.mod": tmp_path / "pkg" / "mod.py",
    }
    (
        logical_source_path_by_module,
        entry_override_by_module,
        namespace_by_module,
        (is_package_by_module),
    ) = cli._build_module_lowering_metadata(
        module_graph,
        generated_module_source_paths={"pkg": "/generated/pkg/__init__.py"},
        entry_module="app_entry",
        namespace_module_names={"pkg"},
    )

    assert logical_source_path_by_module["pkg"] == "/generated/pkg/__init__.py"
    assert logical_source_path_by_module["pkg.mod"] == str(module_graph["pkg.mod"])
    assert entry_override_by_module["app_entry"] is None
    assert entry_override_by_module["pkg.mod"] == "app_entry"
    assert namespace_by_module["pkg"] is True
    assert namespace_by_module["pkg.mod"] is False
    assert is_package_by_module["pkg"] is True
    assert is_package_by_module["pkg.mod"] is False


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
        tmp_path / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
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
        ROOT,
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
    if core_paths:
        core_graph, _ = cli._discover_module_graph_from_paths(
            core_paths,
            roots,
            module_roots,
            stdlib_root,
            ROOT,
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


def test_cached_json_round_trips_molt_value_and_set() -> None:
    payload = {
        "value": MoltValue(name="v1", type_hint="int"),
        "names": {"alpha", "beta"},
    }

    encoded = json.dumps(payload, default=cli._json_ir_default)
    decoded = cli._decode_cached_json_value(json.loads(encoded))

    assert decoded["value"] == MoltValue(name="v1", type_hint="int")
    assert decoded["names"] == {"alpha", "beta"}


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


def test_collect_imports_uses_single_module_tree_walk_for_nested_scans(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tree = ast.parse(
        "import os\n"
        "import importlib\n"
        "MODULE_NAME = 'warnings'\n"
        "def _probe(module_name):\n"
        "    return importlib.import_module(module_name)\n"
        "_probe(MODULE_NAME)\n"
    )
    module_tree_walks = 0
    original_walk = cli.ast.walk

    def wrapped_walk(node: ast.AST):
        nonlocal module_tree_walks
        if node is tree:
            module_tree_walks += 1
        return original_walk(node)

    monkeypatch.setattr(cli.ast, "walk", wrapped_walk)

    imports = cli._collect_imports(tree)

    assert module_tree_walks == 1
    assert "os" in imports
    assert "warnings" in imports


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


def test_link_fingerprint_reuses_inputs_digest_when_unchanged(tmp_path: Path) -> None:
    stub = tmp_path / "main_stub.c"
    obj = tmp_path / "output.o"
    runtime = tmp_path / "libmolt_runtime.a"
    stub.write_text("int main(void) { return 0; }\n")
    obj.write_bytes(b"\x7fELFobject")
    runtime.write_bytes(b"archive")

    fingerprint = cli._link_fingerprint(
        project_root=tmp_path,
        inputs=[stub, obj, runtime],
        link_cmd=["clang", str(stub), str(obj), str(runtime), "-o", "app"],
    )
    assert fingerprint is not None

    reused = cli._link_fingerprint(
        project_root=tmp_path,
        inputs=[stub, obj, runtime],
        link_cmd=["clang", str(stub), str(obj), str(runtime), "-o", "app"],
        stored_fingerprint=fingerprint,
    )
    assert reused == fingerprint


def test_link_fingerprint_changes_when_link_command_changes(tmp_path: Path) -> None:
    stub = tmp_path / "main_stub.c"
    obj = tmp_path / "output.o"
    runtime = tmp_path / "libmolt_runtime.a"
    stub.write_text("int main(void) { return 0; }\n")
    obj.write_bytes(b"\x7fELFobject")
    runtime.write_bytes(b"archive")

    first = cli._link_fingerprint(
        project_root=tmp_path,
        inputs=[stub, obj, runtime],
        link_cmd=["clang", str(stub), str(obj), str(runtime), "-o", "app"],
    )
    second = cli._link_fingerprint(
        project_root=tmp_path,
        inputs=[stub, obj, runtime],
        link_cmd=[
            "clang",
            "-fuse-ld=lld",
            str(stub),
            str(obj),
            str(runtime),
            "-o",
            "app",
        ],
    )
    assert first is not None
    assert second is not None
    assert first["hash"] != second["hash"]


def test_prepare_native_link_includes_stdlib_object_in_link_fingerprint_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    output_binary = tmp_path / "app"
    stdlib_obj = tmp_path / "stdlib.o"
    stdlib_obj.write_bytes(b"stdlib")
    captured_inputs: list[Path] = []
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()

    def fake_link_fingerprint(
        *,
        project_root: Path,
        inputs: list[Path],
        link_cmd: list[str],
        stored_fingerprint: dict[str, object] | None = None,
    ) -> dict[str, str]:
        del project_root, link_cmd, stored_fingerprint
        captured_inputs[:] = inputs
        return {"hash": "fingerprint", "rustc": None, "inputs_digest": None}

    monkeypatch.setattr(cli, "_link_fingerprint", fake_link_fingerprint)
    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: False)

    prepared, error = cli._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=output_binary,
        runtime_lib=runtime_lib,
        molt_root=tmp_path,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        project_root=tmp_path,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        stdlib_obj_path=stdlib_obj,
    )

    assert error is None
    assert prepared is not None
    assert captured_inputs == [
        tmp_path / "artifacts" / "main_stub.c",
        output_obj,
        runtime_lib,
        stdlib_obj,
    ]


def test_prepare_native_link_rehashes_when_stdlib_object_contents_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    output_binary = tmp_path / "app"
    stdlib_obj = tmp_path / "stdlib.o"
    stdlib_obj.write_bytes(b"stdlib-v1")
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()

    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: False)
    monkeypatch.setattr(
        cli,
        "_run_native_link_command",
        lambda **kwargs: subprocess.CompletedProcess(
            kwargs["link_cmd"], 0, "", ""
        ),
    )

    first, first_error = cli._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=output_binary,
        runtime_lib=runtime_lib,
        molt_root=tmp_path,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        project_root=tmp_path,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        stdlib_obj_path=stdlib_obj,
    )
    assert first_error is None
    assert first is not None

    stdlib_obj.write_bytes(b"stdlib-v2")

    second, second_error = cli._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=output_binary,
        runtime_lib=runtime_lib,
        molt_root=tmp_path,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        project_root=tmp_path,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        stdlib_obj_path=stdlib_obj,
    )
    assert second_error is None
    assert second is not None
    assert first.link_fingerprint["hash"] != second.link_fingerprint["hash"]


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
        None,
        stdlib_allowlist,
        resolver_cache=shared_cache,
    )
    shared_first = resolve_calls
    cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        None,
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
        None,
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
        None,
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
        tmp_path,
        stdlib_allowlist,
        resolver_cache=shared_cache,
    )
    first_read_calls = read_calls
    first_parse_calls = parse_calls
    for module_path in graph.values():
        source = shared_cache.read_module_source(module_path)
        shared_cache.parse_module_ast(module_path, source, filename=str(module_path))
    assert read_calls == first_read_calls
    assert parse_calls == first_parse_calls

    read_calls = 0
    parse_calls = 0
    unshared_graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
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
    monkeypatch.setattr(cli, "_read_persisted_import_scan", lambda *args, **kwargs: None)
    monkeypatch.setattr(cli, "_read_persisted_module_graph", lambda *args, **kwargs: None)

    cache = cli._ModuleResolutionCache()
    graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
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


def test_discover_module_graph_reuses_persisted_import_scan_cache(
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

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    assert "pkg.helper" in explicit_imports
    assert "pkg" in graph

    def fail_read(path: Path) -> str:
        raise AssertionError(f"unexpected source read for {path}")

    monkeypatch.setattr(cli, "_read_module_source", fail_read)

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    assert "pkg.helper" in explicit_imports
    assert "pkg" in graph


def test_discover_module_graph_reuses_persisted_graph_cache(
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

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    assert "pkg.helper" in explicit_imports
    assert "pkg" in graph


def test_discover_module_graph_reuses_precomputed_entry_imports(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import pkg.helper\n")
    package = tmp_path / "pkg"
    package.mkdir()
    (package / "__init__.py").write_text("")
    helper = package / "helper.py"
    helper.write_text("VALUE = 1\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    cache = cli._ModuleResolutionCache()
    reads: list[Path] = []
    original_read = cache.read_module_source

    def wrapped_read(path: Path) -> str:
        reads.append(path)
        return original_read(path)

    monkeypatch.setattr(cache, "read_module_source", wrapped_read)

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
        resolver_cache=cache,
        precomputed_imports=("pkg.helper",),
    )

    assert entry not in reads
    assert helper in reads
    assert explicit_imports == {"pkg.helper"}
    assert "main" in graph
    assert "pkg.helper" in graph


def test_discover_module_graph_from_paths_batches_shared_dependency_scan(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = tmp_path / "first.py"
    first.write_text("import shared\n")
    second = tmp_path / "second.py"
    second.write_text("import shared\n")
    shared = tmp_path / "shared.py"
    shared.write_text("VALUE = 1\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    cache = cli._ModuleResolutionCache()
    reads: list[Path] = []
    original_read = cache.read_module_source

    def wrapped_read(path: Path) -> str:
        reads.append(path)
        return original_read(path)

    monkeypatch.setattr(cache, "read_module_source", wrapped_read)

    graph, explicit_imports = cli._discover_module_graph_from_paths(
        [first, second],
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        set(),
        resolver_cache=cache,
    )

    assert reads.count(first) == 1
    assert reads.count(second) == 1
    assert reads.count(shared) == 1
    assert explicit_imports == {"shared"}
    assert "shared" in graph


def test_discover_module_graph_from_paths_deduplicates_repeated_import_names(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = tmp_path / "first.py"
    first.write_text("import shared\n")
    second = tmp_path / "second.py"
    second.write_text("import shared\n")
    shared = tmp_path / "shared.py"
    shared.write_text("VALUE = 1\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    expand_calls = 0
    original_expand = cli._expand_module_chain_cached

    def wrapped_expand(name: str):
        nonlocal expand_calls
        if name == "shared":
            expand_calls += 1
        return original_expand(name)

    monkeypatch.setattr(cli, "_expand_module_chain_cached", wrapped_expand)

    graph, explicit_imports = cli._discover_module_graph_from_paths(
        [first, second],
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        set(),
    )

    assert expand_calls == 1
    assert explicit_imports == {"shared"}
    assert "shared" in graph


def test_discover_module_graph_reuses_persisted_paths_for_unchanged_modules(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "pkg" / "__init__.py"
    entry.parent.mkdir()
    entry.write_text("import pkg.helper\nimport pkg.extra\n")
    helper = entry.parent / "helper.py"
    helper.write_text("VALUE = 1\n")
    extra = entry.parent / "extra.py"
    extra.write_text("VALUE = 2\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    assert {"pkg", "pkg.helper", "pkg.extra"} <= set(graph)
    assert {"pkg.helper", "pkg.extra"} <= explicit_imports

    helper.write_text("VALUE = 10\n")
    cache = cli._ModuleResolutionCache()
    read_paths: list[Path] = []
    original_read = cache.read_module_source
    original_resolve = cache.resolve_module
    resolved_candidates: list[str] = []

    def wrapped_read(path: Path) -> str:
        read_paths.append(path)
        return original_read(path)

    def wrapped_resolve(
        candidate: str,
        roots_arg: list[Path],
        stdlib_root_arg: Path,
        stdlib_allowlist_arg: set[str],
    ) -> Path | None:
        resolved_candidates.append(candidate)
        return original_resolve(
            candidate, roots_arg, stdlib_root_arg, stdlib_allowlist_arg
        )

    monkeypatch.setattr(cache, "read_module_source", wrapped_read)
    monkeypatch.setattr(cache, "resolve_module", wrapped_resolve)

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
        resolver_cache=cache,
    )

    assert {"pkg", "pkg.helper", "pkg.extra"} <= set(graph)
    assert {"pkg.helper", "pkg.extra"} <= explicit_imports
    assert helper in read_paths
    assert entry not in read_paths
    assert extra not in read_paths
    assert resolved_candidates == ["pkg.helper"]


def test_discover_module_graph_prunes_removed_persisted_dependency(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "pkg" / "__init__.py"
    entry.parent.mkdir()
    entry.write_text("import pkg.helper\nimport pkg.old\n")
    (entry.parent / "helper.py").write_text("VALUE = 1\n")
    old = entry.parent / "old.py"
    old.write_text("VALUE = 2\n")

    stdlib_root = cli._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()

    graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    assert "pkg.old" in graph

    entry.write_text("import pkg.helper\n")
    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )

    assert "pkg.old" not in graph
    assert "pkg.old" not in explicit_imports

    cache = cli._ModuleResolutionCache()

    def fail_resolve(*args: object, **kwargs: object) -> Path | None:
        raise AssertionError("unexpected module resolution")

    def fail_read(path: Path) -> str:
        raise AssertionError(f"unexpected source read for {path}")

    monkeypatch.setattr(cache, "resolve_module", fail_resolve)
    monkeypatch.setattr(cache, "read_module_source", fail_read)

    def fail_read_text(*args: object, **kwargs: object) -> str:
        raise AssertionError("unexpected persisted graph reread")

    monkeypatch.setattr(Path, "read_text", fail_read_text)

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
        resolver_cache=cache,
    )
    assert "pkg.helper" in explicit_imports
    assert "pkg" in graph


def test_resolved_artifact_hash_key_is_cached(tmp_path: Path) -> None:
    artifact = tmp_path / "dist" / "output.o"
    cli._resolved_artifact_hash_key.cache_clear()

    first = cli._resolved_artifact_hash_key(str(artifact))
    second = cli._resolved_artifact_hash_key(str(artifact))

    info = cli._resolved_artifact_hash_key.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_fingerprint_path_uses_cached_artifact_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact = tmp_path / "dist" / "backend"
    cli._resolved_artifact_hash_key.cache_clear()

    calls = 0
    original = cli._resolved_artifact_hash_key

    def wrapped(path_str: str) -> str:
        nonlocal calls
        calls += 1
        return original(path_str)

    monkeypatch.setattr(cli, "_resolved_artifact_hash_key", wrapped, raising=True)

    first = cli._backend_fingerprint_path(tmp_path, artifact, "dev-fast")
    second = cli._backend_fingerprint_path(tmp_path, artifact, "dev-fast")

    info = original.cache_info()
    assert first == second
    assert calls == 1
    assert info.currsize >= 1


def test_artifact_state_path_is_cached(tmp_path: Path) -> None:
    artifact = tmp_path / "dist" / "output.o"
    cli._artifact_state_path_cached.cache_clear()

    first = cli._artifact_state_path(
        tmp_path,
        artifact,
        subdir="backend_fingerprints",
        stem_suffix="dev-fast",
        extension="fingerprint",
    )
    second = cli._artifact_state_path(
        tmp_path,
        artifact,
        subdir="backend_fingerprints",
        stem_suffix="dev-fast",
        extension="fingerprint",
    )

    info = cli._artifact_state_path_cached.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_artifact_sync_state_path_uses_cached_artifact_state_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact = tmp_path / "dist" / "output.o"
    cli._artifact_state_path_cached.cache_clear()

    calls = 0
    original = cli._artifact_state_path_cached

    def wrapped(
        build_state_root_str: str,
        artifact_path_str: str,
        artifact_name: str,
        subdir: str,
        stem_suffix: str,
        extension: str,
    ) -> Path:
        nonlocal calls
        calls += 1
        return original(
            build_state_root_str,
            artifact_path_str,
            artifact_name,
            subdir,
            stem_suffix,
            extension,
        )

    monkeypatch.setattr(cli, "_artifact_state_path_cached", wrapped, raising=True)

    first = cli._artifact_sync_state_path(tmp_path, artifact)
    second = cli._artifact_sync_state_path(tmp_path, artifact)

    info = original.cache_info()
    assert first == second
    assert calls == 2
    assert info.hits >= 1


def test_build_state_subdir_is_cached(tmp_path: Path) -> None:
    cli._build_state_subdir_cached.cache_clear()

    build_state_root = cli._build_state_root(tmp_path)
    first = cli._build_state_subdir_cached(str(build_state_root), "module_graph_cache")
    second = cli._build_state_subdir_cached(str(build_state_root), "module_graph_cache")

    info = cli._build_state_subdir_cached.cache_info()
    assert first == second == (build_state_root / "module_graph_cache")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_module_analysis_cache_path_uses_cached_build_state_subdir(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._build_state_subdir_cached.cache_clear()

    calls = 0
    original = cli._build_state_subdir_cached

    def wrapped(build_state_root_str: str, subdir: str) -> Path:
        nonlocal calls
        calls += 1
        return original(build_state_root_str, subdir)

    monkeypatch.setattr(cli, "_build_state_subdir_cached", wrapped, raising=True)

    first = cli._module_analysis_cache_path(
        tmp_path,
        tmp_path / "pkg.py",
        module_name="pkg",
        is_package=False,
    )
    second = cli._module_analysis_cache_path(
        tmp_path,
        tmp_path / "pkg.py",
        module_name="pkg",
        is_package=False,
    )

    info = original.cache_info()
    assert first == second
    assert calls == 2
    assert info.hits >= 1


def test_resolved_module_cache_key_is_cached(tmp_path: Path) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    cli._resolved_module_cache_key.cache_clear()

    first = cli._resolved_module_cache_key(
        str(module_path), "pkg.mod", "mod", "module_analysis_cache"
    )
    second = cli._resolved_module_cache_key(
        str(module_path), "pkg.mod", "mod", "module_analysis_cache"
    )

    info = cli._resolved_module_cache_key.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_module_analysis_cache_path_uses_cached_module_key(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    cli._resolved_module_cache_key.cache_clear()

    calls = 0
    original = cli._resolved_module_cache_key

    def wrapped(path_str: str, *parts: str) -> str:
        nonlocal calls
        calls += 1
        return original(path_str, *parts)

    monkeypatch.setattr(cli, "_resolved_module_cache_key", wrapped, raising=True)

    first = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
    )
    second = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
    )

    info = original.cache_info()
    assert first == second
    assert calls == 2
    assert info.hits >= 1


def test_module_graph_cache_key_is_cached(tmp_path: Path) -> None:
    entry_path = tmp_path / "main.py"
    roots = (str(tmp_path),)
    module_roots = (str(tmp_path / "src"),)
    stdlib_root = str(tmp_path / "stdlib")
    cli._module_graph_cache_key.cache_clear()

    first = cli._module_graph_cache_key(
        str(entry_path),
        roots,
        module_roots,
        stdlib_root,
        ("warnings",),
        ("asyncio",),
        ("tkinter",),
    )
    second = cli._module_graph_cache_key(
        str(entry_path),
        roots,
        module_roots,
        stdlib_root,
        ("warnings",),
        ("asyncio",),
        ("tkinter",),
    )

    info = cli._module_graph_cache_key.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_module_graph_cache_path_uses_cached_graph_key(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry_path = tmp_path / "main.py"
    roots = [tmp_path]
    module_roots = [tmp_path / "src"]
    stdlib_root = tmp_path / "stdlib"
    cli._module_graph_cache_key.cache_clear()

    calls = 0
    original = cli._module_graph_cache_key

    def wrapped(
        entry_path_str: str,
        roots_key: tuple[str, ...],
        module_roots_key: tuple[str, ...],
        stdlib_root_str: str,
        skip_modules: tuple[str, ...],
        stub_parents: tuple[str, ...],
        nested_stdlib_scan_modules: tuple[str, ...],
    ) -> str:
        nonlocal calls
        calls += 1
        return original(
            entry_path_str,
            roots_key,
            module_roots_key,
            stdlib_root_str,
            skip_modules,
            stub_parents,
            nested_stdlib_scan_modules,
        )

    monkeypatch.setattr(cli, "_module_graph_cache_key", wrapped, raising=True)

    first = cli._module_graph_cache_path(
        tmp_path,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules={"warnings"},
        stub_parents={"asyncio"},
        nested_stdlib_scan_modules={"tkinter"},
    )
    second = cli._module_graph_cache_path(
        tmp_path,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules={"warnings"},
        stub_parents={"asyncio"},
        nested_stdlib_scan_modules={"tkinter"},
    )

    info = original.cache_info()
    assert first == second
    assert calls == 2
    assert info.hits >= 1


def test_cargo_target_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", "external-target")
    monkeypatch.chdir(tmp_path)

    first = cli._cargo_target_root(tmp_path)
    second = cli._cargo_target_root(tmp_path)

    info = cli._cargo_target_root_cached.cache_info()
    assert first == second == (tmp_path / "external-target")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_build_state_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._build_state_root_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", "external-target")
    monkeypatch.chdir(tmp_path)

    first = cli._build_state_root(tmp_path)
    second = cli._build_state_root(tmp_path)

    info = cli._build_state_root_cached.cache_info()
    assert first == second == (tmp_path / "external-target" / ".molt_state")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_build_state_root_uses_override_relative_to_project_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._build_state_root_cached.cache_clear()
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", "state-dir")
    other = tmp_path / "other"
    other.mkdir()
    monkeypatch.chdir(other)

    state_root = cli._build_state_root(tmp_path)

    assert state_root == (tmp_path / "state-dir")


def test_lock_check_cache_path_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._lock_check_cache_path_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", "external-target")

    first = cli._lock_check_cache_path(tmp_path, "cargo")
    second = cli._lock_check_cache_path(tmp_path, "cargo")

    info = cli._lock_check_cache_path_cached.cache_info()
    assert (
        first == second == (tmp_path / "external-target" / "lock_checks" / "cargo.json")
    )
    assert info.hits >= 1
    assert info.currsize >= 1


def test_verify_cargo_lock_uses_workspace_member_manifests_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root_manifest = tmp_path / "Cargo.toml"
    root_manifest.write_text(
        '[workspace]\n'
        'members = ["runtime/molt-runtime", "runtime/molt-backend"]\n'
        'resolver = "2"\n'
    )
    (tmp_path / "Cargo.lock").write_text("# lock\n")
    runtime_manifest = tmp_path / "runtime" / "molt-runtime" / "Cargo.toml"
    runtime_manifest.parent.mkdir(parents=True)
    runtime_manifest.write_text('[package]\nname = "molt-runtime"\nversion = "0.1.0"\n')
    backend_manifest = tmp_path / "runtime" / "molt-backend" / "Cargo.toml"
    backend_manifest.parent.mkdir(parents=True)
    backend_manifest.write_text('[package]\nname = "molt-backend"\nversion = "0.1.0"\n')
    stray_manifest = tmp_path / "scratch" / "Cargo.toml"
    stray_manifest.parent.mkdir(parents=True)
    stray_manifest.write_text('[package]\nname = "scratch"\nversion = "0.1.0"\n')

    captured: dict[str, list[Path]] = {}

    monkeypatch.setattr(
        cli.shutil,
        "which",
        lambda name: "/usr/bin/cargo" if name == "cargo" else None,
    )
    monkeypatch.setattr(
        cli,
        "_lock_check_inputs",
        lambda project_root, paths: captured.setdefault("paths", list(paths)) or {},
    )
    monkeypatch.setattr(cli, "_is_lock_check_cache_valid", lambda *args, **kwargs: True)

    assert cli._verify_cargo_lock(tmp_path) is None
    assert "paths" in captured
    assert root_manifest in captured["paths"]
    assert runtime_manifest in captured["paths"]
    assert backend_manifest in captured["paths"]
    assert tmp_path / "Cargo.lock" in captured["paths"]
    assert stray_manifest not in captured["paths"]
    assert len(captured["paths"]) == 4


def test_build_lock_dir_is_cached(tmp_path: Path) -> None:
    cli._build_lock_dir_cached.cache_clear()

    build_state_root = cli._build_state_root(tmp_path)
    first = cli._build_lock_dir_cached(str(tmp_path), str(build_state_root))
    second = cli._build_lock_dir_cached(str(tmp_path), str(build_state_root))

    info = cli._build_lock_dir_cached.cache_info()
    assert first == second == (build_state_root / "build_locks")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_runtime_source_paths_are_cached(tmp_path: Path) -> None:
    cli._runtime_source_paths_cached.cache_clear()

    first = cli._runtime_source_paths(tmp_path)
    second = cli._runtime_source_paths(tmp_path)

    info = cli._runtime_source_paths_cached.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_source_paths_are_cached(tmp_path: Path) -> None:
    cli._backend_source_paths_cached.cache_clear()

    first = cli._backend_source_paths(tmp_path, ("wasm-backend",))
    second = cli._backend_source_paths(tmp_path, ("wasm-backend",))

    info = cli._backend_source_paths_cached.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_source_paths_are_feature_aware(tmp_path: Path) -> None:
    native_paths = {
        path.relative_to(tmp_path).as_posix()
        for path in cli._backend_source_paths(tmp_path, ())
    }
    wasm_paths = {
        path.relative_to(tmp_path).as_posix()
        for path in cli._backend_source_paths(tmp_path, ("wasm-backend",))
    }
    rust_paths = {
        path.relative_to(tmp_path).as_posix()
        for path in cli._backend_source_paths(tmp_path, ("rust-backend",))
    }

    assert "runtime/molt-backend/src/luau.rs" not in native_paths
    assert "runtime/molt-backend/src/rust.rs" not in native_paths
    assert "runtime/molt-backend/src/wasm.rs" not in native_paths

    assert "runtime/molt-backend/src/wasm.rs" in wasm_paths
    assert "runtime/molt-backend/src/rust.rs" not in wasm_paths
    assert "runtime/molt-backend/src/luau.rs" not in wasm_paths

    assert "runtime/molt-backend/src/rust.rs" in rust_paths
    assert "runtime/molt-backend/src/wasm.rs" not in rust_paths
    assert "runtime/molt-backend/src/luau.rs" not in rust_paths


def test_backend_bin_path_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._backend_bin_path_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", "external-target")
    monkeypatch.chdir(tmp_path)

    first = cli._backend_bin_path(tmp_path, "dev-fast")
    second = cli._backend_bin_path(tmp_path, "dev-fast")

    info = cli._backend_bin_path_cached.cache_info()
    assert (
        first == second == (tmp_path / "external-target" / "dev-fast" / "molt-backend")
    )
    assert info.hits >= 1
    assert info.currsize >= 1


def test_cargo_profile_dir_is_cached() -> None:
    cli._cargo_profile_dir.cache_clear()

    first = cli._cargo_profile_dir("dev")
    second = cli._cargo_profile_dir("dev")

    info = cli._cargo_profile_dir.cache_info()
    assert first == second == "debug"
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_env_path_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._resolve_env_path_cached.cache_clear()
    monkeypatch.setenv("MOLT_TEST_PATH", "relative-root")
    monkeypatch.chdir(tmp_path)

    first = cli._resolve_env_path("MOLT_TEST_PATH", tmp_path / "fallback")
    second = cli._resolve_env_path("MOLT_TEST_PATH", tmp_path / "fallback")

    info = cli._resolve_env_path_cached.cache_info()
    assert first == second == (tmp_path / "relative-root")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_default_molt_cache_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._default_molt_cache_cached.cache_clear()
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache-root"))

    first = cli._default_molt_cache()
    second = cli._default_molt_cache()

    info = cli._default_molt_cache_cached.cache_info()
    assert first == second == (tmp_path / "cache-root")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_default_molt_home_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._default_molt_home_cached.cache_clear()
    monkeypatch.setenv("MOLT_HOME", str(tmp_path / "home-root"))

    first = cli._default_molt_home()
    second = cli._default_molt_home()

    info = cli._default_molt_home_cached.cache_info()
    assert first == second == (tmp_path / "home-root")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_default_molt_bin_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._default_molt_bin_cached.cache_clear()
    monkeypatch.setenv("MOLT_BIN", str(tmp_path / "bin-root"))

    first = cli._default_molt_bin()
    second = cli._default_molt_bin()

    info = cli._default_molt_bin_cached.cache_info()
    assert first == second == (tmp_path / "bin-root")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_wasm_runtime_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._wasm_runtime_root_cached.cache_clear()
    monkeypatch.setenv("MOLT_WASM_RUNTIME_DIR", str(tmp_path / "wasm-root"))

    first = cli._wasm_runtime_root(tmp_path)
    second = cli._wasm_runtime_root(tmp_path)

    info = cli._wasm_runtime_root_cached.cache_info()
    assert first == second == (tmp_path / "wasm-root")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_safe_output_base_is_cached() -> None:
    cli._safe_output_base.cache_clear()

    first = cli._safe_output_base("hello/world.py")
    second = cli._safe_output_base("hello/world.py")

    info = cli._safe_output_base.cache_info()
    assert first == second == "hello_world.py"
    assert info.hits >= 1
    assert info.currsize >= 1


def test_default_build_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._default_build_root_cached.cache_clear()
    monkeypatch.setenv("MOLT_HOME", str(tmp_path / "home-root"))

    first = cli._default_build_root("hello/world.py")
    second = cli._default_build_root("hello/world.py")

    info = cli._default_build_root_cached.cache_info()
    assert first == second == (tmp_path / "home-root" / "build" / "hello_world.py")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_cache_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._resolve_cache_root_cached.cache_clear()

    first = cli._resolve_cache_root(tmp_path, "cache-dir")
    second = cli._resolve_cache_root(tmp_path, "cache-dir")

    info = cli._resolve_cache_root_cached.cache_info()
    assert first == second == (tmp_path / "cache-dir")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_out_dir_is_cached(tmp_path: Path) -> None:
    cli._resolve_out_dir_cached.cache_clear()

    first = cli._resolve_out_dir(tmp_path, "dist")
    second = cli._resolve_out_dir(tmp_path, "dist")

    info = cli._resolve_out_dir_cached.cache_info()
    assert first == second == (tmp_path / "dist")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_sysroot_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._resolve_sysroot_cached.cache_clear()
    monkeypatch.setenv("MOLT_SYSROOT", "sdk-root")

    first = cli._resolve_sysroot(tmp_path, None)
    second = cli._resolve_sysroot(tmp_path, None)

    info = cli._resolve_sysroot_cached.cache_info()
    assert first == second == (tmp_path / "sdk-root")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_runtime_lib_path_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._runtime_lib_path_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", "external-target")
    monkeypatch.chdir(tmp_path)

    first = cli._runtime_lib_path(tmp_path, "dev-fast", None)
    second = cli._runtime_lib_path(tmp_path, "dev-fast", None)

    info = cli._runtime_lib_path_cached.cache_info()
    assert (
        first
        == second
        == (tmp_path / "external-target" / "dev-fast" / "libmolt_runtime.a")
    )
    assert info.hits >= 1
    assert info.currsize >= 1


def test_runtime_lib_path_includes_target_triple(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._runtime_lib_path_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", "external-target")
    monkeypatch.chdir(tmp_path)

    runtime_lib = cli._runtime_lib_path(tmp_path, "release", "aarch64-apple-darwin")

    assert runtime_lib == (
        tmp_path
        / "external-target"
        / "aarch64-apple-darwin"
        / "release"
        / "libmolt_runtime.a"
    )


def test_runtime_wasm_artifact_path_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._runtime_wasm_artifact_path_cached.cache_clear()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))

    first = cli._runtime_wasm_artifact_path(tmp_path, "molt_runtime.wasm")
    second = cli._runtime_wasm_artifact_path(tmp_path, "molt_runtime.wasm")

    info = cli._runtime_wasm_artifact_path_cached.cache_info()
    assert first == second == (tmp_path / "wasm" / "molt_runtime.wasm")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_runtime_wasm_artifact_path_uses_explicit_override(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._runtime_wasm_artifact_path_cached.cache_clear()
    override = tmp_path / "custom-wasm"
    monkeypatch.setenv("MOLT_WASM_RUNTIME_DIR", str(override))

    runtime_wasm = cli._runtime_wasm_artifact_path(tmp_path, "molt_runtime_reloc.wasm")

    assert runtime_wasm == (override / "molt_runtime_reloc.wasm")


def test_load_module_imports_reuses_persisted_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n")
    source = cli._read_module_source(module_path)
    cache = cli._ModuleResolutionCache()
    tree = cache.parse_module_ast(module_path, source, filename=str(module_path))

    imports = cli._load_module_imports(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        tree=tree,
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert imports == ("warnings",)

    def fail_collect(*args: object, **kwargs: object) -> tuple[str, ...]:
        raise AssertionError("unexpected import scan")

    monkeypatch.setattr(cache, "collect_imports", fail_collect)
    cached_imports = cli._load_module_imports(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        tree=tree,
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert cached_imports == ("warnings",)


def test_load_module_analysis_reuses_persisted_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n\ndef f(a, *, b=1):\n    return a + b\n")
    source = cli._read_module_source(module_path)
    cache = cli._ModuleResolutionCache()

    (
        tree,
        imports,
        func_defaults,
        cached_source,
        cache_hit,
        interface_changed,
        path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert tree is not None
    assert imports == ("warnings",)
    assert "f" in func_defaults
    assert cached_source == source
    assert cache_hit is False
    assert interface_changed is True
    assert path_stat is not None

    def fail_parse(*args: object, **kwargs: object) -> ast.AST:
        raise AssertionError("unexpected parse")

    monkeypatch.setattr(cache, "parse_module_ast", fail_parse)
    (
        cached_tree,
        cached_imports,
        cached_defaults,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ("warnings",)
    assert cached_defaults == func_defaults
    assert cached_source is None
    assert cache_hit is True
    assert interface_changed is False
    assert cached_path_stat is not None


def test_load_module_analysis_persists_bytes_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("def f(blob=b'abc'):\n    return blob\n")
    source = cli._read_module_source(module_path)
    cache = cli._ModuleResolutionCache()

    (
        tree,
        imports,
        func_defaults,
        cached_source,
        cache_hit,
        interface_changed,
        path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert tree is not None
    assert imports == ()
    assert func_defaults == {
        "f": {
            "params": 1,
            "defaults": [{"const": True, "value": b"abc"}],
        }
    }
    assert cached_source == source
    assert cache_hit is False
    assert interface_changed is True
    assert path_stat is not None

    def fail_parse(*args: object, **kwargs: object) -> ast.AST:
        raise AssertionError("unexpected parse")

    monkeypatch.setattr(cache, "parse_module_ast", fail_parse)
    (
        cached_tree,
        cached_imports,
        cached_defaults,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ()
    assert cached_defaults == func_defaults
    assert cached_source is None
    assert cache_hit is True
    assert interface_changed is False
    assert cached_path_stat is not None


def test_load_module_analysis_reuses_persisted_module_analysis_imports(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n\ndef f(a, *, b=1):\n    return a + b\n")
    source = cli._read_module_source(module_path)
    cache = cli._ModuleResolutionCache()

    cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    def fail_import_scan(*args: object, **kwargs: object) -> tuple[str, ...] | None:
        raise AssertionError("unexpected persisted import-scan read")

    monkeypatch.setattr(cli, "_read_persisted_import_scan", fail_import_scan)
    monkeypatch.setattr(
        cache,
        "parse_module_ast",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected parse")
        ),
    )

    (
        cached_tree,
        cached_imports,
        cached_defaults,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ("warnings",)
    assert "f" in cached_defaults
    assert cached_source is None
    assert cache_hit is True
    assert interface_changed is False
    assert cached_path_stat is not None


def test_load_module_analysis_reuses_single_module_stat_for_persisted_hits(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n\ndef f(a, *, b=1):\n    return a + b\n")
    source = cli._read_module_source(module_path)
    cache = cli._ModuleResolutionCache()

    cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    original_path_stat = cache.path_stat
    calls = 0

    def wrapped_path_stat(path: Path) -> os.stat_result:
        nonlocal calls
        calls += 1
        return original_path_stat(path)

    monkeypatch.setattr(cache, "path_stat", wrapped_path_stat)
    monkeypatch.setattr(
        cache,
        "parse_module_ast",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected parse")
        ),
    )

    (
        cached_tree,
        cached_imports,
        cached_defaults,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ("warnings",)
    assert "f" in cached_defaults
    assert cached_source is None
    assert calls == 1
    assert cache_hit is True
    assert interface_changed is False
    assert cached_path_stat is not None


def test_load_module_analysis_marks_body_only_edit_as_interface_stable(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n\ndef f(a, *, b=1):\n    return a + b\n")
    cache = cli._ModuleResolutionCache()

    cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    module_path.write_text(
        "import warnings\n\ndef f(a, *, b=1):\n    total = a + b\n    return total\n"
    )
    cache = cli._ModuleResolutionCache()

    (
        tree,
        imports,
        func_defaults,
        cached_source,
        cache_hit,
        interface_changed,
        path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        include_nested=True,
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert tree is not None
    assert imports == ("warnings",)
    assert "f" in func_defaults
    assert cached_source is not None
    assert cache_hit is False
    assert interface_changed is False
    assert path_stat is not None


def test_persisted_module_lowering_roundtrip_respects_context_digest(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("x = ...\n")
    context_digest = cli._module_lowering_context_digest({"module": "pkg", "v": 1})
    assert context_digest is not None
    result = {
        "functions": [],
        "func_code_ids": {},
        "local_class_names": [],
        "local_classes": {},
        "midend_policy_outcomes_by_function": {},
        "midend_pass_stats_by_function": {},
        "timings": {"visit_s": 1.0, "lower_s": 2.0, "total_s": 3.0},
        "default_marker": Ellipsis,
    }

    cli._write_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
        result=result,
    )

    cached = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
    )
    assert cached is not None
    assert cached["default_marker"] is Ellipsis

    miss = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest="other",
    )
    assert miss is None


def test_persisted_module_lowering_reuses_process_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("x = 1\n")
    context_digest = cli._module_lowering_context_digest({"module": "pkg", "v": 1})
    assert context_digest is not None
    cli._PERSISTED_JSON_OBJECT_CACHE.clear()
    cli._write_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
        result={
            "functions": [],
            "func_code_ids": {},
            "local_class_names": [],
            "local_classes": {},
            "midend_policy_outcomes_by_function": {},
            "midend_pass_stats_by_function": {},
            "timings": {"visit_s": 0.0, "lower_s": 0.0, "total_s": 0.0},
        },
    )

    first = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
    )

    def fail_read_text(*args: object, **kwargs: object) -> str:
        raise AssertionError("unexpected persisted-lowering file read")

    monkeypatch.setattr(Path, "read_text", fail_read_text)
    second = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
    )

    assert first == second
    assert first is not second


def test_persisted_module_lowering_returns_isolated_mutable_results(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("x = 1\n")
    context_digest = cli._module_lowering_context_digest({"module": "pkg", "v": 1})
    assert context_digest is not None
    cli._PERSISTED_JSON_OBJECT_CACHE.clear()
    cli._write_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
        result={
            "functions": [
                {"name": "molt_main", "ops": [{"kind": "code_slot_set", "value": 1}]}
            ],
            "func_code_ids": {"molt_main": 1},
            "local_class_names": [],
            "local_classes": {},
            "midend_policy_outcomes_by_function": {},
            "midend_pass_stats_by_function": {},
            "timings": {"visit_s": 0.0, "lower_s": 0.0, "total_s": 0.0},
        },
    )

    first = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
    )
    second = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
    )

    assert first is not None
    assert second is not None
    first["functions"][0]["ops"][0]["value"] = 99
    assert second["functions"][0]["ops"][0]["value"] == 1


def test_prepare_frontend_parallel_batch_reuses_precomputed_context_digest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "alpha.py"
    module_path.write_text("VALUE = 1\n")
    project_root = tmp_path
    context_payload_calls = 0

    def fake_context_payload(
        *args: object, **kwargs: object
    ) -> dict[str, object] | None:
        del args, kwargs
        nonlocal context_payload_calls
        context_payload_calls += 1
        return {"module": "alpha"}

    monkeypatch.setattr(cli, "_module_lowering_context_payload", fake_context_payload)
    monkeypatch.setattr(
        cli, "_module_lowering_context_digest", lambda payload: "digest"
    )

    def fake_read(
        root: Path,
        path: Path,
        *,
        module_name: str,
        is_package: bool,
        context_digest: str,
        path_stat: os.stat_result | None = None,
    ) -> dict[str, object] | None:
        assert root == project_root
        assert path == module_path
        assert module_name == "alpha"
        assert is_package is False
        assert context_digest == "digest"
        assert path_stat is not None
        return {"module": module_name, "kind": "cached"}

    monkeypatch.setattr(cli, "_read_persisted_module_lowering", fake_read)
    module_graph_metadata = cli._build_module_graph_metadata(
        {"alpha": module_path},
        generated_module_source_paths={},
        entry_module="__main__",
        namespace_module_names=set(),
    )

    cached_results, worker_payloads, context_digest_by_module, batch_error = (
        cli._prepare_frontend_parallel_batch(
            ["alpha"],
            module_graph={"alpha": module_path},
            module_sources={},
            project_root=project_root,
            known_classes_snapshot={},
            module_resolution_cache=cli._ModuleResolutionCache(),
            parse_codec="json",
            type_hint_policy="ignore",
            fallback_policy="error",
            type_facts=None,
            enable_phi=True,
            known_modules={"alpha"},
            stdlib_allowlist=set(),
            known_func_defaults={},
            module_deps={"alpha": set()},
            module_chunk_max_ops=0,
            optimization_profile="dev",
            pgo_hot_function_names=set(),
            known_modules_sorted=("alpha",),
            stdlib_allowlist_sorted=(),
            pgo_hot_function_names_sorted=(),
            module_dep_closures={"alpha": frozenset({"alpha"})},
            module_graph_metadata=module_graph_metadata,
            module_chunking=False,
            dirty_lowering_modules=set(),
        )
    )

    assert batch_error is None
    assert worker_payloads == []
    assert cached_results == {"alpha": {"module": "alpha", "kind": "cached"}}
    assert context_digest_by_module == {"alpha": "digest"}
    assert context_payload_calls == 1


def test_load_cached_module_lowering_result_reuses_single_module_stat(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "alpha.py"
    module_path.write_text("VALUE = 1\n")
    cache = cli._ModuleResolutionCache()
    context_digest = cli._module_lowering_context_digest({"module": "alpha", "v": 1})
    assert context_digest is not None

    cli._write_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="alpha",
        is_package=False,
        context_digest=context_digest,
        result={
            "functions": [],
            "func_code_ids": {},
            "local_class_names": [],
            "local_classes": {},
            "midend_policy_outcomes_by_function": {},
            "midend_pass_stats_by_function": {},
            "timings": {"visit_s": 0.0, "lower_s": 0.0, "total_s": 0.0},
        },
    )

    original_path_stat = cache.path_stat
    calls = 0

    def wrapped_path_stat(path: Path) -> os.stat_result:
        nonlocal calls
        calls += 1
        return original_path_stat(path)

    monkeypatch.setattr(cache, "path_stat", wrapped_path_stat)

    result = cli._load_cached_module_lowering_result(
        tmp_path,
        "alpha",
        module_path,
        logical_source_path=str(module_path),
        entry_override=None,
        is_package=False,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"alpha"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"alpha": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        known_modules_sorted=("alpha",),
        stdlib_allowlist_sorted=(),
        pgo_hot_function_names_sorted=(),
        context_digest=context_digest,
        resolution_cache=cache,
    )

    assert result is not None
    assert result["functions"] == []
    assert calls == 1


def test_dependent_module_closure_tracks_reverse_frontier() -> None:
    module_deps = {
        "main": {"alpha", "beta"},
        "alpha": {"leaf"},
        "beta": set(),
        "leaf": set(),
    }

    closure = cli._dependent_module_closure(
        {"leaf"},
        module_deps,
        {"main", "alpha", "beta", "leaf"},
    )

    assert closure == {"leaf", "alpha", "main"}


def test_reverse_module_dependencies_maps_dependents_once() -> None:
    module_deps = {
        "main": {"alpha", "beta"},
        "alpha": {"leaf"},
        "beta": set(),
        "leaf": set(),
    }

    reverse = cli._reverse_module_dependencies(
        module_deps,
        {"main", "alpha", "beta", "leaf"},
    )

    assert reverse["leaf"] == {"alpha"}
    assert reverse["alpha"] == {"main"}
    assert reverse["beta"] == {"main"}
    assert reverse["main"] == set()


def test_dependent_module_closure_reuses_precomputed_reverse_frontier() -> None:
    module_deps = {
        "main": {"alpha", "beta"},
        "alpha": {"leaf"},
        "beta": set(),
        "leaf": set(),
    }
    reverse = cli._reverse_module_dependencies(
        module_deps,
        {"main", "alpha", "beta", "leaf"},
    )

    closure = cli._dependent_module_closure(
        {"leaf"},
        module_deps,
        {"main", "alpha", "beta", "leaf"},
        reverse_module_deps=reverse,
    )

    assert closure == {"leaf", "alpha", "main"}


def test_module_dependency_closure_tracks_forward_dependencies() -> None:
    module_deps = {
        "main": {"alpha", "beta"},
        "alpha": {"leaf"},
        "beta": set(),
        "leaf": set(),
    }

    closure = cli._module_dependency_closure("main", module_deps)

    assert closure == {"main", "alpha", "beta", "leaf"}


def test_module_dependency_closures_reuse_topological_order_when_acyclic() -> None:
    module_deps = {
        "main": {"alpha", "beta"},
        "alpha": {"leaf"},
        "beta": set(),
        "leaf": set(),
    }

    closures = cli._module_dependency_closures(
        module_deps,
        {"main", "alpha", "beta", "leaf"},
        module_order=["leaf", "alpha", "beta", "main"],
        has_back_edges=False,
    )

    assert closures["leaf"] == frozenset({"leaf"})
    assert closures["alpha"] == frozenset({"alpha", "leaf"})
    assert closures["beta"] == frozenset({"beta"})
    assert closures["main"] == frozenset({"main", "alpha", "beta", "leaf"})


def test_module_dependency_closures_fallback_on_back_edges() -> None:
    module_deps = {
        "a": {"b"},
        "b": {"a"},
    }

    closures = cli._module_dependency_closures(
        module_deps,
        {"a", "b"},
        module_order=["a", "b"],
        has_back_edges=True,
    )

    assert closures["a"] == frozenset({"a", "b"})
    assert closures["b"] == frozenset({"a", "b"})


def test_module_dependencies_from_imports_resolves_direct_graph_edges() -> None:
    module_graph = {
        "main": Path("/tmp/main.py"),
        "alpha": Path("/tmp/alpha.py"),
        "beta": Path("/tmp/beta.py"),
        "warnings": Path("/tmp/warnings.py"),
    }

    deps = cli._module_dependencies_from_imports(
        "main",
        module_graph,
        ["alpha.helper", "beta", "molt.stdlib.warnings"],
    )

    assert deps == {"alpha", "beta", "warnings"}


def test_module_lowering_context_payload_ignores_unrelated_func_defaults() -> None:
    payload = cli._module_lowering_context_payload(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override=None,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"main", "alpha", "beta"},
        stdlib_allowlist=set(),
        known_func_defaults={
            "main": {"run": {"params": 0, "defaults": []}},
            "alpha": {"helper": {"params": 1, "defaults": []}},
            "beta": {"unused": {"params": 2, "defaults": []}},
        },
        module_deps={"main": {"alpha"}, "alpha": set(), "beta": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        known_modules_sorted=("alpha", "beta", "main"),
        stdlib_allowlist_sorted=(),
        pgo_hot_function_names_sorted=(),
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert payload is not None
    assert set(payload["known_func_defaults"]) == {"main", "alpha"}
    assert "beta" not in payload["known_func_defaults"]


def test_module_lowering_context_payload_scopes_known_modules_and_hot_functions() -> (
    None
):
    payload = cli._module_lowering_context_payload(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override=None,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"main", "alpha", "beta", "unrelated"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": {"alpha", "beta"}, "alpha": set(), "beta": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names={
            "main::hot_func",
            "main.hot_attr",
            "beta::irrelevant",
            "unrelated::nope",
        },
        known_modules_sorted=("alpha", "beta", "main", "unrelated"),
        stdlib_allowlist_sorted=(),
        pgo_hot_function_names_sorted=(
            "beta::irrelevant",
            "main.hot_attr",
            "main::hot_func",
            "unrelated::nope",
        ),
        module_dep_closures={"main": frozenset({"main", "alpha", "beta"})},
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert payload is not None
    assert tuple(payload["known_modules"]) == ("alpha", "beta", "main")
    assert tuple(payload["pgo_hot_functions"]) == ("main.hot_attr", "main::hot_func")


def test_module_lowering_context_payload_scopes_known_classes() -> None:
    payload = cli._module_lowering_context_payload(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override=None,
        known_classes_snapshot={
            "MainClass": {"module": "main", "fields": {}},
            "DepClass": {"module": "alpha", "fields": {}},
            "UnrelatedClass": {"module": "unrelated", "fields": {}},
        },
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"main", "alpha", "unrelated"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": {"alpha"}, "alpha": set(), "unrelated": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        known_modules_sorted=("alpha", "main", "unrelated"),
        stdlib_allowlist_sorted=(),
        pgo_hot_function_names_sorted=(),
        module_dep_closures={"main": frozenset({"main", "alpha"})},
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert payload is not None
    assert set(payload["known_classes"]) == {"MainClass", "DepClass"}
    assert "UnrelatedClass" not in payload["known_classes"]


def test_module_worker_payload_scopes_parallel_lowering_inputs() -> None:
    payload = cli._module_worker_payload(
        "main",
        module_path=Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        source="import alpha\n",
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        module_is_namespace=False,
        entry_module=None,
        type_facts=None,
        enable_phi=True,
        known_modules=("alpha", "main", "unrelated"),
        known_classes_snapshot={
            "MainClass": {"module": "main", "fields": {}},
            "DepClass": {"module": "alpha", "fields": {}},
            "UnrelatedClass": {"module": "unrelated", "fields": {}},
        },
        stdlib_allowlist_sorted=("json",),
        known_func_defaults={
            "main": {"run": {"params": 0, "defaults": []}},
            "alpha": {"helper": {"params": 1, "defaults": []}},
            "unrelated": {"unused": {"params": 0, "defaults": []}},
        },
        module_deps={"main": {"alpha"}, "alpha": set(), "unrelated": set()},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=("main::hot", "unrelated::cold"),
        module_dep_closures={"main": frozenset({"main", "alpha"})},
    )

    assert payload["known_modules"] == ["alpha", "main"]
    assert set(payload["known_classes"]) == {"MainClass", "DepClass"}
    assert set(payload["known_func_defaults"]) == {"main", "alpha"}
    assert payload["pgo_hot_functions"] == ["main::hot"]


def test_module_worker_payload_reuses_prebuilt_stdlib_allowlist() -> None:
    stdlib_allowlist_payload = ["json"]
    payload = cli._module_worker_payload(
        "main",
        module_path=Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        source="VALUE = 1\n",
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        module_is_namespace=False,
        entry_module=None,
        type_facts=None,
        enable_phi=True,
        known_modules=("main",),
        known_classes_snapshot={},
        stdlib_allowlist_sorted=("json",),
        stdlib_allowlist_payload=stdlib_allowlist_payload,
        known_func_defaults={},
        module_deps={"main": set()},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=(),
        module_dep_closures={"main": frozenset({"main"})},
    )

    assert payload["stdlib_allowlist"] is stdlib_allowlist_payload


def test_build_scoped_known_classes_snapshot_precomputes_parallel_views() -> None:
    scoped = cli._build_scoped_known_classes_snapshot(
        {"main", "alpha"},
        module_deps={"main": {"alpha"}, "alpha": set(), "unrelated": set()},
        module_dep_closures={
            "main": frozenset({"main", "alpha"}),
            "alpha": frozenset({"alpha"}),
        },
        known_classes_snapshot={
            "MainClass": {"module": "main", "fields": {}},
            "DepClass": {"module": "alpha", "fields": {}},
            "UnrelatedClass": {"module": "unrelated", "fields": {}},
        },
    )

    assert set(scoped["main"]) == {"MainClass", "DepClass"}
    assert set(scoped["alpha"]) == {"DepClass"}


def test_scoped_known_classes_view_reuses_precomputed_snapshot() -> None:
    scoped = {
        "main": {
            "MainClass": {"module": "main", "fields": {}},
            "DepClass": {"module": "alpha", "fields": {}},
        }
    }

    resolved = cli._scoped_known_classes_view(
        "main",
        module_deps={"main": {"alpha"}, "alpha": set()},
        known_classes_snapshot={
            "MainClass": {"module": "main", "fields": {}},
            "DepClass": {"module": "alpha", "fields": {}},
        },
        module_dep_closures={"main": frozenset({"main", "alpha"})},
        scoped_known_classes_by_module=scoped,
    )

    assert resolved is scoped["main"]


def test_prepare_frontend_parallel_batch_precomputes_scoped_known_classes_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_graph = {
        "main": tmp_path / "main.py",
        "alpha": tmp_path / "alpha.py",
    }
    module_sources = {
        "main": "import alpha\n",
        "alpha": "VALUE = 1\n",
    }
    for path, source in zip(
        module_graph.values(), module_sources.values(), strict=False
    ):
        path.write_text(source)

    original = cli._scoped_known_classes
    calls = 0

    def wrapped_scoped_known_classes(
        *args: object, **kwargs: object
    ) -> dict[str, object]:
        nonlocal calls
        calls += 1
        return original(*args, **kwargs)

    monkeypatch.setattr(cli, "_scoped_known_classes", wrapped_scoped_known_classes)
    monkeypatch.setattr(
        cli, "_load_cached_module_lowering_result", lambda *args, **kwargs: None
    )
    module_graph_metadata = cli._build_module_graph_metadata(
        module_graph,
        generated_module_source_paths={},
        entry_module="__main__",
        namespace_module_names=set(),
    )

    cached_results, worker_payloads, context_digest_by_module, batch_error = (
        cli._prepare_frontend_parallel_batch(
            ["main", "alpha"],
            module_graph=module_graph,
            module_sources=module_sources,
            project_root=tmp_path,
            known_classes_snapshot={
                "MainClass": {"module": "main", "fields": {}},
                "DepClass": {"module": "alpha", "fields": {}},
                "UnrelatedClass": {"module": "unrelated", "fields": {}},
            },
            module_resolution_cache=cli._ModuleResolutionCache(),
            parse_codec="json",
            type_hint_policy="ignore",
            fallback_policy="error",
            type_facts=None,
            enable_phi=True,
            known_modules={"main", "alpha"},
            stdlib_allowlist=set(),
            known_func_defaults={},
            module_deps={"main": {"alpha"}, "alpha": set(), "unrelated": set()},
            module_chunk_max_ops=0,
            optimization_profile="dev",
            pgo_hot_function_names=set(),
            known_modules_sorted=("alpha", "main"),
            stdlib_allowlist_sorted=(),
            pgo_hot_function_names_sorted=(),
            module_dep_closures={
                "main": frozenset({"main", "alpha"}),
                "alpha": frozenset({"alpha"}),
            },
            module_graph_metadata=module_graph_metadata,
            module_chunking=False,
            dirty_lowering_modules={"main", "alpha"},
        )
    )

    assert batch_error is None
    assert cached_results == {}
    assert len(worker_payloads) == 2
    assert set(context_digest_by_module) == {"main", "alpha"}
    assert calls == 2


def test_module_lowering_context_payload_scopes_type_facts() -> None:
    type_facts = TypeFacts(
        modules={
            "main": ModuleFacts(
                globals={"VALUE": Fact(type="int", trust="trusted")},
                functions={
                    "run": FunctionFacts(
                        locals={"x": Fact(type="int", trust="trusted")}
                    )
                },
            ),
            "alpha": ModuleFacts(
                globals={"DEP": Fact(type="str", trust="trusted")},
            ),
            "unrelated": ModuleFacts(
                globals={"NOPE": Fact(type="bytes", trust="trusted")},
            ),
        }
    )

    payload = cli._module_lowering_context_payload(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override=None,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="trust",
        fallback_policy="error",
        type_facts=type_facts,
        enable_phi=True,
        known_modules={"main", "alpha", "unrelated"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": {"alpha"}, "alpha": set(), "unrelated": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        known_modules_sorted=("alpha", "main", "unrelated"),
        stdlib_allowlist_sorted=(),
        pgo_hot_function_names_sorted=(),
        module_dep_closures={"main": frozenset({"main", "alpha"})},
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert payload is not None
    scoped = payload["type_facts"]
    assert isinstance(scoped, TypeFacts)
    assert set(scoped.modules) == {"main", "alpha"}


def test_module_worker_payload_scopes_type_facts() -> None:
    type_facts = TypeFacts(
        modules={
            "main": ModuleFacts(globals={"VALUE": Fact(type="int", trust="trusted")}),
            "alpha": ModuleFacts(globals={"DEP": Fact(type="str", trust="trusted")}),
            "unrelated": ModuleFacts(
                globals={"NOPE": Fact(type="bytes", trust="trusted")}
            ),
        }
    )

    payload = cli._module_worker_payload(
        "main",
        module_path=Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        source="import alpha\n",
        parse_codec="json",
        type_hint_policy="trust",
        fallback_policy="error",
        module_is_namespace=False,
        entry_module=None,
        type_facts=type_facts,
        enable_phi=True,
        known_modules=("alpha", "main", "unrelated"),
        known_classes_snapshot={},
        stdlib_allowlist_sorted=("json",),
        known_func_defaults={},
        module_deps={"main": {"alpha"}, "alpha": set(), "unrelated": set()},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=(),
        module_dep_closures={"main": frozenset({"main", "alpha"})},
    )

    scoped = payload["type_facts"]
    assert isinstance(scoped, TypeFacts)
    assert set(scoped.modules) == {"main", "alpha"}


def test_build_scoped_lowering_inputs_precomputes_scoped_views() -> None:
    type_facts = TypeFacts(
        modules={
            "main": ModuleFacts(globals={"VALUE": Fact(type="int", trust="trusted")}),
            "alpha": ModuleFacts(globals={"DEP": Fact(type="str", trust="trusted")}),
            "unrelated": ModuleFacts(
                globals={"NOPE": Fact(type="bytes", trust="trusted")}
            ),
        }
    )

    scoped_lowering_inputs = cli._build_scoped_lowering_inputs(
        {"main", "alpha", "unrelated"},
        module_deps={"main": {"alpha"}, "alpha": set(), "unrelated": set()},
        module_dep_closures={
            "main": frozenset({"main", "alpha"}),
            "alpha": frozenset({"alpha"}),
            "unrelated": frozenset({"unrelated"}),
        },
        known_modules={"main", "alpha", "unrelated"},
        known_func_defaults={
            "main": {"run": {"params": 0, "defaults": []}},
            "alpha": {"helper": {"params": 1, "defaults": []}},
            "unrelated": {"unused": {"params": 0, "defaults": []}},
        },
        pgo_hot_function_names={"main::hot", "unrelated::cold"},
        type_facts=type_facts,
    )

    assert scoped_lowering_inputs.known_modules_by_module["main"] == ("alpha", "main")
    assert set(scoped_lowering_inputs.known_func_defaults_by_module["main"]) == {
        "main",
        "alpha",
    }
    assert scoped_lowering_inputs.pgo_hot_function_names_by_module["main"] == (
        "main::hot",
    )
    scoped_main_facts = scoped_lowering_inputs.type_facts_by_module["main"]
    assert isinstance(scoped_main_facts, TypeFacts)
    assert set(scoped_main_facts.modules) == {
        "main",
        "alpha",
    }


def test_scoped_lowering_input_view_reuses_precomputed_bundle() -> None:
    type_facts = TypeFacts(
        modules={
            "main": ModuleFacts(globals={"VALUE": Fact(type="int", trust="trusted")}),
            "alpha": ModuleFacts(globals={"DEP": Fact(type="str", trust="trusted")}),
        }
    )
    scoped_lowering_inputs = cli._build_scoped_lowering_inputs(
        {"main", "alpha"},
        module_deps={"main": {"alpha"}, "alpha": set()},
        module_dep_closures={
            "main": frozenset({"main", "alpha"}),
            "alpha": frozenset({"alpha"}),
        },
        known_modules={"main", "alpha"},
        known_func_defaults={
            "main": {"run": {"params": 0, "defaults": []}},
            "alpha": {"helper": {"params": 1, "defaults": []}},
        },
        pgo_hot_function_names={"main::hot"},
        type_facts=type_facts,
    )

    scoped_view = cli._scoped_lowering_input_view(
        "main",
        module_deps={"main": {"alpha"}, "alpha": set()},
        known_modules={"main", "alpha"},
        known_func_defaults={
            "main": {"run": {"params": 0, "defaults": []}},
            "alpha": {"helper": {"params": 1, "defaults": []}},
        },
        pgo_hot_function_names={"main::hot"},
        type_facts=type_facts,
        module_dep_closures={
            "main": frozenset({"main", "alpha"}),
            "alpha": frozenset({"alpha"}),
        },
        scoped_lowering_inputs=scoped_lowering_inputs,
        known_modules_sorted=("alpha", "main"),
        pgo_hot_function_names_sorted=("main::hot",),
    )

    assert scoped_view.known_modules is scoped_lowering_inputs.known_modules_by_module["main"]
    assert (
        scoped_view.known_func_defaults
        is scoped_lowering_inputs.known_func_defaults_by_module["main"]
    )
    assert (
        scoped_view.pgo_hot_function_names
        is scoped_lowering_inputs.pgo_hot_function_names_by_module["main"]
    )
    assert scoped_view.type_facts is scoped_lowering_inputs.type_facts_by_module["main"]
    assert scoped_view.known_modules_payload == ["alpha", "main"]
    assert scoped_view.known_modules_set == frozenset({"alpha", "main"})
    assert scoped_view.pgo_hot_function_names_payload == ["main::hot"]
    assert scoped_view.pgo_hot_function_names_set == frozenset({"main::hot"})


def test_module_lowering_context_payload_reuses_precomputed_scoped_inputs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scoped_inputs = cli._ScopedLoweringInputView(
        known_modules=("alpha", "main"),
        known_func_defaults={"main": {"run": {"params": 0, "defaults": []}}},
        pgo_hot_function_names=("main::hot",),
        type_facts=None,
    )
    monkeypatch.setattr(
        cli,
        "_scoped_lowering_input_view",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected scoped lowering recompute")
        ),
    )

    payload = cli._module_lowering_context_payload(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override=None,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"main", "alpha"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": {"alpha"}, "alpha": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        module_dep_closures={"main": frozenset({"main", "alpha"})},
        scoped_inputs=scoped_inputs,
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert payload is not None
    assert tuple(payload["known_modules"]) == ("alpha", "main")
    assert tuple(payload["pgo_hot_functions"]) == ("main::hot",)
    assert set(payload["known_func_defaults"]) == {"main"}


def test_module_worker_payload_reuses_precomputed_scoped_inputs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scoped_inputs = cli._ScopedLoweringInputView(
        known_modules=("alpha", "main"),
        known_func_defaults={"main": {"run": {"params": 0, "defaults": []}}},
        pgo_hot_function_names=("main::hot",),
        type_facts=None,
    )
    monkeypatch.setattr(
        cli,
        "_scoped_lowering_input_view",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected scoped lowering recompute")
        ),
    )

    payload = cli._module_worker_payload(
        "main",
        module_path=Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        source="import alpha\n",
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        module_is_namespace=False,
        entry_module=None,
        type_facts=None,
        enable_phi=True,
        known_modules=("alpha", "main"),
        known_classes_snapshot={},
        stdlib_allowlist_sorted=("json",),
        known_func_defaults={},
        module_deps={"main": {"alpha"}, "alpha": set()},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=(),
        module_dep_closures={"main": frozenset({"main", "alpha"})},
        scoped_inputs=scoped_inputs,
    )

    assert payload["known_modules"] == ["alpha", "main"]
    assert payload["pgo_hot_functions"] == ["main::hot"]
    assert set(payload["known_func_defaults"]) == {"main"}


def test_load_cached_module_lowering_result_reuses_precomputed_views(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "alpha.py"
    module_path.write_text("VALUE = 1\n")
    cache = cli._ModuleResolutionCache()
    context_digest = cli._module_lowering_context_digest({"module": "alpha", "v": 1})
    assert context_digest is not None

    cli._write_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="alpha",
        is_package=False,
        context_digest=context_digest,
        result={
            "functions": [],
            "func_code_ids": {},
            "local_class_names": [],
            "local_classes": {},
            "midend_policy_outcomes_by_function": {},
            "midend_pass_stats_by_function": {},
            "timings": {"visit_s": 0.0, "lower_s": 0.0, "total_s": 0.0},
        },
    )

    monkeypatch.setattr(
        cli,
        "_module_lowering_context_payload",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected context payload recompute")
        ),
    )

    result = cli._load_cached_module_lowering_result(
        tmp_path,
        "alpha",
        module_path,
        logical_source_path=str(module_path),
        entry_override=None,
        is_package=False,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"alpha"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"alpha": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        context_digest=context_digest,
        scoped_inputs=cli._ScopedLoweringInputView(
            known_modules=("alpha",),
            known_func_defaults={},
            pgo_hot_function_names=(),
            type_facts=None,
        ),
        scoped_known_classes={},
        resolution_cache=cache,
    )

    assert result is not None
    assert result["functions"] == []


def test_module_frontend_generator_uses_scoped_inputs() -> None:
    gen = cli._module_frontend_generator(
        module_name="main",
        logical_source_path="/tmp/main.py",
        entry_override=None,
        module_is_namespace=False,
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        enable_phi=True,
        stdlib_allowlist=("json",),
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        scoped_inputs=cli._ScopedLoweringInputView(
            known_modules=("alpha", "main"),
            known_func_defaults={"main": {"run": {"params": 0, "defaults": []}}},
            pgo_hot_function_names=("main::hot",),
            type_facts=None,
        ),
        scoped_known_classes={"MainClass": {"module": "main", "fields": {}}},
    )

    assert gen.module_name == "main"
    assert gen.known_modules == {"alpha", "main"}
    assert gen.known_func_defaults == {"main": {"run": {"params": 0, "defaults": []}}}
    assert gen.midend_hot_functions == {"main::hot"}
    assert gen.classes["MainClass"]["module"] == "main"


def test_module_lowering_context_digest_for_module_reuses_precomputed_views() -> None:
    digest = cli._module_lowering_context_digest_for_module(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override=None,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"main", "alpha"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": {"alpha"}, "alpha": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        module_dep_closures={"main": frozenset({"main", "alpha"})},
        scoped_inputs=cli._ScopedLoweringInputView(
            known_modules=("alpha", "main"),
            known_func_defaults={},
            pgo_hot_function_names=(),
            type_facts=None,
        ),
        scoped_known_classes={},
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert isinstance(digest, str)
    assert digest


def test_parallel_build_reuses_cached_lowering_across_parallel_builds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("import alpha\nimport beta\nprint(alpha.VALUE + beta.VALUE)\n")
    (project / "alpha.py").write_text("VALUE = 1\n")
    (project / "beta.py").write_text("VALUE = 2\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_bin.write_text("")

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 2)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_min_modules", lambda: 2)
    monkeypatch.setattr(
        cli, "_resolve_frontend_parallel_min_predicted_cost", lambda: 0.0
    )
    monkeypatch.setattr(
        cli, "_resolve_frontend_parallel_target_cost_per_worker", lambda: 1.0
    )
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli,
        "_analyze_module_schedule",
        lambda module_graph, module_deps: (
            cli._topo_sort_modules(module_graph, module_deps),
            cli._reverse_module_dependencies(module_deps, module_graph),
            False,
            cli._module_dependency_layers(
                cli._topo_sort_modules(module_graph, module_deps), module_deps
            ),
            cli._module_dependency_closures(
                module_deps,
                module_graph,
                module_order=cli._topo_sort_modules(module_graph, module_deps),
                has_back_edges=False,
            ),
        ),
    )
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(cli, "_ensure_backend_binary", lambda *args, **kwargs: True)

    original_run = cli.subprocess.run

    def fake_run(cmd: list[str], *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        if cmd and str(cmd[0]) == str(backend_bin):
            output = Path(cmd[cmd.index("--output") + 1])
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(b"OBJ")
            return subprocess.CompletedProcess(cmd, 0, "", "")
        return original_run(cmd, *args, **kwargs)

    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    submit_calls = 0

    class _FakeFuture:
        def __init__(self, payload: dict[str, object]) -> None:
            self._payload = payload

        def result(self) -> dict[str, object]:
            return cli._frontend_lower_module_worker(self._payload)

    class _FakeExecutor:
        def __init__(self, *, max_workers: int) -> None:
            self.max_workers = max_workers

        def __enter__(self) -> "_FakeExecutor":
            return self

        def __exit__(
            self,
            exc_type: object,
            exc: object,
            tb: object,
        ) -> bool:
            return False

        def submit(self, fn: object, payload: dict[str, object]) -> _FakeFuture:
            nonlocal submit_calls
            submit_calls += 1
            assert fn is cli._frontend_lower_module_worker
            return _FakeFuture(payload)

    monkeypatch.setattr(cli, "ProcessPoolExecutor", _FakeExecutor)
    first_stdout = io.StringIO()
    with contextlib.redirect_stdout(first_stdout):
        rc = cli.build(
            str(entry),
            emit="obj",
            output=str(tmp_path / "first.o"),
            profile="dev",
            deterministic=False,
            json_output=True,
            diagnostics=True,
        )
    assert rc == 0
    first_submit_calls = submit_calls
    assert first_submit_calls >= 2

    submit_calls = 0
    second_stdout = io.StringIO()
    with contextlib.redirect_stdout(second_stdout):
        rc = cli.build(
            str(entry),
            emit="obj",
            output=str(tmp_path / "second.o"),
            profile="dev",
            deterministic=False,
            json_output=True,
            diagnostics=True,
        )
    assert rc == 0
    assert 0 <= submit_calls < first_submit_calls

    payload = json.loads(second_stdout.getvalue())
    compile_diagnostics = payload["data"]["compile_diagnostics"]
    worker_modes = {
        item["mode"]
        for item in compile_diagnostics["frontend_parallel"]["worker_timings"]
    }
    assert "parallel_cache_hit" in worker_modes


def test_parallel_build_only_relowers_changed_frontier(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("import alpha\nimport beta\nprint(alpha.VALUE + beta.VALUE)\n")
    alpha = project / "alpha.py"
    alpha.write_text("VALUE = 1\n")
    (project / "beta.py").write_text("VALUE = 2\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_bin.write_text("")

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 2)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_min_modules", lambda: 2)
    monkeypatch.setattr(
        cli, "_resolve_frontend_parallel_min_predicted_cost", lambda: 0.0
    )
    monkeypatch.setattr(
        cli, "_resolve_frontend_parallel_target_cost_per_worker", lambda: 1.0
    )
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli,
        "_analyze_module_schedule",
        lambda module_graph, module_deps: (
            cli._topo_sort_modules(module_graph, module_deps),
            cli._reverse_module_dependencies(module_deps, module_graph),
            False,
            cli._module_dependency_layers(
                cli._topo_sort_modules(module_graph, module_deps), module_deps
            ),
            cli._module_dependency_closures(
                module_deps,
                module_graph,
                module_order=cli._topo_sort_modules(module_graph, module_deps),
                has_back_edges=False,
            ),
        ),
    )
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(cli, "_ensure_backend_binary", lambda *args, **kwargs: True)

    original_run = cli.subprocess.run

    def fake_run(cmd: list[str], *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        if cmd and str(cmd[0]) == str(backend_bin):
            output = Path(cmd[cmd.index("--output") + 1])
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(b"OBJ")
            return subprocess.CompletedProcess(cmd, 0, "", "")
        return original_run(cmd, *args, **kwargs)

    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    class _FakeFuture:
        def __init__(self, payload: dict[str, object]) -> None:
            self._payload = payload

        def result(self) -> dict[str, object]:
            return cli._frontend_lower_module_worker(self._payload)

    class _FakeExecutor:
        def __init__(self, *, max_workers: int) -> None:
            self.max_workers = max_workers

        def __enter__(self) -> "_FakeExecutor":
            return self

        def __exit__(
            self,
            exc_type: object,
            exc: object,
            tb: object,
        ) -> bool:
            return False

        def submit(self, fn: object, payload: dict[str, object]) -> _FakeFuture:
            assert fn is cli._frontend_lower_module_worker
            return _FakeFuture(payload)

    monkeypatch.setattr(cli, "ProcessPoolExecutor", _FakeExecutor)

    first_stdout = io.StringIO()
    with contextlib.redirect_stdout(first_stdout):
        rc = cli.build(
            str(entry),
            emit="obj",
            output=str(tmp_path / "first.o"),
            profile="dev",
            deterministic=False,
            json_output=True,
            diagnostics=True,
        )
    assert rc == 0

    alpha.write_text("VALUE = 10\n")

    second_stdout = io.StringIO()
    with contextlib.redirect_stdout(second_stdout):
        rc = cli.build(
            str(entry),
            emit="obj",
            output=str(tmp_path / "second.o"),
            profile="dev",
            deterministic=False,
            json_output=True,
            diagnostics=True,
        )
    assert rc == 0

    payload = json.loads(second_stdout.getvalue())
    compile_diagnostics = payload["data"]["compile_diagnostics"]
    worker_modes_by_module = {
        item["module"]: item["mode"]
        for item in compile_diagnostics["frontend_parallel"]["worker_timings"]
    }

    assert worker_modes_by_module["beta"] == "parallel_cache_hit"
    assert worker_modes_by_module["alpha"] == "parallel"
    assert worker_modes_by_module["main"] == "parallel_cache_hit"


def test_parallel_build_allows_scoped_type_facts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("import alpha\nprint(alpha.VALUE)\n")
    (project / "alpha.py").write_text("VALUE = 1\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_bin.write_text("")

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 2)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_min_modules", lambda: 2)
    monkeypatch.setattr(
        cli, "_resolve_frontend_parallel_min_predicted_cost", lambda: 0.0
    )
    monkeypatch.setattr(
        cli, "_resolve_frontend_parallel_target_cost_per_worker", lambda: 1.0
    )
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli,
        "_analyze_module_schedule",
        lambda module_graph, module_deps: (
            cli._topo_sort_modules(module_graph, module_deps),
            cli._reverse_module_dependencies(module_deps, module_graph),
            False,
            cli._module_dependency_layers(
                cli._topo_sort_modules(module_graph, module_deps), module_deps
            ),
            cli._module_dependency_closures(
                module_deps,
                module_graph,
                module_order=cli._topo_sort_modules(module_graph, module_deps),
                has_back_edges=False,
            ),
        ),
    )
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(cli, "_ensure_backend_binary", lambda *args, **kwargs: True)

    original_run = cli.subprocess.run

    def fake_run(cmd: list[str], *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        if cmd and str(cmd[0]) == str(backend_bin):
            output = Path(cmd[cmd.index("--output") + 1])
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(b"OBJ")
            return subprocess.CompletedProcess(cmd, 0, "", "")
        return original_run(cmd, *args, **kwargs)

    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    captured_payloads: list[dict[str, object]] = []

    class _FakeFuture:
        def __init__(self, payload: dict[str, object]) -> None:
            self._payload = payload

        def result(self) -> dict[str, object]:
            return cli._frontend_lower_module_worker(self._payload)

    class _FakeExecutor:
        def __init__(self, *, max_workers: int) -> None:
            self.max_workers = max_workers

        def __enter__(self) -> "_FakeExecutor":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def submit(self, fn: object, payload: dict[str, object]) -> _FakeFuture:
            assert fn is cli._frontend_lower_module_worker
            captured_payloads.append(payload)
            return _FakeFuture(payload)

    monkeypatch.setattr(cli, "ProcessPoolExecutor", _FakeExecutor)

    type_facts = TypeFacts(
        modules={
            "main": ModuleFacts(globals={"ENTRY": Fact(type="int", trust="trusted")}),
            "alpha": ModuleFacts(globals={"VALUE": Fact(type="int", trust="trusted")}),
            "unrelated": ModuleFacts(
                globals={"NOPE": Fact(type="bytes", trust="trusted")}
            ),
        }
    )
    monkeypatch.setattr(
        cli,
        "_collect_type_facts_for_build",
        lambda *args, **kwargs: (type_facts, True),
    )

    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        rc = cli.build(
            str(entry),
            emit="obj",
            output=str(tmp_path / "typed.o"),
            profile="dev",
            deterministic=False,
            json_output=True,
            diagnostics=True,
            type_hint_policy="trust",
        )

    assert rc == 0
    assert captured_payloads
    for payload in captured_payloads:
        scoped = payload["type_facts"]
        assert isinstance(scoped, TypeFacts)
        assert "unrelated" not in scoped.modules
    compile_diagnostics = json.loads(stdout.getvalue())["data"]["compile_diagnostics"]
    assert compile_diagnostics["frontend_parallel"]["enabled"] is True
    assert compile_diagnostics["frontend_parallel"]["reason"] == "enabled"


def test_build_one_shot_backend_compile_uses_bytes_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_bin.write_text("")

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 0)
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(cli, "_ensure_backend_binary", lambda *args, **kwargs: True)

    original_run = cli.subprocess.run
    backend_inputs: list[bytes] = []

    def fake_run(cmd: list[str], *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        if cmd and str(cmd[0]) == str(backend_bin):
            backend_input = kwargs.get("input")
            assert isinstance(backend_input, bytes)
            assert kwargs.get("text") in (None, False)
            backend_inputs.append(backend_input)
            output = Path(cmd[cmd.index("--output") + 1])
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(b"OBJ")
            return subprocess.CompletedProcess(cmd, 0, b"", b"")
        return original_run(cmd, *args, **kwargs)

    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    rc = cli.build(
        str(entry),
        emit="obj",
        output=str(tmp_path / "out.o"),
        profile="dev",
        deterministic=False,
        json_output=False,
    )

    assert rc == 0
    assert len(backend_inputs) == 1
    # Backend input is msgpack-encoded (binary), not JSON
    assert isinstance(backend_inputs[0], bytes)
    assert len(backend_inputs[0]) > 0


def test_build_skips_daemon_preflight_when_socket_exists(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_bin.write_text("")
    daemon_socket = tmp_path / "daemon.sock"
    daemon_socket.write_text("")

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 0)
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: True)
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(cli, "_ensure_backend_binary", lambda *args, **kwargs: True)
    monkeypatch.setattr(
        cli,
        "_backend_daemon_socket_path",
        lambda *args, **kwargs: daemon_socket,
    )
    monkeypatch.setattr(
        cli,
        "_backend_daemon_wait_until_ready",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected daemon preflight wait")
        ),
    )
    monkeypatch.setattr(
        cli,
        "_start_backend_daemon",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected daemon restart")
        ),
    )

    compile_calls = 0

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        *,
        ir: dict[str, object],
        backend_output: Path,
        is_wasm: bool,
        wasm_link: bool,
        wasm_data_base: int | None,
        wasm_table_base: int | None,
        target_triple: str | None,
        cache_key: str | None,
        function_cache_key: str | None,
        config_digest: str,
        skip_module_output_if_synced: bool,
        skip_function_output_if_synced: bool,
        timeout: float | None,
        request_bytes: bytes | None = None,
    ) -> cli._BackendDaemonCompileResult:
        del (
            ir,
            is_wasm,
            wasm_link,
            wasm_data_base,
            wasm_table_base,
            target_triple,
            cache_key,
            function_cache_key,
            config_digest,
            skip_module_output_if_synced,
            skip_function_output_if_synced,
            timeout,
            request_bytes,
        )
        nonlocal compile_calls
        compile_calls += 1
        assert socket_path == daemon_socket
        backend_output.parent.mkdir(parents=True, exist_ok=True)
        backend_output.write_bytes(b"OBJ")
        return cli._BackendDaemonCompileResult(
            ok=True,
            error=None,
            health=None,
            cached=False,
            cache_tier=None,
            output_written=True,
            output_exists=True,
        )

    monkeypatch.setattr(
        cli, "_compile_with_backend_daemon", fake_compile_with_backend_daemon
    )

    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        rc = cli.build(
            str(entry),
            emit="obj",
            output=str(tmp_path / "out.o"),
            profile="dev",
            deterministic=False,
            json_output=True,
            diagnostics=True,
        )

    assert rc == 0
    assert compile_calls == 1


def test_build_native_backend_sets_stdlib_object_env_from_helper(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_bin.write_text("")
    expected_stdlib_obj = cache_root / "stdlib-object.o"

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 0)
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(cli, "_ensure_backend_binary", lambda *args, **kwargs: True)
    monkeypatch.setattr(
        cli,
        "_stdlib_object_cache_path",
        lambda cache_path, cache_key: expected_stdlib_obj,
    )

    original_run = cli.subprocess.run
    seen_backend_env: dict[str, str] | None = None

    def fake_run(cmd: list[str], *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        nonlocal seen_backend_env
        if cmd and str(cmd[0]) == str(backend_bin):
            env = kwargs.get("env")
            assert isinstance(env, dict)
            seen_backend_env = env
            output = Path(cmd[cmd.index("--output") + 1])
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(b"OBJ")
            return subprocess.CompletedProcess(cmd, 0, b"", b"")
        return original_run(cmd, *args, **kwargs)

    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    rc = cli.build(
        str(entry),
        emit="obj",
        output=str(tmp_path / "out.o"),
        profile="dev",
        deterministic=False,
        json_output=False,
    )

    assert rc == 0
    assert seen_backend_env is not None
    assert seen_backend_env["MOLT_STDLIB_OBJ"] == str(expected_stdlib_obj)


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
    source_path.write_bytes(
        "# -*- coding: latin-1 -*-\nname = 'caf\xe9'\n".encode("latin-1")
    )
    assert (
        cli._read_module_source(source_path)
        == "# -*- coding: latin-1 -*-\nname = 'café'\n"
    )


def _compile_with_backend_daemon_non_wasm(
    socket_path: Path,
    *,
    ir: dict[str, object],
    backend_output: Path,
    target_triple: str | None,
    cache_key: str | None,
    function_cache_key: str | None,
    config_digest: str | None,
    skip_module_output_if_synced: bool = False,
    skip_function_output_if_synced: bool = False,
    timeout: float | None,
    request_bytes: bytes | None = None,
) -> cli._BackendDaemonCompileResult:
    return cli._compile_with_backend_daemon(
        socket_path,
        ir=ir,
        backend_output=backend_output,
        is_wasm=False,
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        target_triple=target_triple,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        config_digest=config_digest,
        skip_module_output_if_synced=skip_module_output_if_synced,
        skip_function_output_if_synced=skip_function_output_if_synced,
        timeout=timeout,
        request_bytes=request_bytes,
    )


def test_stdlib_graph_ignores_nested_imports_for_core_scan(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print(1)\n")
    graph = _discover_with_core_modules(entry)
    assert "builtins" in graph
    assert "sys" in graph
    # importlib is only included when MOLT_STDLIB_PROFILE != "micro" (the default)
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
        ROOT,
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
    if core_paths:
        core_graph, _ = cli._discover_module_graph_from_paths(
            core_paths,
            roots,
            module_roots,
            stdlib_root,
            ROOT,
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
        ROOT,
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


def test_analyze_module_schedule_reuses_reverse_edges_and_layers() -> None:
    module_graph = {
        "a": Path("/tmp/a.py"),
        "b": Path("/tmp/b.py"),
        "c": Path("/tmp/c.py"),
        "d": Path("/tmp/d.py"),
        "e": Path("/tmp/e.py"),
    }
    deps = {
        "a": set(),
        "b": {"a"},
        "c": {"a"},
        "d": {"b", "c"},
        "e": {"b"},
    }

    order, reverse_deps, has_back_edges, layers, closures = (
        cli._analyze_module_schedule(
            module_graph,
            deps,
        )
    )

    assert order == ["a", "b", "c", "e", "d"]
    assert reverse_deps["a"] == {"b", "c"}
    assert reverse_deps["b"] == {"d", "e"}
    assert reverse_deps["c"] == {"d"}
    assert has_back_edges is False
    assert layers == [["a"], ["b", "c"], ["e", "d"]]
    assert closures["a"] == frozenset({"a"})
    assert closures["b"] == frozenset({"a", "b"})
    assert closures["d"] == frozenset({"a", "b", "c", "d"})


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


def test_build_frontend_module_costs_precomputes_source_and_dep_costs() -> None:
    costs = cli._build_frontend_module_costs(
        {"a", "b"},
        module_sources={"a": "x" * 10, "b": ""},
        module_deps={"a": {"root"}, "b": set()},
    )

    assert costs["a"] == 10.0 + 512.0
    assert costs["b"] == 1.0


def test_build_stdlib_like_module_flags_precomputes_classification() -> None:
    flags = cli._build_stdlib_like_module_flags({"warnings", "pkg.mod"})
    assert flags["warnings"] is True
    assert flags["pkg.mod"] is False


def test_build_module_graph_metadata_bundles_related_views(tmp_path: Path) -> None:
    module_graph = {
        "app_entry": tmp_path / "app.py",
        "pkg": tmp_path / "pkg" / "__init__.py",
    }
    metadata = cli._build_module_graph_metadata(
        module_graph,
        generated_module_source_paths={"pkg": "/generated/pkg/__init__.py"},
        entry_module="app_entry",
        namespace_module_names={"pkg"},
        module_sources={"app_entry": "print('ok')\n", "pkg": ""},
        module_deps={"app_entry": {"pkg"}, "pkg": set()},
    )

    assert metadata.logical_source_path_by_module["pkg"] == "/generated/pkg/__init__.py"
    assert metadata.entry_override_by_module["app_entry"] is None
    assert metadata.module_is_namespace_by_module["pkg"] is True
    assert metadata.module_is_package_by_module["pkg"] is True
    assert metadata.frontend_module_costs is not None
    assert metadata.frontend_module_costs["app_entry"] == 12.0 + 512.0
    assert metadata.stdlib_like_by_module is not None
    assert metadata.stdlib_like_by_module["pkg"] is False


def test_augment_module_graph_does_not_add_entry_alias_as_second_module(
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    examples_dir = project_root / "examples"
    examples_dir.mkdir()
    source_path = examples_dir / "hello.py"
    source_path.write_text("print('hello')\n")
    stdlib_root = cli._stdlib_root_path()
    module_graph = {"hello": source_path}

    augmentation, error = cli._augment_module_graph_for_entry_and_runtime(
        source_path=source_path,
        entry_module="hello",
        module_roots=[project_root, examples_dir],
        stdlib_root=stdlib_root,
        roots=[project_root, examples_dir, stdlib_root],
        project_root=project_root,
        stdlib_allowlist=cli._stdlib_allowlist(),
        entry_imports=(),
        module_resolution_cache=cli._ModuleResolutionCache(),
        module_graph=module_graph,
        module_reasons={},
        diagnostics_enabled=False,
        json_output=False,
        target="native",
    )

    assert error is None
    assert augmentation is not None
    assert set(module_graph) == {"hello"}
    assert list(module_graph.values()) == [source_path]


def test_module_lowering_metadata_view_reuses_precomputed_maps(tmp_path: Path) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("VALUE = 1\n")
    metadata = cli._build_module_graph_metadata(
        {"pkg": module_path},
        generated_module_source_paths={"pkg": "generated/pkg.py"},
        entry_module="pkg",
        namespace_module_names=set(),
    )
    path_stat = module_path.stat()

    view = cli._module_lowering_metadata_view(
        "pkg",
        module_path=module_path,
        module_graph_metadata=metadata,
        path_stat_by_module={"pkg": path_stat},
    )

    assert view.logical_source_path == "generated/pkg.py"
    assert view.entry_override is None
    assert view.module_is_namespace is False
    assert view.is_package is False
    assert view.path_stat is path_stat


def test_module_lowering_execution_view_bundles_metadata_and_scoped_state(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "main.py"
    module_path.write_text("import alpha\n")
    metadata = cli._build_module_graph_metadata(
        {"main": module_path, "alpha": tmp_path / "alpha.py"},
        generated_module_source_paths={"main": "generated/main.py"},
        entry_module="main",
        namespace_module_names=set(),
    )
    execution_view = cli._module_lowering_execution_view(
        "main",
        module_path=module_path,
        module_graph_metadata=metadata,
        module_deps={"main": {"alpha"}, "alpha": set()},
        known_modules={"main", "alpha"},
        known_func_defaults={"main": {"run": {"params": 0, "defaults": []}}},
        pgo_hot_function_names={"main::hot"},
        type_facts=None,
        known_classes_snapshot={"MainClass": {"module": "main", "fields": {}}},
        module_dep_closures={"main": frozenset({"main", "alpha"})},
        path_stat_by_module={"main": module_path.stat()},
        known_modules_sorted=("alpha", "main"),
        pgo_hot_function_names_sorted=("main::hot",),
    )

    assert execution_view.metadata.logical_source_path == "generated/main.py"
    assert execution_view.metadata.path_stat is not None
    assert execution_view.scoped_inputs.known_modules == ("alpha", "main")
    assert execution_view.scoped_inputs.pgo_hot_function_names == ("main::hot",)
    assert set(execution_view.scoped_known_classes) == {"MainClass"}


def test_choose_frontend_parallel_layer_workers_uses_precomputed_costs_and_flags(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        cli,
        "_predict_frontend_module_cost",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected live cost recompute")
        ),
    )
    monkeypatch.setattr(
        cli,
        "_looks_like_stdlib_module_name",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected stdlib classification recompute")
        ),
    )

    decision = cli._choose_frontend_parallel_layer_workers(
        candidates=["a", "b", "warnings"],
        module_sources={},
        module_deps={},
        module_costs={"a": 40000.0, "b": 40000.0, "warnings": 40000.0},
        stdlib_like_by_module={"a": False, "b": False, "warnings": True},
        max_workers=8,
        min_modules=2,
        min_predicted_cost=1.0,
        target_cost_per_worker=50000.0,
    )

    assert decision["enabled"] is True
    assert decision["workers"] == 3
    assert decision["stdlib_candidates"] == 1


def test_module_order_has_back_edges_detects_cycles() -> None:
    order = ["a", "b"]
    assert cli._module_order_has_back_edges(order, {"a": {"b"}, "b": {"a"}})
    assert not cli._module_order_has_back_edges(order, {"a": set(), "b": {"a"}})


def test_analyze_module_schedule_marks_cycles_and_appends_remaining() -> None:
    module_graph = {
        "a": Path("/tmp/a.py"),
        "b": Path("/tmp/b.py"),
    }
    deps = {"a": {"b"}, "b": {"a"}}

    order, reverse_deps, has_back_edges, layers, closures = (
        cli._analyze_module_schedule(
            module_graph,
            deps,
        )
    )

    assert set(order) == {"a", "b"}
    assert reverse_deps["a"] == {"b"}
    assert reverse_deps["b"] == {"a"}
    assert has_back_edges is True
    assert sum(len(layer) for layer in layers) == 2
    assert closures["a"] == frozenset({"a", "b"})
    assert closures["b"] == frozenset({"a", "b"})


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
    assert (
        "- midend.policy.budget_formula: alpha=0.0300 beta=0.7500 scale=1.0000"
        in stderr
    )
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
    cli._resolve_backend_profile_cached.cache_clear()
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
    cli._resolve_cargo_profile_name_cached.cache_clear()
    monkeypatch.delenv("MOLT_DEV_CARGO_PROFILE", raising=False)
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "dev"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "my-dev_1")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "my-dev_1"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "bad profile")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "dev"
    assert error == "Invalid MOLT_DEV_CARGO_PROFILE value: bad profile"


def test_resolve_backend_cargo_profile_name_defaults_and_validation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli._resolve_backend_cargo_profile_name_cached.cache_clear()
    monkeypatch.delenv("MOLT_DEV_BACKEND_CARGO_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_DEV_CARGO_PROFILE", raising=False)
    profile, error = cli._resolve_backend_cargo_profile_name("dev")
    assert profile == "dev"
    assert error is None

    monkeypatch.delenv("MOLT_RELEASE_BACKEND_CARGO_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_RELEASE_CARGO_PROFILE", raising=False)
    profile, error = cli._resolve_backend_cargo_profile_name("release")
    assert profile == "release-fast"
    assert error is None

    monkeypatch.setenv("MOLT_RELEASE_CARGO_PROFILE", "release-iter")
    profile, error = cli._resolve_backend_cargo_profile_name("release")
    assert profile == "release-iter"
    assert error is None

    monkeypatch.setenv("MOLT_RELEASE_BACKEND_CARGO_PROFILE", "backend-prod")
    profile, error = cli._resolve_backend_cargo_profile_name("release")
    assert profile == "backend-prod"
    assert error is None

    monkeypatch.setenv("MOLT_RELEASE_BACKEND_CARGO_PROFILE", "bad profile")
    profile, error = cli._resolve_backend_cargo_profile_name("release")
    assert profile == "release"
    assert error == "Invalid MOLT_RELEASE_BACKEND_CARGO_PROFILE value: bad profile"


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


def test_backend_daemon_start_timeout_defaults_to_finite_bound(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_START_TIMEOUT", raising=False)
    assert cli._backend_daemon_start_timeout() == 2.0


def test_backend_daemon_start_timeout_accepts_env_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_START_TIMEOUT", "2.5")
    assert cli._backend_daemon_start_timeout() == 2.5


def test_backend_daemon_start_timeout_can_be_unbounded_explicitly(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_START_TIMEOUT", "0")
    assert cli._backend_daemon_start_timeout() is None


def test_backend_daemon_spawn_probe_timeout_is_capped() -> None:
    assert cli._backend_daemon_spawn_probe_timeout(2.0) == 0.25
    assert cli._backend_daemon_spawn_probe_timeout(0.1) == 0.1
    assert cli._backend_daemon_spawn_probe_timeout(None) == 0.25


def test_start_backend_daemon_leaves_warming_process_running(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("backend")
    socket_path = tmp_path / "daemon.sock"
    pid_path = tmp_path / "daemon.pid"
    log_path = tmp_path / "daemon.log"
    wait_timeouts: list[float | None] = []
    terminated: list[int] = []
    removed: list[Path] = []

    class _FakePopen:
        pid = 4321

    monkeypatch.setattr(cli, "_backend_daemon_pid_path", lambda *args, **kwargs: pid_path)
    monkeypatch.setattr(cli, "_backend_daemon_log_path", lambda *args, **kwargs: log_path)
    monkeypatch.setattr(cli, "_read_backend_daemon_pid", lambda *args, **kwargs: None)

    def fake_wait_until_ready(*args: object, **kwargs: object) -> tuple[bool, dict[str, object] | None]:
        del args
        wait_timeouts.append(cast(float | None, kwargs.get("ready_timeout")))
        return False, None

    monkeypatch.setattr(cli, "_backend_daemon_wait_until_ready", fake_wait_until_ready)
    monkeypatch.setattr(cli.subprocess, "Popen", lambda *args, **kwargs: _FakePopen())
    monkeypatch.setattr(
        cli,
        "_terminate_backend_daemon_pid",
        lambda pid, **kwargs: terminated.append(pid),
    )
    monkeypatch.setattr(
        cli,
        "_remove_backend_daemon_pid",
        lambda path: removed.append(path),
    )

    assert (
        cli._start_backend_daemon(
            backend_bin,
            socket_path,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            startup_timeout=2.0,
            json_output=True,
        )
        is False
    )
    assert wait_timeouts == [0.25]
    assert pid_path.read_text().strip() == "4321"
    assert terminated == []
    assert removed == []


def test_prepare_backend_setup_defers_runtime_lib_ready_check_for_native_cache_hit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_artifact = tmp_path / "output.o"
    ensure_calls: list[Path | None] = []

    monkeypatch.setattr(
        cli,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: cli._RuntimeArtifactState(runtime_lib=runtime_lib),
    )

    def fake_prepare_backend_cache_setup(**kwargs: object) -> cli._BackendCacheSetup:
        del kwargs
        return cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="module-cache",
            function_cache_key=None,
            cache_path=tmp_path / "module-cache.o",
            function_cache_path=None,
            stdlib_object_path=None,
            cache_candidates=(("module", tmp_path / "module-cache.o"),),
            cache_hit=True,
            cache_hit_tier="module",
        )

    monkeypatch.setattr(cli, "_prepare_backend_cache_setup", fake_prepare_backend_cache_setup)
    monkeypatch.setattr(
        cli,
        "_ensure_runtime_lib_ready",
        lambda runtime_state, **kwargs: ensure_calls.append(runtime_state.runtime_lib)
        or True,
    )

    prepared_backend_setup, backend_setup_error = cli._prepare_backend_setup(
        is_rust_transpile=False,
        is_wasm=False,
        emit_mode="bin",
        molt_root=tmp_path,
        runtime_cargo_profile="dev",
        target_triple=None,
        json_output=True,
        cargo_timeout=1.0,
        target="native",
        profile="dev",
        backend_cargo_profile="dev",
        linked=False,
        project_root=tmp_path,
        cache_dir=None,
        output_artifact=output_artifact,
        warnings=[],
        cache=True,
        ir={"functions": []},
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert prepared_backend_setup.cache_hit is True
    assert ensure_calls == []


def test_prepare_backend_setup_keeps_runtime_lib_ready_check_for_native_cache_miss(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_artifact = tmp_path / "output.o"
    ensure_calls: list[Path | None] = []

    monkeypatch.setattr(
        cli,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: cli._RuntimeArtifactState(runtime_lib=runtime_lib),
    )

    def fake_prepare_backend_cache_setup(**kwargs: object) -> cli._BackendCacheSetup:
        del kwargs
        return cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="module-cache",
            function_cache_key=None,
            cache_path=tmp_path / "module-cache.o",
            function_cache_path=None,
            stdlib_object_path=None,
            cache_candidates=(("module", tmp_path / "module-cache.o"),),
            cache_hit=False,
            cache_hit_tier=None,
        )

    monkeypatch.setattr(cli, "_prepare_backend_cache_setup", fake_prepare_backend_cache_setup)
    monkeypatch.setattr(
        cli,
        "_ensure_runtime_lib_ready",
        lambda runtime_state, **kwargs: ensure_calls.append(runtime_state.runtime_lib)
        or True,
    )

    prepared_backend_setup, backend_setup_error = cli._prepare_backend_setup(
        is_rust_transpile=False,
        is_wasm=False,
        emit_mode="bin",
        molt_root=tmp_path,
        runtime_cargo_profile="dev",
        target_triple=None,
        json_output=True,
        cargo_timeout=1.0,
        target="native",
        profile="dev",
        backend_cargo_profile="dev",
        linked=False,
        project_root=tmp_path,
        cache_dir=None,
        output_artifact=output_artifact,
        warnings=[],
        cache=True,
        ir={"functions": []},
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert prepared_backend_setup.cache_hit is False
    assert ensure_calls == [runtime_lib]


def test_ensure_backend_binary_uses_native_feature_for_native(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "target" / "dev-fast" / "molt-backend"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    seen_features: list[tuple[str, ...]] = []
    build_cmds: list[list[str]] = []

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args
        seen_features.append(cast(tuple[str, ...], kwargs["backend_features"]))
        return dict(fingerprint)

    def fake_run_cargo(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_backend_fingerprint", fake_backend_fingerprint)
    monkeypatch.setattr(cli, "_run_cargo_with_sccache_retry", fake_run_cargo)

    assert cli._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
    )
    assert seen_features == [("native-backend",)]
    assert build_cmds == [
        [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--profile",
            "dev-fast",
            "--no-default-features",
            "--features",
            "native-backend",
        ]
    ]


def test_ensure_backend_binary_enables_wasm_feature_for_wasm(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "target" / "dev-fast" / "molt-backend"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    seen_features: list[tuple[str, ...]] = []
    build_cmds: list[list[str]] = []

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args
        seen_features.append(cast(tuple[str, ...], kwargs["backend_features"]))
        return dict(fingerprint)

    def fake_run_cargo(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_backend_fingerprint", fake_backend_fingerprint)
    monkeypatch.setattr(cli, "_run_cargo_with_sccache_retry", fake_run_cargo)

    assert cli._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("wasm-backend",),
    )
    assert seen_features == [("wasm-backend",)]
    assert build_cmds == [
        [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--profile",
            "dev-fast",
            "--no-default-features",
            "--features",
            "wasm-backend",
        ]
    ]


def test_build_rust_target_uses_rust_backend_feature_and_skips_daemon(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_output = tmp_path / "out.rs"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    seen_features: list[tuple[str, ...]] = []
    build_cmds: list[list[str]] = []
    backend_cmds: list[list[str]] = []

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 0)
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: True)
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(
        cli,
        "_start_backend_daemon",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("rust target should not start backend daemon")
        ),
    )
    monkeypatch.setattr(
        cli,
        "_compile_with_backend_daemon",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("rust target should not use backend daemon compile")
        ),
    )

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args
        seen_features.append(cast(tuple[str, ...], kwargs["backend_features"]))
        return dict(fingerprint)

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        backend_bin.write_text("#!/bin/sh\n")
        backend_bin.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    original_run = cli.subprocess.run

    def fake_run(cmd: list[str], *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        if cmd and str(cmd[0]) == str(backend_bin):
            # Backend cache probe: stdin-based call with no --target/--output
            if "--target" not in cmd and "--output" not in cmd:
                return subprocess.CompletedProcess(cmd, 0, b"", b"")
            backend_cmds.append(list(cmd))
            assert cmd[1:3] == ["--target", "rust"]
            assert "--output" in cmd
            output = Path(cmd[cmd.index("--output") + 1])
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text("fn main() {}\n")
            return subprocess.CompletedProcess(cmd, 0, b"", b"")
        return original_run(cmd, *args, **kwargs)

    monkeypatch.setattr(cli, "_backend_fingerprint", fake_backend_fingerprint)
    monkeypatch.setattr(cli, "_run_cargo_with_sccache_retry", fake_run_cargo)
    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    rc = cli.build(
        str(entry),
        target="rust",
        output=str(backend_output),
        profile="dev",
        deterministic=False,
        json_output=False,
    )

    assert rc == 0
    assert seen_features == [("rust-backend",)]
    assert build_cmds == [
        [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--profile",
            "dev",
            "--no-default-features",
            "--features",
            "rust-backend",
        ]
    ]
    assert len(backend_cmds) == 1
    cmd = backend_cmds[0]
    assert cmd[0] == str(backend_bin)
    assert "--target" in cmd and cmd[cmd.index("--target") + 1] == "rust"
    assert "--output" in cmd
    assert backend_output.read_text() == "fn main() {}\n"


def test_build_release_rust_target_uses_release_fast_backend_profile_by_default(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n")

    build_state_root = tmp_path / "build-state"
    cache_root = tmp_path / "cache"
    backend_bin = tmp_path / "fake-backend"
    backend_output = tmp_path / "out.rs"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    build_cmds: list[list[str]] = []

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.delenv("MOLT_RELEASE_BACKEND_CARGO_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_RELEASE_CARGO_PROFILE", raising=False)
    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_resolve_frontend_parallel_module_workers", lambda: 0)
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: True)
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(
        cli,
        "_start_backend_daemon",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("rust target should not start backend daemon")
        ),
    )
    monkeypatch.setattr(
        cli,
        "_compile_with_backend_daemon",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("rust target should not use backend daemon compile")
        ),
    )

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args, kwargs
        return dict(fingerprint)

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        backend_bin.write_text("#!/bin/sh\n")
        backend_bin.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    original_run = cli.subprocess.run

    def fake_run(cmd: list[str], *args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        if cmd and str(cmd[0]) == str(backend_bin):
            # Backend cache probe: stdin-based call with no --output
            if "--output" not in cmd:
                return subprocess.CompletedProcess(cmd, 0, b"", b"")
            output = Path(cmd[cmd.index("--output") + 1])
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text("fn main() {}\n")
            return subprocess.CompletedProcess(cmd, 0, b"", b"")
        return original_run(cmd, *args, **kwargs)

    monkeypatch.setattr(cli, "_backend_fingerprint", fake_backend_fingerprint)
    monkeypatch.setattr(cli, "_run_cargo_with_sccache_retry", fake_run_cargo)
    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    rc = cli.build(
        str(entry),
        target="rust",
        output=str(backend_output),
        profile="release",
        deterministic=False,
        json_output=False,
    )

    assert rc == 0
    assert build_cmds == [
        [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--profile",
            "release-fast",
            "--no-default-features",
            "--features",
            "rust-backend",
        ]
    ]


def test_run_uses_build_profile_flag_for_nested_build(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n")

    build_cmds: list[list[str]] = []
    run_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        del kwargs
        run_cmds.append(list(cmd))
        return 0

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)
    monkeypatch.setattr(cli, "_run_command", fake_run_command)

    rc = cli.run_script(
        str(entry),
        None,
        [],
        build_profile="dev",
        json_output=False,
    )

    assert rc == 0
    assert build_cmds == [
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--build-profile",
            "dev",
            str(entry),
        ]
    ]
    assert run_cmds


def test_native_backend_compile_routes_stdlib_object_env(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    output_artifact = project_root / "build" / "main.o"
    backend_bin = tmp_path / "backend-bin"
    artifacts_root = tmp_path / "artifacts"
    stdlib_object_path = project_root / "build" / "main.stdlib.o"
    captured_envs: list[dict[str, str] | None] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = cast(dict[str, str] | None, kwargs.get("env"))
        captured_envs.append(env)
        assert env is not None
        assert "MOLT_STDLIB_OBJ" in env
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"object")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)

    result, error = cli._execute_backend_compile(
        cache=False,
        cache_path=None,
        function_cache_path=None,
        artifacts_root=artifacts_root,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        diagnostics_enabled=False,
        phase_starts={},
        daemon_ready=False,
        daemon_socket=None,
        project_root=project_root,
        output_artifact=output_artifact,
        cache_key=None,
        function_cache_key=None,
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=False,
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            function_cache_path=None,
            stdlib_object_path=stdlib_object_path,
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
        ),
        target_triple=None,
        backend_daemon_config_digest=None,
        ir={"functions": []},
        json_output=False,
        warnings=[],
        verbose=False,
        backend_bin=backend_bin,
        backend_env=None,
        backend_timeout=None,
        molt_root=project_root,
        backend_cargo_profile="dev-fast",
        _ensure_backend_ir_bytes=lambda: b"{}",
        _get_backend_ir_fmt=lambda: "json",
        cache_hit=False,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
    )

    assert error is None
    assert result is not None
    assert captured_envs and captured_envs[0] is not None
    stdlib_obj = captured_envs[0]["MOLT_STDLIB_OBJ"]
    assert stdlib_obj == str(stdlib_object_path)
    assert stdlib_obj != str(output_artifact)


def test_compare_uses_build_profile_flag_for_nested_build(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n")
    built_binary = project / "build" / "main_molt"
    built_binary.parent.mkdir(parents=True, exist_ok=True)
    built_binary.write_text("")

    seen_cmds: list[list[str]] = []

    def fake_run_command_timed(
        cmd: list[str], **kwargs: object
    ) -> cli._TimedResult:
        del kwargs
        seen_cmds.append(list(cmd))
        if len(seen_cmds) == 1:
            return cli._TimedResult(0, "ok\n", "", 0.01)
        if len(seen_cmds) == 2:
            return cli._TimedResult(
                0,
                json.dumps({"data": {"output": str(built_binary)}}),
                "",
                0.02,
            )
        return cli._TimedResult(0, "ok\n", "", 0.01)

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli, "_resolve_python_exe", lambda exe: "python3")
    monkeypatch.setattr(cli, "_resolve_binary_output", lambda output: built_binary)
    monkeypatch.setattr(cli, "_run_command_timed", fake_run_command_timed)

    rc = cli.compare(
        str(entry),
        None,
        "python3",
        [],
        build_profile="dev",
    )

    assert rc == 0
    assert seen_cmds[1] == [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        "--build-profile",
        "dev",
        str(entry),
    ]


def test_backend_daemon_enabled_is_cached(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli._backend_daemon_enabled_cached.cache_clear()
    monkeypatch.setenv("MOLT_BACKEND_DAEMON", "1")

    first = cli._backend_daemon_enabled()
    second = cli._backend_daemon_enabled()

    info = cli._backend_daemon_enabled_cached.cache_info()
    assert first is True
    assert second is True
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_wasm_cargo_profile_is_cached(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli._resolve_wasm_cargo_profile_cached.cache_clear()
    monkeypatch.setenv("MOLT_WASM_CARGO_PROFILE", "")

    first = cli._resolve_wasm_cargo_profile("release")
    second = cli._resolve_wasm_cargo_profile("release")

    info = cli._resolve_wasm_cargo_profile_cached.cache_info()
    assert first == second == "wasm-release"
    assert info.hits >= 1
    assert info.currsize >= 1


def test_native_arch_perf_requested_is_cached(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli._native_arch_perf_requested_cached.cache_clear()
    monkeypatch.setenv("MOLT_PERF_PROFILE", "native")

    first = cli._native_arch_perf_requested()
    second = cli._native_arch_perf_requested()

    info = cli._native_arch_perf_requested_cached.cache_info()
    assert first is True
    assert second is True
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_codegen_env_inputs_is_cached(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli._backend_codegen_env_inputs_cached.cache_clear()
    monkeypatch.setenv("MOLT_BACKEND_REGALLOC_ALGORITHM", "single_pass")

    first = cli._backend_codegen_env_inputs(is_wasm=False)
    second = cli._backend_codegen_env_inputs(is_wasm=False)

    info = cli._backend_codegen_env_inputs_cached.cache_info()
    assert first == second == {"MOLT_BACKEND_REGALLOC_ALGORITHM": "single_pass"}
    assert info.hits >= 1
    assert info.currsize >= 1


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
    cli._backend_daemon_paths_cached.cache_clear()
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


def test_backend_daemon_paths_are_cached(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET", raising=False)
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET_DIR", raising=False)
    cli._backend_daemon_paths_cached.cache_clear()

    first = cli._backend_daemon_socket_path(tmp_path, "dev-fast", config_digest="abc")
    second = cli._backend_daemon_socket_path(tmp_path, "dev-fast", config_digest="abc")

    info = cli._backend_daemon_paths_cached.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_daemon_log_and_pid_paths_reuse_cached_bundle(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET", raising=False)
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET_DIR", raising=False)
    cli._backend_daemon_paths_cached.cache_clear()

    log_path = cli._backend_daemon_log_path(tmp_path, "dev-fast")
    pid_path = cli._backend_daemon_pid_path(tmp_path, "dev-fast")

    info = cli._backend_daemon_paths_cached.cache_info()
    assert log_path.name == "molt-backend.dev-fast.log"
    assert pid_path.name == "molt-backend.dev-fast.pid"
    assert log_path.parent == pid_path.parent
    assert info.hits >= 1


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
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": []},
        backend_output=backend_output,
        target_triple=None,
        cache_key=None,
        function_cache_key=None,
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )
    assert result.ok is True
    assert result.cached is True
    assert result.cache_tier == "function"
    assert result.output_written is True
    assert result.output_exists is True
    assert captured_payload.get("config_digest") == "digest123"
    assert "include_health" not in captured_payload


def test_compile_with_backend_daemon_allows_cached_hit_without_output_write(
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
        return (
            {
                "ok": True,
                "jobs": [
                    {
                        "id": "job0",
                        "ok": True,
                        "cached": True,
                        "cache_tier": "module",
                        "output_written": False,
                    }
                ],
                "health": {"pid": 42, "cache_hits": 1, "cache_misses": 0},
            },
            None,
        )

    monkeypatch.setattr(cli, "_backend_daemon_request_bytes", _fake_request)
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": []},
        backend_output=backend_output,
        target_triple=None,
        cache_key=None,
        function_cache_key=None,
        config_digest="digest123",
        skip_module_output_if_synced=True,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )

    assert result.ok is True
    assert result.cached is True
    assert result.cache_tier == "module"
    assert result.output_written is False
    assert result.output_exists is True
    assert "include_health" not in captured_payload


def test_compile_with_backend_daemon_accepts_response_without_health(
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
                        "cached": False,
                        "cache_tier": None,
                        "output_written": True,
                    }
                ],
            },
            None,
        )

    monkeypatch.setattr(cli, "_backend_daemon_request_bytes", _fake_request)
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": []},
        backend_output=backend_output,
        target_triple=None,
        cache_key=None,
        function_cache_key=None,
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )

    assert result.ok is True
    assert result.health is None
    assert result.output_exists is True
    assert "include_health" not in captured_payload
    assert backend_output.exists()
    assert captured_payload["jobs"][0]["skip_module_output_if_synced"] is False
    assert captured_payload["jobs"][0]["skip_function_output_if_synced"] is False


def test_compile_with_backend_daemon_uses_preencoded_request_bytes(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    preencoded = (
        b'{"version":1,"jobs":[{"id":"job0","is_wasm":false,"target_triple":"","output":"'
        + str(backend_output).encode("utf-8")
        + b'","cache_key":"","function_cache_key":"","skip_module_output_if_synced":false,'
        b'"skip_function_output_if_synced":false,"ir":{"functions":[]}}]}\n'
    )

    def fail_encode(payload: dict[str, object]) -> tuple[bytes | None, str | None]:
        raise AssertionError(f"unexpected request encode: {payload}")

    def _fake_request(
        socket_path: Path,
        data: bytes,
        *,
        timeout: float | None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, timeout
        assert data == preencoded
        backend_output.write_bytes(b"\x7fELF")
        return (
            {
                "ok": True,
                "jobs": [{"id": "job0", "ok": True}],
            },
            None,
        )

    monkeypatch.setattr(cli, "_backend_daemon_request_payload_bytes", fail_encode)
    monkeypatch.setattr(cli, "_backend_daemon_request_bytes", _fake_request)
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": []},
        backend_output=backend_output,
        target_triple="",
        cache_key=None,
        function_cache_key=None,
        config_digest=None,
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
        request_bytes=preencoded,
    )

    assert result.ok is True
    assert result.output_written is True
    assert result.output_exists is True


def test_compile_with_backend_daemon_probes_cache_without_ir_on_hit(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    seen_payloads: list[dict[str, object]] = []

    class _FakeSocket:
        def __init__(self) -> None:
            self._chunks: list[bytes] = []

        def settimeout(self, timeout: float) -> None:
            assert timeout == 0.1

        def connect(self, address: str) -> None:
            assert address == "/tmp/fake.sock"

        def sendall(self, data: bytes) -> None:
            payload = json.loads(data)
            seen_payloads.append(payload)
            backend_output.write_bytes(b"\x7fELF")
            self._chunks = [
                json.dumps(
                    {
                        "ok": True,
                        "jobs": [
                            {
                                "id": "job0",
                                "ok": True,
                                "cached": True,
                                "cache_tier": "module",
                                "output_written": True,
                            }
                        ],
                    }
                ).encode("utf-8")
                + b"\n"
            ]

        def shutdown(self, how: int) -> None:
            assert how in (cli.socket.SHUT_WR,)

        def recv_into(self, buffer: memoryview) -> int:
            if not self._chunks:
                return 0
            chunk = self._chunks.pop(0)
            buffer[: len(chunk)] = chunk
            return len(chunk)

        def close(self) -> None:
            return None

    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": [{"name": "heavy"}]},
        backend_output=backend_output,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key="function-cache",
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )

    assert result.ok is True
    assert len(seen_payloads) == 1
    assert seen_payloads[0]["jobs"][0]["probe_cache_only"] is True
    assert "ir" not in seen_payloads[0]["jobs"][0]


def test_compile_with_backend_daemon_retries_with_ir_after_probe_miss(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    seen_payloads: list[dict[str, object]] = []
    connects = 0

    class _FakeSocket:
        def __init__(self) -> None:
            self._chunks: list[bytes] = []

        def settimeout(self, timeout: float) -> None:
            assert timeout == 0.1

        def connect(self, address: str) -> None:
            nonlocal connects
            connects += 1
            assert address == "/tmp/fake.sock"

        def sendall(self, data: bytes) -> None:
            payload = json.loads(data)
            seen_payloads.append(payload)
            if len(seen_payloads) == 1:
                response = {
                    "ok": True,
                    "jobs": [
                        {
                            "id": "job0",
                            "ok": True,
                            "cached": False,
                            "output_written": False,
                            "needs_ir": True,
                        }
                    ],
                }
            else:
                backend_output.write_bytes(b"\x7fELF")
                response = {
                    "ok": True,
                    "jobs": [
                        {
                            "id": "job0",
                            "ok": True,
                            "cached": False,
                            "output_written": True,
                        }
                    ],
                }
            self._chunks = [json.dumps(response).encode("utf-8") + b"\n"]

        def shutdown(self, how: int) -> None:
            assert how in (cli.socket.SHUT_WR,)

        def recv_into(self, buffer: memoryview) -> int:
            if not self._chunks:
                return 0
            chunk = self._chunks.pop(0)
            buffer[: len(chunk)] = chunk
            return len(chunk)

        def close(self) -> None:
            return None

    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": [{"name": "heavy"}]},
        backend_output=backend_output,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key="function-cache",
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )

    assert result.ok is True
    assert len(seen_payloads) == 2
    assert connects == 1
    assert seen_payloads[0]["jobs"][0]["probe_cache_only"] is True
    assert "ir" not in seen_payloads[0]["jobs"][0]
    assert seen_payloads[1]["jobs"][0]["ir"] == {"functions": [{"name": "heavy"}]}


def test_compile_with_backend_daemon_defers_full_encode_until_probe_miss(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    calls: list[tuple[bool, bool]] = []

    original = cli._backend_daemon_compile_request_bytes

    def wrapped_compile_request_bytes(
        **kwargs: object,
    ) -> tuple[bytes | None, str | None]:
        calls.append(
            (bool(kwargs.get("probe_cache_only")), kwargs.get("ir") is not None)
        )
        return original(**kwargs)

    class _FakeSocket:
        def __init__(self) -> None:
            self._chunks: list[bytes] = []

        def settimeout(self, timeout: float) -> None:
            assert timeout == 0.1

        def connect(self, address: str) -> None:
            assert address == "/tmp/fake.sock"

        def sendall(self, data: bytes) -> None:
            payload = json.loads(data)
            if payload["jobs"][0].get("probe_cache_only"):
                backend_output.write_bytes(b"\x7fELF")
                response = {
                    "ok": True,
                    "jobs": [
                        {
                            "id": "job0",
                            "ok": True,
                            "cached": True,
                            "cache_tier": "module",
                            "output_written": True,
                        }
                    ],
                }
                self._chunks = [json.dumps(response).encode("utf-8") + b"\n"]
                return
            raise AssertionError("full IR request should not be sent on cache hit")

        def shutdown(self, how: int) -> None:
            assert how in (cli.socket.SHUT_WR,)

        def recv_into(self, buffer: memoryview) -> int:
            if not self._chunks:
                return 0
            chunk = self._chunks.pop(0)
            buffer[: len(chunk)] = chunk
            return len(chunk)

        def close(self) -> None:
            return None

    monkeypatch.setattr(
        cli, "_backend_daemon_compile_request_bytes", wrapped_compile_request_bytes
    )
    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": [{"name": "heavy"}]},
        backend_output=backend_output,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key="function-cache",
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )

    assert result.ok is True
    assert calls == [(True, False)]


def test_compile_with_backend_daemon_reports_missing_output_in_result(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def _fake_request(
        socket_path: Path,
        data: bytes,
        *,
        timeout: float | None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, data, timeout
        return (
            {
                "ok": True,
                "jobs": [
                    {
                        "id": "job0",
                        "ok": True,
                        "output_written": True,
                    }
                ],
            },
            None,
        )

    monkeypatch.setattr(cli, "_backend_daemon_request_bytes", _fake_request)
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": []},
        backend_output=Path("/tmp/definitely-missing-output.o"),
        target_triple=None,
        cache_key=None,
        function_cache_key=None,
        config_digest=None,
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )

    assert result.ok is False
    assert result.error == "backend daemon reported success but output is missing"
    assert result.output_written is True
    assert result.output_exists is False


def test_backend_daemon_request_bytes_accumulates_partial_chunks(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    sent: list[bytes] = []

    class _FakeSocket:
        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def settimeout(self, timeout: float) -> None:
            assert timeout == 0.25

        def connect(self, address: str) -> None:
            assert address == str(tmp_path / "daemon.sock")

        def sendall(self, data: bytes) -> None:
            sent.append(data)

        def shutdown(self, how: int) -> None:
            assert how == cli.socket.SHUT_WR

        def recv_into(self, buffer: memoryview) -> int:
            assert len(buffer) == 65536
            if not hasattr(self, "_chunks"):
                self._chunks = [b'{"ok":', b'true,"pong":false}', b""]
            chunk = self._chunks.pop(0)
            buffer[: len(chunk)] = chunk
            return len(chunk)

    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())

    response, err = cli._backend_daemon_request_bytes(
        tmp_path / "daemon.sock",
        b'{"version":1}\n',
        timeout=0.25,
    )

    assert err is None
    assert response == {"ok": True, "pong": False}
    assert sent == [b'{"version":1}\n']


def test_backend_daemon_request_bytes_rejects_whitespace_only_response(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    class _FakeSocket:
        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def settimeout(self, timeout: float) -> None:
            assert timeout == 0.25

        def connect(self, address: str) -> None:
            assert address == str(tmp_path / "daemon.sock")

        def sendall(self, data: bytes) -> None:
            assert data == b'{"version":1}\n'

        def shutdown(self, how: int) -> None:
            assert how == cli.socket.SHUT_WR

        def recv_into(self, buffer: memoryview) -> int:
            assert len(buffer) == 65536
            if not hasattr(self, "_chunks"):
                self._chunks = [b" \n\t", b""]
            chunk = self._chunks.pop(0)
            buffer[: len(chunk)] = chunk
            return len(chunk)

    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())

    response, err = cli._backend_daemon_request_bytes(
        tmp_path / "daemon.sock",
        b'{"version":1}\n',
        timeout=0.25,
    )

    assert response is None
    assert err == "backend daemon returned empty response"


def test_backend_daemon_skip_output_sync_flags_track_artifact_state(
    tmp_path: Path,
) -> None:
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key",
        tier="module",
        artifact=output_artifact,
    )

    skip_module, skip_function = cli._backend_daemon_skip_output_sync_flags(
        tmp_path,
        output_artifact,
        cache_key="module-key",
        function_cache_key="function-key",
    )

    assert skip_module is True
    assert skip_function is False


def test_backend_daemon_skip_output_sync_flags_stats_artifact_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key",
        tier="module",
        artifact=output_artifact,
    )
    original_stat = Path.stat
    calls = 0

    def wrapped_stat(self: Path):  # type: ignore[no-untyped-def]
        nonlocal calls
        if self == output_artifact:
            calls += 1
        return original_stat(self)

    monkeypatch.setattr(Path, "stat", wrapped_stat)

    skip_module, skip_function = cli._backend_daemon_skip_output_sync_flags(
        tmp_path,
        output_artifact,
        cache_key="module-key",
        function_cache_key="function-key",
    )

    assert skip_module is True
    assert skip_function is False
    assert calls == 1


def test_backend_daemon_skip_output_sync_flags_uses_known_sync_state_without_reread(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key",
        tier="module",
        artifact=output_artifact,
    )
    state = cli._read_artifact_sync_state(state_path)
    assert state is not None
    output_stat = output_artifact.stat()

    def fail_read(path: Path) -> dict[str, object] | None:
        raise AssertionError(f"unexpected sync-state read: {path}")

    monkeypatch.setattr(cli, "_read_artifact_sync_state", fail_read)

    skip_module, skip_function = cli._backend_daemon_skip_output_sync_flags(
        tmp_path,
        output_artifact,
        cache_key="module-key",
        function_cache_key="function-key",
        state_path=state_path,
        state=state,
        output_stat=output_stat,
    )

    assert skip_module is True
    assert skip_function is False


def test_read_artifact_sync_state_reuses_process_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._ARTIFACT_SYNC_STATE_CACHE.clear()
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key",
        tier="module",
        artifact=output_artifact,
    )

    first = cli._read_artifact_sync_state(state_path)

    def fail_read_text(*args: object, **kwargs: object) -> str:
        raise AssertionError("unexpected sync-state file read")

    monkeypatch.setattr(Path, "read_text", fail_read_text)
    second = cli._read_artifact_sync_state(state_path)

    assert first == second
    assert first is second
    assert second is not None
    assert second["source_key"] == "module-key"


def test_read_cached_json_object_reuses_same_payload_instance(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache_path = tmp_path / "cache" / "payload.json"
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    cli._PERSISTED_JSON_OBJECT_CACHE.clear()
    cli._write_cached_json_object(cache_path, {"version": 1, "hash": "abc"})

    first = cli._read_cached_json_object(cache_path)

    def fail_read_text(*args: object, **kwargs: object) -> str:
        raise AssertionError("unexpected cached-json file read")

    monkeypatch.setattr(Path, "read_text", fail_read_text)
    second = cli._read_cached_json_object(cache_path)

    assert first == second
    assert first is second
    assert second is not None
    assert second["hash"] == "abc"


def test_stage_backend_output_and_caches_promotes_module_cache(
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    function_cache_path = tmp_path / "cache" / "function.o"
    warnings: list[str] = []

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        function_cache_path=function_cache_path,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert cache_path.read_bytes() == b"artifact"
    assert function_cache_path.read_bytes() == b"artifact"
    assert not backend_output.exists()


def test_stage_backend_output_and_caches_reuses_cache_path_as_backend_output(
    tmp_path: Path,
) -> None:
    cache_path = tmp_path / "cache" / "module.o"
    cache_path.parent.mkdir(parents=True)
    cache_path.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    function_cache_path = tmp_path / "cache" / "function.o"
    warnings: list[str] = []

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        cache_path,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        function_cache_path=function_cache_path,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert cache_path.read_bytes() == b"artifact"
    assert output_artifact.read_bytes() == b"artifact"
    assert function_cache_path.read_bytes() == b"artifact"


def test_stage_backend_output_and_caches_skips_output_recopy_when_module_key_is_synced(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    cache_path = tmp_path / "cache" / "module.o"
    function_cache_path = tmp_path / "cache" / "function.o"
    warnings: list[str] = []

    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key",
        tier="module",
        artifact=output_artifact,
    )
    original_copy = cli._atomic_copy_file

    def fail_copy(src: Path, dst: Path) -> None:
        if dst == output_artifact:
            raise AssertionError(f"unexpected output sync {src} -> {dst}")
        original_copy(src, dst)

    monkeypatch.setattr(cli, "_atomic_copy_file", fail_copy)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        function_cache_path=function_cache_path,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert cache_path.read_bytes() == b"artifact"
    assert function_cache_path.read_bytes() == b"artifact"
    assert not backend_output.exists()


def test_stage_backend_output_and_caches_skips_state_rewrite_when_synced(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    cache_path = tmp_path / "cache" / "module.o"
    warnings: list[str] = []

    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key",
        tier="module",
        artifact=output_artifact,
    )

    def fail_write(*args: object, **kwargs: object) -> None:
        raise AssertionError("unexpected sync state rewrite")

    monkeypatch.setattr(cli, "_write_artifact_sync_state", fail_write)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        function_cache_path=None,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert cache_path.read_bytes() == b"artifact"
    assert not backend_output.exists()


def test_stage_backend_output_and_caches_uses_known_sync_state_without_reread(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    cache_path = tmp_path / "cache" / "module.o"
    warnings: list[str] = []

    def fail_read(path: Path) -> dict[str, object] | None:
        raise AssertionError(f"unexpected sync-state read: {path}")

    monkeypatch.setattr(cli, "_read_artifact_sync_state", fail_read)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        function_cache_path=None,
        warnings=warnings,
        output_already_synced=True,
    )

    assert err is None
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert cache_path.read_bytes() == b"artifact"
    assert not backend_output.exists()


def test_stage_backend_output_and_caches_prefers_link_or_copy_for_output_sync(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    function_cache_path = tmp_path / "cache" / "function.o"
    warnings: list[str] = []
    link_calls: list[tuple[Path, Path]] = []
    original_link_or_copy = cli._atomic_link_or_copy_file
    original_copy = cli._atomic_copy_file

    def record_link_or_copy(src: Path, dst: Path) -> None:
        link_calls.append((src, dst))
        original_link_or_copy(src, dst)

    def fail_copy(src: Path, dst: Path) -> None:
        if dst == output_artifact:
            raise AssertionError(f"unexpected copy {src} -> {dst}")
        original_copy(src, dst)

    monkeypatch.setattr(cli, "_atomic_link_or_copy_file", record_link_or_copy)
    monkeypatch.setattr(cli, "_atomic_copy_file", fail_copy)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        function_cache_path=function_cache_path,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert cache_path.read_bytes() == b"artifact"
    assert function_cache_path.read_bytes() == b"artifact"
    assert (cache_path, output_artifact) in link_calls


def test_stage_backend_output_and_caches_without_cache_moves_output(
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    warnings: list[str] = []

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=None,
        cache_key=None,
        function_cache_path=None,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert not backend_output.exists()


def test_stage_backend_output_and_caches_warns_on_function_cache_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    function_cache_path = tmp_path / "cache" / "function.o"
    warnings: list[str] = []
    original = cli._atomic_link_or_copy_file

    def wrapped(src: Path, dst: Path) -> None:
        if dst == function_cache_path:
            raise OSError("link failed")
        original(src, dst)

    monkeypatch.setattr(cli, "_atomic_link_or_copy_file", wrapped)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        function_cache_path=function_cache_path,
        warnings=warnings,
    )

    assert err is None
    assert output_artifact.read_bytes() == b"artifact"
    assert cache_path.read_bytes() == b"artifact"
    assert warnings == ["Function cache write failed: link failed"]


def test_materialize_cached_backend_artifact_promotes_module_cache_from_function_hit(
    tmp_path: Path,
) -> None:
    candidate = tmp_path / "cache" / "function.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    warnings: list[str] = []

    ok = cli._materialize_cached_backend_artifact(
        tmp_path,
        candidate,
        output_artifact,
        tier="function",
        source_key="function-key",
        cache_path=cache_path,
        warnings=warnings,
    )

    assert ok is True
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert cache_path.read_bytes() == b"artifact"


def test_try_cached_backend_candidates_promoted_function_hit_marks_module_synced(
    tmp_path: Path,
) -> None:
    candidate = tmp_path / "cache" / "function.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    warnings: list[str] = []

    ok, cache_hit_tier = cli._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=[("function", candidate)],
        output_artifact=output_artifact,
        is_wasm=False,
        cache_key="module-key",
        function_cache_key="function-key",
        cache_path=cache_path,
        warnings=warnings,
    )

    assert ok is True
    assert cache_hit_tier == "function"
    assert warnings == []
    skip_module, skip_function = cli._backend_daemon_skip_output_sync_flags(
        tmp_path,
        output_artifact,
        cache_key="module-key",
        function_cache_key="function-key",
    )
    assert skip_module is True
    assert skip_function is False


def test_materialize_cached_backend_artifact_skips_recopy_when_synced(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    candidate = tmp_path / "cache" / "module.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    warnings: list[str] = []

    first = cli._materialize_cached_backend_artifact(
        tmp_path,
        candidate,
        output_artifact,
        tier="module",
        source_key="module-key",
        cache_path=candidate,
        warnings=warnings,
    )
    assert first is True

    def fail_copy(src: Path, dst: Path) -> None:
        raise AssertionError(f"unexpected copy {src} -> {dst}")

    monkeypatch.setattr(cli, "_atomic_copy_file", fail_copy)

    second = cli._materialize_cached_backend_artifact(
        tmp_path,
        candidate,
        output_artifact,
        tier="module",
        source_key="module-key",
        cache_path=candidate,
        warnings=warnings,
    )
    assert second is True
    assert warnings == []


def test_materialize_cached_backend_artifact_prefers_link_or_copy_for_output_sync(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    candidate = tmp_path / "cache" / "module.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    warnings: list[str] = []
    link_calls: list[tuple[Path, Path]] = []
    original_link_or_copy = cli._atomic_link_or_copy_file
    original_copy = cli._atomic_copy_file

    def record_link_or_copy(src: Path, dst: Path) -> None:
        link_calls.append((src, dst))
        original_link_or_copy(src, dst)

    def fail_copy(src: Path, dst: Path) -> None:
        if dst == output_artifact:
            raise AssertionError(f"unexpected copy {src} -> {dst}")
        original_copy(src, dst)

    monkeypatch.setattr(cli, "_atomic_link_or_copy_file", record_link_or_copy)
    monkeypatch.setattr(cli, "_atomic_copy_file", fail_copy)

    ok = cli._materialize_cached_backend_artifact(
        tmp_path,
        candidate,
        output_artifact,
        tier="module",
        source_key="module-key",
        cache_path=candidate,
        warnings=warnings,
    )

    assert ok is True
    assert warnings == []
    assert output_artifact.read_bytes() == b"artifact"
    assert (candidate, output_artifact) in link_calls


def test_materialize_cached_backend_artifact_uses_known_sync_state_without_reread(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    candidate = tmp_path / "cache" / "module.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"artifact")
    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key",
        tier="module",
        artifact=output_artifact,
    )
    state = cli._read_artifact_sync_state(state_path)
    assert state is not None
    output_stat = output_artifact.stat()
    warnings: list[str] = []

    def fail_read(path: Path) -> dict[str, object] | None:
        raise AssertionError(f"unexpected sync-state read: {path}")

    monkeypatch.setattr(cli, "_read_artifact_sync_state", fail_read)

    ok = cli._materialize_cached_backend_artifact(
        tmp_path,
        candidate,
        output_artifact,
        tier="module",
        source_key="module-key",
        cache_path=candidate,
        warnings=warnings,
        state_path=state_path,
        state=state,
        output_stat=output_stat,
    )

    assert ok is True
    assert warnings == []


def test_temporary_backend_output_path_uses_expected_suffix_and_cleans_up(
    tmp_path: Path,
) -> None:
    with cli._temporary_backend_output_path(tmp_path, is_wasm=False) as path:
        assert path.suffix == ".o"
        assert not path.exists()
        path.write_bytes(b"artifact")
    assert not path.exists()

    with cli._temporary_backend_output_path(tmp_path, is_wasm=True) as path:
        assert path.suffix == ".wasm"
        assert not path.exists()
        path.write_bytes(b"artifact")
    assert not path.exists()


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
