//! Scalar/container representation-plan facts derived from `FunctionIR`/`OpIR`.
//!
//! Semantic annotations do not authorize raw carriers. Exact producer facts
//! must survive supported native projection, aliases, and storage joins.

use super::super::test_fixtures::{function, op};
use super::super::*;
use crate::tir::ops::OpCode;

fn native_representation_plan(func_ir: &FunctionIR) -> ScalarRepresentationPlan {
    ScalarRepresentationPlan::for_function_ir_for_target(
        func_ir,
        &crate::tir::TargetInfo::native_release_fast(),
    )
}

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
fn binding_snapshots_keep_incoming_facts_when_storage_is_rebound() {
    for kind in ["store_var", "store_fast"] {
        let func = function(
            "binding_snapshot_facts",
            &[],
            None,
            vec![
                const_int("lhs", 1_i64 << 31),
                const_int("rhs", 1_i64 << 31),
                op(
                    "checked_mul",
                    Some("overflow"),
                    Some("wide"),
                    &["lhs", "rhs"],
                ),
                const_float("float", 1.25),
                const_bool("bool", true),
                op(kind, Some("wide_snapshot"), Some("mixed"), &["wide"]),
                op(kind, Some("float_snapshot"), Some("mixed"), &["float"]),
                op(kind, Some("bool_snapshot"), Some("mixed"), &["bool"]),
                op(kind, None, Some("wide_copy"), &["wide_snapshot"]),
                op("list_int_new", Some("items"), None, &[]),
                op(kind, Some("items_snapshot"), Some("items_slot"), &["items"]),
                op("missing", Some("missing_value"), None, &[]),
                op(
                    "delete_var",
                    Some("delete_metadata"),
                    Some("items_slot"),
                    &["missing_value", "items_snapshot"],
                ),
            ],
        );
        let plan = native_representation_plan(&func);
        assert!(plan.is_full_deopt_int_name("wide"), "{kind}");
        assert!(plan.is_full_deopt_int_name("wide_snapshot"), "{kind}");
        assert!(
            plan.name_has_scalar_kind("wide_copy", ScalarKind::Int),
            "{kind}"
        );
        assert_eq!(
            plan.is_full_deopt_int_name("wide_copy"),
            kind == "store_var",
            "field-role knowledge does not admit store_fast as a raw-carrier move"
        );
        assert!(plan.is_float_unboxed("float_snapshot"), "{kind}");
        assert!(plan.is_bool_unboxed("bool_snapshot"), "{kind}");
        assert!(!plan.is_raw_int_carrier_name("mixed"), "{kind}");
        assert!(!plan.is_float_unboxed("mixed"), "{kind}");
        assert!(!plan.is_bool_unboxed("mixed"), "{kind}");
        assert_eq!(
            plan.name_container_kind("items_snapshot"),
            Some(ContainerKind::List)
        );
        assert_eq!(plan.name_container_kind("delete_metadata"), None);
        assert!(plan.integer_family_names().contains("wide_snapshot"));
        assert!(
            plan.scalar_store_targets(ScalarKind::Int)
                .contains("wide_copy")
        );
    }
}

#[test]
fn alias_facts_follow_argument_before_transport_var_metadata() {
    for kind in [
        "copy",
        "copy_var",
        "load_var",
        "identity_alias",
        "binding_alias",
    ] {
        let func = function(
            "alias_source_precedence",
            &[],
            None,
            vec![
                const_int("integer", 7),
                const_float("float_metadata", 1.25),
                op(kind, Some("snapshot"), Some("float_metadata"), &["integer"]),
                op("store_var", None, Some("slot"), &["snapshot"]),
            ],
        );
        let index = FunctionFactIndex::for_function(&func);
        assert_eq!(
            index.alias_edges().collect::<Vec<_>>(),
            [("snapshot", "integer")]
        );
        let plan = native_representation_plan(&func);
        assert_eq!(
            plan.op_scalar_lane(&func.ops[2]),
            Some(ScalarKind::Int),
            "{kind}"
        );
        assert!(
            plan.scalar_store_targets(ScalarKind::Int).contains("slot"),
            "{kind}"
        );
        assert!(
            !plan
                .scalar_store_targets(ScalarKind::Float)
                .contains("slot"),
            "{kind}"
        );
    }
}

