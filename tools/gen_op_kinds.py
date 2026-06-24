#!/usr/bin/env python3
"""Generate the op-kind registry artifacts from the canonical table.

Single source of truth: ``runtime/molt-tir/src/tir/op_kinds.toml``.

Cross-component op-"kind"-string drift is molt's most prolific silent-miscompile
bug class (see ``docs/design/foundation/25_op_kind_registry.md`` and
``tools/audit_op_kinds.py``). Five components independently keyed on the JSON wire
"kind" vocabulary, each with its own private table. This generator renders that
ONE table into every consumer so the tables can never drift:

  - ``runtime/molt-tir/src/tir/op_kinds_generated.rs`` — the data tables the
    backend's ``kind_to_opcode`` mapper, the ``CopyLowering`` classifier
    (``copy_kind_mints_fresh_owned_ref`` / ``classify_copy_kind`` /
    ``copy_kind_is_explicit_no_heap_move``), and the per-OpCode effect oracle
    (``opcode_may_throw`` / ``opcode_is_side_effecting`` / ``opcode_effects``)
    consume. The effect oracle is rendered as an EXHAUSTIVE match over the
    ``OpCode`` enum (no wildcard arm), so a newly added opcode fails to compile
    until it is given an explicit effect classification in the table — the
    structural kill for the ``matches!``-default-false trap.
  - ``src/molt/frontend/lowering/op_kinds_generated.py`` — the canonical wire
    spellings the frontend emitter (``map_ops_to_json``) uses, so the producer
    and the backend mapper share one spelling.

``tests/test_gen_op_kinds.py`` re-renders both files in memory and asserts byte
equality with the checked-in copies, turning any forgotten regeneration into a
test failure (the ``tests/test_gen_intrinsics.py`` pattern).

Usage::

    python3 tools/gen_op_kinds.py            # (re)write the generated files
    python3 tools/gen_op_kinds.py --check    # exit 1 if a generated file is stale
"""

from __future__ import annotations

import argparse
import ast
import sys
import tempfile
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover - fallback for <3.11
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402

TABLE = ROOT / "runtime/molt-tir/src/tir/op_kinds.toml"
OUT_RS = ROOT / "runtime/molt-tir/src/tir/op_kinds_generated.rs"
OUT_PY = ROOT / "src/molt/frontend/lowering/op_kinds_generated.py"
RUSTFMT_TMP = ROOT / "tmp" / "gen_op_kinds"

# Valid enum values for the table's constrained columns. A value outside these
# sets is a hard error (a typo in the table must not silently degrade to a
# fallback classification).
_PURITY_VALUES = {"pure", "pure_may_throw", "impure"}
_RESULT_ARITY_VALUES = {"zero", "one", "two", "variable"}
_VARIABLE_RESULT_ARITY_OPCODES = {
    # Calls may be emitted for value-producing expressions or result-discarding
    # statements. The return-value fact is present only when TIR carries a result.
    "Call",
    "CallMethod",
    "CallBuiltin",
    # Exception checks can be pure control-transfer polls or produce an explicit
    # flag in older/diagnostic lanes.
    "CheckException",
    # Copy is also the legacy SimpleIR fallback carrier and may model zero,
    # one, or multi-result transport shapes until each spelling is promoted.
    "Copy",
    # SCF ops model region-shaped dialect constructs whose result count is
    # determined by the region signature, not the opcode name alone.
    "ScfIf",
    "ScfFor",
    "ScfWhile",
    "ScfYield",
}

# Operand-ownership: the per-operand borrowed|consumed|refinement
# axis (design 27 §2.1). A uniform shorthand ("all_borrowed" / "all_consumed") or
# a per-position list of the leaf values. molt's "callee borrows all args" ABI
# (design 20 §1.2) makes "all_borrowed" the universal default; "consumed" is the
# rare op-frees-it case (the CallArgs builder, the C6 double-free class);
# "interior_borrow_keepalive" is the borrow-of-edge case (design 27 §1.5): the op
# borrows the operand (frees nothing) AND its result holds an INTERIOR reference
# into that operand's backing store, so the operand's drop is deferred to the
# result's last use (the `LoadAttr`/`Index` source — the round-6 `Counter._handle`
# UAF). "container_absorb" is the existing-container store boundary: the op
# borrows the operand while retaining its own container/storage reference, so the
# caller-owned producer ref still drops at the statement. These refinements are
# per-position only. A value outside this set is a hard error (a typo must never
# silently degrade to a borrow assumption, a consume assumption that double-frees,
# or a missing keepalive/release-boundary fact).
_OPERAND_OWNERSHIP_LEAVES = {
    "borrowed",
    "consumed",
    "interior_borrow_keepalive",
    "container_absorb",
}
_OPERAND_OWNERSHIP_UNIFORM = {"all_borrowed", "all_consumed"}

# Per-TERMINATOR operand-category leaves (design 27 §2.4, the ownership-moves-out
# axis). A `Terminator` is NOT an `OpCode` — its operand ownership is a distinct
# table — so it admits the `transferred` move-out leaf (a `Return` value / a
# branch-arg into a successor phi) and the `none` sentinel (a category with no
# operand on that variant). `borrowed` is the still-live-but-not-moved predicate
# (`CondBranch`/`Switch` discriminant). `consumed` is NOT meaningful for a
# terminator (nothing frees a terminator operand internally), so it is excluded.
_TERMINATOR_OWNERSHIP_LEAVES = {"borrowed", "transferred", "none"}
_RESULT_VALIDITY_VALUES = {"conditional_valid_only_on_edge"}
_LITERAL_PAYLOAD_KINDS = {"int": "Int", "bool": "Bool"}
_CANONICALIZE_COMMUTATIVE_DOMAINS = {"numeric", "i64", "unboxed_scalar"}
_CANONICALIZE_BINARY_PREDICATES = {
    "lhs_int": "int",
    "rhs_int": "int",
    "lhs_bool": "bool",
    "rhs_bool": "bool",
    "same_operands": "none",
}
_CANONICALIZE_BINARY_TYPE_GUARDS = {"none", "lhs_i64", "rhs_i64"}
_CANONICALIZE_BINARY_ACTIONS = {
    "copy_lhs": "copy",
    "copy_rhs": "copy",
    "const_int": "int",
    "const_bool": "bool",
}

# The `Terminator` enum variants (blocks.rs). The [[terminator]] section MUST be
# EXHAUSTIVE over this set (a new variant fails to render until classified —
# mirroring the [[opcode]] exhaustiveness discipline). Kept here (not parsed from
# Rust) as the single declarative expectation; tests/test_gen_op_kinds.py
# cross-checks it against the enum declared in blocks.rs so the two cannot drift.
_TERMINATOR_VARIANTS = (
    "Branch",
    "CondBranch",
    "Switch",
    "StateDispatch",
    "Return",
    "Unreachable",
)

# The flat classifier sets (mirroring the flat `matches!` arms in
# alias_analysis.rs). Kept distinct from the mapper's alias grouping because
# the classifier groups per-individual-kind, not per-OpCode-equivalence.
_CLASSIFIER_SETS = (
    "classifier_fresh_value",
    "classifier_exception_creation_ref",
    "classifier_owned_alias",
    "classifier_inert_marker",
    "classifier_transparent_alias",
    "classifier_no_heap_move",
)
_PASS_DELTA_FACT_FIELDS = (
    ("pass_delta_box_opcodes", "box_op"),
    ("pass_delta_unbox_opcodes", "unbox_op"),
    ("pass_delta_generic_call_opcodes", "generic_call"),
    ("pass_delta_direct_call_opcodes", "direct_call"),
    ("pass_delta_method_call_opcodes", "method_call"),
    ("pass_delta_runtime_helper_call_opcodes", "runtime_helper_call"),
    ("pass_delta_rc_event_opcodes", "rc_event"),
    ("pass_delta_inc_ref_opcodes", "inc_ref"),
    ("pass_delta_dec_ref_opcodes", "dec_ref"),
    ("pass_delta_del_boundary_opcodes", "del_boundary"),
    ("pass_delta_exception_event_opcodes", "exception_event"),
    ("pass_delta_type_guard_opcodes", "type_guard"),
    ("pass_delta_heap_alloc_opcodes", "heap_alloc"),
)
_OPCODE_FACT_SETS = (
    "alias_rc_barrier_opcodes",
    "alias_heap_barrier_opcodes",
    "alias_memory_inert_opcodes",
    "alias_typed_slot_load_opcodes",
    "alias_typed_slot_store_opcodes",
    "alias_transparent_type_guard_opcodes",
    "alias_transparent_copy_opcodes",
    "alias_region_copy_refinement_opcodes",
    "alias_region_container_element_opcodes",
    "alias_region_module_dict_opcodes",
    "alias_slot_direct_observer_opcodes",
    "alias_slot_typed_store_opcodes",
    "alias_slot_never_observer_opcodes",
    "refcount_heap_exposure_opcodes",
    "fusion_barrier_opcodes",
    "i64_zero_divisor_guard_opcodes",
    "i64_shift_count_guard_opcodes",
    "exception_label_attr_opcodes",
    "exception_transfer_edge_opcodes",
    *(key for key, _field in _PASS_DELTA_FACT_FIELDS),
)
_ALIAS_TYPED_SLOT_ROLE_SETS = (
    "alias_typed_slot_load_opcodes",
    "alias_typed_slot_store_opcodes",
)
_ALIAS_TRANSPARENT_ALIAS_ROLE_SETS = (
    "alias_transparent_type_guard_opcodes",
    "alias_transparent_copy_opcodes",
)
_ALIAS_MEMORY_REGION_SETS = (
    "alias_typed_slot_load_opcodes",
    "alias_typed_slot_store_opcodes",
    "alias_region_copy_refinement_opcodes",
    "alias_region_container_element_opcodes",
    "alias_region_module_dict_opcodes",
    "alias_memory_inert_opcodes",
)
_ALIAS_SLOT_OBSERVATION_SETS = (
    "alias_slot_direct_observer_opcodes",
    "alias_slot_typed_store_opcodes",
    "alias_transparent_type_guard_opcodes",
    "alias_transparent_copy_opcodes",
    "alias_slot_never_observer_opcodes",
)


# Rust `bool` literal helper.
def _rs_bool(value: bool) -> str:
    return "true" if value else "false"


# ---------------------------------------------------------------------------
# Table loading + validation
# ---------------------------------------------------------------------------


class OpKindTableError(RuntimeError):
    pass


def load_table() -> dict:
    """Load and structurally validate ``op_kinds.toml``.

    Validation is fail-loud: a malformed/ambiguous table must never render a
    silently-degraded generated file.
    """
    if not TABLE.exists():
        raise OpKindTableError(f"op-kind table missing: {TABLE}")
    data = tomllib.loads(TABLE.read_text(encoding="utf-8"))

    opcodes = data.get("opcode", [])
    if not opcodes:
        raise OpKindTableError("table has no [[opcode]] rows")
    seen_opcodes: set[str] = set()
    for row in opcodes:
        name = row.get("name")
        if not isinstance(name, str) or not name:
            raise OpKindTableError(f"[[opcode]] row missing 'name': {row}")
        if name in seen_opcodes:
            raise OpKindTableError(f"duplicate [[opcode]] name: {name}")
        seen_opcodes.add(name)
        if not isinstance(row.get("may_throw"), bool):
            raise OpKindTableError(f"opcode {name}: 'may_throw' must be a bool")
        if not isinstance(row.get("side_effecting"), bool):
            raise OpKindTableError(f"opcode {name}: 'side_effecting' must be a bool")
        purity = row.get("purity")
        if purity not in _PURITY_VALUES:
            raise OpKindTableError(
                f"opcode {name}: 'purity' must be one of {sorted(_PURITY_VALUES)}, "
                f"got {purity!r}"
            )
        result_arity = row.get("result_arity")
        if result_arity not in _RESULT_ARITY_VALUES:
            raise OpKindTableError(
                f"opcode {name}: 'result_arity' must be one of "
                f"{sorted(_RESULT_ARITY_VALUES)}, got {result_arity!r}"
            )
        if result_arity == "variable" and name not in _VARIABLE_RESULT_ARITY_OPCODES:
            raise OpKindTableError(
                f"opcode {name}: result_arity = 'variable' is reserved for "
                "audited context-dependent opcodes; use a fixed arity or add "
                "the opcode to _VARIABLE_RESULT_ARITY_OPCODES with a rationale"
            )
        # Cross-axis invariant: the `purity` class and `may_throw` bit are two
        # views of the same throw property and MUST agree. `OpEffects::PURE` has
        # `nothrow = true`, so a `pure` opcode cannot also be `may_throw`; a
        # `pure_may_throw` opcode is precisely the throwing-but-deterministic
        # class (`Div`/`FloorDiv`/`Mod`/`Pow`/`Shl`/`Shr`), so it MUST be
        # `may_throw`. `impure` is unconstrained (a call both throws and mutates).
        # This is the structural kill for the drift that classified `Pow` as
        # `pure_may_throw` yet `may_throw = false` (and `Shl`/`Shr` as fully
        # `pure`), which let DCE silently drop a dead `1 << -1` / `0 ** -1`.
        if purity == "pure" and row["may_throw"]:
            raise OpKindTableError(
                f"opcode {name}: purity 'pure' requires may_throw = false "
                "(a pure op is nothrow); use purity 'pure_may_throw' if it raises"
            )
        if purity == "pure_may_throw" and not row["may_throw"]:
            raise OpKindTableError(
                f"opcode {name}: purity 'pure_may_throw' requires may_throw = true "
                "(it raises for some inputs); use purity 'pure' if it never raises"
            )
        # Operand ownership is MANDATORY and explicit on every opcode (mirroring
        # the may_throw/side_effecting/purity exhaustive-classification
        # discipline): a new OpCode cannot render until it states whether each
        # operand is borrowed or consumed. Fail-loud — no silent borrow default.
        _validate_operand_ownership(name, row.get("operand_ownership"))
        if not isinstance(row.get("result_absorbs_operands"), bool):
            raise OpKindTableError(
                f"opcode {name}: 'result_absorbs_operands' must be a bool"
            )
        selected_owner = row.get("result_mints_owned_selected_operand", False)
        if not isinstance(selected_owner, bool):
            raise OpKindTableError(
                f"opcode {name}: 'result_mints_owned_selected_operand' must be a bool"
            )
        if selected_owner and row["result_absorbs_operands"]:
            raise OpKindTableError(
                f"opcode {name}: selected-alias ownership and result absorption "
                "are mutually exclusive result-side ownership facts"
            )

    prefixes = data.get("classifier_fresh_value_prefixes", [])
    if not isinstance(prefixes, list) or not all(isinstance(p, str) for p in prefixes):
        raise OpKindTableError(
            "classifier_fresh_value_prefixes must be a list of strings"
        )

    for key in _CLASSIFIER_SETS:
        members = data.get(key, [])
        if not isinstance(members, list) or not all(
            isinstance(x, str) for x in members
        ):
            raise OpKindTableError(f"{key} must be a list of strings")
        if len(set(members)) != len(members):
            raise OpKindTableError(f"{key} has duplicate members")

    _validate_literal_payload_facts(data, seen_opcodes)
    _validate_canonicalize_facts(data, seen_opcodes)
    for key in _OPCODE_FACT_SETS:
        _validate_opcode_fact_set(data, key, seen_opcodes)
    _validate_pass_delta_opcode_facts(data)
    _validate_alias_opcode_role_sets(
        data, _ALIAS_TYPED_SLOT_ROLE_SETS, "alias typed-slot role"
    )
    _validate_alias_opcode_role_sets(
        data, _ALIAS_TRANSPARENT_ALIAS_ROLE_SETS, "alias transparent-alias role"
    )
    _validate_alias_memory_region_sets(data)
    _validate_alias_slot_observation_sets(data)

    kinds = data.get("kind", [])
    # Every mapper spelling (canonical or alias) must be globally unique within
    # the mapper — a kind string maps to exactly one OpCode; two rows owning it
    # is the exact drift this registry kills.
    owner: dict[str, str] = {}
    seen_canon: set[str] = set()
    for row in kinds:
        canon = row.get("canonical")
        if not isinstance(canon, str) or not canon:
            raise OpKindTableError(f"[[kind]] row missing 'canonical': {row}")
        if canon in seen_canon:
            raise OpKindTableError(f"duplicate canonical kind: {canon}")
        seen_canon.add(canon)
        aliases = row.get("aliases", [])
        if not isinstance(aliases, list) or not all(
            isinstance(a, str) for a in aliases
        ):
            raise OpKindTableError(f"kind {canon}: 'aliases' must be a list of strings")
        mapper = row.get("mapper_opcode")
        if not isinstance(mapper, str) or mapper not in seen_opcodes:
            raise OpKindTableError(
                f"kind {canon}: mapper_opcode {mapper!r} is not a known OpCode"
            )
        for spelling in [canon, *aliases]:
            if spelling in owner:
                raise OpKindTableError(
                    f"mapper spelling {spelling!r} owned by both "
                    f"{owner[spelling]!r} and {canon!r}"
                )
            owner[spelling] = canon

    # -- [[consuming_kind]] operand-ownership overrides per wire-kind spelling --
    # Each row names a wire-kind SPELLING (canonical OR alias of a [[kind]] row)
    # that consumes a specific operand. `owner` is exactly the set of valid
    # mapper spellings; a row naming an unknown spelling is a hard error (the
    # structural kill for a typo'd consume override silently doing nothing — the
    # very C6 double-free this column retires).
    _validate_consuming_kinds(data, owner)
    _validate_absorbing_kinds(data, owner)
    _validate_absorbing_operand_kinds(data)
    _validate_result_finalizer_source_kinds(data)
    _validate_result_validity(data, seen_opcodes)
    _validate_explicit_release_operands(data, {row["name"]: row for row in opcodes})

    _validate_terminators(data)

    _validate_frontend_tables(data, opcodes)

    return data


