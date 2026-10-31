from __future__ import annotations

from copy import deepcopy
import sys
import time
from typing import Any

import pytest

import tools.check_ir_structure as check_ir_structure
import tools.verify_ir_suite as verify_ir_suite
from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator, compile_to_tir
from tools.check_ir_structure import Diagnostic, verify_frontend_tir, verify_tir
from tools.rust_ir_verifier import (
    RustIrVerifier,
    close_process_local_verifier,
    process_local_verifier_pid,
)
from tools.verify_ir_suite import _load_pool_policy
from tests.process_guard_common import HUNG_CHILD_SOURCE


def _function_diagnostics(
    name: str,
    params: list[str],
    ops: list[dict[str, object]],
    *,
    function_names: set[str] | None = None,
) -> list[Diagnostic]:
    body = list(ops)
    if not body or body[-1].get("kind") not in {"ret", "ret_void"}:
        body.append({"kind": "ret_void"})
    functions = [{"name": name, "params": params, "return_abi": "value", "ops": body}]
    for sibling in sorted((function_names or set()) - {name}):
        functions.append(
            {
                "name": sibling,
                "params": [],
                "return_abi": "void",
                "ops": [{"kind": "ret_void"}],
            }
        )
    verification = verify_tir({"functions": functions})
    assert all(
        diagnostic.kind != "invalid-format" for diagnostic in verification.errors
    ), verification.errors
    return verification.errors


def _definition_diagnostics(
    name: str,
    params: list[str],
    ops: list[dict[str, object]],
) -> list[Diagnostic]:
    return [
        diagnostic
        for diagnostic in _function_diagnostics(name, params, ops)
        if diagnostic.kind
        in {
            "invalid-phi-arity",
            "non-dominating-definition",
            "non-dominating-phi-input",
            "use-before-def",
        }
    ]


def _frontend_publication_ir() -> dict[str, Any]:
    return {
        "functions": [
            {
                "name": "molt_init_unit",
                "params": [],
                "return_abi": "void",
                "ops": [
                    {"kind": "const", "value": 0, "out": "module"},
                    {
                        "kind": "frame_locals_set",
                        "args": ["module"],
                    },
                    {"kind": "ret_void"},
                ],
            }
        ],
        "function_metadata": {
            "molt_init_unit": {
                "source_module_publication": {
                    "publication": {
                        "module_name": "unit",
                        "module_value": "module",
                        "failure_label": 7,
                    },
                    "boundary_index": 1,
                }
            }
        },
    }


def test_strict_verifier_rejects_frontend_publication_envelope() -> None:
    ir = _frontend_publication_ir()
    del ir["function_metadata"]
    ir["functions"][0]["ops"][1]["source_module_publication_boundary"] = True
    result = verify_tir(ir)

    assert not result.ok
    assert any(
        "source_module_publication_boundary" in diagnostic.message
        for diagnostic in result.errors
    ), result.errors


def test_frontend_verifier_projects_publication_envelope_without_mutation() -> None:
    ir = _frontend_publication_ir()
    before = deepcopy(ir)

    result = verify_frontend_tir(ir)

    assert result.ok, result.errors
    assert ir == before


def test_frontend_verifier_forwards_transport_controls(monkeypatch) -> None:
    observed: dict[str, Any] = {}
    sentinel = object()

    def record_verify(tir, *, request_id=None, timeout_seconds=None):
        observed.update(
            tir=tir,
            request_id=request_id,
            timeout_seconds=timeout_seconds,
        )
        return sentinel

    monkeypatch.setattr(check_ir_structure, "verify_tir", record_verify)

    result = verify_frontend_tir(
        _frontend_publication_ir(),
        request_id=41,
        timeout_seconds=2.5,
    )

    assert result is sentinel
    assert observed["request_id"] == 41
    assert observed["timeout_seconds"] == 2.5
    assert "function_metadata" not in observed["tir"]
    function = observed["tir"]["functions"][0]
    assert "source_module_publication" not in function
    assert all("source_module_publication_boundary" not in op for op in function["ops"])


