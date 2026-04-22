from molt._wasm_runtime_exports import (
    wasm_runtime_dynamic_export_names,
    wasm_runtime_export_link_args,
    wasm_runtime_import_names,
    wasm_runtime_required_import_names,
)
from molt.cli import _backend_codegen_env_digest


RUNTIME_OWNED_GPU_MODULES = {
    "molt.gpu",
    "molt.gpu.tensor",
    "molt.gpu.kv_cache",
    "molt.gpu.interop",
}

RUNTIME_OWNED_GPU_EXPORTS = {
    "molt_gpu_kernel_launch",
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

TINYGRAD_STDLIB_MODULES = {
    "tinygrad.device",
    "tinygrad.lazy",
    "tinygrad.realize",
    "tinygrad.dtypes",
    "tinygrad.tensor",
    "tinygrad.tokenizer",
    "tinygrad.onnx_interpreter",
    "tinygrad.paddleocr",
    "tinygrad.paddleocr_driver",
    "tinygrad.model_config",
    "tinygrad.wasm_driver",
    "tinygrad.examples",
    "tinygrad.examples.falcon_ocr",
}

TINYGRAD_STDLIB_GPU_EXPORTS = {
    "molt_gpu_prim_binary",
    "molt_gpu_prim_create_tensor",
    "molt_gpu_prim_device",
    "molt_gpu_prim_realize",
    "molt_gpu_prim_reduce",
    "molt_gpu_prim_unary",
    "molt_gpu_prim_zeros",
    "molt_gpu_rope_apply_contiguous",
}


def test_wasm_runtime_import_names_include_ssl_and_set_surface() -> None:
    names = set(wasm_runtime_import_names())
    assert "set_update" in names
    assert "frozenset_add" in names


def test_wasm_runtime_export_link_args_prefixes_import_registry() -> None:
    flags = wasm_runtime_export_link_args()
    assert " -C link-arg=--export-if-defined=molt_set_update" in flags
    assert " -C link-arg=--export-if-defined=molt_gpu_matmul_contiguous" in flags


def test_wasm_runtime_export_link_args_adds_stdlib_intrinsics() -> None:
    flags = wasm_runtime_export_link_args(resolved_modules={"ssl"})
    assert " -C link-arg=--export-if-defined=molt_ssl_cert_none" in flags
    assert " -C link-arg=--export-if-defined=molt_ssl_context_new" in flags


def test_wasm_runtime_export_link_args_adds_tinygrad_stdlib_gpu_intrinsics() -> None:
    flags = wasm_runtime_export_link_args(resolved_modules=TINYGRAD_STDLIB_MODULES)

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
        " -C link-arg=--export-if-defined="
        "molt_gpu_turboquant_attention_packed" in flags
    )


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


def test_wasm_runtime_export_link_args_keeps_runtime_owned_dynamic_intrinsics_in_minimal_mode() -> None:
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


def test_wasm_runtime_dynamic_export_names_reports_runtime_owned_gpu_intrinsics() -> None:
    names = set(wasm_runtime_dynamic_export_names({"molt.gpu.tensor", "molt.gpu.kv_cache"}))
    assert "molt_gpu_linear_contiguous" in names
    assert "molt_gpu_tensor__tensor_scaled_dot_product_attention" in names
    assert "molt_gpu_turboquant_attention_packed" in names


def test_wasm_runtime_dynamic_export_names_cover_all_runtime_owned_gpu_intrinsics() -> None:
    names = set(wasm_runtime_dynamic_export_names(RUNTIME_OWNED_GPU_MODULES))

    assert names == RUNTIME_OWNED_GPU_EXPORTS


def test_wasm_runtime_export_link_args_cover_all_runtime_owned_gpu_intrinsics() -> None:
    flags = wasm_runtime_export_link_args(
        {"runtime_init"},
        resolved_modules=RUNTIME_OWNED_GPU_MODULES,
    )

    for name in sorted(RUNTIME_OWNED_GPU_EXPORTS):
        assert f" -C link-arg=--export-if-defined={name}" in flags


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
