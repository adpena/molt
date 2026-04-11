from molt._wasm_runtime_exports import (
    wasm_runtime_export_link_args,
    wasm_runtime_import_names,
    wasm_runtime_required_import_names,
)
from molt.cli import _backend_codegen_env_digest


def test_wasm_runtime_import_names_include_ssl_and_set_surface() -> None:
    names = set(wasm_runtime_import_names())
    assert "set_update" in names
    assert "frozenset_add" in names


def test_wasm_runtime_export_link_args_prefixes_import_registry() -> None:
    flags = wasm_runtime_export_link_args()
    assert " -C link-arg=--export-if-defined=molt_set_update" in flags


def test_wasm_runtime_export_link_args_adds_stdlib_intrinsics() -> None:
    flags = wasm_runtime_export_link_args(resolved_modules={"ssl"})
    assert " -C link-arg=--export-if-defined=molt_ssl_cert_none" in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_context_new" in flags


def test_wasm_runtime_export_link_args_adds_required_runtime_imports() -> None:
    flags = wasm_runtime_export_link_args({"ssl_cert_none", "ssl_context_new"})
    assert " -C link-arg=--export-if-defined=molt_ssl_cert_none" in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_context_new" in flags
    assert " -C link-arg=--export-if-defined=molt_set_update" not in flags


def test_wasm_runtime_export_link_args_does_not_widen_required_imports_with_resolved_modules() -> None:
    flags = wasm_runtime_export_link_args(
        {"runtime_init"},
        resolved_modules={"ssl"},
    )
    assert " -C link-arg=--export-if-defined=molt_runtime_init" in flags
    assert " -C link-arg=--export-if-defined=molt_runtime_shutdown" in flags
    assert " -C link-arg=--export-if-defined=molt_set_wasm_table_base" in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_cert_none" not in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_context_new" not in flags


def test_wasm_runtime_export_link_args_keeps_host_runtime_exports_in_minimal_mode() -> None:
    flags = wasm_runtime_export_link_args({"runtime_init"})
    assert " -C link-arg=--export-if-defined=molt_runtime_init" in flags
    assert " -C link-arg=--export-if-defined=molt_runtime_shutdown" in flags
    assert " -C link-arg=--export-if-defined=molt_set_wasm_table_base" in flags


def test_wasm_runtime_export_link_args_expands_browser_runtime_fallback_exports() -> None:
    flags = wasm_runtime_export_link_args(
        {
            "dict_getitem",
            "dict_setitem",
            "tuple_getitem",
            "fast_dict_get",
            "fast_list_append",
            "fast_str_join",
            "resource_on_allocate",
            "resource_on_free",
        }
    )
    assert " -C link-arg=--export-if-defined=molt_dict_getitem_borrowed" in flags
    assert " -C link-arg=--export-if-defined=molt_dict_set" in flags
    assert " -C link-arg=--export-if-defined=molt_tuple_getitem_borrowed" in flags
    assert " -C link-arg=--export-if-defined=molt_call_bind_ic" in flags
    assert " -C link-arg=--export-if-defined=molt_callargs_new" in flags
    assert " -C link-arg=--export-if-defined=molt_callargs_push_pos" in flags


def test_wasm_runtime_required_import_names_reads_stdlib_intrinsics() -> None:
    names = set(wasm_runtime_required_import_names({"os", "ssl"}))
    assert "os_name" in names
    assert "ssl_cert_none" not in names


def test_wasm_runtime_required_import_names_include_time_capabilities() -> None:
    names = set(wasm_runtime_required_import_names({"time"}))
    assert "time_time" in names
    assert "capabilities_has" in names
    assert "capabilities_trusted" in names


def test_wasm_runtime_required_import_names_canonicalize_intrinsic_aliases() -> None:
    names = set(wasm_runtime_required_import_names({"asyncio"}))
    assert "async_sleep_new" in names
    assert "async_sleep" not in names


def test_wasm_codegen_env_digest_tracks_required_import_set() -> None:
    base_env = {
        "MOLT_WASM_DATA_BASE": "1048576",
        "MOLT_WASM_TABLE_BASE": "4096",
    }
    extra_env = {
        **base_env,
        "MOLT_WASM_EXTRA_REQUIRED_IMPORTS": "os_name,ssl_cert_none",
    }

    assert _backend_codegen_env_digest(
        is_wasm=True, env=base_env
    ) != _backend_codegen_env_digest(is_wasm=True, env=extra_env)


def test_wasm_codegen_env_digest_tracks_split_runtime_table_min() -> None:
    base_env = {
        "MOLT_WASM_DATA_BASE": "1048576",
        "MOLT_WASM_TABLE_BASE": "4096",
    }
    split_env = {
        **base_env,
        "MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN": "8192",
    }

    assert _backend_codegen_env_digest(
        is_wasm=True, env=base_env
    ) != _backend_codegen_env_digest(is_wasm=True, env=split_env)
