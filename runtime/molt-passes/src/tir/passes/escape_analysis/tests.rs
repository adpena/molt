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
/// LoadAttr (i.e. observed locally, never returned, never stored
/// into a non-alloc heap location, never passed to an escaping
/// op) is classified as NoEscape — the same lattice value as a
/// local-only `Alloc`.  This is the prerequisite for the
/// ObjectNewBound → ObjectNewBoundStack rewrite that lands in
/// step 3.
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
    entry
        .ops
        .push(make_op(OpCode::LoadAttr, vec![inst_val], vec![load_result]));
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
fn local_only_object_new_bound_with_layout_rewrites_to_stack() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let load_result = func.fresh_value();
    let const_result = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(8));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs,
        source_span: None,
    });
    entry
        .ops
        .push(make_op(OpCode::LoadAttr, vec![inst_val], vec![load_result]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let stats = run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();

    assert_eq!(stats.values_changed, 1);
    assert_eq!(entry.ops[0].opcode, OpCode::ObjectNewBoundStack);
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

/// Regression (apply-level): the same `ObjectNewBound -> Copy -> BuildList`
/// shape, even with a payload size present, must NOT be rewritten to
/// `ObjectNewBoundStack`, because the object escapes into the container.
#[test]
fn object_new_bound_copied_into_container_is_not_stack_promoted() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let copy_val = func.fresh_value();
    let list_val = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(8));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs,
        source_span: None,
    });
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![inst_val], vec![copy_val]));
    entry
        .ops
        .push(make_op(OpCode::BuildList, vec![copy_val], vec![list_val]));
    entry.terminator = Terminator::Return {
        values: vec![list_val],
    };

    run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();
    assert_eq!(
        entry.ops[0].opcode,
        OpCode::ObjectNewBound,
        "an escaping object must stay heap-allocated"
    );
}

/// Build a `StoreAttr` op with the given `_original_kind` spelling targeting
/// `target` (operand 0). The frontend's offset-keyed `store`/`store_init`
/// forms are typed-slot writes; everything else (`set_attr_generic_ptr`, …)
/// is dict-routed.
fn make_store_attr(original_kind: &str, target: ValueId, value: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
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

/// Regression: a non-escaping `ObjectNewBound` whose instance receives a
/// GENERIC (dict-routed) attribute store — `g.method = fn`, lowered as
/// `set_attr_generic_ptr` — must NOT be stack-promoted. A fixed-layout
/// immortal stack object cannot anchor a `__dict__`, so the store would
/// silently no-op and the later generic load raise AttributeError.
#[test]
fn object_new_bound_with_generic_attr_store_is_not_stack_promoted() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let fn_val = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(16));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs,
        source_span: None,
    });
    // g.method = fn  -> dict-routed generic store.
    entry
        .ops
        .push(make_store_attr("set_attr_generic_ptr", inst_val, fn_val));
    entry.terminator = Terminator::Return { values: vec![] };

    run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();
    assert_eq!(
        entry.ops[0].opcode,
        OpCode::ObjectNewBound,
        "an object receiving a generic (dict-routed) attribute store must \
         stay heap-allocated so the runtime can materialize its __dict__"
    );
}

/// Counterpart: a non-escaping `ObjectNewBound` that receives ONLY typed-slot
/// stores (`store_init` at a declared field offset) IS still stack-promoted —
/// the dict-routed guard must not over-pessimize the common declared-field case.
#[test]
fn object_new_bound_with_only_typed_slot_store_still_stack_promotes() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let field_val = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(16));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs,
        source_span: None,
    });
    let mut store = make_store_attr("store_init", inst_val, field_val);
    store.attrs.insert("value".into(), AttrValue::Int(0)); // field offset 0
    entry.ops.push(store);
    entry.terminator = Terminator::Return { values: vec![] };

    run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();
    assert_eq!(
        entry.ops[0].opcode,
        OpCode::ObjectNewBoundStack,
        "an object with only typed-slot field stores is layout-fixed and \
         must still be stack-promoted (no dict needed)"
    );
}

