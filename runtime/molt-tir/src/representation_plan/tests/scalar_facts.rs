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
        kind: molt_ir::OpKind::Const,
        out: Some(out.to_string()),
        value: Some(value),
        ..OpIR::new(molt_ir::OpKind::Const)
    }
}

fn const_bool(out: &str, value: bool) -> OpIR {
    OpIR {
        kind: molt_ir::OpKind::ConstBool,
        out: Some(out.to_string()),
        value: Some(i64::from(value)),
        ..OpIR::new(molt_ir::OpKind::ConstBool)
    }
}

fn const_float(out: &str, value: f64) -> OpIR {
    OpIR {
        kind: molt_ir::OpKind::ConstFloat,
        out: Some(out.to_string()),
        f_value: Some(value),
        ..OpIR::new(molt_ir::OpKind::ConstFloat)
    }
}

#[test]
fn canonical_block_arg_names_have_bounded_table_comparisons() {
    use std::cell::Cell;
    use std::hash::{Hash, Hasher};

    use crate::tir::blocks::BlockId;

    thread_local! {
        static COMPARISONS: Cell<usize> = const { Cell::new(0) };
    }

    struct CountedName {
        name: String,
    }

    impl Hash for CountedName {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.name.hash(state);
        }
    }

    impl PartialEq for CountedName {
        fn eq(&self, other: &Self) -> bool {
            COMPARISONS.with(|count| count.set(count.get() + 1));
            self.name == other.name
        }
    }

    impl Eq for CountedName {}

    const COUNT: usize = 4096;
    COMPARISONS.set(0);
    let plan = ScalarRepresentationPlan::default();
    // Exercise the actual production name-table hash policy, counting key
    // comparisons instead of relying on clocks or a particular hash output.
    let mut names = HashSet::with_capacity_and_hasher(
        COUNT,
        plan.container_storage_conflicted_names.hasher().clone(),
    );
    let name = |block| CountedName {
        name: SimpleValueNames::canonical_block_arg_slot(BlockId(block), 0),
    };
    for block in 1000..1000 + COUNT as u32 {
        assert!(names.insert(name(block)));
    }
    for block in 1000..1000 + COUNT as u32 {
        assert!(names.contains(&name(block)));
    }
    assert_eq!(names.len(), COUNT);
    let comparisons = COMPARISONS.get();
    assert!(
        comparisons < 8 * COUNT,
        "canonical names caused {} key comparisons for {COUNT} inserts and lookups",
        comparisons,
    );
}

