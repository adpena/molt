from __future__ import annotations

import copy
import importlib.util
import json
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
GEN_WASM_ABI = ROOT / "tools" / "gen_wasm_abi.py"
WASM_ABI_GEN_ROOT = ROOT / "tools"

if str(WASM_ABI_GEN_ROOT) not in sys.path:
    sys.path.insert(0, str(WASM_ABI_GEN_ROOT))

from wasm_abi_gen import manifest  # noqa: E402
from wasm_abi_gen.paths import OUT_RUNTIME_CALLABLES_RS  # noqa: E402

_GEN_CACHE: object | None = None
_MANIFEST_CACHE: dict | None = None
_MANIFEST_CACHE_BASELINE: dict | None = None
_RENDERED_RS_MODULES_CACHE: dict[str, str] | None = None
_RENDERED_RUNTIME_CALLABLES_CACHE: str | None = None
_RENDERED_PY_CACHE: str | None = None
_RENDERED_TABLE_LAYOUT_INC_CACHE: str | None = None
_RENDERED_ALLOWED_IMPORTS_CACHE: str | None = None


def _load_gen_wasm_abi():
    global _GEN_CACHE
    if _GEN_CACHE is not None:
        return _GEN_CACHE
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_wasm_abi", GEN_WASM_ABI
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _install_gen_cache(module)
    _GEN_CACHE = module
    return module


def test_wasm_abi_generator_cache_identity_uses_runtime_abi_surface() -> None:
    input_files = {path.resolve() for path in manifest.generator_input_files()}
    assert OUT_RUNTIME_CALLABLES_RS.resolve() not in input_files
    assert not any(
        path.match("runtime/molt-runtime/src/call/function.rs") for path in input_files
    )

    runtime_exports = manifest.generator_runtime_export_signature_rows()
    assert runtime_exports == tuple(sorted(runtime_exports))
    assert any(
        name == "molt_importlib_import_transaction"
        and params == ("i64", "i64", "i64", "i64", "i64")
        and result == "i64"
        for name, params, result in runtime_exports
    )
    cpython_abi_imports = manifest.generator_cpython_abi_link_import_names()
    assert cpython_abi_imports == tuple(sorted(cpython_abi_imports))
    cpython_abi_import_kinds = manifest.generator_cpython_abi_link_import_kinds()
    assert cpython_abi_import_kinds == tuple(sorted(cpython_abi_import_kinds))
    assert all(kind in {"data", "function"} for _, kind in cpython_abi_import_kinds)
    assert {
        "PyArg_ParseTuple",
        "PyFloat_Check",
        "PyLong_Check",
        "PyBool_Check",
        "PyExc_TypeError",
        "PyObject_Init",
        "PyMemoryView_FromMemory",
        "Py_None",
        "molt_cpython_abi_date_from_date",
        "PyLong_FromString",
        "PyLong_AsSize_t",
        "PyUnstable_Long_IsCompact",
        "PyUnstable_Long_CompactValue",
        "_PyLong_NumBits",
        "_PyLong_Size_t_Converter",
        "_PyLong_UnsignedShort_Converter",
        "_PyLong_UnsignedInt_Converter",
        "_PyLong_UnsignedLong_Converter",
        "_PyLong_UnsignedLongLong_Converter",
        "PyLong_GetInfo",
    } <= set(cpython_abi_imports)


def _install_gen_cache(gen) -> None:
    uncached_load_manifest = gen.load_manifest
    uncached_render_rs_modules = gen.render_rs_modules
    uncached_render_runtime_callables_rs = gen.render_runtime_callables_rs
    uncached_render_py = gen.render_py
    uncached_render_table_layout_inc = gen.render_table_layout_inc
    uncached_render_allowed_imports = gen.render_allowed_imports

    def load_manifest_cached():
        global _MANIFEST_CACHE, _MANIFEST_CACHE_BASELINE
        if _MANIFEST_CACHE is None:
            _MANIFEST_CACHE = uncached_load_manifest()
            _MANIFEST_CACHE_BASELINE = copy.deepcopy(_MANIFEST_CACHE)
        return _MANIFEST_CACHE

    def render_rs_modules_cached(data: dict) -> dict[str, str]:
        global _RENDERED_RS_MODULES_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_RS_MODULES_CACHE is None:
                _RENDERED_RS_MODULES_CACHE = uncached_render_rs_modules(data)
            return _RENDERED_RS_MODULES_CACHE
        return uncached_render_rs_modules(data)

    def render_runtime_callables_rs_cached(data: dict) -> str:
        global _RENDERED_RUNTIME_CALLABLES_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_RUNTIME_CALLABLES_CACHE is None:
                _RENDERED_RUNTIME_CALLABLES_CACHE = (
                    uncached_render_runtime_callables_rs(data)
                )
            return _RENDERED_RUNTIME_CALLABLES_CACHE
        return uncached_render_runtime_callables_rs(data)

    def render_py_cached(data: dict) -> str:
        global _RENDERED_PY_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_PY_CACHE is None:
                _RENDERED_PY_CACHE = uncached_render_py(data)
            return _RENDERED_PY_CACHE
        return uncached_render_py(data)

    def render_table_layout_inc_cached(data: dict) -> str:
        global _RENDERED_TABLE_LAYOUT_INC_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_TABLE_LAYOUT_INC_CACHE is None:
                _RENDERED_TABLE_LAYOUT_INC_CACHE = uncached_render_table_layout_inc(
                    data
                )
            return _RENDERED_TABLE_LAYOUT_INC_CACHE
        return uncached_render_table_layout_inc(data)

    def render_allowed_imports_cached(data: dict) -> str:
        global _RENDERED_ALLOWED_IMPORTS_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_ALLOWED_IMPORTS_CACHE is None:
                _RENDERED_ALLOWED_IMPORTS_CACHE = uncached_render_allowed_imports(data)
            return _RENDERED_ALLOWED_IMPORTS_CACHE
        return uncached_render_allowed_imports(data)

    gen.load_manifest = load_manifest_cached
    gen.render_rs_modules = render_rs_modules_cached
    gen.render_runtime_callables_rs = render_runtime_callables_rs_cached
    gen.render_py = render_py_cached
    gen.render_table_layout_inc = render_table_layout_inc_cached
    gen.render_allowed_imports = render_allowed_imports_cached


def _reset_render_caches() -> None:
    global _RENDERED_RS_MODULES_CACHE
    global _RENDERED_RUNTIME_CALLABLES_CACHE
    global _RENDERED_PY_CACHE
    global _RENDERED_TABLE_LAYOUT_INC_CACHE
    global _RENDERED_ALLOWED_IMPORTS_CACHE

    _RENDERED_RS_MODULES_CACHE = None
    _RENDERED_RUNTIME_CALLABLES_CACHE = None
    _RENDERED_PY_CACHE = None
    _RENDERED_TABLE_LAYOUT_INC_CACHE = None
    _RENDERED_ALLOWED_IMPORTS_CACHE = None


@pytest.fixture(autouse=True)
def _manifest_cache_is_read_only():
    yield
    global _MANIFEST_CACHE
    if (
        _MANIFEST_CACHE is not None
        and _MANIFEST_CACHE_BASELINE is not None
        and _MANIFEST_CACHE != _MANIFEST_CACHE_BASELINE
    ):
        _MANIFEST_CACHE = copy.deepcopy(_MANIFEST_CACHE_BASELINE)
        _reset_render_caches()
        raise AssertionError(
            "tests must copy the cached WASM ABI manifest before mutating it"
        )


def _raw_manifest() -> dict:
    return tomllib.loads(manifest.MANIFEST.read_text(encoding="utf-8"))


def _rendered_rs(gen, data) -> str:
    return "".join(gen.render_rs_modules(data).values())


def _exec_rendered_py(rendered_py: str) -> dict[str, object]:
    namespace: dict[str, object] = {}
    exec(rendered_py, namespace)
    return namespace


def test_rustfmt_many_materializes_cached_sibling_modules(monkeypatch) -> None:
    gen = _load_gen_wasm_abi()
    modules = {
        "mod.rs": "mod child;\n",
        "child.rs": "pub(crate) fn child() {}\n",
    }
    seen: dict[str, bool] = {}

    monkeypatch.setattr(gen, "RUSTFMT_CACHE_ENABLED", True)
    monkeypatch.setattr(gen, "_rustfmt_version", lambda: "rustfmt-test")
    monkeypatch.setattr(
        gen,
        "_load_rustfmt_cache",
        lambda name, source, version: source if name == "child.rs" else None,
    )
    monkeypatch.setattr(gen, "_store_rustfmt_cache", lambda *args: None)

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        mod_path = Path(cmd[-1])
        child_path = mod_path.with_name("child.rs")
        seen["child_exists"] = child_path.exists()
        mod_path.write_text("mod child;\n", encoding="utf-8")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(gen.subprocess, "run", fake_run)

    rendered = gen._rustfmt_many(modules)

    assert seen["child_exists"] is True
    assert rendered["child.rs"] == modules["child.rs"]


