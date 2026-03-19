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


def test_build_driver_state_transport_removed() -> None:
    assert not hasattr(cli, "_PreparedBuildDriverState")


def test_backend_build_context_alias_removed() -> None:
    assert not hasattr(cli, "_PreparedBackendBuildContext")


def test_backend_pipeline_transport_removed() -> None:
    assert not hasattr(cli, "_PreparedBackendPipeline")


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
