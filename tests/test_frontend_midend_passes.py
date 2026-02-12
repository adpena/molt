from __future__ import annotations

import ast
import os
import types
from contextlib import contextmanager

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend.cfg_analysis import build_cfg


def _lower_ops(ops: list[MoltOp]) -> list[dict]:
    gen = SimpleTIRGenerator()
    return gen.map_ops_to_json(ops)


def _undefined_args(lowered: list[dict]) -> list[tuple[int, str, str]]:
    defined: set[str] = set()
    missing: list[tuple[int, str, str]] = []
    for idx, op in enumerate(lowered):
        for name in op.get("args", []) or []:
            if name != "none" and name not in defined:
                missing.append((idx, str(op.get("kind")), name))
        out = op.get("out")
        if isinstance(out, str) and out != "none":
            defined.add(out)
    return missing


@contextmanager
def _temp_env(name: str, value: str) -> object:
    prior = os.environ.get(name)
    os.environ[name] = value
    try:
        yield
    finally:
        if prior is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = prior


def _build_sccp_growth_ops(depth: int, *, constant_cond: bool | None) -> list[MoltOp]:
    ops: list[MoltOp] = [MoltOp(kind="CONST", args=[0], result=MoltValue("acc"))]
    for idx in range(depth):
        cond_name = f"cond_{idx}"
        if constant_cond is None:
            ops.append(MoltOp(kind="MISSING", args=[], result=MoltValue(cond_name)))
        else:
            ops.append(
                MoltOp(
                    kind="CONST_BOOL",
                    args=[constant_cond],
                    result=MoltValue(cond_name),
                )
            )
        ops.append(
            MoltOp(kind="IF", args=[MoltValue(cond_name)], result=MoltValue("none"))
        )
        one_name = f"one_{idx}"
        two_name = f"two_{idx}"
        ops.append(MoltOp(kind="CONST", args=[1], result=MoltValue(one_name)))
        ops.append(
            MoltOp(
                kind="ADD",
                args=[MoltValue("acc"), MoltValue(one_name)],
                result=MoltValue("acc"),
            )
        )
        ops.append(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        ops.append(MoltOp(kind="CONST", args=[2], result=MoltValue(two_name)))
        ops.append(
            MoltOp(
                kind="ADD",
                args=[MoltValue("acc"), MoltValue(two_name)],
                result=MoltValue("acc"),
            )
        )
        ops.append(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
    ops.append(MoltOp(kind="RETURN", args=[MoltValue("acc")], result=MoltValue("none")))
    return ops


def _eval_simple_ops(ops: list[MoltOp]) -> int | None:
    env: dict[str, object] = {}
    label_to_pc: dict[str, int] = {}
    if_to_else: dict[int, int] = {}
    if_to_end: dict[int, int] = {}
    else_to_end: dict[int, int] = {}
    if_stack: list[int] = []

    for idx, op in enumerate(ops):
        if op.kind in {"LABEL", "STATE_LABEL"} and op.args:
            label_to_pc[str(op.args[0])] = idx
        if op.kind == "IF":
            if_stack.append(idx)
        elif op.kind == "ELSE":
            if if_stack:
                if_to_else[if_stack[-1]] = idx
        elif op.kind == "END_IF":
            if if_stack:
                if_idx = if_stack.pop()
                if_to_end[if_idx] = idx
                else_idx = if_to_else.get(if_idx)
                if else_idx is not None:
                    else_to_end[else_idx] = idx

    pc = 0
    step_cap = max(10_000, len(ops) * 256)
    steps = 0
    while 0 <= pc < len(ops):
        steps += 1
        assert steps <= step_cap, "simple evaluator exceeded step cap"
        op = ops[pc]
        if op.kind in {"LINE", "LABEL", "STATE_LABEL", "END_IF"}:
            pc += 1
            continue
        if op.kind == "CONST":
            env[op.result.name] = int(op.args[0])
            pc += 1
            continue
        if op.kind == "CONST_BOOL":
            env[op.result.name] = bool(op.args[0])
            pc += 1
            continue
        if op.kind == "MISSING":
            env[op.result.name] = None
            pc += 1
            continue
        if op.kind == "ADD":
            lhs = int(env[op.args[0].name])
            rhs = int(env[op.args[1].name])
            env[op.result.name] = lhs + rhs
            pc += 1
            continue
        if op.kind == "IF":
            cond = bool(env[op.args[0].name])
            if cond:
                pc += 1
            else:
                false_idx = if_to_else.get(pc, if_to_end.get(pc))
                assert false_idx is not None
                pc = false_idx + 1
            continue
        if op.kind == "ELSE":
            end_idx = else_to_end.get(pc)
            assert end_idx is not None
            pc = end_idx + 1
            continue
        if op.kind == "JUMP":
            target = str(op.args[0]) if op.args else ""
            assert target in label_to_pc
            pc = label_to_pc[target] + 1
            continue
        if op.kind == "RETURN":
            if not op.args:
                return None
            ret = op.args[0]
            if isinstance(ret, MoltValue):
                return int(env[ret.name])
            return int(ret)
        raise AssertionError(f"unsupported op in simple evaluator: {op.kind}")
    raise AssertionError("simple evaluator reached end without RETURN")


def test_trivial_phi_elides_and_rewrites_users() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[3], result=MoltValue("x")),
            MoltOp(
                kind="PHI",
                args=[MoltValue("x"), MoltValue("x")],
                result=MoltValue("y"),
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
            MoltOp(
                kind="ADD",
                args=[MoltValue("y"), MoltValue("one")],
                result=MoltValue("sum"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("sum")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") != "phi" for op in lowered)
    add = next(op for op in lowered if op.get("kind") == "add")
    assert add["args"][0] == "x"


def test_phi_edge_trim_collapses_duplicate_executable_inputs() -> None:
    gen = SimpleTIRGenerator()
    ops = [
        MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("cond")),
        MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("a")),
        MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[2], result=MoltValue("b")),
        MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
        MoltOp(
            kind="PHI",
            args=[MoltValue("a"), MoltValue("a")],
            result=MoltValue("joined"),
        ),
    ]
    cfg = build_cfg(ops)
    sccp = gen._compute_sccp(ops, cfg)
    trimmed, count = gen._trim_phi_args_by_executable_edges(
        ops, cfg, sccp.executable_edges
    )
    phi = next(op for op in trimmed if op.kind == "PHI")
    assert count >= 1
    assert len(phi.args) == 1


def test_guard_tag_elides_when_tag_is_proven() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
            MoltOp(
                kind="ADD",
                args=[MoltValue("value"), MoltValue("one")],
                result=MoltValue("sum"),
            ),
        ]
    )

    assert all(op.get("kind") != "guard_tag" for op in lowered)


