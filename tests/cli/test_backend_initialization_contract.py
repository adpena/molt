"""Startup wrappers preserve failures before publishing usable execution state."""

from __future__ import annotations

import ast
from copy import deepcopy
from pathlib import Path

import pytest

from molt.cli.backend_ir import (
    _build_entry_main_ops,
    _build_isolate_bootstrap_ops,
    _build_isolate_import_ops,
    _guard_initialization_ops,
    _prepare_backend_ir,
)
from molt.cli.frontend_integration import _integrate_module_frontend_result_with_state
from molt.cli.models import _FrontendIntegrationState
from molt.cli.output import fail
from molt.frontend import SimpleTIRGenerator
from molt.target_python import _SUPPORTED_TARGET_PYTHON_BY_SHORT


def test_bootstrap_guards_setup_before_later_work_without_clearing() -> None:
    ops = _build_isolate_bootstrap_ops(
        code_slot_count=17,
        version_ops=[{"kind": "const_str", "s_value": "3.12", "out": "version"}],
    )
    assert [op["kind"] for op in ops] == [
        "code_slots_init",
        "check_exception",
        "const_str",
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
        "return_abi": "value",
        "ops": _build_isolate_bootstrap_ops(code_slot_count=17, version_ops=[]),
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


@pytest.mark.parametrize("target", ["native", "wasm", "llvm", "rust", "luau", "mlir"])
@pytest.mark.parametrize("target_python", ["3.12", "3.13", "3.14"])
@pytest.mark.parametrize(
    "entry_module,chunked,stdlib_profile",
    [
        ("__main__", False, "micro"),
        ("entry", True, "full"),
    ],
)
def test_module_initializers_solely_own_code_and_globals_publication(
    tmp_path: Path,
    target: str,
    target_python: str,
    entry_module: str,
    chunked: bool,
    stdlib_profile: str,
) -> None:
    """Exercise real frontend publication through complete backend assembly."""
    version = _SUPPORTED_TARGET_PYTHON_BY_SHORT[target_python]
    modules = ("sys", "dependency", entry_module)
    module_graph = {module: tmp_path / f"{module}.py" for module in modules}
    state = _FrontendIntegrationState(functions=[], known_classes={})
    expected_slots = {}
    for module in modules:
        generator = SimpleTIRGenerator(
            module_name=module,
            entry_module=entry_module,
            known_modules=set(modules),
            source_path=f"generated/{module}.py",
            target_python=(version.major, version.minor),
            module_chunking=chunked,
            module_chunk_max_ops=1,
        )
        generator.visit(ast.parse("first = 1\nsecond = 2\nthird = 3\n"))
        issue = _integrate_module_frontend_result_with_state(
            state,
            module,
            ir_functions=generator.to_json()["functions"],
            func_code_ids=generator.func_code_ids,
            local_class_names=[],
            local_classes={},
        )
        assert issue is None
        init = SimpleTIRGenerator.module_init_symbol(module)
        ops = next(
            function["ops"] for function in state.functions if function["name"] == init
        )
        slots = [op for op in ops if op["kind"] == "code_slot_set"]
        assert len(slots) == 1
        expected_slots[init] = deepcopy(slots[0])

    prepared, error = _prepare_backend_ir(
        entry_module=entry_module,
        module_graph=module_graph,
        known_modules=set(modules),
        stdlib_allowlist=set(),
        integration_state=state,
        fail=fail,
        json_output=True,
        module_order=modules,
        runtime_import_dispatch_roots=set(modules),
        spawn_enabled=False,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        emit_ir_path=None,
        target_python=version,
        stdlib_profile=stdlib_profile,
        target=target,
    )
    assert error is None
    assert prepared is not None
    functions = {function["name"]: function for function in prepared.ir["functions"]}
    for name, function in functions.items():
        expected = (
            "value"
            if name in {"molt_isolate_bootstrap", "molt_isolate_import"}
            else "void"
        )
        assert function["return_abi"] == expected, name
    for name in (
        "molt_main",
        "molt_host_init",
        "molt_isolate_bootstrap",
        "molt_isolate_import",
    ):
        if name not in functions:
            continue
        ops = functions[name]["ops"]
        assert not any(op["kind"] in {"code_new", "code_slot_set"} for op in ops)
        allocations = [op for op in ops if op["kind"] == "code_slots_init"]
        assert len(allocations) == 1
        assert allocations[0]["value"] == len(state.global_code_ids)
        for index, op in enumerate(ops):
            if op["kind"] == "code_slots_init":
                assert ops[index + 1]["kind"] == "check_exception"

    published = []
    for init, slot in expected_slots.items():
        ops = functions[init]["ops"]
        slots = [op for op in ops if op["kind"] == "code_slot_set"]
        assert slots == [slot]
        assert len(slot["args"]) == 2
        published.append(slot["value"])
        producers = {op["out"]: op for op in ops if "out" in op}
        code = producers[slot["args"][0]]
        globals_dict = producers[slot["args"][1]]
        assert code["kind"] == "code_new"
        assert len(code["args"]) == 9
        assert producers[code["args"][0]]["s_value"].startswith("generated/")
        assert globals_dict["kind"] == "module_get_attr"
        assert producers[globals_dict["args"][1]]["s_value"] == "__dict__"
        enter_index = next(
            index for index, op in enumerate(ops) if op["kind"] == "trace_enter_slot"
        )
        assert ops.index(slot) < enter_index
        assert ops[enter_index]["value"] == slot["value"]
    assert len(set(published)) == len(modules)
    assert sum(
        op["kind"] == "code_slot_set"
        for function in functions.values()
        for op in function["ops"]
    ) == len(modules)
