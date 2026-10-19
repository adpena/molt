from __future__ import annotations

import importlib
import inspect
from copy import deepcopy
from typing import Any

import pytest

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
    "_plan_module_code_slots",
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
    document = generator.to_json(scope="module")
    functions = document["functions"]
    before = deepcopy(functions)
    states = []
    for _ in range(2):
        state = _FrontendIntegrationState(functions=[], known_classes={})
        issue = frontend_integration._integrate_module_frontend_result_with_state(
            state,
            "cached_module",
            ir_functions=functions,
            func_code_ids=document["code_slots"],
            function_metadata=document["function_metadata"],
            local_classes={},
        )
        assert issue is None
        states.append(state)
        assert functions == before
        for symbol, row in document["function_metadata"].items():
            assert state.function_metadata[symbol] is row
        for borrowed, integrated in zip(functions, state.functions, strict=True):
            assert integrated is borrowed
            assert integrated["ops"] is borrowed["ops"]
            assert integrated["params"] is borrowed["params"]
    assert states[0].functions == states[1].functions
    assert states[0].global_code_ids == states[1].global_code_ids
    init_symbol = SimpleTIRGenerator.module_init_symbol("cached_module")
    assert init_symbol in states[0].global_code_ids
    assert "molt_main" not in states[0].global_code_ids


def test_module_integration_retains_unmodified_function_and_operation_owners() -> None:
    untouched_op = {"kind": "ret_void"}
    untouched = {"name": "module__answer", "params": [], "ops": [untouched_op]}
    init_symbol = SimpleTIRGenerator.module_init_symbol("module")
    main_op = {"kind": "trace_enter_slot", "s_value": init_symbol}
    main = {"name": init_symbol, "params": [], "ops": [main_op, untouched_op]}
    functions = [main, untouched]
    before = deepcopy(functions)
    state = _FrontendIntegrationState(functions=[], known_classes={})
    issue = frontend_integration._integrate_module_frontend_result_with_state(
        state,
        "module",
        ir_functions=functions,
        func_code_ids={init_symbol: 0},
        function_metadata={},
        local_classes={},
    )
    assert issue is None
    assert functions == before
    assert state.functions[1] is untouched
    assert state.functions[1]["ops"] is untouched["ops"]
    projected = state.functions[0]
    assert projected is main and projected["ops"] is main["ops"]
    assert projected["params"] is main["params"]
    assert projected["name"] == init_symbol
    assert projected["ops"][0] is main_op
    assert projected["ops"][1] is untouched_op


