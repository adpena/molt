use super::super::analysis::finalizer_alloc_roots;
use super::*;
use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::values::TirValue;

fn empty_block(id: BlockId) -> TirBlock {
    TirBlock {
        id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Unreachable,
    }
}

fn assert_opaque_use_preserves_allocation(
    opcode: OpCode,
    arity: usize,
    result_count: usize,
    attrs: AttrDict,
) {
    let mut func = TirFunction::new("capture".into(), vec![], TirType::None);
    let operands: Vec<_> = (0..arity).map(|_| func.fresh_value()).collect();
    let results = (0..result_count).map(|_| func.fresh_value()).collect();
    let none = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for &value in &operands {
        entry.ops.push(make_op(OpCode::Alloc, vec![], vec![value]));
        entry.ops.push(make_op(OpCode::IncRef, vec![value], vec![]));
    }
    entry
        .ops
        .push(make_op_with_attrs(opcode, operands.clone(), results, attrs));
    for &value in &operands {
        entry.ops.push(make_op(OpCode::DecRef, vec![value], vec![]));
    }
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![none]));
    entry.terminator = Terminator::Return { values: vec![none] };
    let escapes = analyze(&func);
    for &value in &operands {
        assert_eq!(
            escapes[&value],
            EscapeState::GlobalEscape,
            "{opcode:?} {value:?}"
        );
    }
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(
        ops.iter().filter(|op| op.opcode == OpCode::Alloc).count(),
        arity
    );
    assert_eq!(
        ops.iter().filter(|op| op.opcode == OpCode::IncRef).count(),
        arity
    );
    assert_eq!(
        ops.iter().filter(|op| op.opcode == OpCode::DecRef).count(),
        arity
    );
}

#[test]
fn callable_spelling_and_method_encodings_never_prove_noncapture() {
    for name in [
        "len", "print", "str", "sorted", "iter", "reversed", "zip", "tuple", "unknown",
    ] {
        assert_opaque_use_preserves_allocation(
            OpCode::CallBuiltin,
            1,
            1,
            AttrDict::from([("name".into(), AttrValue::Str(name.into()))]),
        );
    }
    for opcode in [
        OpCode::CallMethod,
        OpCode::CallMethodIc,
        OpCode::CallSuperMethodIc,
    ] {
        for method in [
            "upper",
            "join",
            "BoundMethod:str:upper",
            "BoundMethod:str:",
            "BoundMethod:",
        ] {
            assert_opaque_use_preserves_allocation(
                opcode,
                1,
                1,
                AttrDict::from([
                    ("method".into(), AttrValue::Str(method.into())),
                    ("receiver_type".into(), AttrValue::Str("str".into())),
                ]),
            );
        }
    }
    assert_opaque_use_preserves_allocation(OpCode::Call, 2, 1, AttrDict::new());
}

#[test]
fn overloaded_reads_iteration_and_store_receivers_are_capture_boundaries() {
    for opcode in [
        OpCode::Add,
        OpCode::Sub,
        OpCode::Mul,
        OpCode::InplaceAdd,
        OpCode::Div,
        OpCode::FloorDiv,
        OpCode::Mod,
        OpCode::Pow,
        OpCode::Eq,
        OpCode::Ne,
        OpCode::Lt,
        OpCode::Le,
        OpCode::Gt,
        OpCode::Ge,
        OpCode::BitAnd,
        OpCode::BitOr,
        OpCode::BitXor,
        OpCode::Shl,
        OpCode::Shr,
        OpCode::And,
        OpCode::Or,
        OpCode::In,
        OpCode::NotIn,
        OpCode::Index,
    ] {
        assert_opaque_use_preserves_allocation(opcode, 2, 1, AttrDict::new());
    }
    for opcode in [
        OpCode::Neg,
        OpCode::Pos,
        OpCode::BitNot,
        OpCode::Not,
        OpCode::Bool,
        OpCode::LoadAttr,
        OpCode::GetIter,
    ] {
        assert_opaque_use_preserves_allocation(opcode, 1, 1, AttrDict::new());
    }
    for opcode in [OpCode::IterNext, OpCode::IterNextUnboxed, OpCode::ForIter] {
        assert_opaque_use_preserves_allocation(opcode, 1, 2, AttrDict::new());
    }
    assert_opaque_use_preserves_allocation(
        OpCode::UnpackSequence,
        1,
        2,
        AttrDict::from([("value".into(), AttrValue::Int(2))]),
    );
    assert_opaque_use_preserves_allocation(OpCode::StoreAttr, 2, 0, AttrDict::new());
    assert_opaque_use_preserves_allocation(OpCode::StoreIndex, 3, 0, AttrDict::new());
    assert_opaque_use_preserves_allocation(OpCode::DelAttr, 1, 0, AttrDict::new());
    assert_opaque_use_preserves_allocation(OpCode::DelIndex, 2, 0, AttrDict::new());
}

