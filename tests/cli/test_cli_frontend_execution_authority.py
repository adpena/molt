from __future__ import annotations

import inspect
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli import frontend_worker
from molt.cli.models import (
    _FrontendModuleResultTimings,
    _FrontendParallelLayerState,
)
from molt.cli.module_source import _ModuleSourceLease

_FRONTEND_EXECUTION_NAMES = (
    "_consume_frontend_module_result",
    "_consume_frontend_parallel_layer_result",
    "_consume_frontend_serial_layer_result",
    "_prepare_frontend_execution",
    "_run_frontend_layer",
    "_run_frontend_parallel_enabled_layers",
    "_run_frontend_parallel_layer_batches",
    "_run_frontend_pipeline",
    "_run_frontend_serial_disabled_layers",
    "_run_frontend_serial_layer_modules",
    "_validate_frontend_module_source",
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
    path = tmp_path / "m.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    source_lease = _ModuleSourceLease.path_backed(path)
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
        source_lease=source_lease,
        result={
            "source_sha256": source_lease.source_sha256,
            "functions": [],
            "func_code_ids": {},
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


@pytest.mark.parametrize("defect", ["missing_stamp", "wrong_stamp", "wrong_path"])
def test_result_generation_refusal_precedes_integration(
    tmp_path: Path, defect: str
) -> None:
    path = tmp_path / "m.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    lease = _ModuleSourceLease.path_backed(path)
    result = {
        "functions": [],
        "func_code_ids": {},
        "local_classes": {},
    }
    if defect != "missing_stamp":
        result["source_sha256"] = (
            "0" * 64 if defect == "wrong_stamp" else lease.source_sha256
        )
    integrated = []
    diagnosed = []
    failures = []

    def fail(message: str, *args: object, **kwargs: object) -> int:
        failures.append(message)
        return 1

    error = frontend_execution._consume_frontend_module_result(
        "m",
        tmp_path / "other.py" if defect == "wrong_path" else path,
        result,
        source_lease=lease,
        record_frontend_timing=None,
        integrate_module_frontend_result=lambda *args, **kwargs: integrated.append(
            args
        ),
        accumulate_midend_diagnostics=lambda *args, **kwargs: diagnosed.append(args),
        fail=fail,
        json_output=True,
    )
    assert error == 1
    assert len(failures) == 1
    assert "source" in failures[0].lower()
    assert integrated == []
    assert diagnosed == []
