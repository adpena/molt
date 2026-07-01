from __future__ import annotations

# Valid enum values and structural fact vocabularies for op_kinds.toml.
_PURITY_VALUES = {"pure", "pure_may_throw", "impure"}
_FRONTEND_EFFECT_VALUES = {"pure", "reads_heap", "writes_heap", "control"}
_RESULT_ARITY_VALUES = {"zero", "one", "two", "variable"}
_OPERAND_INDEPENDENT_RESULT_TYPES = {
    "i64",
    "f64",
    "bool",
    "str",
    "none",
    "bytes",
    "dynbox",
    "list_dynbox",
    "dict_dynbox_dynbox",
    "set_dynbox",
}
_VARIABLE_RESULT_ARITY_OPCODES = {
    # Calls may be emitted for value-producing expressions or result-discarding
    # statements. The return-value fact is present only when TIR carries a result.
    "Call",
    "CallMethod",
    "CallMethodIc",
    "CallSuperMethodIc",
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
_GVN_VALUE_KEY_KINDS = {
    "i64_attr": "I64Attr",
    "bool_attr": "BoolAttr",
    "none_singleton": "NoneSingleton",
    "f64_bits_attr": "F64BitsAttr",
    "str_attr": "StrAttr",
    "bytes_attr": "BytesAttr",
}
_TYPE_REFINE_ATTR_RESULT_TYPE_RULES = {
    "object_type_hint": "ObjectTypeHint",
    "call_return_type": "CallReturnType",
    "call_builtin_return_type": "CallBuiltinReturnType",
    "type_guard": "TypeGuard",
    "copy_original_kind": "CopyOriginalKind",
}
_TYPE_REFINE_OPERAND_TYPE_RULES = {
    "add": "Add",
    "mul": "Mul",
    "numeric_arithmetic": "NumericArithmetic",
    "true_division": "TrueDivision",
    "unary_numeric": "UnaryNumeric",
    "bool_select": "BoolSelect",
    "bitwise_i64": "BitwiseI64",
    "bit_not_i64": "BitNotI64",
    "build_tuple": "BuildTuple",
    "get_iter": "GetIter",
    "iter_next": "IterNext",
    "index": "Index",
    "copy": "Copy",
    "box_val": "BoxVal",
    "unbox_val": "UnboxVal",
}
_SCCP_CONSTANT_SEED_RULES = {
    "int_attr": "IntAttr",
    "float_attr": "FloatAttr",
    "bool_attr": "BoolAttr",
    "str_attr": "StrAttr",
    "none_singleton": "NoneSingleton",
}
_SCCP_CONSTANT_EVAL_RULES = {
    "add": "Add",
    "sub": "Sub",
    "mul": "Mul",
    "div": "Div",
    "floordiv": "FloorDiv",
    "mod": "Mod",
    "pow": "Pow",
    "eq": "Eq",
    "ne": "Ne",
    "lt": "Lt",
    "le": "Le",
    "gt": "Gt",
    "ge": "Ge",
    "neg": "Neg",
    "not": "Not",
    "build_list": "BuildList",
    "build_dict": "BuildDict",
    "build_tuple_as_list": "BuildTupleAsList",
}
_VALUE_RANGE_TRANSFER_RULES = {
    "add": "Add",
    "sub": "Sub",
    "mul": "Mul",
    "floordiv": "FloorDiv",
    "neg": "Neg",
    "bit_and": "BitAnd",
    "bit_or": "BitOr",
    "bit_xor": "BitXor",
    "mod": "Mod",
    "shr": "Shr",
    "shl": "Shl",
}
_VALUE_RANGE_CONST_FOLD_RULES = {
    "add": "Add",
    "sub": "Sub",
    "mul": "Mul",
    "shl": "Shl",
    "shr": "Shr",
    "bit_and": "BitAnd",
    "bit_or": "BitOr",
    "bit_xor": "BitXor",
}
_VALUE_RANGE_COND_NARROW_RULES = {
    "lt_upper_exclusive": "LtUpperExclusive",
    "le_upper_inclusive": "LeUpperInclusive",
}
_VALUE_RANGE_CONTAINER_LENGTH_RULES = {
    "fixed_literal": "FixedLiteral",
    "list_repeat": "ListRepeat",
    "len_call": "LenCall",
}
_RANGE_DEVIRT_ROLES = {
    "none": "None",
    "range_call_candidate": "RangeCallCandidate",
    "iterator_candidate": "IteratorCandidate",
    "next_unboxed_candidate": "NextUnboxedCandidate",
}
_VECTORIZE_BODY_ACTIONS = {
    "reject": "Reject",
    "scalar_arithmetic": "ScalarArithmetic",
    "copy_if_plain": "CopyIfPlain",
    "iteration_control": "IterationControl",
    "non_escaping_guard": "NonEscapingGuard",
}
_VECTOR_REDUCTION_RULES = {
    "sum": "Sum",
    "product": "Product",
    "and": "And",
    "or": "Or",
    "min": "Min",
    "max": "Max",
}
_LIR_VERIFY_RULES = {
    "box_value": "BoxValue",
    "unbox_value": "UnboxValue",
    "checked_i64_arithmetic": "CheckedI64Arithmetic",
    "truthy_materialization": "TruthyMaterialization",
}
_REPR_RAW_I64_FULL_DEOPT_SEED_RULES = {
    "checked_result0": "CheckedResultZero",
    "const_int_not_inline_safe": "ConstIntNotInlineSafe",
}
_REPR_PROJECTABLE_BOOL_RESULT_RULES = {
    "always": "Always",
    "result1": "ResultOne",
    "all_operands_bool": "AllOperandsBool",
    "index_raw_i64_index": "IndexRawI64Index",
    "copy_source_bool": "CopySourceBool",
}
_REPR_PROJECTABLE_FLOAT_RESULT_RULES = {
    "always": "Always",
    "all_operands_projectable": "AllOperandsProjectable",
    "first_operand_projectable": "FirstOperandProjectable",
    "copy_source_float": "CopySourceFloat",
}
_COUNTED_LOOP_COMPARISON_ROLES = {
    "increasing_exclusive": "IncreasingExclusive",
    "increasing_inclusive": "IncreasingInclusive",
    "decreasing_exclusive": "DecreasingExclusive",
    "decreasing_inclusive": "DecreasingInclusive",
}
_MODULE_CONCURRENCY_MARKER_SOURCE_ROLES = {
    "none": "None",
    "module_name": "ModuleName",
    "thread_intrinsic_callee": "ThreadIntrinsicCallee",
}
_MODULE_SLOT_ACCESS_ROLES = {
    "none": "None",
    "keyed_attr": "KeyedAttr",
    "wildcard_module_dict": "WildcardModuleDict",
}
_TIR_VERIFY_ATTR_RULES = {
    "call_callee": "CallCallee",
    "call_method": "CallMethod",
    "positive_payload_bytes": "PositivePayloadBytes",
}
_SROA_CONST_IMMEDIATE_RULES = {
    "always_immediate": "AlwaysImmediate",
    "inline_int_if_range": "InlineIntIfRange",
}
_STRENGTH_REDUCTION_RULES = {
    "mul_by_two": "MulByTwo",
    "pow_square": "PowSquare",
    "power_two_floor_div": "PowerTwoFloorDiv",
    "power_two_mod": "PowerTwoMod",
}
_SCEV_EXPR_RULES = {
    "add": "Add",
    "sub": "Sub",
    "mul": "Mul",
}
_CALL_OPCODE_ROLES = {
    "not_call": "NotCall",
    "user_call": "UserCall",
    "dynamic_method": "DynamicMethod",
    "runtime_builtin": "RuntimeBuiltin",
    "copy_original_kind": "CopyOriginalKind",
}
_SSA_S_VALUE_ATTR_KEYS = {"module", "name", "method"}
_EXCEPTION_REGION_NESTING_ROLES = {
    "none": "None",
    "enter": "Enter",
    "exit": "Exit",
}
_GENERATOR_FUSION_ITER_USE_ROLES = {
    "none": "None",
    "next_use": "NextUse",
    "none_guard": "NoneGuard",
}
_FUZZ_TIR_ATTR_PAYLOAD_RULES = {
    "none": "None",
    "i64_value": "I64Value",
    "f64_value": "F64Value",
    "bool_value": "BoolValue",
}
_FUZZ_TIR_OPCODE_ATTR_PAYLOAD_RULES = {
    "ConstInt": "i64_value",
    "ConstFloat": "f64_value",
    "ConstBool": "bool_value",
}
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
_SIMPLEIR_CONTROL_FACT_FIELDS = (
    "structural",
    "terminator",
    "suspend",
    "repoll",
    "block_leader",
    "block_ender",
    "conditional_branch",
    "pre_ssa_rewritten",
    "ssa_only",
    "wasm_split_barrier",
    "wasm_dispatch_block_leader",
    "wasm_dispatch_block_terminator",
    "wasm_stateful_dispatch",
    "wasm_state_resume_after",
    "wasm_state_resume_at",
)

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
    "escape_alloc_site_opcodes",
    "polyhedral_loop_header_opcodes",
    "polyhedral_affine_body_opcodes",
    "refcount_heap_exposure_opcodes",
    "refcount_balance_inc_opcodes",
    "refcount_balance_dec_opcodes",
    "lowered_state_machine_body_opcodes",
    "boxed_runtime_inplace_dispatch_opcodes",
    "drop_insertion_suspension_point_opcodes",
    "drop_insertion_return_deferral_barrier_opcodes",
    "fusion_barrier_opcodes",
    "generator_fusion_poll_required_yield_opcodes",
    "generator_fusion_poll_reject_opcodes",
    "state_machine_opcodes",
    "overflow_peel_guard_compare_opcodes",
    "overflow_peel_body_pure_opcodes",
    "exception_handling_opcodes",
    "exception_handler_region_opcodes",
    "structured_scf_marker_opcodes",
    "i64_overflow_box_dispatch_opcodes",
    "i64_checked_overflow_triple_opcodes",
    "i64_zero_divisor_guard_opcodes",
    "i64_shift_count_guard_opcodes",
    "gvn_always_numberable_opcodes",
    "gvn_type_gated_numberable_opcodes",
    "proven_result_type_seed_opcodes",
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
_REFCOUNT_BALANCE_ROLE_SETS = (
    "refcount_balance_inc_opcodes",
    "refcount_balance_dec_opcodes",
)
_GENERATOR_FUSION_POLL_ROLE_SETS = (
    "generator_fusion_poll_required_yield_opcodes",
    "generator_fusion_poll_reject_opcodes",
)
_GVN_NUMBERING_ROLE_SETS = (
    "gvn_always_numberable_opcodes",
    "gvn_type_gated_numberable_opcodes",
    "gvn_value_keyed_constant_opcodes",
)

__all__ = (
    "_ALIAS_MEMORY_REGION_SETS",
    "_ALIAS_SLOT_OBSERVATION_SETS",
    "_ALIAS_TRANSPARENT_ALIAS_ROLE_SETS",
    "_ALIAS_TYPED_SLOT_ROLE_SETS",
    "_CALL_OPCODE_ROLES",
    "_CANONICALIZE_BINARY_ACTIONS",
    "_CANONICALIZE_BINARY_PREDICATES",
    "_CANONICALIZE_BINARY_TYPE_GUARDS",
    "_CANONICALIZE_COMMUTATIVE_DOMAINS",
    "_CLASSIFIER_SETS",
    "_COUNTED_LOOP_COMPARISON_ROLES",
    "_EXCEPTION_REGION_NESTING_ROLES",
    "_FRONTEND_EFFECT_VALUES",
    "_FUZZ_TIR_ATTR_PAYLOAD_RULES",
    "_FUZZ_TIR_OPCODE_ATTR_PAYLOAD_RULES",
    "_GENERATOR_FUSION_ITER_USE_ROLES",
    "_GENERATOR_FUSION_POLL_ROLE_SETS",
    "_GVN_NUMBERING_ROLE_SETS",
    "_GVN_VALUE_KEY_KINDS",
    "_LIR_VERIFY_RULES",
    "_LITERAL_PAYLOAD_KINDS",
    "_MODULE_CONCURRENCY_MARKER_SOURCE_ROLES",
    "_MODULE_SLOT_ACCESS_ROLES",
    "_OPCODE_FACT_SETS",
    "_OPERAND_INDEPENDENT_RESULT_TYPES",
    "_OPERAND_OWNERSHIP_LEAVES",
    "_OPERAND_OWNERSHIP_UNIFORM",
    "_PASS_DELTA_FACT_FIELDS",
    "_PURITY_VALUES",
    "_RANGE_DEVIRT_ROLES",
    "_REFCOUNT_BALANCE_ROLE_SETS",
    "_REPR_PROJECTABLE_BOOL_RESULT_RULES",
    "_REPR_PROJECTABLE_FLOAT_RESULT_RULES",
    "_REPR_RAW_I64_FULL_DEOPT_SEED_RULES",
    "_RESULT_ARITY_VALUES",
    "_RESULT_VALIDITY_VALUES",
    "_SCCP_CONSTANT_EVAL_RULES",
    "_SCCP_CONSTANT_SEED_RULES",
    "_SCEV_EXPR_RULES",
    "_SIMPLEIR_CONTROL_FACT_FIELDS",
    "_SROA_CONST_IMMEDIATE_RULES",
    "_SSA_S_VALUE_ATTR_KEYS",
    "_STRENGTH_REDUCTION_RULES",
    "_TERMINATOR_OWNERSHIP_LEAVES",
    "_TERMINATOR_VARIANTS",
    "_TIR_VERIFY_ATTR_RULES",
    "_TYPE_REFINE_ATTR_RESULT_TYPE_RULES",
    "_TYPE_REFINE_OPERAND_TYPE_RULES",
    "_VALUE_RANGE_COND_NARROW_RULES",
    "_VALUE_RANGE_CONST_FOLD_RULES",
    "_VALUE_RANGE_CONTAINER_LENGTH_RULES",
    "_VALUE_RANGE_TRANSFER_RULES",
    "_VARIABLE_RESULT_ARITY_OPCODES",
    "_VECTORIZE_BODY_ACTIONS",
    "_VECTOR_REDUCTION_RULES",
)
