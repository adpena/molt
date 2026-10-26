from __future__ import annotations

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

import ast
from functools import partial
import inspect
from pathlib import Path
from typing import NoReturn

import pytest

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli import frontend_integration
from molt.cli import frontend_worker
from molt.cli.models import (
    _FrontendModuleResultTimings,
    _FrontendParallelLayerState,
    _FrontendIntegrationState,
    _MidendDiagnosticsState,
)
from molt.cli.module_source import _ModuleSourceLease
from molt.frontend import SimpleTIRGenerator

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
            "body_selection": {"kind": "complete_source", "roots": []},
            "source_sha256": source_lease.source_sha256,
            "functions": [],
            "func_code_ids": {},
            "function_metadata": {},
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


def test_module_result_consumer_keeps_canonical_records_and_diagnostic_names(
    tmp_path: Path,
) -> None:
    path = tmp_path / "m.py"
    path.write_bytes(b"VALUE = 1\n")
    lease = _ModuleSourceLease.path_backed(path)
    generator = SimpleTIRGenerator(module_name="m")
    generator.visit(ast.parse(path.read_bytes()))
    document = generator.to_json(scope="module")
    result = frontend_worker._module_frontend_payload(
        generator,
        document,
        source_sha256=lease.source_sha256,
        visit_s=0.0,
        lower_s=0.0,
        total_s=0.0,
    )
    # Diagnostics retain the frontend's internal function identity until their
    # existing receiver projects a module-qualified display name.
    result["midend_policy_outcomes_by_function"] = {"molt_main": {"degrade_events": []}}
    result["midend_pass_stats_by_function"] = {"molt_main": {"fold": {"visited": 1}}}
    state = _FrontendIntegrationState(functions=[], known_classes={})
    diagnostics = _MidendDiagnosticsState(
        policy_outcomes_by_function={}, pass_stats_by_function={}
    )
    error = frontend_execution._consume_frontend_module_result(
        "m",
        path,
        result,
        source_lease=lease,
        record_frontend_timing=None,
        integrate_module_frontend_result=partial(
            frontend_integration._integrate_module_frontend_result_with_state, state
        ),
        accumulate_midend_diagnostics=partial(
            frontend_integration._accumulate_midend_diagnostics_with_state, diagnostics
        ),
        fail=lambda *args, **kwargs: pytest.fail(
            f"module consumption failed: {args!r}"
        ),
        json_output=True,
    )
    assert error is None
    for borrowed, integrated in zip(
        document["functions"], state.functions, strict=True
    ):
        assert integrated is borrowed
        assert integrated["ops"] is borrowed["ops"]
    for name, row in document["function_metadata"].items():
        assert state.function_metadata[name] is row
    symbol = SimpleTIRGenerator.module_init_symbol("m")
    assert symbol in state.global_code_ids and "molt_main" not in state.global_code_ids
    assert set(diagnostics.policy_outcomes_by_function) == {f"m::{symbol}"}
    assert diagnostics.pass_stats_by_function == {
        f"m::{symbol}": {"fold": {"visited": 1}}
    }