def test_wasm_abi_generated_files_are_in_sync() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    rendered_rs_modules = gen.render_rs_modules(data)
    assert not gen.LEGACY_OUT_RS.exists()
    assert set(rendered_rs_modules) == set(gen.OUT_RS_FILES)
    for name, rendered in rendered_rs_modules.items():
        assert gen.OUT_RS_FILES[name].read_text(encoding="utf-8") == rendered
    assert gen.OUT_RUNTIME_CALLABLES_RS.read_text(
        encoding="utf-8"
    ) == gen.render_runtime_callables_rs(data)
    assert gen.OUT_PY.read_text(encoding="utf-8") == gen.render_py(data)
    assert gen.OUT_JS_ABI.read_text(encoding="utf-8") == gen.render_js_abi(data)
    assert gen.OUT_TABLE_LAYOUT_INC.read_text(
        encoding="utf-8"
    ) == gen.render_table_layout_inc(data)
    for removed_path in gen.REMOVED_GENERATED_FILES:
        assert not removed_path.exists()
    assert gen.OUT_ALLOWED_IMPORTS.read_text(
        encoding="utf-8"
    ) == gen.render_allowed_imports(data)


def test_cpython_abi_link_import_discovery_covers_the_complete_crate() -> None:
    data = manifest.load_manifest()
    generated = dict(manifest.generator_cpython_abi_link_import_kinds())
    bridge_and_hook_exports = {
        "Py_DecRef",
        "Py_IncRef",
        "molt_capi_any_decref",
        "molt_capi_any_incref",
        "molt_capi_handle_to_borrowed_pyobj",
        "molt_capi_handle_to_pyobj",
        "molt_capi_pyobj_is_bridge_managed",
        "molt_capi_pyobj_to_handle",
        "molt_capi_result_to_pyobj",
        "molt_capi_semantic_type",
        "molt_capi_set_semantic_type",
        "molt_cpython_abi_init",
        "molt_cpython_abi_register_hooks",
    }

    assert bridge_and_hook_exports <= generated.keys()
    assert {generated[name] for name in bridge_and_hook_exports} == {"function"}
    link_imports = {
        entry["name"]: entry
        for entry in data["external_native_link_import"]
        if entry["name"] in bridge_and_hook_exports
    }
    assert set(link_imports) == bridge_and_hook_exports
    assert {
        entry["primitive_class"] for entry in link_imports.values()
    } == {"molt_cpython_abi_link_import"}


def test_wasm_abi_manifest_owns_static_type_section() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    static_types = data["static_type"]
    static_type_count = len(static_types)

    assert static_type_count > 50
    assert static_types[0] == {"params": [], "results": ["i64"]}
    assert static_types[1] == {"params": ["i64"], "results": []}
    assert {"params": [], "results": ["i32"]} in static_types
    assert static_types[31] == {"params": ["i64", "i64"], "results": ["i64", "i64"]}
    assert static_types[32] == {
        "params": ["i64", "i64", "i64"],
        "results": ["i64", "i64", "i64"],
    }
    assert all(entry["type"] < len(static_types) for entry in data["import"])

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "STATIC_FUNC_TYPES" in rendered_rs
    assert f"STATIC_TYPE_COUNT: u32 = {static_type_count}" in rendered_rs
    assert "WASM_STATIC_TYPES" in rendered_py
    assert f"WASM_STATIC_TYPE_COUNT: int = {static_type_count}" in rendered_py

    wasm_abi = (ROOT / "runtime/molt-backend-wasm/src/wasm_abi.rs").read_text(
        encoding="utf-8"
    )
    assert "static_func_type(" not in wasm_abi
    assert "const STATIC_FUNC_TYPES" not in wasm_abi


def test_wasm_abi_manifest_owns_runtime_export_policy() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    manifest_names = {entry["name"] for entry in data["import"]}
    imports_by_name = {entry["name"]: entry for entry in data["import"]}
    host_exports = set(data["runtime_export_policy"]["host_exports"])
    host_export_signatures = {
        entry["name"]: entry for entry in data["runtime_host_export_signature"]
    }
    fallback_specs = {
        entry["import"]: entry for entry in data["runtime_import_fallback"]
    }

    runtime_exports_path = ROOT / "src/molt/_wasm_runtime_exports.py"
    text = runtime_exports_path.read_text(encoding="utf-8")
    assert "wasm_imports.rs" not in text
    assert "WASM_IMPORT_REGISTRY" in text
    assert "_normalize_runtime_export_name" not in text
    assert "_HOST_RUNTIME_EXPORTS" not in text
    assert "_BROWSER_RUNTIME_IMPORT_FALLBACK_EXPORTS" not in text
    assert {"alloc", "runtime_init", "socket_connect", "task_new"} <= manifest_names
    assert imports_by_name["runtime_init"]["runtime_name"] == "molt_runtime_init"
    assert "callable_arity" not in imports_by_name["runtime_init"]
    assert (
        imports_by_name["runtime_shutdown"]["runtime_name"] == "molt_runtime_shutdown"
    )
    assert "callable_arity" not in imports_by_name["runtime_shutdown"]
    assert {
        "molt_runtime_shutdown",
        "molt_set_wasm_table_base",
        "molt_gpu_matmul_contiguous",
    } <= host_exports
    assert host_export_signatures["molt_bool_from_i32"] == {
        "name": "molt_bool_from_i32",
        "params": ["i32"],
        "results": ["i64"],
    }
    assert host_export_signatures["molt_buffer_acquire"] == {
        "name": "molt_buffer_acquire",
        "params": ["i64", "i32"],
        "results": ["i32"],
    }
    assert host_export_signatures["molt_handle_resolve"] == {
        "name": "molt_handle_resolve",
        "params": ["i64"],
        "results": ["i32"],
    }
    assert {"molt_gpu_matmul_contiguous", "molt_gpu_tensor__zeros"} <= host_exports
    assert "gpu_intrinsic_manifest_name" not in data
    assert fallback_specs["fast_dict_get"] == {
        "import": "fast_dict_get",
        "strategy": "call_bind_ic",
        "call_arity": 2,
        "exports": [
            "molt_call_bind_ic",
            "molt_callargs_new",
            "molt_callargs_push_pos",
        ],
    }
    assert fallback_specs["dict_getitem"] == {
        "import": "dict_getitem",
        "strategy": "direct_export",
        "exports": ["molt_dict_getitem_borrowed"],
    }

    rendered_py = gen.render_py(data)
    rendered_js_abi = json.loads(gen.render_js_abi(data))
    rendered_rs = _rendered_rs(gen, data)
    assert "GPU_INTRINSIC_MANIFEST_NAMES" not in rendered_rs
    assert "WASM_GPU_INTRINSIC_MANIFEST_NAMES" not in rendered_py
    assert "WASM_RUNTIME_HOST_EXPORTS" in rendered_py
    assert "WASM_RUNTIME_HOST_EXPORT_SIGNATURES" in rendered_py
    assert "WASM_RUNTIME_IMPORT_FALLBACK_EXPORTS" in rendered_py
    assert "WASM_RUNTIME_IMPORT_FALLBACK_SPECS" in rendered_py
    assert "WASM_RUNTIME_IMPORT_EXPORT_NAMES" in rendered_py
    assert "WASM_RUNTIME_EXPORT_BY_IMPORT" in rendered_py
    assert "WASM_RUNTIME_IMPORT_BY_EXPORT" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_SPLIT_EXPORT_NAMES" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_BY_SPLIT_EXPORT_NAME" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS" in rendered_py
    assert "def wasm_runtime_import_name" in rendered_py
    assert "def wasm_runtime_export_name" in rendered_py
    assert '("alloc", "molt_alloc")' in rendered_py
    assert "if name in WASM_RUNTIME_HOST_EXPORTS" in rendered_py
    assert '("runtime_init", "molt_runtime_init")' in rendered_py
    assert '("runtime_shutdown", "molt_runtime_shutdown")' in rendered_py
    assert '("socket_drop", "molt_socket_drop")' in rendered_py
    assert '"PyArg_ParseTuple": "molt_PyArg_ParseTuple"' in rendered_py
    assert '"molt_PyArg_ParseTuple": "PyArg_ParseTuple"' in rendered_py
    assert '"Py_EllipsisObject": "molt_Py_EllipsisObject"' in rendered_py
    assert '"Py_EllipsisObject": "data"' in rendered_py
    assert '"Py_GenericAliasType": "data"' in rendered_py
    assert '"Py_OptimizeFlag": "data"' in rendered_py
    assert '"PyType_Ready": "function"' in rendered_py
    assert rendered_js_abi["runtime_export_by_import"]["socket_drop"] == (
        "molt_socket_drop"
    )
    assert rendered_js_abi["runtime_export_by_import"]["PyArg_ParseTuple"] == (
        "molt_PyArg_ParseTuple"
    )
    assert rendered_js_abi["runtime_export_by_import"]["molt_PyArg_ParseTuple"] == (
        "molt_PyArg_ParseTuple"
    )
    assert (
        rendered_js_abi["runtime_import_canonical_names"]["molt_PyArg_ParseTuple"]
        == "PyArg_ParseTuple"
    )
    assert rendered_js_abi["runtime_export_by_import"]["PyType_Ready"] == (
        "molt_PyType_Ready"
    )
    assert rendered_js_abi["runtime_import_fallbacks"]["fast_dict_get"] == {
        "call_arity": 2,
        "exports": [
            "molt_call_bind_ic",
            "molt_callargs_new",
            "molt_callargs_push_pos",
        ],
        "strategy": "call_bind_ic",
    }
    assert "runtime_export_name" in rendered_rs
    assert "RUNTIME_HOST_EXPORTS" in rendered_rs
    assert "RUNTIME_HOST_EXPORT_SIGNATURES" in rendered_rs
    assert ".find(|export| *export == name)" in rendered_rs
    assert 'Self::Alloc => "molt_alloc"' in rendered_rs
    assert 'Self::RuntimeInit => "molt_runtime_init"' in rendered_rs
    assert '"molt_runtime_init" => Some(WasmRuntimeImport::RuntimeInit)' in rendered_rs
    assert (
        '"molt_runtime_shutdown" => Some(WasmRuntimeImport::RuntimeShutdown)'
        in rendered_rs
    )
    assert 'Self::SocketDrop => "molt_socket_drop"' in rendered_rs
    rendered_namespace: dict[str, object] = {}
    exec(rendered_py, rendered_namespace)
    assert rendered_namespace["wasm_runtime_import_name"]("molt_none") == "molt_none"
    assert rendered_namespace["wasm_runtime_export_name"]("molt_none") == "molt_none"
    assert rendered_namespace["wasm_import_signature"]("molt_none") == ((), ("i64",))
    assert rendered_namespace["wasm_import_signature"]("molt_bool_from_i32") == (
        ("i32",),
        ("i64",),
    )


