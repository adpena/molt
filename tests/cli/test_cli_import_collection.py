from __future__ import annotations

import ast
import builtins as py_builtins
import contextlib
import io
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import types
from typing import cast

import molt.cli as cli
import pytest
from molt.frontend import MoltValue
from molt.type_facts import Fact, FunctionFacts, ModuleFacts, TypeFacts


ROOT = Path(__file__).resolve().parents[2]


def _load_generated_importer(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    module_names: list[str],
    intrinsics: dict[str, object],
):
    importer_path = cli._write_importer_module(module_names, tmp_path)
    intrinsics_mod = types.ModuleType("_intrinsics")

    def require_intrinsic(name: str, namespace=None):
        value = intrinsics.get(name)
        if value is None:
            raise RuntimeError(f"intrinsic unavailable: {name}")
        if namespace is not None:
            namespace[name] = value
        return value

    intrinsics_mod.require_intrinsic = require_intrinsic
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsics_mod)
    module_name = "molt_test_generated_importer"
    monkeypatch.delitem(sys.modules, module_name, raising=False)
    spec = importlib.util.spec_from_file_location(module_name, importer_path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


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


def test_generated_importer_recovers_known_placeholder_modules(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_name = "demo_math"
    placeholder = types.ModuleType(module_name)
    placeholder.__cached__ = "/tmp/demo_math.pyc"
    placeholder.__file__ = "/tmp/demo_math.py"
    placeholder.__loader__ = None
    placeholder.__package__ = ""
    placeholder.__spec__ = types.SimpleNamespace(loader=None)
    placeholder._molt_intrinsic_lookup = lambda name: None
    placeholder._molt_intrinsics = {}
    placeholder._molt_intrinsics_strict = True
    placeholder._molt_runtime = True
    loaded = types.ModuleType(module_name)
    loaded.sqrt = lambda value: value
    import_calls: list[str] = []

    def fake_import_module(name: str):
        import_calls.append(name)
        sys.modules[name] = loaded
        return loaded

    monkeypatch.setitem(sys.modules, module_name, placeholder)
    importer = _load_generated_importer(
        tmp_path,
        monkeypatch,
        module_names=[module_name],
        intrinsics={
            "molt_module_import": fake_import_module,
            "molt_importlib_import_module": lambda name, _util, _machinery: (
                fake_import_module(name)
            ),
        },
    )

    result = importer._molt_import(module_name)

    assert result is loaded
    assert sys.modules[module_name] is loaded
    assert import_calls == [module_name]


def test_generated_importer_can_import_builtins_when_not_preseeded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import_calls: list[str] = []

    def fake_import_module(name: str, _util, _machinery):
        import_calls.append(name)
        return py_builtins

    monkeypatch.delitem(sys.modules, "builtins", raising=False)
    importer = _load_generated_importer(
        tmp_path,
        monkeypatch,
        module_names=["demo_math"],
        intrinsics={
            "molt_module_import": lambda name: py_builtins,
            "molt_importlib_import_module": fake_import_module,
        },
    )

    result = importer._molt_import("builtins")

    assert result is py_builtins
    assert import_calls == ["builtins"]


def test_generated_importer_bootstraps_importlib_support_modules(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    util_mod = types.ModuleType("importlib.util")
    machinery_mod = types.ModuleType("importlib.machinery")
    loaded = types.ModuleType("demo_math")
    bootstrap_calls: list[str] = []
    import_calls: list[tuple[str, object, object]] = []

    def fake_module_import(name: str):
        bootstrap_calls.append(name)
        if name == "importlib.util":
            sys.modules[name] = util_mod
            return util_mod
        if name == "importlib.machinery":
            sys.modules[name] = machinery_mod
            return machinery_mod
        raise AssertionError(f"unexpected support import: {name}")

    def fake_import_module(name: str, util: object, machinery: object):
        import_calls.append((name, util, machinery))
        return loaded

    monkeypatch.delitem(sys.modules, "importlib.util", raising=False)
    monkeypatch.delitem(sys.modules, "importlib.machinery", raising=False)
    importer = _load_generated_importer(
        tmp_path,
        monkeypatch,
        module_names=["demo_math"],
        intrinsics={
            "molt_module_import": fake_module_import,
            "molt_importlib_import_module": fake_import_module,
        },
    )

    result = importer._molt_import("demo_math")

    assert result is loaded
    assert bootstrap_calls == ["importlib.util", "importlib.machinery"]
    assert import_calls == [("demo_math", util_mod, machinery_mod)]


def test_augment_support_modules_adds_importer_runtime_dependencies(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("value = 1\n")
    module_graph = {"demo": entry_path}
    module_reasons: dict[str, set[str]] = {}

    cli._augment_support_modules(
        module_graph=module_graph,
        module_reasons=module_reasons,
        roots=[tmp_path],
        stdlib_root=cli._stdlib_root_path(),
        stdlib_allowlist=set(),
        explicit_imports=set(),
        resolver_cache=cli._ModuleResolutionCache(),
        artifacts_root=tmp_path,
        stub_parents=set(),
        entry_module="demo",
        needs_generated_importer=True,
        needs_runtime_import_support=True,
        diagnostics_enabled=True,
    )

    assert "importlib.util" in module_graph
    assert "importlib.machinery" in module_graph
    assert "import_support" in module_reasons["importlib.util"]
    assert "import_support" in module_reasons["importlib.machinery"]


def test_augment_support_modules_skips_importlib_support_for_static_only_build(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("value = 1\n")
    module_graph = {"demo": entry_path}
    module_reasons: dict[str, set[str]] = {}
    extend_calls: list[dict[str, object]] = []

    def fake_extend_module_graph_with_closure(*args: object, **kwargs: object) -> None:
        extend_calls.append(kwargs)

    monkeypatch.setattr(
        cli,
        "_extend_module_graph_with_closure",
        fake_extend_module_graph_with_closure,
    )

    cli._augment_support_modules(
        module_graph=module_graph,
        module_reasons=module_reasons,
        roots=[tmp_path],
        stdlib_root=cli._stdlib_root_path(),
        stdlib_allowlist=set(),
        explicit_imports=set(),
        resolver_cache=cli._ModuleResolutionCache(),
        artifacts_root=tmp_path,
        stub_parents=set(),
        entry_module="demo",
        needs_generated_importer=False,
        needs_runtime_import_support=False,
        diagnostics_enabled=False,
    )

    assert extend_calls == []
    assert "importlib.util" not in module_graph
    assert "importlib.machinery" not in module_graph


def test_prepare_entry_module_graph_marks_static_entry_as_importer_free(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import math\nvalue = math.sqrt(4)\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert not prepared.needs_generated_importer
    assert not prepared.needs_runtime_import_support


def test_prepare_entry_module_graph_marks_dynamic_import_entry_as_runtime_supported(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text(
        "import importlib as loader\n"
        "value = loader.import_module('json')\n"
    )
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert not prepared.needs_generated_importer
    assert prepared.needs_runtime_import_support


def test_prepare_entry_module_graph_marks_generated_importer_references_explicitly(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import _molt_importer\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.needs_generated_importer
    assert prepared.needs_runtime_import_support


def test_prepare_entry_module_graph_marks_getattr_runtime_import_entry_as_supported(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text(
        "import importlib\n"
        "loader = getattr(importlib, 'import_module')\n"
        "value = loader('json')\n"
    )
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.needs_runtime_import_support


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


def test_run_subprocess_captured_to_tempfiles_does_not_block_on_inherited_pipes(
    tmp_path: Path,
) -> None:
    sleeper = tmp_path / "sleeper.py"
    sleeper.write_text(
        "import time\n"
        "time.sleep(2.0)\n",
        encoding="utf-8",
    )
    parent = tmp_path / "parent.py"
    parent.write_text(
        "import subprocess, sys\n"
        f"subprocess.Popen([sys.executable, {str(sleeper)!r}], stdout=sys.stdout, stderr=sys.stderr)\n"
        "print('parent-done', flush=True)\n",
        encoding="utf-8",
    )

    start = time.perf_counter()
    result = cli._run_subprocess_captured_to_tempfiles(
        [sys.executable, str(parent)],
        timeout=0.5,
    )
    elapsed = time.perf_counter() - start

    assert result.returncode == 0
    assert "parent-done" in cli._subprocess_output_text(result.stdout)
    assert elapsed < 1.0


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
    assert entry_override_by_module["app_entry"] == "app_entry"
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
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(tmp_path))
    monkeypatch.chdir(tmp_path)
    first = cli._stdlib_allowlist()
    second = cli._stdlib_allowlist()
    info = cli._stdlib_allowlist_cached.cache_info()
    assert {"json", "pathlib"} <= first
    assert second == first
    assert info.hits >= 1
    assert info.currsize >= 1
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


def test_discover_with_core_modules_includes_asyncio_ssl_dependency(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import asyncio\n")

    module_graph = _discover_with_core_modules(entry)

    assert "asyncio" in module_graph
    assert "ssl" in module_graph


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
    cli._stdlib_object_key_sidecar_path(stdlib_obj).write_text("stdlib-key\n")
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
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True)

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
        stdlib_object_cache_key="stdlib-key",
    )

    assert error is None
    assert prepared is not None
    staged_stdlib = artifacts_root / stdlib_obj.name
    assert captured_inputs == [
        tmp_path / "artifacts" / "main_stub.c",
        output_obj,
        runtime_lib,
        staged_stdlib,
    ]
    assert staged_stdlib.read_bytes() == b"stdlib"


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
    cli._stdlib_object_key_sidecar_path(stdlib_obj).write_text("stdlib-key\n")
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()

    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True)
    monkeypatch.setattr(
        cli,
        "_run_native_link_command",
        lambda **kwargs: subprocess.CompletedProcess(kwargs["link_cmd"], 0, "", ""),
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
        stdlib_object_cache_key="stdlib-key",
    )
    assert first_error is None
    assert first is not None

    stdlib_obj.write_bytes(b"stdlib-v2")
    cli._stdlib_object_key_sidecar_path(stdlib_obj).write_text("stdlib-key\n")

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
        stdlib_object_cache_key="stdlib-key",
    )
    assert second_error is None
    assert second is not None
    assert first.link_fingerprint["hash"] != second.link_fingerprint["hash"]


def test_prepare_native_link_stages_stdlib_object_for_link_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    output_binary = tmp_path / "app"
    stdlib_obj = tmp_path / "stdlib.o"
    stdlib_obj.write_bytes(b"stdlib")
    cli._stdlib_object_key_sidecar_path(stdlib_obj).write_text("stdlib-key\n")
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()
    captured_link_cmd: list[str] = []

    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True)

    def fake_run_native_link_command(
        *,
        link_cmd: list[str],
        json_output: bool,
        link_timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        del json_output, link_timeout
        captured_link_cmd[:] = link_cmd
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(cli, "_run_native_link_command", fake_run_native_link_command)

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
        stdlib_object_cache_key="stdlib-key",
    )

    assert error is None
    assert prepared is not None
    staged_stdlib = artifacts_root / stdlib_obj.name
    assert str(staged_stdlib) in captured_link_cmd
    assert staged_stdlib.read_bytes() == b"stdlib"


def test_build_native_link_command_does_not_read_ambient_stdlib_env(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.o"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_binary = tmp_path / "app"
    ambient_stdlib = tmp_path / "ambient.stdlib.o"
    output_obj.write_bytes(b"\x7fELFobject")
    stub_path.write_text("int main(void) { return 0; }\n")
    runtime_lib.write_bytes(b"archive")
    ambient_stdlib.write_bytes(b"stdlib")

    monkeypatch.setenv("CC", "clang")
    monkeypatch.setenv("MOLT_STDLIB_OBJ", str(ambient_stdlib))

    link_cmd, _linker_hint, _normalized_target = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        stdlib_obj_path=None,
    )

    assert str(ambient_stdlib) not in link_cmd


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
    monkeypatch.setattr(
        cli, "_read_persisted_import_scan", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        cli, "_read_persisted_module_graph", lambda *args, **kwargs: None
    )

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


def test_discover_module_graph_skips_persisted_caches_when_disabled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "pkg" / "__init__.py"
    entry.parent.mkdir()
    entry.write_text("import pkg.helper\n")
    helper = entry.parent / "helper.py"
    helper.write_text("VALUE = 1\n")

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
    assert graph["pkg.helper"] == helper

    def fail_persisted_graph(*args: object, **kwargs: object) -> None:
        raise AssertionError("unexpected persisted module-graph cache read")

    def fail_persisted_imports(*args: object, **kwargs: object) -> None:
        raise AssertionError("unexpected persisted import-scan cache read")

    # The cache_enabled parameter was removed from _discover_module_graph.
    # Verify that the graph resolves correctly when persisted caches return
    # None (cache miss), confirming the scanner re-derives the graph from
    # source without relying on stale persisted data.
    monkeypatch.setattr(
        cli, "_read_persisted_module_graph", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        cli, "_read_persisted_import_scan", lambda *args, **kwargs: None
    )

    graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )

    assert "pkg.helper" in explicit_imports
    assert graph["pkg.helper"] == helper


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
    expected = Path.cwd() / "external-target"
    assert first == second == expected
    assert info.hits >= 1
    assert info.currsize >= 1


def test_cargo_target_root_uses_canonical_session_subdir_when_unset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")

    target_root = cli._cargo_target_root(tmp_path)

    assert target_root == tmp_path / "target" / "sessions" / "alpha_session_beta"


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
    expected = Path.cwd() / "external-target" / ".molt_state"
    assert first == second == expected
    assert info.hits >= 1
    assert info.currsize >= 1


def test_build_state_root_uses_canonical_session_target_when_unset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._build_state_root_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")

    state_root = cli._build_state_root(tmp_path)

    assert state_root == (
        tmp_path / "target" / "sessions" / "alpha_session_beta" / ".molt_state"
    )


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
    monkeypatch.chdir(tmp_path)

    first = cli._lock_check_cache_path(tmp_path, "cargo")
    second = cli._lock_check_cache_path(tmp_path, "cargo")

    info = cli._lock_check_cache_path_cached.cache_info()
    expected = Path.cwd() / "external-target" / "lock_checks" / "cargo.json"
    assert first == second == expected
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_daemon_binary_is_newer_prefers_explicit_cargo_target_dir(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    (project_root / "Cargo.toml").write_text("[workspace]\n")
    backend_bin = project_root / "target" / "debug" / "molt-backend"
    backend_bin.parent.mkdir(parents=True)
    backend_bin.write_text("backend")
    pid_path = tmp_path / "daemon.pid"
    pid_path.write_text("1234")
    explicit_runtime = tmp_path / "explicit-target" / "release" / "libmolt_runtime.a"
    explicit_runtime.parent.mkdir(parents=True)
    explicit_runtime.write_text("runtime")

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "explicit-target"))
    os.utime(backend_bin, (1, 1))
    os.utime(pid_path, (2, 2))
    os.utime(explicit_runtime, (3, 3))

    assert cli._backend_daemon_binary_is_newer(backend_bin, pid_path) is True


def test_invalidate_stale_stdlib_cache_prefers_explicit_cargo_target_dir(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    (project_root / "Cargo.toml").write_text("[workspace]\n")
    stdlib_object = tmp_path / "stdlib_shared.o"
    stdlib_object.write_text("stdlib")
    explicit_runtime = tmp_path / "explicit-target" / "release" / "libmolt_runtime.a"
    explicit_runtime.parent.mkdir(parents=True)
    explicit_runtime.write_text("runtime")
    removed: list[Path] = []

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "explicit-target"))
    monkeypatch.setattr(
        cli,
        "_remove_shared_stdlib_cache_artifacts",
        lambda path: removed.append(path),
    )
    os.utime(stdlib_object, (2, 2))
    os.utime(explicit_runtime, (3, 3))

    cli._invalidate_stale_stdlib_cache(stdlib_object, project_root)

    assert removed == [stdlib_object]


def test_clean_repo_artifacts_removes_repo_local_cache_roots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    for relative in ("tmp", ".uv-cache", ".molt_cache", ".molt_cache-typing"):
        path = tmp_path / relative
        path.mkdir(parents=True)
        (path / "stamp").write_text(relative)

    monkeypatch.setattr(cli, "_find_molt_root", lambda _cwd: tmp_path)
    monkeypatch.setattr(
        cli,
        "_require_molt_root",
        lambda _root, _json_output, _command: None,
    )

    exit_code = cli.clean(cache=False, artifacts=False, repo_artifacts=True)

    assert exit_code == 0
    assert not (tmp_path / "tmp").exists()
    assert not (tmp_path / ".uv-cache").exists()
    assert not (tmp_path / ".molt_cache").exists()
    assert not (tmp_path / ".molt_cache-typing").exists()


def test_clean_cargo_target_removes_legacy_session_target_dirs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    canonical_target = tmp_path / "target"
    legacy_target = tmp_path / "target-alpha_session_beta"
    canonical_target.mkdir()
    legacy_target.mkdir()
    (canonical_target / "stamp").write_text("canonical")
    (legacy_target / "stamp").write_text("legacy")

    monkeypatch.setattr(cli, "_find_molt_root", lambda _cwd: tmp_path)
    monkeypatch.setattr(
        cli,
        "_require_molt_root",
        lambda _root, _json_output, _command: None,
    )

    exit_code = cli.clean(cache=False, artifacts=False, cargo_target=True)

    assert exit_code == 0
    assert not canonical_target.exists()
    assert not legacy_target.exists()


def test_verify_cargo_lock_uses_workspace_member_manifests_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root_manifest = tmp_path / "Cargo.toml"
    root_manifest.write_text(
        "[workspace]\n"
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


def test_backend_source_paths_are_feature_aware() -> None:
    native_paths = {
        path.relative_to(ROOT).as_posix() for path in cli._backend_source_paths(ROOT, ())
    }
    wasm_paths = {
        path.relative_to(ROOT).as_posix()
        for path in cli._backend_source_paths(ROOT, ("wasm-backend",))
    }
    rust_paths = {
        path.relative_to(ROOT).as_posix()
        for path in cli._backend_source_paths(ROOT, ("rust-backend",))
    }

    # Source-path tracking is intentionally feature-agnostic now: the whole
    # backend src/ tree is watched so new files are covered automatically.
    expected = {
        "runtime/molt-backend/src",
        "runtime/molt-backend/Cargo.toml",
        "runtime/molt-backend/build.rs",
        "Cargo.toml",
        "Cargo.lock",
    }
    assert native_paths == expected
    assert wasm_paths == expected
    assert rust_paths == expected


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
    expected = Path.cwd() / "external-target" / "dev-fast" / "molt-backend"
    assert first == second == expected
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
    expected = Path.cwd() / "relative-root"
    assert first == second == expected
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
    expected = Path.cwd() / "external-target" / "dev-fast" / "libmolt_runtime.a"
    assert first == second == expected
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
        Path.cwd()
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


@pytest.mark.skip(reason="_frontend_cache_epoch was removed from cli")
def test_load_module_analysis_invalidates_on_frontend_cache_epoch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\nVALUE = 1\n")
    cache = cli._ModuleResolutionCache()

    monkeypatch.setattr(cli, "_frontend_cache_epoch", lambda: "epoch-a")
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

    parse_calls = 0
    original_parse = cache.parse_module_ast

    def wrapped_parse(*args: object, **kwargs: object) -> ast.AST:
        nonlocal parse_calls
        parse_calls += 1
        return original_parse(*args, **kwargs)

    monkeypatch.setattr(cli, "_frontend_cache_epoch", lambda: "epoch-b")
    monkeypatch.setattr(cache, "parse_module_ast", wrapped_parse)
    (
        tree,
        imports,
        func_defaults,
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

    assert tree is not None
    assert imports == ("warnings",)
    assert func_defaults == {}
    assert cached_source is not None
    assert cache_hit is False
    assert interface_changed is True
    assert cached_path_stat is not None
    assert parse_calls == 1


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


@pytest.mark.skip(reason="_frontend_cache_epoch was removed from cli")
def test_persisted_module_lowering_invalidates_on_frontend_cache_epoch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("x = 1\n")
    context_digest = cli._module_lowering_context_digest({"module": "pkg", "v": 1})
    assert context_digest is not None

    monkeypatch.setattr(cli, "_frontend_cache_epoch", lambda: "epoch-a")
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

    monkeypatch.setattr(cli, "_frontend_cache_epoch", lambda: "epoch-b")
    cached = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
    )

    assert cached is None


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


@pytest.mark.skip(reason="cache_enabled parameter was removed from _prepare_frontend_parallel_batch")
def test_prepare_frontend_parallel_batch_skips_cache_reads_when_disabled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "alpha.py"
    module_path.write_text("VALUE = 1\n")
    module_graph_metadata = cli._build_module_graph_metadata(
        {"alpha": module_path},
        generated_module_source_paths={},
        entry_module="__main__",
        namespace_module_names=set(),
    )

    monkeypatch.setattr(
        cli,
        "_load_cached_module_lowering_result",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected cached module lowering read")
        ),
    )

    cached_results, worker_payloads, context_digest_by_module, batch_error = (
        cli._prepare_frontend_parallel_batch(
            ["alpha"],
            module_graph={"alpha": module_path},
            module_sources={},
            project_root=tmp_path,
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
            cache_enabled=False,
        )
    )

    assert batch_error is None
    assert cached_results == {}
    assert len(worker_payloads) == 1
    assert context_digest_by_module == {}


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


def test_module_lowering_context_payload_tracks_frontend_tooling_fingerprint(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    kwargs = dict(
        module_name="main",
        module_path=Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override=None,
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={"main"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    monkeypatch.setattr(cli, "_cache_tooling_fingerprint", lambda: "tool-a")
    payload_a = cli._module_lowering_context_payload(**kwargs)
    monkeypatch.setattr(cli, "_cache_tooling_fingerprint", lambda: "tool-b")
    payload_b = cli._module_lowering_context_payload(**kwargs)

    assert payload_a is not None
    assert payload_b is not None
    assert payload_a["compiler_fingerprint"] == "tool-a"
    assert payload_b["compiler_fingerprint"] == "tool-b"
    assert cli._module_lowering_context_digest(payload_a) != cli._module_lowering_context_digest(
        payload_b
    )


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
    assert isinstance(scoped, dict)
    assert set(scoped["modules"]) == {"main", "alpha"}


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

    assert (
        scoped_view.known_modules
        is scoped_lowering_inputs.known_modules_by_module["main"]
    )
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


def test_module_lowering_context_digest_ignores_type_facts_metadata_noise() -> None:
    facts_a = TypeFacts(
        created_at="2026-04-09T00:00:00Z",
        tool="facts-a",
        strict=True,
        modules={
            "main": ModuleFacts(
                globals={"VALUE": Fact(type="int", trust="trusted")},
            )
        },
    )
    facts_b = TypeFacts(
        created_at="2026-04-10T00:00:00Z",
        tool="facts-b",
        strict=True,
        modules={
            "main": ModuleFacts(
                globals={"VALUE": Fact(type="int", trust="trusted")},
            )
        },
    )

    digest_a = cli._module_lowering_context_digest_for_module(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override="main",
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="trust",
        fallback_policy="error",
        type_facts=facts_a,
        enable_phi=True,
        known_modules={"main"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        module_dep_closures={"main": frozenset({"main"})},
        scoped_inputs=cli._ScopedLoweringInputView(
            known_modules=("main",),
            known_func_defaults={},
            pgo_hot_function_names=(),
            type_facts=facts_a,
        ),
        scoped_known_classes={},
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )
    digest_b = cli._module_lowering_context_digest_for_module(
        "main",
        Path("/tmp/main.py"),
        logical_source_path="/tmp/main.py",
        entry_override="main",
        known_classes_snapshot={},
        parse_codec="json",
        type_hint_policy="trust",
        fallback_policy="error",
        type_facts=facts_b,
        enable_phi=True,
        known_modules={"main"},
        stdlib_allowlist=set(),
        known_func_defaults={},
        module_deps={"main": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        module_dep_closures={"main": frozenset({"main"})},
        scoped_inputs=cli._ScopedLoweringInputView(
            known_modules=("main",),
            known_func_defaults={},
            pgo_hot_function_names=(),
            type_facts=facts_b,
        ),
        scoped_known_classes={},
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert isinstance(digest_a, str)
    assert digest_a
    assert digest_a == digest_b


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
    # _start_backend_daemon is always called now (socket existence is
    # checked internally).  Stub it to report the daemon as ready.
    monkeypatch.setattr(
        cli,
        "_start_backend_daemon",
        lambda *args, **kwargs: True,
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
        entry_module: str | None = None,
        stdlib_object_path: Path | None = None,
        stdlib_object_cache_key: str | None = None,
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
            entry_module,
            stdlib_object_path,
            stdlib_object_cache_key,
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


def test_build_emit_obj_does_not_route_stdlib_object_env_from_helper(
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
    assert "MOLT_STDLIB_CACHE_KEY" in seen_backend_env


def test_stdlib_object_cache_path_tracks_build_variant(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache_root = tmp_path / "cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))

    base = cli._stdlib_object_cache_path(tmp_path / "program.o", "variant=a")
    same = cli._stdlib_object_cache_path(tmp_path / "program.o", "variant=a")
    changed = cli._stdlib_object_cache_path(tmp_path / "program.o", "variant=b")

    assert base is not None
    assert same == base
    assert changed is not None
    assert changed != base
    assert base.parent == cache_root
    assert base.name.startswith("stdlib_shared_")
    assert base.suffix == ".o"


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
    stdlib_object_path: Path | None = None,
    stdlib_object_cache_key: str | None = None,
    timeout: float | None,
    request_bytes: bytes | None = None,
    daemon_pid: int | None = None,
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
        stdlib_object_path=stdlib_object_path,
        stdlib_object_cache_key=stdlib_object_cache_key,
        timeout=timeout,
        request_bytes=request_bytes,
        daemon_pid=daemon_pid,
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
    assert metadata.entry_override_by_module["app_entry"] == "app_entry"
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
    assert view.entry_override == "pkg"
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


def test_prepare_frontend_lowering_config_uses_tighter_native_chunk_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source_path = tmp_path / "entry.py"
    source_path.write_text("print('ok')\n", encoding="utf-8")
    monkeypatch.delenv("MOLT_MODULE_CHUNK_OPS", raising=False)
    monkeypatch.delenv("MOLT_WASM_MODULE_CHUNK_OPS", raising=False)

    config, failure = cli._prepare_frontend_lowering_config(
        type_facts_path=None,
        type_hint_policy="ignore",
        module_graph={"entry": source_path},
        source_path=source_path,
        json_output=False,
        warnings=[],
        module_deps={"entry": set()},
        module_dep_closures={"entry": set()},
        has_back_edges=False,
        known_modules={"entry"},
        known_func_defaults={},
        pgo_hot_function_names=set(),
        generated_module_source_paths={},
        entry_module="entry",
        namespace_module_names=set(),
        module_sources={"entry": "print('ok')\n"},
        is_wasm=False,
        target_triple=None,
        frontend_parallel_details={},
        frontend_phase_timeout=None,
    )

    assert failure is None
    assert config is not None
    assert config.module_chunking is True
    assert config.module_chunk_max_ops == 1400


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
    roots_root = tmp_path / "module_roots"
    stdlib_root = roots_root / "stdlib"
    roots = [roots_root / "project", roots_root / "src"]
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
    assert profile == "dev-fast"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "my-dev_1")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "my-dev_1"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "bad profile")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "dev-fast"
    assert error == "Invalid MOLT_DEV_CARGO_PROFILE value: bad profile"


def test_resolve_backend_cargo_profile_name_defaults_and_validation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli._resolve_backend_cargo_profile_name_cached.cache_clear()
    monkeypatch.delenv("MOLT_DEV_BACKEND_CARGO_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_DEV_CARGO_PROFILE", raising=False)
    profile, error = cli._resolve_backend_cargo_profile_name("dev")
    assert profile == "dev-fast"
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
    assert profile == "release-fast"
    assert error == "Invalid MOLT_RELEASE_BACKEND_CARGO_PROFILE value: bad profile"


def test_backend_daemon_retryable_error_classification() -> None:
    assert cli._backend_daemon_retryable_error("backend daemon returned empty response")
    assert cli._backend_daemon_retryable_error("unsupported protocol version 9")
    assert cli._backend_daemon_retryable_error(
        "backend daemon connection failed: timeout"
    )
    assert cli._backend_daemon_retryable_error(
        "backend daemon died while request was in flight"
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
    assert cli._backend_daemon_start_timeout() == 120.0


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
    ready_results = iter([False, True])

    class _FakePopen:
        pid = 4321

    monkeypatch.setattr(
        cli, "_backend_daemon_pid_path", lambda *args, **kwargs: pid_path
    )
    monkeypatch.setattr(
        cli, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(cli, "_read_backend_daemon_pid", lambda *args, **kwargs: None)

    def fake_wait_until_ready(
        *args: object, **kwargs: object
    ) -> tuple[bool, dict[str, object] | None]:
        del args
        wait_timeouts.append(cast(float | None, kwargs.get("ready_timeout")))
        return next(ready_results), None

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
            warnings=[],
        )
        is False
    )
    assert wait_timeouts == [0.25]
    assert pid_path.read_text().strip() == "4321"
    assert terminated == []
    assert removed == []


def test_start_backend_daemon_uses_short_probe_for_stale_socket_with_live_pid(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("backend")
    socket_path = tmp_path / "daemon.sock"
    socket_path.write_text("")
    pid_path = tmp_path / "daemon.pid"
    pid_path.write_text("1234")
    log_path = tmp_path / "daemon.log"
    wait_timeouts: list[float | None] = []
    terminated: list[int] = []
    removed: list[Path] = []
    ready_results = iter([False, True])

    class _FakePopen:
        pid = 4321

    monkeypatch.setattr(
        cli, "_backend_daemon_pid_path", lambda *args, **kwargs: pid_path
    )
    monkeypatch.setattr(
        cli, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(cli, "_read_backend_daemon_pid", lambda *args, **kwargs: 1234)
    monkeypatch.setattr(cli, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(
        cli, "_backend_daemon_binary_is_newer", lambda *args, **kwargs: False
    )

    def fake_wait_until_ready(
        *args: object, **kwargs: object
    ) -> tuple[bool, dict[str, object] | None]:
        del args
        wait_timeouts.append(cast(float | None, kwargs.get("ready_timeout")))
        return next(ready_results), None

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
            warnings=[],
        )
        is True
    )
    assert wait_timeouts == [0.25, 0.25]
    assert terminated == [1234]
    assert removed == [pid_path]


def test_start_backend_daemon_ignores_foreign_socket_dir_entries(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import tempfile

    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("backend")
    build_state_root = tmp_path / "target" / ".molt_state"
    wait_timeouts: list[float | None] = []

    pid_path = build_state_root / "backend_daemon" / "molt-backend.dev-fast.pid"
    log_path = build_state_root / "backend_daemon" / "molt-backend.dev-fast.log"

    class _FakePopen:
        pid = 4321

    def fake_wait_until_ready(
        *args: object, **kwargs: object
    ) -> tuple[bool, dict[str, object] | None]:
        del args
        wait_timeouts.append(cast(float | None, kwargs.get("ready_timeout")))
        return True, None

    spawn_calls: list[list[str]] = []

    def fake_popen(cmd: list[str], **kwargs: object) -> _FakePopen:
        spawn_calls.append(cmd)
        return _FakePopen()

    monkeypatch.setattr(
        cli, "_backend_daemon_pid_path", lambda *args, **kwargs: pid_path
    )
    monkeypatch.setattr(
        cli, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(cli, "_read_backend_daemon_pid", lambda *args, **kwargs: None)
    monkeypatch.setattr(cli, "_backend_daemon_wait_until_ready", fake_wait_until_ready)
    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    with tempfile.TemporaryDirectory(prefix="moltbd-test-", dir=tempfile.gettempdir()) as sockdir:
        socket_dir = Path(sockdir)
        socket_path = socket_dir / "moltbd.current.sock"

        for idx in range(3):
            (socket_dir / f"moltbd.foreign{idx}.sock").write_text("")

        assert (
            cli._start_backend_daemon(
                backend_bin,
                socket_path,
                cargo_profile="dev-fast",
                project_root=tmp_path,
                startup_timeout=2.0,
                json_output=True,
                warnings=[],
            )
            is True
        )

        assert spawn_calls == [
            [str(backend_bin), "--daemon", "--socket", str(socket_path)]
        ]
        assert wait_timeouts == [0.25]
        assert not socket_path.with_suffix(".redirect").exists()
        for idx in range(3):
            assert (socket_dir / f"moltbd.foreign{idx}.sock").exists()


def test_start_backend_daemon_rebuild_prefers_explicit_cargo_target_dir(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import shutil

    project_root = tmp_path / "project"
    project_root.mkdir()
    backend_bin = project_root / "target" / "debug" / "molt-backend"
    backend_bin.parent.mkdir(parents=True)
    backend_bin.write_text("backend")
    socket_path = tmp_path / "daemon.sock"
    pid_path = tmp_path / "daemon.pid"
    pid_path.write_text("1234")
    log_path = tmp_path / "daemon.log"
    explicit_target = tmp_path / "explicit-target"
    captured_env: dict[str, str] = {}

    class _FakePopen:
        pid = 4321

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(explicit_target))
    monkeypatch.setattr(cli, "_backend_daemon_pid_path", lambda *args, **kwargs: pid_path)
    monkeypatch.setattr(cli, "_backend_daemon_log_path", lambda *args, **kwargs: log_path)
    monkeypatch.setattr(cli, "_read_backend_daemon_pid", lambda *args, **kwargs: 1234)
    monkeypatch.setattr(cli, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(
        cli, "_backend_daemon_binary_is_newer", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(cli, "_terminate_backend_daemon_pid", lambda *args, **kwargs: None)
    monkeypatch.setattr(cli, "_remove_backend_daemon_pid", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        cli, "_backend_daemon_wait_until_ready", lambda *args, **kwargs: (True, None)
    )
    monkeypatch.setattr(cli, "_build_slot", lambda: contextlib.nullcontext(0))
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/cargo")

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
        env = cast(dict[str, str] | None, kwargs.get("env"))
        captured_env.update(env or {})
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(cli.subprocess, "run", fake_run)
    monkeypatch.setattr(cli.subprocess, "Popen", lambda *args, **kwargs: _FakePopen())

    assert (
        cli._start_backend_daemon(
            backend_bin,
            socket_path,
            cargo_profile="dev-fast",
            project_root=project_root,
            startup_timeout=2.0,
            json_output=True,
            warnings=[],
        )
        is True
    )
    assert captured_env["CARGO_TARGET_DIR"] == str(explicit_target)


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
            stdlib_object_cache_key=None,
            cache_candidates=(("module", tmp_path / "module-cache.o"),),
            cache_hit=True,
            cache_hit_tier="module",
        )

    monkeypatch.setattr(
        cli, "_prepare_backend_cache_setup", fake_prepare_backend_cache_setup
    )
    monkeypatch.setattr(
        cli,
        "_ensure_runtime_lib_ready",
        lambda runtime_state, **kwargs: ensure_calls.append(runtime_state.runtime_lib)
        or True,
    )
    monkeypatch.setattr(
        cli,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: None,
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
        entry_module="__main__",
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert prepared_backend_setup.cache_hit is True
    assert ensure_calls == []


def test_prepare_backend_setup_defers_runtime_lib_ready_check_for_native_cache_miss(
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
            stdlib_object_cache_key=None,
            cache_candidates=(("module", tmp_path / "module-cache.o"),),
            cache_hit=False,
            cache_hit_tier=None,
        )

    monkeypatch.setattr(
        cli, "_prepare_backend_cache_setup", fake_prepare_backend_cache_setup
    )
    monkeypatch.setattr(
        cli,
        "_ensure_runtime_lib_ready",
        lambda runtime_state, **kwargs: ensure_calls.append(runtime_state.runtime_lib)
        or True,
    )
    monkeypatch.setattr(
        cli,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: None,
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
        entry_module="__main__",
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert prepared_backend_setup.cache_hit is False
    assert ensure_calls == []


def test_prepare_backend_setup_starts_native_runtime_build_async(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_artifact = tmp_path / "output.o"
    scheduled: list[tuple[Path | None, str | None, str, frozenset[str]]] = []

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
            stdlib_object_cache_key=None,
            cache_candidates=(("module", tmp_path / "module-cache.o"),),
            cache_hit=False,
            cache_hit_tier=None,
        )

    monkeypatch.setattr(
        cli, "_prepare_backend_cache_setup", fake_prepare_backend_cache_setup
    )
    monkeypatch.setattr(
        cli,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda runtime_state, **kwargs: scheduled.append(
            (
                runtime_state.runtime_lib,
                cast(str | None, kwargs["target_triple"]),
                cast(str, kwargs["runtime_cargo_profile"]),
                frozenset(cast(set[str], kwargs["resolved_modules"])),
            )
        ),
    )

    prepared_backend_setup, backend_setup_error = cli._prepare_backend_setup(
        is_rust_transpile=False,
        is_wasm=False,
        emit_mode="bin",
        molt_root=tmp_path,
        runtime_cargo_profile="release-fast",
        target_triple=None,
        json_output=True,
        cargo_timeout=1.0,
        target="native",
        profile="release",
        backend_cargo_profile="release-fast",
        linked=False,
        project_root=tmp_path,
        cache_dir=None,
        output_artifact=output_artifact,
        warnings=[],
        cache=True,
        ir={"functions": []},
        entry_module="__main__",
        resolved_modules={"__main__", "json"},
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert scheduled == [(runtime_lib, None, "release-fast", frozenset({"__main__", "json"}))]


def test_prepare_backend_setup_skips_native_runtime_build_async_for_object_emit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_artifact = tmp_path / "output.o"
    scheduled: list[Path | None] = []

    monkeypatch.setattr(
        cli,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: cli._RuntimeArtifactState(runtime_lib=runtime_lib),
    )
    monkeypatch.setattr(
        cli,
        "_prepare_backend_cache_setup",
        lambda **kwargs: cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="module-cache",
            function_cache_key=None,
            cache_path=tmp_path / "module-cache.o",
            function_cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            cache_candidates=(("module", tmp_path / "module-cache.o"),),
            cache_hit=False,
            cache_hit_tier=None,
        ),
    )
    monkeypatch.setattr(
        cli,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda runtime_state, **kwargs: scheduled.append(runtime_state.runtime_lib),
    )

    prepared_backend_setup, backend_setup_error = cli._prepare_backend_setup(
        is_rust_transpile=False,
        is_wasm=False,
        emit_mode="obj",
        molt_root=tmp_path,
        runtime_cargo_profile="release-fast",
        target_triple=None,
        json_output=True,
        cargo_timeout=1.0,
        target="native",
        profile="release",
        backend_cargo_profile="release-fast",
        linked=False,
        project_root=tmp_path,
        cache_dir=None,
        output_artifact=output_artifact,
        warnings=[],
        cache=True,
        ir={"functions": []},
        entry_module="__main__",
        resolved_modules={"__main__"},
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert scheduled == []


def test_ensure_native_runtime_lib_ready_before_link_awaits_async_future(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class FakeFuture:
        def __init__(self) -> None:
            self.calls = 0

        def result(self) -> bool:
            self.calls += 1
            return True

    fake_future = FakeFuture()
    runtime_state = cli._RuntimeArtifactState(
        runtime_lib=tmp_path / "libmolt_runtime.a",
        runtime_lib_ready_future=fake_future,
    )
    monkeypatch.setattr(
        cli,
        "_ensure_runtime_lib_ready",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("sync runtime build should not run when async future exists")
        ),
    )
    phase_starts: dict[str, float] = {}

    ready = cli._ensure_native_runtime_lib_ready_before_link(
        runtime_state,
        target_triple=None,
        json_output=True,
        runtime_cargo_profile="release-fast",
        molt_root=tmp_path,
        cargo_timeout=1.0,
        diagnostics_enabled=False,
        phase_starts=phase_starts,
    )

    assert ready is True
    assert fake_future.calls == 1


def test_ensure_native_runtime_lib_ready_before_link_passes_resolved_modules(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_state = cli._RuntimeArtifactState(runtime_lib=tmp_path / "libmolt_runtime.a")
    captured: list[frozenset[str]] = []

    monkeypatch.setattr(
        cli,
        "_ensure_runtime_lib_ready",
        lambda runtime_state, **kwargs: captured.append(
            frozenset(cast(set[str], kwargs["resolved_modules"]))
        )
        or True,
    )

    ready = cli._ensure_native_runtime_lib_ready_before_link(
        runtime_state,
        target_triple=None,
        json_output=True,
        runtime_cargo_profile="release-fast",
        molt_root=tmp_path,
        cargo_timeout=1.0,
        diagnostics_enabled=False,
        phase_starts={},
        resolved_modules={"json", "socket"},
    )

    assert ready is True
    assert captured == [frozenset({"json", "socket"})]


def test_prepare_backend_runtime_context_passes_resolved_modules_to_wasm_runtime(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_state = cli._RuntimeArtifactState(
        runtime_wasm=tmp_path / "molt_runtime.wasm",
        runtime_reloc_wasm=tmp_path / "molt_runtime_reloc.wasm",
    )
    prepared_backend_setup = cli._PreparedBackendSetup(
        runtime_state=runtime_state,
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            function_cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
        ),
        cache_hit=False,
        cache_hit_tier=None,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        stdlib_object_path=None,
        cache_candidates=[],
    )
    captured: list[tuple[bool, frozenset[str]]] = []

    monkeypatch.setattr(
        cli,
        "_ensure_runtime_wasm_artifact",
        lambda runtime_state, *, reloc, **kwargs: captured.append(
            (reloc, frozenset(cast(set[str], kwargs["resolved_modules"])))
        )
        or True,
    )

    runtime_context = cli._prepare_backend_runtime_context(
        prepared_backend_setup=prepared_backend_setup,
        is_wasm_freestanding=False,
        json_output=True,
        runtime_cargo_profile="dev-fast",
        cargo_timeout=1.0,
        molt_root=tmp_path,
        stdlib_profile="micro",
        resolved_modules={"asyncio", "ssl"},
    )

    assert runtime_context.ensure_runtime_wasm_shared() is True
    assert runtime_context.ensure_runtime_wasm_reloc() is True
    assert captured == [
        (False, frozenset({"asyncio", "ssl"})),
        (True, frozenset({"asyncio", "ssl"})),
    ]


def test_prepare_backend_dispatch_prefers_reloc_runtime_for_wasm_layout_probe(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    runtime_reloc_wasm = tmp_path / "molt_runtime_reloc.wasm"
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0")
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("")

    calls: list[tuple[str, object | None]] = []

    monkeypatch.delenv("MOLT_WASM_DATA_BASE", raising=False)
    monkeypatch.delenv("MOLT_WASM_TABLE_BASE", raising=False)
    monkeypatch.delenv("MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN", raising=False)
    monkeypatch.setattr(cli, "_backend_bin_path", lambda *args, **kwargs: backend_bin)
    monkeypatch.setattr(cli, "_ensure_backend_binary", lambda *args, **kwargs: True)
    monkeypatch.setattr(cli, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(cli, "wasm_runtime_required_import_names", lambda modules: ())
    monkeypatch.setattr(
        cli,
        "_read_wasm_data_end",
        lambda path: 4096 if path == runtime_reloc_wasm else None,
    )
    monkeypatch.setattr(
        cli,
        "_read_wasm_memory_min_bytes",
        lambda path: 8192 if path == runtime_reloc_wasm else None,
    )
    monkeypatch.setattr(
        cli,
        "_read_wasm_table_min",
        lambda path: 1234 if path == runtime_reloc_wasm else None,
    )

    def ensure_shared(required=None):
        calls.append(("shared", frozenset(required) if required else None))
        return True

    def ensure_reloc():
        calls.append(("reloc", None))
        runtime_reloc_wasm.write_bytes(b"\0asm\x01\0\0\0")
        return True

    prepared, err = cli._prepare_backend_dispatch(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        split_runtime=True,
        linked=True,
        deterministic=False,
        profile="dev",
        runtime_state=cli._RuntimeArtifactState(
            runtime_wasm=runtime_wasm,
            runtime_reloc_wasm=runtime_reloc_wasm,
        ),
        runtime_cargo_profile="dev-fast",
        cargo_timeout=1.0,
        molt_root=tmp_path,
        target_triple=None,
        backend_cargo_profile="dev-fast",
        diagnostics_enabled=False,
        phase_starts={},
        json_output=True,
        backend_daemon_config_digest=None,
        ensure_runtime_wasm_shared=ensure_shared,
        ensure_runtime_wasm_reloc=ensure_reloc,
        resolved_modules=frozenset(),
        warnings=[],
    )

    assert err is None
    assert prepared is not None
    assert calls == [("reloc", None)]
    assert prepared.backend_env is not None
    assert prepared.backend_env["MOLT_WASM_DATA_BASE"] == str(64 * 1024 * 1024 + 8192)
    assert prepared.backend_env["MOLT_WASM_TABLE_BASE"] == "1234"


def test_ensure_runtime_wasm_verified_key_tracks_micro_builtin_feature_shape(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0")
    verification_calls: list[frozenset[str]] = []

    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda project_root, **kwargs: {
            "runtime_features": tuple(cast(tuple[str, ...], kwargs["runtime_features"]))
        },
    )
    monkeypatch.setattr(
        cli,
        "_artifact_needs_rebuild",
        lambda artifact, fingerprint, stored_fingerprint: verification_calls.append(
            frozenset(cast(tuple[str, ...], fingerprint["runtime_features"]))
        )
        or False,
    )
    monkeypatch.setattr(cli, "_is_valid_runtime_wasm_artifact", lambda path: True)

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules={"json"},
    )
    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules={"ssl"},
    )

    assert len(verification_calls) == 2
    assert verification_calls[0] != verification_calls[1]
    assert "stdlib_net" in verification_calls[1]


def test_ensure_runtime_wasm_writes_integrity_sidecar_after_copy(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    built_src = tmp_path / "target" / "wasm32-wasip1" / "dev-fast" / "deps" / "molt_runtime-test.wasm"
    built_src.parent.mkdir(parents=True, exist_ok=True)
    built_src.write_bytes(b"\0asm\x01\0\0\0runtime")

    monkeypatch.setattr(cli, "_runtime_fingerprint", lambda *args, **kwargs: {"hash": "new"})
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True)
    monkeypatch.setattr(cli, "_inspect_wasm_binary", lambda path: "valid")
    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        lambda **kwargs: (subprocess.CompletedProcess(kwargs["cmd"], 0, "", ""), built_src),
    )
    monkeypatch.setattr(cli, "_write_runtime_fingerprint", lambda *args, **kwargs: None)

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules=None,
        required_exports=None,
    )

    sidecar = runtime_wasm.with_name(f"{runtime_wasm.name}.sha256")
    assert runtime_wasm.read_bytes() == built_src.read_bytes()
    assert sidecar.exists()
    assert sidecar.read_text(encoding="utf-8").strip() == cli._sha256_file(runtime_wasm)


def test_ensure_runtime_wasm_writes_integrity_sidecar_when_reusing_valid_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0runtime")

    monkeypatch.setattr(cli, "_runtime_fingerprint", lambda *args, **kwargs: {"hash": "same"})
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: False)
    monkeypatch.setattr(cli, "_is_valid_runtime_wasm_artifact", lambda path: True)
    monkeypatch.setattr(cli, "_runtime_wasm_exports_satisfy", lambda path, required: True)
    monkeypatch.setattr(cli, "_inspect_wasm_binary", lambda path: "valid")
    monkeypatch.setattr(
        cli,
        "_resolve_built_runtime_wasm_artifact",
        lambda target_root, profile_dir: runtime_wasm,
    )

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules=None,
        required_exports=None,
    )

    sidecar = runtime_wasm.with_name(f"{runtime_wasm.name}.sha256")
    assert sidecar.exists()
    assert sidecar.read_text(encoding="utf-8").strip() == cli._sha256_file(runtime_wasm)


def test_prepare_non_native_build_result_stages_runtime_wasm_sidecar(
    tmp_path: Path,
) -> None:
    output_wasm = tmp_path / "out" / "output.wasm"
    output_wasm.parent.mkdir(parents=True, exist_ok=True)
    output_wasm.write_bytes(b"\0asm\x01\0\0\0")
    runtime_wasm = tmp_path / "runtime" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0runtime")

    prepared, err = cli._prepare_non_native_build_result(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        is_wasm_freestanding=False,
        linked=False,
        require_linked=False,
        linked_output_path=None,
        output_artifact=output_wasm,
        json_output=True,
        runtime_wasm=runtime_wasm,
        runtime_reloc_wasm=None,
        ensure_runtime_wasm_shared=lambda *_args, **_kwargs: True,
        ensure_runtime_wasm_reloc=lambda: True,
        molt_root=tmp_path,
        split_runtime=False,
        precompile=False,
    )

    assert err is None
    assert prepared is not None
    staged = output_wasm.parent / "molt_runtime.wasm"
    assert staged.exists()
    assert staged.read_bytes() == runtime_wasm.read_bytes()
    assert prepared.artifacts is not None
    assert prepared.artifacts["runtime_wasm"] == str(staged)


def test_prepare_non_native_build_result_skips_runtime_wasm_sidecar_for_linked_wasm(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output_wasm = tmp_path / "out" / "output.wasm"
    output_wasm.parent.mkdir(parents=True, exist_ok=True)
    output_wasm.write_bytes(b"\0asm\x01\0\0\0")
    linked_wasm = tmp_path / "out" / "output_linked.wasm"
    runtime_wasm = tmp_path / "runtime" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0runtime")
    runtime_reloc_wasm = tmp_path / "runtime" / "molt_runtime_reloc.wasm"
    runtime_reloc_wasm.write_bytes(b"\0asm\x01\0\0\0reloc")

    def fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        linked_wasm.write_bytes(b"\0asm\x01\0\0\0linked")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    prepared, err = cli._prepare_non_native_build_result(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        is_wasm_freestanding=False,
        linked=True,
        require_linked=False,
        linked_output_path=linked_wasm,
        output_artifact=output_wasm,
        json_output=True,
        runtime_wasm=runtime_wasm,
        runtime_reloc_wasm=runtime_reloc_wasm,
        ensure_runtime_wasm_shared=lambda *_args, **_kwargs: True,
        ensure_runtime_wasm_reloc=lambda: True,
        molt_root=tmp_path,
        split_runtime=False,
        precompile=False,
    )

    assert err is None
    assert prepared is not None
    staged = output_wasm.parent / "molt_runtime.wasm"
    assert not staged.exists()
    assert prepared.artifacts is not None
    assert "runtime_wasm" not in prepared.artifacts


def test_prepare_non_native_build_result_rebuilds_shared_runtime_with_linked_import_surface(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output_wasm = tmp_path / "out" / "output.wasm"
    output_wasm.parent.mkdir(parents=True, exist_ok=True)
    output_wasm.write_bytes(b"\0asm\x01\0\0\0")
    linked_wasm = tmp_path / "out" / "output_linked.wasm"
    runtime_wasm = tmp_path / "runtime" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0runtime")
    runtime_reloc_wasm = tmp_path / "runtime" / "molt_runtime_reloc.wasm"
    runtime_reloc_wasm.write_bytes(b"\0asm\x01\0\0\0reloc")
    shared_required: list[frozenset[str]] = []

    def fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        linked_wasm.write_bytes(b"\0asm\x01\0\0\0linked")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(cli.subprocess, "run", fake_run)
    monkeypatch.setattr(
        cli,
        "_collect_wasm_module_import_names",
        lambda path, module_name: {"alloc", "molt_fast_list_append"},
    )

    prepared, err = cli._prepare_non_native_build_result(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        is_wasm_freestanding=False,
        linked=True,
        require_linked=False,
        linked_output_path=linked_wasm,
        output_artifact=output_wasm,
        json_output=True,
        runtime_wasm=runtime_wasm,
        runtime_reloc_wasm=runtime_reloc_wasm,
        ensure_runtime_wasm_shared=lambda required=None: shared_required.append(
            frozenset(required or set())
        )
        or True,
        ensure_runtime_wasm_reloc=lambda: True,
        molt_root=tmp_path,
        split_runtime=False,
        precompile=False,
    )

    assert err is None
    assert prepared is not None
    assert shared_required == [frozenset({"alloc", "molt_fast_list_append"})]


def test_runtime_wasm_exports_satisfy_required_surface(tmp_path: Path) -> None:
    wasm = tmp_path / "runtime.wasm"
    payload = bytearray()
    payload.extend(b"\x02")  # export count
    for name, index in (
        ("molt_fast_list_append", 0),
        ("molt_resource_on_free", 1),
    ):
        encoded = name.encode("utf-8")
        payload.append(len(encoded))
        payload.extend(encoded)
        payload.append(0x00)  # func export
        payload.append(index)
    wasm.write_bytes(
        b"\0asm\x01\0\0\0"
        + b"\x07"
        + bytes([len(payload)])
        + payload
    )
    assert cli._runtime_wasm_exports_satisfy(
        wasm, {"molt_fast_list_append", "molt_resource_on_free"}
    )
    assert not cli._runtime_wasm_exports_satisfy(
        wasm, {"molt_fast_list_append", "molt_dict_getitem"}
    )


def test_runtime_wasm_exports_satisfy_browser_runtime_fallback_surface(
    tmp_path: Path,
) -> None:
    def _encode_varuint(value: int) -> bytes:
        out = bytearray()
        while True:
            byte = value & 0x7F
            value >>= 7
            if value:
                out.append(byte | 0x80)
            else:
                out.append(byte)
                return bytes(out)

    wasm = tmp_path / "runtime_fallbacks.wasm"
    payload = bytearray()
    exports = (
        "molt_call_bind_ic",
        "molt_callargs_new",
        "molt_callargs_push_pos",
        "molt_dict_getitem_borrowed",
        "molt_dict_set",
        "molt_tuple_getitem_borrowed",
    )
    payload.append(len(exports))
    for index, name in enumerate(exports):
        encoded = name.encode("utf-8")
        payload.append(len(encoded))
        payload.extend(encoded)
        payload.append(0x00)  # func export
        payload.append(index)
    wasm.write_bytes(
        b"\0asm\x01\0\0\0"
        + b"\x07"
        + _encode_varuint(len(payload))
        + payload
    )

    required = {
        "molt_fast_dict_get",
        "molt_fast_list_append",
        "molt_fast_str_join",
        "molt_dict_getitem",
        "molt_dict_setitem",
        "molt_tuple_getitem",
        "molt_resource_on_allocate",
        "molt_resource_on_free",
    }
    assert cli._runtime_wasm_exports_satisfy(wasm, required)
    assert cli._runtime_wasm_missing_exports(wasm, required) == set()


def test_run_subprocess_captured_to_tempfiles_emits_keepalive(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv("MOLT_SUBPROCESS_KEEPALIVE_SECS", "0.01")
    result = cli._run_subprocess_captured_to_tempfiles(
        [
            sys.executable,
            "-c",
            "import time; print('ok'); time.sleep(0.05)",
        ],
        timeout=1.0,
        progress_label="Tempfile helper",
    )
    assert result.returncode == 0
    assert result.stdout.decode("utf-8").strip() == "ok"
    assert "Tempfile helper still running..." in capsys.readouterr().err


def test_ensure_runtime_lib_native_path_does_not_require_wasm_export_fingerprint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    monkeypatch.setattr(cli, "_runtime_fingerprint", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
    )
    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda *args, **kwargs: None)
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: False)
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
    )

    assert cli._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        cargo_timeout=1.0,
    )


def test_ensure_runtime_wasm_does_not_overwrite_satisfied_runtime_with_unsatisfied_build_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    current_src = tmp_path / "target" / "wasm32-wasip1" / "release-fast" / "molt_runtime.wasm"
    current_src.parent.mkdir(parents=True, exist_ok=True)
    runtime.write_bytes(b"\0asm\x01\0\0\0")
    current_src.write_bytes(b"\0asm\x01\0\0\0")

    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: {"hash": "ok"})
    monkeypatch.setattr(cli, "_runtime_fingerprint", lambda *args, **kwargs: {"hash": "ok"})
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: False)
    monkeypatch.setattr(cli, "_is_valid_runtime_wasm_artifact", lambda path: True)
    monkeypatch.setattr(cli, "_inspect_wasm_binary", lambda path: "valid")
    monkeypatch.setattr(cli, "_resolve_built_runtime_wasm_artifact", lambda *args: current_src)
    monkeypatch.setattr(
        cli,
        "_runtime_wasm_exports_satisfy",
        lambda path, required: path == runtime,
    )

    copied: list[tuple[Path, Path]] = []

    def fake_copy2(src: Path | str, dst: Path | str, *args, **kwargs):
        copied.append((Path(src), Path(dst)))
        return dst

    monkeypatch.setattr(cli.shutil, "copy2", fake_copy2)

    ok = cli._ensure_runtime_wasm(
        runtime,
        reloc=False,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=None,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules=None,
        required_exports={"molt_fast_list_append"},
    )

    assert ok is True
    assert copied == []


def test_ensure_runtime_lib_verified_key_tracks_micro_feature_shape(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    cli._RUNTIME_LIB_VERIFIED.clear()
    verification_calls: list[frozenset[str]] = []

    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda project_root, **kwargs: {
            "runtime_features": tuple(cast(tuple[str, ...], kwargs["runtime_features"]))
        },
    )
    monkeypatch.setattr(
        cli,
        "_artifact_needs_rebuild",
        lambda artifact, fingerprint, stored_fingerprint: verification_calls.append(
            frozenset(cast(tuple[str, ...], fingerprint["runtime_features"]))
        )
        or False,
    )

    try:
        assert cli._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="release-fast",
            project_root=tmp_path,
            cargo_timeout=1.0,
            stdlib_profile="micro",
            resolved_modules={"json"},
        )
        assert cli._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="release-fast",
            project_root=tmp_path,
            cargo_timeout=1.0,
            stdlib_profile="micro",
            resolved_modules={"socket"},
        )
    finally:
        cli._RUNTIME_LIB_VERIFIED.clear()

    assert len(verification_calls) == 2
    assert verification_calls[0] != verification_calls[1]


def test_run_backend_pipeline_defers_native_runtime_readiness_until_after_codegen(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_artifact = tmp_path / "output.o"
    output_binary = tmp_path / "app"
    call_order: list[str] = []

    build_preamble = cli._PreparedBuildPreamble(
        diagnostics_path_spec="",
        diagnostics_enabled=False,
        resolved_diagnostics_verbosity="brief",
        allocation_diagnostics_enabled=False,
        frontend_timing_raw="",
        frontend_timing_enabled=False,
        frontend_timing_threshold=0.0,
        frontend_module_timings=[],
        midend_policy_outcomes_by_function={},
        midend_pass_stats_by_function={},
        frontend_parallel_details={},
        diagnostics_start=0.0,
        phase_starts={},
        backend_daemon_health=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        module_reasons={},
        stdlib_root=tmp_path,
        warnings=[],
        native_arch_perf_enabled=False,
    )
    build_roots = cli._PreparedBuildRoots(
        cwd_root=tmp_path,
        project_root=tmp_path,
        molt_root=tmp_path,
        sysroot_path=None,
    )
    build_config = cli._PreparedBuildConfig(
        pgo_profile_summary=None,
        pgo_profile_path=None,
        runtime_feedback_summary=None,
        runtime_feedback_path=None,
        pgo_hot_function_names=set(),
        pgo_hot_function_names_sorted=(),
        pgo_profile_payload=None,
        runtime_feedback_payload=None,
        cargo_timeout=1.0,
        backend_timeout=1.0,
        link_timeout=1.0,
        frontend_phase_timeout=1.0,
        backend_profile="dev",
        runtime_cargo_profile="release",
        backend_cargo_profile="dev",
        capabilities_list=None,
        capability_profiles=[],
        capabilities_source=None,
        manifest_env_vars={},
    )
    resolved_entry = cli._ResolvedBuildEntry(
        source_path=tmp_path / "main.py",
        entry_module="__main__",
        module_roots=[tmp_path],
        entry_source="print('hi')\n",
        entry_tree=ast.parse("print('hi')\n"),
    )
    output_layout = cli._BuildOutputLayout(
        is_wasm=False,
        is_wasm_freestanding=False,
        is_rust_transpile=False,
        is_luau_transpile=False,
        split_runtime=False,
        linked=False,
        target_triple=None,
        emit_mode="bin",
        output_artifact=output_artifact,
        output_binary=output_binary,
        linked_output_path=None,
        emit_ir_path=None,
    )
    frontend_bundle = (
        object(),
        {},
        set(),
        False,
        output_layout,
        set(),
        {},
        {},
        [],
        None,
        {},
        False,
        0,
        False,
        object(),
        object(),
        lambda *args, **kwargs: None,
        lambda: (None, None),
        tmp_path,
    )

    monkeypatch.setattr(
        cli,
        "_prepare_backend_ir",
        lambda **kwargs: (
            call_order.append("backend_ir") or cli._PreparedBackendIR(ir={}),
            None,
        ),
    )

    def fake_prepare_backend_setup(**kwargs: object) -> tuple[cli._PreparedBackendSetup, None]:
        del kwargs
        call_order.append("backend_setup")
        runtime_state = cli._RuntimeArtifactState(runtime_lib=runtime_lib)
        cache_setup = cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="module-cache",
            function_cache_key=None,
            cache_path=tmp_path / "module-cache.o",
            function_cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            cache_candidates=(("module", tmp_path / "module-cache.o"),),
            cache_hit=False,
            cache_hit_tier=None,
        )
        return (
            cli._PreparedBackendSetup(
                runtime_state=runtime_state,
                cache_setup=cache_setup,
                cache_hit=False,
                cache_hit_tier=None,
                cache_key=cache_setup.cache_key,
                function_cache_key=cache_setup.function_cache_key,
                cache_path=cache_setup.cache_path,
                function_cache_path=cache_setup.function_cache_path,
                stdlib_object_path=cache_setup.stdlib_object_path,
                cache_candidates=list(cache_setup.cache_candidates),
            ),
            None,
        )

    monkeypatch.setattr(cli, "_prepare_backend_setup", fake_prepare_backend_setup)
    monkeypatch.setattr(
        cli,
        "_prepare_backend_runtime_context",
        lambda **kwargs: cli._PreparedBackendRuntimeContext(
            runtime_state=kwargs["prepared_backend_setup"].runtime_state,
            runtime_lib=runtime_lib,
            runtime_wasm=None,
            runtime_reloc_wasm=None,
            ensure_runtime_wasm_shared=lambda: True,
            ensure_runtime_wasm_reloc=lambda: True,
            cache_setup=kwargs["prepared_backend_setup"].cache_setup,
            cache_hit=False,
            cache_hit_tier=None,
            cache_key="module-cache",
            function_cache_key=None,
            cache_path=tmp_path / "module-cache.o",
            function_cache_path=None,
            stdlib_object_path=None,
        ),
    )
    monkeypatch.setattr(
        cli,
        "_prepare_backend_compile",
        lambda **kwargs: (
            call_order.append("backend_compile")
            or cli._PreparedBackendCompile(
                cache_enabled=True,
                cache_hit=False,
                cache_hit_tier=None,
                wasm_table_base=None,
                backend_daemon_cached=None,
                backend_daemon_cache_tier=None,
                backend_daemon_health=None,
                backend_daemon_config_digest=None,
            ),
            None,
        ),
    )
    monkeypatch.setattr(
        cli,
        "_ensure_runtime_lib_ready",
        lambda runtime_state, **kwargs: call_order.append("runtime_ready")
        or False,
    )

    def fake_prepare_native_link(**kwargs: object) -> tuple[None, dict[str, object] | None]:
        del kwargs
        call_order.append("native_link")
        pytest.fail("native link should not run after runtime readiness failure")

    monkeypatch.setattr(cli, "_prepare_native_link", fake_prepare_native_link)

    result = cli._run_backend_pipeline(
        prepared_build_preamble=build_preamble,
        prepared_build_roots=build_roots,
        prepared_build_config=build_config,
        resolved_build_entry=resolved_entry,
        prepared_frontend_pipeline_bundle=frontend_bundle,
        parse_codec="json",
        type_hint_policy="check",
        fallback_policy="error",
        profile="dev",
        json_output=True,
        target="native",
        cache_dir=None,
        cache=True,
        cache_report=False,
        deterministic=False,
        trusted=False,
        verbose=False,
        require_linked=False,
        wasm_opt_level="Oz",
        precompile=False,
        snapshot=False,
        stdlib_profile="micro",
    )

    assert result == 2
    assert call_order == [
        "backend_ir",
        "backend_setup",
        "backend_compile",
        "runtime_ready",
    ]


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

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        backend_bin.parent.mkdir(parents=True, exist_ok=True)
        backend_bin.write_text("#!/bin/sh\n")
        backend_bin.chmod(0o755)
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

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        cargo_output = backend_bin.parent / "molt-backend"
        cargo_output.parent.mkdir(parents=True, exist_ok=True)
        cargo_output.write_text("#!/bin/sh\n")
        cargo_output.chmod(0o755)
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


def test_ensure_backend_binary_fails_when_feature_rebuild_emits_no_binary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "target" / "dev-fast" / "molt-backend.wasm_backend"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args, kwargs
        return dict(fingerprint)

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del cmd, kwargs
        return subprocess.CompletedProcess(["cargo", "build"], 0, "", "")

    monkeypatch.setattr(cli, "_backend_fingerprint", fake_backend_fingerprint)
    monkeypatch.setattr(cli, "_run_cargo_with_sccache_retry", fake_run_cargo)

    assert not cli._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("wasm-backend",),
    )


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
            output = Path(cmd[cmd.index("--output") + 1])
            if output.name.startswith("molt_backend_probe_"):
                assert "--target" in cmd and cmd[cmd.index("--target") + 1] == "rust"
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_text("fn main() {}\n")
                return subprocess.CompletedProcess(cmd, 0, b"", b"")
            backend_cmds.append(list(cmd))
            assert cmd[1:3] == ["--target", "rust"]
            assert "--output" in cmd
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
            "dev-fast",
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


def test_browser_deploy_profile_defaults_to_full_wasm_profile() -> None:
    assert cli._DEPLOY_PROFILE_DEFAULTS["browser"]["wasm_profile"] == "full"


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
    output_binary = tmp_path / "bin" / "main_molt"
    output_binary.parent.mkdir(parents=True)
    output_binary.write_text("")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(output_binary),
            "consumer_output": str(output_binary),
        },
    )

    build_cmds: list[list[str]] = []
    run_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

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
            "--json",
            "--build-profile",
            "dev",
            str(entry),
        ]
    ]
    assert run_cmds


def test_run_script_uses_build_resolved_entry_for_package_override_file(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    pkg_dir = project / "pkg"
    pkg_dir.mkdir(parents=True)
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    entry = pkg_dir / "__main__.py"
    entry.write_text('__package__ = "pkg"\nprint("ok")\n')
    output_binary = tmp_path / "bin" / "pkg_molt"
    output_binary.parent.mkdir(parents=True)
    output_binary.write_text("")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(output_binary),
            "consumer_output": str(output_binary),
        },
    )

    run_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

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
        json_output=False,
    )

    assert rc == 0
    assert run_cmds
    assert Path(run_cmds[0][0]).name == "pkg_molt"


