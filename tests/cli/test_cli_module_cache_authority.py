from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_cache

_MODULE_CACHE_NAMES = (
    "_MODULE_ANALYSIS_CACHE_SCHEMA_VERSION",
    "_MODULE_ANALYSIS_FUNC_KINDS",
    "_MODULE_LOWERING_CACHE_SCHEMA_VERSION",
    "_build_scoped_known_classes_snapshot",
    "_build_scoped_lowering_inputs",
    "_collect_func_defaults",
    "_collect_func_kinds",
    "_decode_cached_json_value",
    "_load_cached_module_lowering_result",
    "_load_module_analysis",
    "_module_analysis_cache_path",
    "_module_lowering_cache_path",
    "_module_lowering_context_digest",
    "_module_lowering_context_digest_for_module",
    "_module_lowering_context_payload",
    "_module_lowering_execution_view",
    "_module_lowering_metadata_view",
    "_module_worker_payload",
    "_normalize_backend_ir_functions",
    "_read_persisted_module_analysis",
    "_read_persisted_module_lowering",
    "_scoped_known_classes",
    "_scoped_known_classes_view",
    "_scoped_known_func_defaults",
    "_scoped_known_func_kinds",
    "_scoped_known_modules",
    "_scoped_lowering_input_view",
    "_scoped_pgo_hot_function_names",
    "_scoped_type_facts",
    "_type_facts_cache_payload",
    "_validate_module_func_default_payload",
    "_write_persisted_module_analysis",
    "_write_persisted_module_lowering",
)


def test_cli_module_cache_authority_is_single_home() -> None:
    for name in _MODULE_CACHE_NAMES:
        assert getattr(cli, name) is getattr(module_cache, name)

    cli_source = inspect.getsource(cli)
    for name in _MODULE_CACHE_NAMES:
        if name.startswith("_MODULE_"):
            assert f"{name} =" not in cli_source
        else:
            assert f"def {name}(" not in cli_source
