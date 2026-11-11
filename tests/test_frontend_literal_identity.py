from __future__ import annotations

import ast
import copy
import struct

import pytest

from molt.compiler_analysis.literal_identity import (
    literal_identity_key,
    same_literal_value,
)
from molt.compiler_analysis.static_truth import _same_scalar_value
from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend.lowering import serialization
from molt.frontend.lowering.serialization_context import SerializationContext


@pytest.mark.parametrize("defined", [False, True])
def test_identity_serialization_never_invents_or_redefines_none_inputs(
    defined: bool,
) -> None:
    gen = SimpleTIRGenerator()
    ctx = SerializationContext([], set(), None)
    operand = MoltValue("none_operand", "None")
    if defined:
        assert gen._serialize_basic_op(MoltOp("CONST_NONE", [], operand), ctx)
    comparison = MoltOp("IS", [operand, MoltValue("other")], MoltValue("same", "bool"))
    assert gen._serialize_basic_op(comparison, ctx)
    assert ctx.json_ops[-1] == {
        "kind": "is",
        "args": ["none_operand", "other"],
        "out": "same",
    }
    assert sum(op.get("out") == "none_operand" for op in ctx.json_ops) == int(defined)


def _serialization_generator() -> SimpleTIRGenerator:
    gen = SimpleTIRGenerator(module_name="pkg.sample")
    gen.visit(
        ast.parse("def identity(value):\n    return value\nanswer = identity(7)\n")
    )
    return gen


@pytest.mark.parametrize("first_scope", ["program", "module"])
def test_serialization_scopes_preserve_canonical_owners_across_repeated_calls(
    first_scope: str,
) -> None:
    gen = _serialization_generator()
    original_slots = dict(gen.func_code_ids)
    second_scope = "module" if first_scope == "program" else "program"
    snapshots = {}
    for scope in (first_scope, second_scope, first_scope, second_scope):
        ir = gen.to_json(scope=scope)
        owner = "molt_init_pkg_sample" if scope == "module" else "molt_main"
        retired_owner = "molt_main" if scope == "module" else "molt_init_pkg_sample"
        functions = {function["name"]: function for function in ir["functions"]}
        assert owner in functions
        assert retired_owner not in functions
        assert ir["code_slots"][owner] == original_slots["molt_main"]
        assert retired_owner not in ir["code_slots"]
        assert {
            name: slot for name, slot in ir["code_slots"].items() if name != owner
        } == {
            name: slot for name, slot in original_slots.items() if name != "molt_main"
        }
        all_ops = [op for function in functions.values() for op in function["ops"]]
        assert sum(op["kind"] == "code_slots_init" for op in all_ops) == (
            1 if scope == "program" else 0
        )
        for kind in ("trace_enter_slot", "code_slot_set"):
            refs = [op["s_value"] for op in all_ops if op["kind"] == kind]
            assert owner in refs
            assert retired_owner not in refs
            assert set(refs) <= ir["code_slots"].keys()
        if scope in snapshots:
            assert ir == snapshots[scope]
        else:
            snapshots[scope] = copy.deepcopy(ir)
        if scope == "program":
            assert gen.to_json() == ir
        # Serialization never renames the generator's semantic function owners.
        assert gen.func_code_ids == original_slots
        assert "molt_main" in gen.funcs_map
        assert "molt_init_pkg_sample" not in gen.funcs_map


@pytest.mark.parametrize("scope", ["program", "module"])
def test_serialization_parameters_are_detached_from_generator_and_other_results(
    scope: str,
) -> None:
    gen = _serialization_generator()
    first = gen.to_json(scope=scope)
    second = gen.to_json(scope=scope)
    first_functions = {function["name"]: function for function in first["functions"]}
    second_functions = {function["name"]: function for function in second["functions"]}
    for name, data in gen.funcs_map.items():
        emitted_name = (
            "molt_init_pkg_sample"
            if name == "molt_main" and scope == "module"
            else name
        )
        left = first_functions[emitted_name]["params"]
        right = second_functions[emitted_name]["params"]
        expected = list(data["params"])
        assert left == right == expected
        assert (
            left is not right
            and left is not data["params"]
            and right is not data["params"]
        )
        left.append("result_only")
        assert data["params"] == right == expected
        data["params"].append("generator_only")
        assert left == [*expected, "result_only"]
        assert right == expected