/// The dict requirement propagates BACKWARD across a pure-move copy: an
/// `ObjectNewBound` whose move-alias receives the generic store must also be
/// kept on the heap (the same heap object is named by both ids).
#[test]
fn generic_store_through_move_alias_keeps_object_heap() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let alias_val = func.fresh_value();
    let fn_val = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(16));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs,
        source_span: None,
    });
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![inst_val], vec![alias_val]));
    entry
        .ops
        .push(make_store_attr("set_attr_generic_ptr", alias_val, fn_val));
    entry.terminator = Terminator::Return { values: vec![] };

    run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();
    assert_eq!(
        entry.ops[0].opcode,
        OpCode::ObjectNewBound,
        "a generic store through a move-alias must keep the originating \
         allocation heap-allocated"
    );
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
/// asserted end-to-end through `run`: the object must stay heap-allocated.
#[test]
fn object_new_bound_into_dict_new_passthrough_is_not_stack_promoted() {
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

    run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();
    let obj_op = entry
        .ops
        .iter()
        .find(|op| op.results.first() == Some(&inst_val))
        .expect("object alloc op present");
    assert_eq!(
        obj_op.opcode,
        OpCode::ObjectNewBound,
        "object built into a dict_new passthrough value must stay heap-allocated"
    );
}

/// A genuine SSA move that does NOT escape must still allow stack promotion —
/// the pure-move classification must not over-conservatively escape locals.
#[test]
fn local_object_new_bound_through_pure_move_still_stack_promotes() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let moved_val = func.fresh_value();
    let load_result = func.fresh_value();
    let const_result = func.fresh_value();

    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(8));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs,
        source_span: None,
    });
    // Pure SSA move (no `_original_kind`): aliases inst_val.
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![inst_val], vec![moved_val]));
    entry.ops.push(make_op(
        OpCode::LoadAttr,
        vec![moved_val],
        vec![load_result],
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();
    assert_eq!(
        entry.ops[0].opcode,
        OpCode::ObjectNewBoundStack,
        "a local object used only through a pure move must still stack-promote"
    );
}

#[test]
fn local_only_object_new_bound_without_layout_stays_heap() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let load_result = func.fresh_value();
    let const_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ObjectNewBound,
        vec![class_ref],
        vec![inst_val],
    ));
    entry
        .ops
        .push(make_op(OpCode::LoadAttr, vec![inst_val], vec![load_result]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let stats = run(&mut func);
    let entry = func.blocks.get(&func.entry_block).unwrap();

    assert_eq!(stats.values_changed, 0);
    assert_eq!(entry.ops[0].opcode, OpCode::ObjectNewBound);
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

/// Test 1: Local-only alloc (created, field read, no escape) → NoEscape.
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
        OpCode::LoadAttr,
        vec![alloc_val],
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

/// Test: len() (borrowing builtin) does not cause GlobalEscape.
#[test]
fn borrowing_builtin_len_does_not_escape() {
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
    // `len()` borrows its argument across a call boundary without capturing
    // it: the value does not escape the function (still stack-promotable) but
    // is now precisely classified `ArgEscape` rather than `NoEscape` (the S5
    // realization of the ArgEscape lattice point).
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::ArgEscape,
        "len() only borrows — arg crosses a call boundary uncaptured (ArgEscape)"
    );
    assert_ne!(
        escapes[&alloc_val],
        EscapeState::GlobalEscape,
        "borrowed arg must remain non-escaping (stack-promotable)"
    );
}

/// Test: list.append() (mutating method) DOES cause GlobalEscape.
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

/// Production frontend canonical form: the SSA lift copies the
/// SimpleIR `call_method` op's `s_value` (e.g.
/// `"BoundMethod:list:append"`) into `attrs["method"]` and does
/// NOT set `receiver_type`.  Pre-fix, `is_borrowing_method_call`
/// returned false on the missing receiver_type and the CallMethod
/// arm still defaulted to GlobalEscape — *correct but for the
/// wrong reason*: the borrow check never fired even for genuinely
/// borrowing methods.  This test pins that, post-fix, the
/// frontend canonical form correctly classifies a mutating
/// method (list.append) as escaping.
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
        "list.append in canonical BoundMethod: form must escape \
         — list.append is not in the effect_free table"
    );
}

