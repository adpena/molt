"""Semantic aliases cannot bypass a generated target contract."""

from __future__ import annotations

import copy
import tomllib
from pathlib import Path

import pytest

from tools.op_kinds.runtime_requirements import (
    integer_semantics_by_kind,
    runtime_kind_requirement_masks,
    target_runtime_requirement_masks,
)

ROOT = Path(__file__).resolve().parents[1]


def registry():
    return tomllib.loads(
        (ROOT / "runtime/molt-ir/src/tir/op_kinds.toml").read_text(encoding="utf-8")
    )


def test_every_canonical_runtime_requirement_dominates_its_aliases():
    data = registry()
    masks = runtime_kind_requirement_masks(data)
    for row in data["kind"]:
        for alias in row.get("aliases", ()):
            assert masks[alias] & masks[row["canonical"]] == masks[row["canonical"]]
    assert masks["load"] == masks["get_attr"] != 0
    assert masks["string_eq"] == masks["eq"] != 0
    assert masks["gpu_barrier"] == masks["gpu_thread_id"] != 0
    assert masks["call_builtin"] == masks["print"] != 0


def test_new_alias_inherits_all_runtime_roles_and_numeric_roles():
    data = registry()
    row = next(row for row in data["kind"] if row["canonical"] == "const")
    row["aliases"].append("test_new_integer_alias")
    for role in data["simpleir_runtime_requirement_roles"][:3]:
        data[role["table"]].append("const")
    masks = runtime_kind_requirement_masks(data)
    assert masks["test_new_integer_alias"] == masks["const"] != 0
    assert integer_semantics_by_kind(data)["test_new_integer_alias"] == "IntegerLiteral"
    assert integer_semantics_by_kind(data)["load_const"] == "IntegerLiteral"


def test_alias_additional_requirements_do_not_reclassify_unrelated_canonical_ops():
    data = registry()
    masks = runtime_kind_requirement_masks(data)
    roles = integer_semantics_by_kind(data)
    assert roles["box_from_raw_int"] == "IntegerProducer"
    assert "box" not in roles
    assert masks["box"] == 0
    assert masks["gpu_thread_id"] != masks["call"]
    changed = copy.deepcopy(data)
    changed["simpleir_integer_only_semantics_kinds"].append("load_const")
    with pytest.raises(ValueError, match="differs from canonical"):
        integer_semantics_by_kind(changed)


def test_typed_runtime_spellings_require_their_actual_semantic_family():
    data = registry()
    masks = runtime_kind_requirement_masks(data)
    roles = {
        row["constant"]: 1 << row["bit"]
        for row in data["simpleir_runtime_requirement_roles"]
    }
    families = {
        "TUPLE": ["build_tuple", "tuple_new"],
        "OBJECT_MODEL": [
            "build_set",
            "build_slice",
            "object_new_bound",
            "function_defaults_version",
        ],
        "FALLIBLE_PROTOCOL": ["call_builtin", "call_method_ic", "call_super_method_ic"],
        "ASYNC_RUNTIME": [
            "yield",
            "yield_from",
            "state_set",
            "task_wait",
            "async_for_start",
        ],
        "EXCEPTION": ["exception_pending"],
        "DETERMINISTIC_LIFETIME": ["free", "del_boundary", "delete_var"],
        "ITERABLE_PROTOCOL": ["get_iter", "for_iter_start", "for_iter_end"],
    }
    profiles = target_runtime_requirement_masks(data)
    for family, spellings in families.items():
        for kind in spellings:
            assert masks[kind] & roles[family]
            assert masks[kind] & ~profiles["rust"]
            for target in ["native", "wasm", "llvm"]:
                assert masks[kind] & ~profiles[target] == 0
