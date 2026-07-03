//! Scalar/container representation-plan facts derived from `FunctionIR`/`OpIR`.
//!
//! Moved verbatim from `representation_plan/tests.rs` during the move-only
//! split of that god-file; no logic changes.

use super::super::test_fixtures::{function, op};
use super::super::*;

fn const_int(out: &str, value: i64) -> OpIR {
    OpIR {
        kind: "const".to_string(),
        out: Some(out.to_string()),
        value: Some(value),
        ..OpIR::default()
    }
}

fn const_bool(out: &str, value: bool) -> OpIR {
    OpIR {
        kind: "const_bool".to_string(),
        out: Some(out.to_string()),
        value: Some(i64::from(value)),
        ..OpIR::default()
    }
}

fn const_float(out: &str, value: f64) -> OpIR {
    OpIR {
        kind: "const_float".to_string(),
        out: Some(out.to_string()),
        f_value: Some(value),
        ..OpIR::default()
    }
}

#[test]
fn dynbox_i64_fact_is_not_a_scalar_integer() {
    let mut plan = ScalarRepresentationPlan::default();
    plan.insert_fact(
        "boxed_word".to_string(),
        ScalarRepresentationFact {
            ty: TirType::I64,
            repr: LirRepr::DynBox,
        },
    );
    let func = function("empty", &[], None, vec![]);
    let fact_index = FunctionFactIndex::for_function(&func);
    plan.propagate_integer_family(&func, &fact_index);

    let (int_like, _, _, _, _) = plan.scalar_name_sets();

    assert!(!int_like.contains("boxed_word"));
    assert!(!plan.integer_family_names().contains("boxed_word"));
}

#[test]
fn container_kind_comes_from_structured_tir_types() {
    let func = function(
        "typed_containers",
        &["xs", "d", "s", "t", "text"],
        Some(vec![
            "list[int]",
            "dict[str, int]",
            "set[bool]",
            "tuple[int, str]",
            "str",
        ]),
        vec![op("ret", None, None, &["xs"])],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.name_container_kind("xs"), Some(ContainerKind::List));
    assert_eq!(plan.name_container_kind("d"), Some(ContainerKind::Dict));
    assert_eq!(plan.name_container_kind("s"), Some(ContainerKind::Set));
    assert_eq!(plan.name_container_kind("t"), Some(ContainerKind::Tuple));
    assert_eq!(plan.name_container_kind("text"), Some(ContainerKind::Str));
}

#[test]
fn container_transport_metadata_does_not_seed_container_kind() {
    let mut index = op("index", Some("item"), None, &["xs", "i"]);
    index.container_type = Some("list".to_string());
    index.type_hint = Some("list".to_string());
    let func = function("transport_only", &["xs", "i"], None, vec![index]);
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.name_container_kind("xs"), None);
    assert_eq!(plan.name_container_kind("item"), None);
}

