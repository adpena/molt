from __future__ import annotations

from dataclasses import fields

from molt import cli


def test_prepared_frontend_pipeline_exposes_only_runtime_handoffs() -> None:
    assert [field.name for field in fields(cli._PreparedFrontendPipeline)] == [
        "prepared_frontend_run_ticket",
        "prepared_frontend_backend_handoff",
    ]
