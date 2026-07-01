from __future__ import annotations

import ast
import re
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover - fallback for <3.11
    import tomli as tomllib  # type: ignore[no-redef]

from .paths import TABLE
from .schema import *  # noqa: F403


class OpKindTableError(RuntimeError):
    pass


def load_table(table_path: Path = TABLE) -> dict:
    """Load and structurally validate ``op_kinds.toml``.

    Validation is fail-loud: a malformed/ambiguous table must never render a
    silently-degraded generated file.
    """
    if not table_path.exists():
        raise OpKindTableError(f"op-kind table missing: {table_path}")
    data = tomllib.loads(table_path.read_text(encoding="utf-8"))

    opcodes = data.get("opcode", [])
    if not opcodes:
        raise OpKindTableError("table has no [[opcode]] rows")
    seen_opcodes: set[str] = set()
    opcodes_by_name: dict[str, dict] = {}
    for row in opcodes:
        name = row.get("name")
        if not isinstance(name, str) or not name:
            raise OpKindTableError(f"[[opcode]] row missing 'name': {row}")
        if name in seen_opcodes:
            raise OpKindTableError(f"duplicate [[opcode]] name: {name}")
        seen_opcodes.add(name)
        opcodes_by_name[name] = row
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
        result_type = row.get("operand_independent_result_type")
        if result_type is not None:
            if result_type not in _OPERAND_INDEPENDENT_RESULT_TYPES:
                raise OpKindTableError(
                    f"opcode {name}: operand_independent_result_type must be one "
                    f"of {sorted(_OPERAND_INDEPENDENT_RESULT_TYPES)}, got "
                    f"{result_type!r}"
                )
            if result_arity != "one":
                raise OpKindTableError(
                    f"opcode {name}: operand_independent_result_type requires "
                    "result_arity = 'one'"
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

    _validate_simpleir_control_kinds(data)
    _validate_literal_payload_facts(data, seen_opcodes)
    _validate_fuzz_tir_opcode_shapes(data, opcodes_by_name)
    _validate_canonicalize_facts(data, seen_opcodes)
    for key in _OPCODE_FACT_SETS:
        _validate_opcode_fact_set(data, key, seen_opcodes)
    _validate_gvn_value_keyed_constant_facts(data, opcodes_by_name)
    _validate_gvn_numberable_attr_key_facts(data, opcodes_by_name)
    _validate_opcode_rule_rows(
        data,
        "type_refine_attr_result_type_rules",
        seen_opcodes,
        _TYPE_REFINE_ATTR_RESULT_TYPE_RULES,
        "type-refine attr result-type rule",
    )
    _validate_opcode_rule_rows(
        data,
        "type_refine_operand_type_rules",
        seen_opcodes,
        _TYPE_REFINE_OPERAND_TYPE_RULES,
        "type-refine operand type rule",
    )
    _validate_opcode_rule_rows(
        data,
        "sccp_constant_seed_rules",
        seen_opcodes,
        _SCCP_CONSTANT_SEED_RULES,
        "SCCP constant seed rule",
    )
    _validate_opcode_rule_rows(
        data,
        "sccp_constant_eval_rules",
        seen_opcodes,
        _SCCP_CONSTANT_EVAL_RULES,
        "SCCP constant eval rule",
    )
    _validate_opcode_rule_rows(
        data,
        "value_range_transfer_rules",
        seen_opcodes,
        _VALUE_RANGE_TRANSFER_RULES,
        "value-range transfer rule",
    )
    _validate_opcode_rule_rows(
        data,
        "value_range_const_fold_rules",
        seen_opcodes,
        _VALUE_RANGE_CONST_FOLD_RULES,
        "value-range const-fold rule",
    )
    _validate_opcode_rule_rows(
        data,
        "value_range_cond_narrow_rules",
        seen_opcodes,
        _VALUE_RANGE_COND_NARROW_RULES,
        "value-range conditional-narrow rule",
    )
    _validate_opcode_rule_rows(
        data,
        "value_range_container_length_rules",
        seen_opcodes,
        _VALUE_RANGE_CONTAINER_LENGTH_RULES,
        "value-range container-length rule",
    )
    _validate_range_devirt_roles(data, seen_opcodes)
    _validate_generator_fusion_iter_use_roles(data, seen_opcodes)
    _validate_vectorize_opcode_facts(data, seen_opcodes)
    _validate_opcode_rule_rows(
        data,
        "lir_verify_rules",
        seen_opcodes,
        _LIR_VERIFY_RULES,
        "LIR verifier rule",
    )
    _validate_opcode_rule_rows(
        data,
        "repr_raw_i64_full_deopt_seed_rules",
        seen_opcodes,
        _REPR_RAW_I64_FULL_DEOPT_SEED_RULES,
        "raw-i64 full-deopt seed rule",
    )
    _validate_opcode_rule_rows(
        data,
        "repr_projectable_bool_result_rules",
        seen_opcodes,
        _REPR_PROJECTABLE_BOOL_RESULT_RULES,
        "projectable bool result rule",
    )
    _validate_opcode_rule_rows(
        data,
        "repr_projectable_float_result_rules",
        seen_opcodes,
        _REPR_PROJECTABLE_FLOAT_RESULT_RULES,
        "projectable float result rule",
    )
    _validate_counted_loop_comparison_roles(data, seen_opcodes)
    _validate_module_concurrency_marker_source_roles(data, seen_opcodes)
    _validate_module_slot_access_roles(data, seen_opcodes)
    _validate_opcode_rule_rows(
        data,
        "tir_verify_attr_rules",
        seen_opcodes,
        _TIR_VERIFY_ATTR_RULES,
        "TIR verifier attr rule",
    )
    _validate_opcode_rule_rows(
        data,
        "sroa_const_immediate_rules",
        seen_opcodes,
        _SROA_CONST_IMMEDIATE_RULES,
        "SROA const-immediate rule",
    )
    _validate_opcode_rule_rows(
        data,
        "strength_reduction_rules",
        seen_opcodes,
        _STRENGTH_REDUCTION_RULES,
        "strength-reduction rule",
    )
    _validate_opcode_rule_rows(
        data,
        "scev_expr_rules",
        seen_opcodes,
        _SCEV_EXPR_RULES,
        "SCEV expression rule",
    )
    _validate_exception_region_nesting_roles(data, seen_opcodes)
    _validate_call_opcode_roles(data, seen_opcodes)
    _validate_pass_delta_opcode_facts(data)
    _validate_disjoint_opcode_role_sets(
        data, _ALIAS_TYPED_SLOT_ROLE_SETS, "alias typed-slot role"
    )
    _validate_disjoint_opcode_role_sets(
        data, _ALIAS_TRANSPARENT_ALIAS_ROLE_SETS, "alias transparent-alias role"
    )
    _validate_disjoint_opcode_role_sets(
        data, _REFCOUNT_BALANCE_ROLE_SETS, "refcount balance role"
    )
    _validate_disjoint_opcode_role_sets(
        data, _GENERATOR_FUSION_POLL_ROLE_SETS, "generator-fusion poll role"
    )
    _validate_disjoint_opcode_role_sets(
        data, _GVN_NUMBERING_ROLE_SETS, "GVN numbering role"
    )
    _validate_alias_memory_region_sets(data)
    _validate_alias_slot_observation_sets(data)

    kinds = data.get("kind", [])
    # Every mapper spelling (canonical or alias) must be globally unique within
    # the mapper — a kind string maps to exactly one OpCode; two rows owning it
    # is the exact drift this registry kills.
    owner: dict[str, str] = {}
    mapper_opcode_by_spelling: dict[str, str] = {}
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
            mapper_opcode_by_spelling[spelling] = mapper

    _validate_call_graph_user_call_kinds(data, mapper_opcode_by_spelling)
    _validate_ssa_attr_transport(data, seen_opcodes, mapper_opcode_by_spelling)
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


def _validate_fuzz_tir_opcode_shapes(data: dict, opcodes: dict[str, dict]) -> None:
    rows = data.get("fuzz_tir_opcode_shapes", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError(
            "fuzz_tir_opcode_shapes must be a non-empty array of tables"
        )
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError("fuzz_tir_opcode_shapes rows must be inline tables")
        unknown = set(row) - {"opcode", "operands", "attr_payload"}
        if unknown:
            raise OpKindTableError(
                f"fuzz_tir_opcode_shapes row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"fuzz_tir_opcode_shapes row missing opcode: {row}")
        opcode_row = opcodes.get(opcode)
        if opcode_row is None:
            raise OpKindTableError(
                f"fuzz_tir_opcode_shapes opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(f"duplicate fuzz_tir_opcode_shapes opcode: {opcode}")
        seen.add(opcode)
        operands = row.get("operands")
        if type(operands) is not int or operands < 0 or operands > 2:
            raise OpKindTableError(
                f"fuzz_tir_opcode_shapes {opcode}: operands must be an integer "
                f"in 0..=2, got {operands!r}"
            )
        if opcode_row.get("result_arity") not in {"zero", "one"}:
            raise OpKindTableError(
                f"fuzz_tir_opcode_shapes {opcode}: fuzz generator supports only "
                "fixed zero/one-result opcodes"
            )
        attr_payload = row.get("attr_payload", "none")
        if attr_payload not in _FUZZ_TIR_ATTR_PAYLOAD_RULES:
            raise OpKindTableError(
                f"fuzz_tir_opcode_shapes {opcode}: attr_payload must be one of "
                f"{sorted(_FUZZ_TIR_ATTR_PAYLOAD_RULES)}, got {attr_payload!r}"
            )
        expected_attr_payload = _FUZZ_TIR_OPCODE_ATTR_PAYLOAD_RULES.get(opcode, "none")
        if attr_payload != expected_attr_payload:
            raise OpKindTableError(
                f"fuzz_tir_opcode_shapes {opcode}: attr_payload must be "
                f"{expected_attr_payload!r}, got {attr_payload!r}"
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


def _opcode_role_members(data: dict, key: str) -> list[str]:
    if key == "gvn_value_keyed_constant_opcodes":
        return [row["opcode"] for row in data.get(key, [])]
    return list(data.get(key, []))


def _validate_disjoint_opcode_role_sets(
    data: dict, role_sets: tuple[str, ...], label: str
) -> None:
    owners: dict[str, str] = {}
    for key in role_sets:
        for opcode in _opcode_role_members(data, key):
            if opcode in owners:
                raise OpKindTableError(
                    f"{label} opcode {opcode!r} appears in both "
                    f"{owners[opcode]} and {key}"
                )
            owners[opcode] = key


def _validate_gvn_value_keyed_constant_facts(
    data: dict, opcodes: dict[str, dict]
) -> None:
    rows = data.get("gvn_value_keyed_constant_opcodes", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError(
            "gvn_value_keyed_constant_opcodes must be a non-empty array of tables"
        )
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "gvn_value_keyed_constant_opcodes rows must be inline tables"
            )
        unknown = set(row) - {"opcode", "key", "attrs"}
        if unknown:
            raise OpKindTableError(
                "gvn_value_keyed_constant_opcodes row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"gvn_value_keyed_constant_opcodes row missing opcode: {row}"
            )
        opcode_row = opcodes.get(opcode)
        if opcode_row is None:
            raise OpKindTableError(
                f"gvn_value_keyed_constant_opcodes opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate gvn_value_keyed_constant_opcodes opcode: {opcode}"
            )
        seen.add(opcode)
        if (
            opcode_row.get("purity") != "pure"
            or opcode_row.get("result_arity") != "one"
        ):
            raise OpKindTableError(
                f"gvn_value_keyed_constant_opcodes {opcode}: value-keyed constants "
                "must be pure single-result opcodes"
            )
        key = row.get("key")
        if key not in _GVN_VALUE_KEY_KINDS:
            raise OpKindTableError(
                f"gvn_value_keyed_constant_opcodes {opcode}: key must be one of "
                f"{sorted(_GVN_VALUE_KEY_KINDS)}"
            )
        attrs = row.get("attrs", [])
        if key == "none_singleton":
            if attrs not in ([], None):
                raise OpKindTableError(
                    f"gvn_value_keyed_constant_opcodes {opcode}: none_singleton "
                    "must not declare attrs"
                )
            continue
        if not isinstance(attrs, list) or not attrs:
            raise OpKindTableError(
                f"gvn_value_keyed_constant_opcodes {opcode}: key {key!r} "
                "requires a non-empty attrs list"
            )
        if not all(
            isinstance(attr, str) and re.fullmatch(r"[_a-z][a-z0-9_]*", attr)
            for attr in attrs
        ):
            raise OpKindTableError(
                f"gvn_value_keyed_constant_opcodes {opcode}: attrs must be "
                "attribute-name strings"
            )
        if len(set(attrs)) != len(attrs):
            raise OpKindTableError(
                f"gvn_value_keyed_constant_opcodes {opcode}: duplicate attrs"
            )


def _validate_gvn_numberable_attr_key_facts(
    data: dict, opcodes: dict[str, dict]
) -> None:
    rows = data.get("gvn_numberable_attr_key_opcodes", [])
    if not isinstance(rows, list):
        raise OpKindTableError(
            "gvn_numberable_attr_key_opcodes must be an array of tables"
        )
    numberable = set(data.get("gvn_always_numberable_opcodes", [])) | set(
        data.get("gvn_type_gated_numberable_opcodes", [])
    )
    constant_keyed = {
        row["opcode"]
        for row in data.get("gvn_value_keyed_constant_opcodes", [])
        if isinstance(row, dict) and isinstance(row.get("opcode"), str)
    }
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "gvn_numberable_attr_key_opcodes rows must be inline tables"
            )
        unknown = set(row) - {"opcode", "key", "attrs"}
        if unknown:
            raise OpKindTableError(
                "gvn_numberable_attr_key_opcodes row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes row missing opcode: {row}"
            )
        opcode_row = opcodes.get(opcode)
        if opcode_row is None:
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate gvn_numberable_attr_key_opcodes opcode: {opcode}"
            )
        seen.add(opcode)
        if opcode in constant_keyed:
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes {opcode}: constants must use "
                "gvn_value_keyed_constant_opcodes"
            )
        if opcode not in numberable:
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes {opcode}: opcode must be in "
                "gvn_always_numberable_opcodes or gvn_type_gated_numberable_opcodes"
            )
        if opcode_row.get("result_arity") != "one":
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes {opcode}: opcode must be single-result"
            )
        key = row.get("key")
        if key not in _GVN_VALUE_KEY_KINDS or key == "none_singleton":
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes {opcode}: key must be one of "
                f"{sorted(k for k in _GVN_VALUE_KEY_KINDS if k != 'none_singleton')}"
            )
        attrs = row.get("attrs")
        if not isinstance(attrs, list) or not attrs:
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes {opcode}: key {key!r} "
                "requires a non-empty attrs list"
            )
        if not all(
            isinstance(attr, str) and re.fullmatch(r"[_a-z][a-z0-9_]*", attr)
            for attr in attrs
        ):
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes {opcode}: attrs must be "
                "attribute-name strings"
            )
        if len(set(attrs)) != len(attrs):
            raise OpKindTableError(
                f"gvn_numberable_attr_key_opcodes {opcode}: duplicate attrs"
            )