def test_wasm_abi_manifest_owns_pure_profile_prefixes() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    prefixes = {entry["prefix"] for entry in data["pure_skip_prefix"]}
    assert {"process_", "socket", "db_", "ws_", "time_"} <= prefixes
    rendered_rs = _rendered_rs(gen, data)
    assert "pure_profile_skips_import" in rendered_rs
    assert "PURE_PROFILE_SKIP_PREFIXES" in rendered_rs


def test_wasm_abi_manifest_owns_runtime_callable_registry() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    assert "runtime_feature" not in manifest.MANIFEST.read_text(encoding="utf-8")
    imports = {entry["name"]: entry for entry in data["import"]}

    assert imports["importlib_import_transaction"]["type"] == 12
    assert imports["importlib_import_transaction"]["runtime_name"] == (
        "molt_importlib_import_transaction"
    )
    assert imports["importlib_import_transaction"]["callable_arity"] == 5
    assert imports["importlib_import_transaction"]["shared_runtime_callable"] is True
    assert imports["importlib_import_transaction"]["callable_dispatch"] == "trampoline"
    assert imports["types_bootstrap"]["runtime_name"] == "molt_types_bootstrap"
    assert imports["types_bootstrap"]["callable_arity"] == 0
    assert imports["types_bootstrap"].get("shared_runtime_callable") is None

    assert imports["socket_drop"]["callable_result"] == "void"
    assert imports["stream_close"]["callable_result"] == "void"
    assert imports["future_features"]["runtime_name"] == "molt_future_features"
    assert imports["site_help0"]["callable_arity"] == 0
    assert imports["site_help1"]["callable_arity"] == 1
    assert imports["load_intrinsic_runtime"]["runtime_name"] == (
        "molt_load_intrinsic_runtime"
    )
    assert imports["load_intrinsic_runtime"]["callable_arity"] == 2
    assert imports["set_app_callable_resolver"]["runtime_name"] == (
        "molt_set_app_callable_resolver"
    )
    assert imports["set_app_callable_resolver"]["type"] == 2
    assert "callable_arity" not in imports["set_app_callable_resolver"]
    assert imports["function_init_metadata_packed"]["runtime_name"] == (
        "molt_function_init_metadata_packed"
    )
    assert imports["function_init_metadata_packed"]["callable_arity"] == 4
    assert imports["file_open_ex"]["runtime_name"] == "molt_file_open_ex"
    assert imports["file_open_ex"]["callable_arity"] == 8
    assert imports["file_exit_method"]["runtime_name"] == "molt_file_exit_method"
    assert imports["file_exit_method"]["callable_arity"] == 4
    assert imports["env_clear"]["runtime_name"] == "molt_env_clear"
    assert imports["env_clear"]["callable_arity"] == 0
    assert imports["env_clear"].get("callable_result", "i64") == "i64"
    assert imports["typing_type_param"]["runtime_name"] == "molt_typing_type_param"
    assert imports["typing_type_param"]["callable_arity"] == 2

    shared_callables = gen._shared_runtime_callables(data)
    reserved_callables = data["reserved_runtime_callable"]
    assert [entry["index"] for entry in shared_callables] == list(
        range(len(shared_callables))
    )
    assert [
        {
            "index": entry["index"],
            "runtime_name": entry["runtime_name"],
            "import_name": entry["import_name"],
            "callable_arity": entry["callable_arity"],
            "trampoline_abi": entry["trampoline_abi"],
        }
        for entry in shared_callables[: len(reserved_callables)]
    ] == [
        {**entry, "trampoline_abi": entry.get("trampoline_abi", "unpack_args")}
        for entry in reserved_callables
    ]
    assert shared_callables[-1] == {
        "index": len(reserved_callables),
        "runtime_name": "molt_importlib_import_transaction",
        "import_name": "importlib_import_transaction",
        "callable_arity": 5,
        "callable_result": None,
        "callable_dispatch": "trampoline",
        "runtime_feature": None,
        "symbol_path": "crate::molt_importlib_import_transaction",
    }
    assert all(
        entry["runtime_name"] != "molt_types_bootstrap" for entry in shared_callables
    )

    rendered_runtime = gen.render_runtime_callables_rs(data)
    assert "molt_importlib_import_transaction" in rendered_runtime
    assert "molt_cpython_abi_cext_call_trampoline" in rendered_runtime
    assert "molt_types_bootstrap" not in rendered_runtime
    assert (
        f'({len(reserved_callables)}, "molt_importlib_import_transaction", '
        '"importlib_import_transaction", 5, "trampoline")' in gen.render_py(data)
    )

    broken_marker = copy.deepcopy(data)
    broken_marker["import"][0]["shared_runtime_callable"] = "yes"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="invalid shared_runtime_callable"
    ):
        manifest.validate_loaded_manifest(broken_marker)

    broken_dispatch = copy.deepcopy(data)
    broken_dispatch["import"][0]["callable_dispatch"] = "sideways"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="invalid callable_dispatch"
    ):
        manifest.validate_loaded_manifest(broken_dispatch)

    broken_missing_arity = copy.deepcopy(data)
    broken_missing_arity["import"][0]["shared_runtime_callable"] = True
    broken_missing_arity["import"][0].pop("callable_arity", None)
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="cannot set shared_runtime_callable without callable_arity",
    ):
        manifest.validate_loaded_manifest(broken_missing_arity)
    assert imports["logging_formatter_format_time"]["callable_arity"] == 3
    assert imports["logging_stream_handler_new"]["callable_arity"] == 2
    assert imports["logging_basic_config"]["callable_arity"] == 4
    for name, arity in {
        "abc_bootstrap": 0,
        "collections_abc_runtime_types": 0,
        "abc_get_cache_token": 0,
        "abc_init": 1,
        "abc_register": 2,
        "abc_instancecheck": 2,
        "abc_subclasscheck": 2,
        "abc_get_dump": 1,
        "abc_reset_registry": 1,
        "abc_reset_caches": 1,
        "abc_update_abstractmethods": 1,
        "abc_abstractmethod_check": 1,
    }.items():
        assert imports[name]["runtime_name"] == f"molt_{name}"
        assert imports[name]["callable_arity"] == arity
    for name, arity in {
        "array_new": 1,
        "array_from_list": 2,
        "array_append": 2,
        "array_buffer_info": 1,
        "array_count": 2,
        "array_delitem": 2,
        "array_extend": 2,
        "array_frombytes": 2,
        "array_getitem": 2,
        "array_index": 2,
        "array_insert": 3,
        "array_itemsize": 1,
        "array_len": 1,
        "array_pop": 2,
        "array_repeat": 2,
        "array_repeat_in_place": 2,
        "array_remove": 2,
        "array_reverse": 1,
        "array_setitem": 3,
        "array_tobytes": 1,
        "array_tolist": 1,
        "array_typecode": 1,
    }.items():
        assert imports[name]["runtime_name"] == f"molt_{name}"
        assert imports[name]["callable_arity"] == arity
    assert imports["runtime_active_runtime"]["callable_arity"] == 0
    assert imports["codecs_decode"]["runtime_name"] == "molt_codecs_decode"
    assert imports["codecs_decode"]["callable_arity"] == 3
    assert imports["codecs_encode"]["runtime_name"] == "molt_codecs_encode"
    assert imports["codecs_encode"]["callable_arity"] == 3
    assert imports["cbor_parse_scalar_obj"]["runtime_name"] == (
        "molt_cbor_parse_scalar_obj"
    )
    assert imports["cbor_parse_scalar_obj"]["callable_arity"] == 1
    assert imports["msgpack_parse_scalar_obj"]["runtime_name"] == (
        "molt_msgpack_parse_scalar_obj"
    )
    assert imports["msgpack_parse_scalar_obj"]["callable_arity"] == 1
    assert imports["thread_current_native_id"]["runtime_name"] == (
        "molt_thread_current_native_id"
    )
    assert imports["thread_current_native_id"]["callable_arity"] == 0
    assert imports["stream_drop"].get("runtime_feature") is None
    assert imports["asyncio_future_drop"]["runtime_feature"] == "stdlib_asyncio"
    assert imports["pipe_transport_drop"]["runtime_feature"] == "stdlib_asyncio"
    assert imports["email_message_drop"]["runtime_feature"] == "stdlib_email"
    assert imports["xml_element_drop"]["runtime_feature"] == "stdlib_xml"
    assert imports["statistics_mean_slice"]["runtime_feature"] == "stdlib_math"
    assert imports["statistics_stdev_slice"]["runtime_feature"] == "stdlib_math"
    assert not any(name.startswith("sqlite3_") for name in imports)
    dual_use_reserved_imports = {"object_new_bound"}
    for reserved in data["reserved_runtime_callable"]:
        if reserved["import_name"] in dual_use_reserved_imports:
            reserved_import = imports[reserved["import_name"]]
            assert "runtime_name" not in reserved_import
            assert "callable_arity" not in reserved_import
            assert "callable_result" not in reserved_import
        else:
            assert reserved["import_name"] not in imports

    rendered_rs = _rendered_rs(gen, data)
    rendered_runtime_rs = gen.render_runtime_callables_rs(data)
    rendered_py = gen.render_py(data)
    assert "RUNTIME_CALLABLE_IMPORTS" in rendered_rs
    assert "use super::import_tokens::WasmRuntimeImport;" in rendered_rs
    assert "import: None" in rendered_rs
    assert "import: Some(WasmRuntimeImport::ImportlibImportTransaction)" in rendered_rs
    assert "ReservedRuntimeCallableDispatch::Trampoline" in rendered_rs
    assert "pub(crate) fn runtime_callable_import" in rendered_rs
    assert (
        '"molt_importlib_import_transaction" => Some(WasmRuntimeImport::ImportlibImportTransaction)'
        in rendered_rs
    )
    assert (
        '"socket_drop" => Some(WasmRuntimeImport::SocketDrop),\n'
        '        "molt_socket_drop" => Some(WasmRuntimeImport::SocketDrop),'
    ) in rendered_rs
    assert "runtime_callable_import_name" not in rendered_rs
    assert "WASM_RUNTIME_CALLABLE_IMPORT_BY_RUNTIME" in rendered_py
    assert "WASM_RUNTIME_CALLABLE_IMPORT_BY_IMPORT" in rendered_py
    assert "WASM_RUNTIME_CALLABLE_ARITY_BY_RUNTIME" in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_ARITY_BY_RUNTIME" in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_IMPORTS" not in rendered_py
    assert "WASM_RUNTIME_CALLABLE_LOOKUP_ROWS" not in rendered_py
    assert "def wasm_runtime_callable_spec(name: str)" in rendered_py
    assert "def wasm_runtime_callable_import_name" not in rendered_py
    assert "def wasm_runtime_callable_arity" in rendered_py
    assert "def wasm_runtime_callable_result" in rendered_py
    assert "molt_sqlite3_" not in rendered_rs
    assert "molt_sqlite3_" not in rendered_py
    assert "RuntimeCallableResult::Void" in rendered_rs
    assert "ReservedRuntimeCallableSpec" in rendered_rs
    assert "RUNTIME_CALLABLE_IMPORTS" in rendered_rs
    assert "ReservedRuntimeCallableDispatch" in rendered_rs
    assert "ReservedRuntimeCallableTrampolineAbi" in rendered_rs
    assert "RuntimeCallableResult" in rendered_rs
    assert "RESERVED_RUNTIME_CALLABLE_SPECS" in rendered_rs
    assert "RESERVED_RUNTIME_CALLABLE_COUNT" in rendered_rs
    assert "runtime_callable_key_from_symbol_name" in rendered_runtime_rs
    assert "runtime_callable_target_ptr" in rendered_runtime_rs
    assert "runtime_callable_returns_void_from_target_ptr" in rendered_runtime_rs
    assert "pub(crate) fn python_builtin_function_info" in rendered_runtime_rs
    assert '        "len" => Some(PythonBuiltinFunctionInfo {' in rendered_runtime_rs
    assert "            index: 2," in rendered_runtime_rs
    assert '            runtime_name: "molt_len",' in rendered_runtime_rs
    assert "pub(crate) fn python_builtin_function_target_ptr(" in rendered_runtime_rs
    assert (
        '        "molt_len" => Some(crate::molt_len as *const ()),'
        in rendered_runtime_rs
    )
    assert "pub(crate) fn python_builtin_function_info(" in rendered_runtime_rs
    assert "            fn_ptr:" not in rendered_runtime_rs
    assert '#[cfg(not(target_arch = "wasm32"))]' in rendered_runtime_rs
    assert '            posonly_params: &["obj"],' in rendered_runtime_rs
    assert "            defaults: &[]," in rendered_runtime_rs
    assert '        "print" => Some(PythonBuiltinFunctionInfo {' in rendered_runtime_rs
    assert '            runtime_name: "molt_print_builtin",' in rendered_runtime_rs
    assert '            vararg: Some("args"),' in rendered_runtime_rs
    assert (
        '            kwonly_params: &["sep", "end", "file", "flush"],'
        in rendered_runtime_rs
    )
    assert (
        '            kw_defaults: &[("sep", GeneratedBuiltinDefaultValue::Str(" ")), '
        r'("end", GeneratedBuiltinDefaultValue::Str("\n")), '
        '("file", GeneratedBuiltinDefaultValue::None), '
        '("flush", GeneratedBuiltinDefaultValue::Bool(false))],'
    ) in rendered_runtime_rs
    assert (
        "pub(crate) const PYTHON_BUILTIN_FUNCTION_COUNT: usize = 41;"
        in rendered_runtime_rs
    )
    assert "RUNTIME_VOID_CALLABLE_NAMES" not in rendered_runtime_rs
    assert "VOID_CALLABLE_TARGETS" not in rendered_runtime_rs
    assert "crate::intrinsics::resolve_symbol" not in rendered_runtime_rs
    void_runtime_names = {
        entry["runtime_name"]
        for entry in data["import"]
        if entry.get("callable_result") == "void"
    }
    assert "molt_asyncio_future_drop" in void_runtime_names
    shared_runtime_names = {entry["runtime_name"] for entry in shared_callables}
    assert void_runtime_names.isdisjoint(shared_runtime_names)
    assert "VOID_RESERVED_RUNTIME_CALLABLE_INDICES" in rendered_runtime_rs
    for runtime_name in sorted(void_runtime_names):
        assert f"crate::{runtime_name} as *const" not in rendered_runtime_rs
    for entry in gen._shared_runtime_callables(data):
        runtime_name = entry["runtime_name"]
        symbol_path = entry["symbol_path"]
        if symbol_path == runtime_name:
            assert (
                f"Some(crate::{runtime_name} as *const ())" not in rendered_runtime_rs
            )
            assert (
                f"let _ = crate::{runtime_name} as *const ();"
                not in rendered_runtime_rs
            )
        assert f"Some({symbol_path} as *const ())" in rendered_runtime_rs
        assert f"let _ = {symbol_path} as *const ();" in rendered_runtime_rs
    assert "fn_addr!(crate::" not in rendered_runtime_rs
    assert "fn_addr!(molt_xml_element_drop)" not in rendered_runtime_rs
    assert "RUNTIME_POLL_CALLABLE_KEY_BASE" in rendered_runtime_rs
    assert (
        ".map(|entry| RUNTIME_CALLABLE_KEY_BASE + entry.index)" in rendered_runtime_rs
    )
    assert 'runtime_name: "molt_type_call"' in rendered_runtime_rs
    assert '"type_call" => Some(WasmRuntimeImport::TypeCall)' not in rendered_rs
    assert (
        '"object_new_bound" => Some(WasmRuntimeImport::ObjectNewBound)' in rendered_rs
    )
    assert "1 => Some(crate::molt_async_sleep_poll as *const ())" in rendered_runtime_rs

    wasm_abi = (ROOT / "runtime/molt-backend-wasm/src/wasm_abi.rs").read_text(
        encoding="utf-8"
    )
    assert "wasm_runtime_callables.inc" not in wasm_abi
    assert "macro_rules! entry_list" not in wasm_abi
    assert "runtime_callable_import_name" not in wasm_abi

    function_abi = (
        ROOT / "runtime/molt-runtime/src/builtins/functions/function_abi.rs"
    ).read_text(encoding="utf-8")
    call_function = (ROOT / "runtime/molt-runtime/src/call/function.rs").read_text(
        encoding="utf-8"
    )
    assert "wasm_runtime_callables.inc" not in function_abi
    assert "wasm_poll_callables.inc" not in function_abi
    assert '"molt_type_call" => Some' not in function_abi
    assert "molt_async_sleep_poll as *const" not in function_abi
    assert "runtime_callable_returns_void" in function_abi
    assert "VOID_INTRINSICS" not in call_function
    assert "runtime_callable_returns_void(fn_ptr)" in call_function