def test_if_join_preserves_proven_tag_facts_across_branches() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(kind="MISSING", args=[], result=MoltValue("cond")),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(kind="CONST_STR", args=["then"], result=MoltValue("branch_msg_a")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST_STR", args=["else"], result=MoltValue("branch_msg_b")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
            MoltOp(
                kind="ADD",
                args=[MoltValue("value"), MoltValue("one")],
                result=MoltValue("sum"),
            ),
        ]
    )

    assert all(op.get("kind") != "guard_tag" for op in lowered)


def test_loop_join_elides_guard_when_backedge_preserves_type_fact() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST_BOOL", args=[False], result=MoltValue("loop_done")),
            MoltOp(
                kind="LOOP_BREAK_IF_TRUE",
                args=[MoltValue("loop_done")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")),
            MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
            MoltOp(
                kind="ADD",
                args=[MoltValue("value"), MoltValue("one")],
                result=MoltValue("sum"),
            ),
        ]
    )

    assert any(op.get("kind") == "loop_start" for op in lowered)
    assert any(op.get("kind") == "loop_end" for op in lowered)
    assert all(op.get("kind") != "guard_tag" for op in lowered)


def test_loop_join_keeps_guard_when_body_changes_type_fact() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST_FLOAT", args=[1.5], result=MoltValue("value")),
            MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
        ]
    )

    assert any(op.get("kind") == "guard_tag" for op in lowered)


def test_try_join_preserves_proven_tag_facts_when_body_keeps_type() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST_STR", args=["ok"], result=MoltValue("tmp")),
            MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
        ]
    )

    assert all(op.get("kind") != "guard_tag" for op in lowered)


def test_label_jump_join_preserves_facts_for_guard_elision() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(kind="JUMP", args=[1], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[1], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
        ]
    )

    # CFG simplification may erase degenerate jump/label scaffolding.
    assert all(op.get("kind") != "guard_tag" for op in lowered)


def test_cfg_const_dedupe_reuses_existing_constant_value() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[7], result=MoltValue("a")),
            MoltOp(kind="CONST", args=[7], result=MoltValue("b")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
            MoltOp(
                kind="ADD",
                args=[MoltValue("b"), MoltValue("one")],
                result=MoltValue("sum"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("sum")], result=MoltValue("none")),
        ]
    )

    const_sevens = [
        op for op in lowered if op.get("kind") == "const" and op.get("value") == 7
    ]
    assert len(const_sevens) == 1
    add = next(op for op in lowered if op.get("kind") == "add")
    assert add["args"][0] == "a"


def test_cfg_dead_const_elimination_removes_unused_constants() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[99], result=MoltValue("unused")),
            MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("flag")),
            MoltOp(kind="NOT", args=[MoltValue("flag")], result=MoltValue("out")),
        ]
    )

    assert all(op.get("value") != 99 for op in lowered if op.get("kind") == "const")


def test_cfg_const_dedupe_keeps_check_exception_users_defined() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST_STR", args=["f.py"], result=MoltValue("filename")),
            MoltOp(kind="CONST_STR", args=["func"], result=MoltValue("name")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("firstline")),
            MoltOp(kind="CONST_NONE", args=[], result=MoltValue("linetable")),
            MoltOp(kind="TUPLE_NEW", args=[], result=MoltValue("varnames")),
            MoltOp(kind="CONST", args=[0], result=MoltValue("argcount")),
            MoltOp(kind="CONST", args=[0], result=MoltValue("posonly")),
            MoltOp(kind="CONST", args=[0], result=MoltValue("kwonly")),
            MoltOp(kind="CHECK_EXCEPTION", args=[1], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[1], result=MoltValue("none")),
            MoltOp(
                kind="CODE_NEW",
                args=[
                    MoltValue("filename"),
                    MoltValue("name"),
                    MoltValue("firstline"),
                    MoltValue("linetable"),
                    MoltValue("varnames"),
                    MoltValue("argcount"),
                    MoltValue("posonly"),
                    MoltValue("kwonly"),
                ],
                result=MoltValue("code"),
            ),
        ]
    )

    assert _undefined_args(lowered) == []


def test_cfg_gvn_reuses_pure_int_arithmetic() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[2], result=MoltValue("a")),
            MoltOp(kind="CONST", args=[3], result=MoltValue("b")),
            MoltOp(
                kind="ADD", args=[MoltValue("a"), MoltValue("b")], result=MoltValue("x")
            ),
            MoltOp(
                kind="ADD", args=[MoltValue("a"), MoltValue("b")], result=MoltValue("y")
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
            MoltOp(
                kind="ADD",
                args=[MoltValue("y"), MoltValue("one")],
                result=MoltValue("sum"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("sum")], result=MoltValue("none")),
        ]
    )

    adds = [
        op for op in lowered if op.get("kind") == "add" and op.get("args") == ["a", "b"]
    ]
    assert len(adds) == 1
    final_add = [op for op in lowered if op.get("kind") == "add"][-1]
    assert final_add["args"][0] == "x"


def test_cfg_dedupes_redundant_guard_tag_after_first_guard() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
        ]
    )

    guards = [op for op in lowered if op.get("kind") == "guard_tag"]
    assert len(guards) == 1


def test_cfg_dedupes_redundant_guard_dict_shape_after_first_guard() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="DICT_NEW", args=[], result=MoltValue("obj")),
            MoltOp(kind="MISSING", args=[], result=MoltValue("dict_type")),
            MoltOp(kind="CONST", args=[0], result=MoltValue("shape_ver")),
            MoltOp(
                kind="GUARD_DICT_SHAPE",
                args=[
                    MoltValue("obj"),
                    MoltValue("dict_type"),
                    MoltValue("shape_ver"),
                ],
                result=MoltValue("guard_a"),
            ),
            MoltOp(
                kind="GUARD_DICT_SHAPE",
                args=[
                    MoltValue("obj"),
                    MoltValue("dict_type"),
                    MoltValue("shape_ver"),
                ],
                result=MoltValue("guard_b"),
            ),
        ]
    )

    guards = [op for op in lowered if op.get("kind") == "guard_dict_shape"]
    assert len(guards) == 1


def test_sccp_prunes_constant_if_else_region() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("cond")),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[11], result=MoltValue("kept")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[99], result=MoltValue("dropped")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"if", "else", "end_if"} for op in lowered)
    kept = [op for op in lowered if op.get("kind") == "const" and op.get("value") == 11]
    dropped = [
        op for op in lowered if op.get("kind") == "const" and op.get("value") == 99
    ]
    assert len(kept) == 1
    assert len(dropped) == 0


def test_sccp_folds_comparison_condition_for_branch_pruning() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[2], result=MoltValue("lhs")),
            MoltOp(kind="CONST", args=[3], result=MoltValue("rhs")),
            MoltOp(
                kind="LT",
                args=[MoltValue("lhs"), MoltValue("rhs")],
                result=MoltValue("cond"),
            ),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[5], result=MoltValue("kept")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[999], result=MoltValue("dead")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"if", "else", "end_if"} for op in lowered)
    assert all(op.get("value") != 999 for op in lowered if op.get("kind") == "const")