def _validate_literal_payload_facts(data: dict, opcodes: set[str]) -> None:
    rows = data.get("literal_payload_opcodes", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError(
            "literal_payload_opcodes must be a non-empty array of tables"
        )
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError("literal_payload_opcodes rows must be inline tables")
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"literal_payload_opcodes row missing opcode: {row}")
        if opcode not in opcodes:
            raise OpKindTableError(
                f"literal_payload_opcodes opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate literal_payload_opcodes opcode: {opcode}"
            )
        seen.add(opcode)
        literal = row.get("literal")
        if literal not in _LITERAL_PAYLOAD_KINDS:
            raise OpKindTableError(
                f"literal_payload_opcodes {opcode}: literal must be one of "
                f"{sorted(_LITERAL_PAYLOAD_KINDS)}, got {literal!r}"
            )


def _validate_operand_ownership(name: str, value: object) -> None:
    """Validate one opcode's ``operand_ownership`` (fail-loud).

    Accepts a uniform shorthand (``"all_borrowed"`` / ``"all_consumed"``) or a
    per-position list of the leaf values (``"borrowed"`` / ``"consumed"`` /
    ``"interior_borrow_keepalive"``). ``interior_borrow_keepalive`` is list-only:
    it marks the operand whose backing store the op's result interior-borrows (the
    borrow-of edge, design 27 §1.5), and an op that interior-borrows one operand
    still merely borrows the rest, so it cannot be a uniform shorthand. Any other
    shape is a hard error — a missing/typo'd classification must never silently
    degrade to a borrow assumption (leak), a consume assumption (double-free), or
    a dropped keepalive (the round-6 interior-borrow UAF).
    """
    if value is None:
        raise OpKindTableError(
            f"opcode {name}: 'operand_ownership' is mandatory — classify every "
            'operand as borrowed|consumed (use "all_borrowed" for the common '
            "callee-borrows-args case; design 20 §1.2 / design 27 §2.1)"
        )
    if isinstance(value, str):
        if value not in _OPERAND_OWNERSHIP_UNIFORM:
            raise OpKindTableError(
                f"opcode {name}: 'operand_ownership' string must be one of "
                f"{sorted(_OPERAND_OWNERSHIP_UNIFORM)}, got {value!r} (or use a "
                "per-position list of borrowed|consumed)"
            )
        return
    if isinstance(value, list):
        if not value:
            raise OpKindTableError(
                f"opcode {name}: 'operand_ownership' list must be non-empty (use "
                'the "all_borrowed" shorthand for a uniform op)'
            )
        for i, leaf in enumerate(value):
            if leaf not in _OPERAND_OWNERSHIP_LEAVES:
                raise OpKindTableError(
                    f"opcode {name}: 'operand_ownership'[{i}] must be one of "
                    f"{sorted(_OPERAND_OWNERSHIP_LEAVES)}, got {leaf!r}"
                )
        return
    raise OpKindTableError(
        f"opcode {name}: 'operand_ownership' must be a string shorthand or a list, "
        f"got {type(value).__name__}"
    )


def _validate_canonicalize_facts(data: dict, opcodes: set[str]) -> None:
    """Validate opcode-level canonicalization facts.

    These rows replace backend-local opcode lists in canonicalize.rs. They must
    be explicit, duplicate-free, and opcode-backed so a typo cannot silently
    disable an algebraic rewrite or make a comparison swap one-way.
    """
    reorder_rows = data.get("canonicalize_commutative_reorder", [])
    if not isinstance(reorder_rows, list) or not reorder_rows:
        raise OpKindTableError(
            "canonicalize_commutative_reorder must be a non-empty array of tables"
        )
    seen_reorder: set[str] = set()
    for row in reorder_rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "canonicalize_commutative_reorder rows must be inline tables"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"canonicalize_commutative_reorder row missing opcode: {row}"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                f"canonicalize_commutative_reorder opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen_reorder:
            raise OpKindTableError(
                f"duplicate canonicalize_commutative_reorder opcode: {opcode}"
            )
        seen_reorder.add(opcode)
        domain = row.get("domain")
        if domain not in _CANONICALIZE_COMMUTATIVE_DOMAINS:
            raise OpKindTableError(
                f"canonicalize_commutative_reorder {opcode}: domain must be one of "
                f"{sorted(_CANONICALIZE_COMMUTATIVE_DOMAINS)}, got {domain!r}"
            )

    swap_rows = data.get("canonicalize_swapped_comparison", [])
    if not isinstance(swap_rows, list) or not swap_rows:
        raise OpKindTableError(
            "canonicalize_swapped_comparison must be a non-empty array of tables"
        )
    swaps: dict[str, str] = {}
    for row in swap_rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "canonicalize_swapped_comparison rows must be inline tables"
            )
        opcode = row.get("opcode")
        swapped = row.get("swapped")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"canonicalize_swapped_comparison row missing opcode: {row}"
            )
        if not isinstance(swapped, str) or not swapped:
            raise OpKindTableError(
                f"canonicalize_swapped_comparison {opcode}: swapped must name an OpCode"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                f"canonicalize_swapped_comparison opcode {opcode!r} is not a known OpCode"
            )
        if swapped not in opcodes:
            raise OpKindTableError(
                f"canonicalize_swapped_comparison {opcode}: swapped opcode "
                f"{swapped!r} is not a known OpCode"
            )
        if opcode == swapped:
            raise OpKindTableError(
                f"canonicalize_swapped_comparison {opcode}: swapped opcode must differ"
            )
        if opcode in swaps:
            raise OpKindTableError(
                f"duplicate canonicalize_swapped_comparison opcode: {opcode}"
            )
        swaps[opcode] = swapped

    for opcode, swapped in swaps.items():
        if swaps.get(swapped) != opcode:
            raise OpKindTableError(
                "canonicalize_swapped_comparison must be symmetric: "
                f"{opcode}->{swapped} but {swapped}->{swaps.get(swapped)!r}"
            )

    binary_rows = data.get("canonicalize_binary_rules", [])
    if not isinstance(binary_rows, list) or not binary_rows:
        raise OpKindTableError(
            "canonicalize_binary_rules must be a non-empty array of tables"
        )
    seen_binary_rules: set[tuple[object, ...]] = set()
    for row in binary_rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "canonicalize_binary_rules rows must be inline tables"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"canonicalize_binary_rules row missing opcode: {row}"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                f"canonicalize_binary_rules opcode {opcode!r} is not a known OpCode"
            )

        predicate = row.get("predicate")
        value_kind = _CANONICALIZE_BINARY_PREDICATES.get(predicate)
        if value_kind is None:
            raise OpKindTableError(
                f"canonicalize_binary_rules {opcode}: predicate must be one of "
                f"{sorted(_CANONICALIZE_BINARY_PREDICATES)}, got {predicate!r}"
            )
        if value_kind == "int":
            value = row.get("value")
            if isinstance(value, bool) or not isinstance(value, int):
                raise OpKindTableError(
                    f"canonicalize_binary_rules {opcode}/{predicate}: value must "
                    f"be an int, got {value!r}"
                )
        elif value_kind == "bool":
            value = row.get("value")
            if not isinstance(value, bool):
                raise OpKindTableError(
                    f"canonicalize_binary_rules {opcode}/{predicate}: value must "
                    f"be a bool, got {value!r}"
                )
        elif "value" in row:
            raise OpKindTableError(
                f"canonicalize_binary_rules {opcode}/{predicate}: value is not used"
            )

        type_guard = row.get("type_guard")
        if type_guard not in _CANONICALIZE_BINARY_TYPE_GUARDS:
            raise OpKindTableError(
                f"canonicalize_binary_rules {opcode}: type_guard must be one of "
                f"{sorted(_CANONICALIZE_BINARY_TYPE_GUARDS)}, got {type_guard!r}"
            )

        action = row.get("action")
        result_kind = _CANONICALIZE_BINARY_ACTIONS.get(action)
        if result_kind is None:
            raise OpKindTableError(
                f"canonicalize_binary_rules {opcode}: action must be one of "
                f"{sorted(_CANONICALIZE_BINARY_ACTIONS)}, got {action!r}"
            )
        if result_kind == "int":
            result = row.get("result")
            if isinstance(result, bool) or not isinstance(result, int):
                raise OpKindTableError(
                    f"canonicalize_binary_rules {opcode}/{action}: result must "
                    f"be an int, got {result!r}"
                )
        elif result_kind == "bool":
            result = row.get("result")
            if not isinstance(result, bool):
                raise OpKindTableError(
                    f"canonicalize_binary_rules {opcode}/{action}: result must "
                    f"be a bool, got {result!r}"
                )
        elif "result" in row:
            raise OpKindTableError(
                f"canonicalize_binary_rules {opcode}/{action}: result is not used"
            )

        fingerprint = (
            opcode,
            predicate,
            row.get("value"),
            type_guard,
            action,
            row.get("result"),
        )
        if fingerprint in seen_binary_rules:
            raise OpKindTableError(
                f"duplicate canonicalize_binary_rules row for {opcode}/{predicate}"
            )
        seen_binary_rules.add(fingerprint)


def _validate_opcode_fact_set(data: dict, key: str, opcodes: set[str]) -> None:
    members = data.get(key, [])
    if not isinstance(members, list) or not all(isinstance(x, str) for x in members):
        raise OpKindTableError(f"{key} must be a list of opcode names")
    if len(set(members)) != len(members):
        raise OpKindTableError(f"{key} has duplicate opcodes")
    unknown = sorted(set(members) - opcodes)
    if unknown:
        raise OpKindTableError(f"{key} contains unknown OpCode names: {unknown}")


def _validate_alias_opcode_role_sets(
    data: dict, role_sets: tuple[str, ...], label: str
) -> None:
    owners: dict[str, str] = {}
    for key in role_sets:
        for opcode in data.get(key, []):
            if opcode in owners:
                raise OpKindTableError(
                    f"{label} opcode {opcode!r} appears in both "
                    f"{owners[opcode]} and {key}"
                )
            owners[opcode] = key


def _validate_alias_slot_observation_sets(data: dict) -> None:
    owners: dict[str, str] = {}
    for key in _ALIAS_SLOT_OBSERVATION_SETS:
        for opcode in data.get(key, []):
            if opcode in owners:
                raise OpKindTableError(
                    f"alias slot observation opcode {opcode!r} appears in both "
                    f"{owners[opcode]} and {key}"
                )
            owners[opcode] = key


def _validate_pass_delta_opcode_facts(data: dict) -> None:
    generic = set(data.get("pass_delta_generic_call_opcodes", []))
    for key in (
        "pass_delta_direct_call_opcodes",
        "pass_delta_method_call_opcodes",
        "pass_delta_runtime_helper_call_opcodes",
    ):
        extra = sorted(set(data.get(key, [])) - generic)
        if extra:
            raise OpKindTableError(
                f"{key} must be a subset of pass_delta_generic_call_opcodes: {extra}"
            )

    rc_events = set(data.get("pass_delta_rc_event_opcodes", []))
    for key in (
        "pass_delta_inc_ref_opcodes",
        "pass_delta_dec_ref_opcodes",
        "pass_delta_del_boundary_opcodes",
    ):
        extra = sorted(set(data.get(key, [])) - rc_events)
        if extra:
            raise OpKindTableError(
                f"{key} must be a subset of pass_delta_rc_event_opcodes: {extra}"
            )


def _validate_alias_memory_region_sets(data: dict) -> None:
    owners: dict[str, str] = {}
    for key in _ALIAS_MEMORY_REGION_SETS:
        for opcode in data.get(key, []):
            if opcode in owners:
                raise OpKindTableError(
                    f"alias memory-region opcode {opcode!r} appears in both "
                    f"{owners[opcode]} and {key}"
                )
            owners[opcode] = key