#[test]
fn fresh_hash_seeds_preserve_plan_facts_and_consumer_views() {
    let func = function(
        "hash_seed_invariance",
        &[],
        None,
        vec![
            const_int("_bb1000_arg0", 3),
            const_int("_bb1001_arg0", 7),
            const_int("index", 0),
            const_bool("flag", true),
            const_float("float", 1.25),
            op(
                molt_ir::OpKind::ListIntNew,
                Some("items"),
                None,
                &["_bb1000_arg0", "_bb1001_arg0"],
            ),
            op(molt_ir::OpKind::Copy, Some("alias"), None, &["items"]),
            op(
                molt_ir::OpKind::Index,
                Some("read"),
                None,
                &["alias", "index"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("saved"),
                &["_bb1001_arg0"],
            ),
            op(
                molt_ir::OpKind::LoadVar,
                Some("restored"),
                Some("saved"),
                &[],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("mixed"),
                &["_bb1001_arg0"],
            ),
            op(molt_ir::OpKind::StoreVar, None, Some("mixed"), &["flag"]),
        ],
    );
    let list_read = op(molt_ir::OpKind::Index, None, None, &["alias", "index"]);
    for target in [
        crate::tir::TargetInfo::native_release_fast(),
        crate::tir::TargetInfo::wasm_release_fast(),
        crate::tir::TargetInfo::llvm_release_fast(),
        crate::tir::TargetInfo::luau_release_fast(),
        crate::tir::TargetInfo::rust_release_fast(),
        crate::tir::TargetInfo::mlir_release_fast(),
    ] {
        let baseline = ScalarRepresentationPlan::for_function_ir_for_target(&func, &target);
        assert_eq!(baseline.name_scalar_kind("restored"), Some(ScalarKind::Int));
        assert_eq!(baseline.name_scalar_kind("mixed"), None);
        assert_eq!(
            baseline.name_container_storage_kind("alias"),
            Some(ContainerStorageKind::FlatListInt),
        );
        assert!(baseline.op_has_exact_builtin_list(&list_read));
        for _ in 0..2 {
            let fresh = ScalarRepresentationPlan::for_function_ir_for_target(&func, &target);
            assert_eq!(fresh, baseline, "{}", target.target.as_str());
            assert_eq!(fresh.scalar_name_sets(), baseline.scalar_name_sets());
            assert_eq!(fresh.primary_name_sets(), baseline.primary_name_sets());
            assert_eq!(
                fresh.scalar_store_targets(ScalarKind::Int),
                baseline.scalar_store_targets(ScalarKind::Int)
            );
            assert_eq!(
                fresh.typed_slot_store_modes(),
                baseline.typed_slot_store_modes()
            );
            assert_eq!(
                fresh.op_has_exact_builtin_list(&list_read),
                baseline.op_has_exact_builtin_list(&list_read)
            );
        }
    }
}

#[test]
fn binding_snapshots_keep_incoming_facts_when_storage_is_rebound() {
    for kind in [molt_ir::OpKind::StoreVar, molt_ir::OpKind::StoreFast] {
        let func = function(
            "binding_snapshot_facts",
            &[],
            None,
            vec![
                const_int("lhs", 1_i64 << 31),
                const_int("rhs", 1_i64 << 31),
                op(
                    molt_ir::OpKind::CheckedMul,
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
                op(
                    molt_ir::OpKind::ListIntNew,
                    Some("items"),
                    None,
                    &["lhs", "rhs"],
                ),
                op(kind, Some("items_snapshot"), Some("items_slot"), &["items"]),
                op(molt_ir::OpKind::Missing, Some("missing_value"), None, &[]),
                op(
                    molt_ir::OpKind::DeleteVar,
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
            kind == molt_ir::OpKind::StoreVar,
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
        molt_ir::OpKind::Copy,
        molt_ir::OpKind::CopyVar,
        molt_ir::OpKind::LoadVar,
        molt_ir::OpKind::IdentityAlias,
        molt_ir::OpKind::BindingAlias,
    ] {
        let func = function(
            "alias_source_precedence",
            &[],
            None,
            vec![
                const_int("integer", 7),
                const_float("float_metadata", 1.25),
                op(kind, Some("snapshot"), Some("float_metadata"), &["integer"]),
                op(molt_ir::OpKind::StoreVar, None, Some("slot"), &["snapshot"]),
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
    for constant_kind in [
        molt_ir::OpKind::Const,
        molt_ir::OpKind::ConstInt,
        molt_ir::OpKind::LoadConst,
    ] {
        for checked_kind in [molt_ir::OpKind::CheckedAdd, molt_ir::OpKind::CheckedMul] {
            assert!(matches!(
                kind_to_opcode_table(checked_kind),
                Some(OpCode::CheckedAdd | OpCode::CheckedMul)
            ));
            let mut seed = const_int("seed", 1_i64 << 31);
            seed.kind = constant_kind;
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
                        molt_ir::OpKind::StoreVar,
                        Some("snapshot"),
                        Some("value_slot"),
                        &["value"],
                    ),
                    op(
                        molt_ir::OpKind::StoreVar,
                        None,
                        Some("snapshot_slot"),
                        &["snapshot"],
                    ),
                    op(
                        molt_ir::OpKind::StoreVar,
                        None,
                        Some("flag_slot"),
                        &["overflow"],
                    ),
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
            op(molt_ir::OpKind::CopyVar, Some("snapshot"), None, &["none"]),
            op(molt_ir::OpKind::Ret, None, None, &["snapshot"]),
        ],
        vec![
            op(
                molt_ir::OpKind::LoadVar,
                Some("snapshot"),
                Some("none"),
                &[],
            ),
            op(molt_ir::OpKind::Ret, None, None, &["snapshot"]),
        ],
        vec![
            op(
                molt_ir::OpKind::StoreVar,
                Some("snapshot"),
                Some("local"),
                &["none"],
            ),
            op(molt_ir::OpKind::Ret, None, None, &["snapshot"]),
        ],
        vec![op(molt_ir::OpKind::Ret, None, None, &["none"])],
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
        vec![op(molt_ir::OpKind::Ret, None, None, &["xs"])],
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
    let mut index = op(molt_ir::OpKind::Index, Some("item"), None, &["xs", "i"]);
    index.container_type = Some("list".to_string());
    index.type_hint = Some("list".to_string());
    let func = function("transport_only", &["xs", "i"], None, vec![index]);
    let plan = native_representation_plan(&func);

    assert_eq!(plan.name_container_kind("xs"), None);
    assert_eq!(plan.name_container_kind("item"), None);
}

#[test]
fn exact_builtin_list_class_requires_allocator_provenance() {
    let index = op(molt_ir::OpKind::Index, Some("item"), None, &["xs", "i"]);
    let annotated = function(
        "annotated_list_parameter",
        &["xs", "i"],
        Some(vec!["list", "int"]),
        vec![index.clone()],
    );
    let plan = native_representation_plan(&annotated);
    assert_eq!(plan.name_container_kind("xs"), Some(ContainerKind::List));
    assert!(!plan.op_has_exact_builtin_list(&index));

    for kind in [
        molt_ir::OpKind::Copy,
        molt_ir::OpKind::IdentityAlias,
        molt_ir::OpKind::BindingAlias,
        molt_ir::OpKind::GuardTag,
        molt_ir::OpKind::GuardType,
    ] {
        let alias_index = op(molt_ir::OpKind::Index, Some("item"), None, &["alias", "i"]);
        let inputs: &[&str] =
            if matches!(kind, molt_ir::OpKind::GuardTag | molt_ir::OpKind::GuardType) {
                &["xs", "expected_tag"]
            } else {
                &["xs"]
            };
        let constructed = function(
            "exact_list_alias",
            &[],
            None,
            vec![
                const_int("i", 0),
                const_int("value", 1),
                const_int("expected_tag", 8),
                op(molt_ir::OpKind::ListNew, Some("xs"), None, &["value"]),
                op(kind, Some("alias"), None, inputs),
                alias_index.clone(),
            ],
        );
        let plan = native_representation_plan(&constructed);
        assert!(plan.op_has_exact_builtin_list(&alias_index), "{kind}");

        let mut ambiguous = constructed.clone();
        ambiguous.params.push("unknown".to_string());
        ambiguous.param_types = None;
        ambiguous.ops.insert(
            5,
            op(molt_ir::OpKind::Copy, Some("alias"), None, &["unknown"]),
        );
        let plan = native_representation_plan(&ambiguous);
        assert!(!plan.op_has_exact_builtin_list(&alias_index), "{kind}");
    }
}

#[test]
fn retained_container_alias_keeps_class_but_escape_revokes_storage() {
    let alias = op(
        molt_ir::OpKind::BindingAlias,
        Some("captured"),
        None,
        &["xs"],
    );
    let index = op(molt_ir::OpKind::Index, Some("item"), None, &["xs", "i"]);
    let captured_index = op(
        molt_ir::OpKind::Index,
        Some("captured_item"),
        None,
        &["captured", "i"],
    );
    let mut func = function(
        "retained_list_storage",
        &[],
        None,
        vec![
            const_int("count", 4),
            const_int("fill", 0),
            const_int("i", 0),
            op(
                molt_ir::OpKind::ListIntNew,
                Some("xs"),
                None,
                &["count", "fill"],
            ),
            alias,
            index.clone(),
            captured_index.clone(),
        ],
    );
    let plan = native_representation_plan(&func);
    for op in [&index, &captured_index] {
        assert!(plan.op_has_exact_builtin_list(op));
    }
    for name in ["xs", "captured"] {
        assert_eq!(
            plan.name_container_storage_kind(name),
            Some(ContainerStorageKind::FlatListInt)
        );
    }

    let mut escape = op(
        molt_ir::OpKind::Call,
        Some("call_result"),
        None,
        &["captured"],
    );
    escape.s_value = Some("opaque_consumer".to_string());
    func.ops.insert(5, escape);
    let plan = native_representation_plan(&func);
    for op in [&index, &captured_index] {
        assert!(plan.op_has_exact_builtin_list(op));
    }
    for name in ["xs", "captured"] {
        assert_eq!(plan.name_container_storage_kind(name), None);
    }
}

#[test]
fn flat_list_storage_requires_structural_producer() {
    let mut index = op(molt_ir::OpKind::Index, Some("item"), None, &["xs", "i"]);
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
    let list_new = op(
        molt_ir::OpKind::ListIntNew,
        Some("xs"),
        None,
        &["storage_count", "storage_fill"],
    );
    let copy = op(molt_ir::OpKind::Copy, Some("ys"), None, &["xs"]);
    let store = op(molt_ir::OpKind::StoreVar, None, Some("slot"), &["ys"]);
    let load = op(molt_ir::OpKind::LoadVar, Some("zs"), Some("slot"), &[]);
    let index = op(molt_ir::OpKind::Index, Some("item"), None, &["zs", "i"]);
    let func = function(
        "storage_aliases",
        &[],
        None,
        vec![
            const_int("storage_count", 4),
            const_int("storage_fill", 0),
            const_int("i", 0),
            list_new,
            copy,
            store,
            load,
            index.clone(),
        ],
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
    assert!(plan.op_has_container_storage(7, &index, ContainerStorageKind::FlatListInt));
}

#[test]
fn non_int_store_index_conflicts_flat_list_storage() {
    let list_new = op(
        molt_ir::OpKind::ListIntNew,
        Some("xs"),
        None,
        &["storage_count", "storage_fill"],
    );
    let idx = const_int("i", 0);
    let value = const_float("f", 1.25);
    let store = op(
        molt_ir::OpKind::StoreIndex,
        Some("ys"),
        None,
        &["xs", "i", "f"],
    );
    let index = op(molt_ir::OpKind::Index, Some("item"), None, &["ys", "i"]);
    let func = function(
        "flat_storage_non_int_write",
        &[],
        None,
        vec![
            const_int("storage_count", 4),
            const_int("storage_fill", 0),
            list_new,
            idx,
            value,
            store.clone(),
            index.clone(),
        ],
    );
    let plan = native_representation_plan(&func);

    assert_eq!(plan.name_container_storage_kind("xs"), None);
    assert_eq!(plan.name_container_storage_kind("ys"), None);
    assert!(!plan.op_has_container_storage(5, &store, ContainerStorageKind::FlatListInt));
    assert!(!plan.op_has_container_storage(6, &index, ContainerStorageKind::FlatListInt));
}

#[test]
fn semantic_list_bool_index_does_not_authorize_raw_bool_primary() {
    let index = op(
        molt_ir::OpKind::Index,
        Some("item"),
        None,
        &["items", "idx"],
    );
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
    let index = op(
        molt_ir::OpKind::Index,
        Some("item"),
        None,
        &["items", "idx"],
    );
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
    let list_new = op(
        molt_ir::OpKind::ListIntNew,
        Some("items"),
        None,
        &["storage_count", "storage_fill"],
    );
    let index = const_int("idx", 0);
    let value = const_int("value", 7);
    let store_index = op(
        molt_ir::OpKind::StoreIndex,
        Some("after_store"),
        None,
        &["items", "idx", "value"],
    );
    let dict_set = op(
        molt_ir::OpKind::DictSet,
        Some("after_dict_set"),
        None,
        &["items", "idx", "value"],
    );
    let func = function(
        "list_write_storage_authority",
        &[],
        None,
        vec![
            const_int("storage_count", 4),
            const_int("storage_fill", 0),
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
    assert!(!plan.op_has_container_storage(5, &store_index, ContainerStorageKind::FlatListInt,));
    assert!(!plan.op_has_container_storage(6, &dict_set, ContainerStorageKind::FlatListInt,));
}

#[test]
fn ord_at_result_is_integer_family_from_tir_not_transport_hints() {
    let mut ord_at = op(molt_ir::OpKind::OrdAt, Some("code"), None, &["text", "idx"]);
    ord_at.type_hint = Some("list".to_string());
    ord_at.container_type = Some("list".to_string());
    let add = op(
        molt_ir::OpKind::Add,
        Some("shifted"),
        None,
        &["code", "bias"],
    );
    let func = function(
        "ord_at_representation",
        &[],
        None,
        vec![
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                out: Some("text".to_string()),
                s_value: Some("AéZ".to_string()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
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
        molt_ir::OpKind::Index,
        Some("object_type_tag"),
        None,
        &["__molt_split_frame", "__molt_split_frame_index"],
    );
    let func = function(
        "split_frame_index",
        &[],
        None,
        vec![
            op(
                molt_ir::OpKind::ListNew,
                Some("__molt_split_frame"),
                None,
                &[],
            ),
            const_int("__molt_split_frame_index", 0),
            index.clone(),
            op(
                molt_ir::OpKind::BuiltinType,
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
            op(molt_ir::OpKind::ConstNone, Some("none_value"), None, &[]),
            const_int("one", 1),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb2_arg0"),
                &["zero"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb2_arg0"),
                &["none_value"],
            ),
            op(
                molt_ir::OpKind::LoadVar,
                Some("_v19"),
                Some("_bb2_arg0"),
                &[],
            ),
            op(molt_ir::OpKind::Add, Some("next"), None, &["one", "one"]),
            op(molt_ir::OpKind::StoreVar, None, Some("_v19"), &["next"]),
            op(molt_ir::OpKind::CopyVar, Some("after"), None, &["_v19"]),
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
            op(molt_ir::OpKind::CopyVar, Some("slot"), None, &["one"]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("slot"),
                &["unproven_source"],
            ),
            op(molt_ir::OpKind::CopyVar, Some("after"), None, &["slot"]),
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
            op(molt_ir::OpKind::CopyVar, Some("slot"), None, &["one"]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("slot"),
                &["unproven_source"],
            ),
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
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("loop_slot"),
                &["unproven_source"],
            ),
            op(molt_ir::OpKind::LoadVar, Some("iv"), Some("loop_slot"), &[]),
            op(molt_ir::OpKind::StoreVar, None, Some("iv"), &["one"]),
            op(molt_ir::OpKind::CopyVar, Some("after"), None, &["iv"]),
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
fn iter_next_done_flag_uses_fused_bool_fact() {
    let done_index = const_int("done_index", 1);
    let done = op(
        molt_ir::OpKind::Index,
        Some("done_flag"),
        None,
        &["pair", "done_index"],
    );
    let value_index = const_int("value_index", 0);
    let value = op(
        molt_ir::OpKind::Index,
        Some("next_value"),
        None,
        &["pair", "value_index"],
    );
    let func = function(
        "iter_next_done_flag",
        &["items"],
        None,
        vec![
            op(molt_ir::OpKind::Iter, Some("iter_obj"), None, &["items"]),
            op(molt_ir::OpKind::LoopStart, None, None, &[]),
            op(molt_ir::OpKind::IterNext, Some("pair"), None, &["iter_obj"]),
            done_index,
            done.clone(),
            op(molt_ir::OpKind::LoopBreakIfTrue, None, None, &["done_flag"]),
            value_index,
            value,
            op(
                molt_ir::OpKind::ModuleCacheSet,
                None,
                None,
                &["next_value", "items"],
            ),
            op(molt_ir::OpKind::LoopContinue, None, None, &[]),
            op(molt_ir::OpKind::LoopEnd, None, None, &[]),
            op(molt_ir::OpKind::RetVoid, None, None, &[]),
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
        "the index result cannot override the fused done flag's bool type"
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
                    op(molt_ir::OpKind::Ret, None, None, &["value"])
                } else {
                    op(molt_ir::OpKind::RetVoid, None, None, &[])
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
            op(molt_ir::OpKind::ListNew, Some("value"), None, &[]),
            op(molt_ir::OpKind::Ret, None, None, &["value"]),
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
        .find(|op| op.kind == molt_ir::OpKind::ListNew)
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
        vec![op(molt_ir::OpKind::Ret, None, Some("x"), &[])],
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
            op(molt_ir::OpKind::StoreVar, None, Some("slot"), &["i"]),
            op(molt_ir::OpKind::StoreVar, None, Some("slot"), &["b"]),
            op(molt_ir::OpKind::Ret, None, Some("slot"), &[]),
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
            op(molt_ir::OpKind::StoreVar, None, Some("slot"), &["i"]),
            op(molt_ir::OpKind::Ret, None, Some("slot"), &[]),
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
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("slot"),
                &["unknown_source"],
            ),
            op(molt_ir::OpKind::Copy, Some("slot"), None, &["seed"]),
            op(molt_ir::OpKind::LoadVar, Some("loaded"), Some("slot"), &[]),
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
    let mut generic = op(molt_ir::OpKind::Call, Some("maybe_int"), None, &[]);
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
            op(
                molt_ir::OpKind::Mul,
                Some("wide"),
                None,
                &["seed", "factor"],
            ),
            const_int("mask", 7),
            op(
                molt_ir::OpKind::BitOr,
                Some("masked"),
                None,
                &["wide", "mask"],
            ),
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
            op(
                molt_ir::OpKind::BitXor,
                Some("masked"),
                None,
                &["lhs", "rhs"],
            ),
            op(molt_ir::OpKind::Add, Some("sum"), None, &["lhs", "rhs"]),
            op(
                molt_ir::OpKind::Lshift,
                Some("shifted"),
                None,
                &["lhs", "rhs"],
            ),
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
        vec![op(molt_ir::OpKind::Add, Some("sum"), None, &["lhs", "rhs"])],
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
            op(molt_ir::OpKind::Add, Some("sum"), None, &["lhs", "rhs"]),
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
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["init"]),
            op(molt_ir::OpKind::LoopStart, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("i_cur"), Some("i"), &[]),
            op(
                molt_ir::OpKind::Lt,
                Some("keep_going"),
                None,
                &["i_cur", "stop"],
            ),
            op(
                molt_ir::OpKind::LoopBreakIfFalse,
                None,
                None,
                &["keep_going"],
            ),
            op(
                molt_ir::OpKind::Add,
                Some("i_next"),
                None,
                &["i_cur", "one"],
            ),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["i_next"]),
            op(molt_ir::OpKind::LoopContinue, None, None, &[]),
            op(molt_ir::OpKind::LoopEnd, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("i_after"), Some("i"), &[]),
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
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["init"]),
            op(molt_ir::OpKind::LoopStart, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("i_cur"), Some("i"), &[]),
            op(
                molt_ir::OpKind::Gt,
                Some("keep_going"),
                None,
                &["i_cur", "stop"],
            ),
            op(
                molt_ir::OpKind::LoopBreakIfFalse,
                None,
                None,
                &["keep_going"],
            ),
            op(
                molt_ir::OpKind::Add,
                Some("i_next"),
                None,
                &["i_cur", "one"],
            ),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["i_next"]),
            op(molt_ir::OpKind::LoopContinue, None, None, &[]),
            op(molt_ir::OpKind::LoopEnd, None, None, &[]),
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
            op(
                molt_ir::OpKind::CopyVar,
                Some("flag_copy"),
                Some("flag"),
                &[],
            ),
            op(molt_ir::OpKind::Eq, Some("cmp"), None, &["lhs", "rhs"]),
            op(molt_ir::OpKind::Not, Some("negated"), None, &["cmp"]),
            op(
                molt_ir::OpKind::CheckedAdd,
                Some("add_overflow"),
                Some("sum"),
                &["lhs", "rhs"],
            ),
            op(
                molt_ir::OpKind::CheckedMul,
                Some("mul_overflow"),
                Some("product"),
                &["lhs", "rhs"],
            ),
            op(
                molt_ir::OpKind::And,
                Some("both"),
                None,
                &["flag_copy", "add_overflow"],
            ),
            op(
                molt_ir::OpKind::Or,
                Some("either"),
                None,
                &["both", "mul_overflow"],
            ),
            op(
                molt_ir::OpKind::Copy,
                Some("either_copy"),
                None,
                &["either"],
            ),
            op(
                molt_ir::OpKind::Index,
                Some("opaque_result"),
                None,
                &["flag", "lhs"],
            ),
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
        !primary.bool_.contains("opaque_result"),
        "an opaque result must not mint a raw bool carrier"
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
    let pow = op(molt_ir::OpKind::Pow, Some("powv"), None, &["base", "exp"]);
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
    let mut add = op(molt_ir::OpKind::Add, Some("sum"), None, &["lhs", "rhs"]);
    add.type_hint = Some("int".to_string());
    let func = function("hinted_add", &["lhs", "rhs"], None, vec![add.clone()]);

    let plan = native_representation_plan(&func);

    assert_eq!(plan.op_scalar_lane(&add), None);
    assert!(!plan.op_prefers_integer_runtime_lane(&add));
    assert!(!plan.op_args_are_integer_family(&add));
}

#[test]
fn typed_operands_prove_integer_runtime_lane_without_transport_hints() {
    let add = op(molt_ir::OpKind::Add, Some("sum"), None, &["lhs", "rhs"]);
    let mul = op(molt_ir::OpKind::Mul, Some("product"), None, &["lhs", "rhs"]);
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
    let add = op(molt_ir::OpKind::Add, Some("sum"), None, &["lhs", "rhs"]);
    let f_lhs = const_float("f_lhs", 1.25);
    let f_rhs = const_float("f_rhs", 2.5);
    let f_add = op(
        molt_ir::OpKind::Add,
        Some("f_sum"),
        None,
        &["f_lhs", "f_rhs"],
    );
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
    let mut safe_add = op(
        molt_ir::OpKind::Add,
        Some("safe_sum"),
        None,
        &["lhs", "rhs"],
    );
    safe_add.source_op_idx = Some(5);
    let unproven_add = op(
        molt_ir::OpKind::Add,
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
    let add = op(molt_ir::OpKind::Add, Some("rebound"), None, &["lhs", "rhs"]);
    let func = function(
        "ambiguous_numeric_producer",
        &["fallback"],
        None,
        vec![
            const_int("lhs", 2),
            const_int("rhs", 3),
            add.clone(),
            op(molt_ir::OpKind::Copy, Some("rebound"), None, &["fallback"]),
        ],
    );

    let plan = native_representation_plan(&func);

    assert_eq!(plan.op_direct_numeric_repr(2, &add), None);
}

#[test]
fn returned_counted_loop_retains_direct_add_op_repr() {
    let add = op(
        molt_ir::OpKind::Add,
        Some("i_next"),
        None,
        &["i_cur", "one"],
    );
    let mut func = function(
        "returned_counted_store_load_loop",
        &[],
        None,
        vec![
            const_int("init", 0),
            const_int("one", 1),
            const_int("stop", 1_000_000),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["init"]),
            op(molt_ir::OpKind::LoopStart, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("i_cur"), Some("i"), &[]),
            op(
                molt_ir::OpKind::Lt,
                Some("keep_going"),
                None,
                &["i_cur", "stop"],
            ),
            op(
                molt_ir::OpKind::LoopBreakIfFalse,
                None,
                None,
                &["keep_going"],
            ),
            add.clone(),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["i_next"]),
            op(molt_ir::OpKind::LoopContinue, None, None, &[]),
            op(molt_ir::OpKind::LoopEnd, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("i_after"), Some("i"), &[]),
            op(molt_ir::OpKind::Ret, None, Some("i_after"), &["i_after"]),
        ],
    );

    for name in [
        "returned_counted_store_load_loop",
        "__molt_module_chunk_returned_counted_loop",
    ] {
        func.name = name.to_string();
        for target in [
            crate::tir::TargetInfo::native_release_fast(),
            crate::tir::TargetInfo::wasm_release_fast(),
        ] {
            let plan = ScalarRepresentationPlan::for_function_ir_for_target(&func, &target);
            assert_eq!(plan.op_direct_numeric_repr(8, &add), Some(Repr::RawI64Safe));
        }
    }
}

#[test]
fn counted_loop_transfer_boundaries_preserve_native_and_wasm_scalar_facts() {
    let add = op(
        molt_ir::OpKind::Add,
        Some("i_next"),
        None,
        &["i_view", "one"],
    );
    let func = function(
        "checked_counted_alias_loop",
        &[],
        None,
        vec![
            const_int("init", 0),
            const_int("one", 1),
            const_int("stop", 1_000_000),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["init"]),
            op(molt_ir::OpKind::LoopStart, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("i_cur"), Some("i"), &[]),
            op(molt_ir::OpKind::Copy, Some("i_view"), None, &["i_cur"]),
            op(
                molt_ir::OpKind::Lt,
                Some("keep_going"),
                None,
                &["i_view", "stop"],
            ),
            op(molt_ir::OpKind::CheckException, None, None, &[]),
            op(
                molt_ir::OpKind::LoopBreakIfFalse,
                None,
                None,
                &["keep_going"],
            ),
            op(molt_ir::OpKind::CheckException, None, None, &[]),
            add.clone(),
            op(molt_ir::OpKind::Copy, Some("next_view"), None, &["i_next"]),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["next_view"]),
            op(molt_ir::OpKind::CheckException, None, None, &[]),
            op(molt_ir::OpKind::LoopContinue, None, None, &[]),
            op(molt_ir::OpKind::LoopEnd, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("i_after"), Some("i"), &[]),
            op(molt_ir::OpKind::Ret, None, Some("i_after"), &["i_after"]),
        ],
    );
    for target in [
        crate::tir::TargetInfo::native_release_fast(),
        crate::tir::TargetInfo::wasm_release_fast(),
    ] {
        let plan = ScalarRepresentationPlan::for_function_ir_for_target(&func, &target);
        let primary = plan.primary_name_sets();
        for name in ["i", "i_cur", "i_view", "i_next", "next_view", "i_after"] {
            assert!(
                primary.int.contains(name),
                "missing bounded scalar fact for {name}"
            );
        }
        assert_eq!(
            plan.op_direct_numeric_repr(11, &add),
            Some(Repr::RawI64Safe)
        );
    }
}

#[test]
fn list_repeat_does_not_take_integer_runtime_lane() {
    let list_new = op(molt_ir::OpKind::ListNew, Some("items"), None, &["item"]);
    let repeat = op(
        molt_ir::OpKind::Mul,
        Some("repeated"),
        None,
        &["items", "count"],
    );
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
    let pow = op(molt_ir::OpKind::Pow, Some("powv"), None, &["base", "exp"]);
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
            op(molt_ir::OpKind::CopyVar, Some("i_copy"), None, &["i_seed"]),
            op(molt_ir::OpKind::StoreVar, None, Some("i_slot"), &["i_copy"]),
            const_float("f_seed", 1.25),
            op(
                molt_ir::OpKind::CopyVar,
                Some("f_copy"),
                Some("f_seed"),
                &[],
            ),
            op(molt_ir::OpKind::StoreVar, None, Some("f_slot"), &["f_copy"]),
            const_bool("b_seed", true),
            op(
                molt_ir::OpKind::IdentityAlias,
                Some("b_copy"),
                None,
                &["b_seed"],
            ),
            op(molt_ir::OpKind::StoreVar, None, Some("b_slot"), &["b_copy"]),
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                out: Some("s_seed".to_string()),
                s_value: Some("lane".to_string()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            op(molt_ir::OpKind::Copy, Some("s_copy"), None, &["s_seed"]),
            op(molt_ir::OpKind::StoreVar, None, Some("s_slot"), &["s_copy"]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("mixed_slot"),
                &["i_seed"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("mixed_slot"),
                &["f_seed"],
            ),
            op(
                molt_ir::OpKind::CallIndirect,
                Some("dynamic"),
                None,
                &["callable", "args"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("dynamic_slot"),
                &["dynamic"],
            ),
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
            op(molt_ir::OpKind::Missing, Some("missing_i"), None, &[]),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["missing_i"]),
            op(
                molt_ir::OpKind::CopyVar,
                Some("missing_copy"),
                None,
                &["missing_i"],
            ),
            const_int("stop", 3),
            const_int("zero", 0),
            const_int("one", 1),
            op(molt_ir::OpKind::CopyVar, Some("zero_copy"), None, &["zero"]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg0"),
                &["zero_copy"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg1"),
                &["missing_copy"],
            ),
            op(molt_ir::OpKind::LoopStart, None, None, &[]),
            op(molt_ir::OpKind::LoadVar, Some("iv"), Some("_bb1_arg0"), &[]),
            op(
                molt_ir::OpKind::LoadVar,
                Some("carried_obj"),
                Some("_bb1_arg1"),
                &[],
            ),
            op(molt_ir::OpKind::Lt, Some("cond"), None, &["iv", "stop"]),
            op(molt_ir::OpKind::LoopBreakIfFalse, None, None, &["cond"]),
            op(molt_ir::OpKind::StoreVar, None, Some("i"), &["iv"]),
            op(molt_ir::OpKind::CopyVar, Some("escaped_iv"), None, &["iv"]),
            op(molt_ir::OpKind::CheckException, None, None, &[]),
            op(molt_ir::OpKind::TypeOf, Some("ty"), None, &["escaped_iv"]),
            op(molt_ir::OpKind::CheckException, None, None, &[]),
            op(
                molt_ir::OpKind::StrFromObj,
                Some("text"),
                None,
                &["escaped_iv"],
            ),
            op(
                molt_ir::OpKind::ExceptionNewBuiltinOne,
                Some("exc"),
                None,
                &["escaped_iv"],
            ),
            op(molt_ir::OpKind::Add, Some("next"), None, &["iv", "one"]),
            op(molt_ir::OpKind::StoreVar, None, Some("iv"), &["next"]),
            op(molt_ir::OpKind::CopyVar, Some("next_copy"), None, &["next"]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg0"),
                &["next_copy"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("_bb1_arg1"),
                &["escaped_iv"],
            ),
            op(molt_ir::OpKind::LoopContinue, None, None, &[]),
            op(molt_ir::OpKind::LoopEnd, None, None, &[]),
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
            op(
                molt_ir::OpKind::Pow,
                Some("pow_result"),
                None,
                &["base", "exp"],
            ),
            op(molt_ir::OpKind::Add, Some("sum"), None, &["base", "exp"]),
            op(molt_ir::OpKind::CopyVar, Some("sum_copy"), Some("sum"), &[]),
            op(molt_ir::OpKind::CopyVar, Some("param_copy"), Some("p"), &[]),
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
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("float_slot"),
                &["f_seed"],
            ),
            const_int("i_seed", 2),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("mixed_slot"),
                &["f_seed"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("mixed_slot"),
                &["i_seed"],
            ),
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
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("int_slot"),
                &["i_seed"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("maybe_int_slot"),
                &["i_seed"],
            ),
            const_bool("b_seed", true),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("bool_slot"),
                &["b_seed"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("maybe_bool_slot"),
                &["b_seed"],
            ),
            const_float("f_seed", 1.5),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("float_slot"),
                &["f_seed"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("maybe_float_slot"),
                &["f_seed"],
            ),
            op(molt_ir::OpKind::Missing, Some("missing_value"), None, &[]),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("maybe_int_slot"),
                &["missing_value"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
                None,
                Some("maybe_bool_slot"),
                &["missing_value"],
            ),
            op(
                molt_ir::OpKind::StoreVar,
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
fn cold_module_chunk_functions_keep_semantic_facts_without_primary_carriers() {
    let func = function(
        "__molt_module_chunk_0",
        &[],
        None,
        vec![
            const_int("value", 1),
            const_bool("flag", true),
            op(molt_ir::OpKind::ListNew, Some("items"), None, &["value"]),
            op(molt_ir::OpKind::ConstNone, Some("nothing"), None, &[]),
        ],
    );

    for target in [
        crate::tir::TargetInfo::native_release_fast(),
        crate::tir::TargetInfo::wasm_release_fast(),
        crate::tir::TargetInfo::llvm_release_fast(),
        crate::tir::TargetInfo::luau_release_fast(),
        crate::tir::TargetInfo::rust_release_fast(),
        crate::tir::TargetInfo::mlir_release_fast(),
    ] {
        let plan = ScalarRepresentationPlan::for_function_ir_for_target(&func, &target);
        let primary = plan.primary_name_sets();

        assert!(primary.int.is_empty());
        assert!(primary.bool_.is_empty());
        assert!(primary.float.is_empty());
        assert_eq!(plan.name_scalar_kind("value"), Some(ScalarKind::Int));
        assert_eq!(plan.name_scalar_kind("flag"), Some(ScalarKind::Bool));
        assert_eq!(
            plan.name_scalar_kind("nothing"),
            Some(ScalarKind::NoneValue)
        );
        assert_eq!(plan.name_container_kind("items"), Some(ContainerKind::List));
    }
}

#[test]
fn cold_non_numeric_plans_match_the_former_eager_optimized_view() {
    let string = |name: &str, value: &str| OpIR {
        kind: molt_ir::OpKind::ConstStr,
        out: Some(name.to_string()),
        s_value: Some(value.to_string()),
        ..OpIR::new(molt_ir::OpKind::ConstStr)
    };
    let cases = [
        vec![
            string("key", "name"),
            string("value", "text"),
            op(
                molt_ir::OpKind::DictNew,
                Some("mapping"),
                None,
                &["key", "value"],
            ),
            op(molt_ir::OpKind::StoreVar, None, Some("saved"), &["mapping"]),
            op(
                molt_ir::OpKind::LoadVar,
                Some("restored"),
                Some("saved"),
                &[],
            ),
        ],
        vec![
            const_int("value", 7),
            const_bool("flag", true),
            op(molt_ir::OpKind::ListNew, Some("items"), None, &["value"]),
            op(molt_ir::OpKind::Copy, Some("alias"), None, &["items"]),
            op(molt_ir::OpKind::ConstNone, Some("nothing"), None, &[]),
        ],
        vec![
            const_int("n", 4),
            const_int("fill", 7),
            const_int("i", 0),
            op(
                molt_ir::OpKind::ListIntNew,
                Some("items"),
                None,
                &["n", "fill"],
            ),
            op(molt_ir::OpKind::Copy, Some("alias"), None, &["items"]),
            op(molt_ir::OpKind::Index, Some("read"), None, &["alias", "i"]),
        ],
    ];
    for target in [
        crate::tir::TargetInfo::native_release_fast(),
        crate::tir::TargetInfo::wasm_release_fast(),
        crate::tir::TargetInfo::llvm_release_fast(),
        crate::tir::TargetInfo::luau_release_fast(),
        crate::tir::TargetInfo::rust_release_fast(),
        crate::tir::TargetInfo::mlir_release_fast(),
    ] {
        for (case, ops) in cases.iter().enumerate() {
            let function = function(
                &format!("__molt_module_chunk_demand_{case}"),
                &[],
                None,
                ops.clone(),
            );
            let mut lifted = lower_to_tir_for_target(&function, &target);
            refine_types(&mut lifted);
            assert!(crate::tir::verify::verify_function(&lifted).is_ok());
            let plan = ScalarRepresentationPlan::for_function_ir_for_target(&function, &target);
            let eager = ScalarRepresentationPlan::for_function_ir_with_optimized_view::<true>(
                &function, &target,
            );
            assert_eq!(
                plan,
                eager,
                "case {case}, target {}",
                target.target.as_str()
            );
            assert!(plan.primary_name_sets().int.is_empty());
            assert!(plan.primary_name_sets().bool_.is_empty());
            assert!(plan.primary_name_sets().float.is_empty());
            if case == 0 {
                assert_eq!(plan.name_scalar_kind("key"), Some(ScalarKind::Str));
                assert_eq!(
                    plan.name_container_kind("restored"),
                    Some(ContainerKind::Dict)
                );
            } else {
                assert!(plan.op_has_exact_builtin_list(&op(
                    molt_ir::OpKind::Index,
                    None,
                    None,
                    &["alias", "i"],
                )));
                if case == 2 {
                    assert_eq!(
                        plan.name_container_storage_kind("alias"),
                        Some(ContainerStorageKind::FlatListInt),
                    );
                }
            }
        }
    }
}

#[test]
fn cold_numeric_results_retain_direct_carriers_without_primary_names() {
    let add = op(molt_ir::OpKind::Add, Some("sum"), None, &["lhs", "rhs"]);
    let float_add = op(
        molt_ir::OpKind::Add,
        Some("float_sum"),
        None,
        &["float_lhs", "float_rhs"],
    );
    let dynamic_add = op(
        molt_ir::OpKind::Add,
        Some("unknown_sum"),
        None,
        &["unknown", "rhs"],
    );
    let function = function(
        "__molt_module_chunk_numeric",
        &["unknown"],
        Some(vec!["int"]),
        vec![
            const_int("lhs", 2),
            const_int("rhs", 3),
            add.clone(),
            const_float("float_lhs", 1.25),
            const_float("float_rhs", 2.5),
            float_add.clone(),
            dynamic_add.clone(),
        ],
    );
    for target in [
        crate::tir::TargetInfo::native_release_fast(),
        crate::tir::TargetInfo::wasm_release_fast(),
    ] {
        let plan = ScalarRepresentationPlan::for_function_ir_for_target(&function, &target);
        assert_eq!(plan.op_direct_numeric_repr(2, &add), Some(Repr::RawI64Safe));
        assert_eq!(
            plan.op_direct_numeric_repr(5, &float_add),
            Some(Repr::FloatUnboxed),
        );
        assert_eq!(plan.op_direct_numeric_repr(6, &dynamic_add), None);
        assert!(plan.primary_name_sets().int.is_empty());
        assert!(plan.primary_name_sets().float.is_empty());
        assert!(!plan.is_raw_int_carrier_name("unknown"));
    }
}

#[test]
fn annotation_only_comparisons_do_not_mint_raw_bool_carriers() {
    for comparison in [
        molt_ir::OpKind::Eq,
        molt_ir::OpKind::Ne,
        molt_ir::OpKind::Lt,
        molt_ir::OpKind::Le,
        molt_ir::OpKind::Gt,
        molt_ir::OpKind::Ge,
    ] {
        let func = function(
            "annotation_comparison",
            &["lhs", "rhs"],
            Some(vec!["int", "int"]),
            vec![
                op(comparison, Some("cmp"), None, &["lhs", "rhs"]),
                op(molt_ir::OpKind::Not, Some("negated"), None, &["cmp"]),
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
                op(molt_ir::OpKind::Copy, Some("exact_copy"), None, &["seed"]),
                op(
                    molt_ir::OpKind::StoreVar,
                    None,
                    Some("exact_slot"),
                    &["exact_copy"],
                ),
                op(
                    molt_ir::OpKind::LoadVar,
                    Some("exact_load"),
                    Some("exact_slot"),
                    &[],
                ),
                op(
                    molt_ir::OpKind::Copy,
                    Some("parameter_copy"),
                    None,
                    &["parameter"],
                ),
                op(
                    molt_ir::OpKind::CopyVar,
                    Some("parameter_alias"),
                    Some("parameter_copy"),
                    &[],
                ),
                op(
                    molt_ir::OpKind::StoreVar,
                    None,
                    Some("parameter_slot"),
                    &["parameter_alias"],
                ),
                op(
                    molt_ir::OpKind::LoadVar,
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

#[test]
fn physical_storage_requires_inline_admission_and_closed_aliases_in_both_consumers() {
    for (case, fill, tail, expected) in [
        ("inline", const_int("fill", 7), vec![], true),
        ("heap", const_int("fill", 1_i64 << 62), vec![], false),
        ("bool", const_bool("fill", true), vec![], false),
        (
            "heap_store",
            const_int("fill", 7),
            vec![
                const_int("wide", 1_i64 << 62),
                op(
                    molt_ir::OpKind::StoreIndex,
                    Some("after"),
                    None,
                    &["alias", "i", "wide"],
                ),
            ],
            false,
        ),
        (
            "bool_store",
            const_int("fill", 7),
            vec![
                const_bool("boolean", true),
                op(
                    molt_ir::OpKind::StoreIndex,
                    Some("after"),
                    None,
                    &["alias", "i", "boolean"],
                ),
            ],
            false,
        ),
        (
            "callback_key",
            const_int("fill", 7),
            vec![op(
                molt_ir::OpKind::Index,
                Some("callback_read"),
                None,
                &["alias", "callback"],
            )],
            false,
        ),
        (
            "escape",
            const_int("fill", 7),
            vec![op(
                molt_ir::OpKind::Call,
                Some("ignored"),
                None,
                &["callee", "alias"],
            )],
            false,
        ),
    ] {
        let mut ops = vec![
            const_int("n", 4),
            const_int("i", 0),
            fill,
            op(
                molt_ir::OpKind::ListIntNew,
                Some("xs"),
                None,
                &["n", "fill"],
            ),
            op(molt_ir::OpKind::Copy, Some("alias"), None, &["xs"]),
        ];
        ops.extend(tail);
        ops.push(op(molt_ir::OpKind::Index, Some("read"), None, &["xs", "i"]));
        let func = function(case, &["callback", "callee"], None, ops);
        let plan = native_representation_plan(&func);
        assert_eq!(
            plan.name_container_storage_kind("xs").is_some(),
            expected,
            "{case}"
        );
        let mut tir =
            lower_to_tir_for_target(&func, &crate::tir::TargetInfo::native_release_fast());
        refine_types(&mut tir);
        let lir = crate::tir::lower_to_lir::lower_function_to_lir(&tir);
        assert_eq!(!lir.container_storage.is_empty(), expected, "LIR {case}");
    }
}

#[test]
fn flat_storage_index_keys_require_exact_integers_not_inline_carriers() {
    for (case, key, expected) in [
        ("inline", const_int("i", 0), true),
        ("wide", const_int("i", 1_i64 << 62), true),
        ("bool", const_bool("i", true), true),
        (
            "annotation",
            op(molt_ir::OpKind::Copy, Some("i"), None, &["key"]),
            false,
        ),
    ] {
        for access in [molt_ir::OpKind::Index, molt_ir::OpKind::StoreIndex] {
            let args = if access == molt_ir::OpKind::Index {
                vec!["xs", "i"]
            } else {
                vec!["xs", "i", "fill"]
            };
            let func = function(
                case,
                &["key"],
                Some(vec!["int"]),
                vec![
                    const_int("n", 4),
                    const_int("fill", 7),
                    op(
                        molt_ir::OpKind::ListIntNew,
                        Some("xs"),
                        None,
                        &["n", "fill"],
                    ),
                    key.clone(),
                    op(access, Some("result"), None, &args),
                ],
            );
            let plan = native_representation_plan(&func);
            assert_eq!(
                plan.op_has_container_storage(4, &func.ops[4], ContainerStorageKind::FlatListInt),
                expected,
                "{case} {access}"
            );
            let mut tir =
                lower_to_tir_for_target(&func, &crate::tir::TargetInfo::native_release_fast());
            refine_types(&mut tir);
            let lir = crate::tir::lower_to_lir::lower_function_to_lir(&tir);
            assert_eq!(
                !lir.container_storage.is_empty(),
                expected,
                "LIR {case} {access}"
            );
        }
    }
}

#[test]
fn canonical_repeat_proves_singleton_aliases_before_publishing_flat_storage() {
    for (mutation, expected) in [(false, true), (true, false)] {
        for reverse in [false, true] {
            let mut ops = vec![
                const_int("n", 3),
                const_int("i", 0),
                const_int("fill", 7),
                op(molt_ir::OpKind::ListNew, Some("source"), None, &["fill"]),
                op(molt_ir::OpKind::Copy, Some("alias"), None, &["source"]),
            ];
            if mutation {
                ops.extend([
                    const_int("wide", 1_i64 << 62),
                    op(
                        molt_ir::OpKind::StoreIndex,
                        Some("after"),
                        None,
                        &["alias", "i", "wide"],
                    ),
                ]);
            }
            let operands = if reverse {
                ["n", "source"]
            } else {
                ["source", "n"]
            };
            ops.push(op(molt_ir::OpKind::Mul, Some("repeated"), None, &operands));
            ops.push(op(
                molt_ir::OpKind::Index,
                Some("read"),
                None,
                &["repeated", "i"],
            ));
            let func = function("repeat_storage", &[], None, ops);
            let plan = native_representation_plan(&func);
            assert_eq!(
                plan.name_container_storage_kind("repeated").is_some(),
                expected
            );
        }
    }
}

#[test]
fn runtime_guard_alias_keeps_source_carrier_despite_mismatching_tag() {
    for kind in [molt_ir::OpKind::GuardTag, molt_ir::OpKind::GuardType] {
        let func = function(
            "runtime_guard_carriers",
            &[],
            None,
            vec![
                const_int("left", 1_i64 << 31),
                const_int("right", 1_i64 << 31),
                op(
                    molt_ir::OpKind::CheckedMul,
                    Some("overflow"),
                    Some("wide"),
                    &["left", "right"],
                ),
                const_int("small", 7),
                const_float("float", 1.25),
                const_bool("bool", true),
                const_int("tag", 5),
                op(kind, Some("wide_alias"), None, &["wide", "tag"]),
                op(kind, Some("small_alias"), None, &["small", "tag"]),
                op(kind, Some("float_alias"), None, &["float", "tag"]),
                op(kind, Some("bool_alias"), None, &["bool", "tag"]),
            ],
        );
        let plan = native_representation_plan(&func);
        assert!(plan.is_full_deopt_int_name("wide_alias"), "{kind}");
        assert!(plan.is_inline_safe_int_name("small_alias"), "{kind}");
        assert!(plan.is_float_unboxed("float_alias"), "{kind}");
        assert!(plan.is_bool_unboxed("bool_alias"), "{kind}");
        let facts = crate::passes::RuntimeGuardFacts::for_function(&func);
        assert!(!facts.is_nonthrowing(&func.ops[7], Some(&plan)));
        assert!(facts.is_nonthrowing(&func.ops[8], Some(&plan)));
    }
}

#[test]
fn final_projection_uses_emitted_ssa_identity_and_never_authored_name_collisions() {
    for returned in [false, true] {
        let mut source = function(
            "final_names",
            &["_v0"],
            None,
            vec![
                const_int("integer", 7),
                const_bool("boolean", true),
                op(
                    molt_ir::OpKind::Ret,
                    None,
                    None,
                    &[if returned { "boolean" } else { "integer" }],
                ),
            ],
        );
        let target = crate::tir::TargetInfo::native_release_fast();
        let mut typed = lower_to_tir_for_target(&source, &target);
        for block in typed.blocks.values_mut() {
            for operation in &mut block.ops {
                if !operation.results.is_empty() {
                    operation
                        .attrs
                        .insert("_simple_out".into(), AttrValue::Str("_v0".into()));
                    operation
                        .attrs
                        .insert("_simple_result_0".into(), AttrValue::Str("_v0".into()));
                }
            }
        }
        refine_types(&mut typed);
        let before = serde_json::to_vec(&typed).unwrap();
        source.ops = vec![op(
            molt_ir::OpKind::Ret,
            None,
            None,
            &["poisoned_old_source"],
        )];
        let plan = ScalarRepresentationPlan::project_final_tir(&mut source, &typed);
        assert_eq!(
            serde_json::to_vec(&typed).unwrap(),
            before,
            "fact extraction must not mutate or optimize final SSA"
        );
        assert!(!source.ops.iter().any(|operation| {
            operation
                .args
                .as_ref()
                .is_some_and(|args| args.iter().any(|arg| arg == "poisoned_old_source"))
        }));
        assert_eq!(
            plan.name_scalar_kind("_v0"),
            None,
            "an unproven ABI parameter cannot inherit a colliding scalar producer"
        );
        let integer = source
            .ops
            .iter()
            .find(|operation| operation.kind == molt_ir::OpKind::Const)
            .unwrap()
            .out
            .as_deref()
            .unwrap();
        let boolean = source
            .ops
            .iter()
            .find(|operation| operation.kind == molt_ir::OpKind::ConstBool)
            .unwrap()
            .out
            .as_deref()
            .unwrap();
        assert_ne!(integer, boolean);
        assert_ne!(integer, "_v0");
        assert_ne!(boolean, "_v0");
        assert!(plan.is_inline_safe_int_name(integer));
        assert_eq!(
            plan.name_scalar_kind(boolean),
            Some(ScalarKind::Bool),
            "the emitted producer must retain its semantic Bool identity"
        );
        assert_eq!(
            plan.is_bool_unboxed(boolean),
            !returned,
            "final identity must preserve the existing Boolean escape-storage policy"
        );
        let returned_name = source
            .ops
            .iter()
            .find(|operation| operation.kind == molt_ir::OpKind::Ret)
            .and_then(|operation| operation.args.as_ref())
            .unwrap();
        assert_eq!(returned_name.len(), 1);
        assert_eq!(returned_name[0], if returned { boolean } else { integer });
    }
}

#[test]
fn final_projection_preserves_inline_integer_boundary_and_float_bool_carriers() {
    for (value, inline) in [
        (0, true),
        ((1_i64 << 46) - 1, true),
        (-(1_i64 << 46), true),
        (-(1_i64 << 46) - 1, false),
        (1_i64 << 46, false),
        (i64::MIN, false),
        (i64::MAX, false),
    ] {
        let mut source = function(
            "final_window",
            &[],
            None,
            vec![
                const_int("integer", value),
                const_float("float", 1.25),
                const_bool("boolean", true),
                op(molt_ir::OpKind::Ret, None, None, &["integer"]),
            ],
        );
        let mut typed =
            lower_to_tir_for_target(&source, &crate::tir::TargetInfo::native_release_fast());
        refine_types(&mut typed);
        let plan = ScalarRepresentationPlan::project_final_tir(&mut source, &typed);
        assert_eq!(plan.is_inline_safe_int_name("integer"), inline, "{value}");
        assert_eq!(
            plan.is_full_deopt_int_name("integer"),
            !inline,
            "{value}: exact wide literals require overflow-safe boxing"
        );
        assert!(plan.is_raw_int_carrier_name("integer"), "{value}");
        assert!(plan.is_bool_unboxed("boolean"));
        assert!(plan.is_float_unboxed("float"));
        let relifted_oracle = native_representation_plan(&source);
        assert_eq!(
            plan.int_carrier_names(),
            relifted_oracle.int_carrier_names()
        );
    }
}

#[test]
fn final_projection_keeps_typed_store_site_bound_to_emitted_operation() {
    let mut source = function(
        "final_store",
        &[],
        None,
        vec![
            OpIR {
                value: Some(16),
                ..op(molt_ir::OpKind::Alloc, Some("object"), None, &[])
            },
            const_int("value", 7),
            OpIR {
                value: Some(8),
                source_op_idx: Some(999),
                ..op(molt_ir::OpKind::Store, None, None, &["object", "value"])
            },
            op(molt_ir::OpKind::Ret, None, None, &["object"]),
        ],
    );
    let mut typed =
        lower_to_tir_for_target(&source, &crate::tir::TargetInfo::native_release_fast());
    refine_types(&mut typed);
    let plan = ScalarRepresentationPlan::project_final_tir(&mut source, &typed);
    let index = source
        .ops
        .iter()
        .position(|operation| operation.kind == molt_ir::OpKind::Store)
        .unwrap();
    assert_eq!(
        plan.typed_slot_store_modes().get(&index),
        Some(&TypedSlotStoreMode::DirectNonHeap)
    );
    assert!(
        !plan.typed_slot_store_modes().contains_key(&999),
        "source origin is not an emitted operation index"
    );
}

#[test]
fn final_projection_keeps_checked_value_flag_and_owned_alias_carriers_distinct() {
    let mut source = function(
        "final_checked",
        &["_v3"],
        None,
        vec![
            const_int("left", 1_i64 << 31),
            const_int("right", 1_i64 << 31),
            op(
                molt_ir::OpKind::CheckedMul,
                Some("overflow"),
                Some("wide"),
                &["left", "right"],
            ),
            op(
                molt_ir::OpKind::BindingAlias,
                Some("owned_alias"),
                None,
                &["wide"],
            ),
            op(molt_ir::OpKind::Ret, None, None, &["owned_alias"]),
        ],
    );
    let mut typed =
        lower_to_tir_for_target(&source, &crate::tir::TargetInfo::native_release_fast());
    refine_types(&mut typed);
    let plan = ScalarRepresentationPlan::project_final_tir(&mut source, &typed);
    let checked = source
        .ops
        .iter()
        .find(|operation| operation.kind == molt_ir::OpKind::CheckedMul)
        .unwrap();
    let value = checked.var.as_deref().expect("checked value result");
    let flag = checked.out.as_deref().expect("checked overflow flag");
    assert_ne!(value, flag);
    assert!(plan.is_full_deopt_int_name(value));
    assert!(!plan.is_inline_safe_int_name(value));
    assert!(plan.is_bool_unboxed(flag));
    assert!(plan.is_full_deopt_int_name("owned_alias"));
    assert_eq!(
        plan.name_scalar_kind("_v3"),
        None,
        "canonical ValueId fallback cannot classify an unrelated ABI parameter"
    );
}