def test_sccp_type_of_eq_chain_prunes_branch() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[11], result=MoltValue("value")),
            MoltOp(
                kind="TYPE_OF", args=[MoltValue("value")], result=MoltValue("value_tag")
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(
                kind="EQ",
                args=[MoltValue("value_tag"), MoltValue("int_tag")],
                result=MoltValue("cond"),
            ),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("kept")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[77], result=MoltValue("dead")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"if", "else", "end_if"} for op in lowered)
    assert all(op.get("value") != 77 for op in lowered if op.get("kind") == "const")


def test_guard_hoist_moves_duplicate_branch_guards_to_dominator() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("cond")),
            MoltOp(kind="MISSING", args=[], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("a")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[2], result=MoltValue("b")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
        ]
    )

    guards = [op for op in lowered if op.get("kind") == "guard_tag"]
    assert len(guards) == 1


def test_dce_lattice_keeps_guard_results_even_when_unused() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="DICT_NEW", args=[], result=MoltValue("obj")),
            MoltOp(kind="MISSING", args=[], result=MoltValue("dict_type")),
            MoltOp(kind="CONST", args=[0], result=MoltValue("shape_ver")),
            MoltOp(
                kind="GUARD_DICT_SHAPE",
                args=[MoltValue("obj"), MoltValue("dict_type"), MoltValue("shape_ver")],
                result=MoltValue("guard_result"),
            ),
        ]
    )

    assert any(op.get("kind") == "guard_dict_shape" for op in lowered)


def test_cfg_gvn_reuses_type_of_and_is() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("obj")),
            MoltOp(kind="TYPE_OF", args=[MoltValue("obj")], result=MoltValue("t1")),
            MoltOp(kind="TYPE_OF", args=[MoltValue("obj")], result=MoltValue("t2")),
            MoltOp(
                kind="IS",
                args=[MoltValue("t1"), MoltValue("t2")],
                result=MoltValue("cmp"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("cmp")], result=MoltValue("none")),
        ]
    )

    type_ofs = [op for op in lowered if op.get("kind") == "type_of"]
    assert len(type_ofs) == 1


def test_cfg_prunes_unreachable_loop_region_after_jump() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="JUMP", args=[2], result=MoltValue("none")),
            MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[99], result=MoltValue("dead")),
            MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[2], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[7], result=MoltValue("kept")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"loop_start", "loop_end"} for op in lowered)
    assert all(op.get("value") != 99 for op in lowered if op.get("kind") == "const")


def test_cfg_prunes_unreachable_try_region_after_jump() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="JUMP", args=[3], result=MoltValue("none")),
            MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[101], result=MoltValue("dead")),
            MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[3], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[5], result=MoltValue("kept")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"try_start", "try_end"} for op in lowered)
    assert all(op.get("value") != 101 for op in lowered if op.get("kind") == "const")


def test_cfg_prunes_noop_jump_and_dead_label() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="JUMP", args=[1], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[1], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
            MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") != "jump" for op in lowered)
    assert all(op.get("kind") != "label" for op in lowered)


def test_sccp_threads_loop_break_if_edges() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("cond")),
            MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
            MoltOp(
                kind="LOOP_BREAK_IF_TRUE",
                args=[MoltValue("cond")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[123], result=MoltValue("dead_inside_loop")),
            MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[9], result=MoltValue("out")),
            MoltOp(kind="RETURN", args=[MoltValue("out")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") != "loop_break_if_true" for op in lowered)
    assert any(op.get("kind") == "loop_break" for op in lowered)
    assert all(op.get("value") != 123 for op in lowered if op.get("kind") == "const")


def test_loop_bound_solver_extracts_monotonic_tuple_and_proof() -> None:
    ops = [
        MoltOp(kind="CONST", args=[10], result=MoltValue("start")),
        MoltOp(kind="CONST", args=[5], result=MoltValue("bound")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("step")),
        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
        MoltOp(
            kind="PHI",
            args=[MoltValue("start"), MoltValue("next_i")],
            result=MoltValue("i"),
        ),
        MoltOp(
            kind="LT",
            args=[MoltValue("i"), MoltValue("bound")],
            result=MoltValue("cond"),
        ),
        MoltOp(
            kind="LOOP_BREAK_IF_FALSE",
            args=[MoltValue("cond")],
            result=MoltValue("none"),
        ),
        MoltOp(
            kind="ADD",
            args=[MoltValue("i"), MoltValue("step")],
            result=MoltValue("next_i"),
        ),
        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    facts = gen._analyze_loop_bound_facts(ops, cfg)
    assert facts
    fact = next(iter(facts.values()))
    assert fact.start == 10
    assert fact.step == 1
    assert fact.bound == 5
    assert fact.compare_op == "LT"
    assert gen._prove_monotonic_loop_compare(fact) is False


def test_cfg_models_check_exception_target_edge() -> None:
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CHECK_EXCEPTION", args=[7], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[7], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
    ]
    cfg = build_cfg(ops)
    check_block = cfg.index_to_block[1]
    label_block = cfg.label_to_block["7"]
    assert label_block in cfg.successors.get(check_block, [])


def test_cfg_tracks_try_end_to_start_and_block_entry_label() -> None:
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[11], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
    ]
    cfg = build_cfg(ops)
    assert cfg.control.try_start_to_end.get(0) == 2
    assert cfg.control.try_end_to_start.get(2) == 0
    label_block = cfg.label_to_block["11"]
    assert cfg.block_entry_label.get(label_block) == "11"


def test_cfg_precanonicalizer_aligns_phi_to_predecessor_shape() -> None:
    ops = [
        MoltOp(kind="MISSING", args=[], result=MoltValue("cond")),
        MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[2], result=MoltValue("y")),
        MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
        MoltOp(kind="PHI", args=[MoltValue("x")], result=MoltValue("z")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    aligned, rewrites = gen._align_phi_args_to_cfg_predecessors(ops, cfg)
    phi = next(op for op in aligned if op.kind == "PHI")
    assert rewrites >= 1
    assert len(phi.args) == len(cfg.predecessors[cfg.index_to_block[6]])


def test_cfg_precanonicalizer_threads_ladders_before_rounds() -> None:
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CHECK_EXCEPTION", args=[10], result=MoltValue("none")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[40], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[10], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[20], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[20], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[30], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[30], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="JUMP", args=[40], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[40], result=MoltValue("none")),
    ]
    gen = SimpleTIRGenerator()
    rewritten, rewrites = gen._canonicalize_cfg_before_optimization(ops)
    assert rewrites >= 1
    checks = [op for op in rewritten if op.kind == "CHECK_EXCEPTION"]
    assert checks and checks[0].args[0] == 30


def test_structural_cfg_validator_canonicalizes_unbalanced_regions() -> None:
    gen = SimpleTIRGenerator()
    ops = [
        MoltOp(kind="MISSING", args=[], result=MoltValue("cond")),
        MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[2], result=MoltValue("y")),
        MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
    ]

    rewritten, rewrites = gen._ensure_structural_cfg_validity(ops, stage="unit_test")
    kinds = [op.kind for op in rewritten]
    assert rewrites >= 1
    assert kinds.count("IF") == kinds.count("END_IF")
    assert kinds.count("LOOP_START") == kinds.count("LOOP_END")