/// Production frontend canonical form: a *pure* monomorphic
/// builtin method (e.g. `str.upper`) on an alloc'd receiver
/// should classify as NoEscape — `str.upper` returns a new
/// string and doesn't capture self.  Before the borrow-check
/// fix that parses the `BoundMethod:` prefix, this test would
/// FAIL: with no `receiver_type` attr set, the old code fell
/// through to GlobalEscape.  Post-fix, the parse extracts
/// `(str, upper)` from the method attr, looks up
/// `effects::method_effects("str", "upper")`, sees `effect_free`,
/// and stays at NoEscape.
#[test]
fn frontend_canonical_form_str_upper_does_not_escape() {
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
    // `str.upper` (effect-free immutable-receiver method) borrows without
    // capture: non-escaping, now precisely `ArgEscape`.
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::ArgEscape,
        "str.upper in canonical BoundMethod: form is borrowing (effect_free) — \
         alloc crosses the call boundary uncaptured (ArgEscape, non-escaping)"
    );
    assert_ne!(escapes[&alloc_val], EscapeState::GlobalEscape);
}

/// Malformed `BoundMethod:` strings (empty receiver, empty
/// method, missing colon) fall through to false.  Soundness
/// failure mode: false-positive borrow ⇒ stack-allocated value
/// dangling past frame ⇒ UAF.  Better to default to escape.
#[test]
fn malformed_bound_method_string_defaults_to_escape() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();

    let mut attrs = AttrDict::new();
    // No method portion (string ends at the receiver colon).
    attrs.insert("method".into(), AttrValue::Str("BoundMethod:str:".into()));

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
        "malformed BoundMethod: string must NOT be parsed into \
         a successful borrow check — soundness over precision"
    );
}

/// Test: print() (I/O but borrowing) does not cause GlobalEscape.
#[test]
fn borrowing_builtin_print_does_not_escape() {
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
    // `print` borrows its argument for I/O without capturing it: non-escaping,
    // now precisely `ArgEscape`.
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::ArgEscape,
        "print() borrows its argument for I/O — non-escaping (ArgEscape)"
    );
    assert_ne!(escapes[&alloc_val], EscapeState::GlobalEscape);
}

/// Test 4: Alloc passed to Call → GlobalEscape (conservative).
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

/// Test 5: Alloc with only local reads → NoEscape, IncRef/DecRef removed.
#[test]
fn no_escape_removes_incref_decref() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let load_result = func.fresh_value();
    let const_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![alloc_val], vec![]));
    entry.ops.push(make_op(
        OpCode::LoadAttr,
        vec![alloc_val],
        vec![load_result],
    ));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![alloc_val], vec![]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let stats = run(&mut func);

    // Alloc should be rewritten to StackAlloc.
    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops[0].opcode, OpCode::StackAlloc);

    // IncRef and DecRef should be removed.
    assert!(
        !entry
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
    );

    assert_eq!(stats.values_changed, 1);
    assert_eq!(stats.ops_removed, 2);
}

