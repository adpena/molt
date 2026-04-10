from molt._wasm_runtime_exports import (
    wasm_runtime_export_link_args,
    wasm_runtime_import_names,
)


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