def test_wasm_runtime_callable_resolver_is_app_local_in_production() -> None:
    registry = (ROOT / "runtime/molt-runtime/src/intrinsics/registry.rs").read_text(
        encoding="utf-8"
    )
    intrinsics_mod = (ROOT / "runtime/molt-runtime/src/intrinsics/mod.rs").read_text(
        encoding="utf-8"
    )

    assert "#[cfg(test)]\nuse crate::intrinsics::generated::resolve_symbol;" in registry
    assert '#[cfg(any(target_arch = "wasm32", test))]' not in registry
    assert '#[cfg(not(any(target_arch = "wasm32", test)))]' not in registry
    assert 'pub extern "C" fn molt_set_app_callable_resolver' in registry
    assert "pub(crate) fn try_app_resolve_runtime_callable" in registry
    assert "try_app_resolve_symbol(spec.symbol)" in registry
    assert "pub(crate) use registry::try_app_resolve_symbol;" in intrinsics_mod


def test_runtime_features_are_derived_not_manifest_owned() -> None:
    gen = _load_gen_wasm_abi()
    loaded = gen.load_manifest()
    manifest.validate_loaded_manifest(copy.deepcopy(loaded))
    loaded_imports = {entry["name"]: entry for entry in loaded["import"]}

    raw = _raw_manifest()
    for entry in raw["import"]:
        if entry["name"] == "hash_builtin":
            entry["runtime_feature"] = loaded_imports["hash_builtin"]["runtime_feature"]
            break
    else:  # pragma: no cover - fixture corruption
        raise AssertionError("hash_builtin import missing")

    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="runtime_feature is generated from intrinsics/categories.toml",
    ):
        manifest.validate_loaded_manifest(raw, reject_manual_runtime_features=True)


