from pathlib import Path

from molt._wasm_runtime_exports import (
    wasm_runtime_dynamic_export_names,
    wasm_runtime_export_link_args,
    wasm_runtime_import_names,
)
from molt._wasm_abi_generated import (
    wasm_runtime_export_name,
    wasm_runtime_import_name,
)
from molt._intrinsic_symbols import intrinsic_runtime_symbol_name
from molt.cli.backend_execution import _backend_codegen_env_digest
from molt.cli.required_features import reached_intrinsic_symbols


ROOT = Path(__file__).resolve().parents[2]


RUNTIME_OWNED_GPU_MODULES = {
    "molt.gpu",
    "molt.gpu.tensor",
    "molt.gpu.kv_cache",
    "molt.gpu.interop",
}

RUNTIME_OWNED_GPU_EXPORTS = {
    "molt_gpu_kernel_launch",
    "molt_gpu_buffer_to_list",
    "molt_gpu_tensor_from_parts",
    "molt_gpu_linear_contiguous",
    "molt_gpu_linear_split_last_dim_contiguous",
    "molt_gpu_tensor__tensor_linear_split_last_dim",
    "molt_gpu_linear_squared_relu_gate_interleaved_contiguous",
    "molt_gpu_permute_contiguous",
    "molt_gpu_softmax_last_axis_contiguous",
    "molt_gpu_tensor__tensor_concat_first_dim",
    "molt_gpu_tensor__tensor_scatter_rows",
    "molt_gpu_broadcast_binary_contiguous",
    "molt_gpu_matmul_contiguous",
    "molt_gpu_repeat_axis_contiguous",
    "molt_gpu_rms_norm_last_axis_contiguous",
    "molt_gpu_squared_relu_gate_interleaved_contiguous",
    "molt_gpu_tensor__tensor_scaled_dot_product_attention",
    "molt_gpu_tensor__zeros",
    "molt_gpu_turboquant_attention_packed",
    "molt_gpu_interop_decode_f16_bytes_to_f32",
    "molt_gpu_interop_decode_bf16_bytes_to_f32",
}

TINYGRAD_STDLIB_GPU_EXPORTS = {
    "molt_gpu_prim_binary",
    "molt_gpu_prim_create_tensor",
    "molt_gpu_prim_create_tensor_raw",
    "molt_gpu_prim_device",
    "molt_gpu_prim_dtype",
    "molt_gpu_prim_free",
    "molt_gpu_prim_nbytes",
    "molt_gpu_prim_numel",
    "molt_gpu_prim_read_data_raw",
    "molt_gpu_prim_realize",
    "molt_gpu_prim_reduce",
    "molt_gpu_prim_shape",
    "molt_gpu_prim_cast",
    "molt_gpu_prim_ternary",
    "molt_gpu_prim_unary",
    "molt_gpu_prim_zeros",
    "molt_gpu_prim_zeros_dtype",
    "molt_gpu_rope_apply_contiguous",
}


def _builtin_func(symbol: str) -> dict[str, object]:
    return {"kind": "builtin_func", "s_value": symbol, "value": 0, "out": "v0"}


def _const_str(symbol: str) -> dict[str, object]:
    return {"kind": "const_str", "s_value": symbol, "out": "v0"}


def _call(symbol: str) -> dict[str, object]:
    return {"kind": "call", "s_value": symbol, "args": [], "out": "v0"}


def _function(name: str, ops: list[dict[str, object]]) -> dict[str, object]:
    return {"name": name, "params": [], "ops": ops}


def _reached_runtime_import_names_for(
    functions: list[dict[str, object]],
) -> set[str]:
    names: set[str] = set()
    for symbol in reached_intrinsic_symbols(functions):
        import_name = wasm_runtime_import_name(intrinsic_runtime_symbol_name(symbol))
        if import_name is not None:
            names.add(import_name)
    return names


def test_wasm_runtime_import_names_include_ssl_and_set_surface() -> None:
    names = set(wasm_runtime_import_names())
    assert "set_update" in names
    assert "frozenset_add" in names
    assert "ord_at" in names