def test_run_script_uses_build_json_output_for_binary_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("print('ok')\n")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    consumer_output = tmp_path / "dist" / "custom_binary"
    consumer_output.parent.mkdir(parents=True)
    consumer_output.write_text("")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(consumer_output),
            "consumer_output": str(consumer_output),
        },
    )
    build_cmds: list[list[str]] = []
    run_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

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
        json_output=False,
    )

    assert rc == 0
    assert build_cmds == [
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--json",
            str(entry),
        ]
    ]
    assert run_cmds == [[str(consumer_output)]]


def test_run_script_replays_build_messages_and_warnings_in_non_json_mode(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("print('ok')\n")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    consumer_output = tmp_path / "dist" / "custom_binary"
    consumer_output.parent.mkdir(parents=True)
    consumer_output.write_text("")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(consumer_output),
            "consumer_output": str(consumer_output),
            "messages": [f"Successfully built {consumer_output}"],
            "compile_diagnostics": {"total_sec": 0.125, "module_count": 1},
        },
        warnings=["cache reused"],
    )

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)
    monkeypatch.setattr(cli, "_run_command", lambda cmd, **kwargs: 0)

    rc = cli.run_script(
        str(entry),
        None,
        [],
        json_output=False,
    )

    assert rc == 0
    captured = capsys.readouterr()
    assert f"Successfully built {consumer_output}" in captured.err
    assert "Warning: cache reused" in captured.err
    assert "Build diagnostics:" in captured.err