def test_structural_cfg_validator_rejects_missing_check_exception_target() -> None:
    gen = SimpleTIRGenerator()
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CHECK_EXCEPTION", args=[404], result=MoltValue("none")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[], result=MoltValue("none")),
    ]

    with pytest.raises(RuntimeError, match="unknown label"):
        gen._ensure_structural_cfg_validity(ops, stage="unit_test")


def test_range_loop_lowering_keeps_loop_index_control_within_loop_markers() -> None:
    source = "for i in range(3):\n    pass\n"
    gen = SimpleTIRGenerator(module_name="unit_test")
    gen.visit(ast.parse(source))
    ops = gen.funcs_map["molt_main"]["ops"]

    assert any(op.kind == "LOOP_INDEX_START" for op in ops)
    gen._ensure_structural_cfg_validity(ops, stage="unit_test")


def test_try_check_exception_threading_rewrites_to_jump_with_dominance_proof() -> None:
    ops = [
        MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
        MoltOp(kind="CONST", args=[2], result=MoltValue("float_tag")),
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(
            kind="GUARD_TAG",
            args=[MoltValue("value"), MoltValue("float_tag")],
            result=MoltValue("none"),
        ),
        MoltOp(kind="LINE", args=[1], result=MoltValue("none")),
        MoltOp(kind="CHECK_EXCEPTION", args=[99], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[333], result=MoltValue("dead_after_check")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[99], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("value")], result=MoltValue("none")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    sccp = gen._compute_sccp(ops, cfg)
    (
        rewritten,
        _loop_rewrites,
        _try_marker_prunes,
        _loop_marker_prunes,
        _try_body_prunes,
        check_threads,
        _check_elisions,
    ) = gen._rewrite_loop_try_edge_threading(
        ops,
        cfg=cfg,
        control=cfg.control,
        executable_edges=sccp.executable_edges,
        loop_break_choice_by_index=sccp.loop_break_choice_by_index,
        try_exception_possible_by_start=sccp.try_exception_possible_by_start,
        try_normal_possible_by_start=sccp.try_normal_possible_by_start,
        guard_fail_indices=sccp.guard_fail_indices,
    )
    assert check_threads >= 1
    assert any(op.kind == "JUMP" and op.args and op.args[0] == 99 for op in rewritten)
    assert all(op.kind != "CHECK_EXCEPTION" for op in rewritten)


def test_loop_break_if_true_threads_to_jump_when_exit_has_label() -> None:
    ops = [
        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("cond")),
        MoltOp(
            kind="LOOP_BREAK_IF_TRUE",
            args=[MoltValue("cond")],
            result=MoltValue("none"),
        ),
        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[77], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    sccp = gen._compute_sccp(ops, cfg)
    rewritten, *_rest = gen._rewrite_loop_try_edge_threading(
        ops,
        cfg=cfg,
        control=cfg.control,
        executable_edges=sccp.executable_edges,
        loop_break_choice_by_index=sccp.loop_break_choice_by_index,
        try_exception_possible_by_start=sccp.try_exception_possible_by_start,
        try_normal_possible_by_start=sccp.try_normal_possible_by_start,
        guard_fail_indices=sccp.guard_fail_indices,
    )
    assert any(
        op.kind == "JUMP" and op.args and str(op.args[0]) == "77" for op in rewritten
    )


def test_loop_break_if_true_threads_through_exit_label_trampoline() -> None:
    ops = [
        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("cond")),
        MoltOp(
            kind="LOOP_BREAK_IF_TRUE",
            args=[MoltValue("cond")],
            result=MoltValue("none"),
        ),
        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[77], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[88], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[88], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    sccp = gen._compute_sccp(ops, cfg)
    rewritten, *_rest = gen._rewrite_loop_try_edge_threading(
        ops,
        cfg=cfg,
        control=cfg.control,
        executable_edges=sccp.executable_edges,
        loop_break_choice_by_index=sccp.loop_break_choice_by_index,
        try_exception_possible_by_start=sccp.try_exception_possible_by_start,
        try_normal_possible_by_start=sccp.try_normal_possible_by_start,
        guard_fail_indices=sccp.guard_fail_indices,
    )
    assert any(
        op.kind == "JUMP" and op.args and str(op.args[0]) == "88" for op in rewritten
    )


def test_try_check_exception_threads_through_nested_label_trampoline() -> None:
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(kind="MISSING", args=[], result=MoltValue("container")),
        MoltOp(kind="MISSING", args=[], result=MoltValue("index")),
        MoltOp(
            kind="INDEX",
            args=[MoltValue("container"), MoltValue("index")],
            result=MoltValue("value"),
        ),
        MoltOp(kind="CHECK_EXCEPTION", args=[10], result=MoltValue("none")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[90], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[10], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[20], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[20], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[30], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[30], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("value")], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[90], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("value")], result=MoltValue("none")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    sccp = gen._compute_sccp(ops, cfg)
    rewritten, *_stats = gen._rewrite_loop_try_edge_threading(
        ops,
        cfg=cfg,
        control=cfg.control,
        executable_edges=sccp.executable_edges,
        loop_break_choice_by_index=sccp.loop_break_choice_by_index,
        try_exception_possible_by_start=sccp.try_exception_possible_by_start,
        try_normal_possible_by_start=sccp.try_normal_possible_by_start,
        guard_fail_indices=sccp.guard_fail_indices,
    )
    rewritten_check = next(op for op in rewritten if op.kind == "CHECK_EXCEPTION")
    assert rewritten_check.args and str(rewritten_check.args[0]) == "30"


def test_try_except_join_normalization_threads_nested_label_trampolines() -> None:
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CHECK_EXCEPTION", args=[10], result=MoltValue("none")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[40], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[10], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[20], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[20], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[30], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[30], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="JUMP", args=[40], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[40], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    rewritten, rewrites = gen._normalize_try_except_join_labels(ops, cfg=cfg)
    assert rewrites >= 2
    check = next(op for op in rewritten if op.kind == "CHECK_EXCEPTION")
    assert check.args[0] == 30
    threaded_jumps = [op for op in rewritten if op.kind == "JUMP" and op.args]
    assert any(op.args[0] == 30 for op in threaded_jumps)


