use super::*;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Helper to make a simple TirOp.
fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

/// Phase 5 step 2: ObjectNewBound result that is only used by
/// identity comparison (i.e. observed locally, never returned, never stored
/// into a non-alloc heap location, never passed to an escaping
/// op) is classified as NoEscape — the same lattice value as a
/// local-only `Alloc`. This is an identity/capture fact, not permission to
/// select frame storage or erase destruction.
#[test]
fn local_only_object_new_bound_is_no_escape() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0); // function parameter standing in for the class ref
    let inst_val = func.fresh_value();
    let load_result = func.fresh_value();
    let const_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![inst_val],
    ));
    entry.ops.push(make_op(
        OpCode::Is,
        vec![inst_val, inst_val],
        vec![load_result],
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let escapes = analyze(&func);
    assert_eq!(escapes[&inst_val], EscapeState::NoEscape);
}

#[test]
fn terminator_direct_uses_preserve_escape_obligations_without_classifying_edges() {
    use crate::tir::blocks::{BlockId, TirBlock};

    let object = ValueId(0);
    let exit = BlockId(1);
    for (terminator, expected) in [
        (
            Terminator::Return {
                values: vec![object],
            },
            EscapeState::GlobalEscape,
        ),
        (
            Terminator::CondBranch {
                cond: object,
                then_block: exit,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
            EscapeState::GlobalEscape,
        ),
        (
            Terminator::Switch {
                value: object,
                cases: vec![(1, exit, vec![])],
                default: exit,
                default_args: vec![],
            },
            EscapeState::GlobalEscape,
        ),
        (
            Terminator::Branch {
                target: exit,
                args: vec![],
            },
            EscapeState::NoEscape,
        ),
        (
            Terminator::StateDispatch {
                cases: vec![(1, exit, vec![])],
                default: exit,
                default_args: vec![],
            },
            EscapeState::NoEscape,
        ),
        (Terminator::Unreachable, EscapeState::NoEscape),
    ] {
        let mut func = TirFunction::new("direct_uses".into(), vec![], TirType::None);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::Alloc, vec![], vec![object]));
        entry.terminator = terminator;
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        assert_eq!(
            analyze(&func)[&object],
            expected,
            "{:?}",
            func.blocks[&func.entry_block].terminator
        );
    }
}

/// Regression: a freshly-constructed object that flows into a container
/// literal *through an SSA `Copy`* must be classified `GlobalEscape`, not
/// `NoEscape`. The frontend lowers `[Box()]` to
/// `obj = ObjectNewBound; tmp = Copy obj; BuildList tmp`. Without Copy-alias
/// tracking, `obj` was wrongly left `NoEscape` and stack-promoted while the
/// escaping list outlived the frame — a use-after-free (objects read back as
/// type `object`) that only manifested under dev-mode codegen.
#[test]
fn object_new_bound_copied_into_container_escapes() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let copy_val = func.fresh_value();
    let list_val = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![inst_val],
    ));
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![inst_val], vec![copy_val]));
    entry
        .ops
        .push(make_op(OpCode::BuildList, vec![copy_val], vec![list_val]));
    entry.terminator = Terminator::Return {
        values: vec![list_val],
    };

    let escapes = analyze(&func);
    assert_eq!(
        escapes[&inst_val],
        EscapeState::GlobalEscape,
        "object copied into an escaping container must escape"
    );
    assert_eq!(
        escapes[&copy_val],
        EscapeState::GlobalEscape,
        "the copy alias must escape too"
    );
}

/// Build a `StoreAttr` op with the given `_original_kind` spelling targeting
/// `target` (operand 0). The frontend's offset-keyed `store` form is a
/// typed-slot write; everything else (`set_attr_generic_ptr`, …)
/// is dict-routed.
fn make_store_attr(original_kind: &str, target: ValueId, value: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    if original_kind == "store" {
        attrs.insert("value".into(), AttrValue::Int(0));
    }
    attrs.insert(
        "_original_kind".into(),
        AttrValue::Str(original_kind.into()),
    );
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreAttr,
        operands: vec![target, value],
        results: vec![],
        attrs,
        source_span: None,
    }
}