def test_run_script_surfaces_nested_build_error_detail_in_non_json_mode(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("print('ok')\n")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    payload = cli._json_payload(
        "build",
        "error",
        data={
            "stderr": "ld: unresolved symbol",
            "stdout": "backend retry log",
        },
        errors=["Linking failed"],
    )

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(cmd, 1, json.dumps(payload), "")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)

    rc = cli.run_script(
        str(entry),
        None,
        [],
        json_output=False,
    )

    assert rc == 1
    captured = capsys.readouterr()
    assert "Linking failed" in captured.err
    assert "ld: unresolved symbol" in captured.err
    assert "backend retry log" in captured.err


def test_run_script_cross_respects_pythonpath_for_module_artifact_resolution(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    pythonpath_root = tmp_path / "pythonpath"
    pkg_dir = pythonpath_root / "demo"
    pkg_dir.mkdir(parents=True)
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    (pkg_dir / "__main__.py").write_text("print('ok')\n")
    artifact = out_dir / "demo.luau"
    artifact.write_text("-- compiled\n")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(artifact),
            "consumer_output": str(artifact),
            "artifacts": {"luau": str(artifact)},
        },
    )

    seen_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)
    monkeypatch.setattr(cli.shutil, "which", lambda name: f"/usr/bin/{name}")
    monkeypatch.setenv("PYTHONPATH", str(pythonpath_root))

    rc = cli._run_script_cross(
        "luau",
        None,
        "demo",
        [],
        build_args=["--respect-pythonpath", "--out-dir", str(out_dir)],
        json_output=False,
    )

    assert rc == 0
    assert seen_cmds[0] == [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        "--respect-pythonpath",
        "--out-dir",
        str(out_dir),
        "--module",
        "demo",
    ]
    assert seen_cmds[1] == ["/usr/bin/lune", "run", str(artifact), "--"]