/// Phase 5 step 3 soundness regression: an `ObjectNewBound`
/// whose result flows into a `CallMethod` (i.e. `pts.append(p)`
/// in user code, lowered as `CALL_METHOD(append, [pts, p])`)
/// MUST be classified as `GlobalEscape`.  The production frontend
/// does not set `receiver_type` on `CallMethod` ops — only the
/// `method` attribute is set from the SimpleIR `s_value` field —
/// so `is_borrowing_method_call` falls through to `false` and
/// the CallMethod arm correctly defaults to `GlobalEscape`.
///
/// If anyone ever propagates `receiver_type=list` to production
/// CallMethod ops AND adds `list.append` to the borrowing list
/// without considering element-escape, this test will catch the
/// regression: stack-allocating `p` would leave `pts` holding
/// dangling pointers into a popped frame.
#[test]
fn object_new_bound_into_list_append_escapes_via_call_method() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let class_ref = ValueId(0);
    let inst_val = func.fresh_value();
    let list_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();

    // ObjectNewBound carrying a positive payload size (24 bytes)
    // — the same shape the frontend emits for a typed class with
    // 2 int fields + the trailing __dict__ slot.
    let mut alloc_attrs = AttrDict::new();
    alloc_attrs.insert("value".into(), AttrValue::Int(24));

    // CallMethod with only `method=append` — production does NOT
    // emit `receiver_type`, so the borrow check falls through to
    // false and the value escapes.
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("method".into(), AttrValue::Str("append".into()));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs: alloc_attrs,
        source_span: None,
    });
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![list_val]));
    entry.ops.push(make_op_with_attrs(
        OpCode::CallMethod,
        vec![list_val, inst_val],
        vec![call_result],
        call_attrs,
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let stats = run(&mut func);

    // The Point alloc must NOT have been rewritten — it escapes
    // via list.append, and stack-allocating it would leave the
    // list with a dangling pointer when the frame pops.
    let entry = &func.blocks[&func.entry_block];
    assert_eq!(
        entry.ops[0].opcode,
        OpCode::ObjectNewBound,
        "ObjectNewBound stored into a list must NOT be rewritten to ObjectNewBoundStack — \
         would dangle when the frame pops"
    );
    assert_eq!(
        stats.values_changed, 0,
        "no values should be rewritten when the alloc escapes"
    );
}

#[test]
fn object_new_bound_stored_to_module_attr_escapes() {
    let mut func = TirFunction::new(
        "molt_init_typing".into(),
        vec![TirType::DynBox, TirType::Str, TirType::DynBox],
        TirType::None,
    );
    let module_obj = ValueId(0);
    let attr_name = ValueId(1);
    let class_ref = ValueId(2);
    let inst_val = func.fresh_value();
    let const_result = func.fresh_value();

    let mut alloc_attrs = AttrDict::new();
    alloc_attrs.insert("value".into(), AttrValue::Int(24));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![class_ref],
        results: vec![inst_val],
        attrs: alloc_attrs,
        source_span: None,
    });
    entry.ops.push(make_op(
        OpCode::ModuleSetAttr,
        vec![module_obj, attr_name, inst_val],
        vec![],
    ));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
    entry.terminator = Terminator::Return {
        values: vec![const_result],
    };

    let stats = run(&mut func);
    let entry = &func.blocks[&func.entry_block];

    assert_eq!(
        entry.ops[0].opcode,
        OpCode::ObjectNewBound,
        "module globals outlive the module init frame, so stored objects must stay heap allocated"
    );
    assert_eq!(stats.values_changed, 0);
}

/// Test 6: Empty function → empty results.
#[test]
fn empty_function_produces_empty_results() {
    let func = TirFunction::new("empty".into(), vec![], TirType::None);
    let escapes = analyze(&func);
    assert!(escapes.is_empty());
}

/// Test 7: effect_free builtin (e.g. `sorted`) does not cause GlobalEscape.
/// This tests the PLDI 2024 ArgEscape→NoEscape downgrade: an effect_free
/// callee cannot store its arguments, so passing an alloc'd value to it
/// should NOT escalate the escape state to GlobalEscape.
#[test]
fn effect_free_builtin_does_not_escalate_escape() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_val = func.fresh_value();
    let call_result = func.fresh_value();
    let const_result = func.fresh_value();

    let mut attrs = AttrDict::new();
    // `sorted` is effect_free (per effects.rs) but NOT in the
    // explicit is_borrowing_builtin list. The ArgEscape→NoEscape
    // downgrade should catch this.
    attrs.insert("name".into(), AttrValue::Str("sorted".into()));

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
    // An effect-free callee cannot store its argument, so the value does not
    // escape the function — but it did cross a call boundary, so it is
    // precisely `ArgEscape` (non-escaping, the key invariant: never
    // GlobalEscape).
    assert_eq!(
        escapes[&alloc_val],
        EscapeState::ArgEscape,
        "effect_free builtin (sorted) borrows without capture → ArgEscape"
    );
    assert_ne!(
        escapes[&alloc_val],
        EscapeState::GlobalEscape,
        "effect_free builtin must not escalate to GlobalEscape"
    );
}