def test_wasm_runtime_export_link_args_prefixes_import_registry() -> None:
    flags = wasm_runtime_export_link_args()
    assert " -C link-arg=--export-if-defined=molt_set_update" in flags
    assert " -C link-arg=--export-if-defined=molt_ord_at" in flags
    assert " -C link-arg=--export-if-defined=molt_gpu_matmul_contiguous" in flags
    assert (
        " -C link-arg=--export-if-defined="
        "molt_gpu_tensor__tensor_scaled_dot_product_attention" in flags
    )


def test_wasm_runtime_export_names_are_generated() -> None:
    assert wasm_runtime_import_name("socket_drop") == "socket_drop"
    assert wasm_runtime_import_name("molt_socket_drop") == "socket_drop"
    assert wasm_runtime_import_name("molt_alloc") == "alloc"
    assert wasm_runtime_export_name("socket_drop") == "molt_socket_drop"
    assert wasm_runtime_export_name("molt_socket_drop") == "molt_socket_drop"
    assert wasm_runtime_export_name("molt_alloc") == "molt_alloc"


def test_wasm_runtime_exports_use_generated_intrinsic_symbols_and_ast_scan() -> None:
    text = (ROOT / "src/molt/_wasm_runtime_exports.py").read_text(encoding="utf-8")
    assert "_intrinsic_symbols" in text
    assert "ast.parse" in text
    assert "_INTRINSIC_SYMBOL_RE" not in text
    assert "generated.rs" not in text
    assert "re.compile" not in text


def test_wasm_runtime_export_link_args_does_not_widen_full_runtime_with_stdlib_modules() -> (
    None
):
    json_flags = wasm_runtime_export_link_args(resolved_modules={"json"})
    ssl_flags = wasm_runtime_export_link_args(resolved_modules={"ssl"})
    assert json_flags == ssl_flags


def test_wasm_runtime_export_link_args_include_tinygrad_gpu_intrinsics() -> None:
    flags = wasm_runtime_export_link_args()

    for name in sorted(TINYGRAD_STDLIB_GPU_EXPORTS):
        assert f" -C link-arg=--export-if-defined={name}" in flags


def test_wasm_runtime_export_link_args_adds_runtime_owned_gpu_intrinsics() -> None:
    flags = wasm_runtime_export_link_args(
        resolved_modules={"molt.gpu.tensor", "molt.gpu.kv_cache"}
    )
    assert " -C link-arg=--export-if-defined=molt_gpu_linear_contiguous" in flags
    assert (
        " -C link-arg=--export-if-defined="
        "molt_gpu_linear_split_last_dim_contiguous" in flags
    )
    assert (
        " -C link-arg=--export-if-defined="
        "molt_gpu_tensor__tensor_scaled_dot_product_attention" in flags
    )
    assert (
        " -C link-arg=--export-if-defined=molt_gpu_turboquant_attention_packed" in flags
    )


def test_wasm_runtime_export_link_args_adds_required_runtime_imports() -> None:
    flags = wasm_runtime_export_link_args({"ssl_cert_none", "ssl_context_new"})
    assert " -C link-arg=--export-if-defined=molt_ssl_cert_none" in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_context_new" in flags
    assert " -C link-arg=--export-if-defined=molt_set_update" not in flags


def test_wasm_runtime_export_link_args_does_not_widen_required_imports_with_resolved_modules() -> (
    None
):
    flags = wasm_runtime_export_link_args(
        {"runtime_init"},
        resolved_modules={"ssl"},
    )
    assert " -C link-arg=--export-if-defined=molt_runtime_init" in flags
    assert " -C link-arg=--export-if-defined=molt_runtime_shutdown" in flags
    assert " -C link-arg=--export-if-defined=molt_set_wasm_table_base" in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_cert_none" not in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_context_new" not in flags


def test_wasm_runtime_export_link_args_keeps_runtime_owned_dynamic_intrinsics_in_minimal_mode() -> (
    None
):
    flags = wasm_runtime_export_link_args(
        {"runtime_init"},
        resolved_modules={"molt.gpu.tensor"},
    )
    assert " -C link-arg=--export-if-defined=molt_runtime_init" in flags
    assert " -C link-arg=--export-if-defined=molt_gpu_linear_contiguous" in flags
    assert (
        " -C link-arg=--export-if-defined="
        "molt_gpu_linear_split_last_dim_contiguous" in flags
    )