def test_run_script_cross_wasm_honors_build_json_output_and_linked_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("print('ok')\n")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    output_wasm = out_dir / "output.wasm"
    linked_wasm = out_dir / "output_linked.wasm"
    linked_wasm.write_text("")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(output_wasm),
            "consumer_output": str(linked_wasm),
            "linked_output": str(linked_wasm),
            "artifacts": {
                "wasm": str(output_wasm),
                "linked_wasm": str(linked_wasm),
            },
        },
    )
    seen_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)
    monkeypatch.setattr(cli.shutil, "which", lambda name: f"/usr/bin/{name}")

    rc = cli._run_script_cross(
        "wasm",
        str(entry),
        None,
        [],
        build_args=["--out-dir", str(out_dir)],
        json_output=False,
    )

    assert rc == 0
    assert seen_cmds[0] == [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        "--out-dir",
        str(out_dir),
        str(entry),
    ]
    assert seen_cmds[1] == ["/usr/bin/wasmtime", "run", str(linked_wasm), "--"]


def test_deploy_roblox_respects_pythonpath_for_module_artifact_resolution(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    pythonpath_root = tmp_path / "pythonpath"
    pkg_dir = pythonpath_root / "demo"
    pkg_dir.mkdir(parents=True)
    roblox_dir = tmp_path / "roblox"
    roblox_dir.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    (pkg_dir / "__main__.py").write_text("print('ok')\n")
    artifact = out_dir / "demo.luau"
    artifact.write_text("-- compiled\n")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(artifact),
            "consumer_output": str(artifact),
            "artifacts": {"luau": str(artifact)},
        },
    )

    seen_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        seen_cmds.append(list(cmd))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(
                cmd, 0, json.dumps(payload).encode(), b""
            )
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)
    monkeypatch.setenv("PYTHONPATH", str(pythonpath_root))

    rc = cli._deploy(
        "roblox",
        None,
        "demo",
        None,
        None,
        str(out_dir),
        str(roblox_dir),
        "",
        False,
        ["--respect-pythonpath"],
        False,
        False,
    )

    assert rc == 0
    assert seen_cmds == [
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--json",
            "--respect-pythonpath",
            "--target",
            "luau",
            "--out-dir",
            str(out_dir),
            "--module",
            "demo",
        ]
    ]
    assert (roblox_dir / "demo.luau").read_text() == "-- compiled\n"