def test_nested_try_except_join_normalization_runs_before_cse_rounds() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[0], result=MoltValue("x")),
            MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CHECK_EXCEPTION", args=[10], result=MoltValue("none")),
            MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
            MoltOp(kind="JUMP", args=[40], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[10], result=MoltValue("none")),
            MoltOp(kind="JUMP", args=[20], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[20], result=MoltValue("none")),
            MoltOp(kind="JUMP", args=[30], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[30], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("y")),
            MoltOp(kind="JUMP", args=[40], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[40], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
        ]
    )

    checks = [op for op in lowered if op.get("kind") == "check_exception"]
    assert checks
    assert checks[0].get("value") == 30
    labels = [op.get("value") for op in lowered if op.get("kind") == "label"]
    assert 10 not in labels
    assert 20 not in labels


def test_try_except_join_normalization_threads_deep_check_ladders() -> None:
    ops = [
        MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
        MoltOp(kind="CHECK_EXCEPTION", args=[10], result=MoltValue("none")),
        MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[90], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[10], result=MoltValue("none")),
        MoltOp(kind="CHECK_EXCEPTION", args=[20], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[20], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[20], result=MoltValue("none")),
        MoltOp(kind="JUMP", args=[30], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[30], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
        MoltOp(kind="JUMP", args=[90], result=MoltValue("none")),
        MoltOp(kind="LABEL", args=[90], result=MoltValue("none")),
        MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
    ]
    gen = SimpleTIRGenerator()
    cfg = build_cfg(ops)
    rewritten, rewrites = gen._normalize_try_except_join_labels(ops, cfg=cfg)
    assert rewrites >= 3
    head_check = next(
        op for op in rewritten if op.kind == "CHECK_EXCEPTION" and op.args
    )
    assert head_check.args[0] == 30
    assert any(op.kind == "JUMP" and op.args and op.args[0] == 30 for op in rewritten)


def test_region_wide_guard_elision_removes_post_join_duplicate_guard() -> None:
    ops = [
        MoltOp(kind="MISSING", args=[], result=MoltValue("cond")),
        MoltOp(kind="MISSING", args=[], result=MoltValue("value")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("tag")),
        MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
        MoltOp(
            kind="GUARD_TAG",
            args=[MoltValue("value"), MoltValue("tag")],
            result=MoltValue("none"),
        ),
        MoltOp(kind="CONST", args=[1], result=MoltValue("a")),
        MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
        MoltOp(
            kind="GUARD_TAG",
            args=[MoltValue("value"), MoltValue("tag")],
            result=MoltValue("none"),
        ),
        MoltOp(kind="CONST", args=[2], result=MoltValue("b")),
        MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
        MoltOp(
            kind="GUARD_TAG",
            args=[MoltValue("value"), MoltValue("tag")],
            result=MoltValue("none"),
        ),
    ]
    gen = SimpleTIRGenerator()
    rewritten, attempted, accepted, rejected = gen._eliminate_redundant_guards_cfg(ops)
    guards = [op for op in rewritten if op.kind == "GUARD_TAG"]
    assert len(guards) == 2
    assert attempted >= 3
    assert accepted >= 1
    assert rejected == attempted - accepted


def test_sccp_prunes_non_raising_try_markers() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[5], result=MoltValue("x")),
            MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"try_start", "try_end"} for op in lowered)


def test_try_exception_edge_prunes_dead_suffix_after_proven_guard_failure() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="CONST", args=[2], result=MoltValue("float_tag")),
            MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("float_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[111], result=MoltValue("dead_after_guard")),
            MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
        ]
    )

    assert any(op.get("kind") == "guard_tag" for op in lowered)
    assert all(op.get("value") != 111 for op in lowered if op.get("kind") == "const")
    assert all(op.get("kind") not in {"try_start", "try_end"} for op in lowered)
    assert all(op.get("kind") != "check_exception" for op in lowered)


def test_try_exception_edge_keeps_markers_when_check_exception_is_pretrap() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST_NONE", args=[], result=MoltValue("exc")),
            MoltOp(kind="TRY_START", args=[], result=MoltValue("none")),
            MoltOp(kind="CHECK_EXCEPTION", args=[99], result=MoltValue("none")),
            MoltOp(kind="RAISE", args=[MoltValue("exc")], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[404], result=MoltValue("dead_after_raise")),
            MoltOp(kind="TRY_END", args=[], result=MoltValue("none")),
            MoltOp(kind="LABEL", args=[99], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("exc")], result=MoltValue("none")),
        ]
    )

    try_starts = [op for op in lowered if op.get("kind") == "try_start"]
    try_ends = [op for op in lowered if op.get("kind") == "try_end"]
    assert len(try_starts) == 1
    assert len(try_ends) == 1
    assert all(op.get("value") != 404 for op in lowered if op.get("kind") == "const")


def test_sccp_folds_range_index_condition() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[range(0, 5)], result=MoltValue("r")),
            MoltOp(kind="CONST", args=[2], result=MoltValue("idx")),
            MoltOp(
                kind="INDEX",
                args=[MoltValue("r"), MoltValue("idx")],
                result=MoltValue("elem"),
            ),
            MoltOp(kind="CONST", args=[2], result=MoltValue("expected")),
            MoltOp(
                kind="EQ",
                args=[MoltValue("elem"), MoltValue("expected")],
                result=MoltValue("cond"),
            ),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("kept")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[999], result=MoltValue("dead")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"if", "else", "end_if"} for op in lowered)
    assert all(op.get("value") != 999 for op in lowered if op.get("kind") == "const")


def test_sccp_uses_type_of_eq_implication_for_branch_fold() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[10], result=MoltValue("value")),
            MoltOp(kind="TYPE_OF", args=[MoltValue("value")], result=MoltValue("tagv")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("int_tag")),
            MoltOp(
                kind="EQ",
                args=[MoltValue("tagv"), MoltValue("int_tag")],
                result=MoltValue("ok"),
            ),
            MoltOp(kind="IF", args=[MoltValue("ok")], result=MoltValue("none")),
            MoltOp(
                kind="GUARD_TAG",
                args=[MoltValue("value"), MoltValue("int_tag")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[1], result=MoltValue("kept")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[999], result=MoltValue("dead")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") != "guard_tag" for op in lowered)
    assert all(op.get("value") != 999 for op in lowered if op.get("kind") == "const")


def test_branch_tail_merge_collapses_identical_line_suffix() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("cond")),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(kind="LINE", args=[100], result=MoltValue("none")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="LINE", args=[100], result=MoltValue("none")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("x")),
            MoltOp(kind="RETURN", args=[MoltValue("x")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"if", "else", "end_if"} for op in lowered)
    lines = [
        op for op in lowered if op.get("kind") == "line" and op.get("value") == 100
    ]
    assert len(lines) == 1


def test_effect_aware_cse_reuses_heap_read_len_without_writes() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("obj")),
            MoltOp(kind="LEN", args=[MoltValue("obj")], result=MoltValue("l1")),
            MoltOp(kind="LEN", args=[MoltValue("obj")], result=MoltValue("l2")),
            MoltOp(kind="RETURN", args=[MoltValue("l2")], result=MoltValue("none")),
        ]
    )

    lens = [op for op in lowered if op.get("kind") == "len"]
    assert len(lens) == 1