def test_failed_module_integration_does_not_change_borrowed_operations() -> None:
    init_symbol = SimpleTIRGenerator.module_init_symbol("module")
    functions = [
        {
            "name": init_symbol,
            "params": [],
            "ops": [
                {"kind": "trace_enter_slot", "s_value": init_symbol},
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
        func_code_ids={init_symbol: 0},
        function_metadata={},
        local_classes={},
    )
    assert (
        issue == "Class statement 'foreign' is stamped outside its owning module module"
    )
    assert functions == before
    assert state.functions == []


@pytest.mark.parametrize(
    ("defect", "message"),
    [
        ("program-entry", "Legacy program entry"),
        ("program-slots", "cannot initialize program code slots"),
        ("program-owner", "Noncanonical module code owner"),
        ("legacy-slot", "Missing canonical code owner"),
        ("foreign-slot", "Missing canonical code owner"),
        ("numeric-slot", "Missing canonical code owner"),
    ],
)
def test_module_integration_rejects_stale_or_foreign_owners_without_rewriting(
    defect: str, message: str
) -> None:
    symbol = SimpleTIRGenerator.module_init_symbol("module")
    op: dict[str, Any] = {"kind": "trace_enter_slot", "s_value": symbol}
    function: dict[str, Any] = {
        "name": symbol,
        "params": [],
        "ops": [op, {"kind": "ret_void"}],
    }
    owners = {symbol: 0}
    if defect == "program-entry":
        function["name"] = "molt_main"
    elif defect == "program-slots":
        function["ops"].insert(0, {"kind": "code_slots_init", "value": 1})
    elif defect == "program-owner":
        owners["molt_main"] = 1
    elif defect == "legacy-slot":
        op["s_value"] = "molt_main"
    elif defect == "foreign-slot":
        op["s_value"] = "foreign"
    else:
        op["value"] = 0
    before = deepcopy(function)
    state = _FrontendIntegrationState(functions=[], known_classes={})
    issue = frontend_integration._integrate_module_frontend_result_with_state(
        state,
        "module",
        ir_functions=[function],
        func_code_ids=owners,
        function_metadata={},
        local_classes={},
    )
    assert issue is not None and message in issue
    assert function == before
    assert function["ops"][-1]["kind"] == "ret_void"
    assert state.functions == []


def test_module_integration_keeps_class_statement_owner_refusal() -> None:
    from molt.frontend import ClassId

    symbol = SimpleTIRGenerator.module_init_symbol("module")
    class_id = ClassId("module", "Owned", (1, 0))
    state = _FrontendIntegrationState(functions=[], known_classes={})
    state.class_definition_owners[class_id.definition_symbol] = "foreign"
    function = {"name": symbol, "params": [], "ops": [{"kind": "ret_void"}]}
    before = deepcopy(function)
    issue = frontend_integration._integrate_module_frontend_result_with_state(
        state,
        "module",
        ir_functions=[function],
        func_code_ids={symbol: 0},
        function_metadata={},
        local_classes={class_id: {"dynamic": False}},
    )
    assert issue == (
        f"Class statement {class_id.definition_symbol!r} is defined by both module "
        "foreign and module module"
    )
    assert function == before
    assert state.functions == []


@pytest.mark.parametrize(
    "defect",
    ["missing", "orphan", "duplicate", "bad-boundary", "legacy-body", "legacy-op"],
)
def test_module_integration_refuses_invalid_sidecar_before_publishing_functions(
    defect: str,
) -> None:
    import ast

    generator = SimpleTIRGenerator(module_name="sidecar")
    generator.visit(ast.parse("VALUE = 1\n"))
    document = generator.to_json(scope="module")
    functions = document["functions"]
    metadata = document["function_metadata"]
    state = _FrontendIntegrationState(functions=[], known_classes={})
    if defect == "missing":
        metadata = None
    elif defect == "orphan":
        metadata["absent"] = metadata.pop("molt_init_sidecar")
    elif defect == "duplicate":
        state.function_metadata["molt_init_sidecar"] = metadata["molt_init_sidecar"]
    elif defect == "bad-boundary":
        metadata["molt_init_sidecar"]["source_module_publication"][
            "boundary_index"
        ] = -1
    elif defect == "legacy-body":
        functions[0]["gpu_body_origin"] = {}
    else:
        functions[0]["ops"][0]["source_module_publication_boundary"] = True
    before = deepcopy(functions)
    before_metadata = deepcopy(metadata)
    existing_metadata = dict(state.function_metadata)
    issue = frontend_integration._integrate_module_frontend_result_with_state(
        state,
        "sidecar",
        ir_functions=functions,
        function_metadata=metadata,
        func_code_ids=document["code_slots"],
        local_classes={},
    )
    assert issue is not None
    assert functions == before
    assert metadata == before_metadata
    assert state.functions == []
    assert state.function_metadata == existing_metadata


@pytest.mark.parametrize(
    "defect",
    [
        None,
        "invalid-sidecar",
        "orphan-sidecar",
        "late-code-owner",
        "class-conflict",
        "copied-seed-body",
        "duplicate-seed-owner",
        "short-seed-digest",
        "nonbytes-seed-digest",
        "none-seeds",
    ],
)
def test_module_admission_commits_all_state_only_after_complete_validation(
    defect: str | None,
) -> None:
    from dataclasses import fields
    from molt.frontend import ClassId
    from molt.cli.function_records import FunctionDigestSeed, encode_lowering_record

    symbol = "molt_init_incoming"
    earlier = ClassId("incoming", "A", (1, 0))
    later = ClassId("incoming", "Z", (2, 0))
    seeded_class = ClassId("seed", "Preserved", (1, 0))
    retained_function = {
        "name": "seed__body",
        "params": [],
        "ops": [{"kind": "ret_void"}],
    }
    retained_class = {"dynamic": False}
    retained_row = {"app_callable_bindings": []}
    _encoded, (retained_seed,) = encode_lowering_record(b"{}", [retained_function])
    state = _FrontendIntegrationState(
        functions=[retained_function],
        known_classes={seeded_class: retained_class},
        function_digest_seeds=[retained_seed],
        global_code_ids={"seed__body": 0},
        function_metadata={"seed__body": retained_row},
        class_definition_owners={seeded_class.definition_symbol: "seed"},
        class_definition_references={seeded_class.definition_symbol: "seed"},
    )
    function = {
        "name": symbol,
        "params": [],
        "ops": [
            {"kind": "trace_enter_slot", "s_value": symbol},
            {
                "kind": "class_set_definition",
                "class_definition": earlier.definition_symbol,
            },
            {
                "kind": "class_set_definition",
                "class_definition": later.definition_symbol,
            },
            {"kind": "ret_void"},
        ],
    }
    metadata = {symbol: {"app_callable_bindings": []}}
    if defect == "invalid-sidecar":
        metadata[symbol]["unknown"] = True
    elif defect == "orphan-sidecar":
        metadata["absent"] = metadata.pop(symbol)
    elif defect == "late-code-owner":
        function["ops"].insert(-1, {"kind": "trace_enter_slot", "s_value": "absent"})
    elif defect == "class-conflict":
        state.class_definition_owners[later.definition_symbol] = "other"
    _encoded, accepted_seeds = encode_lowering_record(b"{}", [function])
    seeds: Any = accepted_seeds  # Deliberately malformed receiver evidence below.
    seed = accepted_seeds[0]
    if defect == "copied-seed-body":
        copied = deepcopy(function)
        assert copied == function and copied is not function
        seeds = (FunctionDigestSeed(copied, seed.digest),)
    elif defect == "duplicate-seed-owner":
        seeds = (seed, FunctionDigestSeed(function, seed.digest))
    elif defect == "short-seed-digest":
        seeds = (FunctionDigestSeed(function, seed.digest[:-1]),)
    elif defect == "nonbytes-seed-digest":
        invalid_digest: Any = bytearray(seed.digest)
        seeds = (FunctionDigestSeed(function, invalid_digest),)
    elif defect == "none-seeds":
        seeds = None
    before = deepcopy(state)
    owners = {field.name: getattr(state, field.name) for field in fields(state)}
    incoming = deepcopy((function, metadata))
    issue = frontend_integration._integrate_module_frontend_result_with_state(
        state,
        "incoming",
        ir_functions=[function],
        function_metadata=metadata,
        func_code_ids={symbol: 0},
        local_classes={earlier: {"dynamic": False}, later: {"dynamic": False}},
        function_digest_seeds=seeds,
    )
    assert (function, metadata) == incoming
    for field, owner in owners.items():
        assert getattr(state, field) is owner
    assert state.functions[0] is retained_function
    assert state.known_classes[seeded_class] is retained_class
    assert state.function_metadata["seed__body"] is retained_row
    assert state.function_digest_seeds[0] is retained_seed
    if defect is not None:
        assert issue is not None
        assert state == before
    else:
        assert issue is None
        assert state.functions[-1] is function
        assert state.function_digest_seeds[-1] is seed
        assert state.function_digest_seeds[-1].function is state.functions[-1]
        assert state.function_metadata[symbol] is metadata[symbol]
        assert list(state.global_code_ids) == [
            "seed__body",
            symbol,
            earlier.definition_symbol,
            later.definition_symbol,
        ]