def test_deploy_roblox_honors_build_json_output_override(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    roblox_dir = tmp_path / "roblox"
    roblox_dir.mkdir()
    entry = project / "demo.py"
    entry.write_text("print('ok')\n")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    consumer_output = tmp_path / "build" / "nested" / "custom.luau"
    consumer_output.parent.mkdir(parents=True)
    consumer_output.write_text("-- custom compiled\n")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(consumer_output),
            "consumer_output": str(consumer_output),
            "artifacts": {"luau": str(consumer_output)},
        },
    )
    seen_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        seen_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload).encode(), b"")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)

    rc = cli._deploy(
        "roblox",
        str(entry),
        None,
        None,
        str(consumer_output),
        None,
        str(roblox_dir),
        "",
        False,
        [],
        False,
        False,
    )

    assert rc == 0
    assert seen_cmds == [
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--json",
            "--target",
            "luau",
            "--output",
            str(consumer_output),
            str(entry),
        ]
    ]
    assert (roblox_dir / consumer_output.name).read_text() == "-- custom compiled\n"


def test_deploy_cloudflare_uses_build_json_bundle_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("print('ok')\n")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )
    bundle_root = tmp_path / "dist" / "worker"
    bundle_root.mkdir(parents=True)
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text('{"name":"demo"}\n')
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(bundle_root / "app.wasm"),
            "consumer_output": str(bundle_root / "app.wasm"),
            "bundle_root": str(bundle_root),
            "artifacts": {"wrangler_config": str(wrangler_config)},
        },
    )
    seen_calls: list[tuple[list[str], Path | None]] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        seen_calls.append((list(cmd), kwargs.get("cwd")))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)
    monkeypatch.setattr(cli.shutil, "which", lambda name: f"/usr/bin/{name}")

    rc = cli._deploy(
        "cloudflare",
        str(entry),
        None,
        None,
        None,
        None,
        None,
        "",
        False,
        [],
        False,
        False,
    )

    assert rc == 0
    assert seen_calls[0][0] == [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        "--target",
        "wasm",
        "--profile",
        "cloudflare",
        "--split-runtime",
        str(entry),
    ]
    assert seen_calls[1] == (
        ["/usr/bin/wrangler", "deploy", "--config", str(wrangler_config)],
        bundle_root,
    )