def test_wasm_abi_reserved_runtime_callable_import_names_are_fail_closed() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    broken = copy.deepcopy(data)
    broken["import"].append({"name": "type_call", "type": 2})
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicated in \\[\\[import\\]\\]"
    ):
        manifest.validate_loaded_manifest(broken)
    broken = copy.deepcopy(data)
    for entry in broken["import"]:
        if entry["name"] == "object_new_bound":
            entry["runtime_name"] = "molt_object_new_bound"
            entry["callable_arity"] = 1
            break
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="dual-use import must not duplicate callable metadata",
    ):
        manifest.validate_loaded_manifest(broken)

    broken = copy.deepcopy(data)
    for entry in broken["import"]:
        if entry["name"] == "object_new_bound":
            entry["type"] = 3
            break
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="reserved runtime callable 'molt_object_new_bound'",
    ):
        manifest.validate_loaded_manifest(broken)

    broken = copy.deepcopy(data)
    broken["import"].append(
        {
            "name": "shadow_type_call",
            "type": 2,
            "runtime_name": "molt_type_call",
            "callable_arity": 1,
        }
    )
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="reserved runtime callable 'molt_type_call'",
    ):
        manifest.validate_loaded_manifest(broken)


def test_witness_frontier_runtime_callables_must_be_reserved() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    broken = copy.deepcopy(data)
    broken["reserved_runtime_callable"] = [
        entry
        for entry in broken["reserved_runtime_callable"]
        if entry["runtime_name"] != "molt_type_new"
    ]
    for index, entry in enumerate(broken["reserved_runtime_callable"]):
        entry["index"] = index

    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="witness-frontier runtime callables are not reserved: molt_type_new",
    ):
        manifest.validate_loaded_manifest(broken)


def test_wasm_abi_runtime_import_aliases_are_unambiguous() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    broken = copy.deepcopy(data)
    broken["import"].append({"name": "molt_socket_drop", "type": 2})
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="runtime import aliases collide with canonical import names",
    ):
        manifest.validate_loaded_manifest(broken)


def test_wasm_abi_manifest_classifies_raw_intrinsics_fail_closed() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    imports = {entry["name"]: entry for entry in data["import"]}
    runtime_callables = {
        entry["runtime_name"] for entry in data["import"] if "runtime_name" in entry
    }

    assert "molt_json_parse_scalar" in data["non_runtime_callable_intrinsic"]
    assert "molt_gpu_prim_create_tensor" in data["non_runtime_callable_intrinsic"]
    assert "runtime_name" not in imports["json_parse_scalar"]
    assert "molt_json_parse_scalar" not in runtime_callables
    assert "molt_gpu_prim_create_tensor" not in runtime_callables
    assert imports["json_parse_scalar_obj"]["runtime_name"] == (
        "molt_json_parse_scalar_obj"
    )

    broken = _raw_manifest()
    broken["non_runtime_callable_intrinsic"] = []
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="molt_json_parse_scalar.*non_runtime_callable_intrinsic",
    ):
        manifest.validate_loaded_manifest(broken)


def test_wasm_abi_runtime_callable_intrinsics_match_rust_exports() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    # load_manifest performs the full Rust-export ABI validation; keep readable
    # sentinels for explicit manifest-owned callables and the old broad
    # synthesis class.
    imports = {
        entry["runtime_name"]: entry
        for entry in data["import"]
        if "runtime_name" in entry
    }
    assert imports["molt_importlib_import_transaction"]["callable_arity"] == 5
    assert imports["molt_load_intrinsic_runtime"]["callable_arity"] == 2
    assert imports["molt_types_bootstrap"]["callable_arity"] == 0
    assert imports["molt_file_exit_method"]["callable_arity"] == 4
    assert imports["molt_logging_formatter_format_time"]["callable_arity"] == 3
    assert imports["molt_logging_stream_handler_new"]["callable_arity"] == 2
    assert imports["molt_logging_basic_config"]["callable_arity"] == 4


def test_wasm_abi_manifest_owns_lir_runtime_calls() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    calls = {entry["variant"]: entry for entry in data["lir_runtime_call"]}
    op_loop_calls = {entry["kind"]: entry for entry in data["op_loop_runtime_call"]}

    assert calls["Add"]["import_name"] == "add"
    assert calls["Add"]["boxed_operand_count"] == 2
    assert calls["FloorDiv"]["import_name"] == "floordiv"
    assert calls["Neg"]["boxed_operand_count"] == 1
    assert calls["StoreIndex"]["boxed_operand_count"] == 3
    assert calls["GetAttrName"]["boxed_operand_count"] == 2
    assert calls["Iter"]["boxed_operand_count"] == 1
    assert calls["ModuleImportStar"] == {
        "variant": "ModuleImportStar",
        "import_name": "module_import_star",
    }
    assert calls["ContextDepth"] == {
        "variant": "ContextDepth",
        "import_name": "context_depth",
    }
    assert calls["IntFromI64"]["import_name"] == "int_from_i64"
    assert op_loop_calls["module_import_star"]["lir_variant"] == "ModuleImportStar"
    assert op_loop_calls["module_import_star"]["lir_operand_count"] == 2
    assert op_loop_calls["context_depth"]["lir_variant"] == "ContextDepth"
    assert op_loop_calls["context_depth"]["lir_operand_count"] == 0
    assert op_loop_calls["gpu_thread_id"] == {
        "kind": "gpu_thread_id",
        "import_name": "gpu_thread_id",
        "args": [],
        "required_imports": ["gpu_thread_id"],
        "sink": "result_or_drop",
    }
    assert op_loop_calls["gpu_barrier"] == {
        "kind": "gpu_barrier",
        "import_name": "gpu_barrier",
        "args": [],
        "required_imports": ["gpu_barrier"],
        "sink": "result_or_drop",
    }

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_lir_rs = rendered_rs_modules["lir_runtime_calls.rs"]
    assert "enum LirRuntimeCall" in rendered_lir_rs
    assert "use super::import_tokens::WasmRuntimeImport;" in rendered_lir_rs
    assert "pub(crate) const fn import(self) -> WasmRuntimeImport" in rendered_lir_rs
    assert "Self::FloorDiv => WasmRuntimeImport::Floordiv" in rendered_lir_rs
    assert "pub(crate) const fn boxed_operand_count" in rendered_lir_rs
    assert "Self::StoreIndex => Some(3)" in rendered_lir_rs
    assert "Self::ModuleImport => WasmRuntimeImport::ModuleImport" in rendered_lir_rs
    assert 'Self::ModuleImport => "module_import"' not in rendered_lir_rs
    assert "Self::ModuleImport => Some" not in rendered_lir_rs
    assert "pub(crate) const fn import_name" not in rendered_lir_rs
    assert "self.import().name()" not in rendered_lir_rs
    assert "lir_fixed_runtime_call" in rendered_lir_rs
    assert '"context_depth" => Some(LirFixedRuntimeCall' in rendered_lir_rs

    broken = copy.deepcopy(data)
    broken["lir_runtime_call"][0]["import_name"] = "not_a_real_import"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="references unknown import"
    ):
        manifest.validate_loaded_manifest(broken)
    broken_count = copy.deepcopy(data)
    broken_count["lir_runtime_call"][0]["boxed_operand_count"] = -1
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed_operand_count"):
        manifest.validate_loaded_manifest(broken_count)

    local_facade = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/lir_fast/runtime_calls.rs"
    ).read_text(encoding="utf-8")
    assert "enum LirRuntimeCall" not in local_facade
    assert "match kind" not in local_facade
    assert "lir_fixed_runtime_call" in local_facade
    assert "LirFixedRuntimeCall" in local_facade
    assert "crate::wasm_abi_generated::LirRuntimeCall" in local_facade

    call_abi = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/lir_fast/lir_runtime_ops/call_abi.rs"
    ).read_text(encoding="utf-8")
    assert "boxed_operand_count().unwrap_or_else" in call_abi
    assert "runtime_call, 2" not in call_abi


