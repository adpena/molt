from __future__ import annotations

from dataclasses import fields

from molt import cli


def test_prepared_frontend_pipeline_exposes_only_runtime_handoffs() -> None:
    assert [field.name for field in fields(cli._PreparedFrontendPipeline)] == [
        "prepared_frontend_run_ticket",
        "prepared_frontend_backend_handoff",
    ]


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
        "prepared_frontend_pipeline",
        "prepared_backend_build_context",
    ]


def test_backend_build_context_owns_backend_finalize_surfaces() -> None:
    field_names = [field.name for field in fields(cli._PreparedBackendBuildContext)]
    for name in (
        "output_layout",
        "artifacts_root",
        "require_linked",
        "link_timeout",
    ):
        assert name in field_names
