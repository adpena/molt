from __future__ import annotations

import sys
import time

import pytest

from molt.frontend import compile_to_tir
from tools.check_ir_structure import Diagnostic, verify_tir
from tools.rust_ir_verifier import (
    RustIrVerifier,
    close_process_local_verifier,
    process_local_verifier_pid,
)
from tools.verify_ir_suite import _load_pool_policy


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
    functions = [{"name": name, "params": params, "ops": body}]
    for sibling in sorted((function_names or set()) - {name}):
        functions.append(
            {"name": sibling, "params": [], "ops": [{"kind": "ret_void"}]}
        )
    return verify_tir({"functions": functions}).errors


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
    assert [diagnostic.message.split(" used by", 1)[0] for diagnostic in diagnostics] == [
        'variable "missing_sequence"',
        'variable "missing_rhs"',
    ]


def test_frontend_unpack_transport_uses_generated_field_roles() -> None:
    ir = compile_to_tir(
        "def pair_sum(pair):\n"
        "    left, right = pair\n"
        "    return left + right\n"
    )

    diagnostics = []
    saw_unpack = False
    for function in ir["functions"]:
        ops = function["ops"]
        saw_unpack |= any(op.get("kind") == "unpack_sequence" for op in ops)
        diagnostics.extend(
            _definition_diagnostics(function["name"], function.get("params", []), ops)
        )

    assert saw_unpack
    assert diagnostics == []


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
        {"functions": [{"name": "first", "params": [], "ops": [{"kind": "ret_void"}]}]},
        request_id=41,
    )
    first_pid = process_local_verifier_pid()
    second = verify_tir(
        {"functions": [{"name": "second", "params": [], "ops": [{"kind": "ret_void"}]}]},
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
    script = (
        "import sys,time\n"
        "for line in sys.stdin:\n"
        "    time.sleep(30)\n"
    )
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
