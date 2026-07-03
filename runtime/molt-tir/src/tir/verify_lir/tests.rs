//! Unit tests for the representation-aware LIR verifier.
//!
//! Moved move-only from the monolithic `verify_lir.rs`; no logic changes.

use super::*;
use crate::tir::blocks::BlockId;
use crate::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator};
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn value(id: u32, ty: TirType, repr: LirRepr) -> LirValue {
    LirValue {
        id: ValueId(id),
        ty,
        repr,
    }
}

fn object_new_bound_stack_ref64_op(
    result: u32,
    semantic_class: &str,
    hinted_class: Option<&str>,
    payload_size: Option<i64>,
) -> LirOp {
    let mut attrs = AttrDict::new();
    if let Some(hinted_class) = hinted_class {
        attrs.insert("_type_hint".into(), AttrValue::Str(hinted_class.into()));
    }
    if let Some(payload_size) = payload_size {
        attrs.insert("value".into(), AttrValue::Int(payload_size));
    }
    LirOp {
        tir_op: TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBoundStack,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        },
        result_values: vec![value(
            result,
            TirType::UserClass(semantic_class.into()),
            LirRepr::Ref64,
        )],
    }
}

fn ref64_provenance_func(entry: LirBlock) -> LirFunction {
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    LirFunction {
        name: "ref64_provenance".to_string(),
        param_names: vec![],
        param_types: vec![],
        return_types: vec![TirType::UserClass("Point".to_string())],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    }
}

#[test]
fn ref64_result_requires_stack_allocation_provenance() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![],
        ops: vec![LirOp {
            tir_op: TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![ValueId(0)],
                attrs: AttrDict::new(),
                source_span: None,
            },
            result_values: vec![value(
                0,
                TirType::UserClass("Point".to_string()),
                LirRepr::Ref64,
            )],
        }],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let func = ref64_provenance_func(entry);
    let errors = verify_lir_function(&func).expect_err("arbitrary Ref64 producer must fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Ref64 producer")),
        "expected Ref64 producer error, got {errors:?}"
    );
}

#[test]
fn ref64_stack_allocation_requires_positive_payload() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![],
        ops: vec![object_new_bound_stack_ref64_op(
            0,
            "Point",
            Some("Point"),
            None,
        )],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let func = ref64_provenance_func(entry);
    let errors = verify_lir_function(&func).expect_err("missing payload must fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Ref64 producer")),
        "expected Ref64 producer error, got {errors:?}"
    );
}

#[test]
fn ref64_stack_allocation_requires_matching_type_hint() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![],
        ops: vec![object_new_bound_stack_ref64_op(
            0,
            "Point",
            Some("Other"),
            Some(24),
        )],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let func = ref64_provenance_func(entry);
    let errors = verify_lir_function(&func).expect_err("mismatched class hint must fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Ref64 producer")),
        "expected Ref64 producer error, got {errors:?}"
    );
}

#[test]
fn ref64_stack_allocation_with_matching_class_and_payload_passes() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![],
        ops: vec![object_new_bound_stack_ref64_op(
            0,
            "Point",
            Some("Point"),
            Some(24),
        )],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let func = ref64_provenance_func(entry);
    assert!(verify_lir_function(&func).is_ok());
}

#[test]
fn non_entry_ref64_block_arg_requires_explicit_phi_provenance() {
    let entry_id = BlockId(0);
    let target_id = BlockId(1);
    let entry = LirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![object_new_bound_stack_ref64_op(
            0,
            "Point",
            Some("Point"),
            Some(24),
        )],
        terminator: LirTerminator::Branch {
            target: target_id,
            args: vec![ValueId(0)],
        },
    };
    let target = LirBlock {
        id: target_id,
        args: vec![value(
            1,
            TirType::UserClass("Point".to_string()),
            LirRepr::Ref64,
        )],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(1)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(target_id, target);
    let func = LirFunction {
        name: "ref64_phi".to_string(),
        param_names: vec![],
        param_types: vec![],
        return_types: vec![TirType::UserClass("Point".to_string())],
        blocks,
        entry_block: entry_id,
        label_id_map: HashMap::new(),
    };
    let errors = verify_lir_function(&func).expect_err("Ref64 block arg must fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Ref64 block argument")),
        "expected Ref64 block argument error, got {errors:?}"
    );
}