def _validate_opcode_rule_rows(
    data: dict,
    key: str,
    opcodes: set[str],
    allowed_rules: dict[str, str],
    label: str,
) -> None:
    rows = data.get(key, [])
    if not isinstance(rows, list):
        raise OpKindTableError(f"{key} must be an array of tables")
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(f"{key} rows must be inline tables")
        unknown = set(row) - {"opcode", "rule"}
        if unknown:
            raise OpKindTableError(
                f"{key} row has unknown fields {sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"{key} row missing opcode: {row}")
        if opcode not in opcodes:
            raise OpKindTableError(f"{key} opcode {opcode!r} is not a known OpCode")
        if opcode in seen:
            raise OpKindTableError(f"duplicate {key} opcode: {opcode}")
        seen.add(opcode)
        rule = row.get("rule")
        if rule not in allowed_rules:
            raise OpKindTableError(
                f"{key} {opcode}: {label} must be one of {sorted(allowed_rules)}"
            )


def _validate_counted_loop_comparison_roles(data: dict, opcodes: set[str]) -> None:
    rows = data.get("counted_loop_comparison_roles", [])
    if not isinstance(rows, list):
        raise OpKindTableError(
            "counted_loop_comparison_roles must be an array of tables"
        )
    seen: set[str] = set()
    inverse_by_opcode: dict[str, str] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "counted_loop_comparison_roles rows must be inline tables"
            )
        unknown = set(row) - {"opcode", "role", "inverse"}
        if unknown:
            raise OpKindTableError(
                "counted_loop_comparison_roles row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"counted_loop_comparison_roles row missing opcode: {row}"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                f"counted_loop_comparison_roles opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate counted_loop_comparison_roles opcode: {opcode}"
            )
        seen.add(opcode)
        role = row.get("role")
        if role not in _COUNTED_LOOP_COMPARISON_ROLES:
            raise OpKindTableError(
                f"counted_loop_comparison_roles {opcode}: role must be one of "
                f"{sorted(_COUNTED_LOOP_COMPARISON_ROLES)}"
            )
        inverse = row.get("inverse")
        if not isinstance(inverse, str) or inverse not in opcodes:
            raise OpKindTableError(
                f"counted_loop_comparison_roles {opcode}: inverse must name an OpCode"
            )
        if inverse == opcode:
            raise OpKindTableError(
                f"counted_loop_comparison_roles {opcode}: inverse must differ"
            )
        inverse_by_opcode[opcode] = inverse
    for opcode, inverse in inverse_by_opcode.items():
        if inverse_by_opcode.get(inverse) != opcode:
            raise OpKindTableError(
                "counted_loop_comparison_roles inverses must be symmetric: "
                f"{opcode}->{inverse}, but {inverse}->{inverse_by_opcode.get(inverse)}"
            )