def test_effect_aware_cse_does_not_reuse_heap_read_across_call_boundary() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("obj")),
            MoltOp(kind="LEN", args=[MoltValue("obj")], result=MoltValue("l1")),
            MoltOp(kind="CALL_INTERNAL", args=["unknown_fn"], result=MoltValue("tmp")),
            MoltOp(kind="LEN", args=[MoltValue("obj")], result=MoltValue("l2")),
            MoltOp(kind="RETURN", args=[MoltValue("l2")], result=MoltValue("none")),
        ]
    )

    lens = [op for op in lowered if op.get("kind") == "len"]
    assert len(lens) == 2


def test_effect_aware_cse_invalidates_immutable_len_across_call_boundary() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[(1, 2, 3)], result=MoltValue("t")),
            MoltOp(kind="LEN", args=[MoltValue("t")], result=MoltValue("l1")),
            MoltOp(kind="CALL_INTERNAL", args=["unknown_fn"], result=MoltValue("tmp")),
            MoltOp(kind="LEN", args=[MoltValue("t")], result=MoltValue("l2")),
            MoltOp(kind="RETURN", args=[MoltValue("l2")], result=MoltValue("none")),
        ]
    )

    lens = [op for op in lowered if op.get("kind") == "len"]
    assert len(lens) == 2


def test_effect_alias_classes_keep_dict_len_cse_across_list_write() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="DICT_NEW", args=[], result=MoltValue("d")),
            MoltOp(kind="LEN", args=[MoltValue("d")], result=MoltValue("l1")),
            MoltOp(kind="LIST_NEW", args=[], result=MoltValue("l")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
            MoltOp(
                kind="LIST_APPEND",
                args=[MoltValue("l"), MoltValue("one")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="LEN", args=[MoltValue("d")], result=MoltValue("l2")),
            MoltOp(kind="RETURN", args=[MoltValue("l2")], result=MoltValue("none")),
        ]
    )

    lens = [op for op in lowered if op.get("kind") == "len"]
    assert len(lens) == 1


def test_effect_alias_classes_keep_list_index_cse_across_dict_write() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="LIST_NEW", args=[], result=MoltValue("lst")),
            MoltOp(kind="CONST", args=[41], result=MoltValue("item")),
            MoltOp(
                kind="LIST_APPEND",
                args=[MoltValue("lst"), MoltValue("item")],
                result=MoltValue("none"),
            ),
            MoltOp(kind="CONST", args=[0], result=MoltValue("idx")),
            MoltOp(
                kind="INDEX",
                args=[MoltValue("lst"), MoltValue("idx")],
                result=MoltValue("a"),
            ),
            MoltOp(kind="DICT_NEW", args=[], result=MoltValue("d")),
            MoltOp(
                kind="DICT_SET",
                args=[MoltValue("d"), MoltValue("idx"), MoltValue("item")],
                result=MoltValue("none"),
            ),
            MoltOp(
                kind="INDEX",
                args=[MoltValue("lst"), MoltValue("idx")],
                result=MoltValue("b"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("b")], result=MoltValue("none")),
        ]
    )

    indexes = [op for op in lowered if op.get("kind") == "index"]
    assert len(indexes) == 1


def test_effect_aware_cse_reuses_module_get_attr_without_writes() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("mod")),
            MoltOp(kind="CONST_STR", args=["x"], result=MoltValue("name")),
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[MoltValue("mod"), MoltValue("name")],
                result=MoltValue("a"),
            ),
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[MoltValue("mod"), MoltValue("name")],
                result=MoltValue("b"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("b")], result=MoltValue("none")),
        ]
    )

    reads = [op for op in lowered if op.get("kind") == "module_get_attr"]
    assert len(reads) == 1


def test_effect_aware_cse_reuses_getattr_generic_obj_without_writes() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("obj")),
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[MoltValue("obj"), "value"],
                result=MoltValue("a"),
            ),
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[MoltValue("obj"), "value"],
                result=MoltValue("b"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("b")], result=MoltValue("none")),
        ]
    )

    reads = [op for op in lowered if op.get("kind") == "get_attr_generic_obj"]
    assert len(reads) == 1


def test_effect_aware_cse_reuses_getattr_name_without_writes() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="MISSING", args=[], result=MoltValue("obj")),
            MoltOp(kind="CONST_STR", args=["x"], result=MoltValue("name")),
            MoltOp(
                kind="GETATTR_NAME",
                args=[MoltValue("obj"), MoltValue("name")],
                result=MoltValue("a"),
            ),
            MoltOp(
                kind="GETATTR_NAME",
                args=[MoltValue("obj"), MoltValue("name")],
                result=MoltValue("b"),
            ),
            MoltOp(kind="RETURN", args=[MoltValue("b")], result=MoltValue("none")),
        ]
    )

    reads = [op for op in lowered if op.get("kind") == "get_attr_name"]
    assert len(reads) == 1


def test_sccp_folds_contains_constant_branch() -> None:
    lowered = _lower_ops(
        [
            MoltOp(kind="CONST", args=[[1, 2, 3]], result=MoltValue("container")),
            MoltOp(kind="CONST", args=[2], result=MoltValue("needle")),
            MoltOp(
                kind="CONTAINS",
                args=[MoltValue("container"), MoltValue("needle")],
                result=MoltValue("cond"),
            ),
            MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[1], result=MoltValue("kept")),
            MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
            MoltOp(kind="CONST", args=[999], result=MoltValue("dead")),
            MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
            MoltOp(kind="RETURN", args=[MoltValue("kept")], result=MoltValue("none")),
        ]
    )

    assert all(op.get("kind") not in {"if", "else", "end_if"} for op in lowered)
    assert all(op.get("value") != 999 for op in lowered if op.get("kind") == "const")


def test_midend_pipeline_is_idempotent_on_second_round() -> None:
    gen = SimpleTIRGenerator()
    input_ops = [
        MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("cond")),
        MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("a")),
        MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[2], result=MoltValue("b")),
        MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[4], result=MoltValue("c")),
        MoltOp(
            kind="ADD", args=[MoltValue("c"), MoltValue("c")], result=MoltValue("sum")
        ),
        MoltOp(kind="RETURN", args=[MoltValue("sum")], result=MoltValue("none")),
    ]
    first = gen._run_ir_midend_passes(input_ops)
    second = gen._run_ir_midend_passes(first)
    assert first == second


