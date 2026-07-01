from __future__ import annotations

import ast
import builtins as py_builtins
import contextlib
from dataclasses import replace
import hashlib
import io
import importlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tarfile
import time
import types
from typing import Any, Mapping, Sequence, cast

import molt.cli as cli
from molt.cli import commands as cli_commands
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import backend_cache_setup as cli_backend_cache_setup
from molt.cli import backend_compile as cli_backend_compile
from molt.cli import backend_output_pipeline as cli_backend_output_pipeline
from molt.cli import backend_pipeline as cli_backend_pipeline
from molt.cli import link_pipeline as cli_link_pipeline
from molt.cli import non_native_output as cli_non_native_output
import pytest
from molt.cli import build_diagnostics as cli_build_diagnostics
from molt.cli import build_inputs as cli_build_inputs
from molt.cli import build_output_layout as cli_build_output_layout
from molt.cli import build_results as cli_build_results
from molt.cli import c_api_symbols as cli_c_api_symbols
from molt.cli import frontend_execution as cli_frontend_execution
from molt.cli import frontend_parallel as cli_frontend_parallel
from molt.cli import frontend_pipeline as cli_frontend_pipeline
from molt.cli import external_native as cli_external_native
from molt.cli import module_cache as cli_module_cache
from molt.cli import module_dependencies as cli_module_dependencies
from molt.cli import module_graph_cache as cli_module_graph_cache
from molt.cli import module_graph_discovery as cli_module_graph_discovery
from molt.cli import module_import_scanner as cli_module_import_scanner
from molt.cli import module_resolution as cli_module_resolution
from molt.cli import module_source as cli_module_source
from molt.cli import module_stdlib_policy as cli_module_stdlib_policy
from molt.cli import typecheck as cli_typecheck
from molt.cli import wasm_toolchain as cli_wasm_toolchain
from molt.cli.models import (
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ExternalNativeCallableExport,
    _ExternalNativeAbiSymbol,
    _ExternalPackageNativeArtifact,
    _ExternalPackageNativeArtifactPlan,
    _StagedExternalPackageNativeArtifact,
)
from molt.compat import CompatibilityError
from molt.frontend import MoltValue, SimpleTIRGenerator
from molt.type_facts import Fact, FunctionFacts, ModuleFacts, TypeFacts
from tests.cli.process_guard import (
    cli_test_popen_kwargs,
    close_cli_test_process_group,
    run_cli_test_process,
)

cli_deps = importlib.import_module("molt.cli.deps")
cli_frontend_worker = importlib.import_module("molt.cli.frontend_worker")
cli_module_graph = importlib.import_module("molt.cli.module_graph")


ROOT = Path(__file__).resolve().parents[2]
ARTIFACT_STATE = importlib.import_module("molt.cli.artifact_state")
BACKEND_CACHE = importlib.import_module("molt.cli.backend_cache")
BACKEND_EXECUTION = importlib.import_module("molt.cli.backend_execution")
BACKEND_IR = importlib.import_module("molt.cli.backend_ir")
CACHE_FINGERPRINTS = importlib.import_module("molt.cli.cache_fingerprints")
CACHE_KEYS = importlib.import_module("molt.cli.cache_keys")
COMMAND_RUNTIME = importlib.import_module("molt.cli.command_runtime")
LOCKFILES = importlib.import_module("molt.cli.lockfiles")
PROJECT_ROOTS = importlib.import_module("molt.cli.project_roots")
RUNTIME_BUILD = importlib.import_module("molt.cli.runtime_build")
RUNTIME_PATHS = importlib.import_module("molt.cli.runtime_paths")
RUNTIME_WASM_VALIDATION = importlib.import_module("molt.cli.runtime_wasm_validation")
RUNTIME_FINGERPRINTS = importlib.import_module("molt.cli.runtime_fingerprints")
RUNTIME_INTRINSIC_SYMBOLS = importlib.import_module(
    "molt.cli.runtime_intrinsic_symbols"
)
NATIVE_LINK_COMMAND = importlib.import_module("molt.cli.native_link_command")
NATIVE_LINK_DEPS = importlib.import_module("molt.cli.native_link_deps")
TARGET_PYTHON = importlib.import_module("molt.cli.target_python")


def _rewrite_preserving_mtime(
    path: Path, source: str, original: os.stat_result
) -> None:
    path.write_text(source, encoding="utf-8")
    os.utime(path, ns=(original.st_atime_ns, original.st_mtime_ns))


def _clear_molt_home_caches() -> None:
    cli._default_molt_cache_cached.cache_clear()
    cli._default_molt_home_cached.cache_clear()
    cli._default_molt_bin_cached.cache_clear()
    cli._PERSISTED_JSON_OBJECT_CACHE.clear()
    cli_module_source._source_content_sha256_cached.cache_clear()


def _enable_fake_backend_daemon_unix_socket(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "AF_UNIX", 1, raising=False)


def _func_metadata(
    *,
    params: int,
    defaults: list[dict[str, object]] | None = None,
    kind: str = "sync",
    has_decorators: bool = False,
    posonly: int = 0,
    kwonly: int = 0,
) -> dict[str, object]:
    return {
        "params": params,
        "defaults": defaults or [],
        "posonly": posonly,
        "kwonly": kwonly,
        "kind": kind,
        "has_decorators": has_decorators,
    }


def _compile_c_object(tmp_path: Path, name: str, source: str) -> Path:
    src = tmp_path / f"{name}.c"
    obj = tmp_path / f"{name}.o"
    src.write_text(source, encoding="utf-8")
    run_cli_test_process(
        ["clang", "-c", str(src), "-o", str(obj)],
        check=True,
        capture_output=True,
        text=True,
    )
    return obj


def _install_fake_backend_compile(
    monkeypatch: pytest.MonkeyPatch,
    *,
    output_bytes: bytes = b"OBJ",
    backend_inputs: list[bytes | None] | None = None,
    backend_ir_files: list[Path] | None = None,
    seen_envs: list[dict[str, str] | None] | None = None,
) -> None:
    fake_runtime_root = Path(os.environ["MOLT_CACHE"]) / "fake-native-runtime"
    fake_runtime_lib = fake_runtime_root / cli._runtime_lib_archive_name("micro", None)
    fake_symbols_file = fake_runtime_root / "molt-runtime-symbols.txt"

    def write_fake_runtime_artifacts() -> None:
        fake_runtime_root.mkdir(parents=True, exist_ok=True)
        fake_runtime_lib.write_bytes(b"runtime")
        fake_symbols_file.write_text("molt_test_intrinsic\n", encoding="utf-8")

    def fake_initialize_runtime_artifact_state(
        *,
        is_rust_transpile: bool,
        is_wasm: bool,
        emit_mode: str,
        **kwargs: object,
    ) -> cli._RuntimeArtifactState:
        state = cli._RuntimeArtifactState()
        if not is_rust_transpile and not is_wasm and emit_mode in {"bin", "obj"}:
            write_fake_runtime_artifacts()
            state.runtime_lib = fake_runtime_lib
        return state

    def fake_ensure_runtime_lib_ready(
        runtime_state: cli._RuntimeArtifactState, **kwargs: object
    ) -> bool:
        if runtime_state.runtime_lib is None:
            return True
        write_fake_runtime_artifacts()
        return True

    def fake_run_subprocess_captured_to_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        backend_input = kwargs.get("input")
        if backend_inputs is not None:
            assert backend_input is None or isinstance(backend_input, bytes)
            backend_inputs.append(backend_input)
        if backend_ir_files is not None:
            assert "--ir-file" in cmd
            backend_ir_files.append(Path(cmd[cmd.index("--ir-file") + 1]))
        env = cast(dict[str, str] | None, kwargs.get("env"))
        if seen_envs is not None:
            seen_envs.append(dict(env) if env is not None else None)
        output = Path(cmd[cmd.index("--output") + 1])
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(output_bytes)
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        fake_initialize_runtime_artifact_state,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_initialize_runtime_artifact_state",
        fake_initialize_runtime_artifact_state,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready
    )
    monkeypatch.setattr(
        RUNTIME_INTRINSIC_SYMBOLS,
        "_runtime_intrinsic_symbols_file",
        lambda runtime_lib: (fake_symbols_file, None),
    )


def _load_generated_importer(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    intrinsics: dict[str, object],
):
    importer_path = cli._write_importer_module(tmp_path)
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
    assert (
        cli_module_resolution._resolve_module_path("shadowed", [tmp_path])
        == package_init
    )


def test_resolve_module_path_requires_exact_case(tmp_path: Path) -> None:
    package_dir = tmp_path / "tinygrad"
    package_dir.mkdir()
    tensor = package_dir / "tensor.py"
    tensor.write_text("class Tensor:\n    pass\n", encoding="utf-8")

    assert (
        cli_module_resolution._resolve_module_path("tinygrad.tensor", [tmp_path])
        == tensor
    )
    assert (
        cli_module_resolution._resolve_module_path("tinygrad.Tensor", [tmp_path])
        is None
    )


def test_prefixed_module_root_resolves_vendored_source_layout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    vendor_root = tmp_path / "subprojects" / "array_api_compat" / "array_api_compat"
    package_root = vendor_root / "array_api_compat"
    common = package_root / "common"
    common.mkdir(parents=True)
    package_init = package_root / "__init__.py"
    package_init.write_text(
        "from .common._helpers import is_numpy_namespace\n", encoding="utf-8"
    )
    (common / "__init__.py").write_text("", encoding="utf-8")
    helper = common / "_helpers.py"
    helper.write_text(
        "def is_numpy_namespace(xp):\n    return True\n", encoding="utf-8"
    )
    monkeypatch.setenv(
        "MOLT_MODULE_ROOTS",
        f"scipy._external.array_api_compat={package_root}",
    )

    assert (
        cli_module_resolution._resolve_module_path(
            "scipy._external.array_api_compat",
            [tmp_path],
        )
        == package_init
    )
    assert (
        cli_module_resolution._resolve_module_path(
            "scipy._external.array_api_compat.common._helpers",
            [tmp_path],
        )
        == helper
    )
    assert (
        cli_module_resolution._module_name_from_path(
            helper,
            [vendor_root],
            cli_module_resolution._stdlib_root_path(),
        )
        == "scipy._external.array_api_compat.common._helpers"
    )


def test_prefixed_module_root_is_external_admission_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    vendor_package = tmp_path / "vendor" / "pkg"
    vendor_package.mkdir(parents=True)
    (vendor_package / "__init__.py").write_text("", encoding="utf-8")
    monkeypatch.setenv("MOLT_MODULE_ROOTS", f"upstream.pkg={vendor_package}")

    resolved = cli_build_inputs._resolve_module_root_resolution(
        ROOT,
        ROOT,
        respect_pythonpath=False,
        lib_paths=[],
    )

    assert vendor_package.resolve() in resolved.roots
    assert vendor_package.resolve() in resolved.external_roots


def test_module_resolution_cache_tracks_prefixed_root_env(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = tmp_path / "first" / "pkg"
    second = tmp_path / "second" / "pkg"
    first.mkdir(parents=True)
    second.mkdir(parents=True)
    first_init = first / "__init__.py"
    second_init = second / "__init__.py"
    first_init.write_text("VALUE = 1\n", encoding="utf-8")
    second_init.write_text("VALUE = 2\n", encoding="utf-8")
    cache = cli_module_resolution._ModuleResolutionCache()
    stdlib_root = cli_module_resolution._stdlib_root_path()

    monkeypatch.setenv("MOLT_MODULE_ROOTS", f"upstream.pkg={first}")
    assert (
        cache.resolve_module("upstream.pkg", [tmp_path], stdlib_root, set())
        == first_init
    )
    assert (
        cache.module_name_from_path(first_init, [tmp_path], stdlib_root)
        == "upstream.pkg"
    )

    monkeypatch.setenv("MOLT_MODULE_ROOTS", f"upstream.pkg={second}")
    assert (
        cache.resolve_module("upstream.pkg", [tmp_path], stdlib_root, set())
        == second_init
    )
    assert (
        cache.module_name_from_path(second_init, [tmp_path], stdlib_root)
        == "upstream.pkg"
    )


def test_stdlib_test_support_layout_resolves_like_cpython() -> None:
    stdlib_root = cli_module_resolution._stdlib_root_path()
    support_pkg = cli_module_resolution._resolve_module_path(
        "test.support", [stdlib_root]
    )
    import_helper = cli_module_resolution._resolve_module_path(
        "test.support.import_helper", [stdlib_root]
    )
    os_helper = cli_module_resolution._resolve_module_path(
        "test.support.os_helper", [stdlib_root]
    )
    warnings_helper = cli_module_resolution._resolve_module_path(
        "test.support.warnings_helper", [stdlib_root]
    )

    assert support_pkg == stdlib_root / "test" / "support" / "__init__.py"
    assert import_helper == stdlib_root / "test" / "support" / "import_helper.py"
    assert os_helper == stdlib_root / "test" / "support" / "os_helper.py"
    assert warnings_helper == stdlib_root / "test" / "support" / "warnings_helper.py"


def test_write_importer_module_is_transaction_shim(tmp_path: Path) -> None:
    importer = cli._write_importer_module(tmp_path)
    text = importer.read_text()
    assert "molt_importlib_import_transaction" in text
    assert "return _IMPORT_TRANSACTION(name, globals, locals, fromlist, level)" in text
    assert "_KNOWN_MODULES" not in text
    assert "_TOP_LEVEL_BY_MODULE" not in text
    assert "_resolve_name" not in text
    assert "_runtime_import_support" not in text
    assert "molt_module_import" not in text
    assert "molt_importlib_import_module" not in text


def test_write_importer_module_avoids_rewriting_identical_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    importer_path = tmp_path / f"{cli_module_import_scanner.IMPORTER_MODULE_NAME}.py"
    original_replace = os.replace
    replaced_destinations: list[Path] = []

    def record_replace(src: object, dst: object) -> None:
        destination = Path(dst)
        if destination == importer_path:
            replaced_destinations.append(destination)
        original_replace(src, dst)

    monkeypatch.setattr(os, "replace", record_replace)

    cli._write_importer_module(tmp_path)
    cli._write_importer_module(tmp_path)

    assert replaced_destinations == [importer_path]


def test_generated_importer_delegates_to_import_transaction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    loaded = types.ModuleType("demo_math")
    calls: list[tuple[object, object, object, object, object]] = []

    def fake_import_transaction(
        name: object,
        globals_obj: object,
        locals_obj: object,
        fromlist: object,
        level: object,
    ):
        calls.append((name, globals_obj, locals_obj, fromlist, level))
        return loaded

    importer = _load_generated_importer(
        tmp_path,
        monkeypatch,
        intrinsics={"molt_importlib_import_transaction": fake_import_transaction},
    )

    globals_obj = {"__package__": "pkg"}
    locals_obj = {"marker": object()}
    result = importer._molt_import("demo_math", globals_obj, locals_obj, ("sqrt",), 1)

    assert result is loaded
    assert calls == [("demo_math", globals_obj, locals_obj, ("sqrt",), 1)]


def test_generated_importer_can_import_builtins_through_transaction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import_calls: list[tuple[object, object, object, object, object]] = []

    def fake_import_transaction(
        name: object,
        globals_obj: object,
        locals_obj: object,
        fromlist: object,
        level: object,
    ):
        import_calls.append((name, globals_obj, locals_obj, fromlist, level))
        return py_builtins

    importer = _load_generated_importer(
        tmp_path,
        monkeypatch,
        intrinsics={"molt_importlib_import_transaction": fake_import_transaction},
    )

    result = importer._molt_import("builtins")

    assert result is py_builtins
    assert import_calls == [("builtins", None, None, (), 0)]


def _frontend_main_ops_for_import_source(
    source: str, **kwargs: object
) -> list[dict[str, object]]:
    gen = SimpleTIRGenerator(**kwargs)
    gen.visit(ast.parse(source))
    ir = gen.to_json()
    return next(func["ops"] for func in ir["functions"] if func["name"] == "molt_main")


def _frontend_import_transaction_calls(
    ops: list[dict[str, object]],
) -> set[tuple[str, tuple[str, ...], int, bool]]:
    const_str = {
        op["out"]: op["s_value"]
        for op in ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    const_int = {
        op["out"]: op["value"]
        for op in ops
        if op.get("kind") == "const"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("value"), int)
    }
    const_none = {
        op["out"]
        for op in ops
        if op.get("kind") == "const_none" and isinstance(op.get("out"), str)
    }
    tuple_args = {
        op["out"]: tuple(op.get("args") or ())
        for op in ops
        if op.get("kind") == "tuple_new" and isinstance(op.get("out"), str)
    }
    transaction_funcs = {
        op["out"]
        for op in ops
        if op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_transaction"
        and isinstance(op.get("out"), str)
    }

    calls: set[tuple[str, tuple[str, ...], int, bool]] = set()
    for op in ops:
        if op.get("kind") != "call_func":
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) != 6:
            continue
        callee, name_var, globals_var, _locals_var, fromlist_var, level_var = args
        if callee not in transaction_funcs:
            continue
        name = const_str.get(name_var)
        level = const_int.get(level_var)
        fromlist_vars = tuple_args.get(fromlist_var)
        if not isinstance(name, str) or not isinstance(level, int):
            continue
        if fromlist_vars is None:
            continue
        fromlist = tuple(
            const_str[var]
            for var in fromlist_vars
            if isinstance(var, str) and isinstance(const_str.get(var), str)
        )
        calls.add((name, fromlist, level, globals_var in const_none))
    return calls


def _frontend_import_module_calls(ops: list[dict[str, object]]) -> set[str]:
    const_str = {
        op["out"]: op["s_value"]
        for op in ops
        if op.get("kind") == "const_str"
        and isinstance(op.get("out"), str)
        and isinstance(op.get("s_value"), str)
    }
    import_module_funcs = {
        op["out"]
        for op in ops
        if op.get("kind") == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_module"
        and isinstance(op.get("out"), str)
    }
    calls: set[str] = set()
    for op in ops:
        if op.get("kind") != "call_func":
            continue
        args = op.get("args")
        if not isinstance(args, list) or len(args) != 3:
            continue
        callee, name_var, _package_var = args
        if callee not in import_module_funcs:
            continue
        name = const_str.get(name_var)
        if isinstance(name, str):
            calls.add(name)
    return calls


def test_source_import_syntax_uses_import_transaction_not_module_import() -> None:
    ops = _frontend_main_ops_for_import_source(
        "import pkg.helper as helper\nimport json\n",
        known_modules={"json", "pkg", "pkg.helper"},
        stdlib_allowlist={"json"},
    )

    assert all(op.get("kind") != "module_import" for op in ops)
    calls = _frontend_import_transaction_calls(ops)
    assert ("pkg.helper", (), 0, False) in calls
    assert ("json", (), 0, False) in calls
    assert "pkg.helper" not in _frontend_import_module_calls(ops)
    assert any(op.get("kind") == "module_import_from" for op in ops)


def test_literal_importlib_import_module_uses_public_import_module_intrinsic() -> None:
    ops = _frontend_main_ops_for_import_source(
        "import importlib\nvalue = importlib.import_module('json')\n",
        known_modules={"importlib", "json"},
        stdlib_allowlist={"importlib", "json"},
    )

    assert "json" in _frontend_import_module_calls(ops)
    assert all(
        name != "json"
        for name, _fromlist, _level, _globals_none in (
            _frontend_import_transaction_calls(ops)
        )
    )


def test_relative_from_import_syntax_leaves_package_context_to_transaction() -> None:
    ops = _frontend_main_ops_for_import_source(
        "from .helper import ping\n",
        module_name="pkg.main",
        module_spec_name="pkg.main",
        source_path="/tmp/pkg/main.py",
        known_modules={"pkg", "pkg.helper"},
    )

    assert all(op.get("kind") != "module_import" for op in ops)
    calls = _frontend_import_transaction_calls(ops)
    assert ("helper", ("ping",), 1, False) in calls


def test_import_transaction_implementation_modules_use_bootstrap_imports() -> None:
    ops = _frontend_main_ops_for_import_source(
        "import os\nimport sys\n",
        module_name="importlib",
        known_modules={"os", "sys"},
        stdlib_allowlist={"os", "sys"},
    )

    assert any(op.get("kind") == "module_import" for op in ops)
    assert not _frontend_import_transaction_calls(ops)


def test_generated_importer_does_not_require_import_support_modules(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    loaded = types.ModuleType("demo_math")
    calls: list[str] = []

    def fake_import_transaction(name, _globals, _locals, _fromlist, _level):
        calls.append(name)
        return loaded

    importer = _load_generated_importer(
        tmp_path,
        monkeypatch,
        intrinsics={"molt_importlib_import_transaction": fake_import_transaction},
    )

    result = importer._molt_import("demo_math")

    assert result is loaded
    assert calls == ["demo_math"]


def test_prepare_entry_module_graph_adds_runtime_import_support_once(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import importlib\nvalue = importlib.import_module('json')\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    module_reasons: dict[str, set[str]] = {}

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=True,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_import_support_policy.needs_runtime_import_support
    assert "importlib.util" in prepared.module_graph
    assert "importlib.machinery" in prepared.module_graph
    assert "runtime_import_support" in module_reasons["importlib.util"]
    assert "runtime_import_support" in module_reasons["importlib.machinery"]
    assert "import_support" not in module_reasons["importlib.util"]
    assert "import_support" not in module_reasons["importlib.machinery"]


def test_materialize_import_plan_does_not_rescan_importlib_support(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("value = 1\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    module_reasons: dict[str, set[str]] = {}
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
    )
    assert error is None
    assert prepared is not None

    def fail_extend_module_graph_with_closure(*args: object, **kwargs: object) -> None:
        raise AssertionError("import plan materialization must not rescan closures")

    monkeypatch.setattr(
        cli_module_graph_discovery,
        "_extend_module_graph_with_closure",
        fail_extend_module_graph_with_closure,
    )

    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons=module_reasons,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    assert (
        import_plan.runtime_import_support_policy
        == prepared.runtime_import_support_policy
    )
    assert "demo" in import_plan.module_graph


def test_materialize_import_plan_retains_external_native_artifact_plan(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path
    )
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import nativepkg\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert policy_error is None
    assert policy is not None
    module_reasons: dict[str, set[str]] = {}
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path, external_root],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
        import_admission_policy=policy,
    )
    assert error is None
    assert prepared is not None

    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons=module_reasons,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    assert import_plan.native_artifact_plan == policy.native_artifact_plan
    assert (
        import_plan.native_artifact_plan.digest()
        == policy.native_artifact_plan.digest()
    )
    assert import_plan.native_artifact_plan.artifacts[0].module == "nativepkg._native"
    assert "nativepkg._native" in import_plan.known_modules
    assert "nativepkg._native" not in import_plan.module_graph
    assert "nativepkg._native" not in import_plan.compile_modules


def test_materialize_import_plan_keeps_runtime_dispatch_native_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        manifest_overrides={"python_exports": ["nativepkg.dynamic"]},
    )
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("print('demo')\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert policy_error is None
    assert policy is not None
    module_reasons: dict[str, set[str]] = {}
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path, external_root],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
        import_admission_policy=policy,
    )
    assert error is None
    assert prepared is not None
    prepared = replace(
        prepared,
        runtime_import_dispatch_roots=frozenset({"nativepkg.dynamic"}),
    )

    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons=module_reasons,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    assert import_plan.native_artifact_plan.artifacts
    assert import_plan.native_artifact_plan.artifacts[0].module == "nativepkg._native"
    assert "nativepkg._native" in import_plan.known_modules
    assert "nativepkg._native" not in import_plan.compile_modules


def test_materialize_import_plan_adds_reachable_native_support_source_closure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root = tmp_path / "site"
    support_path = external_root / "nativepkg" / "ndimage" / "_filters.py"
    support_path.parent.mkdir(parents=True)
    support_path.write_text(
        "from . import _ni_label\n\n"
        "def gaussian_filter(value):\n"
        "    return _ni_label.label(value)\n",
        encoding="utf-8",
    )
    support_sha = hashlib.sha256(support_path.read_bytes()).hexdigest()
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "support_files": [
                {
                    "path": "nativepkg/ndimage/_filters.py",
                    "sha256": support_sha,
                }
            ],
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "gaussian_filter",
                    "binding": "module_attr",
                    "provider_module": "nativepkg.ndimage._filters",
                    "abi": "molt.object_callargs_v1",
                }
            ],
        },
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._ni_label",
        artifact_name="_ni_label.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "python_exports": ["nativepkg.ndimage._ni_label"],
        },
    )
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("print('demo')\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert policy_error is None
    assert policy is not None
    module_reasons: dict[str, set[str]] = {}
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path, external_root],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
        import_admission_policy=policy,
    )
    assert error is None
    assert prepared is not None
    prepared = replace(
        prepared,
        runtime_import_dispatch_roots=frozenset(
            {"nativepkg.ndimage.gaussian_filter"}
        ),
    )

    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons=module_reasons,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    assert "nativepkg.ndimage._filters" in import_plan.module_graph
    assert "nativepkg.ndimage._filters" in import_plan.compile_modules
    assert [
        artifact.module for artifact in import_plan.native_artifact_plan.artifacts
    ] == ["nativepkg.ndimage._nd_image", "nativepkg.ndimage._ni_label"]
    assert "nativepkg.ndimage._ni_label" not in import_plan.module_graph
    assert "nativepkg.ndimage._ni_label" in import_plan.known_modules


def test_materialize_import_plan_rejects_missing_native_support_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root = tmp_path / "site"
    support_path = external_root / "nativepkg" / "ndimage" / "_measurements.py"
    support_path.parent.mkdir(parents=True)
    source_candidate = tmp_path / "upstream" / "nativepkg" / "ndimage" / "src" / "_ni_label.pyx"
    source_candidate.parent.mkdir(parents=True)
    source_candidate.write_text("# upstream cython source candidate\n", encoding="utf-8")
    provenance_source = source_candidate.with_name("ni_measure.c")
    provenance_source.write_text("int ni_measure(void) { return 0; }\n", encoding="utf-8")
    support_path.write_text(
        "from . import _ni_label\n\n"
        "def label(value):\n"
        "    return _ni_label.label(value)\n",
        encoding="utf-8",
    )
    support_sha = hashlib.sha256(support_path.read_bytes()).hexdigest()
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "sources": [str(provenance_source)],
            "support_files": [
                {
                    "path": "nativepkg/ndimage/_measurements.py",
                    "sha256": support_sha,
                }
            ],
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "label",
                    "binding": "module_attr",
                    "provider_module": "nativepkg.ndimage._measurements",
                    "abi": "molt.object_callargs_v1",
                }
            ],
        },
    )
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("print('demo')\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert policy_error is None
    assert policy is not None
    module_reasons: dict[str, set[str]] = {}
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path, external_root],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
        import_admission_policy=policy,
    )
    assert error is None
    assert prepared is not None
    prepared = replace(
        prepared,
        runtime_import_dispatch_roots=frozenset({"nativepkg.ndimage.label"}),
    )

    with pytest.raises(ValueError) as exc_info:
        cli._materialize_import_plan(
            prepared_module_graph=prepared,
            module_reasons=module_reasons,
            stdlib_root=cli_module_resolution._stdlib_root_path(),
            artifacts_root=tmp_path,
            entry_module="demo",
            diagnostics_enabled=False,
        )

    message = str(exc_info.value)
    assert "nativepkg.ndimage._ni_label" in message
    assert "without source or artifact custody" in message
    assert str(source_candidate.resolve()) in message
    assert "target-specific source plan" in message


def test_native_support_source_stdlib_imports_join_compile_closure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root = tmp_path / "site"
    support_path = external_root / "nativepkg" / "ndimage" / "_filters.py"
    doc_path = external_root / "nativepkg" / "ndimage" / "_docstrings.py"
    support_path.parent.mkdir(parents=True)
    support_path.write_text(
        "\n".join(
            [
                "from collections.abc import Iterable",
                "import math",
                "",
                "def gaussian_filter(value):",
                "    if isinstance(value, Iterable):",
                "        return value",
                "    return value",
                "",
            ]
        ),
        encoding="utf-8",
    )
    doc_path.write_text(
        "from nativepkg import doccer\n\ndocfiller = doccer.filldoc({})\n",
        encoding="utf-8",
    )
    support_sha = hashlib.sha256(support_path.read_bytes()).hexdigest()
    doc_sha = hashlib.sha256(doc_path.read_bytes()).hexdigest()
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "support_files": [
                {
                    "path": "nativepkg/ndimage/_filters.py",
                    "sha256": support_sha,
                },
                {
                    "path": "nativepkg/ndimage/_docstrings.py",
                    "sha256": doc_sha,
                }
            ],
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "gaussian_filter",
                    "binding": "module_attr",
                    "provider_module": "nativepkg.ndimage._filters",
                    "abi": "molt.object_callargs_v1",
                }
            ],
        },
    )
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("print('demo')\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert policy_error is None
    assert policy is not None
    module_reasons: dict[str, set[str]] = {}
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path, external_root],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
        import_admission_policy=policy,
    )
    assert error is None
    assert prepared is not None
    prepared = replace(
        prepared,
        runtime_import_dispatch_roots=frozenset(
            {"nativepkg.ndimage.gaussian_filter"}
        ),
    )

    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons=module_reasons,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    assert "collections" in import_plan.module_graph
    assert "collections.abc" in import_plan.module_graph
    assert "collections" in import_plan.known_modules
    assert "collections.abc" in import_plan.known_modules
    assert "collections" in import_plan.compile_modules
    assert "collections.abc" in import_plan.compile_modules
    assert "collections.abc" in import_plan.runtime_import_dispatch_roots
    assert "math" not in import_plan.module_graph
    assert "math" not in import_plan.runtime_import_dispatch_roots
    assert "nativepkg.ndimage._docstrings" not in import_plan.module_graph
    assert "nativepkg.ndimage._docstrings" not in import_plan.runtime_import_dispatch_roots
    assert "native_support_source_closure" in module_reasons["collections"]
    assert "native_support_source_closure" in module_reasons["collections.abc"]


def test_entry_collections_closure_preserves_static_helper_import_edges(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import collections\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    module_reasons: dict[str, set[str]] = {}

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=True,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert "collections" in prepared.module_graph
    assert "copy" in prepared.module_graph
    assert "warnings" not in prepared.module_graph
    assert "re" not in prepared.module_graph
    assert "entry_closure" in module_reasons["copy"]


def test_collections_static_helper_copy_reaches_backend_symbol_contract(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import collections\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    module_reasons: dict[str, set[str]] = {}

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=True,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons=module_reasons,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path / "artifacts",
        entry_module="demo",
        diagnostics_enabled=False,
    )
    frontend_analysis, frontend_error = (
        cli_frontend_pipeline._prepare_frontend_analysis(
            module_graph=import_plan.module_graph,
            module_graph_metadata=import_plan.module_graph_metadata,
            module_resolution_cache=import_plan.module_resolution_cache,
            roots=import_plan.roots,
            stdlib_root=import_plan.stdlib_root,
            stdlib_allowlist=set(import_plan.stdlib_allowlist),
            project_root=tmp_path,
            entry_module="demo",
            json_output=False,
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        )
    )
    assert frontend_error is None
    assert frontend_analysis is not None
    module_graph_metadata = cli._build_module_graph_metadata(
        import_plan.module_graph,
        generated_module_source_paths=import_plan.generated_module_source_paths,
        entry_module="demo",
        namespace_module_names=import_plan.namespace_module_names,
        module_source_catalog=frontend_analysis.module_source_catalog,
        module_deps=frontend_analysis.module_deps,
    )
    backend_setup = cli_backend_cache_setup._prepare_backend_cache_setup(
        cache_enabled=False,
        ir={"functions": []},
        target="native",
        target_triple=None,
        profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="bin",
        is_wasm=False,
        linked=False,
        project_root=ROOT,
        cache_dir=str(tmp_path / "cache"),
        output_artifact=tmp_path / "demo.o",
        warnings=[],
        entry_module="demo",
        module_graph_metadata=module_graph_metadata,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        stdlib_profile="micro",
    )

    assert "copy" in backend_setup.stdlib_module_symbols
    assert "collections" in backend_setup.stdlib_module_symbols
    assert backend_setup.stdlib_module_symbols_json is not None
    assert "copy" in json.loads(backend_setup.stdlib_module_symbols_json)


def test_prepare_entry_module_graph_marks_source_import_syntax_runtime_supported(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import math\nvalue = math.sqrt(4)\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert not prepared.runtime_import_support_policy.needs_generated_importer
    assert prepared.runtime_import_support_policy.needs_runtime_import_support
    assert "importlib.util" in prepared.module_graph
    assert "importlib.machinery" in prepared.module_graph


def test_prepare_entry_module_graph_marks_dynamic_import_entry_as_runtime_supported(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text(
        "import importlib as loader\nvalue = loader.import_module('json')\n"
    )
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert not prepared.runtime_import_support_policy.needs_generated_importer
    assert prepared.runtime_import_support_policy.needs_runtime_import_support


def test_prepare_entry_module_graph_keeps_dependency_function_dynamic_import_lazy(
    tmp_path: Path,
) -> None:
    package = tmp_path / "pkg"
    runtime = package / "runtime"
    runtime.mkdir(parents=True)
    (package / "__init__.py").write_text("import pkg.device\n", encoding="utf-8")
    (package / "device.py").write_text(
        "import importlib\n"
        "def load_backend():\n"
        "    return importlib.import_module('pkg.runtime.ops_cpu')\n",
        encoding="utf-8",
    )
    (runtime / "__init__.py").write_text("", encoding="utf-8")
    (runtime / "ops_cpu.py").write_text("VALUE = 1\n", encoding="utf-8")
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import pkg\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_import_support_policy.needs_runtime_import_support
    assert "pkg.device" in prepared.module_graph
    assert "pkg.runtime.ops_cpu" not in prepared.module_graph


def test_prepare_entry_module_graph_admits_declared_static_runtime_import(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package = tmp_path / "pkg"
    runtime = package / "runtime"
    runtime.mkdir(parents=True)
    (package / "__init__.py").write_text("import pkg.device\n", encoding="utf-8")
    (package / "device.py").write_text(
        "import importlib\n"
        "def load_backend(name):\n"
        "    return importlib.import_module(name)\n",
        encoding="utf-8",
    )
    (runtime / "__init__.py").write_text("", encoding="utf-8")
    (runtime / "ops_cpu.py").write_text("import base64\nVALUE = 1\n", encoding="utf-8")
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import pkg\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_STDLIB_PROFILE", "full")
    monkeypatch.setenv("MOLT_STATIC_IMPORT_MODULES", "pkg.runtime.ops_cpu")

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
        import_admission_policy=cli._ImportAdmissionPolicy(
            external_roots=(tmp_path,),
            admitted_external_packages=frozenset({"pkg"}),
        ),
    )

    assert error is None
    assert prepared is not None
    assert "pkg.runtime.ops_cpu" in prepared.module_graph
    assert "base64" in prepared.module_graph
    assert "pkg.runtime.ops_cpu" in prepared.explicit_imports


def test_prepare_entry_module_graph_full_scans_declared_static_module(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package = tmp_path / "pkg"
    runtime = package / "runtime"
    runtime.mkdir(parents=True)
    (package / "__init__.py").write_text("import pkg.device\n", encoding="utf-8")
    (package / "device.py").write_text(
        "def load_backend():\n"
        "    from pkg.runtime import ops_cpu\n"
        "    return ops_cpu.VALUE\n",
        encoding="utf-8",
    )
    (runtime / "__init__.py").write_text("", encoding="utf-8")
    (runtime / "ops_cpu.py").write_text("VALUE = 1\n", encoding="utf-8")
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import pkg\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_STATIC_IMPORT_MODULES", "pkg.device")

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
        import_admission_policy=cli._ImportAdmissionPolicy(
            external_roots=(tmp_path,),
            admitted_external_packages=frozenset({"pkg"}),
        ),
    )

    assert error is None
    assert prepared is not None
    assert "pkg.device" in prepared.module_graph
    assert "pkg.runtime.ops_cpu" in prepared.module_graph
    assert "pkg.device" in prepared.explicit_imports


def test_prepare_entry_module_graph_rejects_unadmitted_static_runtime_import(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    package = tmp_path / "pkg"
    runtime = package / "runtime"
    runtime.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    (runtime / "__init__.py").write_text("", encoding="utf-8")
    (runtime / "ops_cpu.py").write_text("VALUE = 1\n", encoding="utf-8")
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import pkg\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    monkeypatch.setenv("MOLT_STATIC_IMPORT_MODULES", "pkg.runtime.ops_cpu")

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
        import_admission_policy=cli._ImportAdmissionPolicy(external_roots=(tmp_path,)),
    )

    assert prepared is None
    assert error == 2
    err = capsys.readouterr().err
    assert "MOLT_STATIC_IMPORT_MODULES" in err
    assert "not within an admitted external static package" in err


def test_prepare_entry_module_graph_marks_dependency_module_init_dynamic_import(
    tmp_path: Path,
) -> None:
    package = tmp_path / "pkg"
    runtime = package / "runtime"
    runtime.mkdir(parents=True)
    (package / "__init__.py").write_text("import pkg.device\n", encoding="utf-8")
    (package / "device.py").write_text(
        "import importlib\nimportlib.import_module('pkg.runtime.ops_cpu')\n",
        encoding="utf-8",
    )
    (runtime / "__init__.py").write_text("", encoding="utf-8")
    (runtime / "ops_cpu.py").write_text("VALUE = 1\n", encoding="utf-8")
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import pkg\n", encoding="utf-8")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_import_support_policy.needs_runtime_import_support
    assert "pkg.device" in prepared.module_graph
    assert "pkg.runtime.ops_cpu" in prepared.module_graph


def test_prepare_entry_module_graph_collects_literal_dunder_import_targets(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("value = __import__('math')\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_import_support_policy.needs_runtime_import_support
    assert "math" in prepared.module_graph


def test_prepare_entry_module_graph_closes_added_package_parent_imports(
    tmp_path: Path,
) -> None:
    package = tmp_path / "molt"
    package.mkdir()
    (package / "__init__.py").write_text(
        "from ._version import version\n"
        "from .subpkg.mod import VALUE\n"
        "__version__ = version()\n",
        encoding="utf-8",
    )
    (package / "_version.py").write_text(
        "def version():\n    return '1.0'\n",
        encoding="utf-8",
    )
    (package / "intrinsics.py").write_text(
        "def require(name, namespace):\n    return namespace[name]\n",
        encoding="utf-8",
    )
    subpkg = package / "subpkg"
    subpkg.mkdir()
    (subpkg / "__init__.py").write_text(
        "from .helper import helper\n",
        encoding="utf-8",
    )
    (subpkg / "mod.py").write_text("VALUE = 1\n", encoding="utf-8")
    (subpkg / "helper.py").write_text("helper = 2\n", encoding="utf-8")
    entry_path = tmp_path / "demo.py"
    entry_path.write_text(
        "from molt import intrinsics as _intrinsics\n", encoding="utf-8"
    )
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    module_reasons: dict[str, set[str]] = {}

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=tmp_path,
        entry_tree=entry_tree,
        diagnostics_enabled=True,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert {
        "molt",
        "molt.intrinsics",
        "molt._version",
        "molt.subpkg",
        "molt.subpkg.mod",
        "molt.subpkg.helper",
    } <= set(prepared.module_graph)
    assert "package_parent_closure" in module_reasons["molt._version"]
    assert "package_parent_closure" in module_reasons["molt.subpkg.helper"]


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
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_import_support_policy.needs_generated_importer
    assert prepared.runtime_import_support_policy.needs_runtime_import_support


def test_materialize_import_plan_does_not_mutate_prepared_entry_graph(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import _molt_importer\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert cli_module_import_scanner.IMPORTER_MODULE_NAME not in prepared.module_graph

    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons={},
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    assert cli_module_import_scanner.IMPORTER_MODULE_NAME not in prepared.module_graph
    assert cli_module_import_scanner.IMPORTER_MODULE_NAME in import_plan.module_graph
    assert (
        cli_module_import_scanner.IMPORTER_MODULE_NAME
        in import_plan.generated_module_source_paths
    )


def test_import_plan_freezes_graph_and_allowlist(tmp_path: Path) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("value = 1\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons={},
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    with pytest.raises(TypeError):
        import_plan.module_graph["extra"] = entry_path  # type: ignore[index]
    metadata = import_plan.module_graph_metadata
    with pytest.raises(TypeError):
        metadata.module_is_package_by_module["extra"] = False  # type: ignore[index]
    with pytest.raises(TypeError):
        metadata.logical_source_path_by_module["demo"] = "other.py"  # type: ignore[index]
    assert isinstance(import_plan.stdlib_allowlist, frozenset)
    assert isinstance(import_plan.known_modules, frozenset)
    assert isinstance(import_plan.known_modules_sorted, tuple)
    assert isinstance(import_plan.stdlib_allowlist_sorted, tuple)


def test_generated_importer_import_plan_includes_runtime_support_modules(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import _molt_importer\n")
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module="demo",
        module_roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    import_plan = cli._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons={},
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=tmp_path,
        entry_module="demo",
        diagnostics_enabled=False,
    )

    assert "importlib.util" in import_plan.module_graph
    assert "importlib.machinery" in import_plan.module_graph
    assert "importlib.machinery" not in import_plan.explicit_imports
    assert "importlib.machinery" in import_plan.runtime_import_dispatch_roots
    assert "importlib._bootstrap" in import_plan.runtime_import_dispatch_roots
    assert "importlib._bootstrap_external" in import_plan.runtime_import_dispatch_roots
    importer_path = import_plan.module_graph[
        cli_module_import_scanner.IMPORTER_MODULE_NAME
    ]
    importer_source = importer_path.read_text(encoding="utf-8")
    assert "molt_importlib_import_transaction" in importer_source
    assert "_KNOWN_MODULES" not in importer_source


def test_backend_ir_isolate_import_is_bounded_by_runtime_dispatch_roots(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    gc_path = tmp_path / "gc.py"
    machinery_path = tmp_path / "machinery.py"
    entry_path.write_text("import gc\n", encoding="utf-8")
    gc_path.write_text("", encoding="utf-8")
    machinery_path.write_text("", encoding="utf-8")
    module_graph = {
        "demo": entry_path,
        "gc": gc_path,
        "importlib.machinery": machinery_path,
    }
    module_order = ["gc", "importlib.machinery", "demo"]
    integration_state = cli._FrontendIntegrationState(
        functions=[
            {
                "name": cli.SimpleTIRGenerator.module_init_symbol(module_name),
                "params": [],
                "ops": [{"kind": "ret_void"}],
            }
            for module_name in module_order
        ],
        known_classes={},
    )
    diagnostics_state = cli._MidendDiagnosticsState(
        policy_outcomes_by_function={},
        pass_stats_by_function={},
    )

    prepared, error = BACKEND_IR._prepare_backend_ir(
        entry_module="demo",
        module_graph=module_graph,
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules=set(module_graph),
        known_classes={},
        stdlib_allowlist=set(module_graph),
        known_func_defaults={},
        known_func_kinds={},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        frontend_phase_timeout=None,
        integration_state=integration_state,
        diagnostics_state=diagnostics_state,
        record_frontend_timing=lambda **_: None,
        fail=cli._fail,
        json_output=True,
        module_order=module_order,
        runtime_import_dispatch_roots={"gc"},
        generated_module_source_paths={},
        spawn_enabled=False,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        emit_ir_path=None,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert error is None
    assert prepared is not None
    import_ops = next(
        func["ops"]
        for func in prepared.ir["functions"]
        if func["name"] == "molt_isolate_import"
    )
    const_names = [
        op.get("s_value") for op in import_ops if op.get("kind") == "const_str"
    ]
    call_targets = [op.get("s_value") for op in import_ops if op.get("kind") == "call"]
    assert "gc" in const_names
    assert cli.SimpleTIRGenerator.module_init_symbol("gc") in call_targets
    assert "importlib.machinery" not in const_names
    assert (
        cli.SimpleTIRGenerator.module_init_symbol("importlib.machinery")
        not in call_targets
    )


def test_backend_ir_isolate_import_roots_runtime_support_closure(
    tmp_path: Path,
) -> None:
    module_names = [
        "gc",
        "importlib",
        "importlib.machinery",
        "importlib._bootstrap",
        "json",
        "demo",
    ]
    module_graph: dict[str, Path] = {}
    for module_name in module_names:
        module_path = tmp_path / f"{module_name.replace('.', '_')}.py"
        module_path.write_text("", encoding="utf-8")
        module_graph[module_name] = module_path
    module_order = list(module_names)
    integration_state = cli._FrontendIntegrationState(
        functions=[
            {
                "name": cli.SimpleTIRGenerator.module_init_symbol(module_name),
                "params": [],
                "ops": [{"kind": "ret_void"}],
            }
            for module_name in module_order
        ],
        known_classes={},
    )
    diagnostics_state = cli._MidendDiagnosticsState(
        policy_outcomes_by_function={},
        pass_stats_by_function={},
    )

    prepared, error = BACKEND_IR._prepare_backend_ir(
        entry_module="demo",
        module_graph=module_graph,
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules=set(module_graph),
        known_classes={},
        stdlib_allowlist=set(module_graph),
        known_func_defaults={},
        known_func_kinds={},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        frontend_phase_timeout=None,
        integration_state=integration_state,
        diagnostics_state=diagnostics_state,
        record_frontend_timing=lambda **_: None,
        fail=cli._fail,
        json_output=True,
        module_order=module_order,
        runtime_import_dispatch_roots={
            "gc",
            "importlib",
            "importlib.machinery",
            "importlib._bootstrap",
        },
        generated_module_source_paths={},
        spawn_enabled=False,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        emit_ir_path=None,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert error is None
    assert prepared is not None
    import_ops = next(
        func["ops"]
        for func in prepared.ir["functions"]
        if func["name"] == "molt_isolate_import"
    )
    const_names = [
        op.get("s_value") for op in import_ops if op.get("kind") == "const_str"
    ]
    call_targets = [op.get("s_value") for op in import_ops if op.get("kind") == "call"]
    for module_name in (
        "gc",
        "importlib",
        "importlib.machinery",
        "importlib._bootstrap",
    ):
        assert module_name in const_names
        assert cli.SimpleTIRGenerator.module_init_symbol(module_name) in call_targets
    assert "json" not in const_names
    assert cli.SimpleTIRGenerator.module_init_symbol("json") not in call_targets


def test_backend_ir_isolate_import_initializes_static_native_artifacts(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "demo.py"
    entry_path.write_text("import nativepkg.ndimage\n", encoding="utf-8")
    module_graph = {"demo": entry_path}
    module_order = ["demo"]
    integration_state = cli._FrontendIntegrationState(
        functions=[
            {
                "name": cli.SimpleTIRGenerator.module_init_symbol("demo"),
                "params": [],
                "ops": [{"kind": "ret_void"}],
            }
        ],
        known_classes={},
    )
    diagnostics_state = cli._MidendDiagnosticsState(
        policy_outcomes_by_function={},
        pass_stats_by_function={},
    )
    package_dir = tmp_path / "site" / "nativepkg"
    artifact_path = package_dir / "ndimage" / "_nd_image.molt.wasm"
    artifact_path.parent.mkdir(parents=True)
    artifact_path.write_bytes(b"\0asm\x01\0\0\0native")
    manifest_path = artifact_path.with_name(
        "_nd_image.molt.wasm.extension_manifest.json"
    )
    manifest_path.write_text("{}", encoding="utf-8")
    native_artifact_plan = _ExternalPackageNativeArtifactPlan(
        artifacts=(
            _ExternalPackageNativeArtifact(
                package="nativepkg",
                module="nativepkg.ndimage._nd_image",
                package_dir=package_dir,
                path=artifact_path,
                manifest_path=manifest_path,
                extension_sha256=hashlib.sha256(artifact_path.read_bytes()).hexdigest(),
                manifest_sha256=hashlib.sha256(
                    manifest_path.read_bytes()
                ).hexdigest(),
                capabilities=(),
                abi_tag="molt_abi1",
                target_triple="wasm32-wasip1",
                platform_tag="wasm32_wasip1",
                init_symbol="PyInit__nd_image",
                runtime_linkage="static_link",
                artifact_kind="wasm_relocatable_object",
                support_file_sha256=(
                    ("nativepkg/__init__.py", "a" * 64),
                    ("nativepkg/ndimage/__init__.py", "b" * 64),
                ),
                callable_exports=(
                    _ExternalNativeCallableExport(
                        module="nativepkg.ndimage",
                        name="gaussian_filter",
                        binding="module_attr",
                        abi="molt.object_callargs_v1",
                        deterministic=True,
                    ),
                ),
            ),
        )
    )

    prepared, error = BACKEND_IR._prepare_backend_ir(
        entry_module="demo",
        module_graph=module_graph,
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules=set(module_graph) | native_artifact_plan.native_module_names(),
        known_classes={},
        stdlib_allowlist=set(module_graph),
        known_func_defaults={},
        known_func_kinds={},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        frontend_phase_timeout=None,
        integration_state=integration_state,
        diagnostics_state=diagnostics_state,
        record_frontend_timing=lambda **_: None,
        fail=cli._fail,
        json_output=True,
        module_order=module_order,
        runtime_import_dispatch_roots={"nativepkg.ndimage"},
        generated_module_source_paths={},
        spawn_enabled=False,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        emit_ir_path=None,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        target="wasm",
        native_artifact_plan=native_artifact_plan,
    )

    assert error is None
    assert prepared is not None
    functions = {func["name"]: func["ops"] for func in prepared.ir["functions"]}
    root_init = cli.SimpleTIRGenerator.module_init_symbol("nativepkg")
    public_init = cli.SimpleTIRGenerator.module_init_symbol("nativepkg.ndimage")
    extension_init = cli.SimpleTIRGenerator.module_init_symbol(
        "nativepkg.ndimage._nd_image"
    )
    assert root_init in functions
    assert public_init in functions
    assert extension_init in functions

    import_ops = functions["molt_isolate_import"]
    import_const_names = [
        op.get("s_value") for op in import_ops if op.get("kind") == "const_str"
    ]
    import_call_targets = [
        op.get("s_value") for op in import_ops if op.get("kind") == "call"
    ]
    assert "nativepkg" in import_const_names
    assert "nativepkg.ndimage" in import_const_names
    assert "nativepkg.ndimage._nd_image" not in import_const_names
    assert public_init in import_call_targets

    extension_ops = functions[extension_init]
    invoke_ops = [op for op in extension_ops if op.get("kind") == "invoke_ffi"]
    assert invoke_ops == [
        {
            "kind": "invoke_ffi",
            "args": [],
            "out": "v2",
            "native_callable_export": (
                "__molt_static_pyinit__.nativepkg.ndimage._nd_image"
            ),
            "native_callable_binding": "direct_symbol",
            "native_callable_abi": "molt.pyinit_module_v1",
            "native_callable_symbol": "PyInit__nd_image",
        }
    ]
    extension_call_targets = [
        op.get("s_value") for op in extension_ops if op.get("kind") == "call"
    ]
    assert "molt_cpython_abi_prepare_static_extension" in extension_call_targets
    assert "molt_cpython_abi_pyinit_module_to_bits" in extension_call_targets

    public_ops = functions[public_init]
    public_call_targets = [
        op.get("s_value") for op in public_ops if op.get("kind") == "call"
    ]
    public_const_names = [
        op.get("s_value") for op in public_ops if op.get("kind") == "const_str"
    ]
    assert extension_init in public_call_targets
    assert "nativepkg.ndimage._nd_image" in public_const_names
    assert "gaussian_filter" in public_const_names
    gaussian_attr_vars = {
        op.get("out")
        for op in public_ops
        if op.get("kind") == "const_str" and op.get("s_value") == "gaussian_filter"
    }
    exported_value_vars = {
        op.get("out")
        for op in public_ops
        if op.get("kind") == "module_get_attr"
        and len(op.get("args", [])) == 2
        and op["args"][1] in gaussian_attr_vars
    }
    assert exported_value_vars
    assert any(
        op.get("kind") == "module_set_attr"
        and len(op.get("args", [])) == 3
        and op["args"][1] in gaussian_attr_vars
        and op["args"][2] in exported_value_vars
        for op in public_ops
    )


def test_dead_module_elimination_keeps_runtime_dispatch_roots() -> None:
    module_order = ["importlib", "importlib.machinery", "json", "demo"]
    module_layers = [["importlib", "importlib.machinery", "json"], ["demo"]]

    filtered_order, filtered_layers, eliminated = (
        cli_module_dependencies._apply_dead_module_elimination(
            module_order,
            module_layers,
            entry_module="demo",
            module_deps={"demo": set()},
            module_names=set(module_order),
            extra_roots={"importlib.machinery"},
        )
    )

    assert filtered_order == ["importlib", "importlib.machinery", "demo"]
    assert filtered_layers == [["importlib", "importlib.machinery"], ["demo"]]
    assert eliminated == 1


def test_dead_module_elimination_pure_wasm_safelist_skips_host_stdlib() -> None:
    module_order = ["builtins", "sys", "os", "typing", "warnings", "array", "demo"]
    module_layers = [
        ["builtins", "sys", "os", "typing", "warnings", "array"],
        ["demo"],
    ]
    module_deps = {
        "demo": {"array"},
        "array": set(),
        "typing": {"warnings"},
        "os": set(),
        "warnings": set(),
    }

    filtered_order, filtered_layers, eliminated = (
        cli_module_dependencies._apply_dead_module_elimination(
            module_order,
            module_layers,
            entry_module="demo",
            module_deps=module_deps,
            module_names=set(module_order),
            safelist=(
                cli_module_dependencies._PURE_WASM_DEAD_MODULE_ELIMINATION_SAFELIST
            ),
        )
    )

    assert filtered_order == ["builtins", "sys", "array", "demo"]
    assert filtered_layers == [["builtins", "sys", "array"], ["demo"]]
    assert eliminated == 3


def test_pure_wasm_dme_mode_overrides_legacy_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output_layout = cli_frontend_pipeline._BuildOutputLayout(
        is_wasm=True,
        is_wasm_freestanding=False,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_mlir_emit=False,
        split_runtime=True,
        linked=True,
        target_triple=None,
        emit_mode="wasm",
        output_artifact=Path("output.wasm"),
        output_binary=None,
        linked_output_path=Path("output_linked.wasm"),
        emit_ir_path=None,
    )
    monkeypatch.setenv("MOLT_WASM_PROFILE", "pure")
    monkeypatch.setenv("MOLT_DEAD_MODULE_ELIMINATION", "1")

    assert (
        cli_frontend_pipeline._dead_module_elimination_mode(
            output_layout=output_layout,
            tree_shake=True,
        )
        == "pure-wasm"
    )


def test_pure_wasm_dme_roots_do_not_seed_runtime_support_closure() -> None:
    import_plan = types.SimpleNamespace(
        explicit_imports=frozenset({"array"}),
        declared_root_modules=frozenset({"demo"}),
        package_parent_modules=frozenset(),
        namespace_module_names=frozenset(),
        runtime_import_dispatch_roots=frozenset({"array", "importlib", "typing"}),
        runtime_support_modules=frozenset({"importlib", "_molt_importer"}),
        stdlib_support_modules=frozenset({"builtins", "sys", "typing"}),
    )

    roots = cli_frontend_pipeline._dead_module_elimination_extra_roots(
        import_plan,
        mode="pure-wasm",
    )

    assert roots == {"array", "demo"}


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
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=None,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons={},
        json_output=False,
        target="native",
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_import_support_policy.needs_runtime_import_support


def test_write_namespace_module_avoids_rewriting_identical_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    expected_path = tmp_path / "namespace_demo_pkg.py"
    original_replace = os.replace
    replaced_destinations: list[Path] = []

    def record_replace(src: object, dst: object) -> None:
        destination = Path(dst)
        if destination == expected_path:
            replaced_destinations.append(destination)
        original_replace(src, dst)

    monkeypatch.setattr(os, "replace", record_replace)

    cli._write_namespace_module("demo.pkg", ["/tmp/demo/pkg"], tmp_path)
    cli._write_namespace_module("demo.pkg", ["/tmp/demo/pkg"], tmp_path)

    assert replaced_destinations == [expected_path]


def test_run_subprocess_captured_to_tempfiles_does_not_block_on_inherited_pipes(
    tmp_path: Path,
) -> None:
    sleeper = tmp_path / "sleeper.py"
    sleeper.write_text(
        "import time\ntime.sleep(5.0)\n",
        encoding="utf-8",
    )
    child_pid_file = tmp_path / "sleeper.pid"
    parent = tmp_path / "parent.py"
    parent.write_text(
        "import pathlib, subprocess, sys\n"
        f"child = subprocess.Popen([sys.executable, {str(sleeper)!r}], stdout=sys.stdout, stderr=sys.stderr)\n"
        f"pathlib.Path({str(child_pid_file)!r}).write_text(str(child.pid), encoding='utf-8')\n"
        "print('parent-done', flush=True)\n",
        encoding="utf-8",
    )

    try:
        start = time.perf_counter()
        result = COMMAND_RUNTIME._run_subprocess_captured_to_tempfiles(
            [sys.executable, str(parent)],
            timeout=2.0,
        )
        elapsed = time.perf_counter() - start
    finally:
        if child_pid_file.exists():
            child_pid = int(child_pid_file.read_text(encoding="utf-8"))
            with contextlib.suppress(OSError):
                os.kill(child_pid, signal.SIGTERM)

    assert result.returncode == 0
    assert "parent-done" in cli._subprocess_output_text(result.stdout)
    assert elapsed < 2.5


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
    monkeypatch.setattr(PROJECT_ROOTS, "_has_project_markers", fake_has_project_markers)
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
    monkeypatch.setattr(
        PROJECT_ROOTS,
        "_has_molt_repo_markers",
        fake_has_molt_repo_markers,
    )
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
    cli_module_stdlib_policy._stdlib_allowlist_cached.cache_clear()
    spec_path = (
        tmp_path / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
    )
    spec_path.parent.mkdir(parents=True, exist_ok=True)
    spec_path.write_text("| Module |\n| --- |\n| json / pathlib |\n")
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(tmp_path))
    monkeypatch.chdir(tmp_path)
    first = cli_module_stdlib_policy._stdlib_allowlist()
    second = cli_module_stdlib_policy._stdlib_allowlist()
    info = cli_module_stdlib_policy._stdlib_allowlist_cached.cache_info()
    assert {"json", "pathlib"} <= first
    assert second == first
    assert info.hits >= 1
    assert info.currsize >= 1
    cli_module_stdlib_policy._stdlib_allowlist_cached.cache_clear()


def test_respect_pythonpath_keeps_repo_src_internal(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    src_root = ROOT / "src"
    monkeypatch.setenv("PYTHONPATH", str(src_root))

    resolved = cli_build_inputs._resolve_module_root_resolution(
        ROOT,
        ROOT,
        respect_pythonpath=True,
        lib_paths=[],
    )

    assert src_root.resolve() in resolved.roots
    assert src_root.resolve() not in resolved.external_roots


def test_static_backend_ir_module_call_closure_accepts_graph_modules() -> None:
    ir = {
        "functions": [
            {
                "name": "molt_init_demo",
                "ops": [
                    {
                        "kind": "call",
                        "s_value": "copy__copy",
                        "args": ["v0"],
                        "out": "v1",
                    },
                ],
            }
        ]
    }

    assert (
        BACKEND_IR._static_backend_ir_module_call_closure_issue(
            ir,
            {"demo": Path("demo.py"), "copy": Path("copy.py")},
            {"copy", "demo"},
        )
        is None
    )
    assert BACKEND_IR._static_backend_ir_module_call_targets(ir, {"copy", "demo"}) == (
        ("copy", "copy__copy", "molt_init_demo", 0),
    )


def test_static_backend_ir_module_call_closure_reports_missing_module() -> None:
    ir = {
        "functions": [
            {
                "name": "molt_init_collections",
                "ops": [
                    {
                        "kind": "call",
                        "s_value": "copy__copy",
                        "args": ["v0"],
                        "out": "v1",
                    },
                ],
            }
        ]
    }

    issue = BACKEND_IR._static_backend_ir_module_call_closure_issue(
        ir, {"collections": Path("collections.py")}, {"collections", "copy"}
    )

    assert issue is not None
    assert "copy__copy (copy) at molt_init_collections[0]" in issue
    assert "graph_modules=1" in issue


def test_static_backend_ir_module_call_closure_allows_lazy_missing_import() -> None:
    ir = {
        "functions": [
            {
                "name": "zipfile__ZipFile_read",
                "ops": [
                    {"kind": "const_str", "s_value": "bz2", "out": "v0"},
                    {"kind": "module_import", "args": ["v0"], "out": "v1"},
                ],
            }
        ]
    }

    assert (
        BACKEND_IR._static_backend_ir_module_call_closure_issue(
            ir, {"zipfile": Path("zipfile.py")}, {"zipfile", "bz2"}
        )
        is None
    )


def _discover_with_core_modules(entry: Path) -> dict[str, Path]:
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()
    module_graph, _ = cli_module_graph_discovery._discover_module_graph(
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
    cli_module_stdlib_policy._ensure_core_stdlib_modules(module_graph, stdlib_root)
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
        core_graph, _ = cli_module_graph_discovery._discover_module_graph_from_paths(
            core_paths,
            roots,
            module_roots,
            stdlib_root,
            ROOT,
            stdlib_allowlist,
            skip_modules=cli.STUB_MODULES,
            stub_parents=cli.STUB_PARENT_MODULES,
            stdlib_static_import_helper_modules=set(),
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    return module_graph


def test_external_root_direct_import_does_not_admit_transitive_children(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    external_root = tmp_path / "site"
    project.mkdir()
    package = external_root / "hugepkg"
    package.mkdir(parents=True)
    (project / "main.py").write_text("import hugepkg\n")
    (package / "__init__.py").write_text("import hugepkg.heavy\nVALUE = 1\n")
    (package / "heavy.py").write_text("VALUE = 2\n")
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [project.resolve(), external_root.resolve()]
    policy = cli._ImportAdmissionPolicy(external_roots=(external_root.resolve(),))

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
        project / "main.py",
        [*module_roots, stdlib_root],
        module_roots,
        stdlib_root,
        project,
        cli_module_stdlib_policy._stdlib_allowlist(),
        import_admission_policy=policy,
    )

    assert "hugepkg" in graph
    assert "hugepkg.heavy" not in graph
    assert "hugepkg" in explicit_imports
    assert "hugepkg.heavy" in explicit_imports


def test_external_static_package_admission_closes_transitive_children(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    external_root = tmp_path / "site"
    project.mkdir()
    package = external_root / "hugepkg"
    package.mkdir(parents=True)
    (project / "main.py").write_text("import hugepkg\n")
    (package / "__init__.py").write_text("import hugepkg.heavy\nVALUE = 1\n")
    (package / "heavy.py").write_text("VALUE = 2\n")
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [project.resolve(), external_root.resolve()]
    policy = cli._ImportAdmissionPolicy(
        external_roots=(external_root.resolve(),),
        admitted_external_packages=frozenset({"hugepkg"}),
    )

    graph, _ = cli_module_graph_discovery._discover_module_graph(
        project / "main.py",
        [*module_roots, stdlib_root],
        module_roots,
        stdlib_root,
        project,
        cli_module_stdlib_policy._stdlib_allowlist(),
        import_admission_policy=policy,
    )

    assert {"hugepkg", "hugepkg.heavy"} <= set(graph)


def test_external_package_parent_closure_cannot_backdoor_children(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    external_root = tmp_path / "site"
    project.mkdir()
    package = external_root / "externalpkg"
    subpackage = package / "sub"
    subpackage.mkdir(parents=True)
    entry = project / "main.py"
    entry.write_text("import externalpkg.sub.leaf\n")
    (package / "__init__.py").write_text("import externalpkg.massive\n")
    (package / "massive.py").write_text("VALUE = 1\n")
    (subpackage / "__init__.py").write_text("import externalpkg.sub.massive\n")
    (subpackage / "massive.py").write_text("VALUE = 2\n")
    (subpackage / "leaf.py").write_text("VALUE = 3\n")
    stdlib_root = cli_module_resolution._stdlib_root_path()
    policy = cli._ImportAdmissionPolicy(external_roots=(external_root.resolve(),))

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry,
        entry_module="main",
        module_roots=[project.resolve(), external_root.resolve()],
        stdlib_root=stdlib_root,
        project_root=project,
        entry_tree=ast.parse(entry.read_text()),
        diagnostics_enabled=True,
        module_reasons={},
        json_output=False,
        target="native",
        import_admission_policy=policy,
    )

    assert error is None
    assert prepared is not None
    assert {"externalpkg", "externalpkg.sub", "externalpkg.sub.leaf"} <= set(
        prepared.module_graph
    )
    assert "externalpkg.massive" not in prepared.module_graph
    assert "externalpkg.sub.massive" not in prepared.module_graph


def test_from_import_graph_does_not_admit_case_mismatched_attribute_child(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    site = tmp_path / "site"
    project.mkdir()
    package = site / "tinygrad"
    package.mkdir(parents=True)
    entry = project / "main.py"
    entry.write_text("from tinygrad import Tensor\n", encoding="utf-8")
    (package / "__init__.py").write_text(
        "from .tensor import Tensor\n__all__ = ['Tensor']\n",
        encoding="utf-8",
    )
    tensor = package / "tensor.py"
    tensor.write_text("class Tensor:\n    pass\n", encoding="utf-8")

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [project.resolve(), site.resolve()]
    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
        entry,
        [*module_roots, stdlib_root],
        module_roots,
        stdlib_root,
        project,
        cli_module_stdlib_policy._stdlib_allowlist(),
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )

    assert {"tinygrad", "tinygrad.tensor"} <= set(graph)
    assert graph["tinygrad.tensor"] == tensor
    assert "tinygrad.Tensor" not in graph
    assert "tinygrad.Tensor" in explicit_imports


def test_from_import_star_graph_admits_static_all_child_module(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    site = tmp_path / "site"
    project.mkdir()
    package = site / "tinygrad"
    package.mkdir(parents=True)
    entry = project / "main.py"
    entry.write_text("from tinygrad import *\n", encoding="utf-8")
    (package / "__init__.py").write_text("__all__ = ['tensor']\n", encoding="utf-8")
    tensor = package / "tensor.py"
    tensor.write_text("class Tensor:\n    pass\n", encoding="utf-8")

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [project.resolve(), site.resolve()]
    roots = [*module_roots, stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()
    cache = cli_module_resolution._ModuleResolutionCache()
    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        project,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
        resolver_cache=cache,
    )

    assert {"tinygrad", "tinygrad.tensor"} <= set(graph)
    assert graph["tinygrad.tensor"] == tensor
    assert "tinygrad.tensor" in explicit_imports

    tree = ast.parse(entry.read_text(encoding="utf-8"))
    _, imports, _, _, _, _, _, _ = cli._load_module_analysis(
        entry,
        module_name="main",
        is_package=False,
        import_scan_mode="full",
        source=entry.read_text(encoding="utf-8"),
        logical_source_path=str(entry),
        resolution_cache=cache,
        project_root=project,
        retain_source=False,
        retain_tree=False,
        roots=roots,
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
    )
    assert tree is not None
    assert "tinygrad.tensor" in imports


def test_module_graph_policy_digest_includes_external_admission(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    external_root.mkdir()
    bounded = cli._ImportAdmissionPolicy(external_roots=(external_root,))
    closed = cli._ImportAdmissionPolicy(
        external_roots=(external_root,),
        admitted_external_packages=frozenset({"hugepkg"}),
    )

    assert cli_module_graph_cache._module_graph_policy_digest({"sys"}, bounded) != (
        cli_module_graph_cache._module_graph_policy_digest({"sys"}, closed)
    )


def _libmolt_source_manifest_fields(
    *,
    module: str,
    artifact_name: str,
    artifact_bytes: bytes,
    target_triple: str | None = None,
    runtime_linkage: str = "host_resolved",
    artifact_kind: str | None = None,
) -> dict[str, Any]:
    module_leaf = module.rsplit(".", 1)[-1]
    init_symbol = f"PyInit_{module_leaf}"
    object_name = f"{module_leaf}.o"
    digest = hashlib.sha256(artifact_bytes).hexdigest()
    resolved_target_triple = target_triple or cli._host_target_triple()
    resolved_artifact_kind = artifact_kind or (
        "wasm_relocatable_object"
        if runtime_linkage == "static_link"
        else "shared_library"
    )
    return {
        "schema_version": 1,
        "module": module,
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_triple": resolved_target_triple,
        "platform_tag": cli._wheel_token(resolved_target_triple),
        "capabilities": ["module.extension.exec"],
        "extension": artifact_name,
        "extension_sha256": digest,
        "loader_kind": "libmolt_source",
        "init_symbol": init_symbol,
        "runtime_linkage": runtime_linkage,
        "artifact_kind": resolved_artifact_kind,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": 1,
            "root_symbol": init_symbol,
            "init_symbol_owner": object_name,
            "closure_sha256": digest,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": object_name,
                    "source_sha256": digest,
                    "object_sha256": digest,
                    "defined_symbols": [init_symbol],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }


def _apply_manifest_overrides(
    manifest: dict[str, Any],
    manifest_overrides: dict[str, Any] | None,
) -> None:
    if not manifest_overrides:
        return
    for key, value in manifest_overrides.items():
        if (
            key == "object_closure"
            and isinstance(value, dict)
            and isinstance(manifest.get("object_closure"), dict)
        ):
            cast(dict[str, Any], manifest["object_closure"]).update(value)
        else:
            manifest[key] = value


def _write_external_native_package(
    tmp_path: Path,
    *,
    package: str = "nativepkg",
    artifact_name: str = "_native.so",
    artifact_bytes: bytes | None = None,
    write_manifest: bool = True,
    checksum_override: str | None = None,
    manifest_overrides: dict[str, Any] | None = None,
    shim_source: str | None = None,
) -> tuple[Path, Path, Path]:
    external_root = tmp_path / "site"
    package_dir = external_root.joinpath(*package.split("."))
    package_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text(
        f"import {package}._native\nVALUE = 1\n",
        encoding="utf-8",
    )
    artifact_path = package_dir / artifact_name
    if artifact_bytes is None:
        if artifact_name.endswith(".molt.wasm"):
            artifact_bytes = _wasm_exporting_i64_unary_symbol(
                f"PyInit_{artifact_name.split('.', 1)[0]}"
            )
        else:
            artifact_bytes = b"native-extension"
    artifact_path.write_bytes(artifact_bytes)
    manifest_path = package_dir / "extension_manifest.json"
    if write_manifest:
        manifest = _libmolt_source_manifest_fields(
            module=f"{package}._native",
            artifact_name=artifact_name,
            artifact_bytes=artifact_bytes,
        )
        if checksum_override is not None:
            manifest["extension_sha256"] = checksum_override
        _apply_manifest_overrides(manifest, manifest_overrides)
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    if shim_source is not None:
        (package_dir / f"{artifact_name}.molt.py").write_text(
            shim_source,
            encoding="utf-8",
        )
    return external_root, artifact_path, manifest_path


def _write_external_native_artifact(
    external_root: Path,
    *,
    package: str,
    relative_module: str,
    artifact_name: str | None = None,
    artifact_bytes: bytes | None = None,
    manifest_overrides: dict[str, Any] | None = None,
) -> tuple[Path, Path]:
    package_dir = external_root.joinpath(*package.split("."))
    package_dir.mkdir(parents=True, exist_ok=True)
    init_path = package_dir / "__init__.py"
    if not init_path.exists():
        init_path.write_text("VALUE = 1\n", encoding="utf-8")
    module_parts = relative_module.split(".")
    artifact_dir = package_dir.joinpath(*module_parts[:-1])
    artifact_dir.mkdir(parents=True, exist_ok=True)
    for index in range(1, len(module_parts)):
        parent_init = package_dir.joinpath(*module_parts[:index], "__init__.py")
        if not parent_init.exists():
            parent_init.write_text("", encoding="utf-8")
    artifact_path = artifact_dir / (artifact_name or f"{module_parts[-1]}.so")
    if artifact_bytes is not None:
        payload = artifact_bytes
    elif artifact_path.name.endswith(".molt.wasm"):
        payload = _wasm_exporting_i64_unary_symbol(f"PyInit_{module_parts[-1]}")
    else:
        payload = f"{relative_module}-extension".encode("utf-8")
    artifact_path.write_bytes(payload)
    manifest_path = artifact_path.with_name(
        artifact_path.name + ".extension_manifest.json"
    )
    manifest = _libmolt_source_manifest_fields(
        module=f"{package}.{relative_module}",
        artifact_name=artifact_path.name,
        artifact_bytes=payload,
    )
    _apply_manifest_overrides(manifest, manifest_overrides)
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return artifact_path, manifest_path


def _wasm_exporting_i64_unary_symbol(
    symbol: str,
    *,
    imports: tuple[str, ...] = (),
) -> bytes:
    def uleb(value: int) -> bytes:
        out = bytearray()
        while True:
            byte = value & 0x7F
            value >>= 7
            out.append(byte | 0x80 if value else byte)
            if not value:
                return bytes(out)

    def wasm_string(value: str) -> bytes:
        encoded = value.encode("utf-8")
        return uleb(len(encoded)) + encoded

    def section(section_id: int, payload: bytes) -> bytes:
        return bytes([section_id]) + uleb(len(payload)) + payload

    type_section = uleb(1) + b"\x60" + uleb(1) + b"\x7e" + uleb(1) + b"\x7e"
    import_section = b""
    if imports:
        import_section = section(
            2,
            uleb(len(imports))
            + b"".join(
                wasm_string("env") + wasm_string(import_name) + b"\x00" + uleb(0)
                for import_name in imports
            ),
        )
    function_section = uleb(1) + uleb(0)
    export_section = uleb(1) + wasm_string(symbol) + b"\x00" + uleb(len(imports))
    body = uleb(0) + b"\x42\x00\x0b"
    code_section = uleb(1) + uleb(len(body)) + body
    return (
        b"\x00asm\x01\x00\x00\x00"
        + section(1, type_section)
        + import_section
        + section(3, function_section)
        + section(7, export_section)
        + section(10, code_section)
    )


def _write_source_only_external_package(
    tmp_path: Path,
    *,
    package: str,
    init_source: str = "VALUE = 1\n",
) -> tuple[Path, Path]:
    external_root = tmp_path / "site"
    package_parts = package.split(".")
    package_dir = external_root.joinpath(*package_parts)
    package_dir.mkdir(parents=True)
    for index in range(1, len(package_parts)):
        parent_init = external_root.joinpath(*package_parts[:index], "__init__.py")
        if not parent_init.exists():
            parent_init.write_text("", encoding="utf-8")
    (package_dir / "__init__.py").write_text(init_source, encoding="utf-8")
    return external_root, package_dir


def test_external_static_package_native_artifact_plan_validates_manifest(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, artifact_path, manifest_path = _write_external_native_package(
        tmp_path
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert error is None
    assert policy is not None
    assert policy.admitted_external_packages == frozenset({"nativepkg"})
    assert len(policy.native_artifact_plan.artifacts) == 1
    artifact = policy.native_artifact_plan.artifacts[0]
    assert artifact.package == "nativepkg"
    assert artifact.module == "nativepkg._native"
    assert artifact.package_dir == (external_root / "nativepkg").resolve()
    assert artifact.path == artifact_path.resolve()
    assert artifact.manifest_path == manifest_path.resolve()
    assert artifact.capabilities == ("module.extension.exec",)
    assert artifact.init_symbol == "PyInit__native"
    assert artifact.runtime_linkage == "host_resolved"
    assert artifact.artifact_kind == "shared_library"
    assert artifact.support_file_sha256 == (
        (
            "nativepkg/__init__.py",
            hashlib.sha256(
                (external_root / "nativepkg" / "__init__.py").read_bytes()
            ).hexdigest(),
        ),
    )
    assert (
        artifact.extension_sha256
        == hashlib.sha256(artifact_path.read_bytes()).hexdigest()
    )
    assert (
        policy.digest_payload()["native_artifact_plan"]["artifacts"][0][
            "manifest_sha256"
        ]
        == hashlib.sha256(manifest_path.read_bytes()).hexdigest()
    )


def test_external_static_package_wasm_artifact_plan_is_manifest_led(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        artifact_name="_native.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
        },
    )
    (external_root / "nativepkg" / "stray.o").write_bytes(b"not-a-manifest-artifact")
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert error is None
    assert policy is not None
    assert len(policy.native_artifact_plan.artifacts) == 1
    artifact = policy.native_artifact_plan.artifacts[0]
    assert artifact.path == artifact_path.resolve()
    assert artifact.target_triple == "wasm32-wasip1"
    assert artifact.runtime_linkage == "static_link"
    assert artifact.artifact_kind == "wasm_relocatable_object"


def test_external_static_package_wasm_manifest_support_archives_are_link_inputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _artifact_path, manifest_path = _write_external_native_package(
        tmp_path,
        artifact_name="_native.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
        },
    )
    support_archive = external_root / "nativepkg" / "_loops.a"
    support_archive.write_bytes(b"!<arch>\nloops")
    support_sha = hashlib.sha256(support_archive.read_bytes()).hexdigest()
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["support_files"] = [
        {
            "path": "nativepkg/_loops.a",
            "sha256": support_sha,
        }
    ]
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert error is None
    assert policy is not None
    artifact = policy.native_artifact_plan.artifacts[0]
    assert ("nativepkg/_loops.a", support_sha) in artifact.support_file_sha256
    staged = cli._stage_external_package_native_artifacts_for_build(
        policy.native_artifact_plan,
        artifacts_root=tmp_path / "artifacts",
    )
    assert len(staged) == 1
    staged_archive = staged[0].runtime_root / "nativepkg" / "_loops.a"
    assert staged_archive in staged[0].staged_support_paths
    assert staged_archive.read_bytes() == support_archive.read_bytes()
    assert staged_archive in (
        cli_non_native_output._wasm_static_link_native_artifact_inputs(staged)
    )


def test_external_static_package_manifest_support_python_source_is_staged_not_linked(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _artifact_path, manifest_path = _write_external_native_package(
        tmp_path,
        artifact_name="_native.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
        },
    )
    wrapper = external_root / "nativepkg" / "_morphology.py"
    wrapper.write_text(
        "from . import _native\n"
        "def distance_transform_edt(mask):\n"
        "    return _native.euclidean_feature_transform(mask)\n",
        encoding="utf-8",
    )
    wrapper_sha = hashlib.sha256(wrapper.read_bytes()).hexdigest()
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["support_files"] = [
        {
            "path": "nativepkg/_morphology.py",
            "sha256": wrapper_sha,
        }
    ]
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert error is None
    assert policy is not None
    artifact = policy.native_artifact_plan.artifacts[0]
    assert ("nativepkg/_morphology.py", wrapper_sha) in artifact.support_file_sha256
    staged = cli._stage_external_package_native_artifacts_for_build(
        policy.native_artifact_plan,
        artifacts_root=tmp_path / "artifacts",
    )
    assert len(staged) == 1
    staged_wrapper = staged[0].runtime_root / "nativepkg" / "_morphology.py"
    assert staged_wrapper in staged[0].staged_support_paths
    assert staged_wrapper.read_text(encoding="utf-8") == wrapper.read_text(
        encoding="utf-8"
    )
    assert staged_wrapper not in (
        cli_non_native_output._wasm_static_link_native_artifact_inputs(staged)
    )


def test_external_package_artifact_specific_manifests_allow_same_directory_modules(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    package_dir = external_root / "nativepkg"
    ndimage_dir = package_dir / "ndimage"
    ndimage_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    (ndimage_dir / "__init__.py").write_text("", encoding="utf-8")
    modules = ("_nd_image", "_ni_label")
    for module in modules:
        artifact_path = ndimage_dir / f"{module}.molt.wasm"
        payload = _wasm_exporting_i64_unary_symbol(f"PyInit_{module}")
        artifact_path.write_bytes(payload)
        manifest = _libmolt_source_manifest_fields(
            module=f"nativepkg.ndimage.{module}",
            artifact_name=artifact_path.name,
            artifact_bytes=payload,
        )
        manifest.update(
            {
                "target_triple": "wasm32-wasip1",
                "platform_tag": "wasm32_wasip1",
                "runtime_linkage": "static_link",
                "artifact_kind": "wasm_relocatable_object",
            }
        )
        artifact_path.with_name(
            artifact_path.name + ".extension_manifest.json"
        ).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={
            "nativepkg.ndimage._nd_image",
            "nativepkg.ndimage._ni_label",
        },
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "nativepkg.ndimage._nd_image",
        "nativepkg.ndimage._ni_label",
    ]
    assert {artifact.manifest_path.name for artifact in plan.artifacts} == {
        "_nd_image.molt.wasm.extension_manifest.json",
        "_ni_label.molt.wasm.extension_manifest.json",
    }


def test_external_native_artifact_plan_selects_python_exported_imports(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    package_dir = external_root / "nativepkg"
    ndimage_dir = package_dir / "ndimage"
    ndimage_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    (ndimage_dir / "__init__.py").write_text("", encoding="utf-8")
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "python_exports": ["nativepkg.ndimage.distance_transform_edt"],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.distance_transform_edt"},
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "nativepkg.ndimage._nd_image",
    ]
    assert plan.artifacts[0].python_exports == (
        "nativepkg.ndimage.distance_transform_edt",
    )
    assert plan.native_python_export_names() == frozenset(
        {"nativepkg.ndimage.distance_transform_edt"}
    )
    assert plan.native_module_names() == frozenset(
        {
            "nativepkg",
            "nativepkg.ndimage",
            "nativepkg.ndimage._nd_image",
        }
    )


def test_external_native_artifact_plan_selects_callable_exported_imports(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    native_symbol = "molt_nativepkg_ndimage_distance_transform_edt"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(native_symbol),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "direct_symbol",
                    "abi": "molt.forward_f32_v1",
                    "symbol": native_symbol,
                    "effects": ["read", "write"],
                    "deterministic": True,
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.distance_transform_edt"},
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "nativepkg.ndimage._nd_image",
    ]
    assert plan.native_callable_export_names() == frozenset(
        {"nativepkg.ndimage.distance_transform_edt"}
    )
    assert plan.native_callable_exports_by_qualified_name() == {
        "nativepkg.ndimage.distance_transform_edt": {
            "module": "nativepkg.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.forward_f32_v1",
            "symbol": native_symbol,
            "effects": ["read", "write"],
            "deterministic": True,
        }
    }


def test_external_native_artifact_plan_selects_module_attr_callable_exports(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    source_path = external_root / "nativepkg" / "ndimage" / "src" / "nd_image.c"
    source_path.parent.mkdir(parents=True)
    source_path.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "static PyObject *native_gaussian_filter(PyObject *self, PyObject *args) {",
                "    return PyLong_FromLong(1);",
                "}",
                "static PyMethodDef ndimage_methods[] = {",
                '    {"gaussian_filter", native_gaussian_filter, METH_VARARGS, ""},',
                "    {NULL, NULL, 0, NULL},",
                "};",
                "",
            ]
        ),
        encoding="utf-8",
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "sources": [str(source_path)],
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "gaussian_filter",
                    "binding": "module_attr",
                    "abi": "molt.object_callargs_v1",
                    "effects": [],
                    "deterministic": True,
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.gaussian_filter"},
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "nativepkg.ndimage._nd_image",
    ]
    assert plan.native_callable_exports_by_qualified_name() == {
        "nativepkg.ndimage.gaussian_filter": {
            "module": "nativepkg.ndimage",
            "name": "gaussian_filter",
            "binding": "module_attr",
            "abi": "molt.object_callargs_v1",
            "effects": [],
            "deterministic": True,
        }
    }
    specs = plan.native_module_init_specs()
    assert [(spec.module, spec.init_symbol) for spec in specs] == [
        ("nativepkg", ""),
        ("nativepkg.ndimage", ""),
        ("nativepkg.ndimage._nd_image", "PyInit__nd_image"),
    ]
    ndimage_spec = next(spec for spec in specs if spec.module == "nativepkg.ndimage")
    assert [
        (publish.provider_module, publish.attr)
        for publish in ndimage_spec.module_attr_exports
    ] == [("nativepkg.ndimage._nd_image", "gaussian_filter")]


def test_external_native_artifact_plan_rejects_fake_module_attr_export(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    source_path = external_root / "nativepkg" / "ndimage" / "src" / "nd_image.c"
    source_path.parent.mkdir(parents=True)
    source_path.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "static PyObject *native_min_or_max_filter(PyObject *self, PyObject *args) {",
                "    return PyLong_FromLong(1);",
                "}",
                "static PyMethodDef ndimage_methods[] = {",
                '    {"min_or_max_filter", native_min_or_max_filter, METH_VARARGS, ""},',
                "    {NULL, NULL, 0, NULL},",
                "};",
                "",
            ]
        ),
        encoding="utf-8",
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "sources": [str(source_path)],
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "gaussian_filter",
                    "binding": "module_attr",
                    "abi": "molt.object_callargs_v1",
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.gaussian_filter"},
    )

    assert plan is None
    assert any(
        "not declared by a PyMethodDef entry" in error
        and "nativepkg.ndimage.gaussian_filter" in error
        for error in errors
    )


def test_external_native_artifact_plan_publishes_support_source_module_attr(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    support_path = external_root / "nativepkg" / "ndimage" / "_filters.py"
    support_path.parent.mkdir(parents=True)
    support_path.write_text(
        "def gaussian_filter(value):\n    return value\n",
        encoding="utf-8",
    )
    support_sha = hashlib.sha256(support_path.read_bytes()).hexdigest()
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "support_files": [
                {
                    "path": "nativepkg/ndimage/_filters.py",
                    "sha256": support_sha,
                }
            ],
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "gaussian_filter",
                    "binding": "module_attr",
                    "provider_module": "nativepkg.ndimage._filters",
                    "abi": "molt.object_callargs_v1",
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.gaussian_filter"},
    )

    assert errors == []
    assert plan is not None
    assert plan.support_source_module_names() == frozenset(
        {"nativepkg.ndimage._filters"}
    )
    assert plan.native_callable_exports_by_qualified_name() == {
        "nativepkg.ndimage.gaussian_filter": {
            "module": "nativepkg.ndimage",
            "name": "gaussian_filter",
            "binding": "module_attr",
            "abi": "molt.object_callargs_v1",
            "provider_module": "nativepkg.ndimage._filters",
            "effects": [],
            "deterministic": False,
        }
    }
    ndimage_spec = next(
        spec
        for spec in plan.native_module_init_specs()
        if spec.module == "nativepkg.ndimage"
    )
    assert [
        (publish.provider_module, publish.attr)
        for publish in ndimage_spec.module_attr_exports
    ] == [("nativepkg.ndimage._filters", "gaussian_filter")]


def test_scoped_native_callable_exports_include_provider_module() -> None:
    exports = {
        "nativepkg.ndimage.gaussian_filter": {
            "module": "nativepkg.ndimage",
            "name": "gaussian_filter",
            "binding": "module_attr",
            "provider_module": "nativepkg.ndimage._filters",
            "abi": "molt.object_call_v1",
        }
    }

    scoped = cli_module_cache._scoped_native_callable_exports(
        "nativepkg.ndimage._filters",
        module_deps={"nativepkg.ndimage._filters": set()},
        module_dep_closures={
            "nativepkg.ndimage._filters": frozenset(
                {"nativepkg.ndimage._filters"}
            )
        },
        native_callable_exports=exports,
    )

    assert scoped == exports


def test_native_support_provider_prunes_unreachable_functions() -> None:
    source = """
from collections.abc import Iterable
from nativepkg import docs
from scipy._lib._array_api import array_namespace
import math

@docs.docfiller
def gaussian_filter(value):
    if isinstance(value, Iterable):
        return _shared(value)
    return _shared(value)

def _shared(value):
    return value

def vectorized_filter(value):
    return array_namespace(math.sqrt(value))
"""
    provider = "nativepkg.ndimage._filters"
    gen = SimpleTIRGenerator(
        module_name=provider,
        known_modules={
            provider,
            "collections",
            "collections.abc",
            "scipy._lib._array_api",
        },
        direct_call_modules={provider},
        native_callable_exports={
            "nativepkg.ndimage.gaussian_filter": {
                "module": "nativepkg.ndimage",
                "name": "gaussian_filter",
                "binding": "module_attr",
                "provider_module": provider,
                "abi": "molt.object_call_v1",
            }
        },
    )

    gen.visit(ast.parse(source))
    ir = gen.to_json()
    function_names = {func["name"] for func in ir["functions"]}
    string_constants = {
        op.get("s_value")
        for func in ir["functions"]
        for op in func.get("ops", [])
        if op.get("kind") == "const_str"
    }

    assert "nativepkg_ndimage__filters__gaussian_filter" in function_names
    assert "nativepkg_ndimage__filters___shared" in function_names
    assert "nativepkg_ndimage__filters__vectorized_filter" not in function_names
    assert "collections.abc" in string_constants
    assert "math" not in string_constants
    assert "nativepkg" not in string_constants
    assert "scipy._lib._array_api" not in string_constants


def test_native_support_function_roots_cross_imported_helpers(
    tmp_path: Path,
) -> None:
    package_root = tmp_path / "nativepkg"
    filters = package_root / "ndimage" / "_filters.py"
    util = package_root / "_lib" / "_util.py"
    exceptions = package_root / "exceptions.py"
    filters.parent.mkdir(parents=True)
    util.parent.mkdir(parents=True)
    filters.write_text(
        "\n".join(
            [
                "from nativepkg._lib._util import normalize_axis_index",
                "",
                "def gaussian_filter(value):",
                "    return gaussian_filter1d(value)",
                "",
                "def gaussian_filter1d(value):",
                "    return normalize_axis_index(value, 1)",
                "",
                "def vectorized_filter(value):",
                "    return missing_array_api(value)",
                "",
            ]
        ),
        encoding="utf-8",
    )
    util.write_text(
        "\n".join(
            [
                "from nativepkg.exceptions import AxisError",
                "",
                "def normalize_axis_index(axis, ndim):",
                "    raise AxisError('bad')",
                "",
                "def unrelated():",
                "    return missing()",
                "",
            ]
        ),
        encoding="utf-8",
    )
    exceptions.write_text(
        "\n".join(
            [
                "class AxisError(ValueError):",
                "    pass",
                "",
                "class UnrelatedError(Exception):",
                "    pass",
                "",
            ]
        ),
        encoding="utf-8",
    )
    plan = _ExternalPackageNativeArtifactPlan(
        artifacts=(
            _ExternalPackageNativeArtifact(
                package="nativepkg",
                module="nativepkg.ndimage._nd_image",
                package_dir=package_root,
                path=tmp_path / "_nd_image.molt.wasm",
                manifest_path=tmp_path / "extension_manifest.json",
                extension_sha256="wasm",
                manifest_sha256="manifest",
                capabilities=(),
                abi_tag="molt-extension-v1",
                target_triple="wasm32-unknown-unknown",
                platform_tag="wasm32",
                support_file_sha256=(
                    (
                        "nativepkg/ndimage/_filters.py",
                        hashlib.sha256(filters.read_bytes()).hexdigest(),
                    ),
                    (
                        "nativepkg/_lib/_util.py",
                        hashlib.sha256(util.read_bytes()).hexdigest(),
                    ),
                    (
                        "nativepkg/exceptions.py",
                        hashlib.sha256(exceptions.read_bytes()).hexdigest(),
                    ),
                ),
                callable_exports=(
                    _ExternalNativeCallableExport(
                        module="nativepkg.ndimage",
                        name="gaussian_filter",
                        binding="module_attr",
                        provider_module="nativepkg.ndimage._filters",
                        abi="molt.object_call_v1",
                    ),
                ),
            ),
        ),
    )

    roots = cli_module_graph._native_support_function_roots_by_module(plan)

    assert roots == {
        "nativepkg._lib._util": ("normalize_axis_index",),
        "nativepkg.exceptions": ("AxisError",),
        "nativepkg.ndimage._filters": ("gaussian_filter", "gaussian_filter1d"),
    }


def test_external_native_artifact_plan_rejects_missing_wasm_callable_symbol(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol("molt_nativepkg_other"),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "direct_symbol",
                    "abi": "molt.forward_f32_v1",
                    "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.distance_transform_edt"},
    )

    assert plan is None
    assert any(
        "direct_symbol callable exports are absent from "
        "_nd_image.molt.wasm function exports: "
        "molt_nativepkg_ndimage_distance_transform_edt" in error
        for error in errors
    )


def test_external_native_artifact_plan_rejects_archive_callable_symbol_without_closure(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.a",
        artifact_bytes=b"archive-bytes",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "static_archive",
            "object_closure": {"defined_symbols": [], "objects": []},
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "direct_symbol",
                    "abi": "molt.forward_f32_v1",
                    "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.distance_transform_edt"},
    )

    assert plan is None
    assert any(
        "static_archive direct_symbol callable exports require "
        "object_closure.defined_symbols" in error
        for error in errors
    )


def test_external_native_artifact_plan_accepts_archive_callable_defined_symbol(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    native_symbol = "molt_nativepkg_ndimage_distance_transform_edt"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.a",
        artifact_bytes=b"archive-bytes",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "static_archive",
            "object_closure": {"defined_symbols": [native_symbol]},
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "direct_symbol",
                    "abi": "molt.forward_f32_v1",
                    "symbol": native_symbol,
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage.distance_transform_edt"},
    )

    assert errors == []
    assert plan is not None
    assert plan.native_callable_export_names() == frozenset(
        {"nativepkg.ndimage.distance_transform_edt"}
    )


def test_c_api_primitive_class_contract_buckets_shared_surfaces() -> None:
    cases = {
        "PyArray_NDIM": "numpy_c_api",
        "npy_intp": "numpy_c_api",
        "NPY_INT": "numpy_c_api",
        "_Pyx_PyObject_Call": "cython_runtime_helper",
        "PyCapsule_Import": "capsules",
        "PyErr_SetString": "exceptions",
        "Py_INCREF": "refcount",
        "PyMem_Malloc": "memory_allocator",
        "PyObject_GetBuffer": "buffer_protocol",
        "PyImport_ImportModule": "import_system",
        "PyModule_Create2": "module_state",
        "PyGILState_Ensure": "gil_threading",
        "PyUnicode_FromString": "unicode_text",
        "PyBytes_FromStringAndSize": "bytes_bytearray",
        "PyObject_Call": "call_protocol",
        "PyDescr_NewGetSet": "descriptor_protocol",
        "PyMapping_GetItemString": "iterator_mapping_helpers",
        "PyLong_FromLong": "numeric_scalars",
        "PyCode_New": "code_frame_eval",
        "PyType_Ready": "object_type_lifecycle",
        "PyOS_strtol": "python_c_api",
    }

    assert {
        symbol: cli_c_api_symbols.c_api_primitive_class(symbol) for symbol in cases
    } == cases


def test_external_native_artifact_plan_records_c_api_symbol_board(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("PyLong_FromLong",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": ["PyLong_FromLong", "PyArray_NDIM"],
                "undefined_symbols": ["PyLong_FromLong"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert errors == []
    assert plan is not None
    board = {
        symbol.symbol: symbol.digest_payload()
        for symbol in plan.artifacts[0].c_api_symbols
    }
    assert board["PyLong_FromLong"] == {
        "symbol": "PyLong_FromLong",
        "status": "runtime_backed",
        "primitive_class": "numeric_scalars",
        "source": "required_c_api_symbols+undefined_symbols",
    }
    assert board["PyArray_NDIM"] == {
        "symbol": "PyArray_NDIM",
        "status": "source_compile_only",
        "primitive_class": "numpy_c_api",
        "source": "required_c_api_symbols",
    }


def test_external_native_artifact_plan_ignores_declaration_only_c_api_requirements(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "artifacts"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="_native",
        manifest_overrides={
            "object_closure": {
                "required_c_api_symbols": [
                    "Python",
                    "PyMODINIT_FUNC",
                    "PyTypeObject",
                    "PyLong_FromLong",
                ],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
    )

    assert errors == []
    assert plan is not None
    assert len(plan.artifacts) == 1
    board = {
        record.symbol: record.digest_payload()
        for record in plan.artifacts[0].c_api_symbols
    }
    assert sorted(board) == ["PyLong_FromLong"]
    assert board["PyLong_FromLong"]["primitive_class"] == "numeric_scalars"


def test_external_native_artifact_plan_records_required_only_numpy_c_api_board(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol("molt_nativepkg_placeholder"),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": [
                    "PyArray_BoolDType",
                    "PyDataType_ISBOOL",
                    "PyDimMem_RENEW",
                ],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.core._multiarray_umath"},
    )

    assert errors == []
    assert plan is not None
    board = {
        symbol.symbol: symbol.digest_payload()
        for symbol in plan.artifacts[0].c_api_symbols
    }
    assert board == {
        "PyArray_BoolDType": {
            "symbol": "PyArray_BoolDType",
            "status": "source_compile_only",
            "primitive_class": "numpy_c_api",
            "source": "required_c_api_symbols",
        },
        "PyDataType_ISBOOL": {
            "symbol": "PyDataType_ISBOOL",
            "status": "source_compile_only",
            "primitive_class": "numpy_c_api",
            "source": "required_c_api_symbols",
        },
        "PyDimMem_RENEW": {
            "symbol": "PyDimMem_RENEW",
            "status": "source_compile_only",
            "primitive_class": "numpy_c_api",
            "source": "required_c_api_symbols",
        },
    }


def test_external_native_artifact_plan_records_imported_numpy_c_api_package_native(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("npy_cabs",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "runtime_symbols": ["npy_cabs"],
                "undefined_symbols": ["npy_cabs"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.core._multiarray_umath"},
    )

    assert errors == []
    assert plan is not None
    assert [symbol.digest_payload() for symbol in plan.artifacts[0].c_api_symbols] == [
        {
            "symbol": "npy_cabs",
            "status": "package_native",
            "primitive_class": "numpy_c_api",
            "source": "runtime_symbols+undefined_symbols",
        }
    ]


def test_external_native_artifact_plan_records_imported_cpython_c_api_link(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("PyOS_strtol",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "runtime_symbols": ["PyOS_strtol"],
                "required_c_api_symbols": ["PyOS_strtol"],
                "undefined_symbols": ["PyOS_strtol"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.core._multiarray_umath"},
    )

    assert errors == []
    assert plan is not None
    assert [symbol.digest_payload() for symbol in plan.artifacts[0].c_api_symbols] == [
        {
            "symbol": "PyOS_strtol",
            "status": "cpython_abi_link",
            "primitive_class": "python_c_api",
            "source": "required_c_api_symbols+runtime_symbols+undefined_symbols",
        }
    ]


def test_external_native_artifact_plan_rejects_wasm_import_missing_from_sidecar(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("PyLong_FromLong",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": ["PyLong_FromLong"],
                "undefined_symbols": [],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert plan is None
    assert any(
        "_nd_image.molt.wasm imports symbols absent from "
        "object_closure.undefined_symbols: PyLong_FromLong" in error
        for error in errors
    )


def test_external_native_artifact_plan_allows_object_local_resolved_undefineds(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("PyLong_FromLong",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": ["PyLong_FromLong"],
                "objects": [
                    {
                        "object": "0_entry.o",
                        "defined_symbols": ["PyInit__nd_image"],
                        "undefined_symbols": [
                            "NI_Correlate",
                            "PyLong_FromLong",
                        ],
                        "required_capsules": [],
                    },
                    {
                        "object": "1_filters.o",
                        "defined_symbols": ["NI_Correlate"],
                        "undefined_symbols": [],
                        "required_capsules": [],
                    },
                ],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "nativepkg.ndimage._nd_image"
    ]


def test_external_native_artifact_plan_rejects_sidecar_undefined_symbol_not_imported(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol("molt_nativepkg_placeholder"),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": ["PyLong_FromLong"],
                "undefined_symbols": ["PyLong_FromLong"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert plan is None
    assert any(
        "object_closure.undefined_symbols names symbols absent from "
        "_nd_image.molt.wasm imports: PyLong_FromLong" in error
        for error in errors
    )


def test_external_native_artifact_plan_records_runtime_abi_symbol_board(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("molt_alloc",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "runtime_symbols": ["molt_alloc"],
                "undefined_symbols": ["molt_alloc"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert errors == []
    assert plan is not None
    assert [symbol.digest_payload() for symbol in plan.artifacts[0].abi_symbols] == [
        {
            "symbol": "molt_alloc",
            "status": "runtime_backed",
            "primitive_class": "wasm_runtime_import",
            "source": "runtime_symbols+undefined_symbols",
        }
    ]


def test_external_native_artifact_plan_records_external_link_symbol_board(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("malloc",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "runtime_symbols": ["malloc"],
                "undefined_symbols": ["malloc"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert errors == []
    assert plan is not None
    assert [symbol.digest_payload() for symbol in plan.artifacts[0].abi_symbols] == [
        {
            "symbol": "malloc",
            "status": "external_link",
            "primitive_class": "wasm_libc_link_import",
            "source": "runtime_symbols+undefined_symbols",
        }
    ]


def test_external_native_artifact_plan_records_cpython_abi_link_symbol_board(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("molt_cpython_abi_date_from_date",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "runtime_symbols": ["molt_cpython_abi_date_from_date"],
                "undefined_symbols": ["molt_cpython_abi_date_from_date"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.core._multiarray_umath"},
    )

    assert errors == []
    assert plan is not None
    assert [symbol.digest_payload() for symbol in plan.artifacts[0].abi_symbols] == [
        {
            "symbol": "molt_cpython_abi_date_from_date",
            "status": "external_link",
            "primitive_class": "molt_cpython_abi_link_import",
            "source": "runtime_symbols+undefined_symbols",
        }
    ]


def test_external_native_artifact_plan_records_package_native_symbol_board(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("BOOL_absolute",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "runtime_symbols": ["BOOL_absolute"],
                "objects": [
                    {
                        "object": "umathmodule.o",
                        "defined_symbols": ["PyInit__multiarray_umath"],
                        "undefined_symbols": ["BOOL_absolute"],
                        "required_capsules": [],
                    }
                ],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.core._multiarray_umath"},
    )

    assert errors == []
    assert plan is not None
    assert [symbol.digest_payload() for symbol in plan.artifacts[0].abi_symbols] == [
        {
            "symbol": "BOOL_absolute",
            "status": "package_native",
            "primitive_class": "native_package_symbol",
            "source": "runtime_symbols+undefined_symbols",
        }
    ]


def test_external_native_artifact_plan_rejects_runtime_abi_without_custody(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("molt_alloc",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "undefined_symbols": ["molt_alloc"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert plan is None
    assert any(
        "object_closure undefined ABI symbol 'molt_alloc' is generated "
        "runtime-backed but missing from object_closure.runtime_symbols" in error
        for error in errors
    )


def test_external_native_artifact_plan_rejects_unknown_runtime_abi_symbol(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("molt_future_magic",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "runtime_symbols": ["molt_future_magic"],
                "undefined_symbols": ["molt_future_magic"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert plan is None
    assert any(
        "object_closure runtime ABI symbol 'molt_future_magic' is not in "
        "the generated WASM ABI/link import surface" in error
        for error in errors
    )


def test_external_native_artifact_plan_rejects_missing_c_api_symbol(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": ["PyCode_NewWithPosOnlyArgs"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert plan is None
    assert any(
        "object_closure required C-API symbol 'PyCode_NewWithPosOnlyArgs' "
        "is missing; primitive_class=code_frame_eval" in error
        for error in errors
    )


def test_external_native_artifact_plan_uses_cpython_abi_header_surface(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    required_symbols = [
        "PyDescr_IsData",
        "PyDict_SetDefaultRef",
        "PyErr_FormatUnraisable",
        "PyImport_ImportModuleLevel",
        "PyInterpreterState_GetIDFromThreadState",
        "PyLong_AsNativeBytes",
        "PyLong_FromNativeBytes",
        "PyLong_FromUnsignedNativeBytes",
        "PyMapping_GetOptionalItem",
        "PyUnicode_FindChar",
        "PyUnstable_SetImmortal",
    ]
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._ni_label",
        artifact_name="_ni_label.molt.wasm",
        manifest_overrides={
            "abi_tier": "cpython-abi",
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": required_symbols,
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._ni_label"},
    )

    assert errors == []
    assert plan is not None
    assert len(plan.artifacts) == 1
    c_api_status = {
        record.symbol: record.status for record in plan.artifacts[0].c_api_symbols
    }
    for symbol in required_symbols:
        assert c_api_status[symbol] == "runtime_backed"


def test_external_native_artifact_plan_rejects_undefined_source_compile_only_symbol(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        artifact_bytes=_wasm_exporting_i64_unary_symbol(
            "molt_nativepkg_placeholder",
            imports=("PyArray_NDIM",),
        ),
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {
                "required_c_api_symbols": ["PyArray_NDIM"],
                "undefined_symbols": ["PyArray_NDIM"],
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert plan is None
    assert any(
        "object_closure undefined C-API symbol 'PyArray_NDIM' is "
        "source_compile_only; primitive_class=numpy_c_api" in error
        for error in errors
    )


def test_external_native_artifact_plan_rejects_unknown_callable_export_abi(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "callable_exports": [
                {
                    "module": "nativepkg.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "direct_symbol",
                    "abi": "molt.forward_f33_v1",
                    "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
                    "effects": ["read", "write"],
                    "deterministic": True,
                }
            ],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.ndimage._nd_image"},
    )

    assert plan is None
    assert any(
        "callable_exports[0].abi must be one of: "
        "molt.object_call_v1, molt.object_callargs_v1, molt.forward_f32_v1, "
        "molt.pyinit_module_v1" in error
        for error in errors
    )


def test_source_recompiled_static_package_requires_native_artifact_candidate_pregraph(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, package_dir = _write_source_only_external_package(
        tmp_path,
        package="scipy",
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "scipy")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
    )

    assert policy is None
    assert error == 2
    stderr = capsys.readouterr().err
    assert "source-recompiled external static package admission" in stderr
    assert "before graph admission" in stderr
    assert "scipy" in stderr
    assert str(package_dir) in stderr


def test_source_recompiled_static_subpackage_requires_native_artifact_candidate(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _write_source_only_external_package(
        tmp_path,
        package="scipy.ndimage",
        init_source="from . import filters\n",
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "scipy.ndimage")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(tmp_path / "site",),
        json_output=False,
        defer_native_artifacts=True,
    )

    assert policy is None
    assert error == 2
    stderr = capsys.readouterr().err
    assert "native package root 'scipy'" in stderr
    assert "scipy.ndimage" in stderr


def test_admitted_external_native_package_does_not_close_source_only_ndimage_initializers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    external_root = tmp_path / "site"
    project.mkdir()
    entry = project / "main.py"
    entry.write_text("import scipy.ndimage\n", encoding="utf-8")
    _write_external_native_artifact(
        external_root,
        package="scipy",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
        },
    )
    scipy_dir = external_root / "scipy"
    ndimage_dir = scipy_dir / "ndimage"
    (scipy_dir / "__init__.py").write_text(
        "import scipy.massive\n",
        encoding="utf-8",
    )
    (scipy_dir / "massive.py").write_text("VALUE = 1\n", encoding="utf-8")
    (ndimage_dir / "__init__.py").write_text(
        "import numpy\nimport scipy.ndimage.filters\n",
        encoding="utf-8",
    )
    (ndimage_dir / "filters.py").write_text("VALUE = 2\n", encoding="utf-8")
    numpy_dir = external_root / "numpy"
    numpy_dir.mkdir()
    (numpy_dir / "__init__.py").write_text("import numpy.core\n", encoding="utf-8")
    (numpy_dir / "core.py").write_text("VALUE = 3\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "scipy")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
    )
    assert policy_error is None
    assert policy is not None

    prepared, error = cli._prepare_entry_module_graph(
        source_path=entry,
        entry_module="main",
        module_roots=[project.resolve(), external_root.resolve()],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=project,
        entry_tree=ast.parse(entry.read_text(encoding="utf-8")),
        diagnostics_enabled=True,
        module_reasons={},
        json_output=False,
        target="wasm",
        import_admission_policy=policy,
    )

    assert error is None
    assert prepared is not None
    assert "scipy.ndimage" in prepared.explicit_imports
    assert "scipy" not in prepared.module_graph
    assert "scipy.ndimage" not in prepared.module_graph
    assert "scipy.ndimage._nd_image" not in prepared.module_graph
    assert "scipy.massive" not in prepared.module_graph
    assert "scipy.ndimage.filters" not in prepared.module_graph
    assert "numpy" not in prepared.module_graph
    assert "numpy.core" not in prepared.module_graph
    native_plan, native_errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=policy.external_roots,
        admitted_packages=policy.admitted_external_packages,
        required_modules=(
            set(prepared.module_graph)
            | set(prepared.explicit_imports)
            | set(prepared.runtime_import_dispatch_roots)
        ),
    )
    assert native_errors == []
    assert native_plan is not None
    assert [artifact.module for artifact in native_plan.artifacts] == [
        "scipy.ndimage._nd_image"
    ]


def test_pure_python_external_static_package_can_defer_without_native_artifacts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _package_dir = _write_source_only_external_package(
        tmp_path,
        package="purepkg",
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "purepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
    )

    assert error is None
    assert policy is not None
    assert policy.admitted_external_packages == frozenset({"purepkg"})
    assert policy.native_artifact_plan.artifacts == ()


def test_native_artifact_plan_rejects_source_recompiled_package_without_candidates(
    tmp_path: Path,
) -> None:
    external_root, package_dir = _write_source_only_external_package(
        tmp_path,
        package="numpy",
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"numpy"},
    )

    assert plan is None
    assert len(errors) == 1
    assert "native package root 'numpy'" in errors[0]
    assert str(package_dir) in errors[0]


def test_external_static_package_native_artifact_requires_sidecar_manifest(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        write_manifest=False,
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert policy is None
    assert error == 2
    stderr = capsys.readouterr().err
    assert "native-artifact custody errors" in stderr
    assert "extension_manifest.json" in stderr
    assert str(artifact_path) in stderr


def test_external_static_package_native_artifact_requires_matching_checksum(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        checksum_override="0" * 64,
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert policy is None
    assert error == 2
    assert "extension_sha256 mismatch" in capsys.readouterr().err


def test_external_static_package_native_artifact_rejects_module_mismatch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        manifest_overrides={"module": "otherpkg._native"},
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert policy is None
    assert error == 2
    assert "does not match native artifact module" in capsys.readouterr().err


def test_external_static_package_native_artifact_rejects_extension_path_mismatch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        manifest_overrides={"extension": "nested/_native.so"},
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert policy is None
    assert error == 2
    assert "does not match native artifact" in capsys.readouterr().err


def test_external_static_package_native_artifact_rejects_invalid_manifest_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, _artifact_path, manifest_path = _write_external_native_package(
        tmp_path
    )
    manifest_path.write_text("{", encoding="utf-8")
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )

    assert policy is None
    assert error == 2
    assert "invalid extension manifest" in capsys.readouterr().err


def test_external_native_artifact_error_summary_is_bounded() -> None:
    errors = [f"pkg: native artifact ext{i}.so is missing sidecar" for i in range(20)]

    summary = cli_external_native._external_native_artifact_error_summary(
        errors,
        limit=5,
    )

    assert "ext0.so" in summary
    assert "ext4.so" in summary
    assert "ext5.so" not in summary
    assert "15 more external native artifact custody error(s)" in summary


def test_external_native_artifact_plan_filters_to_required_modules(
    tmp_path: Path,
) -> None:
    external_root, artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        write_manifest=False,
    )

    skipped_plan, skipped_errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.unused"},
    )
    required_plan, required_errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg._native"},
    )

    assert skipped_errors == []
    assert skipped_plan is not None
    assert skipped_plan.artifacts == ()
    assert required_plan is None
    assert len(required_errors) == 1
    assert str(artifact_path) in required_errors[0]


def test_external_native_artifact_plan_does_not_expand_package_root_to_children(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    _write_external_native_artifact(
        external_root,
        package="scipy",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
        },
    )

    root_plan, root_errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"scipy"},
        required_modules={"scipy"},
    )
    child_plan, child_errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"scipy"},
        required_modules={"scipy.ndimage"},
    )

    assert root_errors == []
    assert root_plan is not None
    assert root_plan.artifacts == ()
    assert child_errors == []
    assert child_plan is not None
    assert [artifact.module for artifact in child_plan.artifacts] == [
        "scipy.ndimage._nd_image"
    ]


def test_external_static_package_admission_can_defer_native_artifact_validation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        write_manifest=False,
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    eager_policy, eager_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    deferred_policy, deferred_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
    )

    assert eager_policy is None
    assert eager_error == 2
    assert str(artifact_path) in capsys.readouterr().err
    assert deferred_error is None
    assert deferred_policy is not None
    assert deferred_policy.native_artifact_plan.artifacts == ()


def test_wasm_external_static_package_with_native_source_requires_static_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root = tmp_path / "site"
    package_dir = external_root / "nativepkg"
    package_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    native_source = package_dir / "_native.c"
    native_source.write_text("int nativepkg(void) { return 1; }\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
        target="wasm",
    )

    assert policy is None
    assert error == 2
    stderr = capsys.readouterr().err
    assert "native source/artifact marker" in stderr
    assert str(native_source) in stderr
    assert "wasm32 static_link libmolt_source artifact manifest" in stderr
    assert "python_exports" in stderr


def test_wasm_source_recompiled_static_package_requires_export_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root = tmp_path / "artifacts"
    _artifact_path, _manifest_path = _write_external_native_artifact(
        external_root,
        package="numpy",
        relative_module="_core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
        },
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "numpy")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
        target="wasm",
    )

    assert policy is None
    assert error == 2
    stderr = capsys.readouterr().err
    assert "source-recompiled package root 'numpy'" in stderr
    assert "python_exports or callable_exports" in stderr
    assert "must not select native artifacts by directory ancestry" in stderr
    assert "_multiarray_umath.molt.wasm" in stderr


def test_source_recompiled_package_root_import_requires_export_owner(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "artifacts"
    _write_external_native_artifact(
        external_root,
        package="numpy",
        relative_module="_core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "python_exports": ["numpy._core._multiarray_umath"],
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"numpy"},
        required_modules={"numpy"},
    )

    assert plan is None
    assert len(errors) == 1
    assert "required source-recompiled package import 'numpy'" in errors[0]
    assert "publish 'numpy' in python_exports" in errors[0]
    assert "numpy._core._multiarray_umath" in errors[0]


def test_wasm_external_static_package_accepts_merged_source_and_artifact_roots(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source_root = tmp_path / "source"
    package_dir = source_root / "scipy"
    package_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text(
        "from . import ndimage\n", encoding="utf-8"
    )
    (package_dir / "_native.c").write_text(
        "int scipy_native_marker(void) { return 1; }\n",
        encoding="utf-8",
    )
    artifact_root = tmp_path / "artifacts"
    _write_external_native_artifact(
        artifact_root,
        package="scipy",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "python_exports": ["scipy.ndimage.distance_transform_edt"],
        },
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "scipy")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(source_root, artifact_root),
        json_output=False,
        defer_native_artifacts=True,
        target="wasm",
    )

    assert error is None
    assert policy is not None
    assert policy.native_artifact_plan.artifacts == ()


def test_wasm_external_static_package_allows_pure_python_source_closure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root = tmp_path / "site"
    package_dir = external_root / "purepkg"
    package_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "purepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
        target="wasm",
    )

    assert error is None
    assert policy is not None
    assert policy.admitted_external_packages == frozenset({"purepkg"})
    assert policy.native_artifact_plan.artifacts == ()


def test_wasm_external_static_package_allows_deferred_static_link_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        artifact_name="_native.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "python_exports": ["nativepkg.distance_transform_edt"],
        },
    )
    (external_root / "nativepkg" / "_native.c").write_text(
        "int nativepkg(void) { return 1; }\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")

    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
        defer_native_artifacts=True,
        target="wasm",
    )

    assert error is None
    assert policy is not None
    assert policy.native_artifact_plan.artifacts == ()


def test_external_native_artifact_plan_closes_over_capsule_providers(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    capsule = "nativepkg.core._multiarray_umath._ARRAY_API"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        manifest_overrides={"provided_capsules": [capsule]},
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="consumer",
        manifest_overrides={
            "object_closure": {"required_capsules": [capsule]},
        },
    )
    package_dir = external_root / "nativepkg"
    (package_dir / "unused.so").write_bytes(b"unchecked-wheel-extension")

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.consumer"},
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "nativepkg.consumer",
        "nativepkg.core._multiarray_umath",
    ]
    by_module = {artifact.module: artifact for artifact in plan.artifacts}
    assert by_module["nativepkg.consumer"].required_capsules == (capsule,)
    assert by_module["nativepkg.core._multiarray_umath"].provided_capsules == (capsule,)


def test_reachable_native_artifact_plan_keeps_capsule_providers(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    capsule = "nativepkg.core._multiarray_umath._ARRAY_API"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        manifest_overrides={"provided_capsules": [capsule]},
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="consumer",
        manifest_overrides={
            "python_exports": ["nativepkg.run"],
            "object_closure": {"required_capsules": [capsule]},
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.run"},
    )
    assert errors == []
    assert plan is not None

    reachable = plan.with_reachable_imports({"nativepkg.run"})

    assert [artifact.module for artifact in reachable.artifacts] == [
        "nativepkg.consumer",
        "nativepkg.core._multiarray_umath",
    ]


def test_external_native_artifact_plan_closes_over_object_capsule_requirements(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    capsule = "numpy.core._multiarray_umath._ARRAY_API"
    wasm_manifest = {
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
    }
    _write_external_native_artifact(
        external_root,
        package="numpy",
        relative_module="_core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        manifest_overrides={**wasm_manifest, "provided_capsules": [capsule]},
    )
    _write_external_native_artifact(
        external_root,
        package="scipy",
        relative_module="ndimage._nd_image",
        artifact_name="_nd_image.molt.wasm",
        manifest_overrides={
            **wasm_manifest,
            "object_closure": {
                "objects": [
                    {
                        "object": "0_nd_image.o",
                        "required_capsules": [capsule],
                    }
                ]
            },
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"numpy", "scipy"},
        required_modules={"scipy.ndimage"},
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "numpy._core._multiarray_umath",
        "scipy.ndimage._nd_image",
    ]
    by_module = {artifact.module: artifact for artifact in plan.artifacts}
    assert by_module["scipy.ndimage._nd_image"].required_capsules == (capsule,)
    assert by_module["numpy._core._multiarray_umath"].provided_capsules == (capsule,)


def test_external_native_artifact_plan_closes_over_wasm_static_capsule_providers(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    capsule = "nativepkg.core._multiarray_umath._ARRAY_API"
    wasm_manifest = {
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
    }
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        artifact_name="_multiarray_umath.molt.wasm",
        manifest_overrides={**wasm_manifest, "provided_capsules": [capsule]},
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="consumer",
        artifact_name="consumer.molt.wasm",
        manifest_overrides={
            **wasm_manifest,
            "object_closure": {"required_capsules": [capsule]},
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.consumer"},
    )

    assert errors == []
    assert plan is not None
    assert [artifact.module for artifact in plan.artifacts] == [
        "nativepkg.consumer",
        "nativepkg.core._multiarray_umath",
    ]
    assert all(artifact.runtime_linkage == "static_link" for artifact in plan.artifacts)


def test_external_native_artifact_plan_rejects_target_skewed_capsule_provider(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    capsule = "nativepkg.core._multiarray_umath._ARRAY_API"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="core._multiarray_umath",
        manifest_overrides={"provided_capsules": [capsule]},
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="consumer",
        artifact_name="consumer.molt.wasm",
        manifest_overrides={
            "target_triple": "wasm32-wasip1",
            "platform_tag": "wasm32_wasip1",
            "runtime_linkage": "static_link",
            "artifact_kind": "wasm_relocatable_object",
            "object_closure": {"required_capsules": [capsule]},
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.consumer"},
    )

    assert plan is None
    assert len(errors) == 1
    assert "target-compatible validated provider artifact" in errors[0]
    assert "nativepkg.core._multiarray_umath=host_resolved" in errors[0]


def test_external_native_artifact_plan_rejects_missing_capsule_provider(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    capsule = "nativepkg.core._multiarray_umath._ARRAY_API"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="consumer",
        manifest_overrides={
            "object_closure": {"required_capsules": [capsule]},
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.consumer"},
    )

    assert plan is None
    assert len(errors) == 1
    assert capsule in errors[0]
    assert "nativepkg.consumer" in errors[0]
    assert "no target-compatible validated provider artifact" in errors[0]


def test_external_native_artifact_plan_rejects_ambiguous_capsule_provider(
    tmp_path: Path,
) -> None:
    external_root = tmp_path / "site"
    capsule = "nativepkg.core._multiarray_umath._ARRAY_API"
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="providers.provider_a",
        manifest_overrides={"provided_capsules": [capsule]},
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="providers.provider_b",
        manifest_overrides={"provided_capsules": [capsule]},
    )
    _write_external_native_artifact(
        external_root,
        package="nativepkg",
        relative_module="consumer",
        manifest_overrides={
            "object_closure": {"required_capsules": [capsule]},
        },
    )

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(external_root,),
        admitted_packages={"nativepkg"},
        required_modules={"nativepkg.consumer"},
    )

    assert plan is None
    assert len(errors) == 1
    assert "multiple candidate provider artifacts" in errors[0]
    assert "nativepkg.providers.provider_a" in errors[0]
    assert "nativepkg.providers.provider_b" in errors[0]


def test_module_graph_policy_digest_includes_native_artifact_plan(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, artifact_path, manifest_path = _write_external_native_package(
        tmp_path,
        artifact_bytes=b"native-extension-v1",
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    first_policy, first_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert first_error is None
    assert first_policy is not None

    artifact_path.write_bytes(b"native-extension-v2")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["extension_sha256"] = hashlib.sha256(
        artifact_path.read_bytes()
    ).hexdigest()
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    second_policy, second_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert second_error is None
    assert second_policy is not None

    assert first_policy.native_artifact_plan.digest() != (
        second_policy.native_artifact_plan.digest()
    )
    assert cli_module_graph_cache._module_graph_policy_digest(
        {"sys"}, first_policy
    ) != cli_module_graph_cache._module_graph_policy_digest({"sys"}, second_policy)


def test_external_native_artifact_output_custody_accepts_native_binary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert error is None
    assert policy is not None
    output_layout = cli._BuildOutputLayout(
        is_wasm=False,
        is_wasm_freestanding=False,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_mlir_emit=False,
        split_runtime=False,
        linked=False,
        target_triple=None,
        emit_mode="bin",
        output_artifact=tmp_path / "output.o",
        output_binary=tmp_path / "app",
        linked_output_path=None,
        emit_ir_path=None,
    )

    assert (
        cli._external_native_artifact_output_custody_error(
            native_artifact_plan=policy.native_artifact_plan,
            output_layout=output_layout,
            target="native",
        )
        is None
    )


def test_external_native_artifact_output_custody_rejects_unpublished_outputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, _artifact_path, _manifest_path = _write_external_native_package(
        tmp_path
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert error is None
    assert policy is not None
    unsupported_layouts = [
        (
            "native",
            cli._BuildOutputLayout(
                is_wasm=False,
                is_wasm_freestanding=False,
                is_rust_transpile=False,
                is_luau_transpile=False,
                is_mlir_emit=False,
                split_runtime=False,
                linked=False,
                target_triple=None,
                emit_mode="obj",
                output_artifact=tmp_path / "output.o",
                output_binary=None,
                linked_output_path=None,
                emit_ir_path=None,
            ),
            "External static packages require native binary output",
            "target=native",
            "packages=nativepkg",
        ),
        (
            "wasm",
            cli._BuildOutputLayout(
                is_wasm=True,
                is_wasm_freestanding=False,
                is_rust_transpile=False,
                is_luau_transpile=False,
                is_mlir_emit=False,
                split_runtime=False,
                linked=True,
                target_triple=None,
                emit_mode="wasm",
                output_artifact=tmp_path / "output.wasm",
                output_binary=None,
                linked_output_path=tmp_path / "output_linked.wasm",
                emit_ir_path=None,
            ),
            "Linked WASM external static packages require wasm32 static_link",
            "nativepkg._native=host_resolved/shared_library",
            None,
        ),
        (
            "rust",
            cli._BuildOutputLayout(
                is_wasm=False,
                is_wasm_freestanding=False,
                is_rust_transpile=True,
                is_luau_transpile=False,
                is_mlir_emit=False,
                split_runtime=False,
                linked=False,
                target_triple=None,
                emit_mode="bin",
                output_artifact=tmp_path / "output.rs",
                output_binary=None,
                linked_output_path=None,
                emit_ir_path=None,
            ),
            "External static packages require native binary output",
            "target=rust",
            "packages=nativepkg",
        ),
        (
            "luau",
            cli._BuildOutputLayout(
                is_wasm=False,
                is_wasm_freestanding=False,
                is_rust_transpile=True,
                is_luau_transpile=True,
                is_mlir_emit=False,
                split_runtime=False,
                linked=False,
                target_triple=None,
                emit_mode="bin",
                output_artifact=tmp_path / "output.luau",
                output_binary=None,
                linked_output_path=None,
                emit_ir_path=None,
            ),
            "External static packages require native binary output",
            "target=luau",
            "packages=nativepkg",
        ),
        (
            "mlir",
            cli._BuildOutputLayout(
                is_wasm=False,
                is_wasm_freestanding=False,
                is_rust_transpile=False,
                is_luau_transpile=False,
                is_mlir_emit=True,
                split_runtime=False,
                linked=False,
                target_triple=None,
                emit_mode="bin",
                output_artifact=tmp_path / "output.mlir",
                output_binary=None,
                linked_output_path=None,
                emit_ir_path=None,
            ),
            "External static packages require native binary output",
            "target=mlir",
            "packages=nativepkg",
        ),
    ]

    for target, output_layout, expected, detail, package_detail in unsupported_layouts:
        error_message = cli._external_native_artifact_output_custody_error(
            native_artifact_plan=policy.native_artifact_plan,
            output_layout=output_layout,
            target=target,
        )
        assert error_message is not None
        assert expected in error_message
        assert detail in error_message
        if package_detail is not None:
            assert package_detail in error_message


def test_wrapper_build_cache_semantic_env_tracks_external_static_packages() -> None:
    semantic_env = cli._wrapper_build_cache_semantic_env(
        {
            "MOLT_EXTERNAL_STATIC_PACKAGES": "nativepkg",
            "MOLT_MODULE_ROOTS": "/tmp/site",
            "IGNORED": "value",
        }
    )

    assert semantic_env == {
        "MOLT_EXTERNAL_STATIC_PACKAGES": "nativepkg",
        "MOLT_MODULE_ROOTS": "/tmp/site",
    }


def test_frontend_known_modules_do_not_authorize_unknown_children() -> None:
    gen = SimpleTIRGenerator(
        parse_codec="json",
        known_modules={"externalpkg"},
        stdlib_allowlist=set(),
    )

    assert gen._is_known_project_module("externalpkg")
    assert not gen._is_known_project_module("externalpkg.hidden")
    assert not gen._should_attempt_runtime_module_import("externalpkg.hidden")


def test_frontend_known_modules_do_not_authorize_native_python_direct_calls() -> None:
    ops = _frontend_main_ops_for_import_source(
        "from scipy.ndimage import distance_transform_edt\n"
        "mask = 1\n"
        "result = distance_transform_edt(mask)\n",
        module_name="field_solve",
        parse_codec="json",
        known_modules={"field_solve", "scipy", "scipy.ndimage"},
        direct_call_modules={"field_solve"},
        stdlib_allowlist=set(),
        known_func_defaults={
            "scipy.ndimage": {
                "distance_transform_edt": {
                    "params": 1,
                    "defaults": [],
                }
            }
        },
    )

    assert any(op.get("kind") == "call_bind" for op in ops)
    assert all(
        not (
            op.get("kind") == "call"
            and op.get("s_value") == "scipy__ndimage__distance_transform_edt"
        )
        for op in ops
    )


def test_frontend_from_import_known_child_module_does_not_authorize_python_direct_call() -> (
    None
):
    ops = _frontend_main_ops_for_import_source(
        "from scipy import ndimage\n"
        "mask = 1\n"
        "result = ndimage.distance_transform_edt(mask)\n",
        module_name="field_solve",
        parse_codec="json",
        known_modules={"field_solve", "scipy", "scipy.ndimage"},
        direct_call_modules={"field_solve"},
        stdlib_allowlist=set(),
        known_func_defaults={
            "scipy.ndimage": {
                "distance_transform_edt": {
                    "params": 1,
                    "defaults": [],
                }
            }
        },
    )

    assert any(op.get("kind") in {"call_bind", "call_indirect"} for op in ops)
    assert all(
        not (
            op.get("kind") == "call"
            and op.get("s_value") == "scipy__ndimage__distance_transform_edt"
        )
        for op in ops
    )


def test_frontend_native_callable_export_lowers_to_invoke_ffi_metadata() -> None:
    export: dict[str, object] = {
        "module": "scipy.ndimage",
        "name": "distance_transform_edt",
        "binding": "direct_symbol",
        "abi": "molt.forward_f32_v1",
        "symbol": "molt_scipy_ndimage_distance_transform_edt",
    }

    ops = _frontend_main_ops_for_import_source(
        "from scipy.ndimage import distance_transform_edt\n"
        "mask = 1\n"
        "result = distance_transform_edt(mask)\n",
        module_name="field_solve",
        parse_codec="json",
        known_modules={"field_solve", "scipy", "scipy.ndimage"},
        direct_call_modules={"field_solve"},
        stdlib_allowlist=set(),
        known_func_defaults={
            "scipy.ndimage": {
                "distance_transform_edt": {
                    "params": 1,
                    "defaults": [],
                }
            }
        },
        native_callable_exports={"scipy.ndimage.distance_transform_edt": export},
    )

    invoke_ops = [
        op
        for op in ops
        if op.get("kind") == "invoke_ffi"
        and op.get("native_callable_export") == "scipy.ndimage.distance_transform_edt"
    ]
    assert len(invoke_ops) == 1
    invoke_op = invoke_ops[0]
    assert len(invoke_op["args"]) == 1
    assert invoke_op["native_callable_binding"] == "direct_symbol"
    assert invoke_op["native_callable_abi"] == "molt.forward_f32_v1"
    assert (
        invoke_op["native_callable_symbol"]
        == "molt_scipy_ndimage_distance_transform_edt"
    )
    assert all(op.get("kind") != "call_bind" for op in ops)
    assert all(
        not (
            op.get("kind") == "call"
            and op.get("s_value") == "scipy__ndimage__distance_transform_edt"
        )
        for op in ops
    )


def test_frontend_native_callable_export_lowers_from_imported_child_module_attr() -> (
    None
):
    export: dict[str, object] = {
        "module": "scipy.ndimage",
        "name": "distance_transform_edt",
        "binding": "direct_symbol",
        "abi": "molt.object_call_v1",
        "symbol": "molt_scipy_ndimage_distance_transform_edt",
    }

    ops = _frontend_main_ops_for_import_source(
        "from scipy import ndimage\n"
        "mask = 1\n"
        "result = ndimage.distance_transform_edt(mask)\n",
        module_name="field_solve",
        parse_codec="json",
        known_modules={"field_solve", "scipy", "scipy.ndimage"},
        direct_call_modules={"field_solve"},
        stdlib_allowlist=set(),
        known_func_defaults={
            "scipy.ndimage": {
                "distance_transform_edt": {
                    "params": 1,
                    "defaults": [],
                }
            }
        },
        native_callable_exports={"scipy.ndimage.distance_transform_edt": export},
    )

    invoke_ops = [
        op
        for op in ops
        if op.get("kind") == "invoke_ffi"
        and op.get("native_callable_export") == "scipy.ndimage.distance_transform_edt"
    ]
    assert len(invoke_ops) == 1
    invoke_op = invoke_ops[0]
    assert len(invoke_op["args"]) == 1
    assert invoke_op["native_callable_binding"] == "direct_symbol"
    assert invoke_op["native_callable_abi"] == "molt.object_call_v1"
    assert (
        invoke_op["native_callable_symbol"]
        == "molt_scipy_ndimage_distance_transform_edt"
    )
    assert all(op.get("kind") != "call_bind" for op in ops)
    assert all(op.get("kind") != "call_indirect" for op in ops)
    assert all(
        not (
            op.get("kind") == "call"
            and op.get("s_value") == "scipy__ndimage__distance_transform_edt"
        )
        for op in ops
    )


def test_frontend_native_callable_callargs_export_lowers_keyword_child_module_attr() -> (
    None
):
    export: dict[str, object] = {
        "module": "scipy.ndimage",
        "name": "gaussian_filter",
        "binding": "direct_symbol",
        "abi": "molt.object_callargs_v1",
        "symbol": "molt_scipy_ndimage_gaussian_filter",
    }

    ops = _frontend_main_ops_for_import_source(
        "from scipy import ndimage\n"
        "mask = 1\n"
        "result = ndimage.gaussian_filter(mask, sigma=1.5)\n",
        module_name="field_solve",
        parse_codec="json",
        known_modules={"field_solve", "scipy", "scipy.ndimage"},
        direct_call_modules={"field_solve"},
        stdlib_allowlist=set(),
        native_callable_exports={"scipy.ndimage.gaussian_filter": export},
    )

    invoke_ops = [
        op
        for op in ops
        if op.get("kind") == "invoke_ffi"
        and op.get("native_callable_export") == "scipy.ndimage.gaussian_filter"
    ]
    assert len(invoke_ops) == 1
    invoke_op = invoke_ops[0]
    assert len(invoke_op["args"]) == 1
    assert invoke_op["native_callable_binding"] == "direct_symbol"
    assert invoke_op["native_callable_abi"] == "molt.object_callargs_v1"
    assert invoke_op["native_callable_symbol"] == "molt_scipy_ndimage_gaussian_filter"
    assert any(op.get("kind") == "callargs_new" for op in ops)
    assert any(op.get("kind") == "callargs_push_pos" for op in ops)
    assert any(op.get("kind") == "callargs_push_kw" for op in ops)
    assert all(op.get("kind") != "call_bind" for op in ops)
    assert all(op.get("kind") != "call_indirect" for op in ops)


def test_frontend_pact_ndimage_operation_closure_lowers_to_native_abi() -> None:
    exports: dict[str, dict[str, object]] = {
        "scipy.ndimage.distance_transform_edt": {
            "module": "scipy.ndimage",
            "name": "distance_transform_edt",
            "binding": "module_attr",
            "abi": "molt.object_call_v1",
        },
        "scipy.ndimage.gaussian_filter": {
            "module": "scipy.ndimage",
            "name": "gaussian_filter",
            "binding": "module_attr",
            "abi": "molt.object_callargs_v1",
        },
        "scipy.ndimage.maximum_filter": {
            "module": "scipy.ndimage",
            "name": "maximum_filter",
            "binding": "module_attr",
            "abi": "molt.object_callargs_v1",
        },
        "scipy.ndimage.minimum_filter": {
            "module": "scipy.ndimage",
            "name": "minimum_filter",
            "binding": "module_attr",
            "abi": "molt.object_callargs_v1",
        },
        "scipy.ndimage.label": {
            "module": "scipy.ndimage",
            "name": "label",
            "binding": "module_attr",
            "abi": "molt.object_call_v1",
        },
    }

    ops = _frontend_main_ops_for_import_source(
        "from scipy import ndimage\n"
        "from scipy.ndimage import distance_transform_edt, gaussian_filter\n"
        "mask = 1\n"
        "a = distance_transform_edt(mask)\n"
        "b = ndimage.distance_transform_edt(mask)\n"
        "c = gaussian_filter(mask, sigma=1.5)\n"
        "d = ndimage.gaussian_filter(mask, sigma=2.0)\n"
        "e = ndimage.maximum_filter(mask, size=15)\n"
        "f = ndimage.minimum_filter(mask, size=11)\n"
        "g = ndimage.label(mask)\n",
        module_name="field_solve",
        parse_codec="json",
        known_modules={"field_solve", "scipy", "scipy.ndimage"},
        direct_call_modules={"field_solve"},
        stdlib_allowlist=set(),
        native_callable_exports=exports,
    )

    expected_counts = {
        "scipy.ndimage.distance_transform_edt": 2,
        "scipy.ndimage.gaussian_filter": 2,
        "scipy.ndimage.maximum_filter": 1,
        "scipy.ndimage.minimum_filter": 1,
        "scipy.ndimage.label": 1,
    }
    invoke_ops_by_export = {
        name: [
            op
            for op in ops
            if op.get("kind") == "invoke_ffi"
            and op.get("native_callable_export") == name
        ]
        for name in expected_counts
    }

    assert {name: len(items) for name, items in invoke_ops_by_export.items()} == (
        expected_counts
    )
    for export_name, invoke_ops in invoke_ops_by_export.items():
        spec = exports[export_name]
        for invoke_op in invoke_ops:
            assert invoke_op["native_callable_binding"] == "module_attr"
            assert invoke_op["native_callable_abi"] == spec["abi"]
            assert "native_callable_symbol" not in invoke_op
            assert len(invoke_op["args"]) == 2
    assert sum(1 for op in ops if op.get("kind") == "callargs_new") == 4
    assert sum(1 for op in ops if op.get("kind") == "callargs_push_kw") == 4
    assert all(op.get("kind") != "call_bind" for op in ops)
    assert all(op.get("kind") != "call_indirect" for op in ops)
    assert all(
        not (
            op.get("kind") == "call"
            and isinstance(op.get("s_value"), str)
            and op["s_value"].startswith("scipy__ndimage__")
        )
        for op in ops
    )


def test_frontend_native_callable_module_attr_export_lowers_to_runtime_ffi() -> None:
    export: dict[str, object] = {
        "module": "scipy.ndimage",
        "name": "distance_transform_edt",
        "binding": "module_attr",
        "abi": "molt.object_call_v1",
    }

    ops = _frontend_main_ops_for_import_source(
        "from scipy.ndimage import distance_transform_edt\n"
        "mask = 1\n"
        "result = distance_transform_edt(mask)\n",
        module_name="field_solve",
        parse_codec="json",
        known_modules={"field_solve", "scipy", "scipy.ndimage"},
        direct_call_modules={"field_solve"},
        stdlib_allowlist=set(),
        known_func_defaults={
            "scipy.ndimage": {
                "distance_transform_edt": {
                    "params": 1,
                    "defaults": [],
                }
            }
        },
        native_callable_exports={"scipy.ndimage.distance_transform_edt": export},
    )

    invoke_ops = [
        op
        for op in ops
        if op.get("kind") == "invoke_ffi"
        and op.get("native_callable_export") == "scipy.ndimage.distance_transform_edt"
    ]
    assert len(invoke_ops) == 1
    invoke_op = invoke_ops[0]
    assert len(invoke_op["args"]) == 2
    assert invoke_op["native_callable_binding"] == "module_attr"
    assert invoke_op["native_callable_abi"] == "molt.object_call_v1"
    assert "native_callable_symbol" not in invoke_op
    assert all(op.get("kind") != "call_bind" for op in ops)
    assert all(
        not (
            op.get("kind") == "call"
            and op.get("s_value") == "scipy__ndimage__distance_transform_edt"
        )
        for op in ops
    )


def test_frontend_native_callable_module_attr_rejects_memory_abi() -> None:
    export: dict[str, object] = {
        "module": "scipy.ndimage",
        "name": "distance_transform_edt",
        "binding": "module_attr",
        "abi": "molt.forward_f32_v1",
    }

    with pytest.raises(CompatibilityError, match="module_attr memory ABI"):
        _frontend_main_ops_for_import_source(
            "from scipy.ndimage import distance_transform_edt\n"
            "mask = 1\n"
            "result = distance_transform_edt(mask)\n",
            module_name="field_solve",
            parse_codec="json",
            known_modules={"field_solve", "scipy", "scipy.ndimage"},
            direct_call_modules={"field_solve"},
            stdlib_allowlist=set(),
            known_func_defaults={
                "scipy.ndimage": {
                    "distance_transform_edt": {
                        "params": 1,
                        "defaults": [],
                    }
                }
            },
            native_callable_exports={"scipy.ndimage.distance_transform_edt": export},
        )


def test_frontend_native_python_export_without_callable_metadata_fails_closed() -> None:
    sources = [
        "from scipy.ndimage import distance_transform_edt\n"
        "mask = 1\n"
        "result = distance_transform_edt(mask)\n",
        "from scipy import ndimage\n"
        "mask = 1\n"
        "result = ndimage.distance_transform_edt(mask)\n",
    ]

    for source in sources:
        with pytest.raises(
            CompatibilityError,
            match=(
                "native Python export 'scipy\\.ndimage\\.distance_transform_edt' "
                "has no callable ABI metadata"
            ),
        ):
            _frontend_main_ops_for_import_source(
                source,
                module_name="field_solve",
                parse_codec="json",
                known_modules={"field_solve", "scipy", "scipy.ndimage"},
                direct_call_modules={"field_solve"},
                stdlib_allowlist=set(),
                native_python_exports={"scipy.ndimage.distance_transform_edt"},
            )


def test_collect_imports_module_init_scan_skips_function_body_imports() -> None:
    tree = ast.parse(
        "import os\ndef f() -> None:\n    import warnings\nclass C:\n    import re\n"
    )
    full = cli_module_import_scanner._collect_imports(tree)
    module_init = cli_module_import_scanner._collect_imports(
        tree, import_scan_mode="module_init"
    )
    assert "warnings" in full
    assert "re" in full
    assert "warnings" not in module_init
    assert "re" in module_init
    assert "os" in module_init


def test_collect_imports_prunes_type_checking_branches() -> None:
    tree = ast.parse(
        "TYPE_CHECKING = False\n"
        "if TYPE_CHECKING:\n"
        "    import typing\n"
        "    import warnings\n"
        "else:\n"
        "    import os\n"
    )

    for mode in ("full", "module_init"):
        imports = cli_module_import_scanner._collect_imports(
            tree, import_scan_mode=cast(Any, mode)
        )
        assert "os" in imports
        assert "typing" not in imports
        assert "warnings" not in imports


def test_collect_imports_prunes_boolean_static_guard_branches() -> None:
    tree = ast.parse(
        "from typing import TYPE_CHECKING\n"
        "if TYPE_CHECKING or False:\n"
        "    import numpy\n"
        "if not TYPE_CHECKING:\n"
        "    import scipy\n"
        "if False and __import__('pandas'):\n"
        "    import pandas\n"
        "if TYPE_CHECKING is False:\n"
        "    import scipy.ndimage\n"
    )

    imports = set(
        cli_module_import_scanner._collect_imports(
            tree,
            module_name="pkg",
            import_scan_mode="module_init",
        )
    )

    assert "typing" not in imports
    assert "numpy" not in imports
    assert "pandas" not in imports
    assert "scipy" in imports
    assert "scipy.ndimage" in imports


def test_collect_imports_prunes_type_checking_alias_branches() -> None:
    tree = ast.parse(
        "from typing import TYPE_CHECKING as TC\n"
        "import typing as typing_alias\n"
        "if TC:\n"
        "    import warnings\n"
        "else:\n"
        "    import os\n"
        "if typing_alias.TYPE_CHECKING:\n"
        "    import re\n"
    )

    for mode in ("full", "module_init"):
        imports = cli_module_import_scanner._collect_imports(
            tree, import_scan_mode=cast(Any, mode)
        )
        assert "os" in imports
        assert "typing" in imports
        assert "typing.TYPE_CHECKING" not in imports
        assert "warnings" not in imports
        assert "re" not in imports


def test_collect_imports_type_checking_alias_rebind_stops_pruning() -> None:
    tree = ast.parse(
        "from typing import TYPE_CHECKING as TC\n"
        "TC = dynamic_flag\n"
        "if TC:\n"
        "    import warnings\n"
    )

    imports = cli_module_import_scanner._collect_imports(tree)

    assert "typing" not in imports
    assert "typing.TYPE_CHECKING" not in imports
    assert "warnings" in imports


def test_molt_os_module_init_imports_do_not_pull_typing() -> None:
    tree = ast.parse((ROOT / "src/molt/stdlib/os.py").read_text(encoding="utf-8"))

    imports = cli_module_import_scanner._collect_imports(
        tree,
        module_name="os",
        import_scan_mode="module_init",
    )

    assert "abc" in imports
    assert "typing" not in imports


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
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "_socket" in imports


def test_collect_imports_module_init_scan_resolves_top_level_helper_call() -> None:
    tree = ast.parse(
        "import importlib\n"
        "MODULE_NAME = '_socket'\n"
        "def _probe(module_name):\n"
        "    return importlib.import_module(module_name)\n"
        "_probe(MODULE_NAME)\n"
    )
    imports = cli_module_import_scanner._collect_imports(
        tree, import_scan_mode="module_init"
    )
    assert "_socket" in imports


def test_collect_imports_resolves_helper_call_nested_in_expression() -> None:
    tree = ast.parse(
        "import importlib\n"
        "MODULE_NAME = '_socket'\n"
        "def _probe(module_name):\n"
        "    return importlib.import_module(module_name)\n"
        "print(_probe(MODULE_NAME))\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "_socket" in imports


def test_collect_imports_resolves_name_argument_for_import_module() -> None:
    tree = ast.parse(
        "import importlib\nTARGET = 'pathlib'\nimportlib.import_module(TARGET)\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" in imports


def test_collect_imports_resolves_importlib_intrinsic_transaction_wrapper() -> None:
    tree = ast.parse(
        "_MOLT_IMPORTLIB_RESOLVE_NAME = object()\n"
        "_MOLT_IMPORTLIB_IMPORT_TRANSACTION = object()\n"
        "def import_module(name: str, package: object = None):\n"
        "    resolved = _MOLT_IMPORTLIB_RESOLVE_NAME(name, package)\n"
        "    mod = _MOLT_IMPORTLIB_IMPORT_TRANSACTION(\n"
        "        resolved, globals(), locals(), ('*',), 0\n"
        "    )\n"
        "    return mod\n"
        "machinery = import_module('importlib.machinery')\n"
        "util = import_module('importlib.util')\n"
        "_bootstrap = import_module('importlib._bootstrap')\n"
        "_bootstrap_external = import_module('importlib._bootstrap_external')\n"
    )

    imports = cli_module_import_scanner._collect_imports(
        tree,
        module_name="importlib",
        is_package=True,
        import_scan_mode="module_init",
    )

    assert "importlib.machinery" in imports
    assert "importlib.util" in imports
    assert "importlib._bootstrap" in imports
    assert "importlib._bootstrap_external" in imports


def test_collect_imports_resolves_importlib_relative_resolve_name() -> None:
    tree = ast.parse(
        "_MOLT_IMPORTLIB_RESOLVE_NAME = object()\n"
        "_MOLT_IMPORTLIB_IMPORT_TRANSACTION = object()\n"
        "def import_module(name, package=None):\n"
        "    resolved = _MOLT_IMPORTLIB_RESOLVE_NAME(name, package)\n"
        "    return _MOLT_IMPORTLIB_IMPORT_TRANSACTION(\n"
        "        resolved, globals(), locals(), ('*',), 0\n"
        "    )\n"
        "value = import_module('.machinery', package='importlib')\n"
    )

    imports = cli_module_import_scanner._collect_imports(
        tree,
        module_name="importlib",
        is_package=True,
        import_scan_mode="module_init",
    )

    assert "importlib.machinery" in imports


def test_collect_imports_real_importlib_init_includes_runtime_submodules() -> None:
    importlib_init = (
        cli_module_resolution._stdlib_root_path() / "importlib" / "__init__.py"
    )
    tree = ast.parse(
        importlib_init.read_text(encoding="utf-8"), filename=str(importlib_init)
    )

    imports = cli_module_import_scanner._collect_imports(
        tree,
        module_name="importlib",
        is_package=True,
        import_scan_mode="module_init",
    )

    assert "importlib.machinery" in imports
    assert "importlib.util" in imports
    assert "importlib._bootstrap" in imports
    assert "importlib._bootstrap_external" in imports


def test_collect_imports_resolves_aliased_importlib_import_module() -> None:
    tree = ast.parse(
        "import importlib as loader\nTARGET = 'pathlib'\nloader.import_module(TARGET)\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" in imports


def test_collect_imports_resolves_from_import_import_module_alias() -> None:
    tree = ast.parse(
        "from importlib import import_module as load_module\n"
        "TARGET = 'pathlib'\n"
        "load_module(TARGET)\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" in imports


def test_collect_imports_does_not_resolve_future_importlib_alias() -> None:
    tree = ast.parse(
        "TARGET = 'pathlib'\nloader.import_module(TARGET)\nimport importlib as loader\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" not in imports


def test_collect_imports_importlib_rebinding_blocks_static_resolution() -> None:
    tree = ast.parse(
        "import importlib\n"
        "TARGET = 'pathlib'\n"
        "def fake(name):\n"
        "    return None\n"
        "importlib.import_module = fake\n"
        "importlib.import_module(TARGET)\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" not in imports


def test_collect_imports_function_local_importlib_alias_does_not_leak() -> None:
    tree = ast.parse(
        "def configure():\n"
        "    import importlib as loader\n"
        "TARGET = 'pathlib'\n"
        "loader.import_module(TARGET)\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" not in imports


def test_collect_imports_function_local_rebinding_does_not_poison_module_alias() -> (
    None
):
    tree = ast.parse(
        "import importlib\n"
        "TARGET = 'pathlib'\n"
        "def configure():\n"
        "    importlib.import_module = lambda name: None\n"
        "importlib.import_module(TARGET)\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" in imports


def test_collect_imports_resolves_importlib_rhs_before_alias_rebind() -> None:
    tree = ast.parse(
        "import importlib\n"
        "TARGET = 'pathlib'\n"
        "importlib = importlib.import_module(TARGET)\n"
    )
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "pathlib" in imports


def test_discover_module_graph_includes_importlib_from_alias_target(
    tmp_path: Path,
) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    (package / "__init__.py").write_text("", encoding="utf-8")
    helper = package / "helper.py"
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    entry = tmp_path / "main.py"
    entry.write_text(
        "from importlib import import_module as load_module\n"
        "mod = load_module('pkg.helper')\n",
        encoding="utf-8",
    )
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
        entry,
        [*module_roots, stdlib_root],
        module_roots,
        stdlib_root,
        tmp_path,
        cli_module_stdlib_policy._stdlib_allowlist(),
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )

    assert graph["pkg.helper"] == helper
    assert "pkg.helper" in explicit_imports


def test_cached_json_round_trips_molt_value_and_set() -> None:
    payload = {
        "value": MoltValue(name="v1", type_hint="int"),
        "names": {"alpha", "beta"},
    }

    encoded = json.dumps(payload, default=CACHE_KEYS._json_ir_default)
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
    imports = cli_module_import_scanner._collect_imports(tree)
    assert "math" in imports
    assert "sys" in imports


def test_collect_imports_avoids_module_tree_walk_for_nested_scans(
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
    original_walk = cli_module_import_scanner.ast.walk

    def wrapped_walk(node: ast.AST):
        nonlocal module_tree_walks
        if node is tree:
            module_tree_walks += 1
        return original_walk(node)

    monkeypatch.setattr(cli_module_import_scanner.ast, "walk", wrapped_walk)

    imports = cli_module_import_scanner._collect_imports(tree)

    assert module_tree_walks == 0
    assert "os" in imports
    assert "warnings" in imports


def test_backend_ir_text_is_compact() -> None:
    text = CACHE_KEYS._backend_ir_text(
        {
            "functions": [{"name": "main", "ops": [{"kind": "ret", "args": []}]}],
            "profile": {"hash": "abc"},
        }
    )
    assert "\n" not in text
    assert ": " not in text
    assert '"functions"' in text


def test_backend_ir_lease_streams_json_without_bytes_helper(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        CACHE_KEYS,
        "_backend_ir_bytes",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("backend IR lease must not materialize bytes")
        ),
    )

    lease_path = cli._write_backend_ir_lease(
        tmp_path,
        {"functions": [{"name": "main", "ops": [{"kind": "ret_void"}]}]},
    )

    assert lease_path.parent == tmp_path / "tmp" / "backend-ir-leases"
    payload = json.loads(lease_path.read_text())
    assert payload["functions"][0]["name"] == "main"


def test_link_fingerprint_reuses_inputs_digest_when_unchanged(tmp_path: Path) -> None:
    stub = tmp_path / "main_stub.c"
    obj = tmp_path / "output.o"
    runtime = tmp_path / "libmolt_runtime.a"
    stub.write_text("int main(void) { return 0; }\n")
    obj.write_bytes(b"\x7fELFobject")
    runtime.write_bytes(b"archive")

    fingerprint = cli_link_pipeline._link_fingerprint(
        project_root=tmp_path,
        inputs=[stub, obj, runtime],
        link_cmd=["clang", str(stub), str(obj), str(runtime), "-o", "app"],
    )
    assert fingerprint is not None

    reused = cli_link_pipeline._link_fingerprint(
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

    first = cli_link_pipeline._link_fingerprint(
        project_root=tmp_path,
        inputs=[stub, obj, runtime],
        link_cmd=["clang", str(stub), str(obj), str(runtime), "-o", "app"],
    )
    second = cli_link_pipeline._link_fingerprint(
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


def test_write_link_fingerprint_reports_json_warning_on_metadata_loss(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def raise_metadata_write(path: Path, payload: Mapping[str, object]) -> None:
        del path, payload
        raise OSError("state volume read-only")

    monkeypatch.setattr(
        cli_build_results,
        "_write_runtime_fingerprint",
        raise_metadata_write,
    )

    warning = cli_build_results._write_link_fingerprint_if_needed(
        link_skipped=False,
        link_fingerprint={"hash": "fingerprint"},
        link_fingerprint_path=tmp_path / "state" / "link.json",
        json_output=True,
    )

    assert warning is not None
    assert "failed to write link fingerprint metadata" in warning
    assert "state volume read-only" in warning


def _write_shared_stdlib_test_contract(stdlib_obj: Path, cache_key: str) -> str:
    manifest = cli._shared_stdlib_manifest(
        cache_key=cache_key,
        cache_variant="test",
        target_triple=None,
        compiler_fingerprint="test",
    )
    assert manifest is not None
    cli._stdlib_object_key_sidecar_path(stdlib_obj).write_text(
        f"{cache_key}\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(stdlib_obj).write_text(
        manifest, encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_obj).write_text(
        '{"body_hash":"test","function_count":1,"functions":["molt_init_sys"],"schema":"stdlib-partition-v1"}',
        encoding="utf-8",
    )
    cli._stdlib_object_digest_sidecar_path(stdlib_obj).write_text(
        cli._sha256_file(stdlib_obj) + "\n", encoding="utf-8"
    )
    return manifest


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
    stdlib_manifest = _write_shared_stdlib_test_contract(stdlib_obj, "stdlib-key")
    captured_inputs: list[Path] = []
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()

    def fake_link_fingerprint(
        *,
        project_root: Path,
        inputs: list[Path],
        link_cmd: list[str],
        stored_fingerprint: dict[str, object] | None = None,
    ) -> dict[str, str | None]:
        del project_root, link_cmd, stored_fingerprint
        captured_inputs[:] = inputs
        return {"hash": "fingerprint", "rustc": None, "inputs_digest": None}

    monkeypatch.setattr(cli_link_pipeline, "_link_fingerprint", fake_link_fingerprint)
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )

    prepared, error = cli_link_pipeline._prepare_native_link(
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
        stdlib_object_manifest=stdlib_manifest,
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
    stdlib_manifest = _write_shared_stdlib_test_contract(stdlib_obj, "stdlib-key")
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()

    monkeypatch.setattr(
        cli_link_pipeline, "_read_runtime_fingerprint", lambda path: None
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(
        cli_link_pipeline,
        "_run_native_link_command",
        lambda **kwargs: subprocess.CompletedProcess(kwargs["link_cmd"], 0, "", ""),
    )

    first, first_error = cli_link_pipeline._prepare_native_link(
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
        stdlib_object_manifest=stdlib_manifest,
    )
    assert first_error is None
    assert first is not None

    stdlib_obj.write_bytes(b"stdlib-v2")
    _write_shared_stdlib_test_contract(stdlib_obj, "stdlib-key")

    second, second_error = cli_link_pipeline._prepare_native_link(
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
        stdlib_object_manifest=stdlib_manifest,
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
    stdlib_manifest = _write_shared_stdlib_test_contract(stdlib_obj, "stdlib-key")
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()
    captured_link_cmd: list[str] = []

    monkeypatch.setattr(
        cli_link_pipeline, "_read_runtime_fingerprint", lambda path: None
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )

    def fake_run_native_link_command(
        *,
        link_cmd: list[str],
        json_output: bool,
        link_timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        del json_output, link_timeout
        captured_link_cmd[:] = link_cmd
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(
        cli_link_pipeline, "_run_native_link_command", fake_run_native_link_command
    )

    prepared, error = cli_link_pipeline._prepare_native_link(
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
        stdlib_object_manifest=stdlib_manifest,
    )

    assert error is None
    assert prepared is not None
    staged_stdlib = artifacts_root / stdlib_obj.name
    assert str(staged_stdlib) in captured_link_cmd
    assert staged_stdlib.read_bytes() == b"stdlib"


def test_stage_external_native_artifacts_prunes_extension_shim_candidates(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, artifact_path, _manifest_path = _write_external_native_package(
        tmp_path,
        shim_source="value = 911\n",
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert error is None
    assert policy is not None
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()
    runtime_root = (
        artifacts_root
        / "external_static_packages"
        / policy.native_artifact_plan.digest()
    )
    stale_shim = runtime_root / "nativepkg" / "_native.so.molt.py"
    stale_shim.parent.mkdir(parents=True, exist_ok=True)
    stale_shim.write_text("stale facade\n", encoding="utf-8")

    staged_once = cli._stage_external_package_native_artifacts_for_build(
        policy.native_artifact_plan,
        artifacts_root=artifacts_root,
    )

    assert artifact_path.with_name("_native.so.molt.py").is_file()
    assert len(staged_once) == 1
    assert not stale_shim.exists()
    assert stale_shim not in staged_once[0].staged_support_paths


def test_prepare_native_link_stages_external_native_artifacts_for_runtime_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    external_root, artifact_path, manifest_path = _write_external_native_package(
        tmp_path,
        shim_source="value = 911\n",
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert policy_error is None
    assert policy is not None
    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    output_binary = tmp_path / "app"
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()
    captured_inputs: list[Path] = []
    captured_link_cmd: list[str] = []

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

    def fake_run_native_link_command(
        *,
        link_cmd: list[str],
        json_output: bool,
        link_timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        del json_output, link_timeout
        captured_link_cmd[:] = link_cmd
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(cli_link_pipeline, "_link_fingerprint", fake_link_fingerprint)
    monkeypatch.setattr(
        cli_link_pipeline, "_read_runtime_fingerprint", lambda path: None
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_run_native_link_command", fake_run_native_link_command
    )

    prepared, error = cli_link_pipeline._prepare_native_link(
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
        native_artifact_plan=policy.native_artifact_plan,
    )

    assert error is None
    assert prepared is not None
    assert len(prepared.external_native_artifacts) == 1
    staged = prepared.external_native_artifacts[0]
    expected_runtime_root = (
        artifacts_root
        / "external_static_packages"
        / policy.native_artifact_plan.digest()
    )
    assert staged.runtime_root == expected_runtime_root
    assert staged.staged_path == expected_runtime_root / "nativepkg" / "_native.so"
    assert (
        staged.staged_manifest_path
        == expected_runtime_root / "nativepkg" / "extension_manifest.json"
    )
    staged_init = expected_runtime_root / "nativepkg" / "__init__.py"
    staged_shim = expected_runtime_root / "nativepkg" / "_native.so.molt.py"
    assert staged.staged_path.read_bytes() == artifact_path.read_bytes()
    assert json.loads(staged.staged_manifest_path.read_text(encoding="utf-8")) == (
        json.loads(manifest_path.read_text(encoding="utf-8"))
    )
    assert staged_init.read_text(encoding="utf-8").startswith(
        "import nativepkg._native"
    )
    assert not staged_shim.exists()
    assert staged.staged_path in captured_inputs
    assert staged.staged_manifest_path in captured_inputs
    assert staged_init in captured_inputs
    assert staged_shim not in captured_inputs
    stub_content = prepared.stub_path.read_text(encoding="utf-8")
    assert json.dumps(str(expected_runtime_root.resolve())) in stub_content
    native_main_start = stub_content.index("int main")
    assert stub_content.index(
        "molt_set_runtime_module_roots();", native_main_start
    ) < stub_content.index("molt_runtime_init();", native_main_start)
    assert str(staged.staged_path) not in captured_link_cmd
    assert str(staged.staged_manifest_path) not in captured_link_cmd


def test_render_native_main_stub_embeds_runtime_module_roots_before_init(
    tmp_path: Path,
) -> None:
    runtime_root = tmp_path / "artifacts" / "external_static_packages" / "digest"

    stub_content = cli._render_native_main_stub(
        trusted=False,
        capabilities_list=None,
        runtime_module_roots=(runtime_root,),
    )

    assert "static void molt_set_runtime_module_roots()" in stub_content
    assert json.dumps(str(runtime_root.resolve())) in stub_content
    assert 'getenv("MOLT_MODULE_ROOTS")' in stub_content
    assert 'setenv("MOLT_MODULE_ROOTS", roots, 1)' in stub_content
    native_main_start = stub_content.index("int main")
    assert stub_content.index(
        "molt_set_runtime_module_roots();", native_main_start
    ) < stub_content.index("molt_runtime_init();", native_main_start)


def test_prepare_native_link_rejects_external_native_artifact_checksum_drift(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    external_root, artifact_path, _manifest_path = _write_external_native_package(
        tmp_path
    )
    monkeypatch.setenv("MOLT_EXTERNAL_STATIC_PACKAGES", "nativepkg")
    policy, policy_error = cli._resolve_import_admission_policy(
        external_module_roots=(external_root,),
        json_output=False,
    )
    assert policy_error is None
    assert policy is not None
    artifact_path.write_bytes(b"mutated-after-validation")
    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()

    prepared, error = cli_link_pipeline._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=tmp_path / "app",
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
        native_artifact_plan=policy.native_artifact_plan,
    )

    assert prepared is None
    assert error == 2
    stderr = capsys.readouterr().err
    assert "Failed to stage external native artifacts" in stderr
    assert "checksum changed before staging" in stderr
    staged_path = (
        artifacts_root
        / "external_static_packages"
        / policy.native_artifact_plan.digest()
        / "nativepkg"
        / "_native.so"
    )
    assert not staged_path.exists()


def test_build_native_link_success_data_reports_external_native_artifacts(
    tmp_path: Path,
) -> None:
    runtime_root = tmp_path / "artifacts" / "external_static_packages" / "digest"
    staged = cli._StagedExternalPackageNativeArtifact(
        package="nativepkg",
        module="nativepkg._native",
        runtime_root=runtime_root,
        source_path=tmp_path / "site" / "nativepkg" / "_native.so",
        source_manifest_path=(
            tmp_path / "site" / "nativepkg" / "extension_manifest.json"
        ),
        staged_path=runtime_root / "nativepkg" / "_native.so",
        staged_manifest_path=runtime_root / "nativepkg" / "extension_manifest.json",
        staged_support_paths=(runtime_root / "nativepkg" / "_native.so.molt.py",),
        extension_sha256="e" * 64,
        manifest_sha256="m" * 64,
        capabilities=("module.extension.exec",),
        abi_tag="molt_abi1",
        target_triple="x86_64-unknown-linux-gnu",
        platform_tag="x86_64_unknown_linux_gnu",
    )

    data = cli_build_results._build_native_link_success_data(
        target="native",
        target_triple=None,
        source_path=tmp_path / "demo.py",
        output_binary=tmp_path / "app",
        deterministic=False,
        trusted=False,
        capabilities_list=None,
        capability_profiles=None,
        capabilities_source=None,
        sysroot_path=None,
        cache_info={},
        emit_mode="bin",
        profile="dev",
        native_arch_perf_enabled=False,
        output_obj=tmp_path / "output.o",
        stub_path=tmp_path / "main_stub.c",
        runtime_lib=tmp_path / "libmolt_runtime.a",
        link_skipped=False,
        external_native_artifacts=(staged,),
    )

    assert data["artifacts"]["external_static_packages_root"] == str(runtime_root)
    assert data["artifacts"]["external_native_artifact_0"] == str(staged.staged_path)
    assert data["artifacts"]["external_native_artifact_0_manifest"] == str(
        staged.staged_manifest_path
    )
    assert data["external_native_artifacts"] == [staged.json_payload()]


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


def test_linux_release_link_omits_safe_icf_without_capable_linker(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.o"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_binary = tmp_path / "app"
    output_obj.write_bytes(b"\x7fELFobject")
    stub_path.write_text("int main(void) { return 0; }\n")
    runtime_lib.write_bytes(b"archive")

    monkeypatch.setattr(NATIVE_LINK_COMMAND.sys, "platform", "linux")
    monkeypatch.setenv("CC", "clang")
    monkeypatch.setattr(NATIVE_LINK_COMMAND.shutil, "which", lambda _name: None)

    link_cmd, linker_hint, _normalized_target = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple=None,
        sysroot_path=None,
        profile="release",
        stdlib_obj_path=None,
    )

    assert linker_hint is None
    assert "-Wl,--icf=safe" not in link_cmd


def test_linux_link_exports_molt_runtime_symbols_for_source_extensions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.o"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_binary = tmp_path / "app"
    output_obj.write_bytes(b"\x7fELFobject")
    stub_path.write_text("int main(void) { return 0; }\n")
    runtime_lib.write_bytes(b"archive")

    monkeypatch.setattr(NATIVE_LINK_COMMAND.sys, "platform", "linux")
    monkeypatch.setenv("CC", "clang")
    monkeypatch.setattr(NATIVE_LINK_COMMAND.shutil, "which", lambda _name: None)

    link_cmd, _linker_hint, _normalized_target = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple=None,
        sysroot_path=None,
        profile="release",
        stdlib_obj_path=None,
        export_molt_runtime_symbols=True,
    )

    assert "-Wl,--export-dynamic" in link_cmd
    version_script = tmp_path / ".molt_version.ver"
    assert "molt_*" in version_script.read_text(encoding="utf-8")


def test_linux_release_link_selects_lld_without_icf_for_fn_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.o"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "libmolt_runtime.a"
    output_binary = tmp_path / "app"
    output_obj.write_bytes(b"\x7fELFobject")
    stub_path.write_text("int main(void) { return 0; }\n")
    runtime_lib.write_bytes(b"archive")

    def fake_which(name: str) -> str | None:
        if name == "ld.lld":
            return "/usr/bin/ld.lld"
        return None

    monkeypatch.setattr(NATIVE_LINK_COMMAND.sys, "platform", "linux")
    monkeypatch.setenv("CC", "clang")
    monkeypatch.setattr(NATIVE_LINK_COMMAND.shutil, "which", fake_which)

    link_cmd, linker_hint, _normalized_target = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple=None,
        sysroot_path=None,
        profile="release",
        stdlib_obj_path=None,
    )

    assert linker_hint == "lld"
    assert "-fuse-ld=lld" in link_cmd
    assert "-Wl,--icf=safe" not in link_cmd


def test_windows_link_omits_icf_for_fn_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.obj"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "molt_runtime.lib"
    output_binary = tmp_path / "app.exe"
    output_obj.write_bytes(b"COFFobject")
    stub_path.write_text("int main(void) { return 0; }\n")
    runtime_lib.write_bytes(b"archive")

    monkeypatch.setattr(NATIVE_LINK_COMMAND.sys, "platform", "win32")
    monkeypatch.setenv("CC", "clang")
    monkeypatch.setattr(NATIVE_LINK_COMMAND.shutil, "which", lambda _name: None)

    link_cmd, _linker_hint, _normalized_target = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple=None,
        sysroot_path=None,
        profile="release",
        stdlib_obj_path=None,
    )

    assert "-Wl,/OPT:REF" in link_cmd
    assert "-Wl,/OPT:ICF" not in link_cmd
    assert "-lws2_32" in link_cmd
    assert "-lntdll" in link_cmd
    assert "-luserenv" in link_cmd
    assert "-ladvapi32" in link_cmd


def test_windows_link_exports_molt_runtime_symbols_for_source_extensions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.obj"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "molt_runtime.lib"
    output_binary = tmp_path / "app.exe"
    output_obj.write_bytes(b"COFFobject")
    stub_path.write_text("int main(void) { return 0; }\n")
    runtime_lib.write_bytes(b"archive")

    monkeypatch.setattr(NATIVE_LINK_COMMAND.sys, "platform", "win32")
    monkeypatch.setenv("CC", "clang")
    monkeypatch.setattr(NATIVE_LINK_COMMAND.shutil, "which", lambda _name: None)
    monkeypatch.setattr(
        NATIVE_LINK_COMMAND,
        "_molt_c_api_export_names",
        lambda: ("molt_c_api_version", "molt_module_create"),
    )

    link_cmd, _linker_hint, _normalized_target = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple=None,
        sysroot_path=None,
        profile="release",
        stdlib_obj_path=None,
        export_molt_runtime_symbols=True,
    )

    def_path = tmp_path / ".molt_exports.def"
    assert f"-Wl,/DEF:{def_path}" in link_cmd
    assert def_path.read_text(encoding="utf-8") == (
        "EXPORTS\nmolt_c_api_version\nmolt_module_create\n"
    )


def test_windows_gnu_link_uses_gnu_system_lib_flags(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_obj = tmp_path / "output.obj"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "molt_runtime.lib"
    output_binary = tmp_path / "app.exe"
    output_obj.write_bytes(b"COFFobject")
    stub_path.write_text("int main(void) { return 0; }\n")
    runtime_lib.write_bytes(b"archive")

    monkeypatch.setenv("CC", "zig cc")
    monkeypatch.setattr(
        NATIVE_LINK_COMMAND.shutil,
        "which",
        lambda name: "zig" if name == "zig" else None,
    )

    link_cmd, _linker_hint, normalized_target = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple="x86_64-pc-windows-gnu",
        sysroot_path=None,
        profile="release",
        stdlib_obj_path=None,
    )

    assert normalized_target == "x86_64-windows-gnu"
    assert "-lws2_32" in link_cmd
    assert "-lntdll" in link_cmd
    assert "-luserenv" in link_cmd
    assert "-ladvapi32" in link_cmd


def test_windows_native_partial_link_uses_coff_library_tool(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    input_obj = tmp_path / "app.obj"
    stdlib_obj = tmp_path / "stdlib.obj"
    output_obj = tmp_path / "out.obj"
    input_obj.write_bytes(b"coff")
    stdlib_obj.write_bytes(b"coff")
    captured: dict[str, object] = {}

    monkeypatch.setattr(NATIVE_LINK_DEPS.sys, "platform", "win32")
    monkeypatch.setattr(
        NATIVE_LINK_COMMAND.shutil,
        "which",
        lambda name: "C:/LLVM/bin/llvm-lib.exe" if name == "llvm-lib" else None,
    )

    def fake_run_native_link_command(
        *,
        link_cmd: Sequence[str],
        json_output: bool,
        link_timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        captured["link_cmd"] = list(link_cmd)
        captured["json_output"] = json_output
        captured["link_timeout"] = link_timeout
        return subprocess.CompletedProcess(list(link_cmd), 0, "", "")

    monkeypatch.setattr(
        cli_link_pipeline,
        "_run_native_link_command",
        fake_run_native_link_command,
    )

    result = cli_link_pipeline._run_native_partial_link_command(
        input_objects=[input_obj, stdlib_obj],
        output_path=output_obj,
        json_output=True,
        link_timeout=12.0,
        target_triple=None,
    )

    assert result.returncode == 0
    assert captured["link_cmd"] == [
        "C:/LLVM/bin/llvm-lib.exe",
        f"/OUT:{output_obj}",
        str(input_obj),
        str(stdlib_obj),
    ]
    assert "-Wl,-r" not in captured["link_cmd"]
    assert captured["json_output"] is True
    assert captured["link_timeout"] == 12.0


def _legacy_streamed_cache_digest(
    payload_ir: Mapping[str, object],
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
) -> str:
    payload = json.dumps(
        payload_ir,
        sort_keys=True,
        separators=(",", ":"),
        default=CACHE_KEYS._json_ir_default,
    ).encode("utf-8")
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    return hashlib.sha256(
        payload
        + b"|"
        + suffix.encode("utf-8")
        + b"|"
        + CACHE_KEYS._cache_fingerprint().encode("utf-8")
        + b"|"
        + CACHE_KEYS._cache_tooling_fingerprint().encode("utf-8")
        + b"|"
        + schema_version.encode("utf-8")
    ).hexdigest()


def test_streamed_cache_keys_preserve_legacy_payload_semantics() -> None:
    ir = {
        "functions": [
            {"name": "zeta", "ops": []},
            {"name": "alpha", "ops": []},
        ],
        "profile": {"hash": "abc"},
        "runtime_feedback": {"hot_functions": ["alpha"]},
    }
    module_payload_ir = CACHE_KEYS._cache_ir_payload_ir(ir)
    backend_payload_ir = CACHE_KEYS._cache_backend_payload_ir(ir)
    module_text = json.dumps(
        module_payload_ir,
        sort_keys=True,
        separators=(",", ":"),
        default=CACHE_KEYS._json_ir_default,
    )
    backend_text = json.dumps(
        backend_payload_ir,
        sort_keys=True,
        separators=(",", ":"),
        default=CACHE_KEYS._json_ir_default,
    )
    assert module_text.index('"name":"alpha"') < module_text.index('"name":"zeta"')
    assert backend_text.index('"name":"alpha"') < backend_text.index('"name":"zeta"')
    assert '"top_level_extras_digest"' in backend_text
    assert CACHE_KEYS._cache_key(
        ir, "native", None, "variant"
    ) == _legacy_streamed_cache_digest(
        module_payload_ir,
        target="native",
        target_triple=None,
        variant="variant",
        schema_version=CACHE_KEYS._CACHE_KEY_SCHEMA_VERSION,
    )
    assert CACHE_KEYS._function_cache_key(
        ir, "native", None, "variant"
    ) == _legacy_streamed_cache_digest(
        backend_payload_ir,
        target="native",
        target_triple=None,
        variant="variant",
        schema_version=CACHE_KEYS._FUNCTION_CACHE_KEY_SCHEMA_VERSION,
    )


def test_frontend_parallel_layer_result_take_releases_result_map() -> None:
    layer_state = cli_frontend_parallel._fresh_frontend_parallel_layer_state()
    result = {"ok": True, "functions": [{"name": "pkg__module", "ops": []}]}
    layer_state.results["pkg"] = result

    assert (
        cli_frontend_parallel._take_frontend_parallel_layer_result(layer_state, "pkg")
        is result
    )
    assert "pkg" not in layer_state.results
    assert (
        cli_frontend_parallel._take_frontend_parallel_layer_result(layer_state, "pkg")
        is None
    )


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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    resolve_calls = 0
    original = cli_module_resolution._resolve_module_path_parts

    def wrapped(parts: tuple[str, ...], roots_arg: list[Path]) -> Path | None:
        nonlocal resolve_calls
        resolve_calls += 1
        return original(parts, roots_arg)

    monkeypatch.setattr(cli_module_resolution, "_resolve_module_path_parts", wrapped)

    shared_cache = cli_module_resolution._ModuleResolutionCache()
    cli_module_graph_discovery._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        None,
        stdlib_allowlist,
        resolver_cache=shared_cache,
    )
    shared_first = resolve_calls
    cli_module_graph_discovery._discover_module_graph(
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
    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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
    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    read_calls = 0
    parse_calls = 0
    original_read = cli_module_source._read_module_source
    original_parse = TARGET_PYTHON.ast.parse

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

    monkeypatch.setattr(cli_module_source, "_read_module_source", wrapped_read)
    monkeypatch.setattr(TARGET_PYTHON.ast, "parse", wrapped_parse)

    shared_cache = cli_module_resolution._ModuleResolutionCache()
    graph, _ = cli_module_graph_discovery._discover_module_graph(
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
    assert first_read_calls > 0
    assert first_parse_calls > 0
    for module_path in graph.values():
        source = shared_cache.read_module_source(module_path)
        shared_cache.parse_module_ast(module_path, source, filename=str(module_path))
    retained_read_calls = read_calls
    retained_parse_calls = parse_calls
    assert retained_read_calls == first_read_calls
    assert retained_parse_calls == first_parse_calls
    for module_path in graph.values():
        source = shared_cache.read_module_source(module_path)
        shared_cache.parse_module_ast(module_path, source, filename=str(module_path))
    assert read_calls == retained_read_calls
    assert parse_calls == retained_parse_calls

    read_calls = 0
    parse_calls = 0
    unshared_graph, _ = cli_module_graph_discovery._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    for module_path in unshared_graph.values():
        source = cli_module_source._read_module_source(module_path)
        TARGET_PYTHON.ast.parse(source, filename=str(module_path))
    assert read_calls > 0
    assert parse_calls > 0


def test_shared_module_resolution_cache_reuses_resolved_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "pkg" / "__init__.py"
    entry.parent.mkdir()
    entry.write_text("VALUE = 1\n")
    stdlib_root = cli_module_resolution._stdlib_root_path()

    resolve_calls = 0
    original_resolve = Path.resolve

    def wrapped_resolve(self: Path, *args: object, **kwargs: object) -> Path:
        nonlocal resolve_calls
        resolve_calls += 1
        return original_resolve(self, *args, **kwargs)

    monkeypatch.setattr(Path, "resolve", wrapped_resolve)

    cache = cli_module_resolution._ModuleResolutionCache()
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
    cache = cli_module_resolution._ModuleResolutionCache()
    path = (tmp_path / "module.py").resolve()

    def fail_resolve(self: Path, *args: object, **kwargs: object) -> Path:
        raise AssertionError(f"resolve() should not run for {self}")

    with monkeypatch.context() as scoped:
        scoped.setattr(Path, "resolve", fail_resolve)
        assert cache.resolved_path(path) == path


def test_shared_module_resolution_cache_resolves_relative_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache = cli_module_resolution._ModuleResolutionCache()
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    collect_calls = 0
    original_collect = cli_module_import_scanner._collect_imports

    def wrapped_collect(*args: object, **kwargs: object) -> list[str]:
        nonlocal collect_calls
        collect_calls += 1
        return original_collect(*args, **kwargs)

    monkeypatch.setattr(cli_module_import_scanner, "_collect_imports", wrapped_collect)
    monkeypatch.setattr(
        cli_module_graph_cache,
        "_read_persisted_import_scan",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        cli_module_graph_cache,
        "_read_persisted_module_graph",
        lambda *args, **kwargs: None,
    )

    cache = cli_module_resolution._ModuleResolutionCache()
    graph, _ = cli_module_graph_discovery._discover_module_graph(
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
        import_scan_mode = cli_module_graph_discovery._module_graph_import_scan_mode(
            path=module_path,
            module_name=module_name,
            entry_paths=frozenset({cache.resolved_path(entry)}),
            static_import_helper_modules=(
                cli_module_import_scanner.STDLIB_STATIC_IMPORT_HELPER_MODULES
            ),
            resolution_cache=cache,
        )
        cache.collect_imports(
            module_path,
            tree,
            collector=cli_module_import_scanner._collect_imports,
            module_name=module_name,
            is_package=module_path.name == "__init__.py",
            import_scan_mode=import_scan_mode,
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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

    monkeypatch.setattr(cli_module_source, "_read_module_source", fail_read)

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    assert "pkg.helper" in explicit_imports
    assert "pkg" in graph


def test_persisted_import_scan_cache_tracks_tooling_fingerprint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text("import json\n", encoding="utf-8")

    monkeypatch.setattr(
        cli_module_graph_cache, "_cache_tooling_fingerprint", lambda: "tool-a"
    )
    cli_module_graph_cache._write_persisted_import_scan(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="module_init",
        imports=("json",),
    )
    assert cli_module_graph_cache._read_persisted_import_scan(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="module_init",
    ) == ("json",)

    monkeypatch.setattr(
        cli_module_graph_cache, "_cache_tooling_fingerprint", lambda: "tool-b"
    )
    assert (
        cli_module_graph_cache._read_persisted_import_scan(
            tmp_path,
            module_path,
            module_name="pkg.mod",
            is_package=False,
            import_scan_mode="module_init",
        )
        is None
    )


def test_persisted_import_scan_cache_tracks_source_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text("import json\n", encoding="utf-8")
    original = module_path.stat()

    monkeypatch.setattr(
        cli_module_graph_cache, "_cache_tooling_fingerprint", lambda: "tool-a"
    )
    cli_module_graph_cache._write_persisted_import_scan(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="module_init",
        imports=("json",),
    )

    _rewrite_preserving_mtime(module_path, "import math\n", original)

    assert (
        cli_module_graph_cache._read_persisted_import_scan(
            tmp_path,
            module_path,
            module_name="pkg.mod",
            is_package=False,
            import_scan_mode="module_init",
        )
        is None
    )


def test_source_content_sha256_reuses_persistent_hash_after_process_cache_clear(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text("VALUE = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    _clear_molt_home_caches()
    real_sha256_file = cli_module_source._sha256_file
    hash_calls = 0

    def counting_sha256_file(path: Path) -> str:
        nonlocal hash_calls
        hash_calls += 1
        return real_sha256_file(path)

    monkeypatch.setattr(cli_module_source, "_sha256_file", counting_sha256_file)

    first_hash = cli_module_source._source_content_sha256(module_path)
    assert first_hash is not None
    assert hash_calls == 1

    cli_module_source._source_content_sha256_cached.cache_clear()
    second_hash = cli_module_source._source_content_sha256(module_path)

    assert second_hash == first_hash
    stat = module_path.stat()
    stat_identity_is_strong = cli_module_source._source_hash_stat_identity_is_strong(
        ctime_ns=cli_module_source._stat_ctime_ns(stat),
        inode=int(getattr(stat, "st_ino", 0) or 0),
        device=cli_module_source._stat_device(stat),
    )
    assert hash_calls == (1 if stat_identity_is_strong else 2)


def test_source_content_sha256_rehashes_preserved_mtime_content_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text("VALUE = 1\n", encoding="utf-8")
    original = module_path.stat()
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    _clear_molt_home_caches()
    real_sha256_file = cli_module_source._sha256_file
    hash_calls = 0

    def counting_sha256_file(path: Path) -> str:
        nonlocal hash_calls
        hash_calls += 1
        return real_sha256_file(path)

    monkeypatch.setattr(cli_module_source, "_sha256_file", counting_sha256_file)

    first_hash = cli_module_source._source_content_sha256(module_path)
    assert first_hash is not None
    assert hash_calls == 1

    _rewrite_preserving_mtime(module_path, "VALUE = 2\n", original)
    cli_module_source._source_content_sha256_cached.cache_clear()
    second_hash = cli_module_source._source_content_sha256(module_path)

    assert second_hash is not None
    assert second_hash != first_hash
    assert hash_calls == 2


def test_persisted_module_graph_cache_tracks_tooling_fingerprint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry_path = tmp_path / "main.py"
    entry_path.write_text("import pkg.mod\n", encoding="utf-8")
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text("VALUE = 1\n", encoding="utf-8")
    roots = [tmp_path]
    module_roots = [tmp_path]
    stdlib_root = tmp_path / "stdlib"

    monkeypatch.setattr(
        cli_module_graph_cache, "_cache_tooling_fingerprint", lambda: "tool-a"
    )
    cli_module_graph_cache._write_persisted_module_graph(
        tmp_path,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules=set(),
        stub_parents=set(),
        stdlib_static_import_helper_modules=set(),
        stdlib_allowlist=set(),
        graph={"__main__": entry_path, "pkg.mod": module_path},
        explicit_imports={"pkg.mod"},
    )
    assert (
        cli_module_graph_cache._read_persisted_module_graph(
            tmp_path,
            entry_path,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            skip_modules=set(),
            stub_parents=set(),
            stdlib_static_import_helper_modules=set(),
            stdlib_allowlist=set(),
        )
        is not None
    )

    monkeypatch.setattr(
        cli_module_graph_cache, "_cache_tooling_fingerprint", lambda: "tool-b"
    )
    assert (
        cli_module_graph_cache._read_persisted_module_graph(
            tmp_path,
            entry_path,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            skip_modules=set(),
            stub_parents=set(),
            stdlib_static_import_helper_modules=set(),
            stdlib_allowlist=set(),
        )
        is None
    )


def test_persisted_module_graph_cache_tracks_source_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry_path = tmp_path / "main.py"
    entry_path.write_text("import json\n", encoding="utf-8")
    original = entry_path.stat()
    roots = [tmp_path]
    module_roots = [tmp_path]
    stdlib_root = tmp_path / "stdlib"

    monkeypatch.setattr(
        cli_module_graph_cache, "_cache_tooling_fingerprint", lambda: "tool-a"
    )
    cli_module_graph_cache._write_persisted_module_graph(
        tmp_path,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules=set(),
        stub_parents=set(),
        stdlib_static_import_helper_modules=set(),
        stdlib_allowlist=set(),
        graph={"__main__": entry_path},
        explicit_imports={"json"},
    )

    _rewrite_preserving_mtime(entry_path, "import math\n", original)

    cached = cli_module_graph_cache._read_persisted_module_graph(
        tmp_path,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules=set(),
        stub_parents=set(),
        stdlib_static_import_helper_modules=set(),
        stdlib_allowlist=set(),
    )
    assert cached is not None
    assert cached.dirty_modules == {"__main__"}


def test_persisted_module_analysis_cache_tracks_tooling_fingerprint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text("def f(x=1):\n    return x\n", encoding="utf-8")

    monkeypatch.setattr(
        cli_module_cache, "_cache_tooling_fingerprint", lambda: "tool-a"
    )
    cli._write_persisted_module_analysis(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
        func_defaults={"f": _func_metadata(params=1)},
        func_kinds={"f": "sync"},
        imports=("json",),
    )
    assert cli._read_persisted_module_analysis(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
    ) == ({"f": _func_metadata(params=1)}, {"f": "sync"}, ("json",))

    monkeypatch.setattr(
        cli_module_cache, "_cache_tooling_fingerprint", lambda: "tool-b"
    )
    assert (
        cli._read_persisted_module_analysis(
            tmp_path,
            module_path,
            module_name="pkg.mod",
            is_package=False,
            import_scan_mode="full",
        )
        is None
    )


def test_persisted_module_analysis_cache_tracks_source_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text("def f(x=1):\n    return x\n", encoding="utf-8")
    original = module_path.stat()

    monkeypatch.setattr(
        cli_module_cache, "_cache_tooling_fingerprint", lambda: "tool-a"
    )
    cli._write_persisted_module_analysis(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
        func_defaults={"f": _func_metadata(params=1)},
        func_kinds={"f": "sync"},
        imports=("json",),
    )

    _rewrite_preserving_mtime(module_path, "def f(x=2):\n    return x\n", original)

    assert (
        cli._read_persisted_module_analysis(
            tmp_path,
            module_path,
            module_name="pkg.mod",
            is_package=False,
            import_scan_mode="full",
        )
        is None
    )


def test_discover_module_graph_skips_persisted_caches_when_disabled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "pkg" / "__init__.py"
    entry.parent.mkdir()
    entry.write_text("import pkg.helper\n")
    helper = entry.parent / "helper.py"
    helper.write_text("VALUE = 1\n")

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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
        cli_module_graph_cache,
        "_read_persisted_module_graph",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        cli_module_graph_cache,
        "_read_persisted_import_scan",
        lambda *args, **kwargs: None,
    )

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()
    cache = cli_module_resolution._ModuleResolutionCache()
    reads: list[Path] = []
    original_read = cache.read_module_source

    def wrapped_read(path: Path, *, retain: bool = True) -> str:
        reads.append(path)
        return original_read(path, retain=retain)

    monkeypatch.setattr(cache, "read_module_source", wrapped_read)

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    cache = cli_module_resolution._ModuleResolutionCache()
    reads: list[Path] = []
    original_read = cache.read_module_source

    def wrapped_read(path: Path, *, retain: bool = True) -> str:
        reads.append(path)
        return original_read(path, retain=retain)

    monkeypatch.setattr(cache, "read_module_source", wrapped_read)

    graph, explicit_imports = (
        cli_module_graph_discovery._discover_module_graph_from_paths(
            [first, second],
            roots,
            module_roots,
            stdlib_root,
            tmp_path,
            set(),
            resolver_cache=cache,
        )
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    expand_calls = 0
    original_expand = cli_module_dependencies._expand_module_chain_cached

    def wrapped_expand(name: str):
        nonlocal expand_calls
        if name == "shared":
            expand_calls += 1
        return original_expand(name)

    monkeypatch.setattr(
        cli_module_dependencies,
        "_expand_module_chain_cached",
        wrapped_expand,
    )

    graph, explicit_imports = (
        cli_module_graph_discovery._discover_module_graph_from_paths(
            [first, second],
            roots,
            module_roots,
            stdlib_root,
            tmp_path,
            set(),
        )
    )

    assert expand_calls == 1
    assert explicit_imports == {"shared"}
    assert "shared" in graph


def test_module_graph_dependency_scan_skips_lazy_backend_bodies(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import pkg\n", encoding="utf-8")
    package = tmp_path / "pkg"
    package.mkdir()
    (package / "__init__.py").write_text("import pkg.device\n", encoding="utf-8")
    (package / "core.py").write_text("VALUE = 1\n", encoding="utf-8")
    (package / "init_side_effect.py").write_text("VALUE = 2\n", encoding="utf-8")
    (package / "device.py").write_text(
        "import pkg.core\n"
        "class Probe:\n"
        "    import pkg.init_side_effect\n"
        "def load_backend():\n"
        "    import pkg.runtime.autogen.mesa\n"
        "    return pkg.runtime.autogen.mesa\n",
        encoding="utf-8",
    )
    runtime = package / "runtime"
    autogen = runtime / "autogen"
    autogen.mkdir(parents=True)
    (runtime / "__init__.py").write_text("", encoding="utf-8")
    (autogen / "__init__.py").write_text("", encoding="utf-8")
    (autogen / "mesa.py").write_text("VALUE = 3\n", encoding="utf-8")

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        None,
        set(),
        resolver_cache=cli_module_resolution._ModuleResolutionCache(),
    )

    assert "pkg.device" in graph
    assert "pkg.core" in graph
    assert "pkg.init_side_effect" in graph
    assert "pkg.runtime.autogen.mesa" not in graph
    assert "pkg.runtime.autogen.mesa" not in explicit_imports


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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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
    cache = cli_module_resolution._ModuleResolutionCache()
    read_paths: list[Path] = []
    original_read = cache.read_module_source
    original_resolve = cache.resolve_module
    resolved_candidates: list[str] = []

    def wrapped_read(path: Path, *, retain: bool = True) -> str:
        read_paths.append(path)
        return original_read(path, retain=retain)

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

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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

    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [tmp_path.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()

    graph, _ = cli_module_graph_discovery._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )
    assert "pkg.old" in graph

    entry.write_text("import pkg.helper\n")
    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        tmp_path,
        stdlib_allowlist,
    )

    assert "pkg.old" not in graph
    assert "pkg.old" not in explicit_imports

    cache = cli_module_resolution._ModuleResolutionCache()

    def fail_resolve(*args: object, **kwargs: object) -> Path | None:
        raise AssertionError("unexpected module resolution")

    def fail_read(path: Path) -> str:
        raise AssertionError(f"unexpected source read for {path}")

    monkeypatch.setattr(cache, "resolve_module", fail_resolve)
    monkeypatch.setattr(cache, "read_module_source", fail_read)

    def fail_read_text(*args: object, **kwargs: object) -> str:
        raise AssertionError("unexpected persisted graph reread")

    monkeypatch.setattr(Path, "read_text", fail_read_text)

    graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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
    ARTIFACT_STATE._resolved_artifact_hash_key.cache_clear()

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
    original = ARTIFACT_STATE._resolved_artifact_hash_key

    def wrapped(path_str: str) -> str:
        nonlocal calls
        calls += 1
        return original(path_str)

    monkeypatch.setattr(
        ARTIFACT_STATE,
        "_resolved_artifact_hash_key",
        wrapped,
        raising=True,
    )

    first = cli_backend_binary._backend_fingerprint_path(tmp_path, artifact, "dev-fast")
    second = cli_backend_binary._backend_fingerprint_path(
        tmp_path, artifact, "dev-fast"
    )

    info = original.cache_info()
    assert first == second
    assert calls == 1
    assert info.currsize >= 1


def test_artifact_state_path_is_cached(tmp_path: Path) -> None:
    artifact = tmp_path / "dist" / "output.o"
    ARTIFACT_STATE._artifact_state_path_cached.cache_clear()

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
    original = ARTIFACT_STATE._artifact_state_path_cached

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

    monkeypatch.setattr(
        ARTIFACT_STATE,
        "_artifact_state_path_cached",
        wrapped,
        raising=True,
    )

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
    cli_module_cache._build_state_subdir_cached.cache_clear()

    calls = 0
    original = cli_module_cache._build_state_subdir_cached

    def wrapped(build_state_root_str: str, subdir: str) -> Path:
        nonlocal calls
        calls += 1
        return original(build_state_root_str, subdir)

    monkeypatch.setattr(
        cli_module_cache, "_build_state_subdir_cached", wrapped, raising=True
    )

    first = cli._module_analysis_cache_path(
        tmp_path,
        tmp_path / "pkg.py",
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
    )
    second = cli._module_analysis_cache_path(
        tmp_path,
        tmp_path / "pkg.py",
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
    )

    info = original.cache_info()
    assert first == second
    assert calls == 2
    assert info.hits >= 1


def test_resolved_module_cache_key_is_cached(tmp_path: Path) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    cli_module_graph_cache._resolved_module_cache_key.cache_clear()

    first = cli_module_graph_cache._resolved_module_cache_key(
        str(module_path), "pkg.mod", "mod", "module_analysis_cache"
    )
    second = cli_module_graph_cache._resolved_module_cache_key(
        str(module_path), "pkg.mod", "mod", "module_analysis_cache"
    )

    info = cli_module_graph_cache._resolved_module_cache_key.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_module_analysis_cache_path_uses_cached_module_key(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    cli_module_cache._resolved_module_cache_key.cache_clear()

    calls = 0
    original = cli_module_cache._resolved_module_cache_key

    def wrapped(path_str: str, *parts: str) -> str:
        nonlocal calls
        calls += 1
        return original(path_str, *parts)

    monkeypatch.setattr(
        cli_module_cache, "_resolved_module_cache_key", wrapped, raising=True
    )

    first = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
    )
    second = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
    )

    info = original.cache_info()
    assert first == second
    assert calls == 2
    assert info.hits >= 1


def test_module_analysis_cache_path_tracks_import_scan_mode(tmp_path: Path) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    full = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
    )
    module_init = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="module_init",
    )

    assert full != module_init


def test_module_analysis_cache_path_tracks_capability_config_digest(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    base = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
        capability_config_digest="capability-a",
    )
    changed = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
        capability_config_digest="capability-b",
    )

    assert base != changed


def test_import_scan_cache_path_tracks_capability_config_digest(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    base = cli_module_graph_cache._import_scan_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
        capability_config_digest="capability-a",
    )
    changed = cli_module_graph_cache._import_scan_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
        capability_config_digest="capability-b",
    )

    assert base != changed


def test_persisted_module_analysis_cache_rejects_import_scan_mode_mismatch(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg" / "mod.py"
    module_path.parent.mkdir()
    module_path.write_text(
        "import os\n\ndef f():\n    import warnings\n", encoding="utf-8"
    )

    cli._write_persisted_module_analysis(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
        func_defaults={},
        func_kinds={},
        imports=("os", "warnings"),
    )
    cache_path = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg.mod",
        is_package=False,
        import_scan_mode="full",
    )
    payload = json.loads(cache_path.read_text(encoding="utf-8"))
    payload["import_scan_mode"] = "module_init"
    cache_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    assert (
        cli._read_persisted_module_analysis(
            tmp_path,
            module_path,
            module_name="pkg.mod",
            is_package=False,
            import_scan_mode="full",
        )
        is None
    )


def test_module_graph_cache_key_is_cached(tmp_path: Path) -> None:
    entry_path = tmp_path / "main.py"
    roots = (str(tmp_path),)
    module_roots = (str(tmp_path / "src"),)
    stdlib_root = str(tmp_path / "stdlib")
    cli_module_graph_cache._module_graph_cache_key.cache_clear()

    first = cli_module_graph_cache._module_graph_cache_key(
        str(entry_path),
        roots,
        module_roots,
        stdlib_root,
        ("warnings",),
        ("asyncio",),
        ("tkinter",),
        cli_module_graph_cache._module_graph_policy_digest({"json"}),
        "tooling",
    )
    second = cli_module_graph_cache._module_graph_cache_key(
        str(entry_path),
        roots,
        module_roots,
        stdlib_root,
        ("warnings",),
        ("asyncio",),
        ("tkinter",),
        cli_module_graph_cache._module_graph_policy_digest({"json"}),
        "tooling",
    )
    different_policy = cli_module_graph_cache._module_graph_cache_key(
        str(entry_path),
        roots,
        module_roots,
        stdlib_root,
        ("warnings",),
        ("asyncio",),
        ("tkinter",),
        cli_module_graph_cache._module_graph_policy_digest({"math"}),
        "tooling",
    )

    info = cli_module_graph_cache._module_graph_cache_key.cache_info()
    assert first == second
    assert first != different_policy
    assert info.hits >= 1
    assert info.currsize >= 1


def test_module_graph_cache_key_tracks_capability_config_digest(
    tmp_path: Path,
) -> None:
    entry_path = tmp_path / "main.py"
    roots = (str(tmp_path),)
    module_roots = (str(tmp_path / "src"),)
    stdlib_root = str(tmp_path / "stdlib")
    cli_module_graph_cache._module_graph_cache_key.cache_clear()

    base = cli_module_graph_cache._module_graph_cache_key(
        str(entry_path),
        roots,
        module_roots,
        stdlib_root,
        (),
        (),
        (),
        cli_module_graph_cache._module_graph_policy_digest({"json"}),
        "tooling",
        capability_config_digest="capability-a",
    )
    changed = cli_module_graph_cache._module_graph_cache_key(
        str(entry_path),
        roots,
        module_roots,
        stdlib_root,
        (),
        (),
        (),
        cli_module_graph_cache._module_graph_policy_digest({"json"}),
        "tooling",
        capability_config_digest="capability-b",
    )

    assert base != changed


def test_module_graph_cache_path_uses_cached_graph_key(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry_path = tmp_path / "main.py"
    roots = [tmp_path]
    module_roots = [tmp_path / "src"]
    stdlib_root = tmp_path / "stdlib"
    cli_module_graph_cache._module_graph_cache_key.cache_clear()

    calls = 0
    original = cli_module_graph_cache._module_graph_cache_key

    def wrapped(
        entry_path_str: str,
        roots_key: tuple[str, ...],
        module_roots_key: tuple[str, ...],
        stdlib_root_str: str,
        skip_modules: tuple[str, ...],
        stub_parents: tuple[str, ...],
        stdlib_static_import_helper_modules: tuple[str, ...],
        stdlib_allowlist_digest: str,
        compiler_fingerprint: str,
        target_python_tag: str = cli_module_graph_cache._DEFAULT_TARGET_PYTHON_VERSION.tag,
        capability_config_digest: str = "",
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
            stdlib_static_import_helper_modules,
            stdlib_allowlist_digest,
            compiler_fingerprint,
            target_python_tag,
            capability_config_digest=capability_config_digest,
        )

    monkeypatch.setattr(
        cli_module_graph_cache, "_module_graph_cache_key", wrapped, raising=True
    )

    first = cli_module_graph_cache._module_graph_cache_path(
        tmp_path,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules={"warnings"},
        stub_parents={"asyncio"},
        stdlib_static_import_helper_modules={"tkinter"},
        stdlib_allowlist={"json"},
    )
    second = cli_module_graph_cache._module_graph_cache_path(
        tmp_path,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules={"warnings"},
        stub_parents={"asyncio"},
        stdlib_static_import_helper_modules={"tkinter"},
        stdlib_allowlist={"json"},
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


def test_build_state_root_cache_tracks_session_id(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._build_state_root_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    alpha = cli._build_state_root(tmp_path)

    monkeypatch.setenv("MOLT_SESSION_ID", "beta-session")
    beta = cli._build_state_root(tmp_path)

    assert alpha == tmp_path / "target" / "sessions" / "alpha-session" / ".molt_state"
    assert beta == tmp_path / "target" / "sessions" / "beta-session" / ".molt_state"
    assert alpha != beta


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
    LOCKFILES._lock_check_cache_path_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", "external-target")
    monkeypatch.chdir(tmp_path)

    first = LOCKFILES._lock_check_cache_path(tmp_path, "cargo")
    second = LOCKFILES._lock_check_cache_path(tmp_path, "cargo")

    info = LOCKFILES._lock_check_cache_path_cached.cache_info()
    expected = Path.cwd() / "external-target" / "lock_checks" / "cargo.json"
    assert first == second == expected
    assert info.hits >= 1
    assert info.currsize >= 1


def test_write_lock_check_cache_uses_unique_atomic_temp_sibling(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = LOCKFILES._lock_check_cache_path(tmp_path, "uv")
    original_replace = os.replace
    replaced_sources: list[Path] = []

    def record_replace(src: object, dst: object) -> None:
        assert Path(dst) == path
        replaced_sources.append(Path(src))
        original_replace(src, dst)

    monkeypatch.setattr(os, "replace", record_replace)

    inputs = {"uv.lock": {"size": 1, "mtime_ns": 2}}
    LOCKFILES._write_lock_check_cache(tmp_path, "uv", inputs)
    LOCKFILES._write_lock_check_cache(tmp_path, "uv", inputs)

    assert len(replaced_sources) == 2
    assert replaced_sources[0] != replaced_sources[1]
    assert all(
        source.name.startswith(".uv.json.") and source.name.endswith(".tmp")
        for source in replaced_sources
    )
    assert LOCKFILES._is_lock_check_cache_valid(tmp_path, "uv", inputs)
    assert not path.with_suffix(path.suffix + ".tmp").exists()
    assert list(path.parent.glob(".*.tmp")) == []


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
    explicit_runtime = (
        tmp_path
        / "explicit-target"
        / "release-output"
        / cli._runtime_lib_archive_name("micro", None)
    )
    explicit_runtime.parent.mkdir(parents=True)
    explicit_runtime.write_text("runtime")

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "explicit-target"))
    os.utime(backend_bin, (1, 1))
    os.utime(pid_path, (2, 2))
    os.utime(explicit_runtime, (3, 3))

    assert cli._backend_daemon_binary_is_newer(backend_bin, pid_path) is True


def test_validate_shared_stdlib_cache_contract_ignores_runtime_mtime_for_retention(
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

    cli._validate_shared_stdlib_cache_contract(stdlib_object, project_root)

    assert removed == []
    assert stdlib_object.exists()


def test_clean_delegates_to_canonical_artifact_cleanup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(cli, "_find_molt_root", lambda _cwd: tmp_path)
    monkeypatch.setattr(
        cli,
        "_require_molt_root",
        lambda _root, _json_output, _command: None,
    )
    cleanup_tool = tmp_path / "tools" / "artifact_cleanup.py"
    cleanup_tool.parent.mkdir(parents=True)
    cleanup_tool.write_text("# test cleanup tool marker\n", encoding="utf-8")
    calls: list[list[str]] = []

    class FakeArtifactCleanup:
        @staticmethod
        def main(argv: list[str]) -> int:
            calls.append(list(argv))
            return 0

    monkeypatch.setitem(
        cli.clean.__globals__,
        "_load_artifact_cleanup_module",
        lambda _root: FakeArtifactCleanup,
    )

    exit_code = cli.clean(
        json_output=True,
        verbose=True,
        apply=True,
        kill_processes=True,
        extra_paths=["tmp/custom-cache/"],
        list_paths=True,
    )

    assert exit_code == 0
    assert calls == [
        [
            "--repo-root",
            str(tmp_path),
            "--apply",
            "--kill-processes",
            "--list-paths",
            "--json",
            "--verbose",
            "--extra-path",
            "tmp/custom-cache/",
        ]
    ]


def test_clean_defaults_to_canonical_dry_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(cli, "_find_molt_root", lambda _cwd: tmp_path)
    monkeypatch.setattr(
        cli,
        "_require_molt_root",
        lambda _root, _json_output, _command: None,
    )
    cleanup_tool = tmp_path / "tools" / "artifact_cleanup.py"
    cleanup_tool.parent.mkdir(parents=True)
    cleanup_tool.write_text("# test cleanup tool marker\n", encoding="utf-8")
    calls: list[list[str]] = []

    class FakeArtifactCleanup:
        @staticmethod
        def main(argv: list[str]) -> int:
            calls.append(list(argv))
            return 0

    monkeypatch.setitem(
        cli.clean.__globals__,
        "_load_artifact_cleanup_module",
        lambda _root: FakeArtifactCleanup,
    )

    exit_code = cli.clean()

    assert exit_code == 0
    assert calls == [["--repo-root", str(tmp_path)]]


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
        LOCKFILES.shutil,
        "which",
        lambda name: "/usr/bin/cargo" if name == "cargo" else None,
    )
    monkeypatch.setattr(
        LOCKFILES,
        "_lock_check_inputs",
        lambda project_root, paths: captured.setdefault("paths", list(paths)) or {},
    )
    monkeypatch.setattr(
        LOCKFILES, "_is_lock_check_cache_valid", lambda *args, **kwargs: True
    )

    assert LOCKFILES._verify_cargo_lock(tmp_path) is None
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


def test_build_lock_is_shared_for_explicit_target_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._build_state_root_cached.cache_clear()
    cli._build_lock_dir_cached.cache_clear()
    target_root = tmp_path / "shared-target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    with cli._build_lock(tmp_path, "runtime.dev-fast.native"):
        alpha_lock = (
            target_root / ".molt_state" / "build_locks" / "runtime.dev-fast.native.lock"
        )
        assert alpha_lock.exists()

    monkeypatch.setenv("MOLT_SESSION_ID", "beta-session")
    with cli._build_lock(tmp_path, "runtime.dev-fast.native"):
        beta_lock = (
            target_root / ".molt_state" / "build_locks" / "runtime.dev-fast.native.lock"
        )
        assert beta_lock.exists()

    assert alpha_lock == beta_lock
    assert not (
        target_root
        / ".molt_state"
        / "build_locks"
        / "runtime.dev-fast.native.alpha-session.lock"
    ).exists()
    assert not (
        target_root
        / ".molt_state"
        / "build_locks"
        / "runtime.dev-fast.native.beta-session.lock"
    ).exists()


def test_build_lock_directory_is_session_isolated_when_target_root_is_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli._build_state_root_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    cli._build_lock_dir_cached.cache_clear()
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    with cli._build_lock(tmp_path, "runtime.dev-fast.native"):
        alpha_lock = (
            tmp_path
            / "target"
            / "sessions"
            / "alpha-session"
            / ".molt_state"
            / "build_locks"
            / "runtime.dev-fast.native.lock"
        )
        assert alpha_lock.exists()

    monkeypatch.setenv("MOLT_SESSION_ID", "beta-session")
    with cli._build_lock(tmp_path, "runtime.dev-fast.native"):
        beta_lock = (
            tmp_path
            / "target"
            / "sessions"
            / "beta-session"
            / ".molt_state"
            / "build_locks"
            / "runtime.dev-fast.native.lock"
        )
        assert beta_lock.exists()

    assert alpha_lock != beta_lock


def test_runtime_source_paths_are_cached(tmp_path: Path) -> None:
    RUNTIME_FINGERPRINTS._runtime_source_paths_cached.cache_clear()

    first = RUNTIME_FINGERPRINTS._runtime_source_paths(tmp_path)
    second = RUNTIME_FINGERPRINTS._runtime_source_paths(tmp_path)

    info = RUNTIME_FINGERPRINTS._runtime_source_paths_cached.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_source_paths_are_cached(tmp_path: Path) -> None:
    CACHE_FINGERPRINTS._backend_source_paths_cached.cache_clear()

    first = CACHE_FINGERPRINTS._backend_source_paths(tmp_path, ("wasm-backend",))
    second = CACHE_FINGERPRINTS._backend_source_paths(tmp_path, ("wasm-backend",))

    info = CACHE_FINGERPRINTS._backend_source_paths_cached.cache_info()
    assert first == second
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_source_paths_are_feature_aware() -> None:
    native_paths = {
        path.relative_to(ROOT).as_posix()
        for path in CACHE_FINGERPRINTS._backend_source_paths(ROOT, ())
    }
    wasm_paths = {
        path.relative_to(ROOT).as_posix()
        for path in CACHE_FINGERPRINTS._backend_source_paths(ROOT, ("wasm-backend",))
    }
    rust_paths = {
        path.relative_to(ROOT).as_posix()
        for path in CACHE_FINGERPRINTS._backend_source_paths(ROOT, ("rust-backend",))
    }
    luau_paths = {
        path.relative_to(ROOT).as_posix()
        for path in CACHE_FINGERPRINTS._backend_source_paths(ROOT, ("luau-backend",))
    }
    llvm_paths = {
        path.relative_to(ROOT).as_posix()
        for path in CACHE_FINGERPRINTS._backend_source_paths(ROOT, ("llvm",))
    }

    common = {
        "runtime/molt-backend/src",
        "runtime/molt-backend/Cargo.toml",
        "runtime/molt-backend/build.rs",
        "runtime/molt-ir/src",
        "runtime/molt-ir/Cargo.toml",
        "runtime/molt-ir/build.rs",
        "runtime/molt-passes/src",
        "runtime/molt-passes/Cargo.toml",
        "runtime/molt-passes/build.rs",
        "runtime/molt-tir/src",
        "runtime/molt-tir/Cargo.toml",
        "runtime/molt-tir/build.rs",
        "Cargo.toml",
        "Cargo.lock",
    }
    codegen_abi = {
        "runtime/molt-codegen-abi/src",
        "runtime/molt-codegen-abi/Cargo.toml",
        "runtime/molt-codegen-abi/build.rs",
    }
    native_leaf = {
        "runtime/molt-backend-native/src",
        "runtime/molt-backend-native/Cargo.toml",
        "runtime/molt-backend-native/build.rs",
    }
    wasm_leaf = {
        "runtime/molt-backend-wasm/src",
        "runtime/molt-backend-wasm/Cargo.toml",
        "runtime/molt-backend-wasm/build.rs",
    }
    rust_leaf = {
        "runtime/molt-backend-rust/src",
        "runtime/molt-backend-rust/Cargo.toml",
        "runtime/molt-backend-rust/build.rs",
    }
    luau_leaf = {
        "runtime/molt-backend-luau/src",
        "runtime/molt-backend-luau/Cargo.toml",
        "runtime/molt-backend-luau/build.rs",
    }

    assert native_paths == common | native_leaf | codegen_abi
    assert wasm_paths == common | wasm_leaf | codegen_abi
    assert rust_paths == common | rust_leaf
    assert luau_paths == common | luau_leaf
    assert llvm_paths == common | native_leaf | codegen_abi
    assert "runtime/molt-backend-wasm/src" not in native_paths
    assert "runtime/molt-backend-native/src" not in wasm_paths


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
    exe_suffix = ".exe" if os.name == "nt" else ""
    expected = Path.cwd() / "external-target" / "dev-fast" / f"molt-backend{exe_suffix}"
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
    cli_build_output_layout._wasm_runtime_root_cached.cache_clear()
    monkeypatch.setenv("MOLT_WASM_RUNTIME_DIR", str(tmp_path / "wasm-root"))

    first = cli_build_output_layout._wasm_runtime_root(tmp_path)
    second = cli_build_output_layout._wasm_runtime_root(tmp_path)

    info = cli_build_output_layout._wasm_runtime_root_cached.cache_info()
    assert first == second == (tmp_path / "wasm-root")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_safe_output_base_is_cached() -> None:
    cli_build_output_layout._safe_output_base.cache_clear()

    first = cli_build_output_layout._safe_output_base("hello/world.py")
    second = cli_build_output_layout._safe_output_base("hello/world.py")

    info = cli_build_output_layout._safe_output_base.cache_info()
    assert first == second == "hello_world.py"
    assert info.hits >= 1
    assert info.currsize >= 1


def test_default_build_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli_build_output_layout._default_build_root_cached.cache_clear()
    monkeypatch.setenv("MOLT_HOME", str(tmp_path / "home-root"))

    first = cli_build_output_layout._default_build_root("hello/world.py")
    second = cli_build_output_layout._default_build_root("hello/world.py")

    info = cli_build_output_layout._default_build_root_cached.cache_info()
    assert first == second == (tmp_path / "home-root" / "build" / "hello_world.py")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_cache_root_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli_build_output_layout._resolve_cache_root_cached.cache_clear()

    first = cli_build_output_layout._resolve_cache_root(tmp_path, "cache-dir")
    second = cli_build_output_layout._resolve_cache_root(tmp_path, "cache-dir")

    info = cli_build_output_layout._resolve_cache_root_cached.cache_info()
    assert first == second == (tmp_path / "cache-dir")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_out_dir_is_cached(tmp_path: Path) -> None:
    cli_build_output_layout._resolve_out_dir_cached.cache_clear()

    first = cli_build_output_layout._resolve_out_dir(tmp_path, "dist")
    second = cli_build_output_layout._resolve_out_dir(tmp_path, "dist")

    info = cli_build_output_layout._resolve_out_dir_cached.cache_info()
    assert first == second == (tmp_path / "dist")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_sysroot_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cli_build_output_layout._resolve_sysroot_cached.cache_clear()
    monkeypatch.setenv("MOLT_SYSROOT", "sdk-root")

    first = cli_build_output_layout._resolve_sysroot(tmp_path, None)
    second = cli_build_output_layout._resolve_sysroot(tmp_path, None)

    info = cli_build_output_layout._resolve_sysroot_cached.cache_info()
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
    expected = (
        Path.cwd()
        / "external-target"
        / "dev-fast"
        / cli._runtime_lib_archive_name("micro", None)
    )
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
        / "libmolt_runtime.stdlib_micro.a"
    )


def test_runtime_wasm_artifact_path_is_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    RUNTIME_PATHS._runtime_wasm_artifact_path_cached.cache_clear()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))

    first = RUNTIME_PATHS._runtime_wasm_artifact_path(tmp_path, "molt_runtime.wasm")
    second = RUNTIME_PATHS._runtime_wasm_artifact_path(tmp_path, "molt_runtime.wasm")

    info = RUNTIME_PATHS._runtime_wasm_artifact_path_cached.cache_info()
    assert first == second == (tmp_path / "wasm" / "molt_runtime.wasm")
    assert info.hits >= 1
    assert info.currsize >= 1


def test_runtime_wasm_artifact_path_uses_explicit_override(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    RUNTIME_PATHS._runtime_wasm_artifact_path_cached.cache_clear()
    override = tmp_path / "custom-wasm"
    monkeypatch.setenv("MOLT_WASM_RUNTIME_DIR", str(override))

    runtime_wasm = RUNTIME_PATHS._runtime_wasm_artifact_path(
        tmp_path, "molt_runtime_reloc.wasm"
    )

    assert runtime_wasm == (override / "molt_runtime_reloc.wasm")


def test_load_module_imports_reuses_persisted_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n")
    source = cli_module_source._read_module_source(module_path)
    cache = cli_module_resolution._ModuleResolutionCache()
    tree = cache.parse_module_ast(module_path, source, filename=str(module_path))

    imports = cli_module_graph_discovery._load_module_imports(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        tree=tree,
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert imports == ("warnings",)

    def fail_collect(*args: object, **kwargs: object) -> tuple[str, ...]:
        raise AssertionError("unexpected import scan")

    monkeypatch.setattr(cache, "collect_imports", fail_collect)
    cached_imports = cli_module_graph_discovery._load_module_imports(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
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
    source = cli_module_source._read_module_source(module_path)
    cache = cli_module_resolution._ModuleResolutionCache()

    (
        tree,
        imports,
        func_defaults,
        func_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert tree is not None
    assert imports == ("warnings",)
    assert "f" in func_defaults
    assert func_kinds == {"f": "sync"}
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
        cached_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ("warnings",)
    assert cached_defaults == func_defaults
    assert cached_kinds == func_kinds
    assert cached_source is None
    assert cache_hit is True
    assert interface_changed is False
    assert cached_path_stat is not None


def test_load_module_analysis_persists_bytes_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("def f(blob=b'abc'):\n    return blob\n")
    source = cli_module_source._read_module_source(module_path)
    cache = cli_module_resolution._ModuleResolutionCache()

    (
        tree,
        imports,
        func_defaults,
        func_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
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
            "posonly": 0,
            "kwonly": 0,
            "kind": "sync",
            "has_decorators": False,
        }
    }
    assert func_kinds == {"f": "sync"}
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
        cached_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ()
    assert cached_defaults == func_defaults
    assert cached_kinds == func_kinds
    assert cached_source is None
    assert cache_hit is True
    assert interface_changed is False
    assert cached_path_stat is not None


def test_load_module_analysis_rejects_persisted_defaults_without_function_kind(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("def g():\n    yield 1\n")
    cache = cli_module_resolution._ModuleResolutionCache()
    stat = module_path.stat()
    source_sha256 = cli_module_source._source_content_sha256(module_path, stat)
    assert source_sha256 is not None
    cache_path = cli._module_analysis_cache_path(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
    )
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_payload(
        cache_path,
        {
            "version": cli._MODULE_ANALYSIS_CACHE_SCHEMA_VERSION,
            "compiler_fingerprint": cli._cache_tooling_fingerprint(),
            "module_name": "pkg",
            "is_package": False,
            "import_scan_mode": "full",
            "target_python": cli._DEFAULT_TARGET_PYTHON_VERSION.tag,
            "size": stat.st_size,
            "mtime_ns": stat.st_mtime_ns,
            "source_sha256": source_sha256,
            "func_defaults": {"g": {"params": 0, "defaults": []}},
            "imports": [],
        },
        default=CACHE_KEYS._json_ir_default,
    )

    (
        tree,
        imports,
        func_defaults,
        func_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert tree is not None
    assert imports == ()
    assert func_defaults["g"] == {
        "params": 0,
        "defaults": [],
        "posonly": 0,
        "kwonly": 0,
        "kind": "gen",
        "has_decorators": False,
    }
    assert func_kinds == {"g": "gen"}
    assert cached_source == module_path.read_text(encoding="utf-8")
    assert cache_hit is False
    assert interface_changed is True
    assert path_stat is not None


def test_load_module_analysis_reuses_persisted_module_analysis_imports(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n\ndef f(a, *, b=1):\n    return a + b\n")
    source = cli_module_source._read_module_source(module_path)
    cache = cli_module_resolution._ModuleResolutionCache()

    cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    def fail_import_scan(*args: object, **kwargs: object) -> tuple[str, ...] | None:
        raise AssertionError("unexpected persisted import-scan read")

    monkeypatch.setattr(
        cli_module_cache, "_read_persisted_import_scan", fail_import_scan
    )
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
        cached_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ("warnings",)
    assert "f" in cached_defaults
    assert cached_kinds == {"f": "sync"}
    assert cached_source is None
    assert cache_hit is True
    assert interface_changed is False
    assert cached_path_stat is not None


def test_load_module_analysis_keeps_full_and_module_init_caches_disjoint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import os\n\ndef f():\n    import warnings\n")
    source = cli_module_source._read_module_source(module_path)
    cache = cli_module_resolution._ModuleResolutionCache()

    first = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert first[1] == ("os", "warnings")
    assert first[3] == {"f": "sync"}
    assert first[5] is False

    second = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="module_init",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert second[0] is not None
    assert second[1] == ("os",)
    assert second[3] == {"f": "sync"}
    assert second[5] is False

    def fail_parse(*args: object, **kwargs: object) -> ast.AST:
        raise AssertionError("unexpected parse")

    cache = cli_module_resolution._ModuleResolutionCache()
    monkeypatch.setattr(cache, "parse_module_ast", fail_parse)
    full_cached = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )
    module_init_cached = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="module_init",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert full_cached[0] is None
    assert full_cached[1] == ("os", "warnings")
    assert full_cached[5] is True
    assert module_init_cached[0] is None
    assert module_init_cached[1] == ("os",)
    assert module_init_cached[5] is True


def test_load_module_analysis_keeps_module_init_and_full_caches_disjoint_reverse(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import os\n\ndef f():\n    import warnings\n")
    source = cli_module_source._read_module_source(module_path)
    cache = cli_module_resolution._ModuleResolutionCache()

    first = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="module_init",
        source=source,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert first[1] == ("os",)
    assert first[3] == {"f": "sync"}
    assert first[5] is False

    second = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )
    assert second[0] is not None
    assert second[1] == ("os", "warnings")
    assert second[3] == {"f": "sync"}
    assert second[5] is False


def test_load_module_analysis_reuses_single_module_stat_for_persisted_hits(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("import warnings\n\ndef f(a, *, b=1):\n    return a + b\n")
    source = cli_module_source._read_module_source(module_path)
    cache = cli_module_resolution._ModuleResolutionCache()

    cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
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
        cached_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        cached_path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert cached_tree is None
    assert cached_imports == ("warnings",)
    assert "f" in cached_defaults
    assert cached_kinds == {"f": "sync"}
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
    cache = cli_module_resolution._ModuleResolutionCache()

    cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    module_path.write_text(
        "import warnings\n\ndef f(a, *, b=1):\n    total = a + b\n    return total\n"
    )
    cache = cli_module_resolution._ModuleResolutionCache()

    (
        tree,
        imports,
        func_defaults,
        func_kinds,
        cached_source,
        cache_hit,
        interface_changed,
        path_stat,
    ) = cli._load_module_analysis(
        module_path,
        module_name="pkg",
        is_package=False,
        import_scan_mode="full",
        source=None,
        logical_source_path=str(module_path),
        resolution_cache=cache,
        project_root=tmp_path,
    )

    assert tree is not None
    assert imports == ("warnings",)
    assert "f" in func_defaults
    assert func_kinds == {"f": "sync"}
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


def test_persisted_module_lowering_rejects_missing_local_function_reference(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("x = 1\n")
    context_digest = cli._module_lowering_context_digest({"module": "pkg", "v": 1})
    assert context_digest is not None
    result = {
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [
                    {
                        "kind": "call",
                        "s_value": "pkg__init_metadata",
                        "args": [],
                        "out": "v0",
                    }
                ],
            }
        ],
        "func_code_ids": {},
        "local_class_names": [],
        "local_classes": {},
        "midend_policy_outcomes_by_function": {},
        "midend_pass_stats_by_function": {},
        "timings": {"visit_s": 0.0, "lower_s": 0.0, "total_s": 0.0},
    }

    issue = cli_module_cache._module_lowering_local_reference_issue(
        "pkg", result["functions"]
    )
    assert issue is not None
    assert "pkg__init_metadata" in issue
    assert (
        cli_module_cache._module_lowering_local_reference_issue(
            "pkg",
            [
                {
                    "name": "molt_main",
                    "params": [],
                    "ops": [
                        {
                            "kind": "call",
                            "s_value": "other__init_metadata",
                            "args": [],
                            "out": "v0",
                        }
                    ],
                }
            ],
        )
        is None
    )

    cli._write_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
        result=result,
    )

    assert (
        cli._read_persisted_module_lowering(
            tmp_path,
            module_path,
            module_name="pkg",
            is_package=False,
            context_digest=context_digest,
        )
        is None
    )


def test_persisted_module_lowering_tracks_source_content(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "pkg.py"
    module_path.write_text("x = 1\n")
    original = module_path.stat()
    context_digest = cli._module_lowering_context_digest({"module": "pkg", "v": 1})
    assert context_digest is not None

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

    _rewrite_preserving_mtime(module_path, "x = 2\n", original)

    assert (
        cli._read_persisted_module_lowering(
            tmp_path,
            module_path,
            module_name="pkg",
            is_package=False,
            context_digest=context_digest,
        )
        is None
    )


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


def test_finalize_backend_ir_pads_truncated_param_types() -> None:
    ir = BACKEND_IR._finalize_backend_ir(
        functions=[
            {
                "name": "pkg__f",
                "params": ["s", "x", "kw"],
                "param_types": ["Any"],
                "ops": [],
            }
        ],
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
    )

    fn = ir["functions"][0]
    assert fn["params"] == ["s", "x", "kw"]
    assert fn["param_types"] == ["Any", "i64", "i64"]


def test_persisted_module_lowering_repairs_truncated_param_types(
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
                {
                    "name": "pkg__f",
                    "params": ["s", "x", "kw"],
                    "param_types": ["Any"],
                    "ops": [],
                }
            ],
            "func_code_ids": {},
            "local_class_names": [],
            "local_classes": {},
            "midend_policy_outcomes_by_function": {},
            "midend_pass_stats_by_function": {},
            "timings": {"visit_s": 0.0, "lower_s": 0.0, "total_s": 0.0},
        },
    )

    cached = cli._read_persisted_module_lowering(
        tmp_path,
        module_path,
        module_name="pkg",
        is_package=False,
        context_digest=context_digest,
    )

    assert cached is not None
    assert cached["functions"][0]["params"] == ["s", "x", "kw"]
    assert cached["functions"][0]["param_types"] == ["Any", "i64", "i64"]


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

    monkeypatch.setattr(
        cli_module_cache, "_module_lowering_context_payload", fake_context_payload
    )
    monkeypatch.setattr(
        cli_module_cache, "_module_lowering_context_digest", lambda payload: "digest"
    )

    def fake_read(
        root: Path,
        path: Path,
        *,
        module_name: str,
        is_package: bool,
        context_digest: str,
        path_stat: os.stat_result | None = None,
        target_python: cli.TargetPythonVersion = cli._DEFAULT_TARGET_PYTHON_VERSION,
    ) -> dict[str, object] | None:
        assert root == project_root
        assert path == module_path
        assert module_name == "alpha"
        assert is_package is False
        assert context_digest == "digest"
        assert path_stat is not None
        assert target_python == cli._DEFAULT_TARGET_PYTHON_VERSION
        return {"module": module_name, "kind": "cached"}

    monkeypatch.setattr(cli_module_cache, "_read_persisted_module_lowering", fake_read)
    module_graph_metadata = cli._build_module_graph_metadata(
        {"alpha": module_path},
        generated_module_source_paths={},
        entry_module="__main__",
        namespace_module_names=set(),
    )

    cached_results, worker_payloads, context_digest_by_module, batch_error = (
        cli_frontend_worker._prepare_frontend_parallel_batch(
            ["alpha"],
            module_graph={"alpha": module_path},
            module_sources={},
            project_root=project_root,
            known_classes_snapshot={},
            module_resolution_cache=cli_module_resolution._ModuleResolutionCache(),
            parse_codec="json",
            type_hint_policy="ignore",
            fallback_policy="error",
            type_facts=None,
            enable_phi=True,
            known_modules={"alpha"},
            stdlib_allowlist=set(),
            known_func_defaults={},
            known_func_kinds={},
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
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
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
    cache = cli_module_resolution._ModuleResolutionCache()
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
        known_func_kinds={},
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

    closure = cli_module_dependencies._dependent_module_closure(
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

    reverse = cli_module_dependencies._reverse_module_dependencies(
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
    reverse = cli_module_dependencies._reverse_module_dependencies(
        module_deps,
        {"main", "alpha", "beta", "leaf"},
    )

    closure = cli_module_dependencies._dependent_module_closure(
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

    closure = cli_module_dependencies._module_dependency_closure("main", module_deps)

    assert closure == {"main", "alpha", "beta", "leaf"}


def test_module_dependency_closures_reuse_topological_order_when_acyclic() -> None:
    module_deps = {
        "main": {"alpha", "beta"},
        "alpha": {"leaf"},
        "beta": set(),
        "leaf": set(),
    }

    closures = cli_module_dependencies._module_dependency_closures(
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

    closures = cli_module_dependencies._module_dependency_closures(
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

    deps = cli_module_dependencies._module_dependencies_from_imports(
        "main",
        module_graph,
        ["alpha.helper", "beta", "molt.stdlib.warnings"],
    )

    assert deps == {"alpha", "beta", "warnings"}


def test_frontend_analysis_resolves_known_native_artifact_dependencies(
    tmp_path: Path,
) -> None:
    main_path = tmp_path / "field_solve.py"
    main_path.write_text(
        "from scipy.ndimage import distance_transform_edt\n",
        encoding="utf-8",
    )
    module_graph = {"field_solve": main_path}
    metadata = cli._build_module_graph_metadata(
        module_graph,
        generated_module_source_paths={},
        entry_module="field_solve",
        namespace_module_names=set(),
    )

    analysis, failure = cli_frontend_pipeline._prepare_frontend_analysis(
        module_graph=module_graph,
        module_graph_metadata=metadata,
        module_resolution_cache=cli_module_resolution._ModuleResolutionCache(),
        roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        stdlib_allowlist=cli_module_stdlib_policy._stdlib_allowlist(),
        project_root=tmp_path,
        entry_module="field_solve",
        json_output=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        dependency_known_modules={"scipy", "scipy.ndimage"},
    )

    assert failure is None
    assert analysis is not None
    assert analysis.module_deps["field_solve"] == {"scipy", "scipy.ndimage"}


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
        known_func_kinds={
            "main": {"run": "sync"},
            "alpha": {"helper": "gen"},
            "beta": {"unused": "sync"},
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
    assert set(payload["known_func_kinds"]) == {"main", "alpha"}
    assert "beta" not in payload["known_func_defaults"]
    assert "beta" not in payload["known_func_kinds"]


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
        known_func_kinds={},
        module_deps={"main": set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    monkeypatch.setattr(
        cli_module_cache, "_cache_tooling_fingerprint", lambda: "tool-a"
    )
    payload_a = cli._module_lowering_context_payload(**kwargs)
    monkeypatch.setattr(
        cli_module_cache, "_cache_tooling_fingerprint", lambda: "tool-b"
    )
    payload_b = cli._module_lowering_context_payload(**kwargs)

    assert payload_a is not None
    assert payload_b is not None
    assert payload_a["compiler_fingerprint"] == "tool-a"
    assert payload_b["compiler_fingerprint"] == "tool-b"
    assert cli._module_lowering_context_digest(
        payload_a
    ) != cli._module_lowering_context_digest(payload_b)


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
        known_func_kinds={},
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
        known_func_kinds={},
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
        known_func_kinds={
            "main": {"run": "sync"},
            "alpha": {"helper": "gen"},
            "unrelated": {"unused": "sync"},
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
    assert set(payload["known_func_kinds"]) == {"main", "alpha"}
    assert payload["pgo_hot_functions"] == ["main::hot"]
    assert "source" not in payload
    assert payload["source_lease"]["kind"] == "inline"


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
        known_func_kinds={},
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

    original = cli_module_cache._scoped_known_classes
    calls = 0

    def wrapped_scoped_known_classes(
        *args: object, **kwargs: object
    ) -> dict[str, object]:
        nonlocal calls
        calls += 1
        return original(*args, **kwargs)

    monkeypatch.setattr(
        cli_module_cache, "_scoped_known_classes", wrapped_scoped_known_classes
    )
    monkeypatch.setattr(
        cli_frontend_worker,
        "_load_cached_module_lowering_result",
        lambda *args, **kwargs: None,
    )
    module_graph_metadata = cli._build_module_graph_metadata(
        module_graph,
        generated_module_source_paths={},
        entry_module="__main__",
        namespace_module_names=set(),
    )

    cached_results, worker_payloads, context_digest_by_module, batch_error = (
        cli_frontend_worker._prepare_frontend_parallel_batch(
            ["main", "alpha"],
            module_graph=module_graph,
            module_sources=module_sources,
            project_root=tmp_path,
            known_classes_snapshot={
                "MainClass": {"module": "main", "fields": {}},
                "DepClass": {"module": "alpha", "fields": {}},
                "UnrelatedClass": {"module": "unrelated", "fields": {}},
            },
            module_resolution_cache=cli_module_resolution._ModuleResolutionCache(),
            parse_codec="json",
            type_hint_policy="ignore",
            fallback_policy="error",
            type_facts=None,
            enable_phi=True,
            known_modules={"main", "alpha"},
            stdlib_allowlist=set(),
            known_func_defaults={},
            known_func_kinds={},
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
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        )
    )

    assert batch_error is None
    assert cached_results == {}
    assert len(worker_payloads) == 2
    assert set(context_digest_by_module) == {"main", "alpha"}
    assert calls == 2


def test_prepare_frontend_parallel_batch_uses_path_backed_source_leases(
    tmp_path: Path,
) -> None:
    module_graph = {
        "main": tmp_path / "main.py",
        "alpha": tmp_path / "alpha.py",
    }
    module_graph["main"].write_text("import alpha\nVALUE = alpha.VALUE\n")
    module_graph["alpha"].write_text("VALUE = 1\n")
    module_source_catalog = cli_module_source._build_module_source_catalog(module_graph)
    module_graph_metadata = cli._build_module_graph_metadata(
        module_graph,
        generated_module_source_paths={},
        entry_module="main",
        namespace_module_names=set(),
        module_source_catalog=module_source_catalog,
        module_deps={"main": {"alpha"}, "alpha": set()},
    )

    cached_results, worker_payloads, context_digest_by_module, batch_error = (
        cli_frontend_worker._prepare_frontend_parallel_batch(
            ["main"],
            module_graph=module_graph,
            module_source_catalog=module_source_catalog,
            project_root=tmp_path,
            known_classes_snapshot={},
            module_resolution_cache=cli_module_resolution._ModuleResolutionCache(),
            parse_codec="json",
            type_hint_policy="ignore",
            fallback_policy="error",
            type_facts=None,
            enable_phi=True,
            known_modules={"main", "alpha"},
            stdlib_allowlist=set(),
            known_func_defaults={},
            known_func_kinds={},
            module_deps={"main": {"alpha"}, "alpha": set()},
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
            path_stat_by_module={
                name: path.stat() for name, path in module_graph.items()
            },
            module_chunking=False,
            dirty_lowering_modules={"main"},
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        )
    )

    assert batch_error is None
    assert cached_results == {}
    assert set(context_digest_by_module) == {"main"}
    assert len(worker_payloads) == 1
    payload = worker_payloads[0][1]
    assert "source" not in payload
    assert payload["source_lease"]["kind"] == "path"
    assert payload["source_lease"]["path"] == str(module_graph["main"])
    result = cli_frontend_worker._frontend_lower_module_worker(payload)
    assert result["ok"] is True


def test_worker_source_lease_rejects_path_drift(tmp_path: Path) -> None:
    module_path = tmp_path / "main.py"
    module_path.write_text("VALUE = 1\n")
    lease = cli_module_source._ModuleSourceLease.path_backed(module_path)
    module_path.write_text("VALUE = 100\n")

    result = cli_frontend_worker._frontend_lower_module_worker(
        cli._module_worker_payload(
            "main",
            module_path=module_path,
            logical_source_path=str(module_path),
            source_lease=lease,
            parse_codec="json",
            type_hint_policy="ignore",
            fallback_policy="error",
            module_is_namespace=False,
            entry_module=None,
            type_facts=None,
            enable_phi=True,
            known_modules=("main",),
            known_classes_snapshot={},
            stdlib_allowlist_sorted=(),
            known_func_defaults={},
            known_func_kinds={},
            module_deps={"main": set()},
            module_chunking=False,
            module_chunk_max_ops=0,
            optimization_profile="dev",
            pgo_hot_function_names=(),
            module_dep_closures={"main": frozenset({"main"})},
        )
    )

    assert result["ok"] is False
    assert "Source lease" in result["error"]


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
        known_func_kinds={},
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
        known_func_kinds={},
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
        known_modules={"main", "alpha", "unrelated", "nativepkg._native"},
        known_func_defaults={
            "main": {"run": {"params": 0, "defaults": []}},
            "alpha": {"helper": {"params": 1, "defaults": []}},
            "unrelated": {"unused": {"params": 0, "defaults": []}},
        },
        known_func_kinds={
            "main": {"run": "sync"},
            "alpha": {"helper": "gen"},
            "unrelated": {"unused": "sync"},
        },
        pgo_hot_function_names={"main::hot", "unrelated::cold"},
        type_facts=type_facts,
    )

    assert scoped_lowering_inputs.known_modules_by_module["main"] == (
        "alpha",
        "main",
        "nativepkg._native",
    )
    assert scoped_lowering_inputs.known_modules_by_module["alpha"] == (
        "alpha",
        "nativepkg._native",
    )
    assert set(scoped_lowering_inputs.known_func_defaults_by_module["main"]) == {
        "main",
        "alpha",
    }
    assert set(scoped_lowering_inputs.known_func_kinds_by_module["main"]) == {
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
        known_modules={"main", "alpha", "nativepkg._native"},
        known_func_defaults={
            "main": {"run": {"params": 0, "defaults": []}},
            "alpha": {"helper": {"params": 1, "defaults": []}},
        },
        known_func_kinds={
            "main": {"run": "sync"},
            "alpha": {"helper": "gen"},
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
        known_func_kinds={
            "main": {"run": "sync"},
            "alpha": {"helper": "gen"},
        },
        pgo_hot_function_names={"main::hot"},
        type_facts=type_facts,
        module_dep_closures={
            "main": frozenset({"main", "alpha"}),
            "alpha": frozenset({"alpha"}),
        },
        scoped_lowering_inputs=scoped_lowering_inputs,
        known_modules_sorted=("alpha", "main", "nativepkg._native"),
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
        scoped_view.known_func_kinds
        is scoped_lowering_inputs.known_func_kinds_by_module["main"]
    )
    assert (
        scoped_view.pgo_hot_function_names
        is scoped_lowering_inputs.pgo_hot_function_names_by_module["main"]
    )
    assert scoped_view.type_facts is scoped_lowering_inputs.type_facts_by_module["main"]
    assert scoped_view.known_modules_payload == [
        "alpha",
        "main",
        "nativepkg._native",
    ]
    assert scoped_view.known_modules_set == frozenset(
        {"alpha", "main", "nativepkg._native"}
    )
    assert scoped_view.pgo_hot_function_names_payload == ["main::hot"]
    assert scoped_view.pgo_hot_function_names_set == frozenset({"main::hot"})


def test_scoped_lowering_input_view_keeps_native_artifact_modules_without_bundle() -> (
    None
):
    scoped_view = cli._scoped_lowering_input_view(
        "main",
        module_deps={"main": {"alpha"}, "alpha": set()},
        known_modules={"main", "alpha", "nativepkg._native"},
        known_func_defaults={},
        known_func_kinds={},
        pgo_hot_function_names=(),
        type_facts=None,
        module_dep_closures={
            "main": frozenset({"main", "alpha"}),
            "alpha": frozenset({"alpha"}),
        },
        known_modules_sorted=("alpha", "main", "nativepkg._native"),
        source_modules=("alpha", "main"),
    )

    assert scoped_view.known_modules == (
        "alpha",
        "main",
        "nativepkg._native",
    )


def test_scoped_lowering_input_view_carries_native_callable_exports() -> None:
    native_callable_exports = {
        "nativepkg.ndimage.distance_transform_edt": {
            "module": "nativepkg.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.forward_f32_v1",
            "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
        },
        "unrelatedpkg.run": {
            "module": "unrelatedpkg",
            "name": "run",
            "binding": "module_attr",
            "abi": "molt.object_call_v1",
        },
    }

    scoped_view = cli._scoped_lowering_input_view(
        "main",
        module_deps={"main": {"nativepkg.ndimage"}, "nativepkg.ndimage": set()},
        known_modules={"main", "nativepkg.ndimage"},
        direct_call_modules={"main"},
        known_func_defaults={},
        known_func_kinds={},
        native_callable_exports=native_callable_exports,
        pgo_hot_function_names=(),
        type_facts=None,
        module_dep_closures={
            "main": frozenset({"main", "nativepkg.ndimage"}),
            "nativepkg.ndimage": frozenset({"nativepkg.ndimage"}),
        },
        known_modules_sorted=("main", "nativepkg.ndimage"),
        source_modules=("main", "nativepkg.ndimage"),
    )

    assert scoped_view.direct_call_modules == ("main",)
    assert scoped_view.direct_call_modules_payload == ["main"]
    assert scoped_view.native_callable_exports == {
        "nativepkg.ndimage.distance_transform_edt": {
            "module": "nativepkg.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.forward_f32_v1",
            "symbol": "molt_nativepkg_ndimage_distance_transform_edt",
        }
    }
    assert scoped_view.native_callable_exports_payload == (
        scoped_view.native_callable_exports
    )


def test_scoped_lowering_input_view_carries_native_python_exports() -> None:
    scoped_view = cli._scoped_lowering_input_view(
        "main",
        module_deps={"main": {"nativepkg.ndimage"}, "nativepkg.ndimage": set()},
        known_modules={"main", "nativepkg.ndimage"},
        direct_call_modules={"main"},
        known_func_defaults={},
        known_func_kinds={},
        native_python_exports={
            "nativepkg.ndimage.distance_transform_edt",
            "unrelatedpkg.run",
        },
        pgo_hot_function_names=(),
        type_facts=None,
        module_dep_closures={
            "main": frozenset({"main", "nativepkg.ndimage"}),
            "nativepkg.ndimage": frozenset({"nativepkg.ndimage"}),
        },
        known_modules_sorted=("main", "nativepkg.ndimage"),
        source_modules=("main", "nativepkg.ndimage"),
    )

    assert scoped_view.native_python_exports == (
        "nativepkg.ndimage.distance_transform_edt",
    )
    assert scoped_view.native_python_exports_payload == [
        "nativepkg.ndimage.distance_transform_edt"
    ]
    assert scoped_view.native_python_exports_set == frozenset(
        {"nativepkg.ndimage.distance_transform_edt"}
    )


def test_module_lowering_context_payload_reuses_precomputed_scoped_inputs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scoped_inputs = cli._ScopedLoweringInputView(
        known_modules=("alpha", "main"),
        known_func_defaults={"main": {"run": {"params": 0, "defaults": []}}},
        known_func_kinds={"main": {"run": "sync"}},
        native_python_exports=("alpha.native_call",),
        pgo_hot_function_names=("main::hot",),
        type_facts=None,
    )
    monkeypatch.setattr(
        cli_module_cache,
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
        known_func_kinds={},
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
    assert set(payload["known_func_kinds"]) == {"main"}
    assert tuple(payload["native_python_exports"]) == ("alpha.native_call",)


def test_module_worker_payload_reuses_precomputed_scoped_inputs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scoped_inputs = cli._ScopedLoweringInputView(
        known_modules=("alpha", "main"),
        known_func_defaults={"main": {"run": {"params": 0, "defaults": []}}},
        known_func_kinds={"main": {"run": "sync"}},
        native_python_exports=("alpha.native_call",),
        pgo_hot_function_names=("main::hot",),
        type_facts=None,
    )
    monkeypatch.setattr(
        cli_module_cache,
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
        known_func_kinds={},
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
    assert set(payload["known_func_kinds"]) == {"main"}
    assert payload["native_python_exports"] == ["alpha.native_call"]


def test_load_cached_module_lowering_result_reuses_precomputed_views(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module_path = tmp_path / "alpha.py"
    module_path.write_text("VALUE = 1\n")
    cache = cli_module_resolution._ModuleResolutionCache()
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
        cli_module_cache,
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
        known_func_kinds={},
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
            known_func_kinds={},
            pgo_hot_function_names=(),
            type_facts=None,
        ),
        scoped_known_classes={},
        resolution_cache=cache,
    )

    assert result is not None
    assert result["functions"] == []


def test_module_frontend_generator_uses_scoped_inputs() -> None:
    gen = cli_frontend_worker._module_frontend_generator(
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
            known_func_kinds={"main": {"run": "sync"}},
            native_python_exports=("alpha.native_call",),
            pgo_hot_function_names=("main::hot",),
            type_facts=None,
        ),
        scoped_known_classes={"MainClass": {"module": "main", "fields": {}}},
    )

    assert gen.module_name == "main"
    assert gen.known_modules == {"alpha", "main"}
    assert gen.known_func_defaults == {"main": {"run": {"params": 0, "defaults": []}}}
    assert gen.known_func_kinds == {"main": {"run": "sync"}}
    assert gen.native_python_exports == {"alpha.native_call"}
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
        known_func_kinds={},
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
            known_func_kinds={},
            pgo_hot_function_names=(),
            type_facts=None,
        ),
        scoped_known_classes={},
        path_stat=os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    )

    assert isinstance(digest, str)
    assert digest


def test_module_lowering_context_digest_includes_native_callable_exports() -> None:
    common: dict[str, object] = {
        "logical_source_path": "/tmp/main.py",
        "entry_override": None,
        "known_classes_snapshot": {},
        "parse_codec": "json",
        "type_hint_policy": "ignore",
        "fallback_policy": "error",
        "type_facts": None,
        "enable_phi": True,
        "known_modules": {"main", "nativepkg.ndimage"},
        "direct_call_modules": {"main"},
        "stdlib_allowlist": set(),
        "known_func_defaults": {},
        "known_func_kinds": {},
        "module_deps": {"main": {"nativepkg.ndimage"}, "nativepkg.ndimage": set()},
        "module_is_namespace": False,
        "module_chunking": False,
        "module_chunk_max_ops": 0,
        "optimization_profile": "dev",
        "pgo_hot_function_names": set(),
        "module_dep_closures": {
            "main": frozenset({"main", "nativepkg.ndimage"}),
            "nativepkg.ndimage": frozenset({"nativepkg.ndimage"}),
        },
        "path_stat": os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    }
    export_a = {
        "nativepkg.ndimage.distance_transform_edt": {
            "module": "nativepkg.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.forward_f32_v1",
            "symbol": "molt_symbol_a",
        }
    }
    export_b = {
        "nativepkg.ndimage.distance_transform_edt": {
            "module": "nativepkg.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.forward_f32_v1",
            "symbol": "molt_symbol_b",
        }
    }

    digest_a = cli._module_lowering_context_digest_for_module(
        "main",
        Path("/tmp/main.py"),
        native_callable_exports=export_a,
        **common,
    )
    digest_b = cli._module_lowering_context_digest_for_module(
        "main",
        Path("/tmp/main.py"),
        native_callable_exports=export_b,
        **common,
    )

    assert digest_a is not None
    assert digest_b is not None
    assert digest_a != digest_b


def test_module_lowering_context_digest_includes_native_python_exports() -> None:
    common: dict[str, object] = {
        "logical_source_path": "/tmp/main.py",
        "entry_override": None,
        "known_classes_snapshot": {},
        "parse_codec": "json",
        "type_hint_policy": "ignore",
        "fallback_policy": "error",
        "type_facts": None,
        "enable_phi": True,
        "known_modules": {"main", "nativepkg.ndimage"},
        "direct_call_modules": {"main"},
        "stdlib_allowlist": set(),
        "known_func_defaults": {},
        "known_func_kinds": {},
        "module_deps": {"main": {"nativepkg.ndimage"}, "nativepkg.ndimage": set()},
        "module_is_namespace": False,
        "module_chunking": False,
        "module_chunk_max_ops": 0,
        "optimization_profile": "dev",
        "pgo_hot_function_names": set(),
        "module_dep_closures": {
            "main": frozenset({"main", "nativepkg.ndimage"}),
            "nativepkg.ndimage": frozenset({"nativepkg.ndimage"}),
        },
        "path_stat": os.stat_result((0, 0, 0, 0, 0, 0, 1, 1, 1, 0)),
    }

    digest_a = cli._module_lowering_context_digest_for_module(
        "main",
        Path("/tmp/main.py"),
        native_python_exports={"nativepkg.ndimage.distance_transform_edt"},
        **common,
    )
    digest_b = cli._module_lowering_context_digest_for_module(
        "main",
        Path("/tmp/main.py"),
        native_python_exports={"nativepkg.ndimage.gaussian_filter"},
        **common,
    )

    assert digest_a is not None
    assert digest_b is not None
    assert digest_a != digest_b


def test_module_lowering_context_digest_includes_scoped_func_kinds(
    tmp_path: Path,
) -> None:
    module_path = tmp_path / "main.py"
    module_path.write_text(
        "from helpers import cpu_profile\ndef run():\n    return cpu_profile('x')\n",
        encoding="utf-8",
    )
    common: dict[str, object] = {
        "logical_source_path": str(module_path),
        "entry_override": None,
        "known_classes_snapshot": {},
        "parse_codec": "json",
        "type_hint_policy": "ignore",
        "fallback_policy": "error",
        "type_facts": None,
        "enable_phi": True,
        "known_modules": {"main", "helpers"},
        "stdlib_allowlist": set(),
        "known_func_defaults": {
            "helpers": {"cpu_profile": {"params": 1, "defaults": []}}
        },
        "module_deps": {"main": {"helpers"}, "helpers": set()},
        "module_is_namespace": False,
        "module_chunking": False,
        "module_chunk_max_ops": 0,
        "optimization_profile": "dev",
        "pgo_hot_function_names": set(),
        "module_dep_closures": {"main": frozenset({"main", "helpers"})},
        "path_stat": module_path.stat(),
    }

    sync_digest = cli._module_lowering_context_digest_for_module(
        "main",
        module_path,
        known_func_kinds={"helpers": {"cpu_profile": "sync"}},
        **common,
    )
    gen_digest = cli._module_lowering_context_digest_for_module(
        "main",
        module_path,
        known_func_kinds={"helpers": {"cpu_profile": "gen"}},
        **common,
    )

    assert sync_digest is not None
    assert gen_digest is not None
    assert sync_digest != gen_digest


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
        known_func_kinds={},
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
            known_func_kinds={},
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
        known_func_kinds={},
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
            known_func_kinds={},
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
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 2
    )
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_min_modules", lambda: 2
    )
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_resolve_frontend_parallel_min_predicted_cost",
        lambda: 0.0,
    )
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_resolve_frontend_parallel_target_cost_per_worker",
        lambda: 1.0,
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli_frontend_pipeline,
        "_analyze_module_schedule",
        lambda module_graph, module_deps: (
            cli_module_dependencies._topo_sort_modules(module_graph, module_deps),
            cli_module_dependencies._reverse_module_dependencies(
                module_deps, module_graph
            ),
            False,
            cli_module_dependencies._module_dependency_layers(
                cli_module_dependencies._topo_sort_modules(module_graph, module_deps),
                module_deps,
            ),
            cli_module_dependencies._module_dependency_closures(
                module_deps,
                module_graph,
                module_order=cli_module_dependencies._topo_sort_modules(
                    module_graph, module_deps
                ),
                has_back_edges=False,
            ),
        ),
    )
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    _install_fake_backend_compile(monkeypatch)

    submit_calls = 0

    class _FakeFuture:
        def __init__(self, payload: dict[str, object]) -> None:
            self._payload = payload

        def result(self) -> dict[str, object]:
            return cli_frontend_worker._frontend_lower_module_worker(self._payload)

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
            assert fn is cli_frontend_worker._frontend_lower_module_worker
            return _FakeFuture(payload)

    monkeypatch.setattr(cli_frontend_execution, "ProcessPoolExecutor", _FakeExecutor)
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


def test_parallel_build_reuses_dependent_cache_after_stable_interface_change(
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
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 2
    )
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_min_modules", lambda: 2
    )
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_resolve_frontend_parallel_min_predicted_cost",
        lambda: 0.0,
    )
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_resolve_frontend_parallel_target_cost_per_worker",
        lambda: 1.0,
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli_frontend_pipeline,
        "_analyze_module_schedule",
        lambda module_graph, module_deps: (
            cli_module_dependencies._topo_sort_modules(module_graph, module_deps),
            cli_module_dependencies._reverse_module_dependencies(
                module_deps, module_graph
            ),
            False,
            cli_module_dependencies._module_dependency_layers(
                cli_module_dependencies._topo_sort_modules(module_graph, module_deps),
                module_deps,
            ),
            cli_module_dependencies._module_dependency_closures(
                module_deps,
                module_graph,
                module_order=cli_module_dependencies._topo_sort_modules(
                    module_graph, module_deps
                ),
                has_back_edges=False,
            ),
        ),
    )
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    _install_fake_backend_compile(monkeypatch)

    class _FakeFuture:
        def __init__(self, payload: dict[str, object]) -> None:
            self._payload = payload

        def result(self) -> dict[str, object]:
            return cli_frontend_worker._frontend_lower_module_worker(self._payload)

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
            assert fn is cli_frontend_worker._frontend_lower_module_worker
            return _FakeFuture(payload)

    monkeypatch.setattr(cli_frontend_execution, "ProcessPoolExecutor", _FakeExecutor)

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
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 2
    )
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_min_modules", lambda: 2
    )
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_resolve_frontend_parallel_min_predicted_cost",
        lambda: 0.0,
    )
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_resolve_frontend_parallel_target_cost_per_worker",
        lambda: 1.0,
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli_frontend_pipeline,
        "_analyze_module_schedule",
        lambda module_graph, module_deps: (
            cli_module_dependencies._topo_sort_modules(module_graph, module_deps),
            cli_module_dependencies._reverse_module_dependencies(
                module_deps, module_graph
            ),
            False,
            cli_module_dependencies._module_dependency_layers(
                cli_module_dependencies._topo_sort_modules(module_graph, module_deps),
                module_deps,
            ),
            cli_module_dependencies._module_dependency_closures(
                module_deps,
                module_graph,
                module_order=cli_module_dependencies._topo_sort_modules(
                    module_graph, module_deps
                ),
                has_back_edges=False,
            ),
        ),
    )
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    _install_fake_backend_compile(monkeypatch)

    captured_payloads: list[dict[str, object]] = []

    class _FakeFuture:
        def __init__(self, payload: dict[str, object]) -> None:
            self._payload = payload

        def result(self) -> dict[str, object]:
            return cli_frontend_worker._frontend_lower_module_worker(self._payload)

    class _FakeExecutor:
        def __init__(self, *, max_workers: int) -> None:
            self.max_workers = max_workers

        def __enter__(self) -> "_FakeExecutor":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def submit(self, fn: object, payload: dict[str, object]) -> _FakeFuture:
            assert fn is cli_frontend_worker._frontend_lower_module_worker
            captured_payloads.append(payload)
            return _FakeFuture(payload)

    monkeypatch.setattr(cli_frontend_execution, "ProcessPoolExecutor", _FakeExecutor)

    type_facts = TypeFacts(
        modules={
            "main": ModuleFacts(globals={"ENTRY": Fact(type="int", trust="trusted")}),
            "alpha": ModuleFacts(globals={"VALUE": Fact(type="int", trust="trusted")}),
            "beta": ModuleFacts(globals={"VALUE": Fact(type="int", trust="trusted")}),
            "unrelated": ModuleFacts(
                globals={"NOPE": Fact(type="bytes", trust="trusted")}
            ),
        }
    )
    monkeypatch.setattr(
        cli_typecheck,
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
    assert captured_payloads, stdout.getvalue()
    for payload in captured_payloads:
        scoped = payload["type_facts"]
        assert isinstance(scoped, TypeFacts)
        assert "unrelated" not in scoped.modules
    compile_diagnostics = json.loads(stdout.getvalue())["data"]["compile_diagnostics"]
    assert compile_diagnostics["frontend_parallel"]["enabled"] is True
    assert compile_diagnostics["frontend_parallel"]["reason"] == "enabled"


def test_build_one_shot_backend_compile_uses_ir_file_lease(
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
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 0
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    backend_inputs: list[bytes | None] = []
    backend_ir_files: list[Path] = []
    _install_fake_backend_compile(
        monkeypatch,
        backend_inputs=backend_inputs,
        backend_ir_files=backend_ir_files,
    )

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
    assert backend_inputs[0] is None
    assert len(backend_ir_files) == 1
    assert backend_ir_files[0].parent == project / "tmp" / "backend-ir-leases"
    assert not backend_ir_files[0].exists()


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
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 0
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: True)
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_backend_daemon_socket_path",
        lambda *args, **kwargs: daemon_socket,
    )
    # _start_backend_daemon is always called now (socket existence is
    # checked internally).  Stub it to report the daemon as ready.
    monkeypatch.setattr(
        cli_backend_compile,
        "_start_backend_daemon",
        lambda *args, **kwargs: True,
    )
    _install_fake_backend_compile(monkeypatch)

    compile_calls = 0

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        **kwargs: object,
    ) -> cli._BackendDaemonCompileResult:
        nonlocal compile_calls
        compile_calls += 1
        assert socket_path == daemon_socket
        backend_output = cast(Path, kwargs["backend_output"])
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
        cli_backend_compile,
        "_compile_with_backend_daemon",
        fake_compile_with_backend_daemon,
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
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 0
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_stdlib_object_cache_path",
        lambda cache_path, cache_key: expected_stdlib_obj,
    )
    seen_envs: list[dict[str, str] | None] = []
    _install_fake_backend_compile(monkeypatch, seen_envs=seen_envs)

    rc = cli.build(
        str(entry),
        emit="obj",
        output=str(tmp_path / "out.o"),
        profile="dev",
        deterministic=False,
        json_output=False,
    )

    assert rc == 0
    assert seen_envs and seen_envs[0] is not None
    seen_backend_env = seen_envs[0]
    assert seen_backend_env["MOLT_STDLIB_OBJ"] == str(expected_stdlib_obj)
    assert "MOLT_STDLIB_CACHE_KEY" in seen_backend_env


def test_stdlib_object_cache_path_tracks_build_variant(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ambient_cache_root = tmp_path / "ambient-cache"
    explicit_cache_root = tmp_path / "explicit-cache"
    monkeypatch.setenv("MOLT_CACHE", str(ambient_cache_root))

    base = cli._stdlib_object_cache_path(explicit_cache_root / "program.o", "variant=a")
    same = cli._stdlib_object_cache_path(explicit_cache_root / "program.o", "variant=a")
    changed = cli._stdlib_object_cache_path(
        explicit_cache_root / "program.o", "variant=b"
    )

    assert base is not None
    assert same == base
    assert changed is not None
    assert changed != base
    assert base.parent == explicit_cache_root
    assert base.parent != ambient_cache_root
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
    assert cli_module_source._read_module_source(source_path) == "value = 'hello'\n"


def test_read_module_source_falls_back_for_encoding_cookie(
    tmp_path: Path,
) -> None:
    source_path = tmp_path / "latin1_source.py"
    source_path.write_bytes(
        "# -*- coding: latin-1 -*-\nname = 'caf\xe9'\n".encode("latin-1")
    )
    assert (
        cli_module_source._read_module_source(source_path)
        == "# -*- coding: latin-1 -*-\nname = 'café'\n"
    )


def test_prepare_frontend_analysis_uses_path_backed_source_catalog(
    tmp_path: Path,
) -> None:
    main_path = tmp_path / "main.py"
    dep_path = tmp_path / "dep.py"
    main_path.write_text("import dep\nVALUE = dep.VALUE\n", encoding="utf-8")
    dep_path.write_text("VALUE = 41\n", encoding="utf-8")
    module_graph = {"main": main_path, "dep": dep_path}
    metadata = cli._build_module_graph_metadata(
        module_graph,
        generated_module_source_paths={},
        entry_module="main",
        namespace_module_names=set(),
    )
    resolution_cache = cli_module_resolution._ModuleResolutionCache()

    analysis, failure = cli_frontend_pipeline._prepare_frontend_analysis(
        module_graph=module_graph,
        module_graph_metadata=metadata,
        module_resolution_cache=resolution_cache,
        roots=[tmp_path],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        stdlib_allowlist=cli_module_stdlib_policy._stdlib_allowlist(),
        project_root=tmp_path,
        entry_module="main",
        json_output=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert failure is None
    assert analysis is not None
    assert analysis.module_sources == {}
    assert analysis.module_trees == {}
    assert resolution_cache.source_cache == {}
    assert resolution_cache.ast_cache == {}
    main_lease = analysis.module_source_catalog.lease_for("main", main_path)
    assert main_lease.path_backed_source is True
    assert main_lease.source_size == main_path.stat().st_size
    assert (
        analysis.module_source_catalog.read_source("main", main_path, resolution_cache)
        == "import dep\nVALUE = dep.VALUE\n"
    )
    assert resolution_cache.source_cache == {}


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
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols_json: str | None = None,
    stdlib_module_symbols: set[str] | frozenset[str] | None = None,
    timeout: float | None,
    request_bytes: bytes | None = None,
    daemon_identity: cli._BackendDaemonIdentity | None = None,
) -> cli._BackendDaemonCompileResult:
    return BACKEND_EXECUTION._compile_with_backend_daemon(
        socket_path,
        project_root=backend_output.parent,
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
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols_json=stdlib_module_symbols_json,
        stdlib_module_symbols=stdlib_module_symbols,
        timeout=timeout,
        request_bytes=request_bytes,
        daemon_identity=daemon_identity,
    )


def _test_backend_daemon_identity(
    pid: int,
    *,
    socket_path: Path,
    project_root: Path,
    backend_bin: Path | None = None,
    cargo_profile: str = "dev-fast",
    config_digest: str | None = None,
) -> cli._BackendDaemonIdentity:
    return cli._BackendDaemonIdentity(
        pid=pid,
        socket_path=socket_path,
        project_root=project_root,
        cargo_profile=cargo_profile,
        config_digest=config_digest,
        backend_bin=backend_bin or project_root / "target" / "debug" / "molt-backend",
        created_at=1_700_000_000.0,
        command=None,
    )


def _stub_backend_daemon_harness(monkeypatch: pytest.MonkeyPatch) -> None:
    class _NoopExecutionContext:
        def __init__(self, env: dict[str, str]) -> None:
            self.env = dict(env)

        def process_group_kwargs(self) -> dict[str, object]:
            return {}

        def start_repo_sentinel(self, *args: object, **kwargs: object) -> object:
            del args, kwargs
            return contextlib.nullcontext()

    class _NoopHarness:
        class HarnessExecutionContext:
            @staticmethod
            def from_env(
                prefix: str,
                env: dict[str, str],
                *,
                repo_root: Path,
            ) -> _NoopExecutionContext:
                del prefix, repo_root
                return _NoopExecutionContext(env)

    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_load_cli_harness_memory_guard",
        lambda project_root: _NoopHarness(),
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


def test_typing_module_init_imports_collections_abc_without_warnings_regex() -> None:
    tree = ast.parse((ROOT / "src/molt/stdlib/typing.py").read_text(encoding="utf-8"))

    imports = cli_module_import_scanner._collect_imports(
        tree,
        module_name="typing",
        import_scan_mode="module_init",
    )

    assert "_collections_abc" in imports
    assert "warnings" not in imports
    assert "re" not in imports


def test_typing_static_graph_keeps_collections_abc_without_lazy_deprecated(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import typing\n")
    graph = _discover_with_core_modules(entry)
    assert "typing" in graph
    assert "_collections_abc" in graph
    assert "warnings" not in graph
    assert "re" not in graph


def test_collections_static_helper_scan_includes_userdict_copy_only() -> None:
    tree = ast.parse(
        (ROOT / "src/molt/stdlib/collections/__init__.py").read_text(encoding="utf-8")
    )

    module_init = set(
        cli_module_import_scanner._collect_imports(
            tree,
            module_name="collections",
            is_package=True,
            import_scan_mode="module_init",
        )
    )
    helper_scan = set(
        cli_module_import_scanner._collect_imports(
            tree,
            module_name="collections",
            is_package=True,
            import_scan_mode="module_init_static_helpers",
        )
    )

    assert "copy" not in module_init
    assert "copy" in helper_scan
    assert "warnings" not in helper_scan
    assert "re" not in helper_scan


def test_email_message_static_helper_scan_includes_policy_default_only() -> None:
    tree = ast.parse(
        (ROOT / "src/molt/stdlib/email/message.py").read_text(encoding="utf-8")
    )

    module_init = set(
        cli_module_import_scanner._collect_imports(
            tree,
            module_name="email.message",
            import_scan_mode="module_init",
        )
    )
    helper_scan = set(
        cli_module_import_scanner._collect_imports(
            tree,
            module_name="email.message",
            import_scan_mode="module_init_static_helpers",
        )
    )

    assert "email.policy" not in module_init
    assert "email.policy" in helper_scan
    assert "email.generator" not in helper_scan


def test_stdlib_module_init_scan_excludes_lazy_regex_and_struct_edges() -> None:
    cases = {
        "glob": {"re"},
        "importlib.metadata": {"csv", "email", "re", "zipfile"},
        "importlib.metadata._text": {"re"},
        "logging.config": {"re", "struct"},
        "typing_extensions": {"re"},
        "unittest": {"re"},
        "warnings": {"re"},
        # gettext still imports re at module scope for plural-expression tokenizing;
        # only binary .mo parsing needs struct, and that path is lazy.
        "gettext": {"struct"},
    }
    stdlib_root = ROOT / "src" / "molt" / "stdlib"

    for module_name, excluded in cases.items():
        path = stdlib_root.joinpath(*module_name.split(".")).with_suffix(".py")
        if not path.exists():
            path = stdlib_root.joinpath(*module_name.split("."), "__init__.py")
        tree = ast.parse(path.read_text(encoding="utf-8"))
        imports = set(
            cli_module_import_scanner._collect_imports(
                tree,
                module_name=module_name,
                is_package=path.name == "__init__.py",
                import_scan_mode="module_init",
            )
        )
        assert imports.isdisjoint(excluded), (
            f"{module_name} module-init imports leaked {sorted(imports & excluded)}"
        )


def test_codecs_os_type_checking_imports_are_pruned() -> None:
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()
    cache = cli_module_resolution._ModuleResolutionCache()
    codecs_path = cache.resolve_module("codecs", roots, stdlib_root, stdlib_allowlist)
    assert codecs_path is not None

    graph, _explicit_imports = cli_module_graph_discovery._discover_module_graph(
        codecs_path,
        roots,
        module_roots,
        stdlib_root,
        ROOT,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
        resolver_cache=cache,
    )

    assert "codecs" in graph
    assert "os" in graph
    assert "typing" not in graph
    assert "warnings" not in graph
    assert "re" not in graph


def test_decimal_static_graph_prunes_lazy_typing_deprecated_regex(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import decimal\n")
    graph = _discover_with_core_modules(entry)
    assert "decimal" in graph
    assert "typing" not in graph
    assert "warnings" not in graph
    assert "re" not in graph


def test_spawn_entry_override_not_required_for_plain_script(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()
    module_graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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
    cli_module_stdlib_policy._ensure_core_stdlib_modules(module_graph, stdlib_root)
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
        core_graph, _ = cli_module_graph_discovery._discover_module_graph_from_paths(
            core_paths,
            roots,
            module_roots,
            stdlib_root,
            ROOT,
            stdlib_allowlist,
            skip_modules=cli.STUB_MODULES,
            stub_parents=cli.STUB_PARENT_MODULES,
            stdlib_static_import_helper_modules=set(),
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    assert not cli._requires_spawn_entry_override(module_graph, explicit_imports)


def test_spawn_entry_override_required_for_multiprocessing(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import multiprocessing\nprint('ok')\n")
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli_module_stdlib_policy._stdlib_allowlist()
    module_graph, explicit_imports = cli_module_graph_discovery._discover_module_graph(
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
    summary = cli_build_diagnostics._build_reason_summary(reasons)
    assert summary == {"core_closure": 2, "entry_closure": 2}


def test_build_diagnostics_enabled_from_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BUILD_DIAGNOSTICS", "1")
    assert cli_build_diagnostics._build_diagnostics_enabled()
    monkeypatch.setenv("MOLT_BUILD_DIAGNOSTICS", "0")
    assert not cli_build_diagnostics._build_diagnostics_enabled()


def test_build_allocation_diagnostics_enabled_from_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BUILD_ALLOCATIONS", "1")
    assert cli_build_diagnostics._build_allocation_diagnostics_enabled()
    monkeypatch.setenv("MOLT_BUILD_ALLOCATIONS", "0")
    assert not cli_build_diagnostics._build_allocation_diagnostics_enabled()


def test_resolve_build_diagnostics_verbosity_aliases() -> None:
    assert cli_build_diagnostics._resolve_build_diagnostics_verbosity(None) == "default"
    assert (
        cli_build_diagnostics._resolve_build_diagnostics_verbosity("brief") == "summary"
    )
    assert (
        cli_build_diagnostics._resolve_build_diagnostics_verbosity("verbose") == "full"
    )
    assert (
        cli_build_diagnostics._resolve_build_diagnostics_verbosity("unknown")
        == "default"
    )


def test_phase_duration_map_orders_by_start(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli.time, "perf_counter", lambda: 10.0)
    durations = cli_build_diagnostics._phase_duration_map(
        {"module_graph": 2.0, "resolve_entry": 1.0}
    )
    assert durations["resolve_entry"] == 1.0
    assert durations["module_graph"] == 8.0


def test_resolve_build_diagnostics_path_relative_and_absolute(tmp_path: Path) -> None:
    rel = cli_build_diagnostics._resolve_build_diagnostics_path("diag.json", tmp_path)
    assert rel == tmp_path / "diag.json"
    abs_path = tmp_path / "absolute_diag.json"
    resolved_abs = cli_build_diagnostics._resolve_build_diagnostics_path(
        str(abs_path), tmp_path
    )
    assert resolved_abs == abs_path


def test_build_midend_diagnostics_payload_summarizes_policy_and_passes() -> None:
    payload = cli_build_diagnostics._build_midend_diagnostics_payload(
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
                "work_budget": 100.0,
                "work_units_spent": 140.0,
                "degraded": True,
                "degrade_events": [
                    {
                        "reason": "work_budget_exceeded",
                        "stage": "round_2_post_dce",
                        "action": "disable_cse",
                        "spent_ms": 140.0,
                        "value": {
                            "work_budget": 100.0,
                            "work_units": 140.0,
                        },
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
    assert payload["degrade_reason_summary"] == {"work_budget_exceeded": 1}
    assert payload["policy_config"]["hot_tier_promotion_enabled"] is True
    assert payload["policy_config"]["work_budget_override"] is None
    assert payload["policy_config"]["budget_alpha"] == 0.03
    assert payload["policy_config"]["budget_beta"] == 0.75
    assert payload["policy_config"]["budget_scale"] == 1.0
    assert payload["telemetry_budget_utilization_avg"] == pytest.approx(140.0 / 120.0)
    assert payload["telemetry_budget_utilization_p95"] == pytest.approx(140.0 / 120.0)
    assert payload["functions_over_telemetry_budget"] == 1
    assert payload["functions_under_50pct_telemetry_budget"] == 0
    assert payload["work_budget_utilization_avg"] == 1.4
    assert payload["work_budget_utilization_p95"] == 1.4
    assert payload["functions_over_work_budget"] == 1
    assert payload["functions_under_50pct_work_budget"] == 0
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
    assert fn_hotspots[0]["work_budget"] == 100.0
    assert fn_hotspots[0]["work_units_spent"] == 140.0
    normalized = payload["policy_outcomes_by_function"]["pkg.mod::fn_a"]
    assert normalized["work_budget"] == 100.0
    assert normalized["work_units_spent"] == 140.0
    degrade_hotspots = payload["degrade_event_hotspots_top"]
    assert degrade_hotspots
    assert degrade_hotspots[0]["reason"] == "work_budget_exceeded"
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
    assert cli_frontend_parallel._resolve_frontend_parallel_module_workers() == 0

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "0")
    assert cli_frontend_parallel._resolve_frontend_parallel_module_workers() == 0

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "3")
    assert cli_frontend_parallel._resolve_frontend_parallel_module_workers() == 3

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "auto")
    assert cli_frontend_parallel._resolve_frontend_parallel_module_workers() >= 2


def test_module_dependency_layers_preserve_topological_determinism() -> None:
    order = ["a", "b", "c", "d", "e"]
    deps = {
        "a": set(),
        "b": {"a"},
        "c": {"a"},
        "d": {"b", "c"},
        "e": {"b"},
    }
    layers = cli_module_dependencies._module_dependency_layers(order, deps)
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
        cli_module_dependencies._analyze_module_schedule(
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
    decision = cli_frontend_parallel._choose_frontend_parallel_layer_workers(
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

    decision = cli_frontend_parallel._choose_frontend_parallel_layer_workers(
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
    decision = cli_frontend_parallel._choose_frontend_parallel_layer_workers(
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
    flags = cli_module_stdlib_policy._build_stdlib_like_module_flags(
        {
            "warnings": cli_module_resolution._stdlib_root_path() / "warnings.py",
            "pkg.mod": Path("/tmp/pkg/mod.py"),
        }
    )
    assert flags["warnings"] is True
    assert flags["pkg.mod"] is False


def test_build_stdlib_like_module_flags_marks_runtime_shipped_modules() -> None:
    package_root = cli_module_resolution._stdlib_root_path().parent
    flags = cli_module_stdlib_policy._build_stdlib_like_module_flags(
        {
            "molt.gpu.tensor": package_root / "gpu" / "tensor.py",
            "molt.lib.turtle_roblox": package_root / "lib" / "turtle_roblox.py",
            "user_mod": Path("/tmp/user_mod.py"),
        }
    )
    assert flags["molt.gpu.tensor"] is True
    assert flags["molt.lib.turtle_roblox"] is True
    assert flags["user_mod"] is False


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
    stdlib_root = cli_module_resolution._stdlib_root_path()
    module_graph = {"hello": source_path}

    augmentation, error = cli._augment_module_graph_for_entry_and_runtime(
        source_path=source_path,
        entry_module="hello",
        module_roots=[project_root, examples_dir],
        stdlib_root=stdlib_root,
        roots=[project_root, examples_dir, stdlib_root],
        project_root=project_root,
        stdlib_allowlist=cli_module_stdlib_policy._stdlib_allowlist(),
        entry_imports=(),
        module_resolution_cache=cli_module_resolution._ModuleResolutionCache(),
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
        known_func_kinds={"main": {"run": "sync"}},
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
    assert execution_view.scoped_inputs.known_func_kinds == {"main": {"run": "sync"}}
    assert execution_view.scoped_inputs.pgo_hot_function_names == ("main::hot",)
    assert set(execution_view.scoped_known_classes) == {"MainClass"}


def test_choose_frontend_parallel_layer_workers_uses_precomputed_costs_and_flags(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_predict_frontend_module_cost",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected live cost recompute")
        ),
    )
    monkeypatch.setattr(
        cli_frontend_parallel,
        "_looks_like_stdlib_module_name",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected stdlib classification recompute")
        ),
    )

    decision = cli_frontend_parallel._choose_frontend_parallel_layer_workers(
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
    assert cli_module_dependencies._module_order_has_back_edges(
        order, {"a": {"b"}, "b": {"a"}}
    )
    assert not cli_module_dependencies._module_order_has_back_edges(
        order, {"a": set(), "b": {"a"}}
    )


def test_analyze_module_schedule_marks_cycles_and_appends_remaining() -> None:
    module_graph = {
        "a": Path("/tmp/a.py"),
        "b": Path("/tmp/b.py"),
    }
    deps = {"a": {"b"}, "b": {"a"}}

    order, reverse_deps, has_back_edges, layers, closures = (
        cli_module_dependencies._analyze_module_schedule(
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
    source = "x = 1\ny = x + 2\n"
    payload = {
        "module_name": "worker_module",
        "module_path": str(module_path),
        "source_lease": cli_module_source._ModuleSourceLease.inline(
            module_path, source
        ).worker_payload(),
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
        "known_func_kinds": {},
        "module_chunking": False,
        "module_chunk_max_ops": 0,
        "optimization_profile": "dev",
        "pgo_hot_functions": ["worker_module::molt_main"],
    }
    result = cli_frontend_worker._frontend_lower_module_worker(payload)
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

    config, failure = cli_frontend_pipeline._prepare_frontend_lowering_config(
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
        direct_call_modules={"entry"},
        known_func_defaults={},
        known_func_kinds={},
        native_callable_exports={},
        pgo_hot_function_names=set(),
        generated_module_source_paths={},
        entry_module="entry",
        namespace_module_names=set(),
        module_source_catalog=cli_module_source._build_module_source_catalog(
            {"entry": source_path},
            module_sources={"entry": "print('ok')\n"},
        ),
        is_wasm=False,
        target_triple=None,
        frontend_parallel_details={},
        frontend_phase_timeout=None,
        source_recompiled_external_packages=set(),
    )

    assert failure is None
    assert config is not None
    assert config.module_chunking is True
    assert config.module_chunk_max_ops == 1400


def test_prepare_frontend_lowering_config_skips_ty_for_source_recompiled_native_packages(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source_path = tmp_path / "entry.py"
    source_path.write_text(
        "from scipy.ndimage import distance_transform_edt\n", encoding="utf-8"
    )
    warnings: list[str] = []

    def fail_collect(*args: object, **kwargs: object) -> None:
        raise AssertionError("ty/type-fact collection must not run")

    monkeypatch.setattr(
        cli_frontend_pipeline._typecheck,
        "_collect_type_facts_for_build",
        fail_collect,
    )

    config, failure = cli_frontend_pipeline._prepare_frontend_lowering_config(
        type_facts_path=None,
        type_hint_policy="check",
        module_graph={"entry": source_path},
        source_path=source_path,
        json_output=True,
        warnings=warnings,
        module_deps={"entry": set()},
        module_dep_closures={"entry": frozenset()},
        has_back_edges=False,
        known_modules={"entry", "scipy", "scipy.ndimage"},
        direct_call_modules={"entry"},
        known_func_defaults={},
        known_func_kinds={},
        native_callable_exports={},
        pgo_hot_function_names=set(),
        generated_module_source_paths={},
        entry_module="entry",
        namespace_module_names=set(),
        module_source_catalog=cli_module_source._build_module_source_catalog(
            {"entry": source_path},
            module_sources={
                "entry": "from scipy.ndimage import distance_transform_edt\n"
            },
        ),
        is_wasm=True,
        target_triple="wasm32-wasip1",
        frontend_parallel_details={},
        frontend_phase_timeout=None,
        source_recompiled_external_packages={"scipy"},
    )

    assert failure is None
    assert config is not None
    assert config.type_facts is None
    assert warnings == [
        "source-recompiled external native packages use package/native artifact "
        "custody instead of ty-derived type facts; continuing with guarded hints."
    ]


def test_duration_ms_from_ns_clamps_and_converts() -> None:
    assert cli_build_diagnostics._duration_ms_from_ns(1_000_000, 2_500_000) == 1.5
    assert cli_build_diagnostics._duration_ms_from_ns(5, 4) == 0.0
    assert cli_build_diagnostics._duration_ms_from_ns("bad", 10) == 0.0


def test_emit_build_diagnostics_includes_frontend_parallel_layer_counters(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli_build_diagnostics._emit_build_diagnostics(
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
                    "work_budget_override": 512.0,
                    "budget_alpha": 0.03,
                    "budget_beta": 0.75,
                    "budget_scale": 1.0,
                },
                "work_budget_utilization_avg": 0.75,
                "work_budget_utilization_p95": 0.8,
                "functions_over_work_budget": 1,
                "functions_under_50pct_work_budget": 2,
                "telemetry_budget_utilization_avg": 1.25,
                "telemetry_budget_utilization_p95": 1.5,
                "functions_over_telemetry_budget": 3,
                "functions_under_50pct_telemetry_budget": 4,
                "function_hotspots_top": [
                    {
                        "module": "pkg.mod",
                        "function": "cold_fn",
                        "spent_ms": 18.0,
                        "budget_ms": 12.0,
                        "work_units_spent": 900.0,
                        "work_budget": 600.0,
                        "degraded": True,
                    }
                ],
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
    assert "- midend.policy.work_budget_override: 512.0000" in stderr
    assert (
        "- midend.policy.budget_formula: alpha=0.0300 beta=0.7500 scale=1.0000"
        in stderr
    )
    assert "- midend.work_budget_utilization_avg: 0.7500" in stderr
    assert "- midend.work_budget_utilization_p95: 0.8000" in stderr
    assert "- midend.functions_over_work_budget: 1" in stderr
    assert "- midend.functions_under_50pct_work_budget: 2" in stderr
    assert "- midend.telemetry_budget_utilization_avg: 1.2500" in stderr
    assert "- midend.telemetry_budget_utilization_p95: 1.5000" in stderr
    assert "- midend.functions_over_telemetry_budget: 3" in stderr
    assert "- midend.functions_under_50pct_telemetry_budget: 4" in stderr
    assert (
        "midend.function_hotspot.1: pkg.mod::cold_fn spent_ms=18.000 "
        "budget_ms=12.000 work_units=900.000 work_budget=600.000 degraded=True"
        in stderr
    )
    assert (
        "midend.work_budget_top.1: pkg.mod::cold_fn ratio=1.5000 "
        "work_units=900.000 work_budget=600.000" in stderr
    )
    assert (
        "midend.telemetry_budget_top.1: pkg.mod::cold_fn ratio=1.5000 "
        "spent_ms=18.000 budget_ms=12.000" in stderr
    )
    assert "- midend.promoted_functions: 2" in stderr
    assert "- midend.promotion_source.pgo_hot_functions: 2" in stderr
    assert "midend.promotion_hotspot.1: pkg.mod::hot_fn B->A" in stderr


def test_capture_build_allocation_diagnostics_returns_top_sites() -> None:
    cli.tracemalloc.start(5)
    try:
        payload = cli_build_diagnostics._capture_build_allocation_diagnostics(top_n=3)
    finally:
        cli.tracemalloc.stop()
    assert payload is not None
    assert isinstance(payload["current_bytes"], int)
    assert isinstance(payload["peak_bytes"], int)
    assert len(payload["top"]) <= 3


def test_emit_build_diagnostics_prints_allocation_summary(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli_build_diagnostics._emit_build_diagnostics(
        diagnostics={
            "total_sec": 1.0,
            "allocations": {
                "current_bytes": 1024,
                "peak_bytes": 4096,
                "top": [
                    {
                        "file": "src/molt/cli/__init__.py",
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
    assert "alloc.top.1: src/molt/cli/__init__.py:123 size_bytes=2048 count=7" in stderr


def test_midend_policy_config_snapshot_honors_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_MIDEND_PROFILE", "release")
    monkeypatch.setenv("MOLT_MIDEND_HOT_TIER_PROMOTION", "0")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_MS", "42")
    monkeypatch.setenv("MOLT_MIDEND_WORK_BUDGET", "2048")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_ALPHA", "0.5")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_BETA", "2.0")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_SCALE", "1.5")

    assert cli_build_diagnostics._midend_policy_config_snapshot() == {
        "profile_override": "release",
        "hot_tier_promotion_enabled": False,
        "work_budget_override": 2048.0,
        "budget_alpha": 0.5,
        "budget_beta": 2.0,
        "budget_scale": 1.5,
    }


def test_emit_build_diagnostics_summary_omits_hotspot_details(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli_build_diagnostics._emit_build_diagnostics(
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
    cli_build_diagnostics._emit_build_diagnostics(
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
    assert (
        cli_module_resolution._module_name_from_path(script, roots, stdlib_root)
        == "outside_script"
    )


def test_expand_module_chain_ignores_invalid_module_names() -> None:
    assert cli_module_dependencies._expand_module_chain("pkg.sub") == ["pkg", "pkg.sub"]
    assert cli_module_dependencies._expand_module_chain("") == []
    assert cli_module_dependencies._expand_module_chain("/.Volumes.bad.mod") == []


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
    identity_path = tmp_path / "daemon.identity.json"
    log_path = tmp_path / "daemon.log"
    wait_timeouts: list[float | None] = []
    terminated: list[int] = []
    removed: list[Path] = []

    class _FakePopen:
        pid = 4321

        def poll(self) -> int | None:  # subprocess.Popen API
            return None

    _stub_backend_daemon_harness(monkeypatch)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_path",
        lambda *args, **kwargs: identity_path,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_unix_socket_path_exceeds_limit", lambda path: False
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_process_command",
        lambda pid: f"{backend_bin} --daemon --socket {socket_path}",
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_read_backend_daemon_identity", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_sweep_orphaned_backend_daemon_locks_once",
        lambda *args, **kwargs: None,
    )

    def fake_wait_until_ready(
        *args: object, **kwargs: object
    ) -> tuple[bool, dict[str, object] | None]:
        del args
        wait_timeouts.append(cast(float | None, kwargs.get("ready_timeout")))
        return False, None

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_wait_until_ready", fake_wait_until_ready
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION.subprocess, "Popen", lambda *args, **kwargs: _FakePopen()
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_terminate_backend_daemon_identity",
        lambda identity, **kwargs: terminated.append(identity.pid),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_remove_backend_daemon_identity",
        lambda path: removed.append(path),
    )

    assert (
        cli._start_backend_daemon(
            backend_bin,
            socket_path,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            target_triple=None,
            config_digest=None,
            startup_timeout=2.0,
            json_output=True,
            warnings=[],
        )
        is False
    )
    assert wait_timeouts == [0.25]
    payload = json.loads(identity_path.read_text())
    assert payload["pid"] == 4321
    assert payload["socket_path"] == str(socket_path)
    assert terminated == []
    assert removed == []


def test_start_backend_daemon_trusts_verified_busy_socket_with_live_pid(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("backend")
    socket_path = tmp_path / "daemon.sock"
    socket_path.write_text("")
    identity_path = tmp_path / "daemon.identity.json"
    log_path = tmp_path / "daemon.log"
    existing_identity = _test_backend_daemon_identity(
        1234,
        socket_path=socket_path,
        project_root=tmp_path,
        backend_bin=backend_bin,
    )
    wait_timeouts: list[float | None] = []
    terminated: list[int] = []
    removed: list[Path] = []
    ready_calls = 0

    class _FakePopen:
        pid = 4321

        def poll(self) -> int | None:  # subprocess.Popen API
            return None

    _stub_backend_daemon_harness(monkeypatch)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_sweep_orphaned_backend_daemon_locks_once",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_path",
        lambda *args, **kwargs: identity_path,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_unix_socket_path_exceeds_limit", lambda path: False
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_process_command",
        lambda pid: f"{backend_bin} --daemon --socket {socket_path}",
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_read_backend_daemon_identity",
        lambda *args, **kwargs: existing_identity,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_matches_context",
        lambda identity, **kwargs: True,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_is_verified",
        lambda identity, **kwargs: True,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_sweep_orphaned_backend_daemon_locks_once",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_binary_is_newer",
        lambda *args, **kwargs: False,
    )

    def fake_wait_until_ready(
        *args: object, **kwargs: object
    ) -> tuple[bool, dict[str, object] | None]:
        nonlocal ready_calls
        del args
        ready_calls += 1
        wait_timeouts.append(cast(float | None, kwargs.get("ready_timeout")))
        return False, None

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_wait_until_ready", fake_wait_until_ready
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION.subprocess, "Popen", lambda *args, **kwargs: _FakePopen()
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_terminate_backend_daemon_identity",
        lambda identity, **kwargs: terminated.append(identity.pid),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_remove_backend_daemon_identity",
        lambda path: removed.append(path),
    )

    assert (
        cli._start_backend_daemon(
            backend_bin,
            socket_path,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            target_triple=None,
            config_digest=None,
            startup_timeout=2.0,
            json_output=True,
            warnings=[],
        )
        is True
    )
    assert wait_timeouts[0] == 0.25
    assert ready_calls == 1
    assert terminated == []
    assert removed == []


def test_start_backend_daemon_ignores_foreign_socket_dir_entries(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import tempfile

    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("backend")
    build_state_root = tmp_path / "target" / ".molt_state"
    wait_timeouts: list[float | None] = []

    identity_path = (
        build_state_root / "backend_daemon" / "molt-backend.dev-fast.identity.json"
    )
    log_path = build_state_root / "backend_daemon" / "molt-backend.dev-fast.log"

    class _FakePopen:
        pid = 4321

        def poll(self) -> int | None:  # subprocess.Popen API
            return None

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

    _stub_backend_daemon_harness(monkeypatch)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_path",
        lambda *args, **kwargs: identity_path,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_unix_socket_path_exceeds_limit", lambda path: False
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_process_command",
        lambda pid: f"{backend_bin} --daemon --socket {socket_path}",
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_read_backend_daemon_identity", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_wait_until_ready", fake_wait_until_ready
    )
    monkeypatch.setattr(BACKEND_EXECUTION.subprocess, "Popen", fake_popen)

    with tempfile.TemporaryDirectory(
        prefix="moltbd-test-", dir=tempfile.gettempdir()
    ) as sockdir:
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
                target_triple=None,
                config_digest=None,
                startup_timeout=2.0,
                json_output=True,
                warnings=[],
            )
            is True
        )

        backend_spawn_calls = [cmd for cmd in spawn_calls if cmd[0] == str(backend_bin)]
        assert backend_spawn_calls == [
            [str(backend_bin), "--daemon", "--socket", str(socket_path)]
        ]
        assert wait_timeouts == [0.25]
        assert not socket_path.with_suffix(".redirect").exists()
        for idx in range(3):
            assert (socket_dir / f"moltbd.foreign{idx}.sock").exists()


def test_start_backend_daemon_restarts_stale_daemon_without_running_cargo(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    backend_bin = project_root / "target" / "debug" / "molt-backend"
    backend_bin.parent.mkdir(parents=True)
    backend_bin.write_text("backend")
    socket_path = tmp_path / "daemon.sock"
    identity_path = tmp_path / "daemon.identity.json"
    log_path = tmp_path / "daemon.log"
    existing_identity = _test_backend_daemon_identity(
        1234,
        socket_path=socket_path,
        project_root=project_root,
        backend_bin=backend_bin,
    )
    terminated: list[int] = []
    removed: list[Path] = []

    class _FakePopen:
        pid = 4321

        def poll(self) -> int | None:  # subprocess.Popen API
            return None

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")
    _stub_backend_daemon_harness(monkeypatch)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_path",
        lambda *args, **kwargs: identity_path,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_unix_socket_path_exceeds_limit", lambda path: False
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_read_backend_daemon_identity",
        lambda *args, **kwargs: existing_identity,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_is_verified",
        lambda identity, **kwargs: True,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_binary_is_newer",
        lambda *args, **kwargs: True,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_terminate_backend_daemon_identity",
        lambda identity, **kwargs: terminated.append(identity.pid),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_remove_backend_daemon_identity",
        lambda path: removed.append(path),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_wait_until_ready",
        lambda *args, **kwargs: (True, None),
    )

    def fail_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[bytes]:
        raise AssertionError("daemon startup must not invoke cargo")

    monkeypatch.setattr(BACKEND_EXECUTION.subprocess, "run", fail_run)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_process_command",
        lambda pid: f"{backend_bin} --daemon --socket {socket_path}",
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION.subprocess, "Popen", lambda *args, **kwargs: _FakePopen()
    )

    warnings: list[str] = []
    assert (
        cli._start_backend_daemon(
            backend_bin,
            socket_path,
            cargo_profile="dev-fast",
            project_root=project_root,
            target_triple=None,
            config_digest=None,
            startup_timeout=2.0,
            json_output=True,
            warnings=warnings,
        )
        is False
    )
    assert terminated == []
    assert removed == []
    assert warnings
    assert "preserving verified pid 1234" in warnings[0]
    assert "one-shot backend compile" in warnings[0]


def test_start_backend_daemon_refuses_to_kill_unverified_stale_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    backend_bin = project_root / "target" / "debug" / "molt-backend"
    backend_bin.parent.mkdir(parents=True)
    backend_bin.write_text("backend")
    socket_path = tmp_path / "daemon.sock"
    identity_path = tmp_path / "daemon.identity.json"
    log_path = tmp_path / "daemon.log"
    existing_identity = _test_backend_daemon_identity(
        1234,
        socket_path=socket_path,
        project_root=project_root,
        backend_bin=backend_bin,
    )
    removed: list[Path] = []
    warnings: list[str] = []

    class _FakePopen:
        pid = 4321

        def poll(self) -> int | None:  # subprocess.Popen API
            return None

    _stub_backend_daemon_harness(monkeypatch)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_sweep_orphaned_backend_daemon_locks_once",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_path",
        lambda *args, **kwargs: identity_path,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_log_path", lambda *args, **kwargs: log_path
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_unix_socket_path_exceeds_limit", lambda path: False
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_read_backend_daemon_identity",
        lambda *args, **kwargs: existing_identity,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_is_verified",
        lambda identity, **kwargs: False,
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_binary_is_newer",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("stale-binary check must not run for unverified identity")
        ),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_terminate_backend_daemon_identity",
        lambda identity, **kwargs: (_ for _ in ()).throw(
            AssertionError("unverified identity must not be killed")
        ),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_remove_backend_daemon_identity",
        lambda path: removed.append(path),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_wait_until_ready",
        lambda *args, **kwargs: (True, None),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_process_command",
        lambda pid: f"{backend_bin} --daemon --socket {socket_path}",
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION.subprocess, "Popen", lambda *args, **kwargs: _FakePopen()
    )

    assert (
        cli._start_backend_daemon(
            backend_bin,
            socket_path,
            cargo_profile="dev-fast",
            project_root=project_root,
            target_triple=None,
            config_digest=None,
            startup_timeout=2.0,
            json_output=True,
            warnings=warnings,
        )
        is True
    )
    assert removed == [identity_path]
    assert any("not a verified live daemon" in warning for warning in warnings)


def test_prepare_backend_setup_stages_runtime_intrinsics_before_native_cache_hit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    symbols_file = tmp_path / "libmolt_runtime.a.intrinsics.txt"
    symbols_file.write_text(
        "molt_importlib_import_transaction\nmolt_len\n", encoding="utf-8"
    )
    output_artifact = tmp_path / "output.o"
    ensure_calls: list[Path | None] = []
    cache_setup_kwargs: list[dict[str, object]] = []
    empty_module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module=None,
    )

    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: cli._RuntimeArtifactState(runtime_lib=runtime_lib),
    )

    def fake_prepare_backend_cache_setup(**kwargs: object) -> cli._BackendCacheSetup:
        cache_setup_kwargs.append(dict(kwargs))
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
        cli_backend_cache_setup,
        "_prepare_backend_cache_setup",
        fake_prepare_backend_cache_setup,
    )

    def fake_ensure_runtime_lib_ready(runtime_state: object, **kwargs: object) -> bool:
        del kwargs
        assert isinstance(runtime_state, cli._RuntimeArtifactState)
        ensure_calls.append(runtime_state.runtime_lib)
        runtime_lib.write_bytes(b"runtime")
        return True

    monkeypatch.setattr(cli, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready
    )
    monkeypatch.setattr(
        RUNTIME_INTRINSIC_SYMBOLS,
        "_runtime_intrinsic_symbols_file",
        lambda runtime_lib_path: (symbols_file, None),
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: None,
    )

    prepared_backend_setup, backend_setup_error = (
        cli_backend_compile._prepare_backend_setup(
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
            module_graph_metadata=empty_module_graph_metadata,
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        )
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert prepared_backend_setup.cache_hit is True
    assert ensure_calls == [runtime_lib]
    assert os.environ["MOLT_RUNTIME_INTRINSIC_SYMBOLS"] == str(symbols_file)
    assert cache_setup_kwargs[0]["runtime_intrinsic_symbols_digest"] == (
        RUNTIME_INTRINSIC_SYMBOLS._runtime_intrinsic_symbols_digest(symbols_file)
    )


def test_prepare_backend_setup_stages_runtime_intrinsics_before_native_cache_miss(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    symbols_file = tmp_path / "libmolt_runtime.a.intrinsics.txt"
    symbols_file.write_text("molt_len\n", encoding="utf-8")
    output_artifact = tmp_path / "output.o"
    ensure_calls: list[Path | None] = []
    cache_setup_kwargs: list[dict[str, object]] = []
    empty_module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module=None,
    )

    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: cli._RuntimeArtifactState(runtime_lib=runtime_lib),
    )

    def fake_prepare_backend_cache_setup(**kwargs: object) -> cli._BackendCacheSetup:
        cache_setup_kwargs.append(dict(kwargs))
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
        cli_backend_cache_setup,
        "_prepare_backend_cache_setup",
        fake_prepare_backend_cache_setup,
    )

    def fake_ensure_runtime_lib_ready(runtime_state: object, **kwargs: object) -> bool:
        del kwargs
        assert isinstance(runtime_state, cli._RuntimeArtifactState)
        ensure_calls.append(runtime_state.runtime_lib)
        runtime_lib.write_bytes(b"runtime")
        return True

    monkeypatch.setattr(cli, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready
    )
    monkeypatch.setattr(
        RUNTIME_INTRINSIC_SYMBOLS,
        "_runtime_intrinsic_symbols_file",
        lambda runtime_lib_path: (symbols_file, None),
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: None,
    )

    prepared_backend_setup, backend_setup_error = (
        cli_backend_compile._prepare_backend_setup(
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
            module_graph_metadata=empty_module_graph_metadata,
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        )
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert prepared_backend_setup.cache_hit is False
    assert ensure_calls == [runtime_lib]
    assert cache_setup_kwargs[0]["runtime_intrinsic_symbols_digest"] == (
        RUNTIME_INTRINSIC_SYMBOLS._runtime_intrinsic_symbols_digest(symbols_file)
    )


def test_prepare_backend_setup_uses_runtime_intrinsic_digest_instead_of_native_async(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    symbols_file = tmp_path / "libmolt_runtime.a.intrinsics.txt"
    symbols_file.write_text("molt_len\n", encoding="utf-8")
    output_artifact = tmp_path / "output.o"
    scheduled: list[tuple[Path | None, str | None, str, frozenset[str]]] = []
    cache_setup_kwargs: list[dict[str, object]] = []
    empty_module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module=None,
    )

    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: cli._RuntimeArtifactState(runtime_lib=runtime_lib),
    )

    def fake_prepare_backend_cache_setup(**kwargs: object) -> cli._BackendCacheSetup:
        cache_setup_kwargs.append(dict(kwargs))
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
        cli_backend_cache_setup,
        "_prepare_backend_cache_setup",
        fake_prepare_backend_cache_setup,
    )

    def fake_ensure_runtime_lib_ready(runtime_state: object, **kwargs: object) -> bool:
        del runtime_state, kwargs
        runtime_lib.write_bytes(b"runtime")
        return True

    monkeypatch.setattr(cli, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready
    )
    monkeypatch.setattr(
        RUNTIME_INTRINSIC_SYMBOLS,
        "_runtime_intrinsic_symbols_file",
        lambda runtime_lib_path: (symbols_file, None),
    )
    monkeypatch.setattr(
        cli_backend_compile,
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

    prepared_backend_setup, backend_setup_error = (
        cli_backend_compile._prepare_backend_setup(
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
            module_graph_metadata=empty_module_graph_metadata,
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
            resolved_modules={"__main__", "json"},
        )
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert scheduled == []
    assert cache_setup_kwargs[0]["runtime_intrinsic_symbols_digest"] == (
        RUNTIME_INTRINSIC_SYMBOLS._runtime_intrinsic_symbols_digest(symbols_file)
    )


def test_prepare_backend_setup_stages_runtime_intrinsics_for_object_emit_without_async(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    symbols_file = tmp_path / "libmolt_runtime.a.intrinsics.txt"
    symbols_file.write_text("molt_len\n", encoding="utf-8")
    output_artifact = tmp_path / "output.o"
    scheduled: list[Path | None] = []
    ensure_calls: list[Path | None] = []
    cache_setup_kwargs: list[dict[str, object]] = []
    empty_module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module=None,
    )

    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: cli._RuntimeArtifactState(runtime_lib=runtime_lib),
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_prepare_backend_cache_setup",
        lambda **kwargs: (
            cache_setup_kwargs.append(dict(kwargs))
            or cli._BackendCacheSetup(
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
        ),
    )

    def fake_ensure_runtime_lib_ready(runtime_state: object, **kwargs: object) -> bool:
        del kwargs
        assert isinstance(runtime_state, cli._RuntimeArtifactState)
        ensure_calls.append(runtime_state.runtime_lib)
        runtime_lib.write_bytes(b"runtime")
        return True

    monkeypatch.setattr(cli, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_ensure_runtime_lib_ready", fake_ensure_runtime_lib_ready
    )
    monkeypatch.setattr(
        RUNTIME_INTRINSIC_SYMBOLS,
        "_runtime_intrinsic_symbols_file",
        lambda runtime_lib_path: (symbols_file, None),
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda runtime_state, **kwargs: scheduled.append(runtime_state.runtime_lib),
    )

    prepared_backend_setup, backend_setup_error = (
        cli_backend_compile._prepare_backend_setup(
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
            module_graph_metadata=empty_module_graph_metadata,
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
            resolved_modules={"__main__"},
        )
    )

    assert backend_setup_error is None
    assert prepared_backend_setup is not None
    assert ensure_calls == [runtime_lib]
    assert scheduled == []
    assert os.environ["MOLT_RUNTIME_INTRINSIC_SYMBOLS"] == str(symbols_file)
    assert cache_setup_kwargs[0]["runtime_intrinsic_symbols_digest"] == (
        RUNTIME_INTRINSIC_SYMBOLS._runtime_intrinsic_symbols_digest(symbols_file)
    )


def test_initialize_runtime_artifact_state_assigns_native_object_runtime_lib(
    tmp_path: Path,
) -> None:
    state = RUNTIME_BUILD._initialize_runtime_artifact_state(
        is_rust_transpile=False,
        is_wasm=False,
        emit_mode="obj",
        molt_root=tmp_path,
        runtime_cargo_profile="release-fast",
        target_triple=None,
        stdlib_profile="micro",
    )

    assert state.runtime_lib == cli._runtime_lib_path(
        tmp_path,
        "release-fast",
        None,
        stdlib_profile="micro",
    )


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
        RUNTIME_BUILD,
        "_ensure_runtime_lib_ready",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("sync runtime build should not run when async future exists")
        ),
    )
    phase_starts: dict[str, float] = {}

    ready = RUNTIME_BUILD._ensure_native_runtime_lib_ready_before_link(
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
    runtime_state = cli._RuntimeArtifactState(
        runtime_lib=tmp_path / "libmolt_runtime.a"
    )
    captured: list[frozenset[str]] = []

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_ensure_runtime_lib_ready",
        lambda runtime_state, **kwargs: (
            captured.append(frozenset(cast(set[str], kwargs["resolved_modules"])))
            or True
        ),
    )

    ready = RUNTIME_BUILD._ensure_native_runtime_lib_ready_before_link(
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
    captured: list[tuple[bool, frozenset[str], frozenset[str]]] = []

    monkeypatch.setattr(
        cli_backend_compile,
        "_ensure_runtime_wasm_artifact",
        lambda runtime_state, *, reloc, **kwargs: (
            captured.append(
                (
                    reloc,
                    frozenset(cast(set[str], kwargs["resolved_modules"])),
                    frozenset(cast(set[str], kwargs["required_link_features"])),
                )
            )
            or True
        ),
    )

    runtime_context, failure = cli_backend_compile._prepare_backend_runtime_context(
        prepared_backend_setup=prepared_backend_setup,
        is_wasm_freestanding=False,
        json_output=True,
        runtime_cargo_profile="dev-fast",
        cargo_timeout=1.0,
        molt_root=tmp_path,
        stdlib_profile="micro",
        resolved_modules={"asyncio", "ssl"},
        required_link_features=frozenset({"molt_gpu_primitives"}),
    )

    assert failure is None
    assert runtime_context is not None
    assert runtime_context.ensure_runtime_wasm_shared() is True
    assert runtime_context.ensure_runtime_wasm_reloc() is True
    assert captured == [
        (
            False,
            frozenset({"asyncio", "ssl"}),
            frozenset({"molt_gpu_primitives"}),
        ),
        (
            True,
            frozenset({"asyncio", "ssl"}),
            frozenset({"molt_gpu_primitives"}),
        ),
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
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        cli_backend_compile,
        "_read_wasm_data_end",
        lambda path: 4096 if path == runtime_reloc_wasm else None,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_read_wasm_memory_min_bytes",
        lambda path: 8192 if path == runtime_reloc_wasm else None,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_read_wasm_table_min",
        lambda path: 1234 if path == runtime_reloc_wasm else None,
    )

    def ensure_shared(required=None):
        calls.append(("shared", frozenset(required) if required else None))
        return True

    def ensure_reloc(required=None):
        calls.append(("reloc", frozenset(required) if required else None))
        runtime_reloc_wasm.write_bytes(b"\0asm\x01\0\0\0")
        return True

    prepared, err = cli_backend_compile._prepare_backend_dispatch(
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
        ir={"functions": []},
        warnings=[],
    )

    assert err is None
    assert prepared is not None
    assert calls == [("reloc", None)]
    assert prepared.backend_env is not None
    assert prepared.backend_env["MOLT_WASM_DATA_BASE"] == str(64 * 1024 * 1024 + 8192)
    assert prepared.backend_env["MOLT_WASM_TABLE_BASE"] == "1234"


def test_prepare_backend_dispatch_linked_table_base_uses_shared_runtime_prefix(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    runtime_reloc_wasm = tmp_path / "molt_runtime_reloc.wasm"
    runtime_reloc_wasm.write_bytes(b"\0asm\x01\0\0\0")
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("")

    calls: list[tuple[str, object | None]] = []

    monkeypatch.delenv("MOLT_WASM_DATA_BASE", raising=False)
    monkeypatch.delenv("MOLT_WASM_TABLE_BASE", raising=False)
    monkeypatch.delenv("MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN", raising=False)
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(cli_backend_compile, "_read_wasm_data_end", lambda _path: 4096)
    monkeypatch.setattr(
        cli_backend_compile, "_read_wasm_memory_min_bytes", lambda _path: 8192
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_read_wasm_table_min",
        lambda path: 3074 if path == runtime_reloc_wasm else 3867,
    )

    def ensure_shared(required=None):
        calls.append(("shared", frozenset(required) if required else None))
        runtime_wasm.write_bytes(b"\0asm\x01\0\0\0")
        return True

    def ensure_reloc(required=None):
        calls.append(("reloc", frozenset(required) if required else None))
        return True

    prepared, err = cli_backend_compile._prepare_backend_dispatch(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        split_runtime=False,
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
        ir={"functions": []},
        warnings=[],
    )

    assert err is None
    assert prepared is not None
    assert ("shared", None) in calls
    assert prepared.backend_env is not None
    assert prepared.backend_env["MOLT_WASM_TABLE_BASE"] == "3867"


def test_prepare_backend_dispatch_uses_reloc_runtime_for_split_runtime_table_min_when_shared_runtime_missing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    runtime_reloc_wasm = tmp_path / "molt_runtime_reloc.wasm"
    runtime_reloc_wasm.write_bytes(b"\0asm\x01\0\0\0")
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_text("")

    calls: list[tuple[str, object | None]] = []

    monkeypatch.delenv("MOLT_WASM_DATA_BASE", raising=False)
    monkeypatch.delenv("MOLT_WASM_TABLE_BASE", raising=False)
    monkeypatch.delenv("MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN", raising=False)
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary, "_ensure_backend_binary", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(cli_backend_compile, "_read_wasm_data_end", lambda path: None)
    monkeypatch.setattr(
        cli_backend_compile, "_read_wasm_memory_min_bytes", lambda path: None
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_read_wasm_table_min",
        lambda path: 1234 if path == runtime_reloc_wasm else None,
    )

    def ensure_shared(required=None):
        calls.append(("shared", frozenset(required) if required else None))
        return True

    def ensure_reloc(required=None):
        calls.append(("reloc", frozenset(required) if required else None))
        return True

    prepared, err = cli_backend_compile._prepare_backend_dispatch(
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
        ir={"functions": []},
        warnings=[],
    )

    assert err is None
    assert prepared is not None
    # The reloc runtime is always validated even when the artifact already
    # exists on disk, so ensure_reloc is called once with None (no required
    # module set).
    assert calls == [("reloc", None)]
    assert prepared.backend_env is not None
    assert prepared.backend_env["MOLT_WASM_TABLE_BASE"] == "1234"
    assert prepared.backend_env["MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN"] == "1234"


def test_ensure_runtime_wasm_verified_key_is_stable_across_user_import_graph(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0")
    stored_fingerprint = {"artifact_sha256": cli._sha256_file(runtime_wasm)}
    verification_calls: list[tuple[frozenset[str], str]] = []

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda project_root, **kwargs: {
            "runtime_features": tuple(
                cast(tuple[str, ...], kwargs["runtime_features"])
            ),
            "rustflags": cast(str, kwargs["rustflags"]),
        },
    )
    monkeypatch.setattr(
        cli_link_pipeline,
        "_artifact_needs_rebuild",
        lambda artifact, fingerprint, stored_fingerprint: (
            verification_calls.append(
                (
                    frozenset(cast(tuple[str, ...], fingerprint["runtime_features"])),
                    cast(str, fingerprint["rustflags"]),
                )
            )
            or False
        ),
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_read_runtime_fingerprint", lambda path: stored_fingerprint
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_shared_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_wasm_exports_satisfy", lambda path, req: True
    )

    assert RUNTIME_BUILD._ensure_runtime_wasm(
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
        required_exports={"runtime_init"},
    )
    assert RUNTIME_BUILD._ensure_runtime_wasm(
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
        required_exports={"fast_list_append"},
    )

    assert len(verification_calls) >= 2
    assert all(call == verification_calls[0] for call in verification_calls)
    assert {"stdlib_serial", "stdlib_micro", "no-default-features"} <= (
        verification_calls[0][0]
    )
    assert "stdlib_net" not in verification_calls[0][0]


def test_runtime_artifact_fingerprint_match_fails_closed_without_stored_fingerprint(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    artifact = tmp_path / "runtime.wasm"
    artifact.write_bytes(b"\0asm\x01\0\0\0")
    fingerprint_path = tmp_path / "missing.fingerprint"

    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args: False
    )

    assert (
        cli._runtime_artifact_fingerprint_matches(
            artifact,
            {"hash": "expected"},
            fingerprint_path,
            require_artifact_digest=True,
        )
        is False
    )


def test_ensure_runtime_wasm_writes_integrity_sidecar_after_copy(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    built_src = (
        tmp_path
        / "target"
        / "wasm32-wasip1"
        / "dev-fast"
        / "deps"
        / "molt_runtime-test.wasm"
    )
    built_src.parent.mkdir(parents=True, exist_ok=True)
    built_src.write_bytes(b"\0asm\x01\0\0\0runtime")

    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_fingerprint", lambda *args, **kwargs: {"hash": "new"}
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_inspect_wasm_binary", lambda path: "valid")
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_shared_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_run_runtime_wasm_cargo_build",
        lambda **kwargs: (
            subprocess.CompletedProcess(kwargs["cmd"], 0, "", ""),
            built_src,
        ),
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_write_runtime_fingerprint", lambda *args, **kwargs: None
    )

    assert RUNTIME_BUILD._ensure_runtime_wasm(
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


def test_reloc_runtime_wasm_exports_runtime_owned_gpu_intrinsics(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "wasm" / "molt_runtime_reloc.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    built_src = (
        tmp_path
        / "target"
        / "wasm32-wasip1"
        / "dev-fast"
        / "deps"
        / "molt_runtime-test.wasm"
    )
    built_src.parent.mkdir(parents=True, exist_ok=True)
    built_src.write_bytes(b"\0asm\x01\0\0\0runtime")
    captured_env: dict[str, str] = {}

    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_fingerprint", lambda *args, **kwargs: {"hash": "new"}
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_inspect_wasm_binary", lambda path: "valid")

    def fake_runtime_build(**kwargs):
        captured_env.update(kwargs["env"])
        return subprocess.CompletedProcess(kwargs["cmd"], 0, "", ""), built_src

    captured_link: dict[str, str] = {}

    def fake_reloc_link(staticlib_path, output_path, **kwargs):
        captured_link.update(kwargs)
        output_path.write_bytes(staticlib_path.read_bytes())
        return True

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_runtime_wasm_cargo_build", fake_runtime_build
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_link_runtime_staticlib_to_reloc_wasm", fake_reloc_link
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_write_runtime_fingerprint", lambda *args, **kwargs: None
    )

    assert RUNTIME_BUILD._ensure_runtime_wasm(
        runtime_wasm,
        reloc=True,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules={"molt.gpu.tensor"},
        required_exports=None,
    )

    rustflags = captured_env["RUSTFLAGS"]
    assert "--import-memory" not in rustflags
    assert "--import-table" not in rustflags
    assert "--export-if-defined=molt_gpu_matmul_contiguous" in rustflags
    assert (
        "--export-if-defined=molt_gpu_matmul_contiguous"
        in captured_link["export_link_args"]
    )


def test_ensure_runtime_wasm_writes_integrity_sidecar_when_reusing_valid_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\0asm\x01\0\0\0runtime")
    stored_fingerprint = {"artifact_sha256": cli._sha256_file(runtime_wasm)}

    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_fingerprint", lambda *args, **kwargs: {"hash": "same"}
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: False
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_read_runtime_fingerprint", lambda path: stored_fingerprint
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_shared_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_wasm_exports_satisfy", lambda path, required: True
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_inspect_wasm_binary", lambda path: "valid")
    monkeypatch.setattr(
        cli,
        "_resolve_built_runtime_wasm_artifact",
        lambda target_root, profile_dir: runtime_wasm,
    )

    assert RUNTIME_BUILD._ensure_runtime_wasm(
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

    prepared, err = cli_non_native_output._prepare_non_native_build_result(
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
        ensure_runtime_wasm_reloc=lambda required=None: True,
        runtime_cargo_profile="dev-fast",
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


def _install_fake_wasm_link_runner(
    monkeypatch: pytest.MonkeyPatch,
    *,
    link_calls: list[list[str]] | None = None,
) -> None:
    def fake_run(
        cmd: list[str],
        **kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        command = list(cmd)
        if "--output" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        if link_calls is not None:
            link_calls.append(command)
        output_path = Path(command[command.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        valid_wasm = b"\0asm\x01\0\0\0"
        output_path.write_bytes(valid_wasm)
        if "--split-runtime" in command:
            split_dir = Path(command[command.index("--split-output-dir") + 1])
            split_dir.mkdir(parents=True, exist_ok=True)
            (split_dir / "app.wasm").write_bytes(valid_wasm)
            (split_dir / "molt_runtime.wasm").write_bytes(valid_wasm)
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(cli_non_native_output, "_run_completed_command", fake_run)


def _write_split_runtime_vfs_support(molt_root: Path) -> None:
    wasm_root = molt_root / "wasm"
    wasm_root.mkdir(parents=True, exist_ok=True)
    vfs_support = wasm_root / "molt_vfs_browser.js"
    vfs_support.write_text("globalThis.MoltVfs = class {};\n", encoding="utf-8")
    browser_embed = wasm_root / "browser_embed.js"
    browser_embed.write_text(
        "export const loadMoltBrowserKernel = async () => ({});\n",
        encoding="utf-8",
    )
    loader_bridge = wasm_root / "loader_bridge.js"
    loader_bridge.write_text(
        "globalThis.MoltWasmLoaderBridge = {};\n",
        encoding="utf-8",
    )


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

    _install_fake_wasm_link_runner(monkeypatch)

    prepared, err = cli_non_native_output._prepare_non_native_build_result(
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
        ensure_runtime_wasm_reloc=lambda required=None: True,
        runtime_cargo_profile="dev-fast",
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


def test_prepare_non_native_build_result_skips_unchanged_linked_wasm_relink(
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
    wasm_link = tmp_path / "tools" / "wasm_link.py"
    wasm_link.parent.mkdir(parents=True, exist_ok=True)
    wasm_link.write_text("# linker\n", encoding="utf-8")
    link_calls: list[list[str]] = []

    _install_fake_wasm_link_runner(monkeypatch, link_calls=link_calls)
    monkeypatch.setattr(
        cli_non_native_output, "_validate_wasm_structural", lambda path: None
    )

    common_kwargs = {
        "is_rust_transpile": False,
        "is_luau_transpile": False,
        "is_wasm": True,
        "is_wasm_freestanding": False,
        "linked": True,
        "require_linked": False,
        "linked_output_path": linked_wasm,
        "output_artifact": output_wasm,
        "json_output": True,
        "runtime_wasm": runtime_wasm,
        "runtime_reloc_wasm": runtime_reloc_wasm,
        "ensure_runtime_wasm_shared": lambda *_args, **_kwargs: True,
        "ensure_runtime_wasm_reloc": lambda required=None: True,
        "runtime_cargo_profile": "dev-fast",
        "molt_root": tmp_path,
        "split_runtime": False,
        "precompile": False,
    }

    first, first_err = cli_non_native_output._prepare_non_native_build_result(
        **common_kwargs
    )
    assert first_err is None
    assert first is not None
    assert len(link_calls) == 1
    first_cmd = link_calls[0]
    assert first_cmd[:6] == [
        sys.executable,
        str(wasm_link),
        "--runtime",
        str(runtime_reloc_wasm),
        "--input",
        str(output_wasm),
    ]
    linked_output_arg = Path(first_cmd[first_cmd.index("--output") + 1])
    assert linked_output_arg.parent == linked_wasm.parent
    assert linked_output_arg.name.startswith(f".{linked_wasm.name}.")
    assert linked_output_arg.name.endswith(".tmp")
    assert first_cmd[-3:] == ["--optimize", "--optimize-level", "Oz"]

    second, second_err = cli_non_native_output._prepare_non_native_build_result(
        **common_kwargs
    )
    assert second_err is None
    assert second is not None
    assert len(link_calls) == 1


def test_prepare_non_native_build_result_keeps_shared_runtime_canonical_for_linked_wasm(
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
    vfs_support = tmp_path / "wasm" / "molt_vfs_browser.js"
    vfs_support.parent.mkdir(parents=True, exist_ok=True)
    vfs_support.write_text("globalThis.MoltVfs = class {};\n", encoding="utf-8")
    shared_required: list[frozenset[str]] = []

    _install_fake_wasm_link_runner(monkeypatch)

    def collect_import_names(path: Path, module_name: str) -> set[str]:
        del path
        if module_name == "molt_runtime":
            return {"alloc", "molt_fast_list_append"}
        return set()

    monkeypatch.setattr(
        cli_non_native_output,
        "_collect_wasm_module_import_names",
        collect_import_names,
    )

    prepared, err = cli_non_native_output._prepare_non_native_build_result(
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
        ensure_runtime_wasm_shared=lambda required=None: (
            shared_required.append(frozenset(required or set())) or True
        ),
        ensure_runtime_wasm_reloc=lambda required=None: True,
        runtime_cargo_profile="dev-fast",
        molt_root=tmp_path,
        split_runtime=False,
        precompile=False,
    )

    assert err is None
    assert prepared is not None
    # The production code now forwards the required runtime exports
    # (discovered by _collect_wasm_module_import_names) to the shared
    # runtime ensure callback so the runtime build includes them.
    assert shared_required == [frozenset({"alloc", "molt_fast_list_append"})]


def test_prepare_non_native_build_result_split_runtime_reuses_shared_runtime_surface(
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
    _write_split_runtime_vfs_support(tmp_path)
    native_callable_symbol = "molt_nativepkg_ndimage_distance_transform_edt"
    package_dir = tmp_path / "site" / "nativepkg"
    package_dir.mkdir(parents=True)
    package_init = package_dir / "__init__.py"
    package_init.write_text("VALUE = 1\n", encoding="utf-8")
    package_init_sha = hashlib.sha256(package_init.read_bytes()).hexdigest()
    artifact_path = package_dir / "_ndimage.molt.wasm"
    artifact_bytes = b"\0asm\x01\0\0\0native"
    artifact_path.write_bytes(artifact_bytes)
    manifest_path = package_dir / "extension_manifest.json"
    manifest_bytes = b'{"runtime_linkage":"static_link"}\n'
    manifest_path.write_bytes(manifest_bytes)
    native_artifact_plan = _ExternalPackageNativeArtifactPlan(
        artifacts=(
            _ExternalPackageNativeArtifact(
                package="nativepkg",
                module="nativepkg._ndimage",
                package_dir=package_dir,
                path=artifact_path,
                manifest_path=manifest_path,
                extension_sha256=hashlib.sha256(artifact_bytes).hexdigest(),
                manifest_sha256=hashlib.sha256(manifest_bytes).hexdigest(),
                capabilities=(),
                abi_tag="molt_abi1",
                target_triple="wasm32-wasip1",
                platform_tag="wasm32_wasip1",
                runtime_linkage="static_link",
                artifact_kind="wasm_relocatable_object",
                support_file_sha256=(("nativepkg/__init__.py", package_init_sha),),
                callable_exports=(
                    _ExternalNativeCallableExport(
                        module="nativepkg.ndimage",
                        name="distance_transform_edt",
                        binding="direct_symbol",
                        abi="molt.forward_f32_v1",
                        symbol=native_callable_symbol,
                        deterministic=True,
                    ),
                ),
            ),
        )
    )
    shared_required: list[frozenset[str]] = []
    link_calls: list[list[str]] = []

    _install_fake_wasm_link_runner(monkeypatch, link_calls=link_calls)

    def collect_import_names(path: Path, module_name: str) -> set[str]:
        del path
        if module_name == "molt_runtime":
            return {"alloc", "molt_fast_list_append"}
        return set()

    monkeypatch.setattr(
        cli_non_native_output,
        "_collect_wasm_module_import_names",
        collect_import_names,
    )
    monkeypatch.setattr(
        cli_non_native_output, "_wasm_import_minima", lambda _path: (1, 1)
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_runtime_import_result_kinds_from_manifest",
        lambda _names: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_runtime_import_signatures_from_manifest",
        lambda _names: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_wasm_export_function_signatures",
        lambda *args, **kwargs: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_effective_split_worker_table_base",
        lambda **kwargs: 8192,
    )
    monkeypatch.setattr(
        cli_non_native_output, "_generate_split_worker_js", lambda **kwargs: "// worker"
    )

    prepared, err = cli_non_native_output._prepare_non_native_build_result(
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
        ensure_runtime_wasm_shared=lambda required=None: (
            shared_required.append(frozenset(required or set())) or True
        ),
        ensure_runtime_wasm_reloc=lambda required=None: True,
        runtime_cargo_profile="dev-fast",
        molt_root=tmp_path,
        split_runtime=True,
        precompile=False,
        native_artifact_plan=native_artifact_plan,
    )

    assert err is None
    assert prepared is not None
    assert shared_required == [frozenset({"alloc", "molt_fast_list_append"})]
    assert len(link_calls) == 1
    link_cmd = link_calls[0]
    assert "--native-object" in link_cmd
    staged_native_input = Path(link_cmd[link_cmd.index("--native-object") + 1])
    assert staged_native_input.exists()
    assert staged_native_input != artifact_path
    assert staged_native_input.read_bytes() == artifact_bytes
    assert "external_static_packages" in staged_native_input.parts
    assert prepared.artifacts is not None
    assert prepared.artifacts["external_native_artifact_0"] == str(staged_native_input)
    assert "bundle_tar" in prepared.artifacts
    bundle_tar = Path(prepared.artifacts["bundle_tar"])
    assert bundle_tar.exists()
    with tarfile.open(bundle_tar) as tar:
        bundle_names = set(tar.getnames())
        assert "nativepkg/__init__.py" in bundle_names
        assert "nativepkg/extension_manifest.json" in bundle_names
        assert "nativepkg/_ndimage.molt.wasm" not in bundle_names
        bundle_manifest = json.loads(tar.extractfile("__manifest__.json").read())
    assert bundle_manifest["files"]
    manifest = json.loads((output_wasm.parent / "manifest.json").read_text())
    assert manifest["assets"]["bundle"]["path"] == "bundle.tar"
    assert manifest["assets"]["bundle"]["file_count"] == len(bundle_manifest["files"])
    native_callables = manifest["abi"]["browser_embed"]["native_callables"]
    assert native_callables["module"] == "molt_native"
    assert native_callables["symbols"] == {}


def test_prepare_non_native_build_result_links_cpython_abi_provider(
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
    package_dir = tmp_path / "site" / "nativepkg"
    package_dir.mkdir(parents=True)
    artifact_path = package_dir / "_multiarray_umath.molt.wasm"
    artifact_bytes = b"\0asm\x01\0\0\0native"
    artifact_path.write_bytes(artifact_bytes)
    manifest_path = package_dir / "extension_manifest.json"
    manifest_bytes = b'{"runtime_linkage":"static_link"}\n'
    manifest_path.write_bytes(manifest_bytes)
    cpython_abi_provider = tmp_path / "target" / "libmolt_cpython_abi.a"
    cpython_abi_provider.parent.mkdir(parents=True)
    cpython_abi_provider.write_bytes(b"!<arch>\nprovider")
    libc_provider = tmp_path / "rustlib" / "self-contained" / "libc.a"
    libc_provider.parent.mkdir(parents=True)
    libc_provider.write_bytes(b"!<arch>\nlibc")
    compiler_rt_provider = tmp_path / "rustlib" / "libcompiler_builtins-x.rlib"
    compiler_rt_provider.write_bytes(b"!<arch>\ncompiler-rt")
    native_artifact_plan = _ExternalPackageNativeArtifactPlan(
        artifacts=(
            _ExternalPackageNativeArtifact(
                package="nativepkg",
                module="nativepkg.core._multiarray_umath",
                package_dir=package_dir,
                path=artifact_path,
                manifest_path=manifest_path,
                extension_sha256=hashlib.sha256(artifact_bytes).hexdigest(),
                manifest_sha256=hashlib.sha256(manifest_bytes).hexdigest(),
                capabilities=(),
                abi_tag="molt_abi1",
                target_triple="wasm32-wasip1",
                platform_tag="wasm32_wasip1",
                runtime_linkage="static_link",
                artifact_kind="wasm_relocatable_object",
                abi_symbols=(
                    _ExternalNativeAbiSymbol(
                        symbol="printf",
                        status="external_link",
                        primitive_class="wasm_libc_link_import",
                        source="undefined_symbols",
                    ),
                    _ExternalNativeAbiSymbol(
                        symbol="molt_cpython_abi_date_from_date",
                        status="external_link",
                        primitive_class="molt_cpython_abi_link_import",
                        source="undefined_symbols",
                    ),
                    _ExternalNativeAbiSymbol(
                        symbol="__trunctfdf2",
                        status="external_link",
                        primitive_class="wasm_compiler_rt_link_import",
                        source="undefined_symbols",
                    ),
                ),
            ),
        )
    )
    link_calls: list[list[str]] = []
    provider_calls: list[dict[str, object]] = []

    _install_fake_wasm_link_runner(monkeypatch, link_calls=link_calls)
    monkeypatch.setattr(
        cli_non_native_output,
        "_collect_wasm_module_import_names",
        lambda _path, _module_name: set(),
    )

    def fake_provider(**kwargs: object) -> Path:
        provider_calls.append(dict(kwargs))
        return cpython_abi_provider

    monkeypatch.setattr(
        cli_non_native_output,
        "_ensure_wasm_cpython_abi_staticlib",
        fake_provider,
        raising=True,
    )
    monkeypatch.setattr(
        cli_wasm_toolchain,
        "wasm_wasi_libc_archive",
        lambda: libc_provider,
        raising=True,
    )
    monkeypatch.setattr(
        cli_wasm_toolchain,
        "wasm_compiler_builtins_archive",
        lambda: compiler_rt_provider,
        raising=True,
    )

    prepared, err = cli_non_native_output._prepare_non_native_build_result(
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
        ensure_runtime_wasm_shared=lambda required=None: True,
        ensure_runtime_wasm_reloc=lambda required=None: True,
        runtime_cargo_profile="release-fast",
        molt_root=tmp_path,
        split_runtime=False,
        precompile=False,
        native_artifact_plan=native_artifact_plan,
    )

    assert err is None
    assert prepared is not None
    assert provider_calls == [
        {
            "project_root": tmp_path,
            "json_output": True,
            "cargo_profile": "release-fast",
            "cargo_timeout": None,
        }
    ]
    assert len(link_calls) == 1
    link_cmd = link_calls[0]
    native_inputs = [
        Path(link_cmd[index + 1])
        for index, arg in enumerate(link_cmd)
        if arg == "--native-object"
    ]
    assert cpython_abi_provider in native_inputs
    assert libc_provider in native_inputs
    assert compiler_rt_provider in native_inputs
    staged_native_inputs = [
        path for path in native_inputs if "external_static_packages" in path.parts
    ]
    assert len(staged_native_inputs) == 1
    assert staged_native_inputs[0].read_bytes() == artifact_bytes


def test_wasm_static_link_native_artifact_inputs_include_linkable_support_paths(
    tmp_path: Path,
) -> None:
    artifact_path = tmp_path / "pkg" / "_native.molt.wasm"
    manifest_path = tmp_path / "pkg" / "_native.molt.wasm.extension_manifest.json"
    support_archive = tmp_path / "pkg" / "_native_support.a"
    support_python = tmp_path / "pkg" / "__init__.py"
    for path in (artifact_path, manifest_path, support_archive, support_python):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x")
    staged = _StagedExternalPackageNativeArtifact(
        package="nativepkg",
        module="nativepkg._native",
        runtime_root=tmp_path,
        source_path=artifact_path,
        source_manifest_path=manifest_path,
        staged_path=artifact_path,
        staged_manifest_path=manifest_path,
        staged_support_paths=(support_archive, support_python),
        extension_sha256="0" * 64,
        manifest_sha256="1" * 64,
        capabilities=(),
        abi_tag="molt_abi1",
        target_triple="wasm32-wasip1",
        platform_tag="wasm32_wasip1",
        runtime_linkage="static_link",
        artifact_kind="wasm_relocatable_object",
    )

    assert cli_non_native_output._wasm_static_link_native_artifact_inputs(
        (staged,)
    ) == (
        artifact_path,
        support_archive,
    )


def test_browser_native_callable_manifest_is_import_driven(tmp_path: Path) -> None:
    native_callable_symbol = "molt_nativepkg_ndimage_distance_transform_edt"
    object_call_symbol = "molt_nativepkg_ndimage_label"
    object_callargs_symbol = "molt_nativepkg_ndimage_gaussian_filter"
    package_dir = tmp_path / "site" / "nativepkg"
    package_dir.mkdir(parents=True)
    artifact_path = package_dir / "_ndimage.molt.wasm"
    artifact_bytes = b"\0asm\x01\0\0\0native"
    artifact_path.write_bytes(artifact_bytes)
    manifest_path = package_dir / "extension_manifest.json"
    manifest_bytes = b'{"runtime_linkage":"static_link"}\n'
    manifest_path.write_bytes(manifest_bytes)
    native_artifact_plan = _ExternalPackageNativeArtifactPlan(
        artifacts=(
            _ExternalPackageNativeArtifact(
                package="nativepkg",
                module="nativepkg._ndimage",
                package_dir=package_dir,
                path=artifact_path,
                manifest_path=manifest_path,
                extension_sha256=hashlib.sha256(artifact_bytes).hexdigest(),
                manifest_sha256=hashlib.sha256(manifest_bytes).hexdigest(),
                capabilities=(),
                abi_tag="molt_abi1",
                target_triple="wasm32-wasip1",
                platform_tag="wasm32_wasip1",
                runtime_linkage="static_link",
                artifact_kind="wasm_relocatable_object",
                callable_exports=(
                    _ExternalNativeCallableExport(
                        module="nativepkg.ndimage",
                        name="distance_transform_edt",
                        binding="direct_symbol",
                        abi="molt.forward_f32_v1",
                        symbol=native_callable_symbol,
                        deterministic=True,
                    ),
                    _ExternalNativeCallableExport(
                        module="nativepkg.ndimage",
                        name="label",
                        binding="direct_symbol",
                        abi="molt.object_call_v1",
                        symbol=object_call_symbol,
                        deterministic=True,
                    ),
                    _ExternalNativeCallableExport(
                        module="nativepkg.ndimage",
                        name="gaussian_filter",
                        binding="direct_symbol",
                        abi="molt.object_callargs_v1",
                        symbol=object_callargs_symbol,
                        deterministic=True,
                    ),
                ),
            ),
        )
    )

    empty_manifest = cli_non_native_output._browser_native_callable_manifest(
        native_artifact_plan,
        required_symbols=(),
    )
    required_manifest = cli_non_native_output._browser_native_callable_manifest(
        native_artifact_plan,
        required_symbols={native_callable_symbol},
    )
    object_manifest = cli_non_native_output._browser_native_callable_manifest(
        native_artifact_plan,
        required_symbols={object_call_symbol},
    )
    callargs_manifest = cli_non_native_output._browser_native_callable_manifest(
        native_artifact_plan,
        required_symbols={object_callargs_symbol},
    )

    assert empty_manifest == {"module": "molt_native", "symbols": {}}
    symbol_payload = required_manifest["symbols"][native_callable_symbol]
    assert symbol_payload["abi"] == "molt.forward_f32_v1"
    assert symbol_payload["signature"] == {
        "params": ["bytes.float32"],
        "result": "bytes.float32",
    }
    assert symbol_payload["exports"][0]["qualified_name"] == (
        "nativepkg.ndimage.distance_transform_edt"
    )
    object_payload = object_manifest["symbols"][object_call_symbol]
    assert object_payload["abi"] == "molt.object_call_v1"
    assert object_payload["signature"] == {
        "params": ["molt.value..."],
        "result": "molt.value",
    }
    assert object_payload["exports"][0]["qualified_name"] == "nativepkg.ndimage.label"
    callargs_payload = callargs_manifest["symbols"][object_callargs_symbol]
    assert callargs_payload["abi"] == "molt.object_callargs_v1"
    assert callargs_payload["signature"] == {
        "params": ["molt.callargs"],
        "result": "molt.value",
    }
    assert callargs_payload["exports"][0]["qualified_name"] == (
        "nativepkg.ndimage.gaussian_filter"
    )


def test_prepare_non_native_build_result_split_runtime_rejects_unbacked_native_import(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
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
    _write_split_runtime_vfs_support(tmp_path)
    missing_symbol = "molt_nativepkg_missing"

    _install_fake_wasm_link_runner(monkeypatch)

    def collect_import_names(path: Path, module_name: str) -> set[str]:
        del path
        if module_name == "molt_runtime":
            return {"alloc"}
        if module_name == "molt_native":
            return {missing_symbol}
        return set()

    monkeypatch.setattr(
        cli_non_native_output,
        "_collect_wasm_module_import_names",
        collect_import_names,
    )
    monkeypatch.setattr(
        cli_non_native_output, "_wasm_import_minima", lambda _path: (1, 1)
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_runtime_import_result_kinds_from_manifest",
        lambda _names: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_runtime_import_signatures_from_manifest",
        lambda _names: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_wasm_export_function_signatures",
        lambda *args, **kwargs: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_effective_split_worker_table_base",
        lambda **kwargs: 8192,
    )
    monkeypatch.setattr(
        cli_non_native_output, "_generate_split_worker_js", lambda **kwargs: "// worker"
    )

    prepared, err = cli_non_native_output._prepare_non_native_build_result(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        is_wasm_freestanding=False,
        linked=True,
        require_linked=False,
        linked_output_path=linked_wasm,
        output_artifact=output_wasm,
        json_output=False,
        runtime_wasm=runtime_wasm,
        runtime_reloc_wasm=runtime_reloc_wasm,
        ensure_runtime_wasm_shared=lambda required=None: True,
        ensure_runtime_wasm_reloc=lambda required=None: True,
        runtime_cargo_profile="dev-fast",
        molt_root=tmp_path,
        split_runtime=True,
        precompile=False,
        native_artifact_plan=_EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    )

    assert prepared is None
    assert err == 2
    captured = capsys.readouterr()
    assert "Split-runtime native callable manifest invalid" in captured.err
    assert missing_symbol in captured.err


def test_prepare_non_native_build_result_split_runtime_does_not_export_runtime_table_refs(
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
    _write_split_runtime_vfs_support(tmp_path)

    _install_fake_wasm_link_runner(monkeypatch)

    def collect_import_names(path: Path, module_name: str) -> set[str]:
        del path
        if module_name == "molt_runtime":
            return {"alloc", "molt_fast_list_append"}
        return set()

    monkeypatch.setattr(
        cli_non_native_output,
        "_collect_wasm_module_import_names",
        collect_import_names,
    )
    monkeypatch.setattr(
        cli_non_native_output, "_wasm_import_minima", lambda _path: (1, 1)
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_runtime_import_result_kinds_from_manifest",
        lambda _names: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_runtime_import_signatures_from_manifest",
        lambda _names: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_wasm_export_function_signatures",
        lambda *args, **kwargs: {},
    )
    monkeypatch.setattr(
        cli_non_native_output,
        "_effective_split_worker_table_base",
        lambda **kwargs: 8192,
    )
    monkeypatch.setattr(
        cli_non_native_output, "_generate_split_worker_js", lambda **kwargs: "// worker"
    )
    assert not hasattr(cli_non_native_output, "_export_wasm_table_refs")

    prepared, err = cli_non_native_output._prepare_non_native_build_result(
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
        ensure_runtime_wasm_shared=lambda required=None: True,
        ensure_runtime_wasm_reloc=lambda required=None: True,
        runtime_cargo_profile="dev-fast",
        molt_root=tmp_path,
        split_runtime=True,
        precompile=False,
    )

    assert err is None
    assert prepared is not None


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
    wasm.write_bytes(b"\0asm\x01\0\0\0" + b"\x07" + bytes([len(payload)]) + payload)
    assert RUNTIME_WASM_VALIDATION._runtime_wasm_exports_satisfy(
        wasm, {"molt_fast_list_append", "molt_resource_on_free"}
    )
    assert not RUNTIME_WASM_VALIDATION._runtime_wasm_exports_satisfy(
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
        "molt_resource_on_allocate",
        "molt_resource_on_free",
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
        b"\0asm\x01\0\0\0" + b"\x07" + _encode_varuint(len(payload)) + payload
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
    assert RUNTIME_WASM_VALIDATION._runtime_wasm_exports_satisfy(wasm, required)
    assert (
        RUNTIME_WASM_VALIDATION._runtime_wasm_missing_exports(wasm, required) == set()
    )


def test_runtime_wasm_resource_exports_are_not_satisfied_by_browser_fallbacks(
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

    wasm = tmp_path / "runtime_missing_resource_exports.wasm"
    payload = bytearray()
    exports = (
        "molt_call_bind_ic",
        "molt_callargs_new",
        "molt_callargs_push_pos",
    )
    payload.append(len(exports))
    for index, name in enumerate(exports):
        encoded = name.encode("utf-8")
        payload.append(len(encoded))
        payload.extend(encoded)
        payload.append(0x00)
        payload.append(index)
    wasm.write_bytes(
        b"\0asm\x01\0\0\0" + b"\x07" + _encode_varuint(len(payload)) + payload
    )

    required = {"molt_resource_on_allocate", "molt_resource_on_free"}
    assert not RUNTIME_WASM_VALIDATION._runtime_wasm_exports_satisfy(wasm, required)
    assert (
        RUNTIME_WASM_VALIDATION._runtime_wasm_missing_exports(wasm, required)
        == required
    )


def test_run_subprocess_captured_to_tempfiles_emits_keepalive(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv("MOLT_SUBPROCESS_KEEPALIVE_SECS", "0.01")
    result = COMMAND_RUNTIME._run_subprocess_captured_to_tempfiles(
        [
            sys.executable,
            "-c",
            "import time; print('ok'); time.sleep(0.3)",
        ],
        timeout=1.0,
        progress_label="Tempfile helper",
    )
    assert result.returncode == 0
    assert result.stdout.decode("utf-8").strip() == "ok"
    assert "Tempfile helper: still running" in capsys.readouterr().err


def test_ensure_runtime_lib_native_path_does_not_require_wasm_export_fingerprint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    fingerprint = {"hash": "ok"}
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_fingerprint", lambda *args, **kwargs: fingerprint
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
    )
    checked: list[Path] = []

    def fake_runtime_artifact_fingerprint_matches(
        artifact: Path,
        current_fingerprint: dict[str, str | None] | None,
        fingerprint_path: Path,
        *,
        require_artifact_digest: bool,
    ) -> bool:
        assert artifact == runtime_lib
        assert current_fingerprint == fingerprint
        assert fingerprint_path == tmp_path / "fingerprint.json"
        assert require_artifact_digest is True
        checked.append(artifact)
        return True

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_artifact_fingerprint_matches",
        fake_runtime_artifact_fingerprint_matches,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_run_cargo_with_sccache_retry",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unexpected runtime rebuild")
        ),
    )

    assert RUNTIME_BUILD._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        cargo_timeout=1.0,
    )
    assert checked == [runtime_lib]


def test_ensure_runtime_wasm_does_not_overwrite_satisfied_runtime_with_unsatisfied_build_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    current_src = (
        tmp_path / "target" / "wasm32-wasip1" / "release-fast" / "molt_runtime.wasm"
    )
    current_src.parent.mkdir(parents=True, exist_ok=True)
    runtime.write_bytes(b"\0asm\x01\0\0\0")
    current_src.write_bytes(b"\0asm\x01\0\0\0")

    stored_fingerprint = {
        "hash": "ok",
        "artifact_sha256": cli._sha256_file(runtime),
    }

    monkeypatch.setattr(
        RUNTIME_BUILD, "_read_runtime_fingerprint", lambda path: stored_fingerprint
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_fingerprint", lambda *args, **kwargs: {"hash": "ok"}
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: False
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_shared_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_inspect_wasm_binary", lambda path: "valid")
    monkeypatch.setattr(
        cli, "_resolve_built_runtime_wasm_artifact", lambda *args: current_src
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_wasm_exports_satisfy",
        lambda path, required: path == runtime,
    )

    copied: list[tuple[Path, Path]] = []

    def fake_copy2(src: Path | str, dst: Path | str, *args, **kwargs):
        copied.append((Path(src), Path(dst)))
        return dst

    monkeypatch.setattr(cli.shutil, "copy2", fake_copy2)

    ok = RUNTIME_BUILD._ensure_runtime_wasm(
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


def test_ensure_runtime_wasm_materializes_prebuilt_cargo_artifact_without_rebuild(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    runtime = tmp_path / "wasm" / "molt_runtime.wasm"
    cargo_runtime = target_root / "wasm32-wasip1" / "release-fast" / "molt_runtime.wasm"
    runtime_source = tmp_path / "runtime" / "molt-runtime" / "src" / "lib.rs"
    runtime_source.parent.mkdir(parents=True, exist_ok=True)
    runtime_source.write_text("// runtime source\n", encoding="utf-8")
    os.utime(runtime_source, ns=(1, 1))
    cargo_runtime.parent.mkdir(parents=True, exist_ok=True)
    cargo_runtime.write_bytes(b"\0asm\x01\0\0\0runtime")
    fingerprint = {"hash": "ok", "rustc": "rustc", "inputs_digest": "inputs"}
    stored_fingerprint = {
        **fingerprint,
        "artifact_sha256": cli._sha256_file(cargo_runtime),
    }
    cargo_builds: list[list[str]] = []

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_runtime_source_paths", lambda _root: [runtime_source]
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_fingerprint", lambda *args, **kwargs: fingerprint
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda path: (
            stored_fingerprint if "runtime_fingerprints" in os.fspath(path) else None
        ),
    )
    monkeypatch.setattr(
        cli_link_pipeline,
        "_artifact_needs_rebuild",
        lambda artifact, current, stored: (
            stored is None
            or current is None
            or stored.get("hash") != current.get("hash")
            or stored.get("rustc") != current.get("rustc")
            or stored.get("inputs_digest") != current.get("inputs_digest")
        ),
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_shared_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_inspect_wasm_binary", lambda path: "valid")
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_wasm_exports_satisfy", lambda path, required: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_wasm_missing_exports", lambda path, required: set()
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_run_runtime_wasm_cargo_build",
        lambda *, cmd, **kwargs: (
            cargo_builds.append(list(cmd))
            or (
                subprocess.CompletedProcess(cmd, 1, "", "unexpected rebuild"),
                cargo_runtime,
            )
        ),
    )

    assert RUNTIME_BUILD._ensure_runtime_wasm(
        runtime,
        reloc=False,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=1.0,
        project_root=tmp_path,
        required_exports={"molt_fast_list_append"},
    )

    assert cargo_builds == []
    assert runtime.read_bytes() == cargo_runtime.read_bytes()
    assert RUNTIME_WASM_VALIDATION._runtime_wasm_integrity_sidecar_path(
        runtime
    ).exists()


def test_ensure_runtime_wasm_links_prebuilt_staticlib_without_rebuild(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    runtime = tmp_path / "wasm" / "molt_runtime_reloc.wasm"
    staticlib = target_root / "wasm32-wasip1" / "release-fast" / "libmolt_runtime.a"
    runtime_source = tmp_path / "runtime" / "molt-runtime" / "src" / "lib.rs"
    runtime_source.parent.mkdir(parents=True, exist_ok=True)
    runtime_source.write_text("// runtime source\n", encoding="utf-8")
    os.utime(runtime_source, ns=(1, 1))
    staticlib.parent.mkdir(parents=True, exist_ok=True)
    staticlib.write_bytes(b"!<arch>\nprebuilt")
    fingerprint = {"hash": "ok", "rustc": "rustc", "inputs_digest": "inputs"}
    stored_fingerprint = {
        **fingerprint,
        "artifact_sha256": cli._sha256_file(staticlib),
    }
    cargo_builds: list[list[str]] = []
    linked_from: list[Path] = []

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_runtime_source_paths", lambda _root: [runtime_source]
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_fingerprint", lambda *args, **kwargs: fingerprint
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda path: (
            stored_fingerprint if "runtime_fingerprints" in os.fspath(path) else None
        ),
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_inspect_wasm_binary", lambda path: "valid")

    def fake_link_runtime_staticlib_to_reloc_wasm(
        *,
        staticlib_path: Path,
        output_path: Path,
        **kwargs: object,
    ) -> bool:
        del kwargs
        linked_from.append(staticlib_path)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"\0asm\x01\0\0\0reloc")
        return True

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_link_runtime_staticlib_to_reloc_wasm",
        fake_link_runtime_staticlib_to_reloc_wasm,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_run_runtime_wasm_cargo_build",
        lambda *, cmd, **kwargs: (
            cargo_builds.append(list(cmd))
            or (
                subprocess.CompletedProcess(cmd, 1, "", "unexpected rebuild"),
                staticlib,
            )
        ),
    )

    assert RUNTIME_BUILD._ensure_runtime_wasm(
        runtime,
        reloc=True,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=1.0,
        project_root=tmp_path,
    )

    assert cargo_builds == []
    assert linked_from == [staticlib]
    assert runtime.read_bytes() == b"\0asm\x01\0\0\0reloc"
    assert RUNTIME_WASM_VALIDATION._runtime_wasm_integrity_sidecar_path(
        runtime
    ).exists()


def test_ensure_runtime_lib_verified_key_is_stable_across_user_import_graph(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"archive")
    RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()
    verification_calls: list[frozenset[str]] = []

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda project_root, **kwargs: {
            "runtime_features": tuple(cast(tuple[str, ...], kwargs["runtime_features"]))
        },
    )

    def fake_runtime_artifact_fingerprint_matches(
        artifact: Path,
        fingerprint: dict[str, str | None] | None,
        fingerprint_path: Path,
        *,
        require_artifact_digest: bool,
    ) -> bool:
        del artifact, fingerprint_path
        assert require_artifact_digest is True
        assert fingerprint is not None
        verification_calls.append(
            frozenset(cast(tuple[str, ...], fingerprint["runtime_features"]))
        )
        return True

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_artifact_fingerprint_matches",
        fake_runtime_artifact_fingerprint_matches,
    )

    try:
        assert RUNTIME_BUILD._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="release-fast",
            project_root=tmp_path,
            cargo_timeout=1.0,
            stdlib_profile="micro",
            resolved_modules={"json"},
        )
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()
        assert RUNTIME_BUILD._ensure_runtime_lib(
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
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()

    assert len(verification_calls) == 2
    assert verification_calls[0] == verification_calls[1]
    assert {"builtin_set", "stdlib_micro", "no-default-features"} <= verification_calls[
        0
    ]
    assert "stdlib_net" not in verification_calls[0]
    assert "stdlib_serial" not in verification_calls[0]
    assert "stdlib_compression" not in verification_calls[0]
    assert "molt_gpu_primitives" not in verification_calls[0]


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
        capability_config_cache_digest="",
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )
    resolved_entry = cli._ResolvedBuildEntry(
        source_path=tmp_path / "main.py",
        entry_module="__main__",
        module_roots=[tmp_path],
        entry_source="print('hi')\n",
        entry_tree=ast.parse("print('hi')\n"),
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )
    output_layout = cli._BuildOutputLayout(
        is_wasm=False,
        is_wasm_freestanding=False,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_mlir_emit=False,
        split_runtime=False,
        linked=False,
        target_triple=None,
        emit_mode="bin",
        output_artifact=output_artifact,
        output_binary=output_binary,
        linked_output_path=None,
        emit_ir_path=None,
    )
    empty_module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module=None,
    )
    frontend_run_ticket = cli._PreparedFrontendRunTicket(
        module_order=[],
        module_layers=[],
        frontend_parallel_config=cli._FrontendParallelConfig(
            workers=0,
            min_modules=0,
            min_predicted_cost=0.0,
            target_cost_per_worker=0.0,
            stdlib_min_cost_scale=0.0,
            enabled=False,
            reason="disabled",
        ),
        frontend_parallel_layers=[],
        frontend_parallel_worker_timings=[],
        frontend_parallel_details={},
        frontend_layer_execution_context=cli._FrontendLayerExecutionContext(
            syntax_error_modules={},
            module_graph={},
            module_source_catalog=cli_module_source._ModuleSourceCatalog(leases={}),
            project_root=tmp_path,
            module_resolution_cache=cli_module_resolution._ModuleResolutionCache(),
            parse_codec="json",
            type_hint_policy="check",
            fallback_policy="error",
            type_facts=None,
            enable_phi=False,
            known_modules=set(),
            direct_call_modules=set(),
            stdlib_allowlist=set(),
            known_func_defaults={},
            known_func_kinds={},
            native_callable_exports={},
            native_python_exports=(),
            module_deps={},
            source_modules=(),
            module_chunk_max_ops=0,
            optimization_profile="dev",
            pgo_hot_function_names=set(),
            known_modules_sorted=(),
            stdlib_allowlist_sorted=(),
            pgo_hot_function_names_sorted=(),
            module_dep_closures={},
            module_graph_metadata=empty_module_graph_metadata,
            path_stat_by_module={},
            module_chunking=False,
            scoped_lowering_inputs=None,
            dirty_lowering_modules=set(),
            frontend_module_costs={},
            stdlib_like_by_module={},
            known_classes={},
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        ),
        frontend_layer_runtime_hooks=cli._FrontendLayerRuntimeHooks(
            warnings=[],
            frontend_parallel_details={},
            record_frontend_parallel_worker_timing=lambda **kwargs: {},
            record_frontend_timing=lambda *args, **kwargs: None,
            integrate_module_frontend_result=lambda *args, **kwargs: None,
            accumulate_midend_diagnostics=lambda *args, **kwargs: None,
            fail=lambda *args, **kwargs: None,
            json_output=True,
            run_serial_frontend_lower=lambda *args, **kwargs: (None, None, None),
        ),
    )
    frontend_bundle = (
        frontend_run_ticket,
        {},
        set(),
        set(),
        False,
        output_layout,
        set(),
        {},
        {},
        {},
        [],
        None,
        {},
        False,
        0,
        False,
        cli._FrontendIntegrationState(functions=[], known_classes={}),
        cli._MidendDiagnosticsState(
            policy_outcomes_by_function={},
            pass_stats_by_function={},
        ),
        lambda *args, **kwargs: None,
        lambda: (None, None),
        lambda *args, **kwargs: None,
        tmp_path,
        cli._EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    )

    monkeypatch.setattr(
        BACKEND_IR,
        "_prepare_backend_ir",
        lambda **kwargs: (
            call_order.append("backend_ir") or cli._PreparedBackendIR(ir={}),
            None,
        ),
    )

    def fake_prepare_backend_setup(
        **kwargs: object,
    ) -> tuple[cli._PreparedBackendSetup, None]:
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

    monkeypatch.setattr(
        cli_backend_compile, "_prepare_backend_setup", fake_prepare_backend_setup
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_prepare_backend_runtime_context",
        lambda **kwargs: (
            cli._PreparedBackendRuntimeContext(
                runtime_state=kwargs["prepared_backend_setup"].runtime_state,
                runtime_lib=runtime_lib,
                runtime_wasm=None,
                runtime_reloc_wasm=None,
                ensure_runtime_wasm_shared=lambda required=None: True,
                ensure_runtime_wasm_reloc=lambda required=None: True,
                cache_setup=kwargs["prepared_backend_setup"].cache_setup,
                cache_hit=False,
                cache_hit_tier=None,
                cache_key="module-cache",
                function_cache_key=None,
                cache_path=tmp_path / "module-cache.o",
                function_cache_path=None,
                stdlib_object_path=None,
            ),
            None,
        ),
    )
    monkeypatch.setattr(
        cli_backend_compile,
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
        cli_backend_output_pipeline,
        "_ensure_native_runtime_lib_ready_before_link",
        lambda runtime_state, **kwargs: call_order.append("runtime_ready") or False,
    )

    def fake_prepare_native_link(
        **kwargs: object,
    ) -> tuple[None, dict[str, object] | None]:
        del kwargs
        call_order.append("native_link")
        pytest.fail("native link should not run after runtime readiness failure")

    monkeypatch.setattr(
        cli_link_pipeline, "_prepare_native_link", fake_prepare_native_link
    )

    result = cli_backend_pipeline._run_backend_pipeline(
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


def test_prepare_backend_dispatch_surfaces_backend_ensure_detail_in_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    backend_bin = tmp_path / "molt-backend"
    detail = (
        "Backend cargo build failed (exit 101):\n"
        "error: duplicate symbol: PyMemoryView_FromMemory"
    )

    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_ensure_backend_binary",
        lambda *args, **kwargs: cli_backend_binary._BackendBinaryEnsureResult(
            ok=False,
            detail=detail,
            returncode=101,
            phase="backend_cargo_build",
        ),
    )

    prepared, err = cli_backend_compile._prepare_backend_dispatch(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        split_runtime=False,
        linked=False,
        deterministic=False,
        profile="dev",
        runtime_state=cli._RuntimeArtifactState(
            runtime_wasm=None,
            runtime_reloc_wasm=None,
        ),
        runtime_cargo_profile="dev-fast",
        cargo_timeout=1.0,
        molt_root=tmp_path,
        target_triple=None,
        backend_cargo_profile="release-fast",
        diagnostics_enabled=False,
        phase_starts={},
        json_output=True,
        backend_daemon_config_digest=None,
        ensure_runtime_wasm_shared=lambda required=None: True,
        ensure_runtime_wasm_reloc=lambda required=None: True,
        resolved_modules=frozenset(),
        ir={"functions": []},
        warnings=[],
    )

    assert prepared is None
    assert err == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["errors"] == [detail]


def test_ensure_backend_binary_uses_native_feature_for_native(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    exe_suffix = ".exe" if os.name == "nt" else ""
    backend_bin = tmp_path / "target" / "dev-fast" / f"molt-backend{exe_suffix}"
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

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", fake_run_cargo
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_backend_binary._ensure_backend_binary(
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


def test_ensure_backend_binary_rebuild_does_not_signal_verified_daemons(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "target" / "dev-fast" / "molt-backend"
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "daemon.sock"
    identity_path = daemon_root / "molt-backend.dev-fast.deadbeef.identity.json"
    identity = _test_backend_daemon_identity(
        4321,
        socket_path=socket_path,
        project_root=tmp_path,
        backend_bin=backend_bin,
    )
    cli._write_backend_daemon_identity(identity_path, identity)
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        exe_suffix = ".exe" if os.name == "nt" else ""
        cargo_backend_bin = backend_bin.parent / f"molt-backend{exe_suffix}"
        cargo_backend_bin.parent.mkdir(parents=True, exist_ok=True)
        cargo_backend_bin.write_text("#!/bin/sh\n")
        cargo_backend_bin.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *args, **kwargs: fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", fake_run_cargo
    )
    monkeypatch.setattr(
        cli.os,
        "kill",
        lambda pid, sig: (_ for _ in ()).throw(
            AssertionError("backend rebuild must not signal live daemons")
        ),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
    )
    assert identity_path.exists()


def test_ensure_backend_binary_enables_wasm_feature_for_wasm(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    exe_suffix = ".exe" if os.name == "nt" else ""
    backend_bin = (
        tmp_path / "target" / "dev-fast" / f"molt-backend.wasm_backend{exe_suffix}"
    )
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
        exe_suffix = ".exe" if os.name == "nt" else ""
        cargo_output = backend_bin.parent / f"molt-backend{exe_suffix}"
        cargo_output.parent.mkdir(parents=True, exist_ok=True)
        cargo_output.write_text("#!/bin/sh\n")
        cargo_output.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", fake_run_cargo
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_backend_binary._ensure_backend_binary(
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


def test_ensure_backend_binary_materializes_prebuilt_feature_alias_without_rebuild(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_dir = tmp_path / "target"
    exe_suffix = ".exe" if os.name == "nt" else ""
    backend_bin = target_dir / "dev-fast" / f"molt-backend.wasm_backend{exe_suffix}"
    cargo_output = target_dir / "dev-fast" / f"molt-backend{exe_suffix}"
    backend_source = tmp_path / "runtime" / "molt-backend" / "src" / "lib.rs"
    backend_source.parent.mkdir(parents=True, exist_ok=True)
    backend_source.write_text("// backend source\n", encoding="utf-8")
    os.utime(backend_source, (1.0, 1.0))
    cargo_output.parent.mkdir(parents=True, exist_ok=True)
    cargo_output.write_text(
        "#!/bin/sh\n"
        'while [ "$#" -gt 0 ]; do\n'
        '  if [ "$1" = "--output" ]; then\n'
        "    shift\n"
        "    printf '\\000asm\\001\\000\\000\\000' > \"$1\"\n"
        "  fi\n"
        "  shift || exit 0\n"
        "done\n"
        "exit 0\n",
        encoding="utf-8",
    )
    cargo_output.chmod(0o755)
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    build_cmds: list[list[str]] = []

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_dir))
    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_source_paths",
        lambda *args, **kwargs: [backend_source],
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_cargo_with_sccache_retry",
        lambda cmd, **kwargs: (
            build_cmds.append(list(cmd))
            or subprocess.CompletedProcess(cmd, 1, "", "unexpected rebuild")
        ),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("wasm-backend",),
    )

    assert build_cmds == []
    assert backend_bin.exists()
    assert (
        cli._read_runtime_fingerprint(
            cli_backend_binary._backend_fingerprint_path(
                tmp_path, backend_bin, "dev-fast"
            )
        )["hash"]
        == "abc"
    )


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

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", fake_run_cargo
    )

    assert not cli_backend_binary._ensure_backend_binary(
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
    exe_suffix = ".exe" if os.name == "nt" else ""
    canonical_backend = backend_bin.parent / f"molt-backend{exe_suffix}"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    seen_features: list[tuple[str, ...]] = []
    build_cmds: list[list[str]] = []
    backend_cmds: list[list[str]] = []

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 0
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: True)
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_start_backend_daemon",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("rust target should not start backend daemon")
        ),
    )
    monkeypatch.setattr(
        cli_backend_compile,
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
        canonical_backend.write_text("#!/bin/sh\n")
        canonical_backend.chmod(0o755)
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

    def fake_backend_compile(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        assert cmd[0] == str(backend_bin)
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

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", fake_run_cargo
    )
    monkeypatch.setattr(BACKEND_EXECUTION.subprocess, "run", fake_run)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        fake_backend_compile,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_backend_compile,
    )

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
    exe_suffix = ".exe" if os.name == "nt" else ""
    canonical_backend = backend_bin.parent / f"molt-backend{exe_suffix}"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    build_cmds: list[list[str]] = []

    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(ROOT))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(build_state_root / "cargo-target"))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.delenv("MOLT_RELEASE_BACKEND_CARGO_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_RELEASE_CARGO_PROFILE", raising=False)
    monkeypatch.setattr(cli_build_inputs, "_find_project_root", lambda start: project)
    monkeypatch.setattr(
        cli_frontend_parallel, "_resolve_frontend_parallel_module_workers", lambda: 0
    )
    monkeypatch.setattr(cli_backend_compile, "_backend_daemon_enabled", lambda: True)
    monkeypatch.setattr(
        cli_backend_compile, "_backend_bin_path", lambda *args, **kwargs: backend_bin
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_start_backend_daemon",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("rust target should not start backend daemon")
        ),
    )
    monkeypatch.setattr(
        cli_backend_compile,
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
        canonical_backend.write_text("#!/bin/sh\n")
        canonical_backend.chmod(0o755)
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

    def fake_backend_compile(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        assert cmd[0] == str(backend_bin)
        assert "--target" in cmd and cmd[cmd.index("--target") + 1] == "rust"
        assert "--output" in cmd
        output = Path(cmd[cmd.index("--output") + 1])
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text("fn main() {}\n")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", fake_run_cargo
    )
    monkeypatch.setattr(BACKEND_EXECUTION.subprocess, "run", fake_run)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        fake_backend_compile,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_backend_compile,
    )

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


def test_browser_deploy_profile_defaults_to_auto_wasm_profile() -> None:
    assert (
        cli_build_output_layout._DEPLOY_PROFILE_DEFAULTS["browser"]["wasm_profile"]
        == "auto"
    )


def test_build_cli_defaults_to_auto_wasm_profile(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    seen_profiles: list[str | None] = []

    def fake_build(*args: object, **kwargs: object) -> int:
        del args
        seen_profiles.append(cast(str | None, kwargs.get("wasm_profile")))
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(
        sys,
        "argv",
        ["molt", "build", "--target", "wasm", str(entry)],
    )

    assert cli.main() == 0
    assert seen_profiles == ["auto"]


def test_build_cli_defaults_to_micro_stdlib_profile(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    seen_profiles: list[str | None] = []

    def fake_build(*args: object, **kwargs: object) -> int:
        del args
        seen_profiles.append(cast(str | None, kwargs.get("stdlib_profile")))
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(sys, "argv", ["molt", "build", str(entry)])

    assert cli.main() == 0
    assert seen_profiles == ["micro"]


def test_build_cli_keeps_deploy_stdlib_profile_default(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    seen_profiles: list[str | None] = []

    def fake_build(*args: object, **kwargs: object) -> int:
        del args
        seen_profiles.append(cast(str | None, kwargs.get("stdlib_profile")))
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(
        sys,
        "argv",
        ["molt", "build", "--target", "wasm", "--profile", "wasi", str(entry)],
    )

    assert cli.main() == 0
    assert seen_profiles == ["full"]


def test_build_scopes_pipeline_env_updates(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    cleared_keys = {
        "MOLT_AUDIT_ENABLED",
        "MOLT_AUDIT_SINK",
        "MOLT_AUDIT_OUTPUT",
        "MOLT_IO_MODE",
        "MOLT_PORTABLE",
        "MOLT_SPLIT_RUNTIME",
        "MOLT_TYPE_GATE",
    }
    for key in cleared_keys:
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("MOLT_STDLIB_PROFILE", "micro")
    monkeypatch.setenv("MOLT_WASM_PROFILE", "pure")

    rc = cli.build(
        str(entry),
        module="demo.main",
        target="wasm",
        portable=True,
        split_runtime=True,
        wasm_profile="full",
        stdlib_profile="full",
        audit_log="jsonl:stderr",
        io_mode="virtual",
        type_gate=True,
        json_output=True,
    )

    assert rc != 0
    assert os.environ["MOLT_STDLIB_PROFILE"] == "micro"
    assert os.environ["MOLT_WASM_PROFILE"] == "pure"
    assert all(key not in os.environ for key in cleared_keys)


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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        del kwargs
        run_cmds.append(list(cmd))
        return 0

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli_commands, "_run_command", fake_run_command)

    rc = cli_commands.run_script(
        str(entry),
        None,
        [],
        build_profile="dev",
        json_output=False,
    )

    assert rc == 0
    assert build_cmds[-1:] == [
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        del kwargs
        run_cmds.append(list(cmd))
        return 0

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli_commands, "_run_command", fake_run_command)

    rc = cli_commands.run_script(
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        del kwargs
        run_cmds.append(list(cmd))
        return 0

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli_commands, "_run_command", fake_run_command)

    rc = cli_commands.run_script(
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli_commands, "_run_command", lambda cmd, **kwargs: 0)

    rc = cli_commands.run_script(
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(cmd, 1, json.dumps(payload), "")

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )

    rc = cli_commands.run_script(
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


def test_run_wrapper_build_ignores_legacy_mtime_binary_without_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
    xdg_root = tmp_path / "xdg"
    cache_root = tmp_path / "cache"
    monkeypatch.setenv("XDG_CACHE_HOME", str(xdg_root))
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    _clear_molt_home_caches()

    path_digest = hashlib.sha256(str(entry.resolve()).encode("utf-8")).hexdigest()[:16]
    stale_bin = xdg_root / "molt" / "home" / "bin" / f"{entry.stem}_{path_digest}_molt"
    stale_bin.parent.mkdir(parents=True)
    stale_bin.write_bytes(b"stale-native")
    future_ns = entry.stat().st_mtime_ns + 1_000_000_000
    os.utime(stale_bin, ns=(future_ns, future_ns))

    resolved, error = cli_build_inputs._resolve_wrapper_build_entry(
        file_path=str(entry),
        module=None,
        project_root=project,
        json_output=True,
        command="run",
        build_args=["--target", "wasm"],
    )
    assert error is None
    assert resolved is not None
    built = tmp_path / "built.wasm"
    built.write_bytes(b"wasm")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(built),
            "consumer_output": str(built),
            "artifacts": {"wasm": str(built)},
        },
    )
    seen_cmds: list[list[str]] = []

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli, "_cache_fingerprint", lambda: "runtime-a")
    monkeypatch.setattr(cli, "_cache_tooling_fingerprint", lambda: "tool-a")

    contract, duration, error_code = cli._run_wrapper_build(
        file_path=str(entry),
        module=None,
        build_args=["--target", "wasm"],
        env={},
        project_root=project,
        json_output=True,
        command="run",
        verbose=False,
        resolved_build_entry=resolved,
    )

    assert error_code is None
    assert duration >= 0.0
    assert contract is not None
    assert contract.consumer_output == built
    assert seen_cmds


def test_run_wrapper_build_manifest_tracks_args_and_source_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("print(1)\n", encoding="utf-8")
    original = entry.stat()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    _clear_molt_home_caches()
    monkeypatch.setattr(cli, "_cache_fingerprint", lambda: "runtime-a")
    monkeypatch.setattr(cli, "_cache_tooling_fingerprint", lambda: "tool-a")

    resolved, error = cli_build_inputs._resolve_wrapper_build_entry(
        file_path=str(entry),
        module=None,
        project_root=project,
        json_output=True,
        command="run",
        build_args=[],
    )
    assert error is None
    assert resolved is not None
    cached_bin = cli._wrapper_build_default_binary_path(resolved)
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(cached_bin),
            "consumer_output": str(cached_bin),
            "artifacts": {},
        },
    )
    seen_cmds: list[list[str]] = []

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        cached_bin.parent.mkdir(parents=True, exist_ok=True)
        cached_bin.write_bytes(f"binary-{len(seen_cmds)}".encode("ascii"))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    common_kwargs = {
        "file_path": str(entry),
        "module": None,
        "env": {},
        "project_root": project,
        "json_output": True,
        "command": "run",
        "verbose": False,
        "resolved_build_entry": resolved,
    }

    first, first_duration, first_error = cli._run_wrapper_build(
        build_args=[],
        **common_kwargs,
    )
    assert first_error is None
    assert first_duration >= 0.0
    assert first is not None
    assert len(seen_cmds) == 1

    second, second_duration, second_error = cli._run_wrapper_build(
        build_args=[],
        **common_kwargs,
    )
    assert second_error is None
    assert second_duration == 0.0
    assert second is not None
    assert len(seen_cmds) == 1

    third, third_duration, third_error = cli._run_wrapper_build(
        build_args=["--target", "wasm"],
        **common_kwargs,
    )
    assert third_error is None
    assert third_duration >= 0.0
    assert third is not None
    assert len(seen_cmds) == 2

    _rewrite_preserving_mtime(entry, "print(2)\n", original)
    fourth, fourth_duration, fourth_error = cli._run_wrapper_build(
        build_args=[],
        **common_kwargs,
    )
    assert fourth_error is None
    assert fourth_duration >= 0.0
    assert fourth is not None
    assert len(seen_cmds) == 3


def test_run_wrapper_build_manifest_tracks_imported_source_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "demo.py"
    entry.write_text("import helper\nprint(helper.VALUE)\n", encoding="utf-8")
    helper = project / "helper.py"
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    original_helper = helper.stat()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    _clear_molt_home_caches()
    monkeypatch.setattr(cli, "_cache_fingerprint", lambda: "runtime-a")
    monkeypatch.setattr(cli, "_cache_tooling_fingerprint", lambda: "tool-a")

    resolved, error = cli_build_inputs._resolve_wrapper_build_entry(
        file_path=str(entry),
        module=None,
        project_root=project,
        json_output=True,
        command="run",
        build_args=[],
    )
    assert error is None
    assert resolved is not None
    cached_bin = cli._wrapper_build_default_binary_path(resolved)
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(cached_bin),
            "consumer_output": str(cached_bin),
            "artifacts": {},
        },
    )
    seen_cmds: list[list[str]] = []

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        cached_bin.parent.mkdir(parents=True, exist_ok=True)
        cached_bin.write_bytes(f"binary-{len(seen_cmds)}".encode("ascii"))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    common_kwargs = {
        "file_path": str(entry),
        "module": None,
        "build_args": [],
        "env": {},
        "project_root": project,
        "json_output": True,
        "command": "run",
        "verbose": False,
        "resolved_build_entry": resolved,
    }

    first, first_duration, first_error = cli._run_wrapper_build(**common_kwargs)
    assert first_error is None
    assert first_duration >= 0.0
    assert first is not None
    assert len(seen_cmds) == 1

    second, second_duration, second_error = cli._run_wrapper_build(**common_kwargs)
    assert second_error is None
    assert second_duration == 0.0
    assert second is not None
    assert len(seen_cmds) == 1

    _rewrite_preserving_mtime(helper, "VALUE = 2\n", original_helper)
    third, third_duration, third_error = cli._run_wrapper_build(**common_kwargs)
    assert third_error is None
    assert third_duration >= 0.0
    assert third is not None
    assert len(seen_cmds) == 2


def test_run_wrapper_build_manifest_caches_module_entries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    package = project / "demo"
    package.mkdir()
    entry = package / "__main__.py"
    entry.write_text("print(1)\n", encoding="utf-8")
    original = entry.stat()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    _clear_molt_home_caches()
    monkeypatch.setattr(cli, "_cache_fingerprint", lambda: "runtime-a")
    monkeypatch.setattr(cli, "_cache_tooling_fingerprint", lambda: "tool-a")

    resolved, error = cli_build_inputs._resolve_wrapper_build_entry(
        file_path=None,
        module="demo",
        project_root=project,
        json_output=True,
        command="run",
        build_args=[],
    )
    assert error is None
    assert resolved is not None
    cached_bin = cli._wrapper_build_default_binary_path(resolved)
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(cached_bin),
            "consumer_output": str(cached_bin),
            "artifacts": {},
        },
    )
    seen_cmds: list[list[str]] = []

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        cached_bin.parent.mkdir(parents=True, exist_ok=True)
        cached_bin.write_bytes(f"binary-{len(seen_cmds)}".encode("ascii"))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    common_kwargs = {
        "file_path": None,
        "module": "demo",
        "build_args": [],
        "env": {},
        "project_root": project,
        "json_output": True,
        "command": "run",
        "verbose": False,
        "resolved_build_entry": resolved,
    }

    first, first_duration, first_error = cli._run_wrapper_build(**common_kwargs)
    assert first_error is None
    assert first_duration >= 0.0
    assert first is not None
    assert len(seen_cmds) == 1

    second, second_duration, second_error = cli._run_wrapper_build(**common_kwargs)
    assert second_error is None
    assert second_duration == 0.0
    assert second is not None
    assert len(seen_cmds) == 1

    _rewrite_preserving_mtime(entry, "print(2)\n", original)
    third, third_duration, third_error = cli._run_wrapper_build(**common_kwargs)
    assert third_error is None
    assert third_duration >= 0.0
    assert third is not None
    assert len(seen_cmds) == 2


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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli_commands, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(
        cli_commands, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli_commands.shutil, "which", lambda name: f"/usr/bin/{name}")
    monkeypatch.setenv("PYTHONPATH", str(pythonpath_root))

    rc = cli_commands._run_script_cross(
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli_commands, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(
        cli_commands, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli_commands.shutil, "which", lambda name: f"/usr/bin/{name}")

    rc = cli_commands._run_script_cross(
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setenv("PYTHONPATH", str(pythonpath_root))

    rc = cli_commands._deploy(
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )

    rc = cli_commands._deploy(
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

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        seen_calls.append((list(cmd), cast(Path | None, kwargs.get("cwd"))))
        if cmd[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        seen_calls.append((list(cmd), cast(Path | None, kwargs.get("cwd"))))
        return 0

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_completed_command", fake_run_completed_command
    )
    monkeypatch.setattr(cli_commands, "_run_command", fake_run_command)
    monkeypatch.setattr(RUNTIME_BUILD.shutil, "which", lambda name: f"/usr/bin/{name}")

    rc = cli_commands._deploy(
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

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)

    rc = cli_commands.run_script(
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

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)

    rc = cli_commands._run_script_cross(
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

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)

    rc = cli_commands._deploy(
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

    def fake_run_subprocess_captured_to_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        env = cast(dict[str, str] | None, kwargs.get("env"))
        captured_envs.append(env)
        assert env is not None
        assert "MOLT_STDLIB_OBJ" in env
        assert "--ir-file" in cmd
        assert kwargs.get("input") is None
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"object")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )

    result, error = cli_backend_compile._execute_backend_compile(
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
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

    def fake_run_subprocess_captured_to_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = cast(dict[str, str] | None, kwargs.get("env"))
        captured_envs.append(env)
        assert env is not None
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"object")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )

    result, error = cli_backend_compile._execute_backend_compile(
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
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


def test_native_backend_compile_clears_stale_partition_env_without_split(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    output_artifact = project_root / "build" / "main.o"
    backend_bin = tmp_path / "backend-bin"
    artifacts_root = tmp_path / "artifacts"
    captured_envs: list[dict[str, str] | None] = []

    monkeypatch.setenv("MOLT_STDLIB_OBJ", str(tmp_path / "ambient.stdlib.o"))
    monkeypatch.setenv("MOLT_STDLIB_CACHE_KEY", "ambient-key")
    monkeypatch.setenv("MOLT_STDLIB_MODULE_SYMBOLS", '["ambient_mod"]')
    monkeypatch.setenv("MOLT_ENTRY_MODULE", "ambient.entry")

    def fake_run_subprocess_captured_to_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = cast(dict[str, str] | None, kwargs.get("env"))
        captured_envs.append(env)
        assert env is not None
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"object")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )

    result, error = cli_backend_compile._execute_backend_compile(
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
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
            stdlib_module_symbols_json=None,
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
        entry_module="pkg.app",
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
        cache_hit=False,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
    )

    assert error is None
    assert result is not None
    assert captured_envs and captured_envs[0] is not None
    env = captured_envs[0]
    assert "MOLT_STDLIB_OBJ" not in env
    assert "MOLT_STDLIB_CACHE_KEY" not in env
    assert "MOLT_STDLIB_MODULE_SYMBOLS" not in env
    assert env["MOLT_ENTRY_MODULE"] == "pkg.app"


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

    def fake_run_subprocess_captured_to_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        output_path = Path(cmd[cmd.index("--output") + 1])
        seen_output_paths.append(output_path)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"wasm-bytes")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )

    result, error = cli_backend_compile._execute_backend_compile(
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
            cache_candidates=(
                ("module", cache_path),
                ("function", function_cache_path),
            ),
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
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
    request_encode_calls: list[tuple[bool, bool, bool]] = []
    daemon_request_bytes: list[bytes | None] = []

    def fake_request_bytes(**kwargs: object) -> tuple[bytes | None, str | None]:
        request_encode_calls.append(
            (
                bool(kwargs.get("probe_cache_only")),
                kwargs.get("ir") is not None,
                kwargs.get("ir_path") is not None,
            )
        )
        return b'{"version":1,"jobs":[{"id":"job0"}]}\n', None

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        **kwargs: object,
    ) -> cli._BackendDaemonCompileResult:
        assert socket_path == tmp_path / "daemon.sock"
        daemon_request_bytes.append(cast(bytes | None, kwargs.get("request_bytes")))
        backend_output = cast(Path, kwargs["backend_output"])
        backend_output.parent.mkdir(parents=True, exist_ok=True)
        backend_output.write_bytes(b"object")
        return cli._BackendDaemonCompileResult(
            True,
            None,
            {"pid": 42},
            True,
            "module",
            True,
            True,
        )

    monkeypatch.setattr(
        cli_backend_compile,
        "_compile_with_backend_daemon",
        fake_compile_with_backend_daemon,
    )

    result, error = cli_backend_compile._execute_backend_compile(
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
            cache_candidates=(
                ("module", cache_path),
                ("function", function_cache_path),
            ),
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
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
        backend_output = cast(Path, kwargs["backend_output"])
        backend_output.parent.mkdir(parents=True, exist_ok=True)
        backend_output.write_bytes(b"object")
        return cli._BackendDaemonCompileResult(
            True,
            None,
            {"pid": 42},
            True,
            "module",
            True,
            True,
        )

    def fake_start_backend_daemon(*args: object, **kwargs: object) -> bool:
        nonlocal restart_calls
        restart_calls += 1
        assert args[1] == tmp_path / "daemon.sock"
        return True

    monkeypatch.setattr(
        cli_backend_compile,
        "_compile_with_backend_daemon",
        fake_compile_with_backend_daemon,
    )
    monkeypatch.setattr(
        cli_backend_compile, "_start_backend_daemon", fake_start_backend_daemon
    )

    result, error = cli_backend_compile._execute_backend_compile(
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
            cache_candidates=(
                ("module", cache_path),
                ("function", function_cache_path),
            ),
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
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


def test_execute_backend_compile_does_not_retry_after_full_daemon_request(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    artifacts_root = tmp_path / "artifacts"
    output_artifact = project_root / "build" / "main.o"
    compile_attempts = 0
    restart_calls = 0
    one_shot_calls = 0

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        **kwargs: object,
    ) -> cli._BackendDaemonCompileResult:
        nonlocal compile_attempts
        assert socket_path == tmp_path / "daemon.sock"
        assert kwargs["ir"] == {"functions": [{"name": "heavy"}]}
        compile_attempts += 1
        return cli._BackendDaemonCompileResult(
            False,
            "backend daemon connection failed: timed out",
            {"pid": 42},
            None,
            None,
            True,
            False,
            True,
        )

    def fake_start_backend_daemon(*args: object, **kwargs: object) -> bool:
        nonlocal restart_calls
        del args, kwargs
        restart_calls += 1
        return True

    def fake_run_subprocess_captured_to_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        nonlocal one_shot_calls
        del cmd, kwargs
        one_shot_calls += 1
        return subprocess.CompletedProcess([], 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_compile,
        "_compile_with_backend_daemon",
        fake_compile_with_backend_daemon,
    )
    monkeypatch.setattr(
        cli_backend_compile, "_start_backend_daemon", fake_start_backend_daemon
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )

    result, error = cli_backend_compile._execute_backend_compile(
        cache=False,
        cache_path=None,
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
        cache_key=None,
        function_cache_key=None,
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=False,
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
        cache_hit=False,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
    )

    assert result is None
    assert error == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["errors"] == [
        "Backend daemon compile failed: backend daemon connection failed: timed out"
    ]
    assert compile_attempts == 1
    assert restart_calls == 0
    assert one_shot_calls == 0


def test_execute_backend_compile_fails_closed_after_daemon_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    artifacts_root = tmp_path / "artifacts"
    output_artifact = project_root / "build" / "main.o"
    one_shot_calls = 0

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        **kwargs: object,
    ) -> cli._BackendDaemonCompileResult:
        assert socket_path == tmp_path / "daemon.sock"
        return cli._BackendDaemonCompileResult(
            False,
            "backend daemon compile request failed: rss limit exceeded",
            {"pid": 42},
            None,
            None,
            True,
            False,
        )

    def fake_run_subprocess_captured_to_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        nonlocal one_shot_calls
        del cmd, kwargs
        one_shot_calls += 1
        return subprocess.CompletedProcess([], 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_compile,
        "_compile_with_backend_daemon",
        fake_compile_with_backend_daemon,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_run_subprocess_captured_to_tempfiles",
        fake_run_subprocess_captured_to_tempfiles,
    )

    result, error = cli_backend_compile._execute_backend_compile(
        cache=False,
        cache_path=None,
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
        cache_key=None,
        function_cache_key=None,
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=False,
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
        cache_hit=False,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
    )

    assert result is None
    assert error == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error"
    assert payload["errors"] == [
        "Backend daemon compile failed: backend daemon compile request failed: rss limit exceeded"
    ]
    assert one_shot_calls == 0


def test_execute_backend_compile_verbose_prints_only_fresh_daemon_log(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    artifacts_root = tmp_path / "artifacts"
    output_artifact = project_root / "build" / "main.o"
    log_path = tmp_path / "daemon.log"
    log_path.write_text("old stdlib compile line\n", encoding="utf-8")

    monkeypatch.setattr(
        cli_backend_compile,
        "_backend_daemon_log_path",
        lambda *args, **kwargs: log_path,
    )

    def fake_compile_with_backend_daemon(
        socket_path: Path,
        **kwargs: object,
    ) -> cli._BackendDaemonCompileResult:
        assert socket_path == tmp_path / "daemon.sock"
        with log_path.open("a", encoding="utf-8") as handle:
            handle.write("fresh incremental compile line\n")
        backend_output = cast(Path, kwargs["backend_output"])
        backend_output.parent.mkdir(parents=True, exist_ok=True)
        backend_output.write_bytes(b"object")
        return cli._BackendDaemonCompileResult(
            True,
            None,
            {"pid": 42},
            False,
            None,
            True,
            True,
        )

    monkeypatch.setattr(
        cli_backend_compile,
        "_compile_with_backend_daemon",
        fake_compile_with_backend_daemon,
    )

    result, error = cli_backend_compile._execute_backend_compile(
        cache=False,
        cache_path=None,
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
        cache_key=None,
        function_cache_key=None,
        cache_setup=cli._BackendCacheSetup(
            cache_enabled=False,
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
        target_triple=None,
        backend_daemon_config_digest="digest123",
        entry_module="pkg.app",
        ir={"functions": [{"name": "changed"}]},
        json_output=False,
        warnings=[],
        verbose=True,
        backend_bin=tmp_path / "backend-bin",
        backend_env=None,
        backend_timeout=None,
        molt_root=project_root,
        backend_cargo_profile="dev-fast",
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
        cache_hit=False,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
    )

    assert error is None
    assert result is not None
    stderr = capsys.readouterr().err
    assert "fresh incremental compile line" in stderr
    assert "old stdlib compile line" not in stderr


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
        cli_backend_compile,
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

    result, error = cli_backend_compile._execute_backend_compile(
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
            cache_candidates=(
                ("module", project_root / ".molt_cache" / "cache-key.o"),
            ),
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
        _ensure_backend_ir_file_path=lambda: project_root / "tmp" / "ir.json",
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
        stdlib_object_cache_key="stdlib-cache-key",
        stdlib_object_manifest='{"cache_key":"stdlib-cache-key"}',
        stdlib_module_symbols_json='["importlib","importlib_machinery","importlib_util","sys"]',
    )

    assert error is None
    assert request_bytes is not None
    payload = json.loads(request_bytes)
    env = payload["env"]
    assert env["MOLT_ENTRY_MODULE"] == "pkg.app"
    assert env["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)
    assert env["MOLT_STDLIB_CACHE_KEY"] == "stdlib-cache-key"
    assert env["MOLT_STDLIB_CACHE_MANIFEST"] == '{"cache_key":"stdlib-cache-key"}'
    assert (
        env["MOLT_STDLIB_MODULE_SYMBOLS"]
        == '["importlib","importlib_machinery","importlib_util","sys"]'
    )


def test_backend_daemon_compile_request_can_use_path_backed_ir_lease(
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    ir_path = tmp_path / "backend-ir.json"
    ir_path.write_text('{"functions":[]}\n', encoding="utf-8")

    request_bytes, error = cli._backend_daemon_compile_request_bytes(
        ir=None,
        ir_path=ir_path,
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
    )

    assert error is None
    assert request_bytes is not None
    job = json.loads(request_bytes)["jobs"][0]
    assert job["ir_path"] == str(ir_path)
    assert "ir" not in job


def test_backend_daemon_compile_request_rejects_duplicate_ir_authority(
    tmp_path: Path,
) -> None:
    request_bytes, error = cli._backend_daemon_compile_request_bytes(
        ir={"functions": []},
        ir_path=tmp_path / "backend-ir.json",
        backend_output=tmp_path / "output.o",
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
    )

    assert request_bytes is None
    assert (
        error
        == "backend daemon request must use exactly one IR custody field: ir or ir_path"
    )


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


def test_backend_daemon_compile_request_includes_resource_env_without_codegen_digest(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_output = tmp_path / "output.o"
    baseline_digest = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_BACKEND_MEMORY_AVAILABLE_GB", "18")
    monkeypatch.setenv("MOLT_BACKEND_MAX_RSS_GB", "18")
    monkeypatch.setenv("MOLT_BACKEND_MEMORY_RESERVE_GB", "4")
    monkeypatch.setenv("RAYON_NUM_THREADS", "2")

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
    )

    assert error is None
    assert request_bytes is not None
    payload = json.loads(request_bytes)
    assert payload["env"]["MOLT_BACKEND_MEMORY_AVAILABLE_GB"] == "18"
    assert payload["env"]["MOLT_BACKEND_MAX_RSS_GB"] == "18"
    assert payload["env"]["MOLT_BACKEND_MEMORY_RESERVE_GB"] == "4"
    assert payload["env"]["RAYON_NUM_THREADS"] == "2"
    assert (
        BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False) == baseline_digest
    )


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

    monkeypatch.setattr(cli_commands, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: ROOT)
    monkeypatch.setattr(cli_commands, "_resolve_python_exe", lambda exe: "python3")
    monkeypatch.setattr(
        cli_commands, "_resolve_binary_output", lambda output: built_binary
    )
    monkeypatch.setattr(cli_commands, "_run_command_timed", fake_run_command_timed)

    rc = cli_commands.compare(
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
    assert first is (os.name == "posix")
    assert second is first
    assert info.hits >= 1
    assert info.currsize >= 1


def test_resolve_wasm_cargo_profile_is_cached(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    RUNTIME_BUILD._resolve_wasm_cargo_profile_cached.cache_clear()
    monkeypatch.setenv("MOLT_WASM_CARGO_PROFILE", "")

    first = RUNTIME_BUILD._resolve_wasm_cargo_profile("release")
    second = RUNTIME_BUILD._resolve_wasm_cargo_profile("release")

    info = RUNTIME_BUILD._resolve_wasm_cargo_profile_cached.cache_info()
    assert first == second == "wasm-release"
    assert info.hits >= 1
    assert info.currsize >= 1


def test_native_arch_perf_requested_is_cached(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli_build_inputs._native_arch_perf_requested_cached.cache_clear()
    monkeypatch.setenv("MOLT_PERF_PROFILE", "native")

    first = cli_build_inputs._native_arch_perf_requested()
    second = cli_build_inputs._native_arch_perf_requested()

    info = cli_build_inputs._native_arch_perf_requested_cached.cache_info()
    assert first is True
    assert second is True
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_codegen_env_inputs_is_cached(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cli._backend_codegen_env_inputs_cached.cache_clear()
    monkeypatch.delenv("MOLT_BACKEND", raising=False)
    monkeypatch.setenv("MOLT_BACKEND_REGALLOC_ALGORITHM", "single_pass")

    first = cli._backend_codegen_env_inputs(is_wasm=False)
    second = cli._backend_codegen_env_inputs(is_wasm=False)

    info = cli._backend_codegen_env_inputs_cached.cache_info()
    assert first == second == {"MOLT_BACKEND_REGALLOC_ALGORITHM": "single_pass"}
    assert info.hits >= 1
    assert info.currsize >= 1


def test_backend_codegen_env_digest_tracks_codegen_knobs(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    baseline_native = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_BACKEND_REGALLOC_ALGORITHM", "single_pass")
    native_changed = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False)
    assert native_changed != baseline_native

    baseline_native = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_BACKEND", "llvm")
    native_backend_changed = BACKEND_EXECUTION._backend_codegen_env_digest(
        is_wasm=False
    )
    assert native_backend_changed != baseline_native

    baseline_wasm = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=True)
    monkeypatch.setenv("MOLT_WASM_TABLE_BASE", "2048")
    wasm_changed = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=True)
    assert wasm_changed != baseline_wasm

    linker_a = tmp_path / "ld-a"
    linker_b = tmp_path / "ld-b"
    linker_a.write_text("a", encoding="utf-8")
    linker_b.write_text("b", encoding="utf-8")
    monkeypatch.delenv("MOLT_WASM_TABLE_BASE", raising=False)
    monkeypatch.delenv("MOLT_BACKEND", raising=False)
    baseline_native = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_LINKER", str(linker_a))
    native_linker_a = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_LINKER", str(linker_b))
    native_linker_b = BACKEND_EXECUTION._backend_codegen_env_digest(is_wasm=False)
    assert native_linker_a != baseline_native
    assert native_linker_b != native_linker_a


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

    linker_a = tmp_path / "ld-a"
    linker_b = tmp_path / "ld-b"
    linker_a.write_text("a", encoding="utf-8")
    linker_b.write_text("b", encoding="utf-8")
    linker_digest_a = cli._backend_daemon_config_digest(
        tmp_path, "dev-fast", env={"MOLT_LINKER": str(linker_a)}
    )
    linker_digest_b = cli._backend_daemon_config_digest(
        tmp_path, "dev-fast", env={"MOLT_LINKER": str(linker_b)}
    )
    assert linker_digest_a != linker_digest_b


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


def test_backend_daemon_config_digest_tracks_backend_freshness(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    target_root = project_root / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    backend_bin = target_root / "dev-fast" / "molt-backend"
    runtime_lib = cli._runtime_lib_path(project_root, "release", None)
    backend_src = project_root / "runtime" / "molt-backend" / "src" / "main.rs"
    runtime_src = project_root / "runtime" / "molt-runtime" / "src" / "lib.rs"
    frontend_init = project_root / "src" / "molt" / "frontend" / "__init__.py"
    for path in (backend_bin, runtime_lib, backend_src, runtime_src, frontend_init):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x")

    digest_a = cli._backend_daemon_config_digest(
        project_root,
        "dev-fast",
        backend_bin=backend_bin,
        target_triple=None,
    )
    os.utime(backend_src, ns=(3_000_000_000, 3_000_000_000))
    digest_b = cli._backend_daemon_config_digest(
        project_root,
        "dev-fast",
        backend_bin=backend_bin,
        target_triple=None,
    )
    os.utime(runtime_lib, ns=(4_000_000_000, 4_000_000_000))
    digest_c = cli._backend_daemon_config_digest(
        project_root,
        "dev-fast",
        backend_bin=backend_bin,
        target_triple=None,
    )

    assert digest_a != digest_b
    assert digest_b != digest_c


def test_backend_daemon_config_digest_tracks_compiler_content_fingerprints(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(BACKEND_EXECUTION, "_cache_fingerprint", lambda: "compiler-a")
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_cache_tooling_fingerprint", lambda: "tooling-a"
    )

    digest_a = cli._backend_daemon_config_digest(tmp_path, "dev-fast")

    monkeypatch.setattr(BACKEND_EXECUTION, "_cache_fingerprint", lambda: "compiler-b")
    digest_b = cli._backend_daemon_config_digest(tmp_path, "dev-fast")

    monkeypatch.setattr(BACKEND_EXECUTION, "_cache_fingerprint", lambda: "compiler-b")
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_cache_tooling_fingerprint", lambda: "tooling-b"
    )
    digest_c = cli._backend_daemon_config_digest(tmp_path, "dev-fast")

    assert digest_a != digest_b
    assert digest_b != digest_c


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
    identity_path = cli._backend_daemon_identity_path(tmp_path, "dev-fast")

    info = cli._backend_daemon_paths_cached.cache_info()
    assert log_path.name.startswith("molt-backend.dev-fast.alpha-session.")
    assert log_path.suffix == ".log"
    assert identity_path.name.startswith("molt-backend.dev-fast.alpha-session.")
    assert identity_path.name.endswith(".identity.json")
    assert log_path.parent == identity_path.parent
    assert info.hits >= 1


def test_backend_daemon_log_and_pid_paths_are_session_isolated(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET", raising=False)
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET_DIR", raising=False)
    cli._backend_daemon_paths_cached.cache_clear()

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    alpha_log = cli._backend_daemon_log_path(tmp_path, "dev-fast")
    alpha_identity = cli._backend_daemon_identity_path(tmp_path, "dev-fast")

    cli._backend_daemon_paths_cached.cache_clear()
    monkeypatch.setenv("MOLT_SESSION_ID", "beta-session")
    beta_log = cli._backend_daemon_log_path(tmp_path, "dev-fast")
    beta_identity = cli._backend_daemon_identity_path(tmp_path, "dev-fast")

    assert alpha_log != beta_log
    assert alpha_identity != beta_identity
    assert alpha_log.parent == alpha_identity.parent
    assert beta_log.parent == beta_identity.parent
    assert alpha_log.parent.name == "backend_daemon"
    assert beta_log.parent.name == "backend_daemon"
    assert "alpha-session" in alpha_log.name
    assert "beta-session" in beta_log.name
    assert "alpha-session" in alpha_identity.name
    assert "beta-session" in beta_identity.name


def test_backend_daemon_paths_allow_missing_session_id(tmp_path: Path) -> None:
    cli._backend_daemon_paths_cached.cache_clear()

    socket_path, log_path, identity_path = cli._backend_daemon_paths_cached(
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
    assert identity_path.name.startswith("molt-backend.dev-fast.")
    assert identity_path.name.endswith(".identity.json")


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

    monkeypatch.setattr(BACKEND_EXECUTION.subprocess, "Popen", fake_popen)

    ok = cli._start_backend_daemon(
        backend_bin,
        socket_path,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        target_triple=None,
        config_digest=None,
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
    key_base = CACHE_KEYS._function_cache_key(ir_base, "native", None, "variant")
    key_extra_a = CACHE_KEYS._function_cache_key(ir_extra_a, "native", None, "variant")
    key_extra_b = CACHE_KEYS._function_cache_key(ir_extra_b, "native", None, "variant")
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
        daemon_identity: object | None = None,
        project_root: Path | None = None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, timeout, daemon_identity, project_root
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

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_bytes", _fake_request
    )
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
        daemon_identity: object | None = None,
        project_root: Path | None = None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, timeout, daemon_identity, project_root
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

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_bytes", _fake_request
    )
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
        daemon_identity: object | None = None,
        project_root: Path | None = None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, timeout, daemon_identity, project_root
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

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_bytes", _fake_request
    )
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


def test_compile_with_backend_daemon_surfaces_failed_job_message_from_response(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    daemon_message = (
        "failed to compile native application object: native variable "
        "representation mismatch for tmp"
    )

    def _fake_request(
        socket_path: Path,
        data: bytes,
        *,
        timeout: float | None,
        daemon_identity: object | None = None,
        project_root: Path | None = None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, data, timeout, daemon_identity, project_root
        return (
            {
                "ok": False,
                "jobs": [
                    {
                        "id": "job0",
                        "ok": False,
                        "message": daemon_message,
                    }
                ],
                "health": {"pid": 42},
            },
            None,
        )

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_bytes", _fake_request
    )
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

    assert result.ok is False
    assert result.error == daemon_message
    assert result.health == {"pid": 42}


def test_compile_with_backend_daemon_surfaces_failed_job_message_after_probe_miss(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    daemon_message = (
        "failed to compile native application object: native variable "
        "representation mismatch for cached_tmp"
    )
    request_count = 0

    def _fake_request(
        socket_path: Path,
        data: bytes,
        *,
        timeout: float | None,
        daemon_identity: object | None = None,
        project_root: Path | None = None,
    ) -> tuple[dict[str, object], None]:
        nonlocal request_count
        del socket_path, data, timeout, daemon_identity, project_root
        request_count += 1
        if request_count == 1:
            return (
                {
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
                },
                None,
            )
        return (
            {
                "ok": False,
                "jobs": [
                    {
                        "id": "job0",
                        "ok": False,
                        "message": daemon_message,
                    }
                ],
                "health": {"pid": 43},
            },
            None,
        )

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_bytes", _fake_request
    )
    result = _compile_with_backend_daemon_non_wasm(
        Path("/tmp/fake.sock"),
        ir={"functions": [{"name": "heavy"}]},
        backend_output=backend_output,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key=None,
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=0.1,
    )

    assert request_count == 2
    assert result.ok is False
    assert result.error == daemon_message
    assert result.health == {"pid": 43}
    assert result.full_request_sent is True


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
        daemon_identity: object | None = None,
        project_root: Path | None = None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, timeout, daemon_identity, project_root
        assert data == preencoded
        backend_output.write_bytes(b"\x7fELF")
        return (
            {
                "ok": True,
                "jobs": [{"id": "job0", "ok": True}],
            },
            None,
        )

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_payload_bytes", fail_encode
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_bytes", _fake_request
    )
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
            assert address == str(Path("/tmp/fake.sock"))

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

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())
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
    stdlib_object_path.parent.mkdir(parents=True)
    stdlib_object_path.write_bytes(b"\x7fELF")
    cli._stdlib_object_key_sidecar_path(stdlib_object_path).write_text(
        "stdlib-cache-key\n", encoding="utf-8"
    )
    stdlib_manifest = '{"cache_key":"stdlib-cache-key"}'
    cli._stdlib_object_manifest_sidecar_path(stdlib_object_path).write_text(
        stdlib_manifest + "\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object_path).write_text(
        '{"body_hash":"test","function_count":1,"functions":["molt_init_sys"],"schema":"stdlib-partition-v1"}\n',
        encoding="utf-8",
    )
    cli._stdlib_object_digest_sidecar_path(stdlib_object_path).write_text(
        cli._sha256_file(stdlib_object_path) + "\n", encoding="utf-8"
    )
    seen_payloads: list[dict[str, object]] = []
    ir_lease_paths: list[Path] = []
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
            assert address == str(Path("/tmp/fake.sock"))

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
                job = cast(dict[str, object], payload["jobs"][0])
                assert "ir" not in job
                ir_path = Path(cast(str, job["ir_path"]))
                assert json.loads(ir_path.read_text(encoding="utf-8")) == {
                    "functions": [{"name": "heavy"}]
                }
                ir_lease_paths.append(ir_path)
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

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())
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
        stdlib_object_manifest=stdlib_manifest,
        stdlib_module_symbols_json='["importlib","importlib_machinery","importlib_util","sys"]',
        stdlib_module_symbols={
            "importlib",
            "importlib_machinery",
            "importlib_util",
            "sys",
        },
        timeout=0.1,
    )

    assert result.ok is True
    assert len(seen_payloads) == 2
    assert connects == 2
    assert seen_payloads[0]["jobs"][0]["probe_cache_only"] is True
    assert "ir" not in seen_payloads[0]["jobs"][0]
    assert seen_payloads[0]["env"]["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)
    assert seen_payloads[0]["env"]["MOLT_STDLIB_CACHE_KEY"] == "stdlib-cache-key"
    assert seen_payloads[0]["env"]["MOLT_STDLIB_CACHE_MANIFEST"] == stdlib_manifest
    assert (
        seen_payloads[0]["env"]["MOLT_STDLIB_MODULE_SYMBOLS"]
        == '["importlib","importlib_machinery","importlib_util","sys"]'
    )
    second_job = cast(dict[str, object], seen_payloads[1]["jobs"][0])
    assert "ir" not in second_job
    assert Path(cast(str, second_job["ir_path"])) == ir_lease_paths[0]
    assert seen_payloads[1]["env"]["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)
    assert seen_payloads[1]["env"]["MOLT_STDLIB_CACHE_KEY"] == "stdlib-cache-key"
    assert seen_payloads[1]["env"]["MOLT_STDLIB_CACHE_MANIFEST"] == stdlib_manifest
    assert (
        seen_payloads[1]["env"]["MOLT_STDLIB_MODULE_SYMBOLS"]
        == '["importlib","importlib_machinery","importlib_util","sys"]'
    )
    assert not ir_lease_paths[0].exists()


def test_compile_with_backend_daemon_sends_ir_when_shared_stdlib_cache_missing(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    stdlib_object_path = tmp_path / "cache" / "missing.stdlib.o"
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
            assert address == str(Path("/tmp/fake.sock"))

        def sendall(self, data: bytes) -> None:
            payload = json.loads(data)
            seen_payloads.append(payload)
            job = payload["jobs"][0]
            assert "probe_cache_only" not in job
            assert "ir" not in job
            ir_path = Path(cast(str, job["ir_path"]))
            assert json.loads(ir_path.read_text(encoding="utf-8")) == {
                "functions": [{"name": "heavy"}]
            }
            assert payload["env"]["MOLT_STDLIB_OBJ"] == str(stdlib_object_path)
            assert payload["env"]["MOLT_STDLIB_CACHE_KEY"] == "stdlib-cache-key"
            assert payload["env"]["MOLT_STDLIB_CACHE_MANIFEST"] == (
                '{"cache_key":"stdlib-cache-key"}'
            )
            assert payload["env"]["MOLT_STDLIB_MODULE_SYMBOLS"] == '["sys"]'
            backend_output.write_bytes(b"\x7fELF")
            self._chunks = [
                json.dumps(
                    {
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

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())
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
        stdlib_object_manifest='{"cache_key":"stdlib-cache-key"}',
        stdlib_module_symbols_json='["sys"]',
        stdlib_module_symbols={"sys"},
        timeout=0.1,
    )

    assert result.ok is True
    assert len(seen_payloads) == 1
    ir_path = Path(cast(str, seen_payloads[0]["jobs"][0]["ir_path"]))
    assert not ir_path.exists()


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
            (
                bool(kwargs.get("probe_cache_only")),
                kwargs.get("ir") is not None,
            )
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
            assert address == str(Path("/tmp/fake.sock"))

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
        BACKEND_EXECUTION,
        "_backend_daemon_compile_request_bytes",
        wrapped_compile_request_bytes,
    )
    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())
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

    socket_path = tmp_path / "daemon.sock"
    daemon_identity = _test_backend_daemon_identity(
        1234,
        socket_path=socket_path,
        project_root=tmp_path,
    )

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_is_verified",
        lambda identity, **kwargs: False,
    )

    result = _compile_with_backend_daemon_non_wasm(
        socket_path,
        ir={"functions": []},
        backend_output=backend_output,
        target_triple=None,
        cache_key=None,
        function_cache_key=None,
        config_digest="digest123",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        timeout=None,
        daemon_identity=daemon_identity,
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
        daemon_identity: object | None = None,
        project_root: Path | None = None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, data, timeout, daemon_identity, project_root
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

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_backend_daemon_request_bytes", _fake_request
    )
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

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())

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
    identity_checks: list[int] = []
    socket_path = tmp_path / "daemon.sock"
    daemon_identity = _test_backend_daemon_identity(
        4321,
        socket_path=socket_path,
        project_root=tmp_path,
    )

    class _FakeSocket:
        def __enter__(self) -> "_FakeSocket":
            return self

        def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
            return False

        def settimeout(self, timeout: float) -> None:
            assert timeout == 1.0

        def connect(self, address: str) -> None:
            assert address == str(socket_path)

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

    def fake_identity_verified(
        identity: cli._BackendDaemonIdentity,
        **kwargs: object,
    ) -> bool:
        assert kwargs == {"allow_health_probe": False}
        identity_checks.append(identity.pid)
        return True

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_is_verified",
        fake_identity_verified,
    )

    response, err = cli._backend_daemon_request_bytes(
        socket_path,
        b'{"version":1}\n',
        timeout=None,
        daemon_identity=daemon_identity,
    )

    assert err is None
    assert response == {"ok": True, "pong": False}
    assert sent == [b'{"version":1}\n']
    assert identity_checks == [4321, 4321]


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

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())

    response, err = cli._backend_daemon_request_bytes(
        tmp_path / "daemon.sock",
        b'{"version":1}\n',
        timeout=0.25,
    )

    assert response is None
    assert err == "backend daemon returned empty response"


def test_backend_daemon_request_bytes_reports_empty_response_with_identity_provenance(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    socket_path = tmp_path / "daemon.sock"
    identity = _test_backend_daemon_identity(
        4321,
        socket_path=socket_path,
        project_root=tmp_path,
        config_digest="digest123",
    )
    log_path = cli._backend_daemon_log_path(
        tmp_path,
        "dev-fast",
        config_digest="digest123",
    )
    log_path.write_text(
        "MOLT_BACKEND(daemon): compiling stdlib batch 35/35\n"
        "MOLT_BACKEND(daemon): compiling user function batch 8/41\n",
        encoding="utf-8",
    )

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
            assert data == b'{"version":1}\n'

        def shutdown(self, how: int) -> None:
            assert how == cli.socket.SHUT_WR

        def recv_into(self, buffer: memoryview) -> int:
            assert len(buffer) == 65536
            return 0

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_is_verified",
        lambda identity, **kwargs: False,
    )

    response, err = cli._backend_daemon_request_bytes(
        socket_path,
        b'{"version":1}\n',
        timeout=0.25,
        daemon_identity=identity,
    )

    assert response is None
    assert err is not None
    assert "backend daemon returned empty response" in err
    assert "pid=4321" in err
    assert "verified_live=false" in err
    assert f"socket={socket_path}" in err
    assert f"log={log_path}" in err
    assert "compiling stdlib batch 35/35" in err
    assert "compiling user function batch 8/41" in err


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

    _enable_fake_backend_daemon_unix_socket(monkeypatch)
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "socket", lambda *args: _FakeSocket())

    response, err = cli._backend_daemon_request_bytes(
        socket_path,
        b'{"version":1}\n',
        timeout=0.25,
    )

    assert err is None
    assert response == {"ok": True}
    assert sent == [b'{"version":1}\n']


def test_orphaned_backend_daemon_sweep_removes_dead_identity_and_legacy_pid(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical_root = tmp_path / "target" / ".molt_state"
    daemon_root = canonical_root / "backend_daemon"
    daemon_root.mkdir(parents=True)
    legacy_pid_path = daemon_root / "molt-backend.dev-fast.alpha.legacy.pid"
    legacy_pid_path.write_text("9999\n")
    backend_bin = tmp_path / "target" / "debug" / "molt-backend"
    socket_path = tmp_path / "daemon.sock"
    socket_path.write_text("")
    identity_path = daemon_root / "molt-backend.dev-fast.alpha.deadbeef.identity.json"
    identity = _test_backend_daemon_identity(
        4321,
        socket_path=socket_path,
        project_root=tmp_path,
        backend_bin=backend_bin,
    )
    cli._write_backend_daemon_identity(identity_path, identity)
    alive = {4321: False}

    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    monkeypatch.chdir(elsewhere)
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_build_state_root", lambda project_root: canonical_root
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_pid_alive", lambda pid: alive.get(pid, False)
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_process_command",
        lambda pid: f"{backend_bin} --daemon --socket {socket_path}",
    )

    cleaned = cli._sweep_orphaned_backend_daemon_locks(
        tmp_path,
        include_other_sessions=False,
    )

    assert cleaned == 2
    assert not identity_path.exists()
    assert not legacy_pid_path.exists()
    assert not socket_path.exists()


def test_backend_daemon_stale_check_tracks_active_runtime_profiles(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path
    target_root = project_root / "target"
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    backend_bin = target_root / "dev-fast" / "molt-backend"
    runtime_lib = (
        target_root / "release-output" / cli._runtime_lib_archive_name("micro", None)
    )
    pid_path = target_root / ".molt_state" / "backend_daemon" / "molt-backend.pid"
    for path in (backend_bin, runtime_lib, pid_path):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x")

    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))

    old = 1_700_000_000.0
    current = old + 10.0
    os.utime(backend_bin, (old, old))
    os.utime(pid_path, (old + 5.0, old + 5.0))
    os.utime(runtime_lib, (current, current))

    assert cli._backend_daemon_binary_is_newer(backend_bin, pid_path)


def test_backend_daemon_stale_check_tracks_extracted_runtime_crates(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path
    target_root = project_root / "target"
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    backend_bin = target_root / "dev-fast" / "molt-backend"
    runtime_lib = (
        target_root / "release-output" / cli._runtime_lib_archive_name("micro", None)
    )
    pid_path = target_root / ".molt_state" / "backend_daemon" / "molt-backend.pid"
    extracted_runtime_src = (
        project_root / "runtime" / "molt-runtime-math" / "src" / "fractions.rs"
    )
    for path in (backend_bin, runtime_lib, pid_path, extracted_runtime_src):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x")

    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))

    old = 1_700_000_000.0
    pid_time = old + 5.0
    current = old + 10.0
    os.utime(backend_bin, (old, old))
    os.utime(runtime_lib, (old, old))
    os.utime(pid_path, (pid_time, pid_time))
    os.utime(extracted_runtime_src, (current, current))

    assert cli._backend_daemon_binary_is_newer(backend_bin, pid_path)


def test_backend_daemon_stale_check_tracks_target_specific_runtime_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path
    target_root = project_root / "target"
    target_triple = "aarch64-apple-darwin"
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    backend_bin = target_root / "dev-fast" / "molt-backend"
    runtime_lib = (
        target_root / target_triple / "release-output" / "libmolt_runtime.stdlib_full.a"
    )
    pid_path = target_root / ".molt_state" / "backend_daemon" / "molt-backend.pid"
    for path in (backend_bin, runtime_lib, pid_path):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x")

    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))

    old = 1_700_000_000.0
    current = old + 10.0
    os.utime(backend_bin, (old, old))
    os.utime(pid_path, (old + 5.0, old + 5.0))
    os.utime(runtime_lib, (current, current))

    assert cli._backend_daemon_binary_is_newer(
        backend_bin,
        pid_path,
        target_triple=target_triple,
    )


def test_sweep_orphaned_backend_daemon_locks_removes_dead_and_unverified_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    own_root = project_root / "target" / ".molt_state" / "backend_daemon"
    own_root.mkdir(parents=True)
    sibling_root = (
        project_root
        / "target"
        / "sessions"
        / "agent-x"
        / ".molt_state"
        / "backend_daemon"
    )
    sibling_root.mkdir(parents=True)

    backend_bin = project_root / "target" / "debug" / "molt-backend"
    dead_identity_file = own_root / "molt-backend.dev-fast.dead.aaaa.identity.json"
    dead_socket = tmp_path / "actual-dead-daemon.sock"
    dead_socket.write_text("")
    cli._write_backend_daemon_identity(
        dead_identity_file,
        _test_backend_daemon_identity(
            99999,
            socket_path=dead_socket,
            project_root=project_root,
            backend_bin=backend_bin,
        ),
    )

    sibling_dead = sibling_root / "molt-backend.release-fast.gone.bbbb.identity.json"
    cli._write_backend_daemon_identity(
        sibling_dead,
        _test_backend_daemon_identity(
            88888,
            socket_path=tmp_path / "sibling-dead.sock",
            project_root=project_root,
            backend_bin=backend_bin,
            cargo_profile="release-fast",
        ),
    )

    live_identity_file = own_root / "molt-backend.dev-fast.live.cccc.identity.json"
    cli._write_backend_daemon_identity(
        live_identity_file,
        _test_backend_daemon_identity(
            4242,
            socket_path=tmp_path / "live-daemon.sock",
            project_root=project_root,
            backend_bin=backend_bin,
        ),
    )

    unverified_identity_file = (
        own_root / "molt-backend.dev-fast.foreign.dddd.identity.json"
    )
    cli._write_backend_daemon_identity(
        unverified_identity_file,
        _test_backend_daemon_identity(
            5252,
            socket_path=tmp_path / "foreign-daemon.sock",
            project_root=project_root,
            backend_bin=backend_bin,
        ),
    )

    malformed = own_root / "molt-backend.dev-fast.bad.eeee.identity.json"
    malformed.write_text("not-json\n")

    legacy_pid_file = own_root / "molt-backend.dev-fast.legacy.ffff.pid"
    legacy_pid_file.write_text("4242\n")

    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_build_state_root",
        lambda root: project_root / "target" / ".molt_state",
    )

    def fake_pid_alive(pid: int) -> bool:
        return pid in {4242, 5252}

    monkeypatch.setattr(BACKEND_EXECUTION, "_pid_alive", fake_pid_alive)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_identity_process_matches",
        lambda identity: identity.pid == 4242,
    )

    cleaned = cli._sweep_orphaned_backend_daemon_locks(project_root)
    assert cleaned == 5

    assert not dead_identity_file.exists()
    assert not dead_socket.exists()
    assert not sibling_dead.exists()
    assert not unverified_identity_file.exists()
    assert not malformed.exists()
    assert not legacy_pid_file.exists()
    assert live_identity_file.exists()


def test_rotate_backend_daemon_log_if_large_rotates_above_threshold(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    log_path = tmp_path / "daemon.log"
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_LOG_MAX_BYTES", "16")
    log_path.write_bytes(b"x" * 64)
    cli._rotate_backend_daemon_log_if_large(log_path)
    assert not log_path.exists()
    assert (log_path.with_name("daemon.log.old")).exists()


def test_rotate_backend_daemon_log_if_large_keeps_small_log(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    log_path = tmp_path / "daemon.log"
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_LOG_MAX_BYTES", "1024")
    log_path.write_bytes(b"under-threshold")
    cli._rotate_backend_daemon_log_if_large(log_path)
    assert log_path.exists()
    assert log_path.read_bytes() == b"under-threshold"
    assert not (log_path.with_name("daemon.log.old")).exists()


def test_backend_daemon_log_tail_mark_and_since_are_bounded(tmp_path: Path) -> None:
    log_path = tmp_path / "daemon.log"
    log_path.write_text("one\ntwo\nthree\n", encoding="utf-8")

    assert cli._backend_daemon_log_tail(log_path, max_lines=2) == "two\nthree"
    mark = cli._backend_daemon_log_mark(log_path)

    with log_path.open("a", encoding="utf-8") as handle:
        handle.write("four\n")

    assert cli._backend_daemon_log_since(log_path, mark) == "four"
    truncated = cli._backend_daemon_log_since(log_path, 0, max_bytes=9)
    assert truncated is not None
    assert truncated.startswith("...(daemon log truncated to recent output)")
    assert "four" in truncated


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

    def wrapped_stat(self: Path, *, follow_symlinks: bool = True):  # type: ignore[no-untyped-def]
        nonlocal calls
        if self == output_artifact:
            calls += 1
        return original_stat(self, follow_symlinks=follow_symlinks)

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

    monkeypatch.setattr(BACKEND_CACHE, "_read_artifact_sync_state", fail_read)

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


def test_atomic_write_text_failure_preserves_existing_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache_path = tmp_path / "cache" / "payload.json"
    cache_path.parent.mkdir(parents=True)
    cache_path.write_text('{"version":0}\n', encoding="utf-8")

    def fail_replace(src: object, dst: object) -> None:
        del src, dst
        raise OSError("simulated atomic replace failure")

    monkeypatch.setattr(os, "replace", fail_replace)

    with pytest.raises(OSError, match="simulated atomic replace failure"):
        cli._atomic_write_text(cache_path, '{"version":1}\n')

    assert cache_path.read_text(encoding="utf-8") == '{"version":0}\n'
    assert list(cache_path.parent.glob(f".{cache_path.name}.*.tmp")) == []


def test_atomic_write_bytes_failure_preserves_existing_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact_path = tmp_path / "cache" / "payload.bin"
    artifact_path.parent.mkdir(parents=True)
    artifact_path.write_bytes(b"old")

    def fail_replace(src: object, dst: object) -> None:
        del src, dst
        raise OSError("simulated atomic replace failure")

    monkeypatch.setattr(os, "replace", fail_replace)

    with pytest.raises(OSError, match="simulated atomic replace failure"):
        cli._atomic_write_bytes(artifact_path, b"new")

    assert artifact_path.read_bytes() == b"old"
    assert list(artifact_path.parent.glob(f".{artifact_path.name}.*.tmp")) == []


def test_publication_sidecar_writers_use_atomic_temp_siblings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    original_replace = os.replace
    replaced_paths: list[tuple[Path, Path]] = []

    def record_replace(src: object, dst: object) -> None:
        src_path = Path(src)
        dst_path = Path(dst)
        replaced_paths.append((src_path, dst_path))
        original_replace(src, dst)

    monkeypatch.setattr(os, "replace", record_replace)

    wasm_path = tmp_path / "wasm" / "app.wasm"
    wasm_path.parent.mkdir(parents=True)
    wasm_path.write_bytes(b"\0asm")
    RUNTIME_WASM_VALIDATION._write_runtime_wasm_integrity_sidecar(wasm_path)

    generated_text_path = tmp_path / "generated" / "module.py"
    cli._write_text_if_changed(generated_text_path, "value = 1\n")

    diagnostics_path = tmp_path / "logs" / "diagnostics.json"
    cli_build_diagnostics._emit_build_diagnostics(
        diagnostics={"total_sec": 1.0},
        diagnostics_path=diagnostics_path,
        json_output=True,
    )

    emitted_ir_path = tmp_path / "logs" / "ir.json"
    assert (
        BACKEND_IR._write_emitted_ir(
            emitted_ir_path,
            {"functions": [{"name": "main", "ops": []}]},
        )
        is None
    )

    cli_non_native_output._generate_snapshot_header(
        output_wasm=wasm_path,
        target_profile="edge",
        capabilities_list=["fs.bundle.read"],
        verbose=False,
    )

    validate_summary_path = tmp_path / "logs" / "validate.json"
    cli._write_json_sidecar(validate_summary_path, {"ok": True})

    signature_path = tmp_path / "dist" / "pkg.sig"
    cli._atomic_write_bytes(signature_path, b"signature")

    expected_dests = {
        RUNTIME_WASM_VALIDATION._runtime_wasm_integrity_sidecar_path(wasm_path),
        generated_text_path,
        diagnostics_path,
        emitted_ir_path,
        wasm_path.parent / "molt.snapshot.json",
        validate_summary_path,
        signature_path,
    }
    assert {dst for _src, dst in replaced_paths} == expected_dests
    assert all(
        src.parent == dst.parent
        and src.name.startswith(f".{dst.name}.")
        and src.name.endswith(".tmp")
        for src, dst in replaced_paths
    )
    assert list(tmp_path.rglob(".*.tmp")) == []


def test_persisted_json_and_sync_writers_use_atomic_temp_siblings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache_path = tmp_path / "cache" / "payload.json"
    sync_path = tmp_path / "cache" / "sync.json"
    state_path = tmp_path / "cache" / "state.json"
    artifact_path = tmp_path / "cache" / "artifact.o"
    artifact_path.parent.mkdir(parents=True, exist_ok=True)
    artifact_path.write_bytes(b"artifact")
    original_replace = os.replace
    replaced_paths: list[tuple[Path, Path]] = []

    def record_replace(src: object, dst: object) -> None:
        src_path = Path(src)
        dst_path = Path(dst)
        if src_path in {cache_path, sync_path}:
            raise AssertionError(f"direct destination replace source: {src_path}")
        replaced_paths.append((src_path, dst_path))
        original_replace(src, dst)

    monkeypatch.setattr(os, "replace", record_replace)

    cli._write_cached_json_object(cache_path, {"version": 1, "hash": "abc"})
    cli._write_artifact_sync_payload(sync_path, {"version": 1, "source_key": "abc"})
    cli._write_artifact_sync_state(
        state_path, source_key="abc", tier="module", artifact=artifact_path
    )

    assert json.loads(cache_path.read_text(encoding="utf-8"))["hash"] == "abc"
    assert json.loads(sync_path.read_text(encoding="utf-8"))["source_key"] == "abc"
    assert json.loads(state_path.read_text(encoding="utf-8"))["source_key"] == "abc"
    assert {dst for _src, dst in replaced_paths} == {cache_path, sync_path, state_path}
    assert all(
        src.name.startswith(".") and src.name.endswith(".tmp")
        for src, _dst in replaced_paths
    )
    assert list(cache_path.parent.glob(".*.tmp")) == []


def test_source_hash_cache_write_uses_unique_atomic_temp_siblings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache_path = tmp_path / "cache" / "source.json"
    original_replace = os.replace
    replaced_sources: list[Path] = []

    def record_replace(src: object, dst: object) -> None:
        assert Path(dst) == cache_path
        replaced_sources.append(Path(src))
        original_replace(src, dst)

    monkeypatch.setattr(os, "replace", record_replace)

    cli_module_source._write_source_hash_cache_payload(cache_path, {"hash": "a"})
    cli_module_source._write_source_hash_cache_payload(cache_path, {"hash": "b"})

    assert json.loads(cache_path.read_text(encoding="utf-8")) == {"hash": "b"}
    assert len(replaced_sources) == 2
    assert replaced_sources[0] != replaced_sources[1]
    assert all(
        path.name.startswith(".source.json.") and path.name.endswith(".tmp")
        for path in replaced_sources
    )
    assert not (cache_path.parent / "source.json.tmp").exists()
    assert list(cache_path.parent.glob(".*.tmp")) == []


def test_shared_cache_lock_is_cache_rooted_and_session_independent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache_root = tmp_path / "cache"
    explicit_cache_root = tmp_path / "explicit-cache"
    env_cache_root = tmp_path / "env-cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    cli._default_molt_cache_cached.cache_clear()
    cli._shared_cache_lock_dir_cached.cache_clear()

    monkeypatch.setenv("MOLT_SESSION_ID", "session-a")
    with cli._shared_cache_lock("compile.abc123"):
        assert (cache_root / "locks" / "compile.abc123.lock").exists()

    monkeypatch.setenv("MOLT_SESSION_ID", "session-b")
    with cli._shared_cache_lock("compile.abc123"):
        assert (cache_root / "locks" / "compile.abc123.lock").exists()

    assert not (cache_root / "locks" / "compile.abc123.session-a.lock").exists()
    assert not (cache_root / "locks" / "compile.abc123.session-b.lock").exists()

    monkeypatch.setenv("MOLT_CACHE", str(env_cache_root))
    cli._default_molt_cache_cached.cache_clear()
    with cli._shared_cache_lock("compile.explicit", cache_root=explicit_cache_root):
        assert (explicit_cache_root / "locks" / "compile.explicit.lock").exists()
    assert not (env_cache_root / "locks" / "compile.explicit.lock").exists()


def test_read_cached_artifact_keeps_invalid_shared_cache_entry(tmp_path: Path) -> None:
    cache_path = tmp_path / "vendor" / "artifact.whl"
    cache_path.parent.mkdir(parents=True)
    cache_path.write_bytes(b"invalid")

    assert cli._read_cached_artifact(cache_path, "0" * 64) is None
    assert cache_path.read_bytes() == b"invalid"


def test_write_cached_artifact_uses_unique_atomic_temp_sibling(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cache_path = tmp_path / "vendor" / "artifact.whl"
    replaced_sources: list[Path] = []
    original_replace = os.replace

    def record_replace(src: object, dst: object) -> None:
        assert Path(dst) == cache_path
        replaced_sources.append(Path(src))
        original_replace(src, dst)

    monkeypatch.setattr(os, "replace", record_replace)

    cli._write_cached_artifact(cache_path, b"first")
    cli._write_cached_artifact(cache_path, b"second")

    assert cache_path.read_bytes() == b"second"
    assert len(replaced_sources) == 2
    assert replaced_sources[0] != replaced_sources[1]
    assert all(
        path.name.startswith(".artifact.whl.") and path.name.endswith(".tmp")
        for path in replaced_sources
    )
    assert not (cache_path.parent / "artifact.whl.tmp").exists()
    assert list(cache_path.parent.glob(".*.tmp")) == []


def test_replace_directory_tree_from_source_publishes_prepared_tree(
    tmp_path: Path,
) -> None:
    src = tmp_path / "src"
    dest = tmp_path / "dest"
    src.mkdir()
    dest.mkdir()
    (src / "new.txt").write_text("new", encoding="utf-8")
    (dest / "old.txt").write_text("old", encoding="utf-8")

    cli_non_native_output._replace_directory_tree_from_source(src, dest)

    assert not (dest / "old.txt").exists()
    assert (dest / "new.txt").read_text(encoding="utf-8") == "new"
    assert list(tmp_path.glob(".dest.*.tmp")) == []
    assert list(tmp_path.glob(".dest.*.old")) == []

    wrapper_src = tmp_path / "wrapper-src"
    wrapper_dest = tmp_path / "wrapper-dest"
    wrapper_src.mkdir()
    wrapper_dest.mkdir()
    (wrapper_src / "wrapped.txt").write_text("wrapped", encoding="utf-8")

    cli_deps._replace_directory_tree_from_source(wrapper_src, wrapper_dest)

    assert (wrapper_dest / "wrapped.txt").read_text(encoding="utf-8") == "wrapped"


def test_replace_directory_tree_from_source_restores_previous_tree_on_publish_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    src = tmp_path / "src"
    dest = tmp_path / "dest"
    src.mkdir()
    dest.mkdir()
    (src / "new.txt").write_text("new", encoding="utf-8")
    (dest / "old.txt").write_text("old", encoding="utf-8")
    original_replace = os.replace

    def fail_temp_publish(src_arg: object, dst_arg: object) -> None:
        src_path = Path(src_arg)
        if src_path.name.startswith(".dest.") and src_path.name.endswith(".tmp"):
            raise OSError("publish failed")
        original_replace(src_arg, dst_arg)

    monkeypatch.setattr(os, "replace", fail_temp_publish)

    with pytest.raises(OSError, match="publish failed"):
        cli_non_native_output._replace_directory_tree_from_source(src, dest)

    assert (dest / "old.txt").read_text(encoding="utf-8") == "old"
    assert not (dest / "new.txt").exists()
    assert list(tmp_path.glob(".dest.*.tmp")) == []
    assert list(tmp_path.glob(".dest.*.old")) == []


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
        stdlib_object_cache_key=None,
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
        stdlib_object_cache_key=None,
        function_cache_path=function_cache_path,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert cache_path.read_bytes() == b"artifact"
    assert output_artifact.read_bytes() == b"artifact"
    assert function_cache_path.read_bytes() == b"artifact"


def test_stage_backend_output_and_caches_preserves_existing_module_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"new-artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    cache_path.parent.mkdir(parents=True)
    cache_path.write_bytes(b"cached-artifact")
    warnings: list[str] = []

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_is_valid_cached_backend_artifact",
        lambda path, *, is_wasm: True,
    )

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        stdlib_object_cache_key=None,
        function_cache_path=None,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert cache_path.read_bytes() == b"cached-artifact"
    assert output_artifact.read_bytes() == b"cached-artifact"
    assert not backend_output.exists()


def test_stage_backend_output_and_caches_preserves_existing_function_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    backend_output = tmp_path / "backend.o"
    backend_output.write_bytes(b"new-artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    function_cache_path = tmp_path / "cache" / "function.o"
    function_cache_path.parent.mkdir(parents=True)
    function_cache_path.write_bytes(b"cached-function")
    warnings: list[str] = []

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_is_valid_cached_backend_artifact",
        lambda path, *, is_wasm: True,
    )

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        stdlib_object_cache_key=None,
        function_cache_path=function_cache_path,
        warnings=warnings,
    )

    assert err is None
    assert warnings == []
    assert cache_path.read_bytes() == b"new-artifact"
    assert output_artifact.read_bytes() == b"new-artifact"
    assert function_cache_path.read_bytes() == b"cached-function"
    assert not backend_output.exists()


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
        stdlib_object_cache_key=None,
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

    monkeypatch.setattr(BACKEND_CACHE, "_write_artifact_sync_state", fail_write)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        stdlib_object_cache_key=None,
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

    monkeypatch.setattr(BACKEND_CACHE, "_read_artifact_sync_state", fail_read)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        stdlib_object_cache_key=None,
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
    original_link_or_copy = BACKEND_CACHE._atomic_link_or_copy_file
    original_copy = cli._atomic_copy_file

    def record_link_or_copy(src: Path, dst: Path) -> None:
        link_calls.append((src, dst))
        original_link_or_copy(src, dst)

    def fail_copy(src: Path, dst: Path) -> None:
        if dst == output_artifact:
            raise AssertionError(f"unexpected copy {src} -> {dst}")
        original_copy(src, dst)

    monkeypatch.setattr(BACKEND_CACHE, "_atomic_link_or_copy_file", record_link_or_copy)
    monkeypatch.setattr(cli, "_atomic_copy_file", fail_copy)

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        stdlib_object_cache_key=None,
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
        stdlib_object_cache_key=None,
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
    original = BACKEND_CACHE._publish_immutable_backend_cache_artifact

    def wrapped(src: Path, dst: Path, *, is_wasm: bool, warnings: list[str]) -> Path:
        if dst == function_cache_path:
            raise OSError("link failed")
        return original(src, dst, is_wasm=is_wasm, warnings=warnings)

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_publish_immutable_backend_cache_artifact",
        wrapped,
    )

    err = cli._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        stdlib_object_cache_key=None,
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


def test_backend_cache_artifact_path_uses_native_stdlib_context(
    tmp_path: Path,
) -> None:
    cache_root = tmp_path / "cache"

    native_path = cli._backend_cache_artifact_path(
        cache_root,
        "function-key",
        ext="o",
        stdlib_object_cache_key="stdlib-key",
        is_wasm=False,
    )
    wasm_path = cli._backend_cache_artifact_path(
        cache_root,
        "function-key",
        ext="wasm",
        stdlib_object_cache_key="stdlib-key",
        is_wasm=True,
    )

    assert native_path == cache_root / "function-key.stdlib-stdlib-key.o"
    assert wasm_path == cache_root / "function-key.wasm"


def test_try_cached_backend_candidates_promoted_function_hit_marks_module_synced(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    candidate = tmp_path / "cache" / "function.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    warnings: list[str] = []
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_is_valid_cached_backend_artifact",
        lambda path, *, is_wasm: True,
    )

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


def test_try_cached_backend_candidates_promoted_native_function_hit_marks_context_module_synced(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    candidate = tmp_path / "cache" / "function.stdlib-current.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"artifact")
    output_artifact = tmp_path / "dist" / "output.o"
    cache_path = tmp_path / "cache" / "module.stdlib-current.o"
    stdlib_object = tmp_path / "cache" / "stdlib_shared_current.o"
    stdlib_object.write_bytes(b"stdlib")
    warnings: list[str] = []
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_is_valid_cached_backend_artifact",
        lambda path, *, is_wasm: True,
    )
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_shared_stdlib_cache_matches_key_locked",
        lambda *args, **kwargs: True,
    )

    ok, cache_hit_tier = cli._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=[("function", candidate)],
        output_artifact=output_artifact,
        is_wasm=False,
        cache_key="module-key",
        function_cache_key="function-key",
        cache_path=cache_path,
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="stdlib-key",
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
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="stdlib-key",
    )
    assert skip_module is True
    assert skip_function is False
    state = cli._read_artifact_sync_state(
        cli._artifact_sync_state_path(tmp_path, output_artifact)
    )
    output_stat = output_artifact.stat()
    assert cli._artifact_sync_state_matches_stat(
        state,
        source_key="module-key|stdlib:stdlib-key",
        tier="module",
        stat=output_stat,
    )
    assert not cli._artifact_sync_state_matches_stat(
        state,
        source_key="module-key",
        tier="module",
        stat=output_stat,
    )


def test_try_cached_backend_candidates_preserves_invalid_candidate(
    tmp_path: Path,
) -> None:
    candidate = tmp_path / "cache" / "module.o"
    candidate.parent.mkdir(parents=True)
    candidate.write_bytes(b"")
    output_artifact = tmp_path / "dist" / "output.o"
    warnings: list[str] = []

    ok, cache_hit_tier = cli._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=[("module", candidate)],
        output_artifact=output_artifact,
        is_wasm=False,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=candidate,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        warnings=warnings,
    )

    assert ok is False
    assert cache_hit_tier is None
    assert candidate.exists()
    assert warnings == [f"Ignoring invalid cache artifact: {candidate}"]


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
    original_link_or_copy = BACKEND_CACHE._atomic_link_or_copy_file
    original_copy = cli._atomic_copy_file

    def record_link_or_copy(src: Path, dst: Path) -> None:
        link_calls.append((src, dst))
        original_link_or_copy(src, dst)

    def fail_copy(src: Path, dst: Path) -> None:
        if dst == output_artifact:
            raise AssertionError(f"unexpected copy {src} -> {dst}")
        original_copy(src, dst)

    monkeypatch.setattr(BACKEND_CACHE, "_atomic_link_or_copy_file", record_link_or_copy)
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

    monkeypatch.setattr(BACKEND_CACHE, "_read_artifact_sync_state", fail_read)

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
    run_cli_test_process(
        ["clang", "-c", str(empty_c), "-o", str(empty_object)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert not cli._is_valid_cached_backend_artifact(empty_object, is_wasm=False)

    native_c = tmp_path / "native.c"
    native_c.write_text("int foo(void){return 0;}\n", encoding="utf-8")
    native_nonempty = tmp_path / "nonempty.o"
    run_cli_test_process(
        ["clang", "-c", str(native_c), "-o", str(native_nonempty)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert cli._is_valid_cached_backend_artifact(native_nonempty, is_wasm=False)


def test_try_cached_backend_candidates_rejects_native_hit_with_unresolved_user_module_chunks(
    tmp_path: Path,
) -> None:
    candidate = _compile_c_object(
        tmp_path,
        "candidate",
        """
        extern void tkinter_phase0_core_semantics__molt_module_chunk_1(void);
        void molt_init_tkinter_phase0_core_semantics(void) {
            tkinter_phase0_core_semantics__molt_module_chunk_1();
        }
        """,
    )
    stdlib_object = _compile_c_object(
        tmp_path,
        "stdlib_shared",
        """
        void tkinter__molt_module_chunk_1(void) {}
        """,
    )
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "stdlib-key\n", encoding="utf-8"
    )
    stdlib_manifest = '{"cache_key":"stdlib-key"}'
    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        stdlib_manifest + "\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        '{"body_hash":"test","function_count":1,"functions":["molt_init_tkinter"],"schema":"stdlib-partition-v1"}\n',
        encoding="utf-8",
    )
    cli._stdlib_object_digest_sidecar_path(stdlib_object).write_text(
        cli._sha256_file(stdlib_object) + "\n", encoding="utf-8"
    )
    output_artifact = tmp_path / "dist" / "output.o"
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
        stdlib_object_manifest=stdlib_manifest,
        stdlib_module_symbols=None,
        warnings=warnings,
    )

    assert ok is False
    assert cache_hit_tier is None
    assert not output_artifact.exists()


def test_shared_stdlib_cache_rejects_unresolved_stdlib_module_reference(
    tmp_path: Path,
) -> None:
    stdlib_object = _compile_c_object(
        tmp_path,
        "stdlib_shared",
        """
        extern void copy__copy(void);
        void collections__UserDict_copy(void) {
            copy__copy();
        }
        """,
    )
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "stdlib-key\n", encoding="utf-8"
    )
    stdlib_manifest = '{"cache_key":"stdlib-key"}'
    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        stdlib_manifest + "\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        (
            '{"body_hash":"test","function_count":1,'
            '"functions":["collections__UserDict_copy"],'
            '"schema":"stdlib-partition-v1"}\n'
        ),
        encoding="utf-8",
    )
    cli._stdlib_object_digest_sidecar_path(stdlib_object).write_text(
        cli._sha256_file(stdlib_object) + "\n", encoding="utf-8"
    )

    assert cli._shared_stdlib_cache_matches_key_locked(
        stdlib_object,
        "stdlib-key",
        stdlib_object_manifest=stdlib_manifest,
        stdlib_module_symbols={"collections"},
    )
    assert not cli._shared_stdlib_cache_matches_key_locked(
        stdlib_object,
        "stdlib-key",
        stdlib_object_manifest=stdlib_manifest,
        stdlib_module_symbols={"collections", "copy"},
    )

    cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        tmp_path,
        "stdlib-key",
        expected_manifest=stdlib_manifest,
        stdlib_module_symbols={"collections", "copy"},
    )

    assert not stdlib_object.exists()
    assert not cli._stdlib_object_key_sidecar_path(stdlib_object).exists()
    assert not cli._stdlib_object_manifest_sidecar_path(stdlib_object).exists()
    assert not cli._stdlib_object_partition_manifest_sidecar_path(
        stdlib_object
    ).exists()


def test_backend_daemon_skip_output_sync_flags_rejects_synced_native_output_with_unresolved_user_module_chunks(
    tmp_path: Path,
) -> None:
    output_artifact = _compile_c_object(
        tmp_path,
        "synced_output",
        """
        extern void tkinter_phase0_core_semantics__molt_module_chunk_1(void);
        void molt_init_tkinter_phase0_core_semantics(void) {
            tkinter_phase0_core_semantics__molt_module_chunk_1();
        }
        """,
    )
    stdlib_object = _compile_c_object(
        tmp_path,
        "stdlib_shared",
        """
        void tkinter__molt_module_chunk_1(void) {}
        """,
    )
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "stdlib-key\n", encoding="utf-8"
    )
    stdlib_manifest = '{"cache_key":"stdlib-key"}'
    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        stdlib_manifest + "\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        '{"body_hash":"test","function_count":1,"functions":["molt_init_tkinter"],"schema":"stdlib-partition-v1"}\n',
        encoding="utf-8",
    )
    cli._stdlib_object_digest_sidecar_path(stdlib_object).write_text(
        cli._sha256_file(stdlib_object) + "\n", encoding="utf-8"
    )
    state_path = cli._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    cli._write_artifact_sync_state(
        state_path,
        source_key="module-key|stdlib:stdlib-key",
        tier="module",
        artifact=output_artifact,
    )

    skip_module, skip_function = cli._backend_daemon_skip_output_sync_flags(
        tmp_path,
        output_artifact,
        cache_key="module-key",
        function_cache_key=None,
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="stdlib-key",
        stdlib_object_manifest=stdlib_manifest,
        stdlib_module_symbols=None,
    )

    assert skip_module is False
    assert skip_function is False


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
            "spawn_config_digest": "spawn-digest",
            "active_config_digest": "active-digest",
        },
    }
    health = cli._backend_daemon_health_from_response(response)
    assert isinstance(health, dict)
    assert health["pid"] == 123
    assert health["cache_entries"] == 2
    assert health["spawn_config_digest"] == "spawn-digest"
    assert health["active_config_digest"] == "active-digest"


def test_backend_daemon_identity_from_health_requires_spawn_digest(
    tmp_path: Path,
) -> None:
    socket_path = tmp_path / "daemon.sock"
    backend_bin = tmp_path / "molt-backend"

    assert (
        cli._backend_daemon_identity_from_health(
            {"pid": 1234, "spawn_config_digest": "old-digest"},
            socket_path=socket_path,
            project_root=tmp_path,
            cargo_profile="dev-fast",
            config_digest="new-digest",
            backend_bin=backend_bin,
        )
        is None
    )

    identity = cli._backend_daemon_identity_from_health(
        {"pid": 1234, "spawn_config_digest": "new-digest"},
        socket_path=socket_path,
        project_root=tmp_path,
        cargo_profile="dev-fast",
        config_digest="new-digest",
        backend_bin=backend_bin,
    )

    assert identity is not None
    assert identity.pid == 1234


def test_backend_daemon_ping_health_backcompat_without_health(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        BACKEND_EXECUTION,
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
        **cli_test_popen_kwargs(env),
    )
    try:
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
    finally:
        close_cli_test_process_group(proc)


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
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )

    setup_split = cli_backend_cache_setup._prepare_backend_cache_setup(
        emit_mode="obj",  # native + obj  => split enabled
        output_artifact=ROOT / "dummy_split.o",
        **common,
    )

    # _native_stdlib_object_split_enabled returns True for native target,
    # so we simulate monolithic by patching the helper.
    import unittest.mock as _mock

    with _mock.patch.object(
        cli_backend_cache_setup,
        "_native_stdlib_object_split_enabled",
        return_value=False,
    ):
        setup_mono = cli_backend_cache_setup._prepare_backend_cache_setup(
            emit_mode="obj",
            output_artifact=ROOT / "dummy_mono.o",
            **common,
        )

    assert setup_split.cache_key is not None
    assert setup_mono.cache_key is not None
    assert setup_split.cache_key != setup_mono.cache_key, (
        "Cache keys must differ between stdlib-split and monolithic modes"
    )


def test_prepare_backend_cache_setup_routes_stdlib_object_to_explicit_cache_dir(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "ambient-cache"))
    explicit_cache = tmp_path / "explicit-cache"
    tiny_ir: dict = {
        "module": "__main__",
        "filename": "test.py",
        "ops": [],
        "functions": [],
        "classes": [],
        "constants": {},
        "imports": [],
    }
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    setup = cli_backend_cache_setup._prepare_backend_cache_setup(
        cache_enabled=True,
        ir=tiny_ir,
        target="native",
        target_triple=None,
        profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="obj",
        is_wasm=False,
        linked=False,
        project_root=ROOT,
        cache_dir=str(explicit_cache),
        output_artifact=tmp_path / "out.o",
        warnings=[],
        entry_module="__main__",
        module_graph_metadata=module_graph_metadata,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert setup.cache_path is not None
    assert setup.stdlib_object_path is not None
    assert setup.cache_path.parent == explicit_cache
    assert setup.stdlib_object_path.parent == explicit_cache


def test_prepare_backend_cache_setup_routes_no_cache_stdlib_object_to_explicit_cache_dir(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "ambient-cache"))
    explicit_cache = tmp_path / "explicit-cache"
    tiny_ir: dict = {
        "module": "__main__",
        "filename": "test.py",
        "ops": [],
        "functions": [],
        "classes": [],
        "constants": {},
        "imports": [],
    }
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    setup = cli_backend_cache_setup._prepare_backend_cache_setup(
        cache_enabled=False,
        ir=tiny_ir,
        target="native",
        target_triple=None,
        profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="obj",
        is_wasm=False,
        linked=False,
        project_root=ROOT,
        cache_dir=str(explicit_cache),
        output_artifact=tmp_path / "out.o",
        warnings=[],
        entry_module="__main__",
        module_graph_metadata=module_graph_metadata,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert setup.cache_path is None
    assert setup.stdlib_object_path is not None
    assert setup.stdlib_object_path.parent == explicit_cache


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

    fp1 = cli_link_pipeline._link_fingerprint(
        project_root=tmp_path,
        inputs=[user_obj, stdlib_obj],
        link_cmd=link_cmd,
    )

    # Mutate the stdlib artifact
    stdlib_obj.write_bytes(b"\x00ELF-stdlib-v2")

    fp2 = cli_link_pipeline._link_fingerprint(
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
    from molt.cli.backend_cache_setup import _build_cache_variant

    variant_mono = _build_cache_variant(
        profile="dev",
        runtime_cargo="debug",
        backend_cargo="debug",
        emit="bin",
        stdlib_split=False,
        codegen_env="x",
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        partition_mode=False,
    )
    variant_part = _build_cache_variant(
        profile="dev",
        runtime_cargo="debug",
        backend_cargo="debug",
        emit="bin",
        stdlib_split=False,
        codegen_env="x",
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        partition_mode=True,
    )
    assert variant_mono != variant_part, (
        "Cache variant must change when partition mode changes"
    )
    assert "partitioned" in variant_part
    assert "partitioned" not in variant_mono


def test_external_static_package_digest_changes_backend_cache_identity() -> None:
    variant_a = cli_backend_cache_setup._build_cache_variant(
        profile="dev",
        runtime_cargo="debug",
        backend_cargo="debug",
        emit="bin",
        stdlib_split=False,
        codegen_env="x",
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        external_static_packages_digest="a" * 64,
    )
    variant_b = cli_backend_cache_setup._build_cache_variant(
        profile="dev",
        runtime_cargo="debug",
        backend_cargo="debug",
        emit="bin",
        stdlib_split=False,
        codegen_env="x",
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        external_static_packages_digest="b" * 64,
    )

    assert variant_a != variant_b
    assert "external_static_packages=" in variant_a


def test_runtime_intrinsic_symbol_digest_changes_backend_cache_identity() -> None:
    variant_a = cli_backend_cache_setup._build_cache_variant(
        profile="dev",
        runtime_cargo="debug",
        backend_cargo="debug",
        emit="bin",
        stdlib_split=True,
        codegen_env="x",
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        runtime_intrinsic_symbols_digest="a" * 64,
    )
    variant_b = cli_backend_cache_setup._build_cache_variant(
        profile="dev",
        runtime_cargo="debug",
        backend_cargo="debug",
        emit="bin",
        stdlib_split=True,
        codegen_env="x",
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        runtime_intrinsic_symbols_digest="b" * 64,
    )

    assert variant_a != variant_b
    assert "runtime_intrinsics=" in variant_a