/// Build a `Copy` op carrying an `_original_kind` passthrough (the form the
/// SSA lift assigns to SimpleIR ops without a dedicated TIR opcode, e.g.
/// container constructors like `list_new`/`dict_new`).
fn make_passthrough(original_kind: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert(
        "_original_kind".into(),
        AttrValue::Str(original_kind.into()),
    );
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

/// Regression for the real production lowering: container constructors
/// (`list_new`/`dict_new`/…) have no dedicated TIR opcode and ride the
/// `OpCode::Copy` `_original_kind` passthrough. A freshly-constructed object
/// passed *directly* as such a constructor's operand — `obj = ObjectNewBound;
/// lst = Copy[list_new] obj` — must be classified `GlobalEscape`. Before the
/// fix the escape pass treated every `Copy` as a pure no-escape move, so the
/// object was stack-promoted and freed while the escaping container lived on
/// (the `[Box()]` / `{'k': Box()}` use-after-free that surfaced only under
/// dev-mode codegen).
#[test]
fn object_new_bound_into_list_new_passthrough_escapes() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let list_val = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![inst_val],
    ));
    entry
        .ops
        .push(make_passthrough("list_new", vec![inst_val], vec![list_val]));
    entry.terminator = Terminator::Return {
        values: vec![list_val],
    };

    let escapes = analyze(&func);
    assert_eq!(
        escapes[&inst_val],
        EscapeState::GlobalEscape,
        "object built directly into a list_new passthrough must escape"
    );
}

/// Same for the dict-value position (`{'k': Box()}` → `dict_new`), and
/// preserving the allocation's escape obligation through the value operand.
#[test]
fn object_new_bound_into_dict_new_passthrough_escapes() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let key_val = func.fresh_value();
    let inst_val = func.fresh_value();
    let dict_val = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(8));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::ConstStr, vec![], vec![key_val]));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs,
        source_span: None,
    });
    entry.ops.push(make_passthrough(
        "dict_new",
        vec![key_val, inst_val],
        vec![dict_val],
    ));
    entry.terminator = Terminator::Return {
        values: vec![dict_val],
    };

    assert_eq!(analyze(&func)[&inst_val], EscapeState::GlobalEscape);
}

#[test]
fn generic_store_through_move_alias_is_global_escape() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let instance = func.fresh_value();
    let alias = func.fresh_value();
    let value = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![instance],
    ));
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![instance], vec![alias]));
    entry
        .ops
        .push(make_store_attr("set_attr_generic_ptr", alias, value));
    entry.terminator = Terminator::Return { values: vec![] };

    let escapes = analyze(&func);
    assert_eq!(escapes[&alias], EscapeState::GlobalEscape);
    assert_eq!(
        escapes[&instance],
        EscapeState::GlobalEscape,
        "generic access through an exact alias carries capture to the allocation"
    );
}

#[test]
fn object_new_bound_argument_to_method_call_is_global_escape() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let instance = func.fresh_value();
    let bound_callable = func.fresh_value();
    let call_result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![instance],
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![bound_callable]));
    entry.ops.push(make_op_with_attrs(
        OpCode::CallMethod,
        vec![bound_callable, instance],
        vec![call_result],
        AttrDict::from([("method".into(), AttrValue::Str("append".into()))]),
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    assert_eq!(analyze(&func)[&instance], EscapeState::GlobalEscape);
}

#[test]
fn object_new_bound_stored_to_module_attr_is_global_escape() {
    let mut func = TirFunction::new(
        "module_init".into(),
        vec![TirType::DynBox, TirType::Str, TirType::DynBox],
        TirType::None,
    );
    let module = ValueId(0);
    let attr_name = ValueId(1);
    let class_ref = ValueId(2);
    let instance = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![instance],
    ));
    entry.ops.push(make_op(
        OpCode::ModuleSetAttr,
        vec![module, attr_name, instance],
        vec![],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    assert_eq!(analyze(&func)[&instance], EscapeState::GlobalEscape);
}

/// Phase 5 step 2: ObjectNewBound result that is returned escapes
/// — same lattice handling as `Alloc`.
#[test]
fn returned_object_new_bound_is_global_escape() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![inst_val],
    ));
    entry.terminator = Terminator::Return {
        values: vec![inst_val],
    };

    let escapes = analyze(&func);
    assert_eq!(escapes[&inst_val], EscapeState::GlobalEscape);
}

/// Test 1: Local-only alloc (created, identity observed, no escape) → NoEscape.
#[test]
fn local_only_alloc_is_no_escape() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let load_result = func.fresh_value();
    let const_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry.ops.push(make_op(
        OpCode::Is,
        vec![alloc_val, alloc_val],
        vec![load_result],
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let escapes = analyze(&func);
    assert_eq!(escapes[&alloc_val], EscapeState::NoEscape);
}

/// Test 2: Returned alloc → GlobalEscape.
#[test]
fn returned_alloc_is_global_escape() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let alloc_val = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry.terminator = Terminator::Return {
        values: vec![alloc_val],
    };

    let escapes = analyze(&func);
    assert_eq!(escapes[&alloc_val], EscapeState::GlobalEscape);
}