def test_wasm_runtime_dynamic_export_names_reports_runtime_owned_gpu_intrinsics() -> (
    None
):
    names = set(
        wasm_runtime_dynamic_export_names({"molt.gpu.tensor", "molt.gpu.kv_cache"})
    )
    assert "molt_gpu_linear_contiguous" in names
    assert "molt_gpu_tensor__tensor_scaled_dot_product_attention" in names
    assert "molt_gpu_turboquant_attention_packed" in names


def test_wasm_runtime_dynamic_export_names_cover_all_runtime_owned_gpu_intrinsics() -> (
    None
):
    names = set(wasm_runtime_dynamic_export_names(RUNTIME_OWNED_GPU_MODULES))

    assert names == RUNTIME_OWNED_GPU_EXPORTS


def test_wasm_runtime_export_link_args_cover_all_runtime_owned_gpu_intrinsics() -> None:
    flags = wasm_runtime_export_link_args(
        {"runtime_init"},
        resolved_modules=RUNTIME_OWNED_GPU_MODULES,
    )

    for name in sorted(RUNTIME_OWNED_GPU_EXPORTS):
        assert f" -C link-arg=--export-if-defined={name}" in flags


def test_wasm_runtime_export_link_args_keeps_host_runtime_exports_in_minimal_mode() -> (
    None
):
    flags = wasm_runtime_export_link_args({"runtime_init"})
    assert " -C link-arg=--export-if-defined=molt_runtime_init" in flags
    assert " -C link-arg=--export-if-defined=molt_runtime_shutdown" in flags
    assert " -C link-arg=--export-if-defined=molt_set_wasm_table_base" in flags


def test_wasm_runtime_export_link_args_expands_browser_runtime_fallback_exports() -> (
    None
):
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


def test_reached_runtime_import_names_read_stdlib_intrinsics() -> None:
    import_name = wasm_runtime_import_name(intrinsic_runtime_symbol_name("molt_os_name"))
    assert import_name == "os_name"
    names = {import_name}
    assert "os_name" in names
    assert "ssl_cert_none" not in names


def test_reached_runtime_import_names_keep_codec_numeric_slices_narrow() -> (
    None
):
    codec_names = _reached_runtime_import_names_for(
        [
            _function(
                "molt_main",
                [
                    _builtin_func("molt_codecs_decode"),
                    _builtin_func("molt_codecs_encode"),
                ],
            ),
            _function("dead_codec_helper", [_builtin_func("molt_codecs_charmap_build")]),
        ]
    )
    assert codec_names == {"codecs_decode", "codecs_encode"}

    numeric_names = _reached_runtime_import_names_for(
        [
            _function("molt_main", [_builtin_func("molt_decimal_from_str")]),
            _function("dead_regex", [_builtin_func("molt_re_compile")]),
            _function("dead_struct", [_builtin_func("molt_struct_pack")]),
            _function("dead_warnings", [_builtin_func("molt_warnings_filter")]),
        ]
    )
    leaked = sorted(
        name
        for name in numeric_names
        if name.startswith(("re_", "struct_", "warnings_"))
    )
    assert leaked == []


def test_reached_runtime_import_names_include_time_capabilities() -> None:
    names = _reached_runtime_import_names_for(
        [
            _function(
                "molt_main",
                [_builtin_func("molt_time_time"), _call("molt_time_ensure_caps")],
            ),
            _function(
                "molt_time_ensure_caps",
                [
                    _builtin_func("molt_capabilities_has"),
                    _builtin_func("molt_capabilities_trusted"),
                ],
            ),
        ]
    )
    assert "time_time" in names
    assert "capabilities_has" in names
    assert "capabilities_trusted" in names


def test_reached_runtime_import_names_use_public_async_sleep_symbol() -> None:
    import_name = wasm_runtime_import_name(
        intrinsic_runtime_symbol_name("molt_async_sleep")
    )
    names = {import_name}
    assert "async_sleep" in names
    assert "async_sleep_new" not in names


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
