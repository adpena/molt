from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GEN_WASM_ABI = ROOT / "tools" / "gen_wasm_abi.py"


def _load_gen_wasm_abi():
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_wasm_abi", GEN_WASM_ABI
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_wasm_abi_generated_files_are_in_sync() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    assert gen.OUT_RS.read_text(encoding="utf-8") == gen.render_rs(data)
    assert gen.OUT_PY.read_text(encoding="utf-8") == gen.render_py(data)
    assert gen.OUT_TABLE_LAYOUT_INC.read_text(encoding="utf-8") == gen.render_table_layout_inc(
        data
    )
    assert gen.OUT_POLL_INC.read_text(encoding="utf-8") == gen.render_poll_inc(data)
    assert gen.OUT_RESERVED_INC.read_text(encoding="utf-8") == gen.render_reserved_inc(data)


def test_wasm_abi_manifest_feeds_runtime_export_registry() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    manifest_names = {entry["name"] for entry in data["import"]}

    runtime_exports_path = ROOT / "src/molt/_wasm_runtime_exports.py"
    text = runtime_exports_path.read_text(encoding="utf-8")
    assert "wasm_imports.rs" not in text
    assert "WASM_IMPORT_REGISTRY" in text
    assert {"alloc", "runtime_init", "socket_connect", "task_new"} <= manifest_names


def test_wasm_abi_manifest_owns_pure_profile_prefixes() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    prefixes = {entry["prefix"] for entry in data["pure_skip_prefix"]}
    assert {"process_", "socket", "db_", "ws_", "time_"} <= prefixes
    rendered_rs = gen.render_rs(data)
    assert "pure_profile_skips_import" in rendered_rs
    assert "PURE_PROFILE_SKIP_PREFIXES" in rendered_rs


def test_wasm_abi_manifest_owns_runtime_callable_registry() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    imports = {entry["name"]: entry for entry in data["import"]}

    assert imports["importlib_import_transaction"]["type"] == 12
    assert imports["importlib_import_transaction"]["runtime_name"] == (
        "molt_importlib_import_transaction"
    )
    assert imports["importlib_import_transaction"]["callable_arity"] == 5

    assert imports["socket_drop"]["callable_result"] == "void"
    assert imports["stream_close"]["callable_result"] == "void"
    assert imports["future_features"]["runtime_name"] == "molt_future_features"
    assert imports["site_help0"]["callable_arity"] == 0
    assert imports["site_help1"]["callable_arity"] == 1

    rendered_rs = gen.render_rs(data)
    assert "RUNTIME_CALLABLE_IMPORTS" in rendered_rs
    assert "RuntimeCallableResult::Void" in rendered_rs


def test_wasm_abi_manifest_owns_op_import_deps() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    op_deps = {entry["kind"]: entry["deps"] for entry in data["op_import_dep"]}

    assert "OP_IMPORT_DEPS" in gen.render_rs(data)
    assert "module_cache_del" not in op_deps["__structural__"]
    assert op_deps["module_cache_del"] == ["module_cache_del"]
    assert op_deps["object_new_bound"] == []
    assert op_deps["object_new_bound_stack"] == ["object_new_bound_sized"]


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

    reserved = data["reserved_runtime_callable"]
    assert reserved[0] == {
        "index": 0,
        "runtime_name": "molt_type_call",
        "import_name": "type_call",
        "callable_arity": 1,
    }
    assert reserved[-1]["runtime_name"] == "molt_types_new_class"
    assert [entry["index"] for entry in reserved] == list(range(len(reserved)))

    rendered_rs = gen.render_rs(data)
    rendered_py = gen.render_py(data)
    assert "POLL_TABLE_FUNCS" in rendered_rs
    assert "WASM_LEGACY_TABLE_BASE" in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_BASE" in rendered_py