def _validate_consuming_kinds(data: dict, valid_spellings: dict[str, str]) -> None:
    """Structurally validate the ``[[consuming_kind]]`` operand-ownership
    overrides (fail-loud). Each row pins one wire-kind SPELLING to a consumed
    operand position; the spelling must be a known mapper spelling and the
    consumed-operand selector must be ``"last"`` or a non-negative integer."""
    rows = data.get("consuming_kind", [])
    if not isinstance(rows, list):
        raise OpKindTableError("[[consuming_kind]] must be an array of tables")
    seen: set[str] = set()
    for row in rows:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(f"[[consuming_kind]] row missing 'kind': {row}")
        if kind in seen:
            raise OpKindTableError(f"duplicate consuming_kind: {kind}")
        seen.add(kind)
        if kind not in valid_spellings:
            raise OpKindTableError(
                f"consuming_kind {kind!r} is not a known [[kind]] mapper spelling "
                "(canonical or alias) — a consume override on an unknown spelling "
                "would silently never fire (the C6 double-free it must retire)"
            )
        sel = row.get("consumed_operand")
        if sel == "last":
            continue
        if isinstance(sel, bool) or not isinstance(sel, int) or sel < 0:
            raise OpKindTableError(
                f"consuming_kind {kind}: 'consumed_operand' must be \"last\" or a "
                f"non-negative operand index, got {sel!r}"
            )


def _validate_absorbing_kinds(data: dict, mapper_spellings: dict[str, str]) -> None:
    """Structurally validate ``[[absorbing_kind]]`` rows.

    These are Copy-lifted fresh constructor spellings whose RESULT owns operand
    lifetimes. They are intentionally not first-class ``[[kind]]`` aliases:
    aliasing would hide backend/backconversion spelling differences instead of
    carrying the ownership fact explicitly.
    """
    rows = data.get("absorbing_kind", [])
    if not isinstance(rows, list):
        raise OpKindTableError("[[absorbing_kind]] must be an array of tables")
    fresh_members = set(data.get("classifier_fresh_value", []))
    seen: set[str] = set()
    for row in rows:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(f"[[absorbing_kind]] row missing 'kind': {row}")
        if kind in seen:
            raise OpKindTableError(f"duplicate absorbing_kind: {kind}")
        seen.add(kind)
        if kind in mapper_spellings:
            raise OpKindTableError(
                f"absorbing_kind {kind!r} overlaps a [[kind]] mapper spelling; "
                "record first-class opcode absorption on the opcode row instead"
            )
        if kind not in fresh_members:
            raise OpKindTableError(
                f"absorbing_kind {kind!r} must also be in classifier_fresh_value "
                "(a result cannot absorb operand ownership unless it mints a fresh "
                "owned container result)"
            )


def _validate_absorbing_operand_kinds(data: dict) -> None:
    """Structurally validate Copy-lifted existing-container store facts.

    These rows name preserved SimpleIR spellings whose operand is retained by an
    existing container/store. The caller still owns and drops its operand ref;
    the fact only tells finalizer-boundary placement that the producer temp's
    Python-visible obligation ended at this statement.
    """
    rows = data.get("absorbing_operand_kind", [])
    if not isinstance(rows, list):
        raise OpKindTableError("[[absorbing_operand_kind]] must be an array of tables")
    seen: set[str] = set()
    for row in rows:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[absorbing_operand_kind]] row missing 'kind': {row}"
            )
        if kind in seen:
            raise OpKindTableError(f"duplicate absorbing_operand_kind: {kind}")
        seen.add(kind)
        sel = row.get("absorbed_operand")
        if sel == "last":
            continue
        if isinstance(sel, bool) or not isinstance(sel, int) or sel < 0:
            raise OpKindTableError(
                f"absorbing_operand_kind {kind}: 'absorbed_operand' must be "
                f'"last" or a non-negative operand index, got {sel!r}'
            )


def _validate_result_finalizer_source_kinds(data: dict) -> None:
    """Validate Copy-lifted extraction facts whose fresh result can carry a
    finalizer-sensitive value from one source operand."""
    rows = data.get("result_finalizer_source_kind", [])
    if not isinstance(rows, list):
        raise OpKindTableError(
            "[[result_finalizer_source_kind]] must be an array of tables"
        )
    fresh_members = set(data.get("classifier_fresh_value", []))
    seen: set[str] = set()
    for row in rows:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[result_finalizer_source_kind]] row missing 'kind': {row}"
            )
        if kind in seen:
            raise OpKindTableError(f"duplicate result_finalizer_source_kind: {kind}")
        seen.add(kind)
        if kind not in fresh_members:
            raise OpKindTableError(
                f"result_finalizer_source_kind {kind!r} must also be in "
                "classifier_fresh_value (the result must carry its own owned ref)"
            )
        sel = row.get("source_operand")
        if sel == "last":
            continue
        if isinstance(sel, bool) or not isinstance(sel, int) or sel < 0:
            raise OpKindTableError(
                f"result_finalizer_source_kind {kind}: 'source_operand' must be "
                f'"last" or a non-negative operand index, got {sel!r}'
            )


def _validate_result_validity(data: dict, opcodes: set[str]) -> None:
    """Validate per-opcode result-validity rows.

    These rows encode result slots whose bits are only valid on a specific
    outgoing edge, currently the `IterNextUnboxed` value-out result. Missing or
    misspelled rows must fail at generation rather than silently reintroduce a
    drop-insertion hand list.
    """
    rows = data.get("result_validity", [])
    if not isinstance(rows, list):
        raise OpKindTableError("[[result_validity]] must be an array of tables")
    seen: set[tuple[str, int]] = set()
    for row in rows:
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"[[result_validity]] row missing 'opcode': {row}")
        if opcode not in opcodes:
            raise OpKindTableError(
                f"result_validity opcode {opcode!r} is not a known OpCode"
            )
        result = row.get("result")
        if isinstance(result, bool) or not isinstance(result, int) or result < 0:
            raise OpKindTableError(
                f"result_validity {opcode}: 'result' must be a non-negative "
                f"result index, got {result!r}"
            )
        validity = row.get("validity")
        if validity not in _RESULT_VALIDITY_VALUES:
            raise OpKindTableError(
                f"result_validity {opcode}: 'validity' must be one of "
                f"{sorted(_RESULT_VALIDITY_VALUES)}, got {validity!r}"
            )
        key = (opcode, result)
        if key in seen:
            raise OpKindTableError(
                f"duplicate result_validity row for opcode {opcode} result {result}"
            )
        seen.add(key)


def _validate_explicit_release_operands(data: dict, opcodes: dict[str, dict]) -> None:
    """Validate opcodes that explicitly release Python-owned operand roots.

    These rows encode release boundaries such as `DecRef` (all operands) and
    `DeleteVar` (the old slot value at operand 1). The fact is intentionally
    distinct from operand ownership: it is a Python lifetime boundary consumed by
    DropInsertion, not an ABI consume/borrow rule.
    """
    rows = data.get("explicit_release_operand", [])
    if not isinstance(rows, list):
        raise OpKindTableError(
            "[[explicit_release_operand]] must be an array of tables"
        )
    seen: set[str] = set()
    for row in rows:
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"[[explicit_release_operand]] row missing 'opcode': {row}"
            )
        opcode_row = opcodes.get(opcode)
        if opcode_row is None:
            raise OpKindTableError(
                f"explicit_release_operand opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(f"duplicate explicit_release_operand row: {opcode}")
        seen.add(opcode)
        operand = row.get("operand")
        if operand in {"all", "last"}:
            continue
        if isinstance(operand, bool) or not isinstance(operand, int) or operand < 0:
            raise OpKindTableError(
                f"explicit_release_operand {opcode}: 'operand' must be \"all\", "
                f'"last", or a non-negative operand index, got {operand!r}'
            )
        ownership = opcode_row.get("operand_ownership")
        if not isinstance(ownership, list):
            raise OpKindTableError(
                f"explicit_release_operand {opcode}: numeric operand {operand} "
                "requires a fixed per-position operand_ownership list"
            )
        if operand >= len(ownership):
            raise OpKindTableError(
                f"explicit_release_operand {opcode}: operand index {operand} "
                f"is out of range for {len(ownership)} declared operands"
            )


def _validate_terminators(data: dict) -> None:
    """Structurally validate the ``[[terminator]]`` per-terminator operand
    ownership (design 27 §2.4, fail-loud). Each row classifies one ``Terminator``
    enum variant's two operand categories (``direct`` / ``branch_arg``) as a
    ``_TERMINATOR_OWNERSHIP_LEAVES`` value. The section MUST be EXHAUSTIVE over
    the ``Terminator`` enum (a new variant unclassified is a generation-time
    failure — the kill for a terminator silently inheriting a transfer/borrow
    assumption, mirroring the [[opcode]] exhaustiveness discipline)."""
    rows = data.get("terminator", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError("table has no [[terminator]] rows")
    seen: set[str] = set()
    for row in rows:
        name = row.get("name")
        if not isinstance(name, str) or not name:
            raise OpKindTableError(f"[[terminator]] row missing 'name': {row}")
        if name in seen:
            raise OpKindTableError(f"duplicate [[terminator]] name: {name}")
        seen.add(name)
        for col in ("direct", "branch_arg"):
            leaf = row.get(col)
            if leaf not in _TERMINATOR_OWNERSHIP_LEAVES:
                raise OpKindTableError(
                    f"terminator {name}: {col!r} must be one of "
                    f"{sorted(_TERMINATOR_OWNERSHIP_LEAVES)}, got {leaf!r}"
                )
    if seen != set(_TERMINATOR_VARIANTS):
        raise OpKindTableError(
            "[[terminator]] must be EXHAUSTIVE over the Terminator enum "
            "(an unclassified variant would silently inherit a transfer/borrow "
            "assumption in drop_insertion's transfer carve-out): "
            f"table-only={sorted(seen - set(_TERMINATOR_VARIANTS))} "
            f"enum-only={sorted(set(_TERMINATOR_VARIANTS) - seen)}"
        )


# ---------------------------------------------------------------------------
# Frontend op.kind table validation (molt task #44, F2a)
# ---------------------------------------------------------------------------


def _validate_frontend_tables(data: dict, opcodes: list[dict]) -> None:
    """Structurally validate the three frontend `op.kind` tables.

    These describe the FRONTEND's UPPERCASE pre-serialization `op.kind`
    vocabulary (distinct from the wire `[[kind]]` spellings). The validation is
    the structural kill for the frontend⇄backend dual raising-oracle drift:

      * Every `[[frontend_raising_kind]]` row carrying `opcode = X` is
        cross-checked X.may_throw == true (flipping an opcode to nothrow while
        leaving it raising here is a generation-time FAILURE).
      * Every `[[frontend_check_exception_skip]]` row carrying `opcode = X` is
        cross-checked X.may_throw == false UNLESS it sets `control_flow = true`
        (a may_throw skip member must justify itself as structurally handled).
      * `[[binary_op]]` is cross-checked EXHAUSTIVE over `ast.operator` — a
        missing operator subclass is a generation-time FAILURE (the task-#27
        lesson that the hand augassign map silently omitted 7 inplace kinds).
    """
    may_throw_ops = {r["name"] for r in opcodes if r["may_throw"]}
    opcode_names = {r["name"] for r in opcodes}

    # -- [[frontend_raising_kind]] ------------------------------------------
    raising = data.get("frontend_raising_kind", [])
    if not isinstance(raising, list) or not raising:
        raise OpKindTableError("table has no [[frontend_raising_kind]] rows")
    seen_raising: set[str] = set()
    for row in raising:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[frontend_raising_kind]] row missing 'kind': {row}"
            )
        if kind in seen_raising:
            raise OpKindTableError(f"duplicate frontend_raising_kind: {kind}")
        seen_raising.add(kind)
        has_opcode = "opcode" in row
        has_reason = "reason" in row
        if has_opcode == has_reason:
            raise OpKindTableError(
                f"frontend_raising_kind {kind}: exactly one of 'opcode' / 'reason' "
                "required (opcode = a may_throw OpCode it maps to; reason = a "
                "documented frontend-specific justification)"
            )
        if has_opcode:
            op = row["opcode"]
            if op not in opcode_names:
                raise OpKindTableError(
                    f"frontend_raising_kind {kind}: opcode {op!r} is not a known OpCode"
                )
            if op not in may_throw_ops:
                raise OpKindTableError(
                    f"frontend_raising_kind {kind}: opcode {op!r} is NOT may_throw — "
                    "a raising frontend kind must map to a may_throw OpCode (or use "
                    "'reason' for a frontend-specific pre-specialization/preserved kind)"
                )
        else:
            if not isinstance(row["reason"], str) or not row["reason"]:
                raise OpKindTableError(
                    f"frontend_raising_kind {kind}: 'reason' must be a non-empty string"
                )

    # -- [[frontend_check_exception_skip]] ----------------------------------
    skip = data.get("frontend_check_exception_skip", [])
    if not isinstance(skip, list) or not skip:
        raise OpKindTableError("table has no [[frontend_check_exception_skip]] rows")
    seen_skip: set[str] = set()
    for row in skip:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[frontend_check_exception_skip]] row missing 'kind': {row}"
            )
        if kind in seen_skip:
            raise OpKindTableError(f"duplicate frontend_check_exception_skip: {kind}")
        seen_skip.add(kind)
        has_opcode = "opcode" in row
        has_reason = "reason" in row
        if has_opcode == has_reason:
            raise OpKindTableError(
                f"frontend_check_exception_skip {kind}: exactly one of 'opcode' / "
                "'reason' required"
            )
        if has_opcode:
            op = row["opcode"]
            if op not in opcode_names:
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: opcode {op!r} is not a "
                    "known OpCode"
                )
            control_flow = row.get("control_flow", False)
            if not isinstance(control_flow, bool):
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: 'control_flow' must be a bool"
                )
            if control_flow:
                # A may_throw opcode is skip-listed because its exceptional edge
                # is handled structurally; the flag must be justified by an
                # actually-throwing opcode.
                if op not in may_throw_ops:
                    raise OpKindTableError(
                        f"frontend_check_exception_skip {kind}: control_flow = true but "
                        f"opcode {op!r} is NOT may_throw (the flag is spurious — a "
                        "nothrow opcode needs no control_flow exception)"
                    )
            else:
                if op in may_throw_ops:
                    raise OpKindTableError(
                        f"frontend_check_exception_skip {kind}: opcode {op!r} is "
                        "may_throw but not flagged control_flow — skipping its "
                        "CHECK_EXCEPTION would drop the exception edge. Set "
                        "control_flow = true (with justification) or remove the row."
                    )
        else:
            if "control_flow" in row:
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: 'control_flow' only applies "
                    "to opcode-backed rows (a frontend-only structural kind needs none)"
                )
            if not isinstance(row["reason"], str) or not row["reason"]:
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: 'reason' must be a "
                    "non-empty string"
                )

    # -- [[binary_op]] (EXHAUSTIVE over ast.operator) -----------------------
    binary = data.get("binary_op", [])
    if not isinstance(binary, list) or not binary:
        raise OpKindTableError("table has no [[binary_op]] rows")
    seen_binary: set[str] = set()
    for row in binary:
        ast_op = row.get("ast_op")
        if not isinstance(ast_op, str) or not ast_op:
            raise OpKindTableError(f"[[binary_op]] row missing 'ast_op': {row}")
        if ast_op in seen_binary:
            raise OpKindTableError(f"duplicate binary_op ast_op: {ast_op}")
        seen_binary.add(ast_op)
        for col in ("binop_kind", "augassign_kind"):
            if not isinstance(row.get(col), str) or not row[col]:
                raise OpKindTableError(
                    f"binary_op {ast_op}: {col!r} must be a non-empty string"
                )
    ast_operator_names = {cls.__name__ for cls in ast.operator.__subclasses__()}
    if seen_binary != ast_operator_names:
        raise OpKindTableError(
            "[[binary_op]] must be EXHAUSTIVE over ast.operator subclasses "
            "(every binary/augmented operator must have a row, or visit_BinOp / "
            "visit_AugAssign would silently miss it — the task-#27 inplace-kind gap):"
            f" table-only={sorted(seen_binary - ast_operator_names)} "
            f"ast-only={sorted(ast_operator_names - seen_binary)}"
        )