def test_run_script_reports_run_command_on_resolution_failure_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)

    rc = cli.run_script(
        None,
        "missing_module",
        [],
        json_output=True,
    )

    assert rc == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["command"] == "run"
    assert payload["errors"] == ["Entry module not found: missing_module"]


def test_run_script_cross_reports_run_command_on_resolution_failure_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)

    rc = cli._run_script_cross(
        "luau",
        None,
        "missing_module",
        [],
        json_output=True,
    )

    assert rc == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["command"] == "run"
    assert payload["errors"] == ["Entry module not found: missing_module"]


def test_deploy_reports_deploy_command_on_resolution_failure_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n'
    )

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)

    rc = cli._deploy(
        "roblox",
        None,
        "missing_module",
        None,
        None,
        None,
        None,
        "",
        False,
        [],
        True,
        False,
    )

    assert rc == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["command"] == "deploy"
    assert payload["errors"] == ["Entry module not found: missing_module"]


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
    entry_module = "pkg.app"
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
            stdlib_object_cache_key=None,
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
        entry_module=entry_module,
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
    assert captured_envs[0]["MOLT_ENTRY_MODULE"] == entry_module


def test_native_backend_compile_overrides_stale_ambient_partition_env(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    output_artifact = project_root / "build" / "main.o"
    backend_bin = tmp_path / "backend-bin"
    artifacts_root = tmp_path / "artifacts"
    stdlib_object_path = project_root / "build" / "main.stdlib.o"
    entry_module = "pkg.app"
    captured_envs: list[dict[str, str] | None] = []

    monkeypatch.setenv("MOLT_STDLIB_OBJ", str(tmp_path / "ambient.stdlib.o"))
    monkeypatch.setenv("MOLT_STDLIB_CACHE_KEY", "ambient-key")
    monkeypatch.setenv("MOLT_STDLIB_MODULE_SYMBOLS", '["ambient_mod"]')
    monkeypatch.setenv("MOLT_ENTRY_MODULE", "ambient.entry")

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = cast(dict[str, str] | None, kwargs.get("env"))
        captured_envs.append(env)
        assert env is not None
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
            stdlib_object_cache_key="real-key",
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
            stdlib_module_symbols_json='["builtins","sys"]',
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
        entry_module=entry_module,
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
    env = captured_envs[0]
    assert env["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)
    assert env["MOLT_STDLIB_CACHE_KEY"] == "real-key"
    assert env["MOLT_STDLIB_MODULE_SYMBOLS"] == '["builtins","sys"]'
    assert env["MOLT_ENTRY_MODULE"] == entry_module


def test_backend_compile_stages_one_shot_output_into_cache(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    artifacts_root = tmp_path / "artifacts"
    output_artifact = project_root / "dist" / "output.wasm"
    cache_path = project_root / ".molt_cache" / "cache-key.wasm"
    function_cache_path = project_root / ".molt_cache" / "fn-cache-key.wasm"
    backend_bin = tmp_path / "backend-bin"
    seen_output_paths: list[Path] = []

    def fake_subprocess_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        output_path = Path(cmd[cmd.index("--output") + 1])
        seen_output_paths.append(output_path)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"wasm-bytes")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)

    result, error = cli._execute_backend_compile(
        cache=True,
        cache_path=cache_path,
        function_cache_path=function_cache_path,
        artifacts_root=artifacts_root,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        diagnostics_enabled=False,
        phase_starts={},
        daemon_ready=False,
        daemon_socket=None,
        project_root=project_root,
        output_artifact=output_artifact,
        cache_key="cache-key",
        function_cache_key="fn-cache-key",
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="cache-key",
            function_cache_key="fn-cache-key",
            cache_path=cache_path,
            function_cache_path=function_cache_path,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            cache_candidates=(("module", cache_path), ("function", function_cache_path)),
            cache_hit=False,
            cache_hit_tier=None,
        ),
        target_triple=None,
        backend_daemon_config_digest=None,
        entry_module="pkg.app",
        ir={"functions": []},
        json_output=False,
        warnings=[],
        verbose=False,
        backend_bin=backend_bin,
        backend_env={},
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
    assert seen_output_paths
    assert seen_output_paths[0] != cache_path
    assert seen_output_paths[0].parent == artifacts_root
    assert output_artifact.read_bytes() == b"wasm-bytes"
    assert cache_path.read_bytes() == b"wasm-bytes"
    assert function_cache_path.read_bytes() == b"wasm-bytes"


def test_execute_backend_compile_defers_full_daemon_request_encode_until_probe_miss(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    artifacts_root = tmp_path / "artifacts"
    output_artifact = project_root / "build" / "main.o"
    cache_path = project_root / ".molt_cache" / "cache-key.o"
    function_cache_path = project_root / ".molt_cache" / "fn-cache-key.o"
    stdlib_object_path = project_root / "build" / "main.stdlib.o"
    request_encode_calls: list[tuple[bool, bool]] = []
    daemon_request_bytes: list[bytes | None] = []

    def fake_request_bytes(**kwargs: object) -> tuple[bytes | None, str | None]:
        request_encode_calls.append(
            (
                bool(kwargs.get("probe_cache_only")),
                kwargs.get("ir") is not None,
            )
        )
        return b'{"version":1,"jobs":[{"id":"job0"}]}\n', None

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        **kwargs: object,
    ) -> cli._BackendDaemonCompileResult:
        assert socket_path == tmp_path / "daemon.sock"
        daemon_request_bytes.append(cast(bytes | None, kwargs.get("request_bytes")))
        return cli._BackendDaemonCompileResult(
            True,
            None,
            {"pid": 42},
            True,
            "module",
            False,
            True,
        )

    monkeypatch.setattr(cli, "_backend_daemon_compile_request_bytes", fake_request_bytes)
    monkeypatch.setattr(
        cli, "_compile_with_backend_daemon", fake_compile_with_backend_daemon
    )

    result, error = cli._execute_backend_compile(
        cache=True,
        cache_path=cache_path,
        function_cache_path=function_cache_path,
        artifacts_root=artifacts_root,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        diagnostics_enabled=False,
        phase_starts={},
        daemon_ready=True,
        daemon_socket=tmp_path / "daemon.sock",
        project_root=project_root,
        output_artifact=output_artifact,
        cache_key="cache-key",
        function_cache_key="fn-cache-key",
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="cache-key",
            function_cache_key="fn-cache-key",
            cache_path=cache_path,
            function_cache_path=function_cache_path,
            stdlib_object_path=stdlib_object_path,
            stdlib_object_cache_key="stdlib-cache-key",
            cache_candidates=(("module", cache_path), ("function", function_cache_path)),
            cache_hit=False,
            cache_hit_tier=None,
        ),
        target_triple=None,
        backend_daemon_config_digest="digest123",
        entry_module="pkg.app",
        ir={"functions": [{"name": "heavy"}]},
        json_output=False,
        warnings=[],
        verbose=False,
        backend_bin=tmp_path / "backend-bin",
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
    assert request_encode_calls == []
    assert daemon_request_bytes == [None]
    assert result.backend_daemon_cached is True
    assert result.backend_daemon_cache_tier == "module"


def test_execute_backend_compile_keeps_probe_path_across_daemon_restart(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    artifacts_root = tmp_path / "artifacts"
    output_artifact = project_root / "build" / "main.o"
    cache_path = project_root / ".molt_cache" / "cache-key.o"
    function_cache_path = project_root / ".molt_cache" / "fn-cache-key.o"
    request_encode_calls: list[tuple[bool, bool]] = []
    daemon_request_bytes: list[bytes | None] = []
    compile_attempts = 0
    restart_calls = 0

    def fake_request_bytes(**kwargs: object) -> tuple[bytes | None, str | None]:
        request_encode_calls.append(
            (
                bool(kwargs.get("probe_cache_only")),
                kwargs.get("ir") is not None,
            )
        )
        return b'{"version":1,"jobs":[{"id":"job0"}]}\n', None

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        **kwargs: object,
    ) -> cli._BackendDaemonCompileResult:
        nonlocal compile_attempts
        assert socket_path == tmp_path / "daemon.sock"
        compile_attempts += 1
        daemon_request_bytes.append(cast(bytes | None, kwargs.get("request_bytes")))
        if compile_attempts == 1:
            return cli._BackendDaemonCompileResult(
                False,
                "backend daemon connection failed: boom",
                None,
                None,
                None,
                True,
                False,
            )
        return cli._BackendDaemonCompileResult(
            True,
            None,
            {"pid": 42},
            True,
            "module",
            False,
            True,
        )

    def fake_start_backend_daemon(*args: object, **kwargs: object) -> bool:
        nonlocal restart_calls
        restart_calls += 1
        assert args[1] == tmp_path / "daemon.sock"
        return True

    monkeypatch.setattr(cli, "_backend_daemon_compile_request_bytes", fake_request_bytes)
    monkeypatch.setattr(
        cli, "_compile_with_backend_daemon", fake_compile_with_backend_daemon
    )
    monkeypatch.setattr(cli, "_start_backend_daemon", fake_start_backend_daemon)

    result, error = cli._execute_backend_compile(
        cache=True,
        cache_path=cache_path,
        function_cache_path=function_cache_path,
        artifacts_root=artifacts_root,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        diagnostics_enabled=False,
        phase_starts={},
        daemon_ready=True,
        daemon_socket=tmp_path / "daemon.sock",
        project_root=project_root,
        output_artifact=output_artifact,
        cache_key="cache-key",
        function_cache_key="fn-cache-key",
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="cache-key",
            function_cache_key="fn-cache-key",
            cache_path=cache_path,
            function_cache_path=function_cache_path,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            cache_candidates=(("module", cache_path), ("function", function_cache_path)),
            cache_hit=False,
            cache_hit_tier=None,
        ),
        target_triple=None,
        backend_daemon_config_digest="digest123",
        entry_module="pkg.app",
        ir={"functions": [{"name": "heavy"}]},
        json_output=False,
        warnings=[],
        verbose=False,
        backend_bin=tmp_path / "backend-bin",
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
    assert compile_attempts == 2
    assert restart_calls == 1
    assert request_encode_calls == []
    assert daemon_request_bytes == [None, None]
    assert result.backend_daemon_cached is True
    assert result.backend_daemon_cache_tier == "module"


def test_execute_backend_compile_rejects_unsynced_daemon_output_skip(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    artifacts_root = tmp_path / "artifacts"
    output_artifact = project_root / "build" / "main.o"
    output_artifact.parent.mkdir(parents=True, exist_ok=True)
    output_artifact.write_bytes(b"stale")

    monkeypatch.setattr(
        cli,
        "_compile_with_backend_daemon",
        lambda socket_path, **kwargs: cli._BackendDaemonCompileResult(
            True,
            None,
            {"pid": 42},
            False,
            "module",
            False,
            False,
        ),
    )

    result, error = cli._execute_backend_compile(
        cache=True,
        cache_path=project_root / ".molt_cache" / "cache-key.o",
        function_cache_path=None,
        artifacts_root=artifacts_root,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        diagnostics_enabled=False,
        phase_starts={},
        daemon_ready=True,
        daemon_socket=tmp_path / "daemon.sock",
        project_root=project_root,
        output_artifact=output_artifact,
        cache_key="cache-key",
        function_cache_key=None,
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=True,
            cache_key="cache-key",
            function_cache_key=None,
            cache_path=project_root / ".molt_cache" / "cache-key.o",
            function_cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            cache_candidates=(("module", project_root / ".molt_cache" / "cache-key.o"),),
            cache_hit=False,
            cache_hit_tier=None,
        ),
        target_triple=None,
        backend_daemon_config_digest="digest123",
        entry_module="pkg.app",
        ir={"functions": [{"name": "heavy"}]},
        json_output=True,
        warnings=[],
        verbose=False,
        backend_bin=tmp_path / "backend-bin",
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

    assert result is None
    assert error == 2
    captured = capsys.readouterr()
    assert "skipped output write without a synced-artifact contract" in captured.out


def test_backend_daemon_compile_request_includes_partition_env(
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    stdlib_object_path = tmp_path / "cache" / "main.stdlib.o"
    request_bytes, error = cli._backend_daemon_compile_request_bytes(
        ir={"functions": []},
        backend_output=backend_output,
        is_wasm=False,
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key="function-cache",
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        entry_module="pkg.app",
        stdlib_object_path=stdlib_object_path,
    )

    assert error is None
    assert request_bytes is not None
    payload = json.loads(request_bytes)
    env = payload["env"]
    assert env["MOLT_ENTRY_MODULE"] == "pkg.app"
    assert env["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)


def test_backend_daemon_compile_request_includes_batch_op_budget_env(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_output = tmp_path / "output.o"
    monkeypatch.setenv("MOLT_BACKEND_BATCH_OP_BUDGET", "16384")

    request_bytes, error = cli._backend_daemon_compile_request_bytes(
        ir={"functions": []},
        backend_output=backend_output,
        is_wasm=False,
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key="function-cache",
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        entry_module="pkg.app",
        stdlib_object_path=tmp_path / "cache" / "main.stdlib.o",
    )

    assert error is None
    assert request_bytes is not None
    payload = json.loads(request_bytes)
    assert payload["env"]["MOLT_BACKEND_BATCH_OP_BUDGET"] == "16384"


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

    def fake_run_command_timed(cmd: list[str], **kwargs: object) -> cli._TimedResult:
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

    monkeypatch.delenv("MOLT_BACKEND_REGALLOC_ALGORITHM", raising=False)
    baseline_native = cli._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_TIR_OPT", "0")
    native_tir_changed = cli._backend_codegen_env_digest(is_wasm=False)
    assert native_tir_changed != baseline_native

    monkeypatch.delenv("MOLT_TIR_OPT", raising=False)
    baseline_native = cli._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_BACKEND", "llvm")
    native_backend_changed = cli._backend_codegen_env_digest(is_wasm=False)
    assert native_backend_changed != baseline_native

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


def test_backend_daemon_config_digest_tracks_batch_op_budget(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET", raising=False)
    cli._backend_daemon_paths_cached.cache_clear()
    digest_a = cli._backend_daemon_config_digest(tmp_path, "dev-fast")
    monkeypatch.setenv("MOLT_BACKEND_BATCH_OP_BUDGET", "8192")
    digest_b = cli._backend_daemon_config_digest(tmp_path, "dev-fast")

    assert digest_a != digest_b


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
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    cli._backend_daemon_paths_cached.cache_clear()

    log_path = cli._backend_daemon_log_path(tmp_path, "dev-fast")
    pid_path = cli._backend_daemon_pid_path(tmp_path, "dev-fast")

    info = cli._backend_daemon_paths_cached.cache_info()
    assert log_path.name.startswith("molt-backend.dev-fast.alpha-session.")
    assert log_path.suffix == ".log"
    assert pid_path.name.startswith("molt-backend.dev-fast.alpha-session.")
    assert pid_path.suffix == ".pid"
    assert log_path.parent == pid_path.parent
    assert info.hits >= 1


def test_backend_daemon_log_and_pid_paths_are_session_isolated(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET", raising=False)
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET_DIR", raising=False)
    cli._backend_daemon_paths_cached.cache_clear()

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    alpha_log = cli._backend_daemon_log_path(tmp_path, "dev-fast")
    alpha_pid = cli._backend_daemon_pid_path(tmp_path, "dev-fast")

    cli._backend_daemon_paths_cached.cache_clear()
    monkeypatch.setenv("MOLT_SESSION_ID", "beta-session")
    beta_log = cli._backend_daemon_log_path(tmp_path, "dev-fast")
    beta_pid = cli._backend_daemon_pid_path(tmp_path, "dev-fast")

    assert alpha_log != beta_log
    assert alpha_pid != beta_pid
    assert alpha_log.parent == beta_log.parent
    assert alpha_pid.parent == beta_pid.parent


def test_backend_daemon_paths_allow_missing_session_id(tmp_path: Path) -> None:
    cli._backend_daemon_paths_cached.cache_clear()

    socket_path, log_path, pid_path = cli._backend_daemon_paths_cached(
        os.fspath(tmp_path),
        "dev-fast",
        "digest123",
        "",
        None,
        os.fspath(tmp_path / "build-state"),
        os.fspath(tmp_path / "tmp"),
        session_id=None,
    )

    assert socket_path.name.startswith("moltbd.")
    assert socket_path.suffix == ".sock"
    assert log_path.name.startswith("molt-backend.dev-fast.")
    assert log_path.suffix == ".log"
    assert pid_path.name.startswith("molt-backend.dev-fast.")
    assert pid_path.suffix == ".pid"


@pytest.mark.skipif(os.name == "nt", reason="daemon sockets use unix paths")
def test_backend_daemon_paths_fallback_to_short_socket_dir_for_deep_temp_roots(
    tmp_path: Path,
) -> None:
    cli._backend_daemon_paths_cached.cache_clear()
    deep_tempdir = tmp_path / ("deep-" + "a" * 160)

    socket_path, _log_path, _pid_path = cli._backend_daemon_paths_cached(
        os.fspath(tmp_path),
        "dev-fast",
        "digest123",
        "",
        None,
        os.fspath(tmp_path / "build-state"),
        os.fspath(deep_tempdir),
        session_id="alpha-session",
    )

    assert socket_path.name.startswith("moltbd.")
    assert socket_path.suffix == ".sock"
    assert not os.fspath(socket_path).startswith(os.fspath(deep_tempdir))
    assert len(os.fsencode(socket_path)) < 104


@pytest.mark.skipif(os.name == "nt", reason="daemon sockets use unix paths")
def test_start_backend_daemon_rejects_overlong_unix_socket_paths(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("backend")
    socket_path = tmp_path / ("sock-" + "a" * 140)
    warnings: list[str] = []
    popen_called = False

    def fake_popen(*args: object, **kwargs: object) -> object:
        nonlocal popen_called
        popen_called = True
        raise AssertionError("daemon should not spawn for an overlong unix socket path")

    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    ok = cli._start_backend_daemon(
        backend_bin,
        socket_path,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        startup_timeout=2.0,
        json_output=True,
        warnings=warnings,
    )

    assert ok is False
    assert popen_called is False
    assert warnings
    assert "unix socket path" in warnings[0].lower()


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

        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

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
    stdlib_object_path = tmp_path / "cache" / "main.stdlib.o"
    seen_payloads: list[dict[str, object]] = []
    connects = 0

    class _FakeSocket:
        def __init__(self) -> None:
            self._chunks: list[bytes] = []

        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

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
        stdlib_object_path=stdlib_object_path,
        stdlib_object_cache_key="stdlib-cache-key",
        timeout=0.1,
    )

    assert result.ok is True
    assert len(seen_payloads) == 2
    assert connects == 2
    assert seen_payloads[0]["jobs"][0]["probe_cache_only"] is True
    assert "ir" not in seen_payloads[0]["jobs"][0]
    assert seen_payloads[0]["env"]["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)
    assert seen_payloads[0]["env"]["MOLT_STDLIB_CACHE_KEY"] == "stdlib-cache-key"
    assert seen_payloads[1]["jobs"][0]["ir"] == {"functions": [{"name": "heavy"}]}
    assert seen_payloads[1]["env"]["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)
    assert seen_payloads[1]["env"]["MOLT_STDLIB_CACHE_KEY"] == "stdlib-cache-key"


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

        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

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


def test_compile_with_backend_daemon_fails_fast_when_daemon_dies_mid_request(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"

    class _FakeSocket:
        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def settimeout(self, timeout: float) -> None:
            assert timeout == 1.0

        def connect(self, address: str) -> None:
            assert address == str(tmp_path / "daemon.sock")

        def sendall(self, data: bytes) -> None:
            payload = json.loads(data)
            assert payload["jobs"][0]["id"] == "job0"

        def shutdown(self, how: int) -> None:
            assert how == cli.socket.SHUT_WR

        def recv_into(self, buffer: memoryview) -> int:
            raise cli.socket.timeout("timed out")

    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())
    monkeypatch.setattr(cli, "_pid_alive", lambda pid: False)

    result = _compile_with_backend_daemon_non_wasm(
        tmp_path / "daemon.sock",
        ir={"functions": []},
        backend_output=backend_output,
        target_triple=None,
        cache_key=None,
        function_cache_key=None,
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=None,
        daemon_pid=1234,
    )

    assert result.ok is False
    assert result.error == "backend daemon died while request was in flight"


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


def test_backend_daemon_request_bytes_waits_while_live_daemon_is_still_compiling(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    sent: list[bytes] = []
    pid_checks: list[int] = []

    class _FakeSocket:
        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def settimeout(self, timeout: float) -> None:
            assert timeout == 1.0

        def connect(self, address: str) -> None:
            assert address == str(tmp_path / "daemon.sock")

        def sendall(self, data: bytes) -> None:
            sent.append(data)

        def shutdown(self, how: int) -> None:
            assert how == cli.socket.SHUT_WR

        def recv_into(self, buffer: memoryview) -> int:
            if not hasattr(self, "_steps"):
                self._steps = [
                    cli.socket.timeout("still compiling"),
                    cli.socket.timeout("still compiling"),
                    b'{"ok":true,"pong":false}',
                    b"",
                ]
            step = self._steps.pop(0)
            if isinstance(step, BaseException):
                raise step
            buffer[: len(step)] = step
            return len(step)

    def fake_pid_alive(pid: int) -> bool:
        pid_checks.append(pid)
        return True

    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())
    monkeypatch.setattr(cli, "_pid_alive", fake_pid_alive)

    response, err = cli._backend_daemon_request_bytes(
        tmp_path / "daemon.sock",
        b'{"version":1}\n',
        timeout=None,
        daemon_pid=4321,
    )

    assert err is None
    assert response == {"ok": True, "pong": False}
    assert sent == [b'{"version":1}\n']
    assert pid_checks == [4321, 4321]


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


def test_backend_daemon_request_bytes_ignores_redirect_file(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    sent: list[bytes] = []
    socket_path = tmp_path / "daemon.sock"
    socket_path.with_suffix(".redirect").write_text(str(tmp_path / "foreign.sock"))

    class _FakeSocket:
        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def settimeout(self, timeout: float) -> None:
            assert timeout == 0.25

        def connect(self, address: str) -> None:
            assert address == str(socket_path)

        def sendall(self, data: bytes) -> None:
            sent.append(data)

        def shutdown(self, how: int) -> None:
            assert how == cli.socket.SHUT_WR

        def recv_into(self, buffer: memoryview) -> int:
            assert len(buffer) == 65536
            if not hasattr(self, "_chunks"):
                self._chunks = [b'{"ok":true}', b""]
            chunk = self._chunks.pop(0)
            buffer[: len(chunk)] = chunk
            return len(chunk)

    monkeypatch.setattr(cli.socket, "socket", lambda *args: _FakeSocket())

    response, err = cli._backend_daemon_request_bytes(
        socket_path,
        b'{"version":1}\n',
        timeout=0.25,
    )

    assert err is None
    assert response == {"ok": True}
    assert sent == [b'{"version":1}\n']


def test_kill_stale_backend_daemon_uses_project_canonical_sidecars(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical_root = tmp_path / "target" / ".molt_state"
    daemon_root = canonical_root / "backend_daemon"
    daemon_root.mkdir(parents=True)
    pid_path = daemon_root / "molt-backend.dev-fast.alpha.deadbeef.pid"
    pid_path.write_text("4321\n")
    killed: list[tuple[int, int]] = []
    removed: list[Path] = []

    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    monkeypatch.chdir(elsewhere)
    monkeypatch.setattr(cli, "_build_state_root", lambda project_root: canonical_root)

    def fake_kill(pid: int, sig: int) -> None:
        killed.append((pid, sig))

    monkeypatch.setattr(cli.os, "kill", fake_kill)
    monkeypatch.setattr(
        cli,
        "_remove_backend_daemon_pid",
        lambda path: removed.append(path),
    )

    cli._kill_stale_backend_daemon(tmp_path, "dev-fast")

    assert killed == [(4321, signal.SIGTERM)]
    assert removed == [pid_path]


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


def test_backend_daemon_skip_output_sync_flags_rejects_missing_shared_stdlib(
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
        stdlib_object_path=tmp_path / "cache" / "stdlib_shared_test.o",
        stdlib_object_cache_key="stdlib-key",
    )

    assert skip_module is False
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


def test_write_cached_json_object_creates_parent_directories(tmp_path: Path) -> None:
    cache_path = tmp_path / "nested" / "cache" / "payload.json"
    cli._PERSISTED_JSON_OBJECT_CACHE.clear()

    cli._write_cached_json_object(cache_path, {"version": 1, "hash": "abc"})

    assert cache_path.exists()
    assert json.loads(cache_path.read_text(encoding="utf-8")) == {
        "version": 1,
        "hash": "abc",
    }


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
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
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


def test_try_cached_backend_candidates_rejects_native_hit_without_shared_stdlib(
    tmp_path: Path,
) -> None:
    candidate = tmp_path / "cache" / "module.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    stdlib_object = tmp_path / "cache" / "stdlib_shared_test.o"
    warnings: list[str] = []

    ok, cache_hit_tier = cli._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=[("module", candidate)],
        output_artifact=output_artifact,
        is_wasm=False,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=candidate,
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="stdlib-key",
        warnings=warnings,
    )

    assert ok is False
    assert cache_hit_tier is None
    assert not output_artifact.exists()


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

    empty_c = tmp_path / "empty.c"
    empty_c.write_text("", encoding="utf-8")
    empty_object = tmp_path / "empty-object.o"
    subprocess.run(
        ["clang", "-c", str(empty_c), "-o", str(empty_object)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert not cli._is_valid_cached_backend_artifact(empty_object, is_wasm=False)

    native_c = tmp_path / "native.c"
    native_c.write_text("int foo(void){return 0;}\n", encoding="utf-8")
    native_nonempty = tmp_path / "nonempty.o"
    subprocess.run(
        ["clang", "-c", str(native_c), "-o", str(native_nonempty)],
        check=True,
        capture_output=True,
        text=True,
    )
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


# ---------------------------------------------------------------------------
# C2.1  Cache identity encodes stdlib partition mode
# ---------------------------------------------------------------------------


def test_cache_variant_differs_when_stdlib_split_toggles() -> None:
    """Prove that the cache key changes when stdlib partition mode is toggled.

    ``_prepare_backend_cache_setup`` builds a ``cache_variant`` that feeds into
    the cache key.  The variant includes ``stdlib_split=0`` vs ``stdlib_split=1``
    so a monolithic build can never hit a split-object cache entry (or vice-versa).
    """
    # Minimal IR that _cache_key can digest
    tiny_ir: dict = {
        "module": "__main__",
        "filename": "test.py",
        "ops": [],
        "functions": [],
        "classes": [],
        "constants": {},
        "imports": [],
    }

    warnings: list[str] = []
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    common = dict(
        cache_enabled=True,
        ir=tiny_ir,
        target="native",
        target_triple=None,
        profile="release",
        runtime_cargo_profile="release",
        backend_cargo_profile="release",
        is_wasm=False,
        linked=False,
        project_root=ROOT,
        cache_dir=None,
        warnings=warnings,
        entry_module="__main__",
        module_graph_metadata=module_graph_metadata,
    )

    setup_split = cli._prepare_backend_cache_setup(
        emit_mode="obj",             # native + obj  => split enabled
        output_artifact=ROOT / "dummy_split.o",
        **common,
    )

    # _native_stdlib_object_split_enabled returns True for native target,
    # so we simulate monolithic by patching the helper.
    import unittest.mock as _mock

    with _mock.patch.object(
        cli,
        "_native_stdlib_object_split_enabled",
        return_value=False,
    ):
        setup_mono = cli._prepare_backend_cache_setup(
            emit_mode="obj",
            output_artifact=ROOT / "dummy_mono.o",
            **common,
        )

    assert setup_split.cache_key is not None
    assert setup_mono.cache_key is not None
    assert setup_split.cache_key != setup_mono.cache_key, (
        "Cache keys must differ between stdlib-split and monolithic modes"
    )


# ---------------------------------------------------------------------------
# C2.2  Link fingerprint hashes stdlib partition artifact contents
# ---------------------------------------------------------------------------


def test_link_fingerprint_changes_when_stdlib_artifact_content_changes(
    tmp_path: Path,
) -> None:
    """Prove that _link_fingerprint hashes file contents so that changing a
    stdlib partition artifact forces a relink."""
    user_obj = tmp_path / "user.o"
    user_obj.write_bytes(b"\x00ELF-user")

    stdlib_obj = tmp_path / "stdlib.o"
    stdlib_obj.write_bytes(b"\x00ELF-stdlib-v1")

    link_cmd = ["cc", "-o", "out", "user.o", "stdlib.o"]

    fp1 = cli._link_fingerprint(
        project_root=tmp_path,
        inputs=[user_obj, stdlib_obj],
        link_cmd=link_cmd,
    )

    # Mutate the stdlib artifact
    stdlib_obj.write_bytes(b"\x00ELF-stdlib-v2")

    fp2 = cli._link_fingerprint(
        project_root=tmp_path,
        inputs=[user_obj, stdlib_obj],
        link_cmd=link_cmd,
    )

    assert fp1 is not None and fp2 is not None
    assert fp1["hash"] != fp2["hash"], (
        "Link fingerprint must change when stdlib artifact content changes"
    )


def test_stdlib_partition_mode_changes_cache_identity():
    """Cache identity must differ when stdlib partition mode changes."""
    import sys
    sys.path.insert(0, "src")
    from molt.cli import _build_cache_variant

    variant_mono = _build_cache_variant(
        profile="dev", runtime_cargo="debug", backend_cargo="debug",
        emit="bin", stdlib_split=False, codegen_env="x", linked=False,
        partition_mode=False,
    )
    variant_part = _build_cache_variant(
        profile="dev", runtime_cargo="debug", backend_cargo="debug",
        emit="bin", stdlib_split=False, codegen_env="x", linked=False,
        partition_mode=True,
    )
    assert variant_mono != variant_part, (
        "Cache variant must change when partition mode changes"
    )
    assert "partitioned" in variant_part
    assert "partitioned" not in variant_mono