#[test]
fn scalar_store_facts_project_both_checked_results_and_all_constant_spellings() {
    for constant_kind in ["const", "const_int", "load_const"] {
        for checked_kind in ["checked_add", "checked_mul"] {
            assert!(matches!(
                kind_to_opcode_table(checked_kind),
                Some(OpCode::CheckedAdd | OpCode::CheckedMul)
            ));
            let mut seed = const_int("seed", 1_i64 << 31);
            seed.kind = constant_kind.into();
            let function = function(
                "checked_result_store_facts",
                &[],
                None,
                vec![
                    seed,
                    op(
                        checked_kind,
                        Some("overflow"),
                        Some("value"),
                        &["seed", "seed"],
                    ),
                    op(
                        "store_var",
                        Some("snapshot"),
                        Some("value_slot"),
                        &["value"],
                    ),
                    op("store_var", None, Some("snapshot_slot"), &["snapshot"]),
                    op("store_var", None, Some("flag_slot"), &["overflow"]),
                ],
            );
            let plan = native_representation_plan(&function);
            assert_eq!(plan.op_scalar_lane(&function.ops[0]), Some(ScalarKind::Int));
            assert_eq!(
                plan.scalar_store_targets(ScalarKind::Int),
                BTreeSet::from(["value_slot".into(), "snapshot_slot".into()]),
                "{constant_kind}/{checked_kind}"
            );
            assert_eq!(
                plan.scalar_store_targets(ScalarKind::Bool),
                BTreeSet::from(["flag_slot".into()]),
                "{constant_kind}/{checked_kind}"
            );
        }
    }
}

