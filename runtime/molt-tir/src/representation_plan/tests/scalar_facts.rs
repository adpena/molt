//! Producer carriers through native linkage and reference ownership.
use super::super::test_fixtures::{function, op};
use super::super::*;
use crate::ir::OpIR;
use crate::tir::lower_from_simple::lower_to_tir_for_target;
use crate::tir::ops::AttrValue;
use crate::tir::type_refine::refine_types;

#[test]
fn packed_extents_follow_the_constructor_word_through_a_frame_home() {
    use crate::tir::ops::{OpCode, TirOp};
    use molt_ir::runtime_fn_generated::RuntimeFn;

    let mut tir = TirFunction::new(
        "packed_extents".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let entry = tir.entry_block;
    let parameter = tir.blocks[&entry].args[0].id;
    let rows = tir.fresh_value();
    let cols = tir.fresh_value();
    let zero = tir.fresh_value();
    let local = tir.fresh_value();
    let home = tir.fresh_value();
    let load = tir.fresh_value();
    let loaded = tir.fresh_value();
    let stored = tir.fresh_value();
    let unknown = tir.fresh_value();
    let mut constructor = TirOp::new(OpCode::Call, vec![rows, cols, zero], vec![local]);
    constructor.runtime = Some(RuntimeFn::Buffer2dNew);
    let mut store_home = TirOp::new(OpCode::Copy, vec![local], vec![home]);
    store_home.origin = Some(molt_ir::OpKind::FrameHomeStore);
    store_home.attrs.insert("value".into(), AttrValue::Int(0));
    tir.blocks.get_mut(&entry).unwrap().ops = vec![(rows, 1), (cols, 64), (zero, 0)]
        .into_iter()
        .map(|(value, n)| {
            let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![value]);
            op.attrs.insert("value".into(), AttrValue::Int(n));
            op
        })
        .chain([
            constructor,
            store_home,
            TirOp::new(
                OpCode::TryBuffer2dLoadI64,
                vec![local, zero, zero],
                vec![load, loaded],
            ),
            TirOp::new(
                OpCode::TryBuffer2dStoreI64,
                vec![home, zero, zero, zero],
                vec![stored],
            ),
            TirOp::new(
                OpCode::TryBuffer2dStoreI64,
                vec![parameter, zero, zero, zero],
                vec![unknown],
            ),
        ])
        .collect();
    let plan = ProducerCarrierPlan::default();
    assert_eq!(plan.packed_buffer_extents(&tir, local), Some([1, 64]));
    assert_eq!(plan.packed_buffer_extents(&tir, home), Some([1, 64]));
    assert_eq!(plan.packed_buffer_extents(&tir, parameter), None);
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

fn final_result(function: &TirFunction, opcode: crate::tir::ops::OpCode) -> ValueId {
    function
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == opcode)
        .unwrap()
        .results[0]
}

#[test]
fn final_carriers_follow_ssa_identity_across_authored_name_collisions() {
    for returned in [false, true] {
        let source = function(
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
        let plan = ProducerCarrierPlan::build(&typed);
        assert_eq!(
            serde_json::to_vec(&typed).unwrap(),
            before,
            "fact extraction must not mutate or optimize final SSA"
        );
        let parameter = typed.blocks[&typed.entry_block].args[0].id;
        let integer = final_result(&typed, crate::tir::ops::OpCode::ConstInt);
        let boolean = final_result(&typed, crate::tir::ops::OpCode::ConstBool);
        assert_ne!(parameter, integer);
        assert_ne!(parameter, boolean);
        assert_ne!(integer, boolean);
        assert_eq!(plan.effective_param_types(&typed), [TirType::DynBox]);
        assert!(plan.is_inline_safe_int(integer));
        assert_eq!(
            plan.effective_value_type(boolean, &TirType::DynBox),
            TirType::Bool
        );
        let expected = if returned {
            TirType::Bool
        } else {
            TirType::I64
        };
        assert_eq!(plan.effective_return_type(&typed), expected);
    }
}

#[test]
fn final_carriers_preserve_inline_integer_boundary_and_float_bool_carriers() {
    for (value, inline) in [
        (0, true),
        ((1_i64 << 46) - 1, true),
        (-(1_i64 << 46), true),
        (-(1_i64 << 46) - 1, false),
        (1_i64 << 46, false),
        (i64::MIN, false),
        (i64::MAX, false),
    ] {
        let source = function(
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
        let plan = ProducerCarrierPlan::build(&typed);
        let integer = final_result(&typed, crate::tir::ops::OpCode::ConstInt);
        let float = final_result(&typed, crate::tir::ops::OpCode::ConstFloat);
        let boolean = final_result(&typed, crate::tir::ops::OpCode::ConstBool);
        assert_eq!(plan.is_inline_safe_int(integer), inline, "{value}");
        assert_eq!(plan.is_full_deopt_int(integer), !inline, "{value}");
        assert!(plan.is_raw_int_carrier(integer));
        assert_eq!(
            plan.effective_value_type(float, &TirType::DynBox),
            TirType::F64
        );
        assert_eq!(
            plan.effective_value_type(boolean, &TirType::DynBox),
            TirType::Bool
        );
    }
}

#[test]
fn final_carriers_keep_checked_value_flag_and_owned_alias_distinct() {
    let source = function(
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
    let plan = ProducerCarrierPlan::build(&typed);
    let ops = &typed.blocks[&typed.entry_block].ops;
    let checked = ops
        .iter()
        .find(|op| op.opcode == crate::tir::ops::OpCode::CheckedMul)
        .unwrap();
    let &[product, overflow] = checked.results.as_slice() else {
        panic!("checked product and overflow bit");
    };
    let alias = ops
        .iter()
        .find(|op| op.origin == Some(molt_ir::OpKind::BindingAlias))
        .unwrap();
    assert_eq!(alias.operands, [product]);
    assert!(plan.is_full_deopt_int(product));
    assert!(!plan.is_inline_safe_int(product));
    assert_eq!(
        plan.effective_value_type(overflow, &TirType::DynBox),
        TirType::Bool
    );
    assert!(plan.is_full_deopt_int(alias.results[0]));
    assert_eq!(plan.effective_param_types(&typed), [TirType::DynBox]);
}

#[test]
fn producer_carrier_plan_refcount_obligations_require_a_value_proof() {
    let plan = ProducerCarrierPlan {
        repr_by_value: HashMap::from_iter([
            (ValueId(0), Repr::RawI64FullDeopt),
            (ValueId(1), Repr::BoxedImmortal),
            (ValueId(2), Repr::DynBox),
        ]),
        ..Default::default()
    };
    assert!(plan.carrier_refcount_is_noop(ValueId(0)));
    assert!(
        !plan.boxed_refcount_is_noop(ValueId(0)),
        "boxing may allocate a BigInt"
    );
    assert!(plan.carrier_refcount_is_noop(ValueId(1)));
    assert!(plan.boxed_refcount_is_noop(ValueId(1)));
    for value in [ValueId(2), ValueId(3)] {
        assert!(!plan.carrier_refcount_is_noop(value));
        assert!(!plan.boxed_refcount_is_noop(value));
    }
}