/// Test 3: Alloc stored into another (non-alloc) object's field → GlobalEscape.
#[test]
fn alloc_stored_into_non_alloc_field_is_global_escape() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let param = ValueId(0); // function parameter, not an alloc
    let alloc_val = func.fresh_value();
    let const_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    // StoreAttr: target=param (non-alloc), value=alloc_val
    entry
        .ops
        .push(make_op(OpCode::StoreAttr, vec![param, alloc_val], vec![]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let escapes = analyze(&func);
    assert_eq!(escapes[&alloc_val], EscapeState::GlobalEscape);
}

/// Helper to make a TirOp with attributes.
fn make_op_with_attrs(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    attrs: AttrDict,
) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

/// A builtin spelling is not a proof that callbacks cannot retain an argument.
#[test]
fn builtin_name_does_not_prove_noncapture() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();
    let mut attrs = AttrDict::new();
    attrs.insert("name".into(), AttrValue::Str("len".into()));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry.ops.push(make_op_with_attrs(
        OpCode::CallBuiltin,
        vec![alloc_val],
        vec![call_result],
        attrs,
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };
    let escapes = analyze(&func);
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::GlobalEscape,
        "builtin spelling alone is not a non-capture proof"
    );
}

/// A mutating method remains an opaque escape boundary.
#[test]
fn mutating_method_append_causes_escape() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let list_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("method".into(), AttrValue::Str("append".into()));
    attrs.insert("receiver_type".into(), AttrValue::Str("list".into()));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![list_val]));
    // list_val.append(alloc_val) — alloc_val is stored into the list
    entry.ops.push(make_op_with_attrs(
        OpCode::CallMethod,
        vec![list_val, alloc_val],
        vec![call_result],
        attrs,
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let escapes = analyze(&func);
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::GlobalEscape,
        "list.append() stores its argument — alloc must escape"
    );
}

/// Frontend `BoundMethod:` text is a receiver hint, not exact dispatch
/// provenance. A mutating method call remains an opaque escape boundary.
#[test]
fn frontend_canonical_form_list_append_still_escapes() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let list_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert(
        "method".into(),
        AttrValue::Str("BoundMethod:list:append".into()),
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![list_val]));
    entry.ops.push(make_op_with_attrs(
        OpCode::CallMethod,
        vec![list_val, alloc_val],
        vec![call_result],
        attrs,
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let escapes = analyze(&func);
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::GlobalEscape,
        "list.append in canonical BoundMethod: form must escape"
    );
}

/// A frontend `BoundMethod:` receiver hint is not an exact non-capture fact.
#[test]
fn bound_method_hint_does_not_prove_noncapture() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();
    let mut attrs = AttrDict::new();
    attrs.insert(
        "method".into(),
        AttrValue::Str("BoundMethod:str:upper".into()),
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry.ops.push(make_op_with_attrs(
        OpCode::CallMethod,
        vec![alloc_val],
        vec![call_result],
        attrs,
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };
    let escapes = analyze(&func);
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::GlobalEscape,
        "BoundMethod text is not exact dispatch or non-capture provenance"
    );
}

/// Even an apparently read-only builtin may invoke user callbacks that capture.
#[test]
fn callback_capable_builtin_defaults_to_escape() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();
    let mut attrs = AttrDict::new();
    attrs.insert("name".into(), AttrValue::Str("print".into()));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry.ops.push(make_op_with_attrs(
        OpCode::CallBuiltin,
        vec![alloc_val],
        vec![call_result],
        attrs,
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };
    let escapes = analyze(&func);
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::GlobalEscape,
        "read-style builtin names cannot bypass opaque callback capture"
    );
}

/// Alloc passed to Call becomes GlobalEscape.
#[test]
fn alloc_passed_to_call_is_global_escape() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![alloc_val], vec![call_result]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let escapes = analyze(&func);
    assert_eq!(escapes[&alloc_val], EscapeState::GlobalEscape);
}

/// Test 6: Empty function → empty results.
#[test]
fn empty_function_produces_empty_results() {
    let func = TirFunction::new("empty".into(), vec![], TirType::None);
    let escapes = analyze(&func);
    assert!(escapes.is_empty());
}

mod capture_boundaries;