@pytest.mark.parametrize("marker", [False, 1, "true"])
def test_frontend_serialization_rejects_invalid_publication_marker(
    marker: object,
) -> None:
    generator = SimpleTIRGenerator(module_name="unit")
    generator.funcs_map["molt_main"]["source_module_publication"] = {
        "module_name": "unit",
        "module_value": "module",
        "failure_label": 7,
    }
    generator.funcs_map["molt_main"]["ops"] = [
        MoltOp(
            kind="FRAME_LOCALS_SET",
            args=[MoltValue("module")],
            result=MoltValue("none"),
            metadata={"source_module_publication_boundary": marker},
        )
    ]
    with pytest.raises(ValueError, match="publication boundary"):
        generator.to_json(scope="module", midend_stage="pre-midend")


@pytest.mark.parametrize("marker_count", [0, 1, 2])
def test_frontend_serialization_rejects_missing_or_orphan_or_duplicate_publication_marker(
    marker_count: int,
) -> None:
    generator = SimpleTIRGenerator(module_name="unit")
    if marker_count != 1:
        generator.funcs_map["molt_main"]["source_module_publication"] = {
            "module_name": "unit",
            "module_value": "module",
            "failure_label": 7,
        }
    generator.funcs_map["molt_main"]["ops"] = [
        MoltOp(
            kind="FRAME_LOCALS_SET",
            args=[MoltValue("module")],
            result=MoltValue("none"),
            metadata={"source_module_publication_boundary": True},
        )
        for _ in range(marker_count)
    ]
    with pytest.raises(ValueError, match="publication boundary"):
        generator.to_json(scope="module", midend_stage="pre-midend")


@pytest.mark.parametrize(
    "malformation",
    ["metadata", "orphan", "bool-index", "missing-index", "past-end", "wrong-kind"],
)
def test_frontend_verifier_rejects_malformed_publication_envelope(
    malformation: str,
) -> None:
    ir = _frontend_publication_ir()
    envelope = ir["function_metadata"]["molt_init_unit"]["source_module_publication"]
    if malformation == "metadata":
        del envelope["publication"]["failure_label"]
    elif malformation == "orphan":
        ir["function_metadata"]["absent"] = ir["function_metadata"].pop(
            "molt_init_unit"
        )
    elif malformation == "bool-index":
        envelope["boundary_index"] = False
    elif malformation == "missing-index":
        del envelope["boundary_index"]
    elif malformation == "past-end":
        envelope["boundary_index"] = 3
    else:
        ir["functions"][0]["ops"][1]["kind"] = "const"
    result = verify_frontend_tir(ir)
    assert not result.ok
    assert [diagnostic.kind for diagnostic in result.errors] == ["invalid-format"]


def test_generated_field_roles_cover_every_multi_result_transport_shape() -> None:
    ops = [
        {"kind": "const", "out": "sequence", "value": 0},
        {
            "kind": "unpack_sequence",
            "args": ["sequence", "left", "right"],
            "value": 2,
        },
        {
            "kind": "checked_add",
            "args": ["left", "right"],
            "var": "sum",
            "out": "overflow",
        },
        {
            "kind": "checked_mul",
            "args": ["sum", "right"],
            "var": "product",
            "out": "mul_overflow",
        },
        {
            "kind": "iter_next_unboxed",
            "args": ["iterator"],
            "var": "item",
            "out": "done",
        },
        {"kind": "ret", "args": ["product"]},
    ]

    assert _definition_diagnostics("multi", ["iterator"], ops) == []


def test_multi_result_outputs_do_not_mask_a_real_undefined_source() -> None:
    ops = [
        {
            "kind": "unpack_sequence",
            "args": ["missing_sequence", "left", "right"],
            "value": 2,
        },
        {
            "kind": "checked_add",
            "args": ["left", "missing_rhs"],
            "var": "sum",
            "out": "overflow",
        },
    ]

    diagnostics = _definition_diagnostics("bad_multi", [], ops)
    assert [
        diagnostic.message.split(" used by", 1)[0] for diagnostic in diagnostics
    ] == [
        'variable "missing_sequence"',
        'variable "missing_rhs"',
    ]


def test_frontend_unpack_transport_uses_generated_field_roles() -> None:
    ir = compile_to_tir(
        "def pair_sum(pair):\n    left, right = pair\n    return left + right\n"
    )

    assert any(
        op.get("kind") == "unpack_sequence"
        for function in ir["functions"]
        for op in function["ops"]
    )
    verification = verify_frontend_tir(ir)
    assert verification.ok, verification.errors