# ---------------------------------------------------------------------------
# Rust rendering
# ---------------------------------------------------------------------------

_RS_HEADER = """\
// @generated by tools/gen_op_kinds.py from
// runtime/molt-tir/src/tir/op_kinds.toml. DO NOT EDIT.
//
// The single source of truth for the cross-component op-"kind"-string vocabulary
// (docs/design/foundation/25_op_kind_registry.md). These tables back the
// `kind_to_opcode` mapper (ssa.rs), the `CopyLowering` classifier
// (alias_analysis.rs), the per-OpCode effect oracle (effects.rs), and the
// operand-ownership tables (design 27 §2.1/§2.3, consumed by drop_insertion.rs's
// `op_consumed_operand_root`). A drift between this file and op_kinds.toml is
// caught by tests/test_gen_op_kinds.py; a new op kind that the frontend can emit
// but that is absent here is caught by tools/audit_op_kinds.py --check.

use crate::tir::ops::OpCode;

"""


def render_rs(data: dict) -> str:
    return _rustfmt_rust_source(_render_rs_unformatted(data))


def _render_rs_unformatted(data: dict) -> str:
    opcodes = data["opcode"]
    kinds = data.get("kind", [])
    prefixes = data.get("classifier_fresh_value_prefixes", [])

    out: list[str] = [_RS_HEADER]

    # -- kind_to_opcode table ------------------------------------------------
    out.append(
        "/// Map a SimpleIR `kind` string to its first-class TIR `OpCode`, or\n"
        "/// `None` when the kind has no first-class opcode (the caller lifts it to\n"
        "/// `OpCode::Copy{_original_kind}`). Mirrors the `|`-grouped arms in the\n"
        "/// table; the round-trip / legacy spellings live in each row's aliases.\n"
        "#[inline]\n"
        "pub fn kind_to_opcode_table(kind: &str) -> Option<OpCode> {\n"
        "    match kind {\n"
    )
    for row in kinds:
        opcode = row.get("mapper_opcode")
        if opcode is None:
            continue
        if row.get("group") == "gpu":
            out.append(
                "        // GPU offload primitives lower through the call machinery.\n"
            )
        spellings = [row["canonical"], *row.get("aliases", [])]
        pat = " | ".join(f'"{s}"' for s in spellings)
        out.append(f"        {pat} => Some(OpCode::{opcode}),\n")
    out.append("        _ => None,\n")
    out.append("    }\n}\n\n")

    # -- fresh-value classifier exact set ------------------------------------
    fresh = list(data.get("classifier_fresh_value", []))
    out.append(
        "/// EXACT-match arm of `copy_kind_mints_fresh_owned_ref`: kinds whose\n"
        "/// runtime mints a fresh +1 owned reference. The `vec_*` prefix rule is\n"
        "/// applied separately by the caller (see `fresh_value_prefixes`).\n"
        "#[inline]\n"
        "pub fn copy_kind_mints_fresh_owned_ref_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(fresh))
    out.append("    )\n}\n\n")

    # -- owned-alias classifier exact set -------------------------------------
    owned_alias = list(data.get("classifier_owned_alias", []))
    out.append(
        "/// EXACT-match arm of `copy_kind_mints_owned_alias_ref`: kinds whose\n"
        "/// result aliases operand 0's object bits but whose lowering mints a new\n"
        "/// +1 owned reference, so ownership treats the result as an independent\n"
        "/// droppable root.\n"
        "#[inline]\n"
        "pub fn copy_kind_mints_owned_alias_ref_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(owned_alias))
    out.append("    )\n}\n\n")

    # -- exception CreationRef classifier exact set ---------------------------
    exception_creation = list(data.get("classifier_exception_creation_ref", []))
    out.append(
        "/// EXACT-match arm for exception CreationRef producers. These Copy-lifted\n"
        "/// kinds return the fresh exception object reference whose source ownership\n"
        "/// is released at the `raise` boundary after runtime exception state records\n"
        "/// its own references.\n"
        "#[inline]\n"
        "pub fn copy_kind_is_exception_creation_ref_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(exception_creation))
    out.append("    )\n}\n\n")

    # -- fresh-value prefix rule ---------------------------------------------
    out.append(
        "/// Prefix rules for `copy_kind_mints_fresh_owned_ref`: a kind starting\n"
        "/// with any of these mints a fresh owned reference (e.g. the `vec_*`\n"
        "/// vectorized-reduction family, each calling a dedicated `molt_vec_*`).\n"
        "pub const FRESH_VALUE_PREFIXES: &[&str] = &[\n"
    )
    for p in prefixes:
        out.append(f'    "{p}",\n')
    out.append("];\n\n")

    # -- inert-marker classifier exact set -----------------------------------
    inert = list(data.get("classifier_inert_marker", []))
    out.append(
        "/// EXACT-match arm of `classify_copy_kind`'s inert bucket: kinds with a\n"
        "/// dedicated RC-inert backend lowering and no surviving heap reference to\n"
        "/// own (`line`/`trace_*`/`missing`/`nop`, the read-only repr/layout guards).\n"
        "#[inline]\n"
        "pub fn copy_kind_is_inert_marker_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(inert))
    out.append("    )\n}\n\n")

    # -- explicit transparent-alias classifier exact set ---------------------
    transparent_alias = list(data.get("classifier_transparent_alias", []))
    out.append(
        "/// EXACT-match arm of `classify_copy_kind`'s explicit transparent-alias\n"
        "/// bucket: known Copy-lifted runtime ops that intentionally keep the\n"
        "/// drop-insertion fail-closed behavior (not FreshValue, not InertMarker)\n"
        "/// while remaining distinct from `copy_kind_is_explicit_no_heap_move`.\n"
        "/// Membership here DOES NOT grant MemGVN/SROA no-heap-move privileges.\n"
        "#[inline]\n"
        "pub fn copy_kind_is_explicit_transparent_alias_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(transparent_alias))
    out.append("    )\n}\n\n")

    # -- explicit no-heap-move classifier exact set --------------------------
    no_heap = list(data.get("classifier_no_heap_move", []))
    out.append(
        "/// EXACT-match arm of `copy_kind_is_explicit_no_heap_move`: kinds that are\n"
        "/// a provable no-incref pure move of operand 0 (bare `copy`, the named SSA/\n"
        "/// var moves, the validate-and-pass-through guards). A bare `Copy` with no\n"
        "/// `_original_kind` is handled by the caller (it is also a no-heap move).\n"
        "#[inline]\n"
        "pub fn copy_kind_is_explicit_no_heap_move_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(no_heap))
    out.append("    )\n}\n\n")

    # -- effect oracle: exhaustive over OpCode -------------------------------
    may_throw = [r["name"] for r in opcodes if r["may_throw"]]
    side = [r["name"] for r in opcodes if r["side_effecting"]]
    out.append(
        "/// Whether an `OpCode` may raise an exception (DCE must preserve it even\n"
        "/// when its result is dead). EXHAUSTIVE over the enum — a new variant fails\n"
        "/// to compile until it is classified in op_kinds.toml.\n"
        "#[inline]\n"
        "pub fn opcode_may_throw_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, may_throw))
    out.append("    }\n}\n\n")

    out.append(
        "/// Whether an `OpCode` has an observable side effect. EXHAUSTIVE over the\n"
        "/// enum — a new variant fails to compile until it is classified.\n"
        "#[inline]\n"
        "pub fn opcode_is_side_effecting_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, side))
    out.append("    }\n}\n\n")

    out.append(
        "/// Purity class for the LICM/GVN purity core (`opcode_effects`). The\n"
        "/// consumer (effects.rs) maps each variant to its `OpEffects` triple,\n"
        "/// keeping that triple's canonical definition on the consumer side:\n"
        "///   `Pure`         => (consistent, effect_free, nothrow) = (T, T, T)\n"
        "///   `PureMayThrow` => (T, T, F)\n"
        "///   `Impure`       => (F, F, F)\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum OpcodePurity {\n"
        "    Pure,\n"
        "    PureMayThrow,\n"
        "    Impure,\n"
        "}\n\n"
        "/// Per-OpCode purity class. EXHAUSTIVE over the enum — a new variant fails\n"
        "/// to compile until classified in op_kinds.toml.\n"
        "#[inline]\n"
        "pub fn opcode_purity_table(opcode: OpCode) -> OpcodePurity {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_purity_arms(opcodes))
    out.append("    }\n}\n\n")

    out.append(
        "/// Fixed result count for opcodes whose arity is statically known.\n"
        "/// `None` means the opcode has a variable/context-dependent result count.\n"
        "/// EXHAUSTIVE over OpCode so verifier result-count policy cannot drift\n"
        "/// behind newly added opcodes.\n"
        "#[inline]\n"
        "pub fn opcode_fixed_result_count_table(opcode: OpCode) -> Option<usize> {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_result_arity_arms(opcodes))
    out.append("    }\n}\n\n")

    alias_rc_barriers = list(data.get("alias_rc_barrier_opcodes", []))
    alias_heap_barriers = list(data.get("alias_heap_barrier_opcodes", []))
    out.append(
        "/// Whether an opcode is an alias-analysis refcount barrier. EXHAUSTIVE\n"
        "/// over OpCode; the conservative barrier set lives in op_kinds.toml.\n"
        "#[inline]\n"
        "pub fn opcode_is_alias_rc_barrier_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, alias_rc_barriers))
    out.append("    }\n}\n\n")

    out.append(
        "/// Whether an opcode may observe, mutate, or escape arbitrary heap memory\n"
        "/// for alias analysis. EXHAUSTIVE over OpCode.\n"
        "#[inline]\n"
        "pub fn opcode_is_alias_heap_barrier_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, alias_heap_barriers))
    out.append("    }\n}\n\n")

    refcount_heap_exposures = list(data.get("refcount_heap_exposure_opcodes", []))
    out.append(
        "/// Whether this opcode makes its operands heap/external roots for\n"
        "/// deferred reference-count elimination. DISTINCT from alias heap\n"
        "/// barriers: this answers ownership exposure, not memory-def effects.\n"
        "#[inline]\n"
        "pub fn opcode_is_refcount_heap_exposure_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, refcount_heap_exposures))
    out.append("    }\n}\n\n")

    fusion_barriers = list(data.get("fusion_barrier_opcodes", []))
    out.append(
        "/// Whether an opcode makes a comprehension/generator body ineligible for\n"
        "/// deforestation iterator-chain fusion (`sum`/`list`/`map`/`filter`/`any`/\n"
        "/// `all`/`min`/`max` over a `for` loop). This is a DISTINCT fact from\n"
        "/// `opcode_is_side_effecting`: fusion preserves per-element evaluation order\n"
        "/// and count, so allocation/attribute-read/may-throw ops are deliberately\n"
        "/// NOT barriers. The barrier set lives in op_kinds.toml. EXHAUSTIVE over\n"
        "/// OpCode — a new variant fails to compile until it is classified, closing\n"
        "/// the prior default-false drift trap in deforestation's hand-written set.\n"
        "#[inline]\n"
        "pub fn opcode_is_fusion_barrier_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, fusion_barriers))
    out.append("    }\n}\n\n")

    i64_zero_divisor_guards = list(data.get("i64_zero_divisor_guard_opcodes", []))
    out.append(
        "/// Whether a binary opcode needs a proven nonzero RHS before raw i64\n"
        "/// division/remainder lowering may be used, or before CheckException may\n"
        "/// be eliminated after it. This is the shared authority for lower_to_lir.rs\n"
        "/// and check_exception_elim.rs. EXHAUSTIVE over OpCode so a new division\n"
        "/// family opcode cannot silently skip the zero-divisor proof requirement.\n"
        "#[inline]\n"
        "pub fn opcode_requires_i64_zero_divisor_guard_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, i64_zero_divisor_guards))
    out.append("    }\n}\n\n")

    i64_shift_count_guards = list(data.get("i64_shift_count_guard_opcodes", []))
    out.append(
        "/// Whether raw-i64 shift hoist/lowering proofs must prove count in [0, 63].\n"
        "/// EXHAUSTIVE over OpCode so optimizer and backend guards share one\n"
        "/// source of truth for machine-shift count safety.\n"
        "#[inline]\n"
        "pub fn opcode_requires_i64_shift_count_guard_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, i64_shift_count_guards))
    out.append("    }\n}\n\n")

    exception_label_attrs = list(data.get("exception_label_attr_opcodes", []))
    out.append(
        "/// Whether an opcode's `value` attr is a SimpleIR exception label id.\n"
        "/// EXHAUSTIVE over OpCode so cloning/lowering/remapping consumers share\n"
        "/// one source of truth for label-valued exception metadata.\n"
        "#[inline]\n"
        "pub fn opcode_has_exception_label_attr_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, exception_label_attrs))
    out.append("    }\n}\n\n")

    exception_transfer_edges = list(data.get("exception_transfer_edge_opcodes", []))
    out.append(
        "/// Whether an opcode's exception label attr contributes an implicit CFG\n"
        "/// transfer edge. TryEnd deliberately maps false: its label is pairing\n"
        "/// metadata, not a handler branch.\n"
        "#[inline]\n"
        "pub fn opcode_is_exception_transfer_edge_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, exception_transfer_edges))
    out.append("    }\n}\n\n")

    out.append(_render_alias_typed_slot_role(opcodes, data))
    out.append("\n")
    out.append(_render_alias_transparent_alias_role(opcodes, data))
    out.append("\n")
    out.append(_render_alias_memory_region(opcodes, data))
    out.append("\n")
    out.append(_render_alias_slot_observation(opcodes, data))
    out.append("\n")
    out.append(_render_pass_delta_opcode_facts(opcodes, data))
    out.append("\n")

    # -- literal payload facts: exhaustive over OpCode ----------------------
    literal_kinds = {
        row["opcode"]: row["literal"] for row in data.get("literal_payload_opcodes", [])
    }
    out.append(
        "/// Literal payload kind consumers may record for an opcode.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum LiteralPayloadKind {\n"
        "    Int,\n"
        "    Bool,\n"
        "}\n\n"
        "/// Literal payload classifier. EXHAUSTIVE over OpCode; non-literal\n"
        "/// opcodes map to None instead of pass-local wildcards.\n"
        "#[inline]\n"
        "pub fn opcode_literal_payload_kind_table(\n"
        "    opcode: OpCode,\n"
        ") -> Option<LiteralPayloadKind> {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        opcode = row["name"]
        literal = literal_kinds.get(opcode)
        if literal is None:
            out.append(f"        OpCode::{opcode} => None,\n")
        else:
            variant = _LITERAL_PAYLOAD_KINDS[literal]
            out.append(
                f"        OpCode::{opcode} => Some(LiteralPayloadKind::{variant}),\n"
            )
    out.append("    }\n}\n\n")

    # -- canonicalize facts: exhaustive over OpCode -------------------------
    commutative_domains = {
        row["opcode"]: row["domain"]
        for row in data.get("canonicalize_commutative_reorder", [])
    }
    swapped_comparisons = {
        row["opcode"]: row["swapped"]
        for row in data.get("canonicalize_swapped_comparison", [])
    }
    out.append(
        "/// Type domain required before canonicalize.rs may reorder a commutative\n"
        "/// opcode. The domain belongs in the generated opcode oracle; the\n"
        "/// consumer still checks the live operand types before rewriting.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum CanonicalizeCommutativeDomain {\n"
        "    Numeric,\n"
        "    I64,\n"
        "    UnboxedScalar,\n"
        "}\n\n"
        "/// Canonicalize commutative-reorder domain. EXHAUSTIVE over the enum; a new\n"
        "/// opcode cannot silently inherit a false default in canonicalize.rs.\n"
        "#[inline]\n"
        "pub fn opcode_canonicalize_commutative_domain_table(\n"
        "    opcode: OpCode,\n"
        ") -> Option<CanonicalizeCommutativeDomain> {\n"
        "    match opcode {\n"
    )
    out.append(
        _render_canonicalize_commutative_domain_arms(opcodes, commutative_domains)
    )
    out.append("    }\n}\n\n")

    out.append(
        "/// Swapped comparison opcode for canonicalizing constants to the RHS.\n"
        "/// EXHAUSTIVE over OpCode; non-comparison opcodes map to None.\n"
        "#[inline]\n"
        "pub fn opcode_swapped_comparison_for_canonicalize_table(\n"
        "    opcode: OpCode,\n"
        ") -> Option<OpCode> {\n"
        "    match opcode {\n"
    )
    out.append(_render_swapped_comparison_arms(opcodes, swapped_comparisons))
    out.append("    }\n}\n\n")

    out.append(
        _render_canonicalize_binary_rules(
            opcodes,
            data.get("canonicalize_binary_rules", []),
        )
    )
    out.append("\n")

    # -- operand ownership: per-OpCode default + per-spelling consume override --
    out.append(
        _render_operand_ownership(
            opcodes,
            data.get("consuming_kind", []),
            data.get("absorbing_operand_kind", []),
        )
    )
    out.append("\n")
    out.append(
        _render_result_absorption(
            opcodes,
            data.get("absorbing_kind", []),
            data.get("result_finalizer_source_kind", []),
        )
    )
    out.append("\n")
    out.append(_render_result_validity(opcodes, data.get("result_validity", [])))
    out.append("\n")
    out.append(
        _render_explicit_release_operands(
            opcodes, data.get("explicit_release_operand", [])
        )
    )

    # -- per-terminator operand ownership (the ownership-moves-out / transfer axis) --
    out.append("\n")
    out.append(_render_terminator_ownership(data.get("terminator", [])))

    return "".join(out)


def _rustfmt_rust_source(source: str) -> str:
    """Format generated Rust before freshness checks or writes.

    The generated file is compiler-owned, so the formatter is part of the
    generator contract rather than an optional developer cleanup command.
    """
    RUSTFMT_TMP.mkdir(parents=True, exist_ok=True)
    path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            newline="\n",
            suffix=".rs",
            prefix="op_kinds_",
            dir=RUSTFMT_TMP,
            delete=False,
        ) as tmp:
            path = Path(tmp.name)
            tmp.write(source)
        result = harness_memory_guard.guarded_completed_process(
            ["rustfmt", "--edition", "2024", str(path)],
            prefix="MOLT_GENERATOR",
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=60.0,
        )
        result.check_returncode()
        return path.read_text(encoding="utf-8")
    finally:
        if path is not None:
            path.unlink(missing_ok=True)


def _render_matches_arm(spellings: list[str]) -> str:
    """Render the body of a `matches!(kind, ...)` as one `|`-joined OR-pattern,
    one spelling per line, in the order given. Empty set renders a never-match
    arm so the function is still well-formed."""
    if not spellings:
        # An empty exact set means "only the prefix/None paths apply". Render a
        # single impossible literal pattern (a NUL-prefixed kind never occurs as
        # a wire spelling) so the `matches!` stays well-formed and always false.
        return '        "\\0__never__"\n'
    lines = []
    for i, s in enumerate(spellings):
        sep = "" if i == len(spellings) - 1 else " |"
        lines.append(f'        "{s}"{sep}\n')
    return "".join(lines)


def _render_opcode_bool_arms(opcodes: list[dict], truthy: list[str]) -> str:
    """Render exhaustive `OpCode::X => bool` arms in table order."""
    truthy_set = set(truthy)
    lines = []
    for row in opcodes:
        name = row["name"]
        lines.append(f"        OpCode::{name} => {_rs_bool(name in truthy_set)},\n")
    return "".join(lines)


def _render_opcode_result_arity_arms(opcodes: list[dict]) -> str:
    """Render exhaustive `OpCode::X => Option<usize>` result-count arms."""
    rendered = {
        "zero": "Some(0)",
        "one": "Some(1)",
        "two": "Some(2)",
        "variable": "None",
    }
    lines = []
    for row in opcodes:
        name = row["name"]
        lines.append(f"        OpCode::{name} => {rendered[row['result_arity']]},\n")
    return "".join(lines)


def _render_pass_delta_opcode_facts(opcodes: list[dict], data: dict) -> str:
    fact_sets = {
        field: set(data.get(key, [])) for key, field in _PASS_DELTA_FACT_FIELDS
    }
    lines = [
        "/// Pass-delta dashboard opcode facts. These are diagnostic categories,\n",
        "/// not optimizer legality facts, but they still live in op_kinds.toml so\n",
        "/// TIR diagnostics do not grow a second hand-classified opcode registry.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub struct PassDeltaOpcodeFacts {\n",
    ]
    for _key, field in _PASS_DELTA_FACT_FIELDS:
        lines.append(f"    pub {field}: bool,\n")
    lines.extend(
        [
            "}\n\n",
            "const PASS_DELTA_OPCODE_FACTS_NONE: PassDeltaOpcodeFacts = PassDeltaOpcodeFacts {\n",
        ]
    )
    for _key, field in _PASS_DELTA_FACT_FIELDS:
        lines.append(f"    {field}: false,\n")
    lines.extend(
        [
            "};\n\n",
            "/// Per-OpCode pass-delta diagnostic facts. EXHAUSTIVE over OpCode so a\n",
            "/// new opcode cannot silently disappear from pass-delta attribution.\n",
            "#[inline]\n",
            "pub fn opcode_pass_delta_facts_table(opcode: OpCode) -> PassDeltaOpcodeFacts {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        enabled = [
            field for _key, field in _PASS_DELTA_FACT_FIELDS if name in fact_sets[field]
        ]
        if not enabled:
            lines.append(f"        OpCode::{name} => PASS_DELTA_OPCODE_FACTS_NONE,\n")
            continue
        lines.append(f"        OpCode::{name} => PassDeltaOpcodeFacts {{\n")
        for field in enabled:
            lines.append(f"            {field}: true,\n")
        lines.append("            ..PASS_DELTA_OPCODE_FACTS_NONE\n")
        lines.append("        },\n")
    lines.append("    }\n}\n")
    return "".join(lines)


_PURITY_VARIANT = {
    "pure": "OpcodePurity::Pure",
    "pure_may_throw": "OpcodePurity::PureMayThrow",
    "impure": "OpcodePurity::Impure",
}


def _render_opcode_purity_arms(opcodes: list[dict]) -> str:
    lines = []
    for row in opcodes:
        name = row["name"]
        variant = _PURITY_VARIANT[row["purity"]]
        lines.append(f"        OpCode::{name} => {variant},\n")
    return "".join(lines)


_CANONICALIZE_COMMUTATIVE_VARIANT = {
    "numeric": "CanonicalizeCommutativeDomain::Numeric",
    "i64": "CanonicalizeCommutativeDomain::I64",
    "unboxed_scalar": "CanonicalizeCommutativeDomain::UnboxedScalar",
}


def _render_canonicalize_commutative_domain_arms(
    opcodes: list[dict], domains: dict[str, str]
) -> str:
    """Render exhaustive `OpCode::X => Option<CanonicalizeCommutativeDomain>` arms."""
    lines = []
    for row in opcodes:
        name = row["name"]
        domain = domains.get(name)
        if domain is None:
            lines.append(f"        OpCode::{name} => None,\n")
        else:
            lines.append(
                f"        OpCode::{name} => Some({_CANONICALIZE_COMMUTATIVE_VARIANT[domain]}),\n"
            )
    return "".join(lines)


def _render_swapped_comparison_arms(
    opcodes: list[dict], swapped_comparisons: dict[str, str]
) -> str:
    """Render exhaustive `OpCode::X => Option<OpCode>` comparison-swap arms."""
    lines = []
    for row in opcodes:
        name = row["name"]
        swapped = swapped_comparisons.get(name)
        if swapped is None:
            lines.append(f"        OpCode::{name} => None,\n")
        else:
            lines.append(f"        OpCode::{name} => Some(OpCode::{swapped}),\n")
    return "".join(lines)


_ALIAS_SLOT_OBSERVATION_VARIANTS = {
    "alias_slot_direct_observer_opcodes": "AliasSlotObservation::DirectObserver",
    "alias_slot_typed_store_opcodes": "AliasSlotObservation::TypedSlotStore",
    "alias_transparent_type_guard_opcodes": "AliasSlotObservation::TransparentAlias",
    "alias_transparent_copy_opcodes": "AliasSlotObservation::TransparentAlias",
    "alias_slot_never_observer_opcodes": "AliasSlotObservation::NeverObserver",
}


_ALIAS_MEMORY_REGION_VARIANTS = {
    "alias_typed_slot_load_opcodes": "AliasMemoryRegionClass::TypedSlotAttr",
    "alias_typed_slot_store_opcodes": "AliasMemoryRegionClass::TypedSlotAttr",
    "alias_region_copy_refinement_opcodes": "AliasMemoryRegionClass::CopyRefinement",
    "alias_region_container_element_opcodes": "AliasMemoryRegionClass::ContainerElement",
    "alias_region_module_dict_opcodes": "AliasMemoryRegionClass::ModuleDict",
    "alias_memory_inert_opcodes": "AliasMemoryRegionClass::ScalarRegister",
}


_ALIAS_TYPED_SLOT_ROLE_VARIANTS = {
    "alias_typed_slot_load_opcodes": "AliasTypedSlotRole::Load",
    "alias_typed_slot_store_opcodes": "AliasTypedSlotRole::Store",
}


_ALIAS_TRANSPARENT_ALIAS_ROLE_VARIANTS = {
    "alias_transparent_type_guard_opcodes": "AliasTransparentAliasRole::TypeGuard",
    "alias_transparent_copy_opcodes": "AliasTransparentAliasRole::Copy",
}


def _render_alias_typed_slot_role(opcodes: list[dict], data: dict) -> str:
    role_by_opcode: dict[str, str] = {}
    for key, variant in _ALIAS_TYPED_SLOT_ROLE_VARIANTS.items():
        for opcode in data.get(key, []):
            role_by_opcode[opcode] = variant

    out: list[str] = []
    out.append(
        "/// Opcode role for offset-based typed-slot field helpers. Omitted\n"
        "/// opcodes are not typed-slot field candidates.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum AliasTypedSlotRole {\n"
        "    Load,\n"
        "    Store,\n"
        "    NotTypedSlot,\n"
        "}\n\n"
        "/// Typed-slot opcode role for alias_analysis.rs. EXHAUSTIVE over OpCode.\n"
        "#[inline]\n"
        "pub fn opcode_alias_typed_slot_role_table(\n"
        "    opcode: OpCode,\n"
        ") -> AliasTypedSlotRole {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        variant = role_by_opcode.get(name, "AliasTypedSlotRole::NotTypedSlot")
        out.append(f"        OpCode::{name} => {variant},\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_alias_transparent_alias_role(opcodes: list[dict], data: dict) -> str:
    role_by_opcode: dict[str, str] = {}
    for key, variant in _ALIAS_TRANSPARENT_ALIAS_ROLE_VARIANTS.items():
        for opcode in data.get(key, []):
            role_by_opcode[opcode] = variant

    out: list[str] = []
    out.append(
        "/// Opcode role for transparent alias-root propagation. Omitted opcodes\n"
        "/// do not forward object identity through their result.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum AliasTransparentAliasRole {\n"
        "    TypeGuard,\n"
        "    Copy,\n"
        "    NotTransparentAlias,\n"
        "}\n\n"
        "/// Transparent-alias opcode role for alias_analysis.rs. EXHAUSTIVE over OpCode.\n"
        "#[inline]\n"
        "pub fn opcode_alias_transparent_alias_role_table(\n"
        "    opcode: OpCode,\n"
        ") -> AliasTransparentAliasRole {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        variant = role_by_opcode.get(
            name, "AliasTransparentAliasRole::NotTransparentAlias"
        )
        out.append(f"        OpCode::{name} => {variant},\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_alias_memory_region(opcodes: list[dict], data: dict) -> str:
    class_by_opcode: dict[str, str] = {}
    for key, variant in _ALIAS_MEMORY_REGION_VARIANTS.items():
        for opcode in data.get(key, []):
            class_by_opcode[opcode] = variant

    out: list[str] = []
    out.append(
        "/// Alias-analysis memory-region class for an opcode before live\n"
        "/// operand/attribute refinements. Omitted opcodes conservatively map to\n"
        "/// GenericHeap.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum AliasMemoryRegionClass {\n"
        "    TypedSlotAttr,\n"
        "    CopyRefinement,\n"
        "    ContainerElement,\n"
        "    ModuleDict,\n"
        "    ScalarRegister,\n"
        "    GenericHeap,\n"
        "}\n\n"
        "/// Memory-region opcode class for alias_analysis.rs. EXHAUSTIVE over\n"
        "/// OpCode; unlisted opcodes conservatively touch the generic heap.\n"
        "#[inline]\n"
        "pub fn opcode_alias_memory_region_table(\n"
        "    opcode: OpCode,\n"
        ") -> AliasMemoryRegionClass {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        variant = class_by_opcode.get(name, "AliasMemoryRegionClass::GenericHeap")
        out.append(f"        OpCode::{name} => {variant},\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_alias_slot_observation(opcodes: list[dict], data: dict) -> str:
    class_by_opcode: dict[str, str] = {}
    for key, variant in _ALIAS_SLOT_OBSERVATION_VARIANTS.items():
        for opcode in data.get(key, []):
            class_by_opcode[opcode] = variant

    out: list[str] = []
    out.append(
        "/// Alias-analysis slot observation class for an opcode after the caller\n"
        "/// has proven that the op aliases the object root. Omitted opcodes are\n"
        "/// conservative observers.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum AliasSlotObservation {\n"
        "    DirectObserver,\n"
        "    TypedSlotStore,\n"
        "    TransparentAlias,\n"
        "    NeverObserver,\n"
        "    ConservativeObserver,\n"
        "}\n\n"
        "/// Slot-observation opcode class for alias_analysis.rs. EXHAUSTIVE over\n"
        "/// OpCode; unlisted opcodes conservatively observe the slot.\n"
        "#[inline]\n"
        "pub fn opcode_alias_slot_observation_table(\n"
        "    opcode: OpCode,\n"
        ") -> AliasSlotObservation {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        variant = class_by_opcode.get(
            name, "AliasSlotObservation::ConservativeObserver"
        )
        out.append(f"        OpCode::{name} => {variant},\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_canonicalize_binary_rules(opcodes: list[dict], rows: list[dict]) -> str:
    """Render canonicalize.rs's ordered binary fold rules as typed data."""
    by_opcode: dict[str, list[dict]] = {}
    for row in rows:
        by_opcode.setdefault(row["opcode"], []).append(row)

    out: list[str] = []
    out.append(
        "/// Operand side used by canonicalize binary rule predicates/actions.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum CanonicalizeOperandSide {\n"
        "    Lhs,\n"
        "    Rhs,\n"
        "}\n\n"
        "/// Predicate for one ordered binary canonicalization rule.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum CanonicalizeBinaryPredicate {\n"
        "    IntConst {\n"
        "        side: CanonicalizeOperandSide,\n"
        "        value: i64,\n"
        "    },\n"
        "    BoolConst {\n"
        "        side: CanonicalizeOperandSide,\n"
        "        value: bool,\n"
        "    },\n"
        "    SameOperands,\n"
        "}\n\n"
        "/// Live type guard for one binary canonicalization rule.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum CanonicalizeBinaryTypeGuard {\n"
        "    None,\n"
        "    OperandI64(CanonicalizeOperandSide),\n"
        "}\n\n"
        "/// Rewrite action for one binary canonicalization rule.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub enum CanonicalizeBinaryAction {\n"
        "    Copy(CanonicalizeOperandSide),\n"
        "    ConstInt(i64),\n"
        "    ConstBool(bool),\n"
        "}\n\n"
        "/// Ordered binary canonicalization rule. The pass evaluates rows in table\n"
        "/// order and applies the first match, preserving the previous match-arm\n"
        "/// priority without keeping opcode semantics in canonicalize.rs.\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub struct CanonicalizeBinaryRule {\n"
        "    pub predicate: CanonicalizeBinaryPredicate,\n"
        "    pub type_guard: CanonicalizeBinaryTypeGuard,\n"
        "    pub action: CanonicalizeBinaryAction,\n"
        "}\n\n"
    )

    for row in opcodes:
        opcode = row["name"]
        rules = by_opcode.get(opcode)
        if not rules:
            continue
        out.append(
            f"const {_canonicalize_binary_rules_const(opcode)}: &[CanonicalizeBinaryRule] = &[\n"
        )
        for rule in rules:
            out.append("    CanonicalizeBinaryRule {\n")
            out.append(
                f"        predicate: {_render_canonicalize_binary_predicate(rule)},\n"
            )
            out.append(
                f"        type_guard: {_render_canonicalize_binary_type_guard(rule)},\n"
            )
            out.append(f"        action: {_render_canonicalize_binary_action(rule)},\n")
            out.append("    },\n")
        out.append("];\n\n")

    out.append(
        "/// Ordered binary canonicalization rules. EXHAUSTIVE over OpCode; opcodes\n"
        "/// without binary folds map to the empty rule slice.\n"
        "#[inline]\n"
        "pub fn opcode_canonicalize_binary_rules_table(\n"
        "    opcode: OpCode,\n"
        ") -> &'static [CanonicalizeBinaryRule] {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        opcode = row["name"]
        if opcode in by_opcode:
            out.append(
                f"        OpCode::{opcode} => {_canonicalize_binary_rules_const(opcode)},\n"
            )
        else:
            out.append(f"        OpCode::{opcode} => &[],\n")
    out.append("    }\n}\n")
    return "".join(out)


def _canonicalize_binary_rules_const(opcode: str) -> str:
    words: list[str] = []
    current = ""
    for ch in opcode:
        if ch.isupper() and current:
            words.append(current)
            current = ch
        else:
            current += ch
    if current:
        words.append(current)
    suffix = "_".join(w.upper() for w in words)
    return f"CANONICALIZE_BINARY_RULES_{suffix}"


def _canonicalize_operand_side(side: str) -> str:
    if side == "lhs":
        return "CanonicalizeOperandSide::Lhs"
    if side == "rhs":
        return "CanonicalizeOperandSide::Rhs"
    raise AssertionError(f"unknown canonicalize operand side: {side!r}")


def _render_canonicalize_binary_predicate(row: dict) -> str:
    predicate = row["predicate"]
    if predicate == "same_operands":
        return "CanonicalizeBinaryPredicate::SameOperands"
    side, value_kind = predicate.split("_", 1)
    side_rs = _canonicalize_operand_side(side)
    if value_kind == "int":
        return (
            "CanonicalizeBinaryPredicate::IntConst { "
            f"side: {side_rs}, value: {row['value']} "
            "}"
        )
    if value_kind == "bool":
        return (
            "CanonicalizeBinaryPredicate::BoolConst { "
            f"side: {side_rs}, value: {_rs_bool(row['value'])} "
            "}"
        )
    raise AssertionError(f"unknown canonicalize binary predicate: {predicate!r}")


def _render_canonicalize_binary_type_guard(row: dict) -> str:
    guard = row["type_guard"]
    if guard == "none":
        return "CanonicalizeBinaryTypeGuard::None"
    side, ty = guard.split("_", 1)
    if ty == "i64":
        return (
            "CanonicalizeBinaryTypeGuard::OperandI64("
            f"{_canonicalize_operand_side(side)})"
        )
    raise AssertionError(f"unknown canonicalize binary type guard: {guard!r}")


def _render_canonicalize_binary_action(row: dict) -> str:
    action = row["action"]
    if action == "copy_lhs":
        return "CanonicalizeBinaryAction::Copy(CanonicalizeOperandSide::Lhs)"
    if action == "copy_rhs":
        return "CanonicalizeBinaryAction::Copy(CanonicalizeOperandSide::Rhs)"
    if action == "const_int":
        return f"CanonicalizeBinaryAction::ConstInt({row['result']})"
    if action == "const_bool":
        return f"CanonicalizeBinaryAction::ConstBool({_rs_bool(row['result'])})"
    raise AssertionError(f"unknown canonicalize binary action: {action!r}")


_OPERAND_OWNERSHIP_VARIANT = {
    "borrowed": "OperandOwnership::Borrowed",
    "consumed": "OperandOwnership::Consumed",
    # The borrow-of-edge leaf (design 27 §1.5 / §2.1, ladder #73): a per-position
    # opcode operand whose result holds an interior reference into it (the
    # `LoadAttr`/`Index` source — the round-6 `Counter._handle` keepalive). Read by
    # `opcode_borrows_source_operand` and `op_borrow_source` in alias_analysis.rs.
    "interior_borrow_keepalive": "OperandOwnership::InteriorBorrowKeepAlive",
    # Existing-container store leaf: the op borrows the operand while retaining
    # its own container/storage reference. DropInsertion uses this as a release
    # boundary for finalizer-sensitive producer temps.
    "container_absorb": "OperandOwnership::ContainerAbsorb",
    # Move-out leaves used by the per-TERMINATOR table (design 27 §2.4). The
    # opcode `operand_ownership` validator restricts opcodes to
    # borrowed|consumed|interior_borrow_keepalive|container_absorb; these are reachable only via
    # the terminator categories.
    "transferred": "OperandOwnership::Transferred",
    "none": "OperandOwnership::NoOperand",
}

_RESULT_VALIDITY_VARIANT = {
    "conditional_valid_only_on_edge": "ResultValidity::ConditionalValidOnlyOnEdge",
}


def _render_operand_ownership(
    opcodes: list[dict],
    consuming: list[dict],
    absorbing_operands: list[dict],
) -> str:
    """Render the operand-ownership tables (design 27 §2.1/§2.3):

    * ``OperandOwnership`` — the per-operand borrowed|consumed leaf.
    * ``opcode_operand_ownership_table(opcode, operand_idx)`` — the per-OpCode
      DEFAULT, EXHAUSTIVE over the enum (a new variant fails to compile until
      classified). Honors the per-position list form (a list opcode dispatches
      on ``operand_idx``); a uniform opcode ignores the index.
    * ``kind_consumed_operand_table(kind, arity)`` — the per-SPELLING consume
      override keyed on the ``_original_kind`` attr. Returns the 0-based index
      of the consumed operand, resolving ``"last"`` against the op's ``arity``.
      This is the table ``op_consumed_operand_root`` reads (replacing the
      hand-coded ``matches!(_original_kind, "call_bind" | "call_indirect")``).
    """
    out: list[str] = []
    # `operand_idx` is referenced by the match body ONLY when some opcode carries
    # a per-position list (which renders a `match operand_idx { … }` arm). When
    # every opcode is uniform (`all_borrowed`/`all_consumed`), the index is
    # genuinely unused — emit the idiomatic `_operand_idx` so the generated file
    # stays warning-free (rather than an `#[allow]` blanket). The PUBLIC contract
    # is still "indexed by operand position"; the name flips to `operand_idx` the
    # moment a per-position classification lands.
    any_per_position = any(
        isinstance(row["operand_ownership"], list)
        and len(set(row["operand_ownership"])) > 1
        for row in opcodes
    )
    idx_param = "operand_idx" if any_per_position else "_operand_idx"
    out.append(
        "/// Operand-ownership leaf (design 27 §2.1): does an op release this\n"
        "/// operand internally (`Consumed` — the holder must NOT also drop it, a\n"
        "/// double-free otherwise) or merely borrow it (`Borrowed` — the holder\n"
        "/// keeps its obligation and drops at the value's true last use)? molt's\n"
        "/// `callee borrows all args` ABI (design 20 §1.2) makes `Borrowed` the\n"
        "/// universal default; `Consumed` is the CallArgs-builder / move-into class.\n"
        "/// The result-side lattice (Owned/Borrowed/Raw/MaybeUninit) is the\n"
        "/// classifier_* tables — a SEPARATE axis from this operand-side leaf.\n"
        "///\n"
        "/// The variant set models molt's FULL operand-ownership domain so the\n"
        "/// design-27 ownership-boundary lattice (#58) and the next consumer\n"
        "/// migrations are TABLE edits, not enum surgery. `Borrowed`/`Consumed`\n"
        "/// seed the per-OpCode + per-spelling tables; `InteriorBorrowKeepAlive`\n"
        "/// seeds the per-position borrow-of column (ladder #73);\n"
        "/// `ContainerAbsorb` marks borrowed operands retained by container/storage\n"
        "/// mutation; `Transferred`\n"
        "/// seeds the per-TERMINATOR table (design 27 §2.4 transfer sites — ladder\n"
        "/// #72). Every variant below is constructed by a generated table today:\n"
        "///   * `Transferred` — ownership moves OUT of the function/block: a\n"
        "///     `Return` value or a branch-arg passed into a successor block arg.\n"
        "///     LIVE: constructed by `terminator_operand_ownership_table` and read\n"
        "///     by drop_insertion's `terminator_uses_root` / `terminator_branch_args`.\n"
        "///   * `InteriorBorrowKeepAlive` — the round-6 interior-borrow keepalive:\n"
        "///     the operand must stay live because the result holds an INTERIOR\n"
        "///     reference into it (drop deferred to the interior ref's last use).\n"
        "///     LIVE: constructed by `opcode_operand_ownership_table` for the\n"
        "///     `LoadAttr`/`Index` source position and read by\n"
        "///     `opcode_borrows_source_operand` / `op_borrow_source` to build the\n"
        "///     `BorrowProvenance` relation (the `Counter._handle` UAF fix).\n"
        "///   * `ContainerAbsorb` — an existing-container/store mutation retains\n"
        "///     this operand while the caller still owns the producer temp ref. This\n"
        "///     gives DropInsertion a shared release boundary for absorbed temps\n"
        "///     without making the mutator consume the operand.\n"
        "///   * `ConditionalValidOnlyOnEdge` — the §2.8 `IterNextUnboxed` value-out:\n"
        "///     valid only on the not-exhausted edge, NEVER unconditionally\n"
        "///     droppable (non-owned `None` sentinel on the exhaustion edge). The LONE\n"
        "///     remaining `from_str`-only variant (its consumer hand-list —\n"
        "///     `iter_cond_value_results` — migrates in the iter-cond tranche, #74).\n"
        "///   * `NoOperand` — no ref-bearing operand in that category (a\n"
        "///     raw lane; a terminator category absent on a variant — `Branch` has\n"
        "///     no direct operand, `Return` forwards no branch arg).\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum OperandOwnership {\n"
        "    Borrowed,\n"
        "    Consumed,\n"
        "    Transferred,\n"
        "    InteriorBorrowKeepAlive,\n"
        "    ContainerAbsorb,\n"
        "    ConditionalValidOnlyOnEdge,\n"
        "    NoOperand,\n"
        "}\n\n"
        "// Parse/render path for the operand-ownership vocabulary. `Transferred`\n"
        "// is LIVE through `terminator_operand_ownership_table` (ladder #72) and\n"
        "// `InteriorBorrowKeepAlive` through `opcode_operand_ownership_table` /\n"
        "// `opcode_borrows_source_operand` (ladder #73); `from_str` remains the\n"
        "// toml-ingest path the LAST migration (the `conditional_valid_only_on_edge`\n"
        "// row, #74) reads and is not yet wired to a runtime caller, so\n"
        "// `from_str`/`as_str`/`ALL` keep allow(dead_code) — SCOPED to this\n"
        "// forward-compat parse API, never the enum (every variant is constructed)\n"
        "// nor the file. `ALL` + the round-trip test keep every variant constructed\n"
        "// and live today.\n"
        "#[allow(dead_code)]\n"
        "impl OperandOwnership {\n"
        "    pub const ALL: [OperandOwnership; 7] = [\n"
        "        OperandOwnership::Borrowed,\n"
        "        OperandOwnership::Consumed,\n"
        "        OperandOwnership::Transferred,\n"
        "        OperandOwnership::InteriorBorrowKeepAlive,\n"
        "        OperandOwnership::ContainerAbsorb,\n"
        "        OperandOwnership::ConditionalValidOnlyOnEdge,\n"
        "        OperandOwnership::NoOperand,\n"
        "    ];\n"
        "    pub fn as_str(self) -> &'static str {\n"
        "        match self {\n"
        '            OperandOwnership::Borrowed => "borrowed",\n'
        '            OperandOwnership::Consumed => "consumed",\n'
        '            OperandOwnership::Transferred => "transferred",\n'
        '            OperandOwnership::InteriorBorrowKeepAlive => "interior_borrow_keepalive",\n'
        '            OperandOwnership::ContainerAbsorb => "container_absorb",\n'
        '            OperandOwnership::ConditionalValidOnlyOnEdge => "conditional_valid_only_on_edge",\n'
        '            OperandOwnership::NoOperand => "no_operand_ownership",\n'
        "        }\n"
        "    }\n"
        "    pub fn from_str(s: &str) -> Option<OperandOwnership> {\n"
        "        match s {\n"
        '            "borrowed" => Some(OperandOwnership::Borrowed),\n'
        '            "consumed" => Some(OperandOwnership::Consumed),\n'
        '            "transferred" => Some(OperandOwnership::Transferred),\n'
        '            "interior_borrow_keepalive" => Some(OperandOwnership::InteriorBorrowKeepAlive),\n'
        '            "container_absorb" => Some(OperandOwnership::ContainerAbsorb),\n'
        '            "conditional_valid_only_on_edge" => Some(OperandOwnership::ConditionalValidOnlyOnEdge),\n'
        '            "no_operand_ownership" => Some(OperandOwnership::NoOperand),\n'
        "            _ => None,\n"
        "        }\n"
        "    }\n"
        "}\n\n"
        "#[cfg(test)]\n"
        "mod operand_ownership_schema_tests {\n"
        "    use super::OperandOwnership;\n"
        "    #[test]\n"
        "    fn every_variant_round_trips() {\n"
        "        // The schema is alive: every declared variant parses + renders +\n"
        "        // round-trips. Dropping or renaming a variant breaks this test.\n"
        "        for v in OperandOwnership::ALL {\n"
        "            assert_eq!(OperandOwnership::from_str(v.as_str()), Some(v));\n"
        "        }\n"
        '        assert_eq!(OperandOwnership::from_str("bogus"), None);\n'
        "    }\n"
        "}\n\n"
    )

    out.append(
        "/// Per-OpCode operand-ownership DEFAULT: how `OpCode` treats the operand\n"
        "/// at `operand_idx`. EXHAUSTIVE over the enum — a new variant fails to\n"
        "/// compile until it is given an `operand_ownership` row in op_kinds.toml.\n"
        "/// A uniform opcode (`all_borrowed`/`all_consumed`) ignores the index; a\n"
        "/// per-position opcode dispatches on it (positions past the listed arity\n"
        "/// fall back to the LAST listed leaf — variadic tails inherit the final\n"
        "/// position's treatment). This is the per-OpCode floor; a finer\n"
        "/// per-`_original_kind` consume is `kind_consumed_operand_table`.\n"
        "#[inline]\n"
        "pub fn opcode_operand_ownership_table(\n"
        "    opcode: OpCode,\n"
        f"    {idx_param}: usize,\n"
        ") -> OperandOwnership {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        spec = row["operand_ownership"]
        out.append(f"        OpCode::{name} => {_operand_ownership_arm(spec)},\n")
    out.append("    }\n}\n\n")

    # Derived borrow-of authority (design 27 §1.5 / §2.1, ladder #73): the
    # operand index an opcode's result interior-borrows (its
    # `interior_borrow_keepalive` position), or `None`. This is the single
    # declarative fact `op_borrow_source` (alias_analysis.rs) reads — the migrated
    # interior-borrow-keepalive relation, no longer a hardcoded `LoadAttr | Index`
    # match. EXHAUSTIVE over the enum (every opcode is classified by its
    # `operand_ownership` row). A future op whose result interior-borrows an
    # operand gets correct keepalive by setting that position to
    # `interior_borrow_keepalive` in op_kinds.toml — never by editing the pass.
    out.append(
        "/// The operand index whose backing store this op's result interior-borrows\n"
        "/// (design 27 §1.5 borrow-of edge): the operand position classified\n"
        "/// `OperandOwnership::InteriorBorrowKeepAlive`, or `None` if the op's result\n"
        "/// borrows into no operand. Derived from the per-OpCode `operand_ownership`\n"
        "/// row — the SINGLE declarative authority `op_borrow_source`\n"
        "/// (alias_analysis.rs) reads to build the `BorrowProvenance` keepalive\n"
        "/// relation, REPLACING the hand-coded\n"
        "/// `LoadAttr | Index` match (the round-6 `Counter._handle` UAF fix). The\n"
        "/// source object's drop is deferred to the borrow result's last use, so a\n"
        "/// finalizer that owns the backing store cannot run while the borrow lives.\n"
        "/// EXHAUSTIVE over the enum — a new interior-borrowing op is classified by a\n"
        "/// table edit, not a pass edit. At most one interior-borrow operand exists in\n"
        "/// molt's lowering today (the container/object at position 0); the first such\n"
        "/// position is returned.\n"
        "#[inline]\n"
        "pub fn opcode_borrows_source_operand(opcode: OpCode) -> Option<usize> {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        idx = _borrows_source_operand_index(row["operand_ownership"])
        if idx is not None:
            out.append(f"        OpCode::{name} => Some({idx}),\n")
    out.append("        _ => None,\n")
    out.append("    }\n}\n\n")

    out.append(
        "/// The operand index retained by an existing container/store mutation.\n"
        "/// The op still borrows the operand for ABI/drop purposes; this fact only\n"
        "/// records that the container now owns its own reference, so a\n"
        "/// finalizer-sensitive producer temp can release its caller-owned ref at\n"
        "/// this statement. Derived from `container_absorb` operand rows.\n"
        "#[inline]\n"
        "pub fn opcode_container_absorbed_operand(opcode: OpCode) -> Option<usize> {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        idx = _container_absorb_operand_index(row["operand_ownership"])
        if idx is not None:
            out.append(f"        OpCode::{name} => Some({idx}),\n")
    out.append("        _ => None,\n")
    out.append("    }\n}\n\n")

    out.append(
        "/// Per-SPELLING consume override (design 27 §2.3): for a `Copy`-lifted op\n"
        "/// carrying `_original_kind = kind`, the 0-based index of the operand the\n"
        "/// op CONSUMES (frees internally), or `None` if it consumes none. `arity`\n"
        '/// is the op\'s operand count, used to resolve a `"last"` selector. The\n'
        "/// drop pass treats a value whose last use is the consumed-operand\n"
        "/// position exactly like a `Return` transfer — no trailing `DecRef`.\n"
        "/// Replaces the hand-coded `op_consumed_operand_root` match.\n"
        "#[inline]\n"
        "pub fn kind_consumed_operand_table(kind: &str, arity: usize) -> Option<usize> {\n"
        "    match kind {\n"
    )
    if consuming:
        for row in consuming:
            kind = row["kind"]
            sel = row["consumed_operand"]
            if sel == "last":
                out.append(f'        "{kind}" => arity.checked_sub(1),\n')
            else:
                out.append(f'        "{kind}" => Some({int(sel)}),\n')
    out.append("        _ => None,\n")
    out.append("    }\n}\n")
    absorbed_uses_arity = any(
        row["absorbed_operand"] == "last" for row in absorbing_operands
    )
    absorbed_arity_param = "arity" if absorbed_uses_arity else "_arity"
    out.append(
        "\n/// Per-SPELLING existing-container absorption override. These preserved\n"
        "/// SimpleIR spellings lower as `Copy` with `_original_kind`, so they need a\n"
        "/// spelling table parallel to `kind_consumed_operand_table`.\n"
        "#[inline]\n"
        f"pub fn kind_container_absorbed_operand_table(kind: &str, {absorbed_arity_param}: usize) -> Option<usize> {{\n"
        "    match kind {\n"
    )
    if absorbing_operands:
        for row in absorbing_operands:
            kind = row["kind"]
            sel = row["absorbed_operand"]
            if sel == "last":
                out.append(f'        "{kind}" => arity.checked_sub(1),\n')
            else:
                out.append(f'        "{kind}" => Some({int(sel)}),\n')
    out.append("        _ => None,\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_result_absorption(
    opcodes: list[dict],
    absorbing: list[dict],
    result_sources: list[dict],
) -> str:
    """Render the result-absorbs-operands ownership-transfer tables.

    This is a RESULT-side fact: the returned value owns the operands' lifetimes
    even though the operands remain borrowed at the ABI/drop-insertion edge.
    First-class opcodes use the exhaustive opcode bit; Copy-lifted SimpleIR
    spellings use the spelling table.
    """
    out: list[str] = []
    out.append(
        "/// Result-side ownership-transfer fact: this op returns a value whose\n"
        "/// lifetime absorbs the lifetimes of its operands (container builders).\n"
        "/// This is deliberately separate from operand_ownership: operands are still\n"
        "/// borrowed at the call/drop boundary, but a finalizer-sensitive operand\n"
        "/// makes the returned container finalizer-sensitive. EXHAUSTIVE over\n"
        "/// OpCode; Copy-lifted spellings use `kind_result_absorbs_operand_ownership_table`.\n"
        "#[inline]\n"
        "pub fn opcode_result_absorbs_operand_ownership_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        out.append(
            f"        OpCode::{row['name']} => {_rs_bool(row['result_absorbs_operands'])},\n"
        )
    out.append("    }\n}\n\n")

    out.append(
        "/// Result-side selected-alias ownership fact. These opcodes return one\n"
        "/// borrowed operand's bits as their result, so backend lowering must\n"
        "/// retain the selected object when an owned boxed result is produced.\n"
        "/// Raw scalar lanes remain refcount-free. The table is keyed by explicit\n"
        "/// `result_mints_owned_selected_operand` rows in op_kinds.toml.\n"
        "#[inline]\n"
        "pub fn opcode_result_mints_owned_selected_operand_table(opcode: OpCode) -> bool {\n"
    )
    selected_owner_opcodes = sorted(
        row["name"]
        for row in opcodes
        if row.get("result_mints_owned_selected_operand", False)
    )
    if selected_owner_opcodes:
        out.append("    matches!(\n        opcode,\n")
        for i, name in enumerate(selected_owner_opcodes):
            sep = "" if i == len(selected_owner_opcodes) - 1 else " |"
            out.append(f"        OpCode::{name}{sep}\n")
        out.append("    )\n")
    else:
        out.append("    let _ = opcode;\n    false\n")
    out.append("}\n\n")

    out.append(
        "/// Same selected-alias result ownership fact keyed by SimpleIR kind spelling.\n"
        "/// String-dispatch backends must query this rather than duplicating an\n"
        "/// `and`/`or` list by hand.\n"
        "#[inline]\n"
        "pub fn kind_result_mints_owned_selected_operand_table(kind: &str) -> bool {\n"
        "    kind_to_opcode_table(kind)\n"
        "        .is_some_and(opcode_result_mints_owned_selected_operand_table)\n"
        "}\n\n"
    )

    out.append(
        "/// Result-side ownership-transfer fact for Copy-lifted SimpleIR spellings.\n"
        "/// These spellings intentionally remain outside `[[kind]]` so backconversion\n"
        "/// and backend dispatch preserve their public wire names while still sharing\n"
        "/// the finalizer/escape ownership fact with first-class Build* opcodes.\n"
        "#[inline]\n"
        "pub fn kind_result_absorbs_operand_ownership_table(kind: &str) -> bool {\n"
        "    matches!(kind,\n"
    )
    absorbing_kinds = sorted(row["kind"] for row in absorbing)
    out.append(_render_matches_arm(absorbing_kinds))
    out.append("    )\n}\n\n")

    result_source_uses_arity = any(
        row["source_operand"] == "last" for row in result_sources
    )
    result_source_arity_param = "arity" if result_source_uses_arity else "_arity"
    out.append(
        "/// Per-SPELLING result finalizer-source facts. These Copy-lifted\n"
        "/// extraction spellings return a fresh owned result whose finalizer\n"
        "/// sensitivity is inherited from one source operand, but whose own\n"
        "/// temporary ref should release at the statement unless Python-bound.\n"
        "#[inline]\n"
        f"pub fn kind_result_finalizer_source_operand_table(kind: &str, {result_source_arity_param}: usize) -> Option<usize> {{\n"
        "    match kind {\n"
    )
    if result_sources:
        for row in result_sources:
            kind = row["kind"]
            sel = row["source_operand"]
            if sel == "last":
                out.append(f'        "{kind}" => arity.checked_sub(1),\n')
            else:
                out.append(f'        "{kind}" => Some({int(sel)}),\n')
    out.append("        _ => None,\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_result_validity(opcodes: list[dict], rows: list[dict]) -> str:
    """Render per-result validity facts.

    `IterNextUnboxed` result 0 is only initialized on the not-done edge. The
    table keeps that path-sensitive result fact beside the other op-kind
    semantics instead of duplicating it inside drop insertion.
    """
    by_opcode: dict[str, dict[int, str]] = {}
    for row in rows:
        by_opcode.setdefault(row["opcode"], {})[row["result"]] = row["validity"]

    out: list[str] = []
    out.append(
        "/// Result-validity fact for op results whose bits are not valid on every\n"
        "/// outgoing edge. `ConditionalValidOnlyOnEdge` is the §2.8\n"
        "/// `IterNextUnboxed` value-out: result 0 is initialized only on the\n"
        "/// not-done edge and must never be dropped or retained from the exhaustion\n"
        "/// edge. EXHAUSTIVE over OpCode; result indices not listed for an opcode\n"
        "/// are unconditionally valid.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum ResultValidity {\n"
        "    AlwaysValid,\n"
        "    ConditionalValidOnlyOnEdge,\n"
        "}\n\n"
        "#[inline]\n"
        "pub fn opcode_result_validity_table(\n"
        "    opcode: OpCode,\n"
        "    result_idx: usize,\n"
        ") -> ResultValidity {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        result_rows = by_opcode.get(name, {})
        if not result_rows:
            out.append(f"        OpCode::{name} => ResultValidity::AlwaysValid,\n")
            continue
        out.append(f"        OpCode::{name} => match result_idx {{\n")
        for idx in sorted(result_rows):
            variant = _RESULT_VALIDITY_VARIANT[result_rows[idx]]
            out.append(f"            {idx} => {variant},\n")
        out.append("            _ => ResultValidity::AlwaysValid,\n")
        out.append("        },\n")
    out.append("    }\n}\n\n")
    out.append(
        "#[inline]\n"
        "pub fn opcode_result_is_conditionally_valid_only_on_edge(\n"
        "    opcode: OpCode,\n"
        "    result_idx: usize,\n"
        ") -> bool {\n"
        "    matches!(\n"
        "        opcode_result_validity_table(opcode, result_idx),\n"
        "        ResultValidity::ConditionalValidOnlyOnEdge\n"
        "    )\n"
        "}\n"
    )
    return "".join(out)


def _render_explicit_release_operands(opcodes: list[dict], rows: list[dict]) -> str:
    """Render Python lifetime release-boundary operand facts."""
    by_opcode = {row["opcode"]: row["operand"] for row in rows}
    uses_arity = any(row["operand"] == "last" for row in rows)
    arity_param = "arity" if uses_arity else "_arity"
    out: list[str] = []
    out.append(
        "/// Python lifetime release-boundary fact: which operand roots an opcode\n"
        "/// explicitly releases. This is separate from operand ownership: `DecRef`\n"
        "/// consumes/releases all operands, while `DeleteVar` releases the old slot\n"
        "/// occupant at operand 1 after storing the missing sentinel. DropInsertion\n"
        "/// uses this table to avoid a pass-local `DecRef | DeleteVar` hand list.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum ExplicitReleaseOperands {\n"
        "    None,\n"
        "    All,\n"
        "    One(usize),\n"
        "}\n\n"
        "#[inline]\n"
        "pub fn opcode_explicit_release_operands_table(\n"
        "    opcode: OpCode,\n"
        f"    {arity_param}: usize,\n"
        ") -> ExplicitReleaseOperands {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        operand = by_opcode.get(name)
        if operand is None:
            out.append(f"        OpCode::{name} => ExplicitReleaseOperands::None,\n")
        elif operand == "all":
            out.append(f"        OpCode::{name} => ExplicitReleaseOperands::All,\n")
        elif operand == "last":
            out.append(
                f"        OpCode::{name} => match arity.checked_sub(1) {{\n"
                "            Some(idx) => ExplicitReleaseOperands::One(idx),\n"
                "            None => ExplicitReleaseOperands::None,\n"
                "        },\n"
            )
        else:
            out.append(
                f"        OpCode::{name} => ExplicitReleaseOperands::One({int(operand)}),\n"
            )
    out.append("    }\n}\n")
    return "".join(out)


def _render_terminator_ownership(terminators: list[dict]) -> str:
    """Render the per-TERMINATOR operand-ownership authority (design 27 §2.4):

    * ``TerminatorKind`` — a zero-cost discriminant of the ``Terminator`` enum
      (blocks.rs) the table is keyed on (the drop pass maps ``&Terminator`` ->
      ``TerminatorKind`` with one structural match). EXHAUSTIVE over the enum.
    * ``OperandCategory`` — ``Direct`` (the terminator's own operands: a
      ``Return`` value, a ``CondBranch``/``Switch`` predicate) vs ``BranchArg``
      (a value forwarded into a successor's phi). The two categories have
      different ownership, so they are classified independently.
    * ``terminator_operand_ownership_table(kind, category)`` — the per-(variant,
      category) ``OperandOwnership`` leaf, EXHAUSTIVE over both axes.
    * ``terminator_operand_is_transferred(kind, category)`` — the derived
      predicate drop_insertion reads: ``true`` iff the leaf is ``Transferred``
      (ownership moves OUT — no trailing ``DecRef`` at the transfer point). This
      is the generated authority that REPLACES the hand-coded transfer carve-out
      in ``terminator_branch_args`` + the ``Return`` arm of ``terminator_uses_root``.
    """
    out: list[str] = []
    out.append(
        "/// Zero-cost discriminant of the `Terminator` enum (blocks.rs) the\n"
        "/// per-terminator operand-ownership table is keyed on. EXHAUSTIVE over the\n"
        "/// enum — a new `Terminator` variant fails to render until it is given a\n"
        "/// [[terminator]] row in op_kinds.toml (the transfer-carve-out kill: an\n"
        "/// unclassified terminator can't silently inherit a borrow/transfer\n"
        "/// assumption). The drop pass maps `&Terminator` -> `TerminatorKind` with\n"
        "/// one structural match; this keeps the ownership FACT declarative while\n"
        "/// the structural shape (which fields carry args) stays in the pass.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum TerminatorKind {\n"
    )
    for row in terminators:
        out.append(f"    {row['name']},\n")
    out.append("}\n\n")

    out.append(
        "/// Which operand CATEGORY of a terminator a query is about: the\n"
        "/// terminator's own `Direct` operands (a `Return` value, a `CondBranch`/\n"
        "/// `Switch` predicate) versus a `BranchArg` forwarded into a successor's\n"
        "/// block-arg (phi). The two have different ownership (a `Return` value\n"
        "/// transfers to the caller; a predicate is borrowed; a branch-arg transfers\n"
        "/// into the phi) so they are classified on separate axes.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum OperandCategory {\n"
        "    Direct,\n"
        "    BranchArg,\n"
        "}\n\n"
    )

    out.append(
        "/// Per-(terminator variant, operand category) ownership leaf (design 27\n"
        "/// §2.4). EXHAUSTIVE over both axes — a new `Terminator` variant fails to\n"
        "/// compile until classified. `Transferred` = ownership moves OUT (a\n"
        "/// `Return` value to the caller; a branch-arg into a successor phi);\n"
        "/// `Borrowed` = the predicate is read but not moved (drop relocated to the\n"
        "/// dying edge); `NoOperand` = the variant has no operand in that\n"
        "/// category. The consume axis is N/A for a terminator (nothing frees a\n"
        "/// terminator operand internally), so `Consumed` never appears here.\n"
        "#[inline]\n"
        "pub fn terminator_operand_ownership_table(\n"
        "    kind: TerminatorKind,\n"
        "    category: OperandCategory,\n"
        ") -> OperandOwnership {\n"
        "    match (kind, category) {\n"
    )
    for row in terminators:
        name = row["name"]
        direct = _OPERAND_OWNERSHIP_VARIANT[row["direct"]]
        branch = _OPERAND_OWNERSHIP_VARIANT[row["branch_arg"]]
        out.append(
            f"        (TerminatorKind::{name}, OperandCategory::Direct) => {direct},\n"
        )
        out.append(
            f"        (TerminatorKind::{name}, OperandCategory::BranchArg) => {branch},\n"
        )
    out.append("    }\n}\n\n")

    out.append(
        "/// Derived transfer predicate drop_insertion reads (design 27 §2.4): does\n"
        "/// the terminator TRANSFER ownership of an operand in `category`? `true`\n"
        "/// iff the leaf is `Transferred` — the drop pass must NOT emit a trailing\n"
        "/// `DecRef` at the transfer point (the caller / successor phi owns it).\n"
        "/// This single declarative authority REPLACES the hand-coded transfer\n"
        "/// carve-out (the `Return` arm of `terminator_uses_root` + the\n"
        "/// `terminator_branch_args` membership). A future terminator transfer fact\n"
        "/// is a [[terminator]] row edit, never a drop-pass edit.\n"
        "#[inline]\n"
        "pub fn terminator_operand_is_transferred(\n"
        "    kind: TerminatorKind,\n"
        "    category: OperandCategory,\n"
        ") -> bool {\n"
        "    matches!(\n"
        "        terminator_operand_ownership_table(kind, category),\n"
        "        OperandOwnership::Transferred\n"
        "    )\n"
        "}\n"
    )
    return "".join(out)


def _borrows_source_operand_index(spec: object) -> int | None:
    """The operand index this op's result interior-borrows (design 27 §1.5), or
    ``None``. The first position whose `operand_ownership` leaf is
    ``interior_borrow_keepalive``. A uniform spec (``all_borrowed`` /
    ``all_consumed``) interior-borrows nothing — only the per-position list form
    can carry the keepalive leaf (the validator forbids it as a uniform shorthand,
    so a borrow-of op MUST spell out its operand positions)."""
    if not isinstance(spec, list):
        return None
    for i, leaf in enumerate(spec):
        if leaf == "interior_borrow_keepalive":
            return i
    return None


def _container_absorb_operand_index(spec: object) -> int | None:
    """The operand index retained by an existing container/store mutation, or
    ``None``. Like interior borrows, this is per-position only: a uniform opcode
    cannot name one absorbed value operand without also identifying container/key
    operands."""
    if not isinstance(spec, list):
        return None
    for i, leaf in enumerate(spec):
        if leaf == "container_absorb":
            return i
    return None


def _operand_ownership_arm(spec: object) -> str:
    """Render the RHS of one `opcode_operand_ownership_table` match arm.

    A uniform spec collapses to a constant variant; a per-position list renders a
    nested `match operand_idx` whose final listed position also serves every
    higher index (the variadic-tail rule), keeping the function total."""
    if spec == "all_borrowed":
        return "OperandOwnership::Borrowed"
    if spec == "all_consumed":
        return "OperandOwnership::Consumed"
    assert isinstance(spec, list)
    leaves = [_OPERAND_OWNERSHIP_VARIANT[x] for x in spec]
    if len(set(leaves)) == 1:
        # A homogeneous list is just the uniform case (e.g. ["borrowed"]).
        return leaves[0]
    arms = []
    for i, leaf in enumerate(leaves[:-1]):
        arms.append(f"{i} => {leaf}")
    # The final listed position is the catch-all (covers its index AND any
    # higher variadic-tail index).
    arms.append(f"_ => {leaves[-1]}")
    return "match operand_idx { " + ", ".join(arms) + " }"


# ---------------------------------------------------------------------------
# Python rendering (frontend canonical spellings)
# ---------------------------------------------------------------------------

_PY_HEADER = """\
# @generated by tools/gen_op_kinds.py from
# runtime/molt-tir/src/tir/op_kinds.toml. DO NOT EDIT.
#
# The canonical JSON wire "kind" spellings the frontend emitter (map_ops_to_json
# in serialization.py) must use, so the producer and the backend `kind_to_opcode`
# mapper share ONE spelling. Sourced from op_kinds.toml (the cross-component
# single source of truth, docs/design/foundation/25_op_kind_registry.md).
#
# `CANONICAL_KIND` maps every alias spelling to its canonical wire kind; the
# emitter routes its spelling through it so a `floordiv`/`floor_div`-style schism
# can never re-open. `tests/test_gen_op_kinds.py` pins this file in sync.
#
# This file ALSO carries the frontend's four pre-serialization `op.kind` tables
# (molt task #44, F2a), absorbed from the hand-kept structures that previously
# lived in src/molt/frontend/__init__.py:
#   RAISING_KIND_NAMES         — op.kinds that can raise (emit() attaches the
#                                caret col_offset), from [[frontend_raising_kind]].
#   CHECK_EXCEPTION_SKIP_KINDS — op.kinds after which emit() skips the auto
#                                CHECK_EXCEPTION, from [[frontend_check_exception_skip]].
#   BINOP_OP_KIND / AUGASSIGN_OP_KIND — ast.operator subclass __name__ -> the
#                                binary / augmented-assignment op.kind, from
#                                [[binary_op]] (EXHAUSTIVE over ast.operator).

from __future__ import annotations

"""


def render_py(data: dict) -> str:
    kinds = data.get("kind", [])
    out: list[str] = [_PY_HEADER]

    # The canonical-spelling map: every spelling (canonical or alias) -> canonical.
    out.append("CANONICAL_KIND: dict[str, str] = {\n")
    for row in kinds:
        canon = row["canonical"]
        for spelling in [canon, *row.get("aliases", [])]:
            out.append(f'    "{spelling}": "{canon}",\n')
    out.append("}\n\n")

    # The set of canonical wire kinds (the emitter's allowed output vocabulary
    # for kinds that have a first-class mapper opcode).
    mapper_canon = [r["canonical"] for r in kinds if r.get("mapper_opcode") is not None]
    out.append("MAPPER_CANONICAL_KINDS: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for canon in mapper_canon:
        out.append(f'        "{canon}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    # -- frontend op.kind tables (F2a) --------------------------------------
    raising = data.get("frontend_raising_kind", [])
    out.append(
        "# Frontend `op.kind`s that can raise at runtime — emit() attaches the\n"
    )
    out.append(
        "# expression-level col_offset for traceback caret annotations. Each row\n"
    )
    out.append(
        "# is either an opcode-mapped may_throw kind (cross-checked against the\n"
    )
    out.append(
        "# [[opcode]] oracle at generation) or a documented frontend-specific kind.\n"
    )
    out.append("RAISING_KIND_NAMES: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for row in raising:
        out.append(f'        "{row["kind"]}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    skip = data.get("frontend_check_exception_skip", [])
    out.append("# Frontend `op.kind`s after which emit() does NOT auto-insert a\n")
    out.append(
        "# CHECK_EXCEPTION (control-flow / structural kinds, plus the two may_throw\n"
    )
    out.append("# kinds whose exceptional edge is handled structurally — RAISE,\n")
    out.append(
        "# STATE_TRANSITION). NOT the complement of may_throw; see op_kinds.toml.\n"
    )
    out.append("CHECK_EXCEPTION_SKIP_KINDS: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for row in skip:
        out.append(f'        "{row["kind"]}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    binary = data.get("binary_op", [])
    out.append(
        "# `ast.operator` subclass __name__ -> the binary-form frontend op.kind\n"
    )
    out.append(
        "# (visit_BinOp). EXHAUSTIVE over ast.operator (generation-time checked).\n"
    )
    out.append("BINOP_OP_KIND: dict[str, str] = {\n")
    for row in binary:
        out.append(f'    "{row["ast_op"]}": "{row["binop_kind"]}",\n')
    out.append("}\n\n")

    out.append(
        "# `ast.operator` subclass __name__ -> the augmented-assignment op.kind\n"
    )
    out.append(
        "# (visit_AugAssign). The in-place kind routes through the in-place dunder\n"
    )
    out.append(
        "# (__iadd__/__ifloordiv__/...) before the binary fallback, matching CPython.\n"
    )
    out.append("AUGASSIGN_OP_KIND: dict[str, str] = {\n")
    for row in binary:
        out.append(f'    "{row["ast_op"]}": "{row["augassign_kind"]}",\n')
    out.append("}\n\n\n")

    out.append("def canonical_kind(kind: str) -> str:\n")
    out.append('    """Return the canonical wire spelling for *kind*.\n\n')
    out.append(
        "    Identity for any kind not in the registry (the overwhelming common\n"
    )
    out.append(
        "    case: the kind is already canonical). The registry only records the\n"
    )
    out.append('    alias collapses that exist today (e.g. the floordiv family)."""\n')
    out.append("    return CANONICAL_KIND.get(kind, kind)\n")

    return "".join(out)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def _check(path: Path, rendered: str) -> bool:
    """Return True if *path* is in sync with *rendered* (prints a diff hint)."""
    if not path.exists():
        print(f"MISSING generated file: {path}", file=sys.stderr)
        return False
    current = path.read_bytes()
    expected = rendered.encode("utf-8")
    if current != expected:
        print(
            f"STALE generated file: {path}\n"
            f"  run `python3 tools/gen_op_kinds.py` to regenerate from "
            f"{TABLE.relative_to(ROOT)}",
            file=sys.stderr,
        )
        return False
    return True


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if a generated file is stale (CI mode); do not write",
    )
    args = ap.parse_args(argv)

    data = load_table()
    rs = render_rs(data)
    py = render_py(data)

    if args.check:
        ok = _check(OUT_RS, rs)
        ok = _check(OUT_PY, py) and ok
        if ok:
            print("op-kind generated files: in sync")
        return 0 if ok else 1

    OUT_RS.write_text(rs, encoding="utf-8", newline="\n")
    OUT_PY.write_text(py, encoding="utf-8", newline="\n")
    print(f"wrote {OUT_RS.relative_to(ROOT)}")
    print(f"wrote {OUT_PY.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