def _validate_module_concurrency_marker_source_roles(
    data: dict, opcodes: set[str]
) -> None:
    rows = data.get("module_concurrency_marker_source_roles", [])
    if not isinstance(rows, list):
        raise OpKindTableError(
            "module_concurrency_marker_source_roles must be an array of tables"
        )
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "module_concurrency_marker_source_roles rows must be inline tables"
            )
        unknown = set(row) - {"opcode", "role", "attrs"}
        if unknown:
            raise OpKindTableError(
                "module_concurrency_marker_source_roles row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"module_concurrency_marker_source_roles row missing opcode: {row}"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                "module_concurrency_marker_source_roles opcode "
                f"{opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate module_concurrency_marker_source_roles opcode: {opcode}"
            )
        seen.add(opcode)
        role = row.get("role")
        if role not in _MODULE_CONCURRENCY_MARKER_SOURCE_ROLES or role == "none":
            raise OpKindTableError(
                f"module_concurrency_marker_source_roles {opcode}: role must be one of "
                f"{sorted(k for k in _MODULE_CONCURRENCY_MARKER_SOURCE_ROLES if k != 'none')}"
            )
        attrs = row.get("attrs")
        if not isinstance(attrs, list) or not attrs:
            raise OpKindTableError(
                f"module_concurrency_marker_source_roles {opcode}: attrs must be "
                "a non-empty list"
            )
        if not all(
            isinstance(attr, str) and re.fullmatch(r"[_a-z][a-z0-9_]*", attr)
            for attr in attrs
        ):
            raise OpKindTableError(
                f"module_concurrency_marker_source_roles {opcode}: attrs must be "
                "attribute-name strings"
            )
        if len(set(attrs)) != len(attrs):
            raise OpKindTableError(
                f"module_concurrency_marker_source_roles {opcode}: duplicate attrs"
            )


