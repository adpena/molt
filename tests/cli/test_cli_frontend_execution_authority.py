from __future__ import annotations

import inspect
from pathlib import Path

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli import frontend_worker
from molt.cli.models import (
    _FrontendModuleResultTimings,
    _FrontendParallelLayerState,
)

_FRONTEND_EXECUTION_NAMES = (
    "_consume_frontend_module_result",
    "_consume_frontend_parallel_layer_result",
    "_consume_frontend_serial_layer_result",
    "_lower_entry_module_as_main",
    "_prepare_frontend_execution",
    "_run_frontend_layer",
    "_run_frontend_parallel_enabled_layers",
    "_run_frontend_parallel_layer_batches",
    "_run_frontend_pipeline",
    "_run_frontend_serial_disabled_layers",
    "_run_frontend_serial_layer_modules",
    "_write_parallel_persisted_module_lowering",
)


def test_cli_frontend_execution_authority_is_single_home() -> None:
    for name in _FRONTEND_EXECUTION_NAMES:
        assert hasattr(frontend_execution, name)
        assert not hasattr(cli, name)
        assert not hasattr(frontend_worker, name)

    cli_source = inspect.getsource(cli)
    frontend_worker_source = inspect.getsource(frontend_worker)
    for name in _FRONTEND_EXECUTION_NAMES:
        assert f"def {name}(" not in cli_source
        assert f"def {name}(" not in frontend_worker_source


def test_serial_layer_result_records_cache_hits_as_cache_hits(tmp_path: Path) -> None:
    recorded: list[dict[str, object]] = []

    def record_worker_timing(**kwargs: object) -> dict[str, object]:
        recorded.append(dict(kwargs))
        return recorded[-1]

    error = frontend_execution._consume_frontend_serial_layer_result(
        record_frontend_parallel_worker_timing=record_worker_timing,
        integrate_module_frontend_result=lambda *args, **kwargs: None,
        accumulate_midend_diagnostics=lambda *args, **kwargs: None,
        fail=lambda *args, **kwargs: None,
        json_output=True,
        layer_state=_FrontendParallelLayerState(),
        layer_index=0,
        module_name="m",
        module_path=tmp_path / "m.py",
        result={
            "functions": [],
            "func_code_ids": {},
            "local_class_names": [],
            "local_classes": {},
        },
        result_timings=_FrontendModuleResultTimings(
            visit_s=0.0,
            lower_s=0.0,
            total_s=0.0,
            cache_hit=True,
            reused_s=1.25,
        ),
        serial_mode="serial_disabled",
    )

    assert error is None
    assert recorded == [
        {
            "layer_index": 0,
            "module_name": "m",
            "module_path": tmp_path / "m.py",
            "mode": "serial_cache_hit",
            "queue_ms": 0.0,
            "wait_ms": 0.0,
            "exec_ms": 0.0,
            "roundtrip_ms": 0.0,
            "reused_ms": 1250.0,
            "worker_pid": None,
        }
    ]