#[test]
fn reserved_none_operands_remain_singletons_through_lift_and_roundtrip() {
    for ops in [
        vec![
            op("copy_var", Some("snapshot"), None, &["none"]),
            op("ret", None, None, &["snapshot"]),
        ],
        vec![
            op("load_var", Some("snapshot"), Some("none"), &[]),
            op("ret", None, None, &["snapshot"]),
        ],
        vec![
            op("store_var", Some("snapshot"), Some("local"), &["none"]),
            op("ret", None, None, &["snapshot"]),
        ],
        vec![op("ret", None, None, &["none"])],
    ] {
        let mut source = function("reserved_none_roundtrip", &[], None, ops);
        for _ in 0..2 {
            let tir =
                lower_to_tir_for_target(&source, &crate::tir::TargetInfo::native_release_fast());
            assert!(
                !tir.blocks.values().flat_map(|block| &block.ops).any(|op| {
                    op.opcode == OpCode::ConstStr
                        && op.attrs.get("s_value") == Some(&AttrValue::Str("none".into()))
                }),
                "reserved singleton must never become a string"
            );
            let returned = tir
                .blocks
                .values()
                .find_map(|block| match &block.terminator {
                    crate::tir::blocks::Terminator::Return { values } => Some(values),
                    _ => None,
                })
                .expect("value-returning function");
            assert_eq!(
                returned.len(),
                1,
                "explicit None retains value-return shape"
            );
            assert_eq!(tir.value_types.get(&returned[0]), Some(&TirType::None));
            source.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir);
        }
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
    let plan = native_representation_plan(&func);

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
    let plan = native_representation_plan(&func);

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
    let plan = native_representation_plan(&func);

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
    let plan = native_representation_plan(&func);

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
    let plan = native_representation_plan(&func);

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
    let plan = native_representation_plan(&func);
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
    let plan = native_representation_plan(&func);
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
fn list_write_storage_facts_do_not_enable_the_read_index_lane() {
    let list_new = op("list_int_new", Some("items"), None, &[]);
    let index = const_int("idx", 0);
    let value = const_int("value", 7);
    let store_index = op(
        "store_index",
        Some("after_store"),
        None,
        &["items", "idx", "value"],
    );
    let dict_set = op(
        "dict_set",
        Some("after_dict_set"),
        None,
        &["items", "idx", "value"],
    );
    let func = function(
        "list_write_storage_authority",
        &[],
        None,
        vec![
            list_new,
            index,
            value,
            store_index.clone(),
            dict_set.clone(),
        ],
    );
    let plan = native_representation_plan(&func);

    assert!(!plan.op_index_key_is_integer_family(&store_index));
    assert!(!plan.op_index_key_is_integer_family(&dict_set));
    assert!(plan.op_has_container_storage(3, &store_index, ContainerStorageKind::FlatListInt,));
    assert!(plan.op_has_container_storage(4, &dict_set, ContainerStorageKind::FlatListInt,));
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
    let plan = native_representation_plan(&func);
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
    let plan = native_representation_plan(&func);
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
            op("const_none", Some("none_value"), None, &[]),
            const_int("one", 1),
            op("store_var", None, Some("_bb2_arg0"), &["zero"]),
            op("store_var", None, Some("_bb2_arg0"), &["none_value"]),
            op("load_var", Some("_v19"), Some("_bb2_arg0"), &[]),
            op("add", Some("next"), None, &["one", "one"]),
            op("store_var", None, Some("_v19"), &["next"]),
            op("copy_var", Some("after"), None, &["_v19"]),
        ],
    );

    let plan = native_representation_plan(&func);
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

    let plan = native_representation_plan(&func);
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

    let plan = native_representation_plan(&func);
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

    let plan = native_representation_plan(&func);
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
            op("loop_start", None, None, &[]),
            op("iter_next", Some("pair"), None, &["iter_obj"]),
            done_index,
            done.clone(),
            op("loop_break_if_true", None, None, &["done_flag"]),
            value_index,
            value,
            op("module_cache_set", None, None, &["next_value", "items"]),
            op("loop_continue", None, None, &[]),
            op("loop_end", None, None, &[]),
            op("ret_void", None, None, &[]),
        ],
    );
    let plan = native_representation_plan(&func);
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
fn input_producer_identity_is_separate_from_injective_lowered_names() {
    for returned in [false, true] {
        let source = function(
            "shadowed_source_identity",
            &["value"],
            None,
            vec![
                const_bool("value", true),
                if returned {
                    op("ret", None, None, &["value"])
                } else {
                    op("ret_void", None, None, &[])
                },
            ],
        );
        let tir = lower_to_tir_for_target(&source, &crate::tir::TargetInfo::native_release_fast());
        let names = SimpleValueNames::for_function(&tir);
        let produced = tir
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .find(|op| op.opcode == OpCode::ConstBool)
            .unwrap()
            .results[0];
        assert_eq!(names.source_value_name(produced), Some("value"));
        let emitted = names.value_name(produced);
        assert_ne!(emitted, "value");
        let source_plan = native_representation_plan(&source);
        assert!(
            !source_plan.is_bool_unboxed("value"),
            "one source spelling cannot select one of two SSA producers"
        );
        assert!(
            !source_plan.is_bool_unboxed(&emitted),
            "the input plan must not fabricate a fact under a future emitted suffix"
        );

        let lowered = function(
            "lowered_unique_identity",
            &["value"],
            None,
            crate::tir::lower_to_simple::lower_to_simple_ir(&tir),
        );
        let lowered_plan = native_representation_plan(&lowered);
        assert!(!lowered_plan.is_bool_unboxed("value"));
        assert!(
            lowered_plan.name_has_scalar_kind(&emitted, ScalarKind::Bool),
            "emitted transport must retain its independent semantic identity"
        );
        assert_eq!(
            lowered_plan.is_bool_unboxed(&emitted),
            !returned,
            "producer identity does not waive the Boolean escape-storage policy"
        );
    }
}

