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


def _rendered_rs(gen, data) -> str:
    return "".join(gen.render_rs_modules(data).values())


def test_wasm_abi_generated_files_are_in_sync() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    rendered_rs_modules = gen.render_rs_modules(data)
    assert not gen.LEGACY_OUT_RS.exists()
    assert set(rendered_rs_modules) == set(gen.OUT_RS_FILES)
    for name, rendered in rendered_rs_modules.items():
        assert gen.OUT_RS_FILES[name].read_text(encoding="utf-8") == rendered
    assert gen.OUT_PY.read_text(encoding="utf-8") == gen.render_py(data)
    assert gen.OUT_TABLE_LAYOUT_INC.read_text(encoding="utf-8") == gen.render_table_layout_inc(
        data
    )
    assert gen.OUT_POLL_INC.read_text(encoding="utf-8") == gen.render_poll_inc(data)
    assert gen.OUT_RESERVED_INC.read_text(encoding="utf-8") == gen.render_reserved_inc(data)
    assert gen.OUT_ALLOWED_IMPORTS.read_text(
        encoding="utf-8"
    ) == gen.render_allowed_imports(data)


def test_wasm_abi_manifest_owns_static_type_section() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    static_types = data["static_type"]

    assert len(static_types) == 51
    assert static_types[0] == {"params": [], "results": ["i64"]}
    assert static_types[1] == {"params": ["i64"], "results": []}
    assert static_types[31] == {"params": ["i64", "i64"], "results": ["i64", "i64"]}
    assert static_types[32] == {
        "params": ["i64", "i64", "i64"],
        "results": ["i64", "i64", "i64"],
    }
    assert all(entry["type"] < len(static_types) for entry in data["import"])

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "STATIC_FUNC_TYPES" in rendered_rs
    assert "STATIC_TYPE_COUNT: u32 = 51" in rendered_rs
    assert "WASM_STATIC_TYPES" in rendered_py
    assert "WASM_STATIC_TYPE_COUNT: int = 51" in rendered_py

    wasm_abi = (
        ROOT / "runtime/molt-backend-wasm/src/wasm_abi.rs"
    ).read_text(encoding="utf-8")
    assert "static_func_type(" not in wasm_abi
    assert "const STATIC_FUNC_TYPES" not in wasm_abi


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
    rendered_rs = _rendered_rs(gen, data)
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

    rendered_rs = _rendered_rs(gen, data)
    assert "RUNTIME_CALLABLE_IMPORTS" in rendered_rs
    assert "RuntimeCallableResult::Void" in rendered_rs


def test_wasm_abi_manifest_owns_op_import_deps() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    op_deps = {entry["kind"]: entry["deps"] for entry in data["op_import_dep"]}

    assert "OP_IMPORT_DEPS" in _rendered_rs(gen, data)
    assert "module_cache_del" not in op_deps["__structural__"]
    assert op_deps["module_cache_del"] == ["module_cache_del"]
    assert op_deps["print"] == ["print_obj"]
    assert op_deps["object_new_bound"] == []
    assert op_deps["object_new_bound_stack"] == ["object_new_bound_sized"]


def test_wasm_abi_manifest_owns_runtime_surface_required_import_matchers() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    import_names = {entry["name"] for entry in data["import"]}
    prefixes = {entry["prefix"] for entry in data["runtime_required_import_prefix"]}
    singletons = {entry["name"] for entry in data["runtime_required_import_singleton"]}

    assert {"os_", "path_", "socket_", "math_", "dataclass_"} <= prefixes
    assert {
        "socketpair",
        "spawn",
        "block_on",
        "open_builtin",
        "errno_constants",
    } <= singletons
    assert "os_name" not in singletons
    assert any("os_name".startswith(prefix) for prefix in prefixes)
    assert all(
        any(name.startswith(prefix) for name in import_names)
        for prefix in prefixes
    )
    assert singletons <= import_names

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "REQUIRED_RUNTIME_IMPORT_PREFIXES" in rendered_rs
    assert "REQUIRED_RUNTIME_IMPORT_SINGLETONS" in rendered_rs
    assert "runtime_surface_requires_direct_import" in rendered_rs
    assert "WASM_REQUIRED_RUNTIME_IMPORT_PREFIXES" in rendered_py
    assert "runtime_surface_requires_direct_import" in rendered_py

    runtime_surface = (
        ROOT
        / "runtime/molt-backend-wasm/src/wasm/module_abi/runtime_surface.rs"
    ).read_text(encoding="utf-8")
    assert "REQUIRED_IMPORT_PREFIXES" not in runtime_surface
    assert "REQUIRED_IMPORT_SINGLETONS" not in runtime_surface
    assert "runtime_surface_requires_direct_import(kind)" in runtime_surface


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

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "POLL_TABLE_FUNCS" in rendered_rs
    assert "WASM_LEGACY_TABLE_BASE" in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_BASE" in rendered_py


def test_wasm_abi_manifest_owns_host_import_policy() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()

    allowed = [entry["name"] for entry in data["link_allowed_import"]]
    assert "fd_write" in allowed
    assert "__indirect_function_table" in allowed
    assert "molt_call_indirect0" in allowed
    assert "molt_call_indirect13" in allowed
    assert "molt_cbor_parse_scalar" in allowed
    assert len(allowed) == len(set(allowed))

    strip_rules = {
        (entry["module"], entry["name"]): entry
        for entry in data["strip_import_rule"]
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
    assert prefix_rules[("env", "molt_call_indirect")]["category"] == (
        "indirect_call"
    )

    rendered_py = gen.render_py(data)
    assert "WASM_LINK_ALLOWED_IMPORTS" in rendered_py
    assert "WASM_STRIP_IMPORT_RULES" in rendered_py
    assert "WASM_STRIP_IMPORT_PREFIX_RULES" in rendered_py

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
        "molt_table_init",
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
    assert '"molt_alloc"' not in link_format
    assert '"molt_isolate_import"' not in link_format