def test_serialization_context_allocation_is_per_function_not_per_operation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gen = _serialization_generator()
    constructed = []

    def observe_context(*args, **kwargs):
        context = SerializationContext(*args, **kwargs)
        constructed.append(context)
        return context

    monkeypatch.setattr(serialization, "SerializationContext", observe_context)
    ops = [MoltOp("CONST", [value], MoltValue(f"value_{value}")) for value in range(8)]
    mapped = gen.map_ops_to_json(ops, function_name="allocation_probe")
    assert mapped == [
        {"kind": "const", "value": value, "out": f"value_{value}"} for value in range(8)
    ] + [{"kind": "ret_void"}]
    assert len(constructed) == 1
    assert constructed[0].json_ops == mapped
    constructed.clear()
    ir = gen.to_json(scope="module")
    assert sum(len(function["ops"]) for function in ir["functions"]) > len(
        ir["functions"]
    )
    assert len(constructed) == len(ir["functions"])
    assert {context.function_name for context in constructed} == set(gen.funcs_map)


def _float(bits: int) -> float:
    return struct.unpack("!d", bits.to_bytes(8, "big"))[0]


_DISTINCT_VALUES = [
    (True, 1),
    (1, 1.0),
    (False, 0),
    (0.0, -0.0),
    (complex(0.0, 0.0), complex(-0.0, 0.0)),
    (_float(0x7FF8000000000001), _float(0x7FF8000000000002)),
    (_float(0x7FF8000000000001), _float(0xFFF8000000000001)),
    ((True, (0.0,)), (1, (-0.0,))),
    (frozenset({True}), frozenset({1})),
    (range(0), range(1, 1)),
]


def test_mutable_and_subclass_values_never_become_literal_identity() -> None:
    class CallbackInt(int):
        def __eq__(self, other: object) -> bool:
            raise AssertionError("literal identity must not invoke equality callbacks")

        def __hash__(self) -> int:
            raise AssertionError("literal identity must not invoke hash callbacks")

    for value in [[], {}, set(), ([],), CallbackInt(1), (CallbackInt(1),)]:
        assert literal_identity_key(value) is None
        assert not same_literal_value(value, value)


@pytest.mark.parametrize("scope", ["program", "module"])
def test_serialization_separates_assembly_metadata_from_executable_records(
    scope: str,
) -> None:
    gen = _serialization_generator()
    document = gen.to_json(scope=scope)
    owner = "molt_main" if scope == "program" else "molt_init_pkg_sample"
    assert "function_metadata" in document
    assert owner in document["function_metadata"]
    functions = {function["name"]: function for function in document["functions"]}
    for function in functions.values():
        assert {
            "source_module_publication",
            "gpu_body_origin",
            "app_callable_bindings",
        }.isdisjoint(function)
        assert all(
            "source_module_publication_boundary" not in op for op in function["ops"]
        )
    envelope = document["function_metadata"][owner]["source_module_publication"]
    assert envelope["publication"]["module_name"] == "pkg.sample"
    assert (
        functions[owner]["ops"][envelope["boundary_index"]]["kind"]
        == "frame_locals_set"
    )
    before = copy.deepcopy(document)
    # Detach the emitted publication facts from the still-live generator.
    gen.funcs_map["molt_main"]["source_module_publication"]["module_value"] = (
        "changed_generator"
    )
    assert document == before


@pytest.mark.parametrize(("left", "right"), _DISTINCT_VALUES)
def test_literal_identity_preserves_exact_types_and_bits(
    left: object, right: object
) -> None:
    assert not same_literal_value(left, right)
    assert literal_identity_key(left) != literal_identity_key(right)


def test_nan_identity_is_bit_stable_without_python_equality() -> None:
    left, right = _float(0x7FF8000000000001), _float(0x7FF8000000000001)
    assert left != right
    assert same_literal_value(left, right)
    assert not _same_scalar_value(
        left, right
    )  # static-truth policy remains conservative
    assert literal_identity_key(frozenset({left, right})) != literal_identity_key(
        frozenset({left})
    )