def _validate_module_slot_access_roles(data: dict, opcodes: set[str]) -> None:
    rows = data.get("module_slot_access_roles", [])
    if not isinstance(rows, list):
        raise OpKindTableError("module_slot_access_roles must be an array of tables")
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "module_slot_access_roles rows must be inline tables"
            )
        unknown = set(row) - {"opcode", "role"}
        if unknown:
            raise OpKindTableError(
                "module_slot_access_roles row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"module_slot_access_roles row missing opcode: {row}"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                f"module_slot_access_roles opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate module_slot_access_roles opcode: {opcode}"
            )
        seen.add(opcode)
        role = row.get("role")
        if role not in _MODULE_SLOT_ACCESS_ROLES or role == "none":
            raise OpKindTableError(
                f"module_slot_access_roles {opcode}: role must be one of "
                f"{sorted(k for k in _MODULE_SLOT_ACCESS_ROLES if k != 'none')}"
            )


def _validate_vectorize_opcode_facts(data: dict, opcodes: set[str]) -> None:
    rows = data.get("vectorize_opcode_facts", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError(
            "vectorize_opcode_facts must be a non-empty array of tables"
        )
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError("vectorize_opcode_facts rows must be inline tables")
        unknown = set(row) - {
            "opcode",
            "body",
            "reduction",
            "annotation_target",
        }
        if unknown:
            raise OpKindTableError(
                f"vectorize_opcode_facts row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"vectorize_opcode_facts row missing opcode: {row}")
        if opcode not in opcodes:
            raise OpKindTableError(
                f"vectorize_opcode_facts opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(f"duplicate vectorize_opcode_facts opcode: {opcode}")
        seen.add(opcode)

        body = row.get("body", "reject")
        if body not in _VECTORIZE_BODY_ACTIONS or body == "reject":
            allowed = sorted(k for k in _VECTORIZE_BODY_ACTIONS if k != "reject")
            raise OpKindTableError(
                f"vectorize_opcode_facts {opcode}: body must be one of "
                f"{allowed}, got {body!r}"
            )
        reduction = row.get("reduction")
        if reduction is not None and reduction not in _VECTOR_REDUCTION_RULES:
            raise OpKindTableError(
                f"vectorize_opcode_facts {opcode}: reduction must be one of "
                f"{sorted(_VECTOR_REDUCTION_RULES)}, got {reduction!r}"
            )
        if not isinstance(row.get("annotation_target", False), bool):
            raise OpKindTableError(
                f"vectorize_opcode_facts {opcode}: annotation_target must be bool"
            )
        if reduction is not None and body != "scalar_arithmetic":
            raise OpKindTableError(
                f"vectorize_opcode_facts {opcode}: reduction requires "
                "body='scalar_arithmetic'"
            )
        if row.get("annotation_target", False) and body != "iteration_control":
            raise OpKindTableError(
                f"vectorize_opcode_facts {opcode}: annotation_target requires "
                "body='iteration_control'"
            )


def _validate_call_opcode_roles(data: dict, opcodes: set[str]) -> None:
    rows = data.get("call_opcode_roles", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError("call_opcode_roles must be a non-empty array of tables")
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError("call_opcode_roles rows must be inline tables")
        unknown = set(row) - {"opcode", "role"}
        if unknown:
            raise OpKindTableError(
                f"call_opcode_roles row has unknown fields {sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"call_opcode_roles row missing opcode: {row}")
        if opcode not in opcodes:
            raise OpKindTableError(
                f"call_opcode_roles opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(f"duplicate call_opcode_roles opcode: {opcode}")
        seen.add(opcode)
        role = row.get("role")
        if role not in _CALL_OPCODE_ROLES or role == "not_call":
            allowed = sorted(k for k in _CALL_OPCODE_ROLES if k != "not_call")
            raise OpKindTableError(
                f"call_opcode_roles {opcode}: role must be one of {allowed}, "
                f"got {role!r}"
            )
        if role == "copy_original_kind" and opcode != "Copy":
            raise OpKindTableError(
                "call_opcode_roles copy_original_kind is reserved for OpCode::Copy"
            )


def _validate_ssa_attr_transport(
    data: dict,
    opcodes: set[str],
    mapper_opcode_by_spelling: dict[str, str],
) -> None:
    rows = data.get("ssa_s_value_attr_keys", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError("ssa_s_value_attr_keys must be a non-empty array")
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError("ssa_s_value_attr_keys rows must be inline tables")
        unknown = set(row) - {"opcode", "attr"}
        if unknown:
            raise OpKindTableError(
                f"ssa_s_value_attr_keys row has unknown fields {sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"ssa_s_value_attr_keys row missing opcode: {row}")
        if opcode not in opcodes:
            raise OpKindTableError(
                f"ssa_s_value_attr_keys opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(f"duplicate ssa_s_value_attr_keys opcode: {opcode}")
        seen.add(opcode)
        attr = row.get("attr")
        if attr not in _SSA_S_VALUE_ATTR_KEYS:
            raise OpKindTableError(
                f"ssa_s_value_attr_keys {opcode}: attr must be one of "
                f"{sorted(_SSA_S_VALUE_ATTR_KEYS)}, got {attr!r}"
            )

    preserve = data.get("ssa_original_kind_preserving_kinds", [])
    if not isinstance(preserve, list) or not preserve:
        raise OpKindTableError(
            "ssa_original_kind_preserving_kinds must be a non-empty list"
        )
    if not all(isinstance(kind, str) and kind for kind in preserve):
        raise OpKindTableError(
            "ssa_original_kind_preserving_kinds must contain non-empty strings"
        )
    if len(set(preserve)) != len(preserve):
        raise OpKindTableError("ssa_original_kind_preserving_kinds has duplicates")
    valid_opcodes = {
        "Copy",
        "Call",
        "CallBuiltin",
        "LoadAttr",
        "StoreAttr",
        "DelAttr",
        "Index",
        "StoreIndex",
        "DelIndex",
    }
    for kind in preserve:
        opcode = mapper_opcode_by_spelling.get(kind)
        if opcode is None:
            raise OpKindTableError(
                f"ssa_original_kind_preserving_kinds kind {kind!r} is not a "
                "known mapper spelling"
            )
        if opcode not in valid_opcodes:
            raise OpKindTableError(
                f"ssa_original_kind_preserving_kinds {kind!r} maps to "
                f"OpCode::{opcode}, which is not an SSA original-kind transport opcode"
            )
        if opcode == "Copy" and kind != "store_var":
            raise OpKindTableError(
                "ssa_original_kind_preserving_kinds only store_var may preserve "
                "for OpCode::Copy"
            )
    for forbidden in ("copy", "load_var", "copy_var"):
        if forbidden in preserve:
            raise OpKindTableError(
                f"ssa_original_kind_preserving_kinds must not include {forbidden!r}"
            )


def _validate_range_devirt_roles(data: dict, opcodes: set[str]) -> None:
    rows = data.get("range_devirt_roles", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError("range_devirt_roles must be a non-empty array of tables")
    seen: set[str] = set()
    expected_opcode_by_role = {
        "range_call_candidate": "CallBuiltin",
        "iterator_candidate": "GetIter",
        "next_unboxed_candidate": "IterNextUnboxed",
    }
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError("range_devirt_roles rows must be inline tables")
        unknown = set(row) - {"opcode", "role"}
        if unknown:
            raise OpKindTableError(
                f"range_devirt_roles row has unknown fields {sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(f"range_devirt_roles row missing opcode: {row}")
        if opcode not in opcodes:
            raise OpKindTableError(
                f"range_devirt_roles opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(f"duplicate range_devirt_roles opcode: {opcode}")
        seen.add(opcode)
        role = row.get("role")
        if role not in _RANGE_DEVIRT_ROLES or role == "none":
            allowed = sorted(k for k in _RANGE_DEVIRT_ROLES if k != "none")
            raise OpKindTableError(
                f"range_devirt_roles {opcode}: role must be one of {allowed}, "
                f"got {role!r}"
            )
        expected_opcode = expected_opcode_by_role[role]
        if opcode != expected_opcode:
            raise OpKindTableError(
                f"range_devirt_roles {opcode}: role {role!r} is reserved for "
                f"OpCode::{expected_opcode}"
            )


def _validate_generator_fusion_iter_use_roles(data: dict, opcodes: set[str]) -> None:
    rows = data.get("generator_fusion_iter_use_roles", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError(
            "generator_fusion_iter_use_roles must be a non-empty array of tables"
        )
    seen: set[str] = set()
    expected_opcode_by_role = {
        "next_use": "IterNext",
        "none_guard": "Is",
    }
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "generator_fusion_iter_use_roles rows must be inline tables"
            )
        unknown = set(row) - {"opcode", "role"}
        if unknown:
            raise OpKindTableError(
                "generator_fusion_iter_use_roles row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"generator_fusion_iter_use_roles row missing opcode: {row}"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                f"generator_fusion_iter_use_roles opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate generator_fusion_iter_use_roles opcode: {opcode}"
            )
        seen.add(opcode)
        role = row.get("role")
        if role not in _GENERATOR_FUSION_ITER_USE_ROLES or role == "none":
            allowed = sorted(k for k in _GENERATOR_FUSION_ITER_USE_ROLES if k != "none")
            raise OpKindTableError(
                f"generator_fusion_iter_use_roles {opcode}: role must be one of "
                f"{allowed}, got {role!r}"
            )
        expected_opcode = expected_opcode_by_role[role]
        if opcode != expected_opcode:
            raise OpKindTableError(
                f"generator_fusion_iter_use_roles {opcode}: role {role!r} is "
                f"reserved for OpCode::{expected_opcode}"
            )


def _validate_exception_region_nesting_roles(data: dict, opcodes: set[str]) -> None:
    rows = data.get("exception_region_nesting_roles", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError(
            "exception_region_nesting_roles must be a non-empty array of tables"
        )
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(
                "exception_region_nesting_roles rows must be inline tables"
            )
        unknown = set(row) - {"opcode", "role"}
        if unknown:
            raise OpKindTableError(
                f"exception_region_nesting_roles row has unknown fields "
                f"{sorted(unknown)}: {row}"
            )
        opcode = row.get("opcode")
        if not isinstance(opcode, str) or not opcode:
            raise OpKindTableError(
                f"exception_region_nesting_roles row missing opcode: {row}"
            )
        if opcode not in opcodes:
            raise OpKindTableError(
                f"exception_region_nesting_roles opcode {opcode!r} is not a known OpCode"
            )
        if opcode in seen:
            raise OpKindTableError(
                f"duplicate exception_region_nesting_roles opcode: {opcode}"
            )
        seen.add(opcode)
        role = row.get("role")
        if role not in _EXCEPTION_REGION_NESTING_ROLES or role == "none":
            allowed = sorted(k for k in _EXCEPTION_REGION_NESTING_ROLES if k != "none")
            raise OpKindTableError(
                f"exception_region_nesting_roles {opcode}: role must be one of "
                f"{allowed}, got {role!r}"
            )
        expected_opcode = {"enter": "TryStart", "exit": "TryEnd"}[role]
        if opcode != expected_opcode:
            raise OpKindTableError(
                f"exception_region_nesting_roles {opcode}: role {role!r} is "
                f"reserved for OpCode::{expected_opcode}"
            )


def _validate_call_graph_user_call_kinds(
    data: dict, mapper_opcode_by_spelling: dict[str, str]
) -> None:
    members = data.get("call_graph_user_call_kinds", [])
    if not isinstance(members, list) or not members:
        raise OpKindTableError(
            "call_graph_user_call_kinds must be a non-empty array of strings"
        )
    if not all(isinstance(kind, str) and kind for kind in members):
        raise OpKindTableError(
            "call_graph_user_call_kinds must contain only non-empty strings"
        )
    if len(set(members)) != len(members):
        raise OpKindTableError("call_graph_user_call_kinds has duplicate members")
    for kind in members:
        opcode = mapper_opcode_by_spelling.get(kind)
        if opcode is None:
            raise OpKindTableError(
                f"call_graph_user_call_kinds kind {kind!r} is not a known kind spelling"
            )
        if opcode not in {"Call", "CallMethod"}:
            raise OpKindTableError(
                f"call_graph_user_call_kinds {kind!r} maps to OpCode::{opcode}; "
                "user-call Copy fallbacks may only map to Call or CallMethod"
            )


def _validate_simpleir_control_kinds(data: dict) -> None:
    rows = data.get("simpleir_control_kind", [])
    if not isinstance(rows, list):
        raise OpKindTableError("simpleir_control_kind must be an array of tables")
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            raise OpKindTableError(f"simpleir_control_kind row must be a table: {row}")
        kind = row.get("kind")
        if not isinstance(kind, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", kind):
            raise OpKindTableError(
                f"simpleir_control_kind row has invalid kind spelling: {row}"
            )
        if kind in seen:
            raise OpKindTableError(f"duplicate simpleir_control_kind: {kind}")
        seen.add(kind)
        for field in _SIMPLEIR_CONTROL_FACT_FIELDS:
            if not isinstance(row.get(field), bool):
                raise OpKindTableError(
                    f"simpleir_control_kind {kind}: {field!r} must be a bool"
                )
        unknown = set(row) - {"kind", *_SIMPLEIR_CONTROL_FACT_FIELDS}
        if unknown:
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: unknown fields {sorted(unknown)}"
            )
        if row["ssa_only"] and any(
            row[field] for field in _SIMPLEIR_CONTROL_FACT_FIELDS if field != "ssa_only"
        ):
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: ssa_only cannot overlap runtime facts"
            )
        if row["repoll"] and not (row["suspend"] and row["block_leader"]):
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: repoll requires suspend and block_leader"
            )
        if row["suspend"] and not row["block_ender"]:
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: suspend requires block_ender"
            )
        if row["terminator"] and not row["structural"]:
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: terminator requires structural"
            )
        if row["wasm_dispatch_block_leader"] and not row["wasm_split_barrier"]:
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: wasm dispatch block leader requires wasm split barrier"
            )
        if row["wasm_dispatch_block_terminator"] and not row["wasm_split_barrier"]:
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: wasm dispatch block terminator requires wasm split barrier"
            )
        if row["wasm_stateful_dispatch"] and not row["wasm_split_barrier"]:
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: wasm stateful dispatch requires wasm split barrier"
            )
        if row["wasm_state_resume_after"] and not (
            row["wasm_stateful_dispatch"] and row["suspend"]
        ):
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: wasm resume-after requires suspend and stateful dispatch"
            )
        if row["wasm_state_resume_at"] and not row["wasm_dispatch_block_leader"]:
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: wasm resume-at requires dispatch block leader"
            )
        if not any(row[field] for field in _SIMPLEIR_CONTROL_FACT_FIELDS):
            raise OpKindTableError(
                f"simpleir_control_kind {kind}: at least one fact must be true"
            )


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


def _frontend_wire_spelling_to_op_kind(spelling: str) -> str:
    """Map a wire-kind spelling to the frontend's pre-serialization op.kind."""

    return spelling.upper()


def _frontend_effect_from_opcode(row: dict) -> str:
    """Return the frontend optimizer's memory-effect class for an OpCode row.

    This is the alias/CSE axis only. Raising capability is a separate DCE
    barrier rendered from [[frontend_raising_kind]] as RAISING_KIND_NAMES.
    """

    if row["side_effecting"]:
        return "writes_heap"
    if row["purity"] == "impure":
        return "reads_heap"
    return "pure"


def _frontend_raising_nothrow_on_primitives(data: dict) -> set[str]:
    """Return raising kinds whose raise is disproved by primitive constants."""

    out: set[str] = set()
    for row in data.get("frontend_raising_kind", []):
        if row.get("nothrow_on_primitives", False):
            out.add(row["kind"])
    return out


def _frontend_effect_class_map(data: dict) -> dict[str, str]:
    """Build the generated frontend pre-serialization effect oracle."""

    opcodes_by_name = {row["name"]: row for row in data.get("opcode", [])}
    effects: dict[str, str] = {}

    for row in data.get("kind", []):
        opcode_name = row.get("mapper_opcode")
        if not opcode_name:
            continue
        opcode = opcodes_by_name[opcode_name]
        effect = _frontend_effect_from_opcode(opcode)
        for spelling in [row["canonical"], *row.get("aliases", [])]:
            effects[_frontend_wire_spelling_to_op_kind(spelling)] = effect

    # [[frontend_raising_kind]] is the may-raise axis, not the memory axis.
    # Memory classes come from opcode facts above or explicit
    # [[frontend_effect_kind]] overrides below.

    for row in data.get("simpleir_control_kind", []):
        if any(
            row.get(flag, False)
            for flag in (
                "structural",
                "terminator",
                "suspend",
                "repoll",
                "block_leader",
                "block_ender",
                "conditional_branch",
            )
        ):
            effects[_frontend_wire_spelling_to_op_kind(row["kind"])] = "control"

    for row in data.get("frontend_check_exception_skip", []):
        kind = row["kind"]
        if row.get("control_flow", False) or kind.startswith(("EXCEPTION_", "STATE_")):
            effects[kind] = "control"

    for row in data.get("frontend_effect_kind", []):
        effects[row["kind"]] = row["effect"]

    return effects


def _validate_frontend_tables(data: dict, opcodes: list[dict]) -> None:
    """Structurally validate the frontend `op.kind` tables.

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
        if "nothrow_on_primitives" in row and not isinstance(
            row["nothrow_on_primitives"], bool
        ):
            raise OpKindTableError(
                f"frontend_raising_kind {kind}: 'nothrow_on_primitives' must be a bool"
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

    # -- [[frontend_effect_kind]] ------------------------------------------
    frontend_effect_rows = data.get("frontend_effect_kind", [])
    if not isinstance(frontend_effect_rows, list) or not frontend_effect_rows:
        raise OpKindTableError("table has no [[frontend_effect_kind]] rows")
    seen_effect: set[str] = set()
    for row in frontend_effect_rows:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[frontend_effect_kind]] row missing 'kind': {row}"
            )
        if kind in seen_effect:
            raise OpKindTableError(f"duplicate frontend_effect_kind: {kind}")
        seen_effect.add(kind)
        effect = row.get("effect")
        if effect not in _FRONTEND_EFFECT_VALUES:
            raise OpKindTableError(
                f"frontend_effect_kind {kind}: effect must be one of "
                f"{sorted(_FRONTEND_EFFECT_VALUES)}, got {effect!r}"
            )
        if not isinstance(row.get("reason"), str) or not row["reason"]:
            raise OpKindTableError(
                f"frontend_effect_kind {kind}: 'reason' must be a non-empty string"
            )

    effect_map = _frontend_effect_class_map(data)
    raising_kinds = {row["kind"] for row in raising}

    required_effects = {
        "ADD": "pure",
        "SUB": "pure",
        "MUL": "pure",
        "EQ": "pure",
        "NE": "pure",
        "LT": "pure",
        "LE": "pure",
        "GT": "pure",
        "GE": "pure",
        "NEG": "pure",
        "POS": "pure",
        "INVERT": "pure",
        "ABS": "pure",
        "CONST_STR": "pure",
        "INDEX": "reads_heap",
        "GET_ATTR": "reads_heap",
        "MODULE_GET_ATTR": "reads_heap",
        "GETATTR_GENERIC_OBJ": "reads_heap",
        "GUARDED_GETATTR": "reads_heap",
        "LOAD_VAR": "reads_heap",
        "STORE_VAR": "writes_heap",
        "SETATTR_GENERIC_OBJ": "writes_heap",
        "CHECK_EXCEPTION": "control",
        "STATE_TRANSITION": "control",
        "EXCEPTION_MATCH_BUILTIN": "reads_heap",
    }
    for kind, expected in required_effects.items():
        actual = effect_map.get(kind)
        if actual != expected:
            raise OpKindTableError(
                f"frontend memory-effect invariant {kind}: expected {expected}, "
                f"got {actual}"
            )

    required_raising = {
        "ADD": True,
        "EQ": True,
        "NEG": True,
        "ABS": True,
        "INVERT": True,
        "GET_ATTR": True,
        "INDEX": True,
        "MODULE_GET_ATTR": True,
        "SETATTR_GENERIC_OBJ": True,
        "PHI": False,
        "CONST_STR": False,
        "LOAD_VAR": False,
        "STORE_VAR": False,
    }
    for kind, should_raise in required_raising.items():
        actual = kind in raising_kinds
        if actual != should_raise:
            raise OpKindTableError(
                f"frontend raising-axis invariant {kind}: expected {should_raise}, "
                f"got {actual}"
            )

    nothrow_on_primitives = _frontend_raising_nothrow_on_primitives(data)
    for kind in ("ADD", "SUB", "MUL", "EQ", "NEG", "ABS", "INVERT"):
        if kind not in nothrow_on_primitives:
            raise OpKindTableError(
                f"frontend primitive-nothrow invariant {kind}: missing opt-in"
            )
    for kind in ("DIV", "FLOORDIV", "MOD", "POW", "LSHIFT", "RSHIFT", "IN", "NOT_IN"):
        if kind in nothrow_on_primitives:
            raise OpKindTableError(
                f"frontend primitive-nothrow invariant {kind}: unsafe opt-in"
            )


__all__ = [
    name
    for name in globals()
    if name in {"OpKindTableError", "load_table", "_opcode_role_members"}
    or name.startswith("_validate")
    or name.startswith("_frontend")
]