def test_affine_loop_compare_truth_proves_same_iv_offset_compare() -> None:
    gen = SimpleTIRGenerator()
    ops = [
        MoltOp(kind="CONST", args=[0], result=MoltValue("start")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("one")),
        MoltOp(kind="CONST", args=[2], result=MoltValue("two")),
        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
        MoltOp(
            kind="PHI",
            args=[MoltValue("start"), MoltValue("next_i")],
            result=MoltValue("i"),
        ),
        MoltOp(
            kind="ADD",
            args=[MoltValue("i"), MoltValue("one")],
            result=MoltValue("lhs"),
        ),
        MoltOp(
            kind="ADD",
            args=[MoltValue("i"), MoltValue("two")],
            result=MoltValue("rhs"),
        ),
        MoltOp(
            kind="LT",
            args=[MoltValue("lhs"), MoltValue("rhs")],
            result=MoltValue("cond"),
        ),
        MoltOp(
            kind="LOOP_BREAK_IF_FALSE",
            args=[MoltValue("cond")],
            result=MoltValue("none"),
        ),
        MoltOp(
            kind="ADD",
            args=[MoltValue("i"), MoltValue("one")],
            result=MoltValue("next_i"),
        ),
        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
    ]
    cfg = build_cfg(ops)
    proven = gen._analyze_affine_loop_compare_truth(ops, cfg)
    assert 7 in proven
    assert proven[7] is True


def test_licm_hoists_invariant_pure_ops_beyond_loop_prefix() -> None:
    gen = SimpleTIRGenerator()
    ops = [
        MoltOp(kind="CONST", args=[10], result=MoltValue("a")),
        MoltOp(kind="CONST", args=[32], result=MoltValue("b")),
        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")),
        MoltOp(kind="MISSING", args=[], result=MoltValue("cond")),
        MoltOp(
            kind="LOOP_BREAK_IF_TRUE",
            args=[MoltValue("cond")],
            result=MoltValue("none"),
        ),
        MoltOp(
            kind="ADD", args=[MoltValue("a"), MoltValue("b")], result=MoltValue("s")
        ),
        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")),
    ]

    rewritten, hoists = gen._hoist_loop_invariant_pure_ops(ops)
    assert hoists >= 1
    loop_start_idx = next(
        i for i, op in enumerate(rewritten) if op.kind == "LOOP_START"
    )
    add_idx = next(i for i, op in enumerate(rewritten) if op.kind == "ADD")
    assert add_idx < loop_start_idx


def test_definite_assignment_verifier_flags_join_missing_defs() -> None:
    gen = SimpleTIRGenerator()
    ops = [
        MoltOp(kind="CONST_BOOL", args=[True], result=MoltValue("cond")),
        MoltOp(kind="IF", args=[MoltValue("cond")], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[1], result=MoltValue("lhs")),
        MoltOp(kind="ELSE", args=[], result=MoltValue("none")),
        MoltOp(kind="CONST", args=[2], result=MoltValue("rhs")),
        MoltOp(kind="END_IF", args=[], result=MoltValue("none")),
        MoltOp(
            kind="ADD",
            args=[MoltValue("lhs"), MoltValue("rhs")],
            result=MoltValue("sum"),
        ),
    ]

    failures = gen._verify_definite_assignment_in_ops(ops, predefined_value_names=set())
    missing = {(kind, name) for _, kind, name in failures}
    assert ("ADD", "lhs") in missing or ("ADD", "rhs") in missing


def test_midend_telemetry_counters_account_for_expanded_vs_fallback() -> None:
    gen = SimpleTIRGenerator()
    _ = gen.map_ops_to_json(
        [
            MoltOp(kind="CONST", args=[1], result=MoltValue("a")),
            MoltOp(kind="CONST", args=[2], result=MoltValue("b")),
            MoltOp(
                kind="ADD", args=[MoltValue("a"), MoltValue("b")], result=MoltValue("s")
            ),
        ]
    )
    attempts = gen.midend_stats["expanded_attempts"]
    accepted = gen.midend_stats["expanded_accepted"]
    fallbacks = gen.midend_stats["expanded_fallbacks"]
    assert "gvn_hits" in gen.midend_stats
    assert "sccp_branch_prunes" in gen.midend_stats
    assert "loop_edge_thread_prunes" in gen.midend_stats
    assert "try_edge_thread_prunes" in gen.midend_stats
    assert "dce_removed_total" in gen.midend_stats
    assert "cfg_region_prunes" in gen.midend_stats
    assert "label_prunes" in gen.midend_stats
    assert "jump_noop_elisions" in gen.midend_stats
    assert "licm_hoists" in gen.midend_stats
    assert "<direct>" in gen.midend_stats_by_function
    assert "sccp_attempted" in gen.midend_stats_by_function["<direct>"]
    assert "edge_thread_attempted" in gen.midend_stats_by_function["<direct>"]
    assert "edge_thread_rejected" in gen.midend_stats_by_function["<direct>"]
    assert "cse_attempted" in gen.midend_stats_by_function["<direct>"]
    assert "licm_attempted" in gen.midend_stats_by_function["<direct>"]
    assert "loop_rewrite_attempted" in gen.midend_stats_by_function["<direct>"]
    assert "loop_rewrite_rejected" in gen.midend_stats_by_function["<direct>"]
    assert "guard_hoist_rejected" in gen.midend_stats_by_function["<direct>"]
    assert "cse_readheap_attempted" in gen.midend_stats_by_function["<direct>"]
    assert "cse_readheap_rejected" in gen.midend_stats_by_function["<direct>"]
    assert "licm_rejected" in gen.midend_stats_by_function["<direct>"]
    assert "dce_pure_op_attempted" in gen.midend_stats_by_function["<direct>"]
    assert "dce_pure_op_rejected" in gen.midend_stats_by_function["<direct>"]
    assert attempts >= 1
    assert accepted + fallbacks == attempts


def test_midend_fixed_point_round_cap_degrades_without_semantic_failure() -> None:
    gen = SimpleTIRGenerator()
    ops = [MoltOp(kind="CONST", args=[1], result=MoltValue("x"))]
    flip = {"value": False}

    def oscillating_cse(
        self: SimpleTIRGenerator,
        round_ops: list[MoltOp],
        *,
        allow_cross_block_const_dedupe: bool,
        max_cse_iterations_override: int | None = None,
        sccp_iter_cap_override: int | None = None,
    ) -> tuple[list[MoltOp], int]:
        del (
            allow_cross_block_const_dedupe,
            max_cse_iterations_override,
            sccp_iter_cap_override,
        )
        flip["value"] = not flip["value"]
        if flip["value"]:
            return (
                [
                    *round_ops,
                    MoltOp(kind="LINE", args=[777], result=MoltValue("none")),
                ],
                0,
            )
        return (
            [op for op in round_ops if not (op.kind == "LINE" and op.args == [777])],
            0,
        )

    gen._run_cse_canonicalization_round = types.MethodType(  # type: ignore[method-assign]
        oscillating_cse, gen
    )

    rewritten = gen._canonicalize_control_aware_ops_impl(
        ops, allow_cross_block_const_dedupe=True
    )
    assert isinstance(rewritten, list)
    assert gen.midend_stats["fixed_point_fail_fast"] >= 1
    outcome = gen.midend_policy_outcomes_by_function["<direct>"]
    assert outcome["degraded"] is True
    actions = [event.get("action") for event in outcome.get("degrade_events", [])]
    assert "accept_last_verified_round" in actions