def test_wasm_abi_manifest_owns_container_runtime_selector() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    selectors = {
        (entry["op"], entry["fact"]): (
            entry["import_name"],
            entry.get("lir_variant"),
        )
        for entry in data["container_runtime_selector"]
    }

    assert selectors == {
        ("index", "flat_list_int"): ("list_int_getitem", "ListIntGetitem"),
        ("store_index", "flat_list_int"): (
            "list_int_setitem",
            "ListIntSetitem",
        ),
        ("index", "dict"): ("dict_getitem", "DictGetitem"),
        ("index", "tuple"): ("tuple_getitem", "TupleGetitem"),
        ("store_index", "dict"): ("dict_setitem", "DictSetitem"),
        ("contains", "set"): ("set_contains", "SetContains"),
        ("contains", "dict"): ("dict_contains", "DictContains"),
        ("contains", "list"): ("list_contains", "ListContains"),
        ("contains", "str"): ("str_contains", "StrContains"),
        ("len", "list"): ("len_list", None),
        ("len", "str"): ("len_str", None),
        ("len", "dict"): ("len_dict", None),
        ("len", "tuple"): ("len_tuple", None),
        ("len", "set"): ("len_set", None),
    }

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_selector_rs = rendered_rs_modules["container_runtime_selector.rs"]
    rendered_mod_rs = rendered_rs_modules["mod.rs"]
    rendered_py = gen.render_py(data)
    assert "WASM_CONTAINER_RUNTIME_SELECTORS" in rendered_selector_rs
    assert "WasmContainerRuntimeFact::FlatListInt" in rendered_selector_rs
    assert "import: WasmRuntimeImport::ListIntGetitem" in rendered_selector_rs
    assert "LirRuntimeCall::ListIntGetitem" in rendered_selector_rs
    assert "wasm_container_runtime_selection" in rendered_selector_rs
    assert "mod container_runtime_selector;" in rendered_mod_rs
    assert "WasmContainerRuntimeOp" in rendered_mod_rs
    assert "WASM_CONTAINER_RUNTIME_SELECTORS" in rendered_py
    assert (
        '("index", "flat_list_int", "list_int_getitem", "ListIntGetitem")'
        in rendered_py
    )

    broken_duplicate = copy.deepcopy(data)
    broken_duplicate["container_runtime_selector"].append(
        copy.deepcopy(broken_duplicate["container_runtime_selector"][0])
    )
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate container_runtime_selector"
    ):
        manifest.validate_loaded_manifest(broken_duplicate)

    broken_import = copy.deepcopy(data)
    broken_import["container_runtime_selector"][0]["import_name"] = "not_a_real_import"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="references unknown import"
    ):
        manifest.validate_loaded_manifest(broken_import)

    broken_lir = copy.deepcopy(data)
    broken_lir["container_runtime_selector"][0]["lir_variant"] = "DictGetitem"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="does not match lir_variant"
    ):
        manifest.validate_loaded_manifest(broken_lir)


def test_wasm_abi_manifest_owns_method_ic_selector() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    selectors = {
        (entry["family"], entry["extra_arg_count"]): entry["import_name"]
        for entry in data["method_ic_selector"]
    }

    assert selectors == {
        ("method", 0): "call_method_ic0",
        ("method", 1): "call_method_ic1",
        ("method", 2): "call_method_ic2",
        ("method", 3): "call_method_ic3",
        ("method", 4): "call_method_ic4",
        ("super_method", 0): "call_super_method_ic0",
        ("super_method", 1): "call_super_method_ic1",
        ("super_method", 2): "call_super_method_ic2",
        ("super_method", 3): "call_super_method_ic3",
        ("super_method", 4): "call_super_method_ic4",
    }

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_selector_rs = rendered_rs_modules["method_ic_selector.rs"]
    rendered_mod_rs = rendered_rs_modules["mod.rs"]
    rendered_py = gen.render_py(data)
    assert "WASM_METHOD_IC_SELECTORS" in rendered_selector_rs
    assert "WASM_METHOD_IC_MAX_EXTRA_ARGS: usize = 4" in rendered_selector_rs
    assert "WasmMethodIcFamily::Method" in rendered_selector_rs
    assert "WasmMethodIcFamily::SuperMethod" in rendered_selector_rs
    assert "import: WasmRuntimeImport::CallSuperMethodIc4" in rendered_selector_rs
    assert "wasm_method_ic_selection" in rendered_selector_rs
    assert "mod method_ic_selector;" in rendered_mod_rs
    assert "WasmMethodIcFamily" in rendered_mod_rs
    assert "WASM_METHOD_IC_SELECTORS" in rendered_py
    assert '("super_method", 4, "call_super_method_ic4")' in rendered_py

    broken_missing = copy.deepcopy(data)
    broken_missing["method_ic_selector"] = broken_missing["method_ic_selector"][:-1]
    with pytest.raises(manifest.WasmAbiManifestError, match="must declare exactly"):
        manifest.validate_loaded_manifest(broken_missing)

    broken_duplicate = copy.deepcopy(data)
    broken_duplicate["method_ic_selector"].append(
        copy.deepcopy(broken_duplicate["method_ic_selector"][0])
    )
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate method_ic_selector"
    ):
        manifest.validate_loaded_manifest(broken_duplicate)

    broken_import = copy.deepcopy(data)
    broken_import["method_ic_selector"][0]["import_name"] = "not_a_real_import"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="references unknown import"
    ):
        manifest.validate_loaded_manifest(broken_import)

    broken_count = copy.deepcopy(data)
    broken_count["method_ic_selector"][0]["extra_arg_count"] = 5
    with pytest.raises(manifest.WasmAbiManifestError, match="invalid extra_arg_count"):
        manifest.validate_loaded_manifest(broken_count)


def test_wasm_abi_manifest_owns_numeric_runtime_selector() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    selectors = {
        entry["kind"]: (
            entry["import_name"],
            entry["op_loop_variant"],
            entry.get("lir_variant"),
            entry.get("lir_operand_count"),
            tuple(entry["deps"]),
        )
        for entry in data["numeric_runtime_selector"]
    }

    assert selectors["add"] == ("add", "Add", "Add", 2, ("add", "str_concat"))
    assert selectors["inplace_add"] == (
        "inplace_add",
        "Add",
        "InplaceAdd",
        2,
        ("inplace_add", "str_concat"),
    )
    assert selectors["shl"] == ("lshift", "LShift", "LShift", 2, ("lshift",))
    assert selectors["bit_not"] == ("invert", "Invert", "Invert", 1, ("invert",))
    assert selectors["pow_mod"] == ("pow_mod", "PowMod", "PowMod", 3, ("pow_mod",))
    assert selectors["vec_sum_int"] == (
        "vec_sum_int",
        "VectorReduction",
        None,
        None,
        ("vec_sum_int",),
    )

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_selector_rs = rendered_rs_modules["numeric_runtime_selector.rs"]
    rendered_lir_rs = rendered_rs_modules["lir_runtime_calls.rs"]
    rendered_mod_rs = rendered_rs_modules["mod.rs"]
    rendered_py = gen.render_py(data)
    assert "WASM_NUMERIC_RUNTIME_SELECTORS" in rendered_selector_rs
    assert "WasmNumericOpLoopKind::VectorReduction" in rendered_selector_rs
    assert "import: WasmRuntimeImport::InplaceAdd" in rendered_selector_rs
    assert "LirRuntimeCall::InplaceAdd" in rendered_selector_rs
    assert (
        "deps: &[WasmRuntimeImport::InplaceAdd, WasmRuntimeImport::StrConcat]"
        in rendered_selector_rs
    )
    assert '"shl" => Some(WasmNumericRuntimeSelection' in rendered_selector_rs
    assert "call: LirRuntimeCall::PowMod" in rendered_lir_rs
    assert "mod numeric_runtime_selector;" in rendered_mod_rs
    assert "WasmNumericOpLoopKind" in rendered_mod_rs
    assert "WASM_NUMERIC_RUNTIME_SELECTORS" in rendered_py
    assert '("bit_not", "invert", "Invert", "Invert", 1, ("invert",))' in rendered_py

    broken_duplicate = copy.deepcopy(data)
    broken_duplicate["numeric_runtime_selector"].append(
        copy.deepcopy(broken_duplicate["numeric_runtime_selector"][0])
    )
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate numeric_runtime_selector"
    ):
        manifest.validate_loaded_manifest(broken_duplicate)

    broken_import = copy.deepcopy(data)
    broken_import["numeric_runtime_selector"][0]["import_name"] = "not_a_real_import"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="references unknown import"
    ):
        manifest.validate_loaded_manifest(broken_import)

    broken_lir = copy.deepcopy(data)
    broken_lir["numeric_runtime_selector"][0]["lir_variant"] = "Sub"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="does not match lir_variant"
    ):
        manifest.validate_loaded_manifest(broken_lir)

    broken_deps = copy.deepcopy(data)
    broken_deps["numeric_runtime_selector"][0]["deps"] = ["str_concat"]
    with pytest.raises(manifest.WasmAbiManifestError, match="deps must include"):
        manifest.validate_loaded_manifest(broken_deps)

    broken_count = copy.deepcopy(data)
    broken_count["numeric_runtime_selector"][0]["lir_operand_count"] = -1
    with pytest.raises(
        manifest.WasmAbiManifestError, match="invalid lir_operand_count"
    ):
        manifest.validate_loaded_manifest(broken_count)


