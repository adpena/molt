"""Startup wrappers preserve failures before publishing usable execution state."""

from __future__ import annotations

from molt.cli.backend_ir import (
    _build_entry_main_ops,
    _build_isolate_bootstrap_ops,
    _build_isolate_import_ops,
    _guard_initialization_ops,
)


def test_bootstrap_guards_setup_before_later_work_without_clearing() -> None:
    ops = _build_isolate_bootstrap_ops(
        code_slot_count=17,
        version_ops=[{"kind": "const_str", "s_value": "3.12", "out": "version"}],
        module_code_ops=[{"kind": "code_slot_set", "args": ["code"], "value": 3}],
    )
    assert [op["kind"] for op in ops] == [
        "code_slots_init",
        "check_exception",
        "const_str",
        "check_exception",
        "code_slot_set",
        "check_exception",
        "const_none",
        "check_exception",
        "ret",
        "label",
        "const_none",
        "ret",
    ]
    assert {op["value"] for op in ops if op["kind"] == "check_exception"} == {
        ops[-3]["value"]
    }
    assert ops[-1]["args"] != ops[-4]["args"]


def test_entry_guards_include_late_insertions() -> None:
    ops = _build_entry_main_ops(
        entry_init="molt_init_main",
        version_ops=[{"kind": "const_str", "s_value": "3.12", "out": "version"}],
        register_global_code_id=lambda _symbol: 1,
    )
    ops.insert(1, {"kind": "code_slots_init", "value": 5})
    ops.insert(
        -1, {"kind": "call", "s_value": "molt_init_sys", "args": [], "out": "sys"}
    )
    guarded = _guard_initialization_ops(ops)
    assert not any(op["kind"] == "exception_clear" for op in guarded)
    for index, op in enumerate(guarded):
        if op["kind"] in {"call", "code_slots_init", "const_str"}:
            assert guarded[index + 1]["kind"] == "check_exception"


def test_import_setup_and_each_module_call_share_one_failure_exit() -> None:
    ops = _build_isolate_import_ops(
        code_slot_count=17,
        module_init_symbols=[("sys", "molt_init_sys"), ("json", "molt_init_json")],
        register_global_code_id=lambda _symbol: 1,
    )
    assert not any(op["kind"] == "exception_clear" for op in ops)
    assert ops[1] == {"kind": "check_exception", "value": 1}
    for index, op in enumerate(ops[:-3]):
        if op["kind"] in {"call", "module_cache_get", "const_str", "string_eq"}:
            assert ops[index + 1] == {"kind": "check_exception", "value": 1}
    assert ops[-3] == {"kind": "label", "value": 1}
    assert ops[-1] == {"kind": "ret", "args": [ops[-2]["out"]]}
    assert not any(
        left == right and left["kind"] == "check_exception"
        for left, right in zip(ops, ops[1:])
    )


def test_guard_preserves_control_structure_and_uses_fresh_exit_label() -> None:
    body = [
        {"kind": "label", "value": 7},
        {"kind": "if", "args": ["condition"]},
        {"kind": "call", "s_value": "left", "args": []},
        {"kind": "else"},
        {"kind": "call", "s_value": "right", "args": []},
        {"kind": "end_if"},
        {"kind": "ret_void"},
    ]
    before = [dict(op) for op in body]
    guarded = _guard_initialization_ops(body)
    assert body == before
    assert guarded == [
        body[0],
        body[1],
        body[2],
        {"kind": "check_exception", "value": 8},
        body[3],
        body[4],
        {"kind": "check_exception", "value": 8},
        body[5],
        body[6],
        {"kind": "label", "value": 8},
        {"kind": "ret_void"},
    ]


def test_startup_control_projection_uses_exact_registry_wire_kinds() -> None:
    import tomllib
    from pathlib import Path
    from molt.frontend.lowering.op_kinds_generated import SIMPLEIR_STRUCTURAL_KINDS

    root = Path(__file__).resolve().parents[2]
    with (root / "runtime/molt-ir/src/tir/op_kinds.toml").open("rb") as source:
        table = tomllib.load(source)
    assert SIMPLEIR_STRUCTURAL_KINDS == frozenset(
        row["kind"] for row in table["simpleir_control_kind"] if row["structural"]
    )
    assert "if" in SIMPLEIR_STRUCTURAL_KINDS
    assert "IF" not in SIMPLEIR_STRUCTURAL_KINDS
    assert "exception_new_builtin" not in SIMPLEIR_STRUCTURAL_KINDS


def test_serialized_bootstrap_matches_native_signature_fixture() -> None:
    import json
    from pathlib import Path

    root = Path(__file__).resolve().parents[2]
    fixture = json.loads(
        (root / "tests/fixtures/bootstrap_return_abi.json").read_text(encoding="utf-8")
    )
    emitted = {
        "name": "molt_isolate_bootstrap",
        "params": [],
        "ops": _build_isolate_bootstrap_ops(
            code_slot_count=17, version_ops=[], module_code_ops=[]
        ),
    }
    assert json.loads(json.dumps(emitted)) == fixture
    for name in ("molt_main", "molt_host_init"):
        wrapper = _guard_initialization_ops(
            _build_entry_main_ops(
                entry_init=name,
                version_ops=[],
                register_global_code_id=lambda _symbol: 1,
            )
        )
        assert any(op["kind"] == "ret_void" for op in wrapper)
        assert not any(op["kind"] == "ret" for op in wrapper)