#[test]
fn every_cfg_edge_carries_escape_back_to_the_allocation() {
    for shape in 0..4 {
        let mut func = TirFunction::new("edge".into(), vec![], TirType::DynBox);
        let root = func.fresh_value();
        let parameter = func.fresh_value();
        let control = func.fresh_value();
        let destination = func.fresh_block();
        let mut target = empty_block(destination);
        target.args.push(TirValue {
            id: parameter,
            ty: TirType::DynBox,
        });
        target.terminator = Terminator::Return {
            values: vec![parameter],
        };
        func.blocks.insert(destination, target);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::Alloc, vec![], vec![root]));
        entry
            .ops
            .push(make_op(OpCode::ConstBool, vec![], vec![control]));
        entry.terminator = match shape {
            0 => Terminator::Branch {
                target: destination,
                args: vec![root],
            },
            1 => Terminator::CondBranch {
                cond: control,
                then_block: destination,
                then_args: vec![root],
                else_block: destination,
                else_args: vec![root],
            },
            2 => Terminator::Switch {
                value: control,
                cases: vec![(1, destination, vec![root])],
                default: destination,
                default_args: vec![root],
            },
            _ => Terminator::StateDispatch {
                cases: vec![(1, destination, vec![root])],
                default: destination,
                default_args: vec![root],
            },
        };
        let escapes = analyze(&func);
        assert_eq!(escapes[&root], EscapeState::GlobalEscape, "shape {shape}");
        assert_eq!(
            escapes[&parameter],
            EscapeState::GlobalEscape,
            "shape {shape}"
        );
    }
}

#[test]
fn finalizer_obligations_flow_through_cfg_and_retained_fields() {
    let mut func = TirFunction::new("finalizer".into(), vec![], TirType::None);
    let owner = func.fresh_value();
    let child = func.fresh_value();
    let parameter = func.fresh_value();
    let none = func.fresh_value();
    let destination = func.fresh_block();
    let mut target = empty_block(destination);
    target.args.push(TirValue {
        id: parameter,
        ty: TirType::DynBox,
    });
    target
        .ops
        .push(make_op(OpCode::DecRef, vec![parameter], vec![]));
    target
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![none]));
    target.terminator = Terminator::Return { values: vec![none] };
    func.blocks.insert(destination, target);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op_with_attrs(
        OpCode::Alloc,
        vec![],
        vec![owner],
        AttrDict::from([("defines_del".into(), AttrValue::Bool(true))]),
    ));
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![child]));
    entry.ops.push(make_store_attr("store", owner, child));
    entry.terminator = Terminator::Branch {
        target: destination,
        args: vec![owner],
    };
    assert!(finalizer_alloc_roots(&func).contains(&parameter));
    let escapes = analyze(&func);
    assert_eq!(escapes[&owner], EscapeState::GlobalEscape);
    assert_eq!(escapes[&child], EscapeState::GlobalEscape);
}

#[test]
fn escaping_root_preserves_reference_counts_on_every_copy_alias() {
    let mut func = TirFunction::new("aliases".into(), vec![], TirType::DynBox);
    let root = func.fresh_value();
    let alias = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![root]));
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![root], vec![alias]));
    entry.ops.push(make_op(OpCode::IncRef, vec![alias], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![alias], vec![]));
    entry.terminator = Terminator::Return { values: vec![root] };
    let escapes = analyze(&func);
    assert_eq!(escapes[&alias], EscapeState::GlobalEscape);
}

#[test]
fn storing_into_mixed_cfg_owner_does_not_prove_local_containment() {
    let mut func = TirFunction::new("mixed_owner".into(), vec![TirType::DynBox], TirType::None);
    let owner = func.fresh_value();
    let child = func.fresh_value();
    let parameter = func.fresh_value();
    let control = func.fresh_value();
    let none = func.fresh_value();
    let destination = func.fresh_block();
    let mut target = empty_block(destination);
    target.args.push(TirValue {
        id: parameter,
        ty: TirType::DynBox,
    });
    target.ops.push(make_store_attr("store", parameter, child));
    target
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![none]));
    target.terminator = Terminator::Return { values: vec![none] };
    func.blocks.insert(destination, target);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![owner]));
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![child]));
    entry
        .ops
        .push(make_op(OpCode::ConstBool, vec![], vec![control]));
    entry.terminator = Terminator::CondBranch {
        cond: control,
        then_block: destination,
        then_args: vec![owner],
        else_block: destination,
        else_args: vec![ValueId(0)],
    };
    assert_eq!(analyze(&func)[&child], EscapeState::GlobalEscape);
}
