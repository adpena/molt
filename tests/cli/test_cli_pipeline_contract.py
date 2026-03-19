from __future__ import annotations

from dataclasses import fields

from molt import cli


def test_frontend_run_ticket_owns_runtime_execution_surfaces() -> None:
    assert [field.name for field in fields(cli._PreparedFrontendRunTicket)] == [
        "module_order",
        "module_layers",
        "frontend_parallel_config",
        "frontend_parallel_layers",
        "frontend_parallel_worker_timings",
        "frontend_parallel_details",
        "frontend_layer_execution_context",
        "frontend_layer_runtime_hooks",
    ]


def test_build_driver_state_no_longer_carries_frontend_runtime_detail_sideband() -> None:
    assert [field.name for field in fields(cli._PreparedBuildDriverState)] == [
        "prepared_frontend_run_ticket",
        "prepared_backend_build_context",
    ]


def test_backend_build_context_owns_backend_finalize_surfaces() -> None:
    field_names = [field.name for field in fields(cli._PreparedBackendBuildContext)]
    for name in (
        "output_layout",
        "artifacts_root",
        "require_linked",
        "link_timeout",
        "module_graph",
        "stdlib_allowlist",
        "spawn_enabled",
        "known_modules",
        "generated_module_source_paths",
        "known_func_defaults",
        "module_order",
        "type_facts",
        "known_classes",
        "enable_phi",
        "module_chunk_max_ops",
        "module_chunking",
        "integration_state",
        "diagnostics_state",
        "record_frontend_timing",
        "build_diagnostics_payload",
    ):
        assert name in field_names


def test_backend_pipeline_owns_result_emission_surfaces() -> None:
    field_names = [field.name for field in fields(cli._PreparedBackendPipeline)]
    for name in (
        "runtime_lib",
        "runtime_reloc_wasm",
        "ensure_runtime_wasm_reloc",
        "cache",
        "cache_hit",
        "cache_key",
        "function_cache_key",
        "cache_path",
        "function_cache_path",
        "cache_hit_tier",
        "backend_daemon_cached",
        "backend_daemon_cache_tier",
        "backend_daemon_config_digest",
        "target",
        "target_triple",
        "source_path",
        "deterministic",
        "trusted",
        "capabilities_list",
        "capability_profiles",
        "capabilities_source",
        "sysroot_path",
        "emit_mode",
        "profile",
        "native_arch_perf_enabled",
        "pgo_profile_payload",
        "runtime_feedback_payload",
        "emit_ir_path",
        "warnings",
        "json_output",
        "resolved_diagnostics_verbosity",
        "diagnostics_payload",
        "diagnostics_path",
    ):
        assert name in field_names


def test_frontend_pipeline_transport_removed() -> None:
    assert not hasattr(cli, "_PreparedFrontendPipeline")


def test_frontend_backend_handoff_transport_removed() -> None:
    assert not hasattr(cli, "_PreparedFrontendBackendHandoff")


def test_frontend_execution_seed_removed() -> None:
    assert not hasattr(cli, "_PreparedFrontendExecutionSeed")


def test_frontend_internal_stage_transport_removed() -> None:
    assert not hasattr(cli, "_PreparedFrontendStageState")


def test_frontend_internal_execution_transport_removed() -> None:
    assert not hasattr(cli, "_PreparedFrontendExecutionContext")


def test_frontend_stage_context_transport_removed() -> None:
    assert not hasattr(cli, "_PreparedFrontendStageContext")


def test_build_result_emission_context_removed() -> None:
    assert not hasattr(cli, "_BuildResultEmissionContext")