def test_midend_fixed_point_round_cap_hard_fail_opt_in() -> None:
    gen = SimpleTIRGenerator()
    ops = [MoltOp(kind="CONST", args=[1], result=MoltValue("x"))]
    flip = {"value": False}

    def oscillating_cse(
        self: SimpleTIRGenerator,
        round_ops: list[MoltOp],
        *,
        allow_cross_block_const_dedupe: bool,
        max_cse_iterations_override: int | None = None,
        sccp_iter_cap_override: int | None = None,
    ) -> tuple[list[MoltOp], int]:
        del (
            allow_cross_block_const_dedupe,
            max_cse_iterations_override,
            sccp_iter_cap_override,
        )
        flip["value"] = not flip["value"]
        if flip["value"]:
            return (
                [
                    *round_ops,
                    MoltOp(kind="LINE", args=[778], result=MoltValue("none")),
                ],
                0,
            )
        return (
            [op for op in round_ops if not (op.kind == "LINE" and op.args == [778])],
            0,
        )

    gen._run_cse_canonicalization_round = types.MethodType(  # type: ignore[method-assign]
        oscillating_cse, gen
    )
    with _temp_env("MOLT_MIDEND_HARD_FAIL", "1"):
        with pytest.raises(RuntimeError, match="failed to converge"):
            _ = gen._canonicalize_control_aware_ops_impl(
                ops, allow_cross_block_const_dedupe=True
            )


def test_sccp_worklist_solver_handles_large_cfg_without_cap_hit() -> None:
    gen = SimpleTIRGenerator()
    ops = _build_sccp_growth_ops(depth=160, constant_cond=None)
    cfg = build_cfg(ops)
    sccp = gen._compute_sccp(ops, cfg)

    assert sccp.executable_blocks
    assert (0 in sccp.executable_blocks) is True
    assert gen.midend_stats["sccp_iteration_cap_hits"] == 0


def test_sccp_cap_hits_only_for_pathological_cases_and_preserves_semantics() -> None:
    normal_ops = _build_sccp_growth_ops(depth=48, constant_cond=True)
    expected = _eval_simple_ops(normal_ops)
    assert expected == 48

    normal_gen = SimpleTIRGenerator()
    with _temp_env("MOLT_SCCP_MAX_ITERS", "200000"):
        normal_out = normal_gen._canonicalize_control_aware_ops_impl(
            normal_ops, allow_cross_block_const_dedupe=True
        )
    assert _eval_simple_ops(normal_out) == expected
    assert normal_gen.midend_stats["sccp_iteration_cap_hits"] == 0

    pathological_ops = _build_sccp_growth_ops(depth=220, constant_cond=None)
    pathological_gen = SimpleTIRGenerator()
    with _temp_env("MOLT_SCCP_MAX_ITERS", "1"):
        pathological_sccp = pathological_gen._compute_sccp(
            pathological_ops, build_cfg(pathological_ops)
        )
    assert pathological_gen.midend_stats["sccp_iteration_cap_hits"] >= 1
    pathological_cfg = build_cfg(pathological_ops)
    assert pathological_sccp.executable_blocks == {
        block.id for block in pathological_cfg.blocks
    }

    capped_gen = SimpleTIRGenerator()
    with _temp_env("MOLT_SCCP_MAX_ITERS", "1"):
        capped_out = capped_gen._canonicalize_control_aware_ops_impl(
            normal_ops, allow_cross_block_const_dedupe=True
        )
    assert _eval_simple_ops(capped_out) == expected
    assert capped_gen.midend_stats["sccp_iteration_cap_hits"] >= 1


def test_midend_policy_matrix_resolves_profile_and_tier() -> None:
    dev_gen = SimpleTIRGenerator(optimization_profile="dev", module_name="__main__")
    dev_ops = _build_sccp_growth_ops(depth=4, constant_cond=True)
    dev_policy = dev_gen._resolve_midend_function_policy(
        dev_ops,
        function_name="molt_main",
        block_count=3,
    )
    assert dev_policy.profile == "dev"
    assert dev_policy.tier == "A"
    assert dev_policy.max_rounds == 2

    stdlib_gen = SimpleTIRGenerator(
        optimization_profile="release",
        module_name="_collections_abc",
        source_path="src/molt/stdlib/_collections_abc.py",
    )
    stdlib_ops = [
        MoltOp(kind="CONST", args=[idx], result=MoltValue(f"v{idx}"))
        for idx in range(700)
    ]
    stdlib_policy = stdlib_gen._resolve_midend_function_policy(
        stdlib_ops,
        function_name="stdlib_heavy",
        block_count=6,
    )
    assert stdlib_policy.profile == "release"
    assert stdlib_policy.tier == "C"
    assert stdlib_policy.enable_deep_edge_thread is False


def test_midend_pass_timing_and_policy_outcome_are_recorded() -> None:
    gen = SimpleTIRGenerator(optimization_profile="dev")
    with _temp_env("MOLT_MIDEND_DEV_ENABLE", "1"):
        _ = gen.map_ops_to_json(
            [
                MoltOp(kind="CONST", args=[1], result=MoltValue("a")),
                MoltOp(kind="CONST", args=[2], result=MoltValue("b")),
                MoltOp(
                    kind="ADD",
                    args=[MoltValue("a"), MoltValue("b")],
                    result=MoltValue("s"),
                ),
            ]
        )
    assert "<direct>" in gen.midend_pass_stats_by_function
    simplify_stats = gen.midend_pass_stats_by_function["<direct>"]["simplify"]
    assert simplify_stats["attempted"] >= 1
    assert float(simplify_stats["ms_total"]) >= 0.0
    assert isinstance(simplify_stats["samples_ms"], list)

    assert "<direct>" in gen.midend_policy_outcomes_by_function
    outcome = gen.midend_policy_outcomes_by_function["<direct>"]
    assert outcome["profile"] == "dev"
    assert outcome["tier"] in {"A", "B", "C"}
    assert float(outcome["spent_ms"]) >= 0.0


def test_midend_budget_degrade_preserves_correctness() -> None:
    ops = _build_sccp_growth_ops(depth=32, constant_cond=True)
    expected = _eval_simple_ops(ops)
    gen = SimpleTIRGenerator(optimization_profile="release")
    with _temp_env("MOLT_MIDEND_BUDGET_MS", "0"):
        out = gen._canonicalize_control_aware_ops_impl(
            ops, allow_cross_block_const_dedupe=True
        )

    assert _eval_simple_ops(out) == expected
    outcome = gen.midend_policy_outcomes_by_function["<direct>"]
    assert outcome["degraded"] is True
    reasons = {event.get("reason") for event in outcome.get("degrade_events", [])}
    assert "budget_exceeded" in reasons
    cse_stats = gen.midend_pass_stats_by_function["<direct>"]["cse"]
    assert int(cse_stats["degraded"]) >= 1