def test_wasm_abi_manifest_owns_python_runtime_import_signatures() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    static_types = data["static_type"]
    imports = {entry["name"]: entry for entry in data["import"]}

    def signature(import_name: str) -> tuple[tuple[str, ...], tuple[str, ...]]:
        import_entry = imports[import_name]
        static_type = static_types[import_entry["type"]]
        return tuple(static_type["params"]), tuple(static_type["results"])

    assert signature("function_set_builtin") == (("i64",), ("i64",))
    assert signature("string_from_bytes") == (("i32", "i64", "i32"), ("i32",))
    assert signature("codecs_decode") == (("i64", "i64", "i64"), ("i64",))

    rendered_py = gen.render_py(data)
    assert "WASM_IMPORT_SIGNATURES" in rendered_py
    assert "WASM_IMPORT_SIGNATURE_BY_NAME" in rendered_py
    assert "WASM_IMPORT_NAME_BY_LOOKUP" in rendered_py
    rendered_ns = _exec_rendered_py(rendered_py)
    assert rendered_ns["wasm_import_name"]("molt_socket_drop") == "socket_drop"
    assert rendered_ns["wasm_runtime_import_name"]("molt_socket_drop") == "socket_drop"
    assert rendered_ns["wasm_import_result_kind"]("molt_socket_drop") == "nil"
    assert "def wasm_import_name" in rendered_py
    assert "def wasm_import_signature" in rendered_py
    assert "def wasm_import_result_kind" in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_ARITY_BY_RUNTIME" in rendered_py
    assert "WASM_RUNTIME_CALLABLE_ARITY_BY_RUNTIME" in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_IMPORTS" not in rendered_py
    assert "WASM_RUNTIME_CALLABLE_LOOKUP_ROWS" not in rendered_py
    assert rendered_ns["wasm_runtime_callable_spec"](
        "molt_importlib_import_transaction"
    ) == ("importlib_import_transaction", 5, "i64")


def test_wasm_abi_deletes_pre_emission_import_dependency_table() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    op_loop_calls = {entry["kind"]: entry for entry in data["op_loop_runtime_call"]}

    rendered_rs = _rendered_rs(gen, data)
    assert "op_import_dep" not in data
    assert "OP_IMPORT_DEPS" not in rendered_rs
    assert op_loop_calls["module_cache_del"]["required_imports"] == ["module_cache_del"]
    assert op_loop_calls["context_enter"]["required_imports"] == [
        "context_enter",
        "context_exit",
        "context_depth",
        "context_closing",
        "context_null",
        "context_unwind",
        "context_unwind_to",
    ]
    assert op_loop_calls["thread_submit"]["required_imports"] == [
        "thread_poll",
        "thread_submit",
    ]
    assert op_loop_calls["gpu_thread_id"]["required_imports"] == ["gpu_thread_id"]
    assert op_loop_calls["gpu_barrier"]["required_imports"] == ["gpu_barrier"]
    assert not (
        ROOT / "runtime/molt-backend-wasm/src/wasm/module_abi/runtime_import_demand.rs"
    ).exists()


def test_wasm_abi_manifest_owns_bulk_memory_ops() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    ops = {entry["kind"]: entry for entry in data["wasm_bulk_memory_op"]}

    assert ops == {
        "memory_copy": {
            "kind": "memory_copy",
            "instruction": "memory_copy",
            "arg_count": 3,
        },
        "memory_fill": {
            "kind": "memory_fill",
            "instruction": "memory_fill",
            "arg_count": 3,
        },
    }

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_bulk_rs = rendered_rs_modules["bulk_memory_ops.rs"]
    rendered_mod_rs = rendered_rs_modules["mod.rs"]
    rendered_py = gen.render_py(data)
    assert "WasmBulkMemoryInstruction" in rendered_bulk_rs
    assert "WasmBulkMemoryOpSpec" in rendered_bulk_rs
    assert '"memory_copy" => Some(WasmBulkMemoryOpSpec' in rendered_bulk_rs
    assert "WasmBulkMemoryInstruction::Copy" in rendered_bulk_rs
    assert '"memory_fill" => Some(WasmBulkMemoryOpSpec' in rendered_bulk_rs
    assert "WasmBulkMemoryInstruction::Fill" in rendered_bulk_rs
    assert "mod bulk_memory_ops;" in rendered_mod_rs
    assert "wasm_bulk_memory_op" in rendered_mod_rs
    assert "WASM_BULK_MEMORY_OPS" in rendered_py

    local_emitter = (
        ROOT
        / "runtime/molt-backend-wasm/src/wasm/op_loop/runtime_service_ops/linear_memory_ops.rs"
    ).read_text(encoding="utf-8")
    assert '"memory_copy" =>' not in local_emitter
    assert '"memory_fill" =>' not in local_emitter
    assert "wasm_bulk_memory_op(op.kind.as_str())" in local_emitter

    broken_duplicate = copy.deepcopy(data)
    broken_duplicate["wasm_bulk_memory_op"].append(
        copy.deepcopy(broken_duplicate["wasm_bulk_memory_op"][0])
    )
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate wasm_bulk_memory_op"
    ):
        manifest.validate_loaded_manifest(broken_duplicate)

    broken_instruction = copy.deepcopy(data)
    broken_instruction["wasm_bulk_memory_op"][0]["instruction"] = "memory_grow"
    with pytest.raises(manifest.WasmAbiManifestError, match="invalid instruction"):
        manifest.validate_loaded_manifest(broken_instruction)

    broken_arg_count = copy.deepcopy(data)
    broken_arg_count["wasm_bulk_memory_op"][0]["arg_count"] = 2
    with pytest.raises(manifest.WasmAbiManifestError, match="arg_count = 3"):
        manifest.validate_loaded_manifest(broken_arg_count)


def test_wasm_abi_manifest_owns_const_op_policy() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    policies = {entry["kind"]: entry for entry in data["const_op_policy"]}

    assert policies["const"] == {
        "kind": "const",
        "inline_seed": "int",
        "literal_payload": "none",
        "scalar_payload": "int",
        "raw_int_effect": "set_int",
        "lir_fast": "lower",
        "parse_scalar_literal": False,
        "dispatch_runtime_seed": False,
    }
    assert policies["const_bool"]["scalar_payload"] == "bool"
    assert policies["const_float"]["scalar_payload"] == "float"
    assert policies["const_none"]["scalar_payload"] == "none"
    assert policies["const_str"]["materializer_import"] == "string_from_bytes"
    assert policies["const_str"]["literal_payload"] == "string"
    assert policies["const_str"]["scalar_payload"] == "none"
    assert policies["const_str"]["parse_scalar_literal"] is True
    assert policies["const_str"]["lir_fast"] == "materialize"
    assert policies["const_bytes"]["materializer_import"] == "bytes_from_bytes"
    assert policies["const_bytes"]["literal_payload"] == "bytes"
    assert policies["const_bytes"]["parse_scalar_literal"] is True
    assert policies["const_bytes"]["lir_fast"] == "materialize"
    assert policies["const_bigint"]["materializer_import"] == "bigint_from_str"
    assert policies["const_bigint"]["literal_payload"] == "bigint_decimal"
    assert policies["const_bigint"]["parse_scalar_literal"] is False
    assert policies["const_bigint"]["lir_fast"] == "materialize"

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "WASM_CONST_OP_POLICIES" in rendered_rs
    assert "WasmConstScalarPayload::Int" in rendered_rs
    assert "required_tir_scalar_value" in rendered_rs
    assert "WasmConstLiteralPayload::BigintDecimal" in rendered_rs
    assert "wasm_const_op_policy" in rendered_rs
    assert "wasm_const_op_policy_for_opcode" in rendered_rs
    assert "opcode_canonical_kind_table(opcode)" in rendered_rs
    assert "PlaceholderZero" not in rendered_rs
    assert "WASM_CONST_OP_POLICIES" in rendered_py


def test_wasm_abi_manifest_keeps_runtime_surface_metadata_without_import_matchers() -> (
    None
):
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "runtime_required_import_prefix" not in data
    assert "runtime_required_import_singleton" not in data
    assert "runtime_surface_requires_direct_import" not in rendered_rs
    assert "WASM_REQUIRED_RUNTIME_IMPORT_PREFIXES" not in rendered_py
    assert "runtime_surface_requires_direct_import" not in rendered_py

    host_surface = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/module_abi/host_surface.rs"
    ).read_text(encoding="utf-8")
    assert "GPU_INTRINSIC_MANIFEST_NAMES" not in host_surface
    assert "DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES" not in host_surface