#[test]
fn constructor_fact_repair_cannot_override_ambiguous_source_identity() {
    let source = function(
        "shadowed_container_identity",
        &["value"],
        None,
        vec![
            op("list_new", Some("value"), None, &[]),
            op("ret", None, None, &["value"]),
        ],
    );
    let source_plan = native_representation_plan(&source);
    assert_eq!(
        source_plan.name_container_kind("value"),
        None,
        "the constructor fact cannot type the earlier opaque ABI parameter"
    );
    let tir = lower_to_tir_for_target(&source, &crate::tir::TargetInfo::native_release_fast());
    let lowered_ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir);
    let produced = lowered_ops
        .iter()
        .find(|op| op.kind == "list_new")
        .unwrap()
        .out
        .clone()
        .unwrap();
    assert_ne!(produced, "value");
    let lowered = function("lowered_container_identity", &["value"], None, lowered_ops);
    let lowered_plan = native_representation_plan(&lowered);
    assert_eq!(lowered_plan.name_container_kind("value"), None);
    assert_eq!(
        lowered_plan.name_container_kind(&produced),
        Some(ContainerKind::List)
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

    let (int_like, bool_like, _, _, _) = native_representation_plan(&func).scalar_name_sets();

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
    let (int_like, bool_like, _, _, _) = native_representation_plan(&mixed).scalar_name_sets();
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
    let (int_like, _, _, _, _) = native_representation_plan(&uniform).scalar_name_sets();
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

    let (int_like, _, _, _, _) = native_representation_plan(&func).scalar_name_sets();

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

    let plan = native_representation_plan(&func);
    let (int_like, _, float_like, _, _) = plan.scalar_name_sets();
    let integer_family = plan.integer_family_names();

    assert!(integer_family.contains("wide"));
    assert!(integer_family.contains("masked"));
    assert!(int_like.contains("wide"));
    assert!(!plan.is_raw_int_carrier_name("wide"));
    assert!(!plan.is_raw_int_carrier_name("masked"));
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

    let plan = native_representation_plan(&func);
    let primary = plan.primary_name_sets();

    assert!(primary.int.contains("lhs"));
    assert!(primary.int.contains("rhs"));
    assert!(primary.int.contains("masked"));
    assert!(primary.int.contains("sum"));
    assert!(primary.int_inline_safe.contains("shifted"));
}

#[test]
fn primary_int_names_exclude_unbounded_param_arithmetic_without_range_proof() {
    let func = function(
        "int_primary_params",
        &["lhs", "rhs"],
        Some(vec!["int", "int"]),
        vec![op("add", Some("sum"), None, &["lhs", "rhs"])],
    );

    let plan = native_representation_plan(&func);
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

    let primary = native_representation_plan(&func).primary_name_sets();

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

    let primary = native_representation_plan(&func).primary_name_sets();

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

    let primary = native_representation_plan(&func).primary_name_sets();

    assert!(!primary.int.contains("i"));
    assert!(!primary.int.contains("i_cur"));
    assert!(!primary.int.contains("i_next"));
}