#[test]
fn flat_list_storage_requires_structural_producer() {
    let mut index = op("index", Some("item"), None, &["xs", "i"]);
    index.container_type = Some("list".to_string());
    let func = function(
        "transport_only_storage",
        &["xs", "i"],
        None,
        vec![index.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.name_container_storage_kind("xs"), None);
    assert!(!plan.op_has_container_storage(0, &index, ContainerStorageKind::FlatListInt));
}

#[test]
fn list_int_new_seeds_flat_storage_and_aliases() {
    let list_new = op("list_int_new", Some("xs"), None, &[]);
    let copy = op("copy", Some("ys"), None, &["xs"]);
    let store = op("store_var", None, Some("slot"), &["ys"]);
    let load = op("load_var", Some("zs"), Some("slot"), &[]);
    let index = op("index", Some("item"), None, &["zs", "i"]);
    let func = function(
        "storage_aliases",
        &["i"],
        Some(vec!["int"]),
        vec![list_new, copy, store, load, index.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(
        plan.name_container_storage_kind("xs"),
        Some(ContainerStorageKind::FlatListInt)
    );
    assert_eq!(
        plan.name_container_storage_kind("ys"),
        Some(ContainerStorageKind::FlatListInt)
    );
    assert_eq!(
        plan.name_container_storage_kind("slot"),
        Some(ContainerStorageKind::FlatListInt)
    );
    assert_eq!(
        plan.name_container_storage_kind("zs"),
        Some(ContainerStorageKind::FlatListInt)
    );
    assert!(plan.op_has_container_storage(4, &index, ContainerStorageKind::FlatListInt));
}

#[test]
fn non_int_store_index_conflicts_flat_list_storage() {
    let list_new = op("list_int_new", Some("xs"), None, &[]);
    let idx = const_int("i", 0);
    let value = const_float("f", 1.25);
    let store = op("store_index", Some("ys"), None, &["xs", "i", "f"]);
    let index = op("index", Some("item"), None, &["ys", "i"]);
    let func = function(
        "flat_storage_non_int_write",
        &[],
        None,
        vec![list_new, idx, value, store.clone(), index.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.name_container_storage_kind("xs"), None);
    assert_eq!(plan.name_container_storage_kind("ys"), None);
    assert!(!plan.op_has_container_storage(3, &store, ContainerStorageKind::FlatListInt));
    assert!(!plan.op_has_container_storage(4, &index, ContainerStorageKind::FlatListInt));
}

#[test]
fn semantic_list_bool_index_does_not_authorize_raw_bool_primary() {
    let index = op("index", Some("item"), None, &["items", "idx"]);
    let func = function(
        "typed_list_bool_index",
        &["items", "idx"],
        Some(vec!["list[bool]", "int"]),
        vec![index],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (_, bool_like, _, _, _) = plan.scalar_name_sets();
    let primary = plan.primary_name_sets();

    assert!(
        bool_like.contains("item"),
        "semantic list[bool] indexing should refine the element type"
    );
    assert!(
        !primary.bool_.contains("item"),
        "semantic element type alone must not prove native raw-bool carrier codegen"
    );
    assert!(
        !plan.is_bool_unboxed("item"),
        "native raw-bool predicate must derive from repr_by_name eligibility, not semantic type"
    );
}

#[test]
fn index_result_lane_comes_from_element_fact_not_key() {
    let index = op("index", Some("item"), None, &["items", "idx"]);
    let func = function(
        "typed_list_int_index",
        &["items", "idx"],
        Some(vec!["list[int]", "int"]),
        vec![index.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, _, _, _) = plan.scalar_name_sets();
    let primary = plan.primary_name_sets();

    assert_eq!(plan.op_scalar_lane(&index), Some(ScalarKind::Int));
    assert!(plan.op_index_key_is_integer_family(&index));
    assert!(int_like.contains("item"));
    assert!(
        !primary.int.contains("item"),
        "generic index results are boxed transport unless lowering proves a raw element carrier"
    );
}

#[test]
fn ord_at_result_is_integer_family_from_tir_not_transport_hints() {
    let mut ord_at = op("ord_at", Some("code"), None, &["text", "idx"]);
    ord_at.type_hint = Some("list".to_string());
    ord_at.container_type = Some("list".to_string());
    ord_at.fast_int = Some(true);
    let add = op("add", Some("shifted"), None, &["code", "bias"]);
    let func = function(
        "ord_at_representation",
        &[],
        None,
        vec![
            OpIR {
                kind: "const_str".to_string(),
                out: Some("text".to_string()),
                s_value: Some("AéZ".to_string()),
                ..OpIR::default()
            },
            const_int("idx", 1),
            ord_at,
            const_int("bias", 1),
            add.clone(),
        ],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, _, _, _) = plan.scalar_name_sets();

    assert!(
        int_like.contains("code"),
        "ord_at result must be proven by first-class typed TIR/LIR lowering"
    );
    assert!(
        plan.name_is_integer_family("shifted"),
        "downstream arithmetic must consume ord_at's structural integer-family fact"
    );
    assert_eq!(plan.name_container_kind("code"), None);
    assert_eq!(plan.op_scalar_lane(&add), Some(ScalarKind::Int));
    assert!(
        plan.integer_family_names().contains("code"),
        "legacy result metadata must not be required for ord_at integer-family propagation"
    );
}

#[test]
fn generic_index_does_not_promote_result_from_integer_key() {
    let index = op(
        "index",
        Some("object_type_tag"),
        None,
        &["__molt_split_frame", "__molt_split_frame_index"],
    );
    let func = function(
        "split_frame_index",
        &[],
        None,
        vec![
            op("list_new", Some("__molt_split_frame"), None, &[]),
            const_int("__molt_split_frame_index", 0),
            index.clone(),
            op(
                "builtin_type",
                Some("object_type"),
                None,
                &["object_type_tag"],
            ),
        ],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, _, _, _) = plan.scalar_name_sets();
    let primary = plan.primary_name_sets();

    assert_eq!(plan.op_scalar_lane(&index), None);
    assert!(plan.op_index_key_is_integer_family(&index));
    assert!(!int_like.contains("object_type_tag"));
    assert!(!primary.int.contains("object_type_tag"));
}

#[test]
fn alias_group_unknown_loop_header_source_terminates_without_promotion() {
    let func = function(
        "alias_group_unknown_loop_header_source",
        &[],
        None,
        vec![
            const_int("zero", 0),
            op("const_none", Some("none"), None, &[]),
            const_int("one", 1),
            op("store_var", None, Some("_bb2_arg0"), &["zero"]),
            op("store_var", None, Some("_bb2_arg0"), &["none"]),
            op("load_var", Some("_v19"), Some("_bb2_arg0"), &[]),
            op("add", Some("next"), None, &["one", "one"]),
            op("store_var", None, Some("_v19"), &["next"]),
            op("copy_var", Some("after"), None, &["_v19"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, _, _, _) = plan.scalar_name_sets();

    assert!(
        !int_like.contains("_v19"),
        "ambiguous loop-header alias/store join must not re-promote _v19"
    );
    assert!(
        !int_like.contains("after"),
        "aliases fed by an ambiguous loop-header source must stay unpromoted"
    );
}

#[test]
fn pending_store_target_dominates_same_name_alias_output() {
    let func = function(
        "pending_store_target_dominates_same_name_alias_output",
        &[],
        None,
        vec![
            const_int("one", 1),
            op("copy_var", Some("slot"), None, &["one"]),
            op("store_var", None, Some("slot"), &["unproven_source"]),
            op("copy_var", Some("after"), None, &["slot"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, _, _, _) = plan.scalar_name_sets();

    assert!(
        !int_like.contains("slot"),
        "pending store target must prevent same-name alias output reinsertion"
    );
    assert!(
        !int_like.contains("after"),
        "aliases from a pending store target must not inherit stale facts"
    );
}

#[test]
fn pending_store_target_remains_relevant_for_same_name_alias_output() {
    let func = function(
        "pending_store_target_remains_relevant_for_same_name_alias_output",
        &[],
        None,
        vec![
            const_int("one", 1),
            op("copy_var", Some("slot"), None, &["one"]),
            op("store_var", None, Some("slot"), &["unproven_source"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, _, _, _) = plan.scalar_name_sets();

    assert!(
        !int_like.contains("slot"),
        "same-name alias output must keep a pending store target relevant"
    );
}

#[test]
fn pending_alias_source_blocks_store_target_reinsert_loop() {
    let func = function(
        "pending_alias_source_blocks_store_target_reinsert_loop",
        &[],
        None,
        vec![
            const_int("one", 1),
            op("store_var", None, Some("loop_slot"), &["unproven_source"]),
            op("load_var", Some("iv"), Some("loop_slot"), &[]),
            op("store_var", None, Some("iv"), &["one"]),
            op("copy_var", Some("after"), None, &["iv"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, _, _, _) = plan.scalar_name_sets();

    assert!(
        !int_like.contains("iv"),
        "a name defined by both a pending load alias and a store target must not oscillate back to int"
    );
    assert!(
        !int_like.contains("after"),
        "aliases fed by the blocked name must not inherit a stale fact"
    );
}

#[test]
fn iter_next_done_flag_uses_fused_bool_fact_not_index_fast_int_hint() {
    let mut done_index = const_int("done_index", 1);
    done_index.fast_int = Some(true);
    let mut done = op("index", Some("done_flag"), None, &["pair", "done_index"]);
    done.fast_int = Some(true);
    let mut value_index = const_int("value_index", 0);
    value_index.fast_int = Some(true);
    let mut value = op("index", Some("next_value"), None, &["pair", "value_index"]);
    value.fast_int = Some(true);
    let func = function(
        "iter_next_done_flag",
        &["items"],
        None,
        vec![
            op("iter", Some("iter_obj"), None, &["items"]),
            op("iter_next", Some("pair"), None, &["iter_obj"]),
            done_index,
            done.clone(),
            value_index,
            value,
            op("loop_break_if_true", None, None, &["done_flag"]),
        ],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, bool_like, _, _, _) = plan.scalar_name_sets();
    let primary = plan.primary_name_sets();

    assert!(
        bool_like.contains("done_flag"),
        "fused iter_next done flag must retain its bool fact under the original SimpleIR name"
    );
    assert!(
        !int_like.contains("done_flag"),
        "index fast_int metadata cannot override the fused done flag's bool type"
    );
    assert_eq!(plan.op_scalar_lane(&done), Some(ScalarKind::Bool));
    assert!(
        !primary.int.contains("done_flag"),
        "done flag must never be routed through raw-int primary storage"
    );
}

#[test]
fn conflicting_facts_do_not_pick_order_dependent_scalar_lane() {
    let mut plan = ScalarRepresentationPlan::default();
    plan.insert_fact(
        "ambiguous".to_string(),
        ScalarRepresentationFact {
            ty: TirType::I64,
            repr: LirRepr::I64,
        },
    );
    plan.insert_fact(
        "ambiguous".to_string(),
        ScalarRepresentationFact {
            ty: TirType::Bool,
            repr: LirRepr::Bool1,
        },
    );
    let func = function("empty", &[], None, vec![]);
    let fact_index = FunctionFactIndex::for_function(&func);
    plan.propagate_integer_family(&func, &fact_index);

    let (int_like, bool_like, _, _, _) = plan.scalar_name_sets();

    assert!(!int_like.contains("ambiguous"));
    assert!(!bool_like.contains("ambiguous"));
    assert!(!plan.integer_family_names().contains("ambiguous"));
}

#[test]
fn plan_uses_entry_param_names_as_scalar_facts() {
    let func = function(
        "typed_params",
        &["x", "flag"],
        Some(vec!["int", "bool"]),
        vec![op("ret", None, Some("x"), &[])],
    );

    let (int_like, bool_like, _, _, _) =
        ScalarRepresentationPlan::for_function_ir(&func).scalar_name_sets();

    assert!(int_like.contains("x"));
    assert!(bool_like.contains("flag"));
}

#[test]
fn plan_propagates_store_targets_only_when_all_sources_match() {
    let mixed = function(
        "mixed_store",
        &[],
        None,
        vec![
            const_int("i", 1),
            const_bool("b", true),
            op("store_var", None, Some("slot"), &["i"]),
            op("store_var", None, Some("slot"), &["b"]),
            op("ret", None, Some("slot"), &[]),
        ],
    );
    let (int_like, bool_like, _, _, _) =
        ScalarRepresentationPlan::for_function_ir(&mixed).scalar_name_sets();
    assert!(!int_like.contains("slot"));
    assert!(!bool_like.contains("slot"));

    let uniform = function(
        "uniform_store",
        &[],
        None,
        vec![
            const_int("i", 1),
            op("store_var", None, Some("slot"), &["i"]),
            op("ret", None, Some("slot"), &[]),
        ],
    );
    let (int_like, _, _, _, _) =
        ScalarRepresentationPlan::for_function_ir(&uniform).scalar_name_sets();
    assert!(int_like.contains("slot"));
}

#[test]
fn unknown_store_target_blocks_alias_output_reinsertion() {
    let func = function(
        "store_alias_output_cycle",
        &[],
        None,
        vec![
            op("store_var", None, Some("slot"), &["unknown_source"]),
            op("copy", Some("slot"), None, &["seed"]),
            op("load_var", Some("loaded"), Some("slot"), &[]),
        ],
    );
    let fact_index = FunctionFactIndex::for_function(&func);
    let mut plan = ScalarRepresentationPlan::default();
    let int_fact = ScalarRepresentationFact {
        ty: TirType::I64,
        repr: LirRepr::I64,
    };
    plan.insert_fact("seed".to_string(), int_fact.clone());
    plan.insert_fact("slot".to_string(), int_fact);

    let indexed_fact_index = IndexedFunctionFactIndex::for_function_facts(&fact_index);
    plan.propagate_simple_aliases(&indexed_fact_index);

    let (int_like, _, _, _, _) = plan.scalar_name_sets();
    assert!(int_like.contains("seed"));
    assert!(
        !int_like.contains("slot"),
        "unknown store targets must not be reintroduced through alias outputs"
    );
    assert!(
        !int_like.contains("loaded"),
        "aliases loaded from an unknown store target must remain unproven"
    );
}

#[test]
fn generic_type_hint_does_not_seed_plan_scalar_fact() {
    let mut generic = op("call", Some("maybe_int"), None, &[]);
    generic.type_hint = Some("int".to_string());
    let func = function("generic_hint", &[], None, vec![generic]);

    let (int_like, _, _, _, _) =
        ScalarRepresentationPlan::for_function_ir(&func).scalar_name_sets();

    assert!(!int_like.contains("maybe_int"));
}

#[test]
fn integer_family_preserves_boxed_unbounded_arithmetic_lane() {
    let func = function(
        "integer_family",
        &["seed"],
        Some(vec!["int"]),
        vec![
            const_int("factor", 3_266_489_917),
            op("mul", Some("wide"), None, &["seed", "factor"]),
            const_int("mask", 7),
            op("bit_or", Some("masked"), None, &["wide", "mask"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let (int_like, _, float_like, _, _) = plan.scalar_name_sets();
    let integer_family = plan.integer_family_names();

    assert!(integer_family.contains("wide"));
    assert!(integer_family.contains("masked"));
    assert!(!int_like.contains("wide"));
    assert!(!float_like.contains("wide"));
    assert!(!float_like.contains("masked"));
}

#[test]
fn primary_int_names_admit_bounded_arithmetic_range_proof() {
    let func = function(
        "int_primary",
        &[],
        None,
        vec![
            const_int("lhs", 5),
            const_int("rhs", 3),
            op("bit_xor", Some("masked"), None, &["lhs", "rhs"]),
            op("add", Some("sum"), None, &["lhs", "rhs"]),
            op("lshift", Some("shifted"), None, &["lhs", "rhs"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let primary = plan.primary_name_sets();

    assert!(primary.int.contains("lhs"));
    assert!(primary.int.contains("rhs"));
    assert!(primary.int.contains("masked"));
    assert!(primary.int.contains("sum"));
    assert!(!primary.int.contains("shifted"));
}

#[test]
fn primary_int_names_exclude_unbounded_param_arithmetic_without_range_proof() {
    let func = function(
        "int_primary_params",
        &["lhs", "rhs"],
        Some(vec!["int", "int"]),
        vec![op("add", Some("sum"), None, &["lhs", "rhs"])],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let primary = plan.primary_name_sets();

    assert!(!primary.int.contains("lhs"));
    assert!(!primary.int.contains("rhs"));
    assert!(!primary.int.contains("sum"));
}

#[test]
fn primary_int_names_exclude_arithmetic_that_can_overflow_i64() {
    let func = function(
        "int_primary_overflow",
        &[],
        None,
        vec![
            const_int("lhs", i64::MAX),
            const_int("rhs", 1),
            op("add", Some("sum"), None, &["lhs", "rhs"]),
        ],
    );

    let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

    assert!(primary.int.contains("lhs"));
    assert!(primary.int.contains("rhs"));
    assert!(!primary.int.contains("sum"));
}

#[test]
fn counted_store_load_loop_proves_bounded_i64_add() {
    let func = function(
        "counted_store_load_loop",
        &[],
        None,
        vec![
            const_int("init", 0),
            const_int("one", 1),
            const_int("stop", 1_000_000),
            op("store_var", None, Some("i"), &["init"]),
            op("loop_start", None, None, &[]),
            op("load_var", Some("i_cur"), Some("i"), &[]),
            op("lt", Some("keep_going"), None, &["i_cur", "stop"]),
            op("loop_break_if_false", None, None, &["keep_going"]),
            op("add", Some("i_next"), None, &["i_cur", "one"]),
            op("store_var", None, Some("i"), &["i_next"]),
            op("loop_continue", None, None, &[]),
            op("loop_end", None, None, &[]),
            op("load_var", Some("i_after"), Some("i"), &[]),
        ],
    );

    let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

    assert!(primary.int.contains("i"));
    assert!(primary.int.contains("i_cur"));
    assert!(primary.int.contains("i_next"));
    assert!(primary.int.contains("i_after"));
}

#[test]
fn mismatched_counted_loop_direction_does_not_prove_update_range() {
    let func = function(
        "mismatched_counted_loop",
        &[],
        None,
        vec![
            const_int("init", 0),
            const_int("one", 1),
            const_int("stop", 1_000_000),
            op("store_var", None, Some("i"), &["init"]),
            op("loop_start", None, None, &[]),
            op("load_var", Some("i_cur"), Some("i"), &[]),
            op("gt", Some("keep_going"), None, &["i_cur", "stop"]),
            op("loop_break_if_false", None, None, &["keep_going"]),
            op("add", Some("i_next"), None, &["i_cur", "one"]),
            op("store_var", None, Some("i"), &["i_next"]),
            op("loop_continue", None, None, &[]),
            op("loop_end", None, None, &[]),
        ],
    );

    let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

    assert!(!primary.int.contains("i"));
    assert!(!primary.int.contains("i_cur"));
    assert!(!primary.int.contains("i_next"));
}

#[test]
fn bool_primary_projection_is_tir_value_owned() {
    let func = function(
        "bool_primary_projection",
        &["lhs", "rhs"],
        Some(vec!["int", "int"]),
        vec![
            const_bool("flag", true),
            op("copy_var", Some("flag_copy"), Some("flag"), &[]),
            op("eq", Some("cmp"), None, &["lhs", "rhs"]),
            op("not", Some("negated"), None, &["cmp"]),
            op("is_truthy", Some("legacy_truthy"), None, &["flag"]),
        ],
    );

    let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

    for name in ["flag", "flag_copy", "cmp", "negated"] {
        assert!(
            primary.bool_.contains(name),
            "{name} must be projected through TIR bool ValueId facts; got {:?}",
            primary.bool_
        );
    }
    assert!(
        !primary.bool_.contains("legacy_truthy"),
        "legacy SimpleIR truthiness must not mint a raw bool carrier"
    );
}

#[test]
fn scalar_lane_does_not_classify_unbounded_int_pow_as_inline_int() {
    let pow = op("pow", Some("powv"), None, &["base", "exp"]);
    let func = function(
        "int_pow",
        &["base", "exp"],
        Some(vec!["int", "int"]),
        vec![pow.clone()],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.op_scalar_lane(&pow), None);
}

#[test]
fn transport_hints_do_not_prove_scalar_representation() {
    let mut add = op("add", Some("sum"), None, &["lhs", "rhs"]);
    add.fast_int = Some(true);
    add.fast_float = Some(true);
    add.type_hint = Some("int".to_string());
    let func = function("hinted_add", &["lhs", "rhs"], None, vec![add.clone()]);

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.op_scalar_lane(&add), None);
    assert!(!plan.op_prefers_integer_runtime_lane(&add));
    assert!(!plan.op_args_are_integer_family(&add));
}

#[test]
fn typed_operands_prove_integer_runtime_lane_without_transport_hints() {
    let add = op("add", Some("sum"), None, &["lhs", "rhs"]);
    let mul = op("mul", Some("product"), None, &["lhs", "rhs"]);
    let func = function(
        "typed_add",
        &["lhs", "rhs"],
        Some(vec!["int", "int"]),
        vec![add.clone(), mul.clone()],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(plan.op_prefers_integer_runtime_lane(&add));
    assert!(plan.op_prefers_integer_runtime_lane(&mul));
    assert!(plan.op_args_are_integer_family(&add));
    assert!(plan.op_args_are_integer_family(&mul));
}

#[test]
fn list_repeat_does_not_take_integer_runtime_lane() {
    let list_new = op("list_new", Some("items"), None, &["item"]);
    let repeat = op("mul", Some("repeated"), None, &["items", "count"]);
    let func = function(
        "list_repeat",
        &["item", "count"],
        Some(vec!["bool", "int"]),
        vec![list_new, repeat.clone()],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.name_scalar_kind("items"), None);
    assert!(!plan.op_prefers_integer_runtime_lane(&repeat));
    assert!(!plan.op_args_are_integer_family(&repeat));
}

#[test]
fn scalar_lane_keeps_float_pow_on_float_lane() {
    let pow = op("pow", Some("powv"), None, &["base", "exp"]);
    let func = function(
        "float_pow",
        &["base", "exp"],
        Some(vec!["float", "float"]),
        vec![pow.clone()],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(plan.op_scalar_lane(&pow), Some(ScalarKind::Float));
}

#[test]
fn scalar_store_targets_are_plan_owned_and_all_sources() {
    let func = function(
        "scalar_store_targets",
        &["callable", "args"],
        None,
        vec![
            const_int("i_seed", 7),
            op("copy_var", Some("i_copy"), None, &["i_seed"]),
            op("store_var", None, Some("i_slot"), &["i_copy"]),
            const_float("f_seed", 1.25),
            op("copy_var", Some("f_copy"), Some("f_seed"), &[]),
            op("store_var", None, Some("f_slot"), &["f_copy"]),
            const_bool("b_seed", true),
            op("identity_alias", Some("b_copy"), None, &["b_seed"]),
            op("store_var", None, Some("b_slot"), &["b_copy"]),
            OpIR {
                kind: "const_str".to_string(),
                out: Some("s_seed".to_string()),
                s_value: Some("lane".to_string()),
                ..OpIR::default()
            },
            op("copy", Some("s_copy"), None, &["s_seed"]),
            op("store_var", None, Some("s_slot"), &["s_copy"]),
            op("store_var", None, Some("mixed_slot"), &["i_seed"]),
            op("store_var", None, Some("mixed_slot"), &["f_seed"]),
            op(
                "call_indirect",
                Some("dynamic"),
                None,
                &["callable", "args"],
            ),
            op("store_var", None, Some("dynamic_slot"), &["dynamic"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(
        plan.scalar_store_targets(ScalarKind::Int),
        BTreeSet::from(["i_slot".to_string()]),
    );
    assert_eq!(
        plan.scalar_store_targets(ScalarKind::Float),
        BTreeSet::from(["f_slot".to_string()]),
    );
    assert_eq!(
        plan.scalar_store_targets(ScalarKind::Bool),
        BTreeSet::from(["b_slot".to_string()]),
    );
    assert_eq!(
        plan.scalar_store_targets(ScalarKind::Str),
        BTreeSet::from(["s_slot".to_string()]),
    );
}

#[test]
fn raw_loop_iv_copy_used_by_object_ops_stays_primary_until_escape() {
    let func = function(
        "raw_loop_iv_copy_used_by_object_ops",
        &[],
        None,
        vec![
            op("missing", Some("missing_i"), None, &[]),
            op("store_var", None, Some("i"), &["missing_i"]),
            op("copy_var", Some("missing_copy"), None, &["missing_i"]),
            const_int("stop", 3),
            const_int("zero", 0),
            const_int("one", 1),
            op("copy_var", Some("zero_copy"), None, &["zero"]),
            op("store_var", None, Some("_bb1_arg0"), &["zero_copy"]),
            op("store_var", None, Some("_bb1_arg1"), &["missing_copy"]),
            op("loop_start", None, None, &[]),
            op("load_var", Some("iv"), Some("_bb1_arg0"), &[]),
            op("load_var", Some("carried_obj"), Some("_bb1_arg1"), &[]),
            op("lt", Some("cond"), None, &["iv", "stop"]),
            op("loop_break_if_false", None, None, &["cond"]),
            op("store_var", None, Some("i"), &["iv"]),
            op("copy_var", Some("escaped_iv"), None, &["iv"]),
            op("check_exception", None, None, &[]),
            op("type_of", Some("ty"), None, &["escaped_iv"]),
            op("check_exception", None, None, &[]),
            op("str_from_obj", Some("text"), None, &["escaped_iv"]),
            op(
                "exception_new_builtin_one",
                Some("exc"),
                None,
                &["escaped_iv"],
            ),
            op("add", Some("next"), None, &["iv", "one"]),
            op("store_var", None, Some("iv"), &["next"]),
            op("copy_var", Some("next_copy"), None, &["next"]),
            op("store_var", None, Some("_bb1_arg0"), &["next_copy"]),
            op("store_var", None, Some("_bb1_arg1"), &["escaped_iv"]),
            op("loop_continue", None, None, &[]),
            op("loop_end", None, None, &[]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let int_primary = plan.primary_name_sets().int;

    for name in ["_bb1_arg0", "iv", "escaped_iv", "next", "next_copy"] {
        assert!(
            int_primary.contains(name),
            "{name} must stay int-primary until boxed escape; got {int_primary:?}"
        );
    }
}

#[test]
fn float_primary_scope_excludes_pow_without_disabling_unrelated_float_defs() {
    let func = function(
        "float_primary_pow_scope",
        &["p"],
        Some(vec!["float"]),
        vec![
            const_float("base", 2.0),
            const_float("exp", 3.0),
            op("pow", Some("pow_result"), None, &["base", "exp"]),
            op("add", Some("sum"), None, &["base", "exp"]),
            op("copy_var", Some("sum_copy"), Some("sum"), &[]),
            op("copy_var", Some("param_copy"), Some("p"), &[]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let primary = plan.primary_name_sets();

    assert!(primary.float.contains("base"));
    assert!(primary.float.contains("exp"));
    assert!(primary.float.contains("sum"));
    assert!(primary.float.contains("sum_copy"));
    assert!(!primary.float.contains("pow_result"));
    assert!(!primary.float.contains("p"));
    assert!(primary.float.contains("param_copy"));
    assert!(!plan.is_float_unboxed("pow_result"));
}

#[test]
fn float_primary_store_targets_require_all_sources() {
    let func = function(
        "float_primary_store_sources",
        &[],
        None,
        vec![
            const_float("f_seed", 1.5),
            op("store_var", None, Some("float_slot"), &["f_seed"]),
            const_int("i_seed", 2),
            op("store_var", None, Some("mixed_slot"), &["f_seed"]),
            op("store_var", None, Some("mixed_slot"), &["i_seed"]),
        ],
    );

    let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

    assert!(primary.float.contains("f_seed"));
    assert!(primary.float.contains("float_slot"));
    assert!(!primary.float.contains("mixed_slot"));
}

#[test]
fn scalar_primary_excludes_missing_sentinel_store_sources() {
    let func = function(
        "scalar_primary_missing_sentinel_sources",
        &[],
        None,
        vec![
            const_int("i_seed", 7),
            op("store_var", None, Some("int_slot"), &["i_seed"]),
            op("store_var", None, Some("maybe_int_slot"), &["i_seed"]),
            const_bool("b_seed", true),
            op("store_var", None, Some("bool_slot"), &["b_seed"]),
            op("store_var", None, Some("maybe_bool_slot"), &["b_seed"]),
            const_float("f_seed", 1.5),
            op("store_var", None, Some("float_slot"), &["f_seed"]),
            op("store_var", None, Some("maybe_float_slot"), &["f_seed"]),
            op("missing", Some("missing_value"), None, &[]),
            op(
                "store_var",
                None,
                Some("maybe_int_slot"),
                &["missing_value"],
            ),
            op(
                "store_var",
                None,
                Some("maybe_bool_slot"),
                &["missing_value"],
            ),
            op(
                "store_var",
                None,
                Some("maybe_float_slot"),
                &["missing_value"],
            ),
        ],
    );

    let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

    assert!(primary.int.contains("int_slot"));
    assert!(primary.bool_.contains("bool_slot"));
    assert!(primary.float.contains("float_slot"));
    assert!(!primary.int.contains("missing_value"));
    assert!(!primary.bool_.contains("missing_value"));
    assert!(!primary.float.contains("missing_value"));
    assert!(!primary.int.contains("maybe_int_slot"));
    assert!(!primary.bool_.contains("maybe_bool_slot"));
    assert!(!primary.float.contains("maybe_float_slot"));
}

#[test]
fn cold_module_chunk_functions_have_empty_primary_sets() {
    let func = function(
        "__molt_module_chunk_0",
        &[],
        None,
        vec![
            const_int("value", 1),
            const_bool("flag", true),
            op("list_new", Some("items"), None, &["value"]),
        ],
    );

    let plan = ScalarRepresentationPlan::for_function_ir(&func);
    let primary = plan.primary_name_sets();

    assert!(primary.int.is_empty());
    assert!(primary.bool_.is_empty());
    assert!(primary.float.is_empty());
    assert_eq!(plan.name_scalar_kind("value"), None);
    assert_eq!(plan.name_scalar_kind("flag"), None);
    assert_eq!(plan.name_container_kind("items"), None);
}