def test_wasm_abi_manifest_owns_split_runtime_table_prefix() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    poll_slots = {
        entry["name"]: entry["poll_table_slot"]
        for entry in data["import"]
        if "poll_table_slot" in entry
    }
    assert poll_slots["async_sleep_poll"] == 1
    assert poll_slots["contextlib_async_exitstack_enter_context_poll"] == 32
    assert sorted(poll_slots.values()) == list(range(1, len(poll_slots) + 1))
    broken = copy.deepcopy(data)
    for entry in broken["import"]:
        if entry.get("name") == "contextlib_async_exitstack_enter_context_poll":
            entry["poll_table_slot"] = 34
            break
    with pytest.raises(manifest.WasmAbiManifestError, match="poll_table_slot values"):
        manifest.validate_loaded_manifest(broken)

    reserved = data["reserved_runtime_callable"]
    assert reserved[0] == {
        "index": 0,
        "runtime_name": "molt_type_call",
        "import_name": "type_call",
        "callable_arity": 1,
    }
    assert reserved[-2]["runtime_name"] == "molt_types_new_class"
    assert reserved[-1] == {
        "index": 22,
        "runtime_name": "molt_cpython_abi_cext_call_trampoline",
        "import_name": "cpython_abi_cext_call_trampoline",
        "callable_arity": 3,
        "trampoline_abi": "call_frame",
    }
    assert [entry["index"] for entry in reserved] == list(range(len(reserved)))

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    rendered_table_layout = gen.render_table_layout_inc(data)
    assert "PollTableImportSpec" in rendered_rs
    assert "POLL_TABLE_IMPORTS" in rendered_rs
    assert "import: WasmRuntimeImport::AsyncSleepPoll" in rendered_rs
    assert "pub(crate) const fn poll_table_import_slot" in rendered_rs
    assert (
        "WasmRuntimeImport::ContextlibAsyncExitstackEnterContextPoll => Some(32)"
        in rendered_rs
    )
    assert "POLL_TABLE_FUNCS" not in rendered_rs
    assert "WASM_POLL_TABLE_IMPORTS: tuple[tuple[int, str], ...]" in rendered_py
    assert '(32, "contextlib_async_exitstack_enter_context_poll")' in rendered_py
    assert "WASM_DEFAULT_APP_TABLE_BASE" in rendered_py
    assert data["callable_table_publication"] == {
        "section_name": "molt.callable_table",
        "version": 1,
        "layout_section_name": "molt.callable_table.layout",
        "layout_version": 1,
        "active_element_role": 0,
        "value_type_format": 1,
    }
    for name in (
        "WASM_CALLABLE_TABLE_SECTION_NAME",
        "WASM_CALLABLE_TABLE_SECTION_VERSION",
        "WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME",
        "WASM_CALLABLE_TABLE_LAYOUT_VERSION",
        "WASM_CALLABLE_TABLE_ACTIVE_ELEMENT_ROLE",
        "WASM_CALLABLE_TABLE_VALUE_TYPE_FORMAT",
    ):
        assert name in rendered_py
    assert "WASM_TABLE_REF_EXPORT_PREFIX" not in rendered_py
    assert "WASM_TABLE_REF_EXPORT_PREFIX" not in rendered_table_layout
    assert "WASM_RESERVED_RUNTIME_CALLABLE_BASE" in rendered_py

    callable_layout = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/module_abi/callable_table/layout.rs"
    ).read_text(encoding="utf-8")
    assert (
        "poll_table.seed_function_table_slots(&mut func_to_table_idx)"
        in callable_layout
    )
    assert "for spec in RESERVED_RUNTIME_CALLABLE_SPECS" in callable_layout
    assert "table_base\n                .checked_add(slot - fixed_prefix_len)" in callable_layout
    assert "WasmCallableTableAddress::FixedSharedRuntimeAbi" in callable_layout


def test_wasm_abi_manifest_owns_host_import_policy() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    allowed = [entry["name"] for entry in data["link_allowed_import"]]
    allowed_classes = {
        entry["name"]: entry["primitive_class"] for entry in data["link_allowed_import"]
    }
    external_native_classes = {
        entry["name"]: entry["primitive_class"]
        for entry in data["external_native_link_import"]
    }
    call_indirect = [name for name in allowed if name.startswith("molt_call_indirect")]
    assert "fd_write" in allowed
    assert "__indirect_function_table" in allowed
    assert allowed_classes["fd_write"] == "wasi_link_import"
    assert allowed_classes["__indirect_function_table"] == (
        "wasm_toolchain_link_import"
    )
    assert call_indirect == [f"molt_call_indirect{arity}" for arity in range(14)]
    assert all(
        allowed_classes[name] == "molt_indirect_call_import" for name in call_indirect
    )
    assert "molt_cbor_parse_scalar" in allowed
    assert allowed_classes["molt_cbor_parse_scalar"] == "molt_runtime_host_import"
    assert external_native_classes["__cpp_exception"] == ("wasm_toolchain_link_import")
    assert external_native_classes["molt_cpython_abi_date_from_date"] == (
        "molt_cpython_abi_link_import"
    )
    generated_cpython_abi_imports = set(
        manifest.generator_cpython_abi_link_import_names()
    )
    assert {
        "PyArg_ParseTuple",
        "PyFloat_Check",
        "PyLong_Check",
        "PyBool_Check",
        "PyObject_Init",
        "PyObject_InitVar",
        "_PyObject_New",
        "PyMemoryView_FromMemory",
        "PyBuffer_FillInfo",
        "PyErr_SetString",
        "PyExc_ValueError",
        "PyExc_TypeError",
        "Py_None",
        "molt_cpython_abi_date_from_date",
    } <= generated_cpython_abi_imports
    assert {
        name
        for name, primitive_class in external_native_classes.items()
        if primitive_class == "molt_cpython_abi_link_import"
    } == generated_cpython_abi_imports
    provider_classes = {
        "wasm_libc_link_import",
        "wasm_compiler_rt_link_import",
        "wasm_libcxx_link_import",
    }
    assert not (provider_classes & set(external_native_classes.values()))
    assert len(allowed) == len(set(allowed))
    broken = copy.deepcopy(data)
    broken["link_allowed_import"] = [
        entry
        for entry in broken["link_allowed_import"]
        if entry.get("name") != "molt_call_indirect7"
    ]
    with pytest.raises(manifest.WasmAbiManifestError, match="call_indirect import"):
        manifest.validate_loaded_manifest(broken)

    strip_rules = {
        (entry["module"], entry["name"]): entry for entry in data["strip_import_rule"]
    }
    assert strip_rules[("wasi_snapshot_preview1", "fd_write")]["category"] == (
        "io_stdout"
    )
    assert strip_rules[("env", "molt_socket_connect_host")]["category"] == "socket"
    assert strip_rules[("env", "__indirect_function_table")]["category"] == "table"

    prefix_rules = {
        (entry["module"], entry["prefix"]): entry
        for entry in data["strip_import_prefix_rule"]
    }
    assert prefix_rules[("env", "molt_call_indirect")]["category"] == ("indirect_call")

    rendered_py = gen.render_py(data)
    rendered_js_abi = json.loads(gen.render_js_abi(data))
    rendered_rs = _rendered_rs(gen, data)
    assert "CallIndirectImportSpec" in rendered_rs
    assert "CALL_INDIRECT_IMPORTS" in rendered_rs
    assert "CALL_INDIRECT_MAX_ARITY" in rendered_rs
    assert "WASM_LINK_ALLOWED_IMPORTS" in rendered_py
    assert "WASM_LINK_ALLOWED_IMPORT_PRIMITIVE_CLASSES" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORTS" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES" in rendered_py
    assert "WASM_CALL_INDIRECT_IMPORTS" in rendered_py
    assert "WASM_STRIP_IMPORT_RULES" in rendered_py
    assert "WASM_STRIP_IMPORT_PREFIX_RULES" in rendered_py
    js_link_imports = rendered_js_abi["external_native_link_imports"]
    assert not (
        provider_classes & set(js_link_imports["primitive_classes"].values())
    )

    allowlist = gen.OUT_ALLOWED_IMPORTS.read_text(encoding="utf-8")
    assert allowlist == gen.render_allowed_imports(data)
    assert "# DO NOT EDIT BY HAND." in allowlist

    strip_tool = (ROOT / "tools/wasm_strip_unused.py").read_text(encoding="utf-8")
    assert "WASM_STRIP_IMPORT_RULES" in strip_tool
    assert "WASM_STRIP_IMPORT_PREFIX_RULES" in strip_tool
    assert "molt_process_write_host" not in strip_tool


def test_wasm_abi_manifest_owns_link_export_policy() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    policy = data["output_export_policy"]

    assert policy["alias_prefix"] == "__molt_export_alias__"
    assert {
        "memory",
        "molt_memory",
        "molt_main",
        "molt_table",
        "__indirect_function_table",
    } <= set(policy["essential_exports"])
    assert policy["runtime_export_aliases"] == [
        "molt_isolate_bootstrap",
        "molt_isolate_import",
    ]
    assert {"genexpr_", "listcomp_", "lambda_"} <= set(
        policy["internal_output_export_prefixes"]
    )

    rendered_py = gen.render_py(data)
    assert "WASM_OUTPUT_EXPORT_ALIAS_PREFIX" in rendered_py
    assert "WASM_OUTPUT_RUNTIME_EXPORT_ALIASES" in rendered_py
    assert "WASM_INTERNAL_OUTPUT_EXPORT_PREFIXES" in rendered_py
    assert "WASM_ESSENTIAL_EXPORTS" in rendered_py

    link_format = (ROOT / "tools/wasm_link_format.py").read_text(encoding="utf-8")
    assert "_WASM_ABI.WASM_OUTPUT_EXPORT_ALIAS_PREFIX" in link_format
    assert "_WASM_ABI.WASM_OUTPUT_RUNTIME_EXPORT_ALIASES" in link_format
    assert "_WASM_ABI.WASM_INTERNAL_OUTPUT_EXPORT_PREFIXES" in link_format
    assert "_WASM_ABI.WASM_ESSENTIAL_EXPORTS" in link_format
    assert "_WASM_ABI.wasm_runtime_export_name" in link_format
    assert '"molt_alloc"' not in link_format
    assert '"molt_isolate_import"' not in link_format