def test_definition_oracle_cannot_hide_malformed_executable_transport() -> None:
    with pytest.raises(AssertionError, match="invalid-format"):
        _definition_diagnostics(
            "malformed", [], [{"kind": "const", "out": "value", "metadata": {}}]
        )


def test_source_runner_projects_envelope_and_preserves_request_custody(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    ir = _frontend_publication_ir()
    before = deepcopy(ir)
    observed: dict[str, Any] = {}

    def strict_verifier(tir, *, request_id, timeout_seconds):
        observed.update(tir=tir, request_id=request_id, timeout_seconds=timeout_seconds)
        return check_ir_structure.VerificationResult(
            verifier_pid=123,
            verifier_cpu_seconds=0.25,
            verifier_lifetime_peak_rss_bytes=4096,
        )

    monkeypatch.setattr(check_ir_structure, "verify_tir", strict_verifier)
    compiled = {
        "source": "unit.py",
        "status": "compiled",
        "tir": ir,
        "duration_seconds": 1.5,
        "compiler_worker_cpu_seconds": 0.75,
    }
    result = verify_ir_suite._finalize_compiled_result(
        compiled, 41, per_case_timeout=4.0
    )

    assert result is compiled
    assert result["status"] == "pass"
    assert result["verifier_request_id"] == observed["request_id"] == 41
    assert observed["timeout_seconds"] == 2.5
    assert result["verifier_pid"] == 123
    assert result["verifier_cpu_seconds"] == 0.25
    assert result["worker_cpu_seconds"] == 1.0
    assert result["verifier_lifetime_peak_rss_bytes"] == 4096
    assert "tir" not in result
    assert ir == before
    assert "function_metadata" not in observed["tir"]
    function = observed["tir"]["functions"][0]
    assert "source_module_publication" not in function
    assert all("source_module_publication_boundary" not in op for op in function["ops"])


def test_source_runner_reports_malformed_envelope_without_rust_dispatch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    ir = _frontend_publication_ir()
    ir["function_metadata"]["molt_init_unit"]["source_module_publication"][
        "boundary_index"
    ] = False
    before = deepcopy(ir)

    def unexpected_dispatch(*args, **kwargs):
        pytest.fail("malformed frontend envelope reached executable verification")

    monkeypatch.setattr(check_ir_structure, "verify_tir", unexpected_dispatch)
    result = verify_ir_suite._finalize_compiled_result(
        {"status": "compiled", "tir": ir, "duration_seconds": 0.0},
        42,
        per_case_timeout=4.0,
    )

    assert result["status"] == "fail"
    assert result["returncode"] == 1
    assert result["verifier_request_id"] == 42
    assert result["verifier_pid"] is None
    assert "invalid-format" in result["detail"]
    assert "invalid source-module publication envelope" in result["detail"]
    assert ir == before


def test_source_runner_real_frontend_to_strict_verifier(tmp_path) -> None:
    source = tmp_path / "unpack.py"
    source.write_text(
        "def pair_sum(pair):\n    left, right = pair\n    return left + right\n",
        encoding="utf-8",
    )
    compiled = verify_ir_suite._verify_source_worker(str(source))
    assert compiled["status"] == "compiled", compiled
    ir = compiled["tir"]
    before = deepcopy(ir)
    assert any(
        "source_module_publication" in row for row in ir["function_metadata"].values()
    )
    assert all(
        "source_module_publication" not in function for function in ir["functions"]
    )

    result = verify_ir_suite._finalize_compiled_result(
        compiled, 43, per_case_timeout=60.0
    )

    assert result["status"] == "pass", result
    assert result["compile"]["status"] == "pass"
    assert result["verifier_request_id"] == 43
    assert result["verifier_pid"] is not None
    assert ir == before


def test_suite_streams_each_case_with_its_cost_before_the_summary(
    tmp_path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sources = []
    for name in ("first", "second"):
        source = tmp_path / f"{name}.py"
        source.write_text(f"def {name}():\n    return 1\n", encoding="utf-8")
        sources.append(source)

    def stub_verifier(tir, *, request_id, timeout_seconds):
        return check_ir_structure.VerificationResult(
            verifier_pid=7,
            verifier_lifetime_peak_rss_bytes=(request_id + 1) * 2**30,
        )

    monkeypatch.setattr(check_ir_structure, "verify_tir", stub_verifier)
    streamed: list[dict[str, object]] = []
    results, _telemetry = verify_ir_suite._run_worker_pool_owned(
        sources,
        worker_count=1,
        per_case_timeout=120.0,
        max_cases_per_worker=8,
        fail_fast=False,
        resource_policy={},
        on_result=streamed.append,
    )

    assert [result["source"] for result in streamed] == [str(s) for s in sources]
    assert streamed == results
    assert [result["status"] for result in streamed] == ["pass", "pass"]
    assert [result["verifier_lifetime_peak_rss_bytes"] for result in streamed] == [
        2**30,
        2 * 2**30,
    ]


def test_case_line_names_time_and_verifier_peak(capsys) -> None:
    verify_ir_suite._print_case_result(
        {
            "source": "big.py",
            "status": "fail",
            "duration_seconds": 12.345,
            "verifier_lifetime_peak_rss_bytes": 3 * 2**30,
            "detail": "ERRORS (1):\nbad op",
        },
        quiet=True,
    )
    verify_ir_suite._print_case_result(
        {"source": "small.py", "status": "pass", "duration_seconds": 0.5},
        quiet=True,
    )

    assert capsys.readouterr().out == (
        "  FAIL big.py (12.35 s, verifier peak 3072 MiB)\n"
        "       ERRORS (1):\n"
        "       bad op\n"
    )


def test_branch_local_definition_does_not_dominate_join_use() -> None:
    diagnostics = _definition_diagnostics(
        "branch",
        ["condition"],
        [
            {"kind": "if", "args": ["condition"]},
            {"kind": "const", "out": "branch_local", "value": 1},
            {"kind": "end_if"},
            {"kind": "ret", "args": ["branch_local"]},
        ],
    )

    assert [diagnostic.kind for diagnostic in diagnostics] == [
        "non-dominating-definition"
    ]


def test_phi_inputs_follow_generated_predecessor_order() -> None:
    prefix = [
        {"kind": "if", "args": ["condition"]},
        {"kind": "const", "out": "left", "value": 1},
        {"kind": "else"},
        {"kind": "const", "out": "right", "value": 2},
        {"kind": "end_if"},
    ]
    valid = [
        *prefix,
        {"kind": "phi", "args": ["left", "right"], "out": "merged"},
        {"kind": "ret", "args": ["merged"]},
    ]
    invalid = [
        *prefix,
        {"kind": "phi", "args": ["right", "left"], "out": "merged"},
        {"kind": "ret", "args": ["merged"]},
    ]

    assert _definition_diagnostics("phi", ["condition"], valid) == []
    assert [
        diagnostic.kind
        for diagnostic in _definition_diagnostics("phi", ["condition"], invalid)
    ] == ["non-dominating-phi-input", "non-dominating-phi-input"]


def test_structured_labels_and_internal_calls_fail_closed() -> None:
    diagnostics = _function_diagnostics(
        "caller",
        [],
        [
            {"kind": "else"},
            {"kind": "jump", "value": 7},
            {"kind": "call_internal", "s_value": "missing"},
        ],
        function_names={"caller"},
    )
    kinds = {diagnostic.kind for diagnostic in diagnostics}
    assert {
        "invalid-call-target",
        "invalid-jump-target",
        "unbalanced-control-flow",
    } <= kinds


def test_rust_verifier_reuses_one_ordered_process() -> None:
    close_process_local_verifier()
    first = verify_tir(
        {
            "functions": [
                {
                    "name": "first",
                    "params": [],
                    "return_abi": "void",
                    "ops": [{"kind": "ret_void"}],
                }
            ]
        },
        request_id=41,
    )
    first_pid = process_local_verifier_pid()
    second = verify_tir(
        {
            "functions": [
                {
                    "name": "second",
                    "params": [],
                    "return_abi": "void",
                    "ops": [{"kind": "ret_void"}],
                }
            ]
        },
        request_id=42,
    )
    try:
        assert first.ok and second.ok
        assert first_pid is not None
        assert process_local_verifier_pid() == first_pid
        assert first.verifier_pid == second.verifier_pid == first_pid
    finally:
        close_process_local_verifier()


def test_ir_worker_defaults_come_from_calibrated_policy() -> None:
    payload = _load_pool_policy()
    assert payload["schema"] == "molt.ir-verification-pool.v1"
    assert payload["policy"] == {
        "max_workers": 4,
        "gb_per_worker": 1.0,
        "max_cases_per_worker": 64,
        "per_case_timeout_seconds": 60.0,
    }


def test_verifier_request_deadline_closes_a_wedged_owned_child() -> None:
    # The child reads one request, then never answers until its owner kills it.
    script = "import sys\nsys.stdin.readline()\n" + HUNG_CHILD_SOURCE
    verifier = RustIrVerifier(
        command=[sys.executable, "-u", "-c", script],
        request_timeout_seconds=0.05,
    )
    started = time.monotonic()
    try:
        with pytest.raises(TimeoutError, match="request 7 exceeded"):
            verifier.verify({"functions": []}, request_id=7)
    finally:
        verifier.close()
    assert time.monotonic() - started < 2.0


def test_verifier_build_refused_below_the_capacity_floor_starts_no_cargo(
    tmp_path, monkeypatch
) -> None:
    # HF-101: the verifier built molt-ir with 22 GiB free against the 25 GiB
    # build floor because nothing admitted capacity before its Cargo build.
    from molt import disk_capacity
    import tools.rust_ir_verifier as rust_ir_verifier

    launched: list[list[str]] = []
    measured: list[object] = []
    free_bytes = disk_capacity.DEFAULT_MINIMUM_HEADROOM_BYTES - 1

    class _RecordingCommands:
        def run(self, command, **_kwargs):
            launched.append(list(command))

    def measure(path):
        measured.append(path)
        return free_bytes

    monkeypatch.setattr(rust_ir_verifier, "_COMMANDS", _RecordingCommands())
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", measure)
    monkeypatch.setattr(
        disk_capacity, "_darwin_important_usage_bytes", lambda path: free_bytes
    )
    monkeypatch.delenv(disk_capacity.DISK_GUARD_HIGH_WATER_ENV, raising=False)
    env = {
        "CARGO_TARGET_DIR": str(tmp_path / "verifier-target"),
        "MOLT_EXT_ROOT": str(tmp_path / "artifacts"),
    }

    with pytest.raises(disk_capacity.DiskCapacityError):
        rust_ir_verifier.verifier_binary(env=env)

    assert launched == []
    assert measured == [tmp_path]  # the target's nearest existing directory

    # Every caller sees the refusal itself, never an IR format diagnostic.
    close_process_local_verifier()
    monkeypatch.setattr(rust_ir_verifier, "_verifier_environment", lambda: dict(env))
    with pytest.raises(disk_capacity.DiskCapacityError):
        verify_tir({"functions": []})
    assert launched == []

    # Control: with capacity admitted, the same request reaches Cargo.
    free_bytes = disk_capacity.DEFAULT_MINIMUM_HEADROOM_BYTES
    with pytest.raises(FileNotFoundError):  # the recording Cargo builds nothing
        rust_ir_verifier.verifier_binary(env=env)
    assert launched == [["cargo", "build", "-p", "molt-ir", "--bin", "molt-ir-verify"]]


@pytest.mark.parametrize("failure", [None, "launch", "reader", "cleanup"])
def test_verifier_owns_managed_state_through_child_exit(
    tmp_path, monkeypatch, failure
) -> None:
    import os
    import gc
    import weakref
    from types import SimpleNamespace
    import tools.rust_ir_verifier as verifier_module
    from molt.build_state_layout import build_state_root
    from molt.checkout_state import checkout_state_retirement
    from molt.dx import checkout_component
    from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

    artifact = tmp_path / "artifacts"
    parent = (
        artifact / "target" / "checkouts" / checkout_component(verifier_module.ROOT)
    )
    target = parent / "sessions" / "verifier-lifetime"
    target.mkdir(parents=True)
    env = ProcessEnvironmentBuilder(os.environ)
    for key in (
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "UV_PROJECT_ENVIRONMENT",
        "MOLT_BUILD_STATE_DIR",
    ):
        env.pop(key, None)
    env.update({"MOLT_EXT_ROOT": str(artifact), "CARGO_TARGET_DIR": str(target)})
    environment = ProcessEnvironment(env)
    control = build_state_root(
        project_root=verifier_module.ROOT,
        cargo_target=target,
        environment=environment,
    )
    protected = (parent, target, control)
    processes = []
    terminators = []
    verifier_refs = []
    start = verifier_module._COMMANDS.start_owned

    def observe_start(*args, **kwargs):
        # The binary lookup/build-to-spawn boundary must already be pinned.
        for path in protected:
            with checkout_state_retirement(path) as admitted:
                assert not admitted
        process = start(*args, **kwargs)
        processes.append(process)
        if failure == "cleanup":
            terminators.append(process.terminate)

            def fail_terminate():
                raise OSError("termination fault")

            monkeypatch.setattr(process, "terminate", fail_terminate)
        return process

    monkeypatch.setattr(
        verifier_module, "_COMMANDS", SimpleNamespace(start_owned=observe_start)
    )
    if failure in {"reader", "cleanup"}:
        from tests.process_guard_common import install_module_view

        class FailingReader(verifier_module.threading.Thread):
            def __init__(self, *args, **kwargs):
                verifier_refs.append(weakref.ref(kwargs["target"].__self__))
                super().__init__(*args, **kwargs)

            def start(self):
                raise RuntimeError("reader start failed")

        install_module_view(
            monkeypatch,
            "threading",
            verifier_module.threading,
            verifier_module,
            Thread=FailingReader,
        )
    command = (
        [str(tmp_path / "missing-verifier")]
        if failure == "launch"
        else [sys.executable, "-I", "-S", "-c", "import sys; sys.stdin.buffer.read()"]
    )
    if failure is not None:
        try:
            with pytest.raises(
                FileNotFoundError
                if failure == "launch"
                else OSError
                if failure == "cleanup"
                else RuntimeError,
                match=None
                if failure == "launch"
                else "termination fault"
                if failure == "cleanup"
                else "reader start failed",
            ):
                RustIrVerifier(command=command, env=environment)
            if failure == "cleanup":
                # No exception/traceback or test-owned strong verifier remains.
                # Its existing cleanup owner must survive GC with a live child.
                gc.collect()
                assert verifier_refs[0]() is not None
                assert processes[0].poll() is None
                for path in protected:
                    with checkout_state_retirement(path) as admitted:
                        assert not admitted
            else:
                assert all(process.poll() is not None for process in processes)
        finally:
            if failure == "cleanup" and processes:
                monkeypatch.setattr(processes[0], "terminate", terminators[0])
                owner = verifier_refs[0]()
                if owner is not None:
                    owner.close(graceful=False)
                elif processes[0].poll() is None:
                    terminators[0]()
                    processes[0].wait(timeout=5)
    else:
        verifier = RustIrVerifier(command=command, env=environment)
        try:
            assert processes[0].poll() is None
            for path in protected:
                with checkout_state_retirement(path) as admitted:
                    assert not admitted
        finally:
            verifier.close()
        verifier.close()  # Existing repeated-close owner remains idempotent.
        assert processes[0].poll() is not None
    for path in protected:
        with checkout_state_retirement(path) as admitted:
            assert admitted


def test_verifier_rebuilds_after_completed_target_retirement(tmp_path, monkeypatch):
    import tools.rust_ir_verifier as verifier_module

    binary = tmp_path / "debug" / verifier_module._BINARY_NAME
    builds = []

    class Commands:
        def run(self, command, **kwargs):
            builds.append(tuple(command))
            binary.parent.mkdir(parents=True, exist_ok=True)
            binary.write_bytes(b"compiled output")

    monkeypatch.setattr(verifier_module, "_COMMANDS", Commands())
    monkeypatch.setattr(verifier_module, "_BINARY_READY", set())
    monkeypatch.setattr(
        verifier_module, "require_cargo_build_capacity", lambda *a, **k: None
    )
    environment = {"CARGO_TARGET_DIR": str(tmp_path)}
    assert verifier_module.verifier_binary(env=environment) == binary
    assert verifier_module.verifier_binary(env=environment) == binary
    assert len(builds) == 1
    binary.unlink()  # Simulate retirement after its preceding user completed.
    assert verifier_module.verifier_binary(env=environment) == binary
    assert len(builds) == 2