#[test]
fn repr_for_bool_return_must_match_bool1() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![value(0, TirType::Bool, LirRepr::Bool1)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    let func = LirFunction {
        name: "bool_return".to_string(),
        param_names: vec!["flag".to_string()],
        param_types: vec![TirType::Bool],
        return_types: vec![TirType::Bool],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    assert!(verify_lir_function(&func).is_ok());
}

#[test]
fn dynbox_return_accepts_ref64_class_handle() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![value(
            0,
            TirType::UserClass("Point".to_string()),
            LirRepr::Ref64,
        )],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    let func = LirFunction {
        name: "dynbox_ref64_return".to_string(),
        param_names: vec!["obj".to_string()],
        param_types: vec![TirType::UserClass("Point".to_string())],
        return_types: vec![TirType::DynBox],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    assert!(verify_lir_function(&func).is_ok());
}

#[test]
fn dynbox_return_rejects_ref64_non_reference_value() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![value(0, TirType::I64, LirRepr::Ref64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    let func = LirFunction {
        name: "dynbox_bad_ref64_return".to_string(),
        param_names: vec!["bits".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![TirType::DynBox],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    let errors = verify_lir_function(&func).expect_err("non-reference Ref64 must fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("representation mismatch")),
        "expected representation mismatch, got {errors:?}"
    );
}

#[test]
fn user_class_return_requires_matching_class_identity_for_ref64() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![value(
            0,
            TirType::UserClass("Point".to_string()),
            LirRepr::Ref64,
        )],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    let func = LirFunction {
        name: "wrong_class_ref64_return".to_string(),
        param_names: vec!["obj".to_string()],
        param_types: vec![TirType::UserClass("Point".to_string())],
        return_types: vec![TirType::UserClass("Other".to_string())],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    let errors = verify_lir_function(&func).expect_err("mismatched class return must fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("type mismatch")),
        "expected class identity type mismatch, got {errors:?}"
    );
}

#[test]
fn user_class_return_accepts_dynbox_when_class_proof_is_unavailable() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![value(0, TirType::DynBox, LirRepr::DynBox)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    let func = LirFunction {
        name: "boxed_frozenset_return".to_string(),
        param_names: vec!["value".to_string()],
        param_types: vec![TirType::DynBox],
        return_types: vec![TirType::UserClass("frozenset".to_string())],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    assert!(verify_lir_function(&func).is_ok());
}

#[test]
fn union_return_accepts_concrete_member_type() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![value(0, TirType::None, LirRepr::DynBox)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    let func = LirFunction {
        name: "union_none_return".to_string(),
        param_names: vec!["obj".to_string()],
        param_types: vec![TirType::None],
        return_types: vec![TirType::Union(vec![TirType::Bool, TirType::None])],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    assert!(verify_lir_function(&func).is_ok());
}

#[test]
fn union_return_accepts_identical_union_type() {
    let union_ty = TirType::Union(vec![TirType::Bool, TirType::None]);
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![value(0, union_ty.clone(), LirRepr::DynBox)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(0)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    let func = LirFunction {
        name: "union_identity_return".to_string(),
        param_names: vec!["value".to_string()],
        param_types: vec![union_ty.clone()],
        return_types: vec![union_ty],
        blocks,
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    assert!(verify_lir_function(&func).is_ok());
}

#[test]
fn branch_args_enforce_user_class_identity_when_boxed() {
    let entry_id = BlockId(0);
    let target_id = BlockId(1);
    let entry = LirBlock {
        id: entry_id,
        args: vec![value(
            0,
            TirType::UserClass("Other".to_string()),
            LirRepr::DynBox,
        )],
        ops: vec![],
        terminator: LirTerminator::Branch {
            target: target_id,
            args: vec![ValueId(0)],
        },
    };
    let target = LirBlock {
        id: target_id,
        args: vec![value(
            1,
            TirType::UserClass("Point".to_string()),
            LirRepr::DynBox,
        )],
        ops: vec![],
        terminator: LirTerminator::Return { values: vec![] },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(target_id, target);
    let func = LirFunction {
        name: "boxed_class_branch_mismatch".to_string(),
        param_names: vec!["obj".to_string()],
        param_types: vec![TirType::UserClass("Other".to_string())],
        return_types: vec![],
        blocks,
        entry_block: entry_id,
        label_id_map: HashMap::new(),
    };
    let errors =
        verify_lir_function(&func).expect_err("boxed branch class identity mismatch must fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("branch type mismatch")),
        "expected branch type mismatch, got {errors:?}"
    );
}