@pytest.mark.parametrize("defect", ["missing_stamp", "wrong_stamp", "wrong_path"])
def test_result_generation_refusal_precedes_integration(
    tmp_path: Path, defect: str
) -> None:
    path = tmp_path / "m.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    lease = _ModuleSourceLease.path_backed(path)
    result = {
        "body_selection": {"kind": "complete_source", "roots": []},
        "functions": [],
        "func_code_ids": {},
        "function_metadata": {},
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


def test_parallel_persistence_seeds_follow_parent_received_body_owners(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, isolated_molt_cache: Path
) -> None:
    import pickle

    from molt.cli.function_records import FunctionDigestSeed
    from molt.cli import module_cache

    path = tmp_path / "m.py"
    path.write_bytes(b"def answer(value):\n    return value + 1\n")
    lease = _ModuleSourceLease.path_backed(path)
    generator = SimpleTIRGenerator(module_name="m")
    generator.visit(ast.parse(path.read_bytes()))
    document = generator.to_json(scope="module")
    worker_result = frontend_worker._module_frontend_payload(
        generator,
        document,
        source_sha256=lease.source_sha256,
        visit_s=0.0,
        lower_s=0.0,
        total_s=0.0,
    )
    # ProcessPool transports values; a child's object identities are not a
    # reusable digest map in its receiving parent.
    result = pickle.loads(pickle.dumps(worker_result))
    assert any(function["name"] == "m__answer" for function in result["functions"])
    assert all(
        parent is not child
        for parent, child in zip(
            result["functions"], worker_result["functions"], strict=True
        )
    )
    digest = "ad" * 32
    seeds = frontend_execution._write_parallel_persisted_module_lowering(
        project_root=tmp_path,
        module_path=path,
        source_lease=lease,
        worker_mode="parallel",
        context_digest=digest,
        result=result,
    )
    assert isinstance(seeds, tuple) and len(seeds) == len(result["functions"])
    assert all(isinstance(seed, FunctionDigestSeed) for seed in seeds)
    for seed, function in zip(seeds, result["functions"], strict=True):
        assert seed.function is function
        assert isinstance(seed.digest, bytes) and len(seed.digest) == 32
    loaded = module_cache._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=lease,
        module_name="m",
        context_digest=digest,
    )
    assert loaded is not None
    assert loaded["functions"] == result["functions"]
    assert [seed.digest for seed in loaded["function_digest_seeds"]] == [
        seed.digest for seed in seeds
    ]
    assert all(
        seed.function is function
        for seed, function in zip(
            loaded["function_digest_seeds"], loaded["functions"], strict=True
        )
    )

    result["function_digest_seeds"] = seeds
    state = _FrontendIntegrationState(functions=[], known_classes={})
    issue = frontend_execution._consume_frontend_module_result(
        "m",
        path,
        result,
        source_lease=lease,
        record_frontend_timing=None,
        integrate_module_frontend_result=partial(
            frontend_integration._integrate_module_frontend_result_with_state, state
        ),
        accumulate_midend_diagnostics=lambda *_a, **_k: None,
        fail=lambda *args, **kwargs: pytest.fail(f"seed consumption failed: {args!r}"),
        json_output=True,
    )
    assert issue is None
    assert all(
        integrated is seed and integrated.function is function
        for integrated, seed, function in zip(
            state.function_digest_seeds, seeds, state.functions, strict=True
        )
    )

    monkeypatch.setattr(
        frontend_execution,
        "_write_persisted_module_lowering",
        lambda *_a, **_k: pytest.fail("a cache-hit result was published again"),
    )
    reused = frontend_execution._write_parallel_persisted_module_lowering(
        project_root=tmp_path,
        module_path=path,
        source_lease=lease,
        worker_mode="parallel_cache_hit",
        context_digest=digest,
        result=result,
    )
    assert isinstance(reused, tuple)
    assert len(reused) == len(seeds)
    assert all(
        actual is expected for actual, expected in zip(reused, seeds, strict=True)
    )


def test_optional_parallel_persistence_refusal_returns_empty_seed_tuple(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "m.py"
    path.write_bytes(b"VALUE = 1\n")
    lease = _ModuleSourceLease.path_backed(path)

    def unavailable(*_args: object, **_kwargs: object) -> NoReturn:
        raise OSError("injected optional cache publication refusal")

    monkeypatch.setattr(
        frontend_execution, "_write_persisted_module_lowering", unavailable
    )
    result = frontend_execution._write_parallel_persisted_module_lowering(
        project_root=tmp_path,
        module_path=path,
        source_lease=lease,
        worker_mode="parallel",
        context_digest="ad" * 32,
        result={"source_sha256": lease.source_sha256, "functions": []},
    )
    assert result == ()
    assert isinstance(result, tuple)
