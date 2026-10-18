from __future__ import annotations

import importlib
import inspect
from copy import deepcopy

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli.models import _FrontendIntegrationState
from molt.frontend import SimpleTIRGenerator

frontend_integration = importlib.import_module("molt.cli.frontend_integration")
frontend_worker = importlib.import_module("molt.cli.frontend_worker")

_FRONTEND_INTEGRATION_NAMES = (
    "_accumulate_midend_diagnostics_with_state",
    "_integrate_module_frontend_result_with_state",
    "_register_global_code_id_with_state",
    "_integrate_module_code_slots_with_state",
)

_FRONTEND_INTEGRATION_DEFINITIONS = tuple(
    f"def {name}(" for name in _FRONTEND_INTEGRATION_NAMES
)


def test_cli_frontend_integration_authority_is_single_home() -> None:
    for name in _FRONTEND_INTEGRATION_NAMES:
        assert hasattr(frontend_integration, name), name
        assert not hasattr(frontend_execution, name), name
        assert not hasattr(frontend_worker, name), name
        assert not hasattr(cli, name), name

    frontend_execution_source = inspect.getsource(frontend_execution)
    frontend_worker_source = inspect.getsource(frontend_worker)
    cli_source = inspect.getsource(cli)
    for marker in _FRONTEND_INTEGRATION_DEFINITIONS:
        assert marker not in frontend_execution_source
        assert marker not in frontend_worker_source
        assert marker not in cli_source


def test_module_result_can_be_integrated_twice_without_mutating_cached_ir() -> None:
    import ast

    generator = SimpleTIRGenerator(module_name="cached_module")
    generator.visit(ast.parse("def answer(value):\n    return value + 1\n"))
    functions = generator.to_json()["functions"]
    before = deepcopy(functions)
    states = []
    for _ in range(2):
        state = _FrontendIntegrationState(functions=[], known_classes={})
        issue = frontend_integration._integrate_module_frontend_result_with_state(
            state,
            "cached_module",
            ir_functions=functions,
            func_code_ids=dict(generator.func_code_ids),
            local_classes={},
        )
        assert issue is None
        states.append(state)
        assert functions == before
    assert states[0].functions == states[1].functions
    assert states[0].global_code_ids == states[1].global_code_ids
    init_symbol = SimpleTIRGenerator.module_init_symbol("cached_module")
    assert init_symbol in states[0].global_code_ids
    assert "molt_main" not in states[0].global_code_ids


def test_module_integration_retains_unmodified_function_and_operation_owners() -> None:
    untouched_op = {"kind": "ret_void"}
    untouched = {"name": "module__answer", "params": [], "ops": [untouched_op]}
    main_op = {"kind": "trace_enter_slot", "s_value": "molt_main"}
    main = {"name": "molt_main", "params": [], "ops": [main_op, untouched_op]}
    functions = [main, untouched]
    before = deepcopy(functions)
    state = _FrontendIntegrationState(functions=[], known_classes={})
    issue = frontend_integration._integrate_module_frontend_result_with_state(
        state,
        "module",
        ir_functions=functions,
        func_code_ids={"molt_main": 0},
        local_classes={},
    )
    assert issue is None
    assert functions == before
    assert state.functions[1] is untouched
    assert state.functions[1]["ops"] is untouched["ops"]
    projected = state.functions[0]
    assert projected is not main and projected["ops"] is not main["ops"]
    assert projected["name"] == SimpleTIRGenerator.module_init_symbol("module")
    assert projected["ops"][0] == {
        "kind": "trace_enter_slot",
        "s_value": SimpleTIRGenerator.module_init_symbol("module"),
    }
    assert projected["ops"][1] is untouched_op


def test_failed_module_integration_does_not_change_borrowed_operations() -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                {"kind": "trace_enter_slot", "s_value": "molt_main"},
                {"kind": "class_set_definition", "class_definition": "foreign"},
            ],
        }
    ]
    before = deepcopy(functions)
    state = _FrontendIntegrationState(functions=[], known_classes={})
    issue = frontend_integration._integrate_module_frontend_result_with_state(
        state,
        "module",
        ir_functions=functions,
        func_code_ids={"molt_main": 0},
        local_classes={},
    )
    assert (
        issue == "Class statement 'foreign' is stamped outside its owning module module"
    )
    assert functions == before
    assert state.functions == []