#[test]
fn bool_primary_projection_is_tir_value_owned() {
    let func = function(
        "bool_primary_projection",
        &[],
        None,
        vec![
            const_int("lhs", 1),
            const_int("rhs", 2),
            const_bool("flag", true),
            op("copy_var", Some("flag_copy"), Some("flag"), &[]),
            op("eq", Some("cmp"), None, &["lhs", "rhs"]),
            op("not", Some("negated"), None, &["cmp"]),
            op(
                "checked_add",
                Some("add_overflow"),
                Some("sum"),
                &["lhs", "rhs"],
            ),
            op(
                "checked_mul",
                Some("mul_overflow"),
                Some("product"),
                &["lhs", "rhs"],
            ),
            op("and", Some("both"), None, &["flag_copy", "add_overflow"]),
            op("or", Some("either"), None, &["both", "mul_overflow"]),
            op("copy", Some("either_copy"), None, &["either"]),
            op("is_truthy", Some("legacy_truthy"), None, &["flag"]),
        ],
    );

    let primary = native_representation_plan(&func).primary_name_sets();

    for name in [
        "flag",
        "flag_copy",
        "cmp",
        "negated",
        "add_overflow",
        "mul_overflow",
        "both",
        "either",
        "either_copy",
    ] {
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
    for name in ["sum", "product"] {
        assert!(
            primary.int.contains(name),
            "checked result zero {name} is an integer"
        );
        assert!(
            !primary.bool_.contains(name),
            "status type must not leak into {name}"
        );
    }
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

    let plan = native_representation_plan(&func);

    assert_eq!(plan.op_scalar_lane(&pow), None);
}

#[test]
fn transport_hints_do_not_prove_scalar_representation() {
    let mut add = op("add", Some("sum"), None, &["lhs", "rhs"]);
    add.fast_int = Some(true);
    add.fast_float = Some(true);
    add.type_hint = Some("int".to_string());
    let func = function("hinted_add", &["lhs", "rhs"], None, vec![add.clone()]);

    let plan = native_representation_plan(&func);

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

    let plan = native_representation_plan(&func);

    assert!(plan.op_prefers_integer_runtime_lane(&add));
    assert!(plan.op_prefers_integer_runtime_lane(&mul));
    assert!(plan.op_args_are_integer_family(&add));
    assert!(plan.op_args_are_integer_family(&mul));
    assert_eq!(plan.op_direct_numeric_repr(0, &add), None);
    assert_eq!(plan.op_direct_numeric_repr(1, &mul), None);
}

#[test]
fn const_numeric_ops_prove_direct_numeric_result_lanes() {
    let const_lhs = const_int("lhs", 2);
    let const_rhs = const_int("rhs", 3);
    let add = op("add", Some("sum"), None, &["lhs", "rhs"]);
    let f_lhs = const_float("f_lhs", 1.25);
    let f_rhs = const_float("f_rhs", 2.5);
    let f_add = op("add", Some("f_sum"), None, &["f_lhs", "f_rhs"]);
    let func = function(
        "const_numeric_ops",
        &[],
        None,
        vec![
            const_lhs,
            const_rhs,
            add.clone(),
            f_lhs,
            f_rhs,
            f_add.clone(),
        ],
    );

    let plan = native_representation_plan(&func);

    assert_eq!(plan.op_direct_numeric_repr(2, &add), Some(Repr::RawI64Safe));
    assert_eq!(
        plan.op_direct_numeric_repr(5, &f_add),
        Some(Repr::FloatUnboxed)
    );
}

#[test]
fn direct_numeric_repr_uses_current_producer_not_durable_source_index() {
    let mut safe_add = op("add", Some("safe_sum"), None, &["lhs", "rhs"]);
    safe_add.source_op_idx = Some(5);
    let unproven_add = op(
        "add",
        Some("unproven_sum"),
        None,
        &["unknown_lhs", "unknown_rhs"],
    );
    let func = function(
        "relifted_numeric_producers",
        &["unknown_lhs", "unknown_rhs"],
        None,
        vec![
            const_int("lhs", 2),
            const_int("rhs", 3),
            safe_add.clone(),
            const_bool("padding_a", true),
            const_bool("padding_b", false),
            unproven_add.clone(),
        ],
    );

    let plan = native_representation_plan(&func);

    assert_eq!(
        plan.op_direct_numeric_repr(2, &safe_add),
        Some(Repr::RawI64Safe)
    );
    assert_eq!(plan.op_direct_numeric_repr(5, &unproven_add), None);
}

#[test]
fn duplicate_output_producers_block_direct_numeric_projection() {
    let add = op("add", Some("rebound"), None, &["lhs", "rhs"]);
    let func = function(
        "ambiguous_numeric_producer",
        &["fallback"],
        None,
        vec![
            const_int("lhs", 2),
            const_int("rhs", 3),
            add.clone(),
            op("copy", Some("rebound"), None, &["fallback"]),
        ],
    );

    let plan = native_representation_plan(&func);

    assert_eq!(plan.op_direct_numeric_repr(2, &add), None);
}

#[test]
fn returned_counted_loop_retains_direct_add_op_repr() {
    let add = op("add", Some("i_next"), None, &["i_cur", "one"]);
    let func = function(
        "returned_counted_store_load_loop",
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
            add.clone(),
            op("store_var", None, Some("i"), &["i_next"]),
            op("loop_continue", None, None, &[]),
            op("loop_end", None, None, &[]),
            op("load_var", Some("i_after"), Some("i"), &[]),
            op("ret", None, Some("i_after"), &["i_after"]),
        ],
    );

    let plan = native_representation_plan(&func);

    assert_eq!(plan.op_direct_numeric_repr(8, &add), Some(Repr::RawI64Safe));
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

    let plan = native_representation_plan(&func);

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

    let plan = native_representation_plan(&func);

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

    let plan = native_representation_plan(&func);

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

    let plan = native_representation_plan(&func);
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

    let plan = native_representation_plan(&func);
    let primary = plan.primary_name_sets();

    assert!(primary.float.contains("base"));
    assert!(primary.float.contains("exp"));
    assert!(primary.float.contains("sum"));
    assert!(primary.float.contains("sum_copy"));
    assert!(!primary.float.contains("pow_result"));
    assert!(!primary.float.contains("p"));
    assert!(
        !primary.float.contains("param_copy"),
        "copying an annotated parameter cannot create exact float provenance"
    );
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

    let primary = native_representation_plan(&func).primary_name_sets();

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

    let primary = native_representation_plan(&func).primary_name_sets();

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

    let plan = native_representation_plan(&func);
    let primary = plan.primary_name_sets();

    assert!(primary.int.is_empty());
    assert!(primary.bool_.is_empty());
    assert!(primary.float.is_empty());
    assert_eq!(plan.name_scalar_kind("value"), None);
    assert_eq!(plan.name_scalar_kind("flag"), None);
    assert_eq!(plan.name_container_kind("items"), None);
}

#[test]
fn annotation_only_comparisons_do_not_mint_raw_bool_carriers() {
    for comparison in ["eq", "ne", "lt", "le", "gt", "ge"] {
        let func = function(
            "annotation_comparison",
            &["lhs", "rhs"],
            Some(vec!["int", "int"]),
            vec![
                op(comparison, Some("cmp"), None, &["lhs", "rhs"]),
                op("not", Some("negated"), None, &["cmp"]),
            ],
        );
        let primary = native_representation_plan(&func).primary_name_sets();
        assert!(
            !primary.bool_.contains("cmp"),
            "{comparison} can return an arbitrary object"
        );
        assert!(
            primary.bool_.contains("negated"),
            "truth conversion has an actual Bool result"
        );
    }
}

#[test]
fn annotation_only_scalar_aliases_remain_boxed_beside_exact_producers() {
    for (annotation, seed) in [
        ("bool", const_bool("seed", true)),
        ("float", const_float("seed", 1.25)),
        ("int", const_int("seed", 7)),
    ] {
        let func = function(
            "annotation_aliases",
            &["parameter"],
            Some(vec![annotation]),
            vec![
                seed,
                op("copy", Some("exact_copy"), None, &["seed"]),
                op("store_var", None, Some("exact_slot"), &["exact_copy"]),
                op("load_var", Some("exact_load"), Some("exact_slot"), &[]),
                op("copy", Some("parameter_copy"), None, &["parameter"]),
                op(
                    "copy_var",
                    Some("parameter_alias"),
                    Some("parameter_copy"),
                    &[],
                ),
                op(
                    "store_var",
                    None,
                    Some("parameter_slot"),
                    &["parameter_alias"],
                ),
                op(
                    "load_var",
                    Some("parameter_load"),
                    Some("parameter_slot"),
                    &[],
                ),
            ],
        );
        let primary = native_representation_plan(&func).primary_name_sets();
        for name in [
            "parameter",
            "parameter_copy",
            "parameter_alias",
            "parameter_slot",
            "parameter_load",
        ] {
            assert!(
                !primary.int.contains(name)
                    && !primary.bool_.contains(name)
                    && !primary.float.contains(name),
                "{annotation} annotation must not become a raw carrier through {name}"
            );
        }
        let exact_names = match annotation {
            "bool" => &primary.bool_,
            "float" => &primary.float,
            "int" => &primary.int,
            _ => unreachable!(),
        };
        for name in ["seed", "exact_copy", "exact_slot", "exact_load"] {
            assert!(
                exact_names.contains(name),
                "exact {annotation} producer must retain its carrier through {name}"
            );
        }
    }
}
