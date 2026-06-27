use super::*;

#[test]
fn linearize_simple_function_compiles() {
    let func = add_function();
    let ops = lower_to_simple_ir(&func);
    // Must produce at least one op.
    assert!(!ops.is_empty(), "expected non-empty ops for add function");
}

#[test]
fn linearize_emits_return() {
    let func = add_function();
    let ops = lower_to_simple_ir(&func);
    let has_ret = ops.iter().any(|o| o.kind == "ret" || o.kind == "ret_void");
    assert!(has_ret, "expected a return op, got: {:?}", ops);
}

#[test]
fn lower_to_simple_emits_separate_drop_fact_markers() {
    let mut func = TirFunction::new("drop_fact_markers".into(), vec![], TirType::None);
    func.attrs.insert(
        crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR.to_string(),
        AttrValue::Bool(true),
    );
    func.attrs.insert(
        crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR.to_string(),
        AttrValue::Bool(true),
    );
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };

    let ops = lower_to_simple_ir(&func);

    assert_eq!(
        ops.iter()
            .take(2)
            .map(|op| op.kind.as_str())
            .collect::<Vec<_>>(),
        vec![
            crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR,
            crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR,
        ],
        "full drop ownership and exception-only drop facts must remain distinct on SimpleIR transport"
    );
}

#[test]
fn state_yield_resume_continuation_is_linearized_immediately_after_suspend() {
    let mut func = TirFunction::new(
        "state_yield_resume_continuation_is_linearized_immediately_after_suspend".into(),
        vec![],
        TirType::DynBox,
    );
    let entry = func.entry_block;
    let yield_block = func.fresh_block();
    let unrelated_unreachable = func.fresh_block();
    let resume_block = func.fresh_block();
    let yielded_pair = func.fresh_value();
    let done_value = func.fresh_value();

    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(5, resume_block, vec![])],
        default: yield_block,
        default_args: vec![],
    };

    let mut yield_attrs = AttrDict::new();
    yield_attrs.insert("value".into(), AttrValue::Int(5));
    func.blocks.insert(
        yield_block,
        TirBlock {
            id: yield_block,
            args: vec![],
            ops: vec![
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstNone,
                    operands: vec![],
                    results: vec![yielded_pair],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::StateYield,
                    operands: vec![yielded_pair],
                    results: vec![],
                    attrs: yield_attrs,
                    source_span: None,
                },
            ],
            terminator: Terminator::Unreachable,
        },
    );
    func.blocks.insert(
        unrelated_unreachable,
        TirBlock {
            id: unrelated_unreachable,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        resume_block,
        TirBlock {
            id: resume_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstNone,
                operands: vec![],
                results: vec![done_value],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![done_value],
            },
        },
    );

    let ops = lower_to_simple_ir(&func);
    let state_yield_idx = ops
        .iter()
        .position(|op| op.kind == "state_yield")
        .expect("state_yield op");
    let next = ops
        .get(state_yield_idx + 1)
        .expect("resume continuation after state_yield");
    assert_eq!(next.kind, "state_label", "{ops:?}");
    assert_eq!(next.value, Some(5), "{ops:?}");
    assert!(
        ops[state_yield_idx + 1..]
            .iter()
            .take_while(|op| !(op.kind == "ret" || op.kind == "ret_void"))
            .all(|op| op.kind != "unreachable"),
        "pure state_yield suspend must not emit an unreachable before its resume continuation: {ops:?}"
    );
}

#[test]
fn result_carrying_store_var_lowers_to_defined_alias_value() {
    let mut func = TirFunction::new("store_var_result_alias".into(), vec![], TirType::None);
    let source = func.fresh_value();
    let stored = func.fresh_value();
    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![source],
            attrs: AttrDict::new(),
            source_span: None,
        });
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str("store_var".into()));
        attrs.insert("_var".into(), AttrValue::Str("slot".into()));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![source],
            results: vec![stored],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return {
            values: vec![stored],
        };
    }

    let ops = lower_to_simple_ir(&func);

    let source_name = value_var(source);
    let stored_name = value_var(stored);
    let store = ops
        .iter()
        .find(|op| op.kind == "store_var" && op.var.as_deref() == Some("slot"))
        .expect("result-carrying store_var must preserve the local lifetime boundary");
    assert_eq!(
        store.args.as_deref(),
        Some(&[source_name.clone()][..]),
        "store_var boundary must store the original source bits"
    );
    let alias = ops
        .iter()
        .find(|op| op.kind == "copy_var" && op.out.as_deref() == Some(stored_name.as_str()))
        .expect("result-carrying store_var must define its SSA alias result");
    assert_eq!(
        alias.var.as_deref(),
        Some(source_name.as_str()),
        "store_var alias result must preserve the canonical copy_var source"
    );
    assert_eq!(
        alias.args, None,
        "store_var alias copy_var must not duplicate its source through args"
    );

    let relifted = lower_to_tir(&FunctionIR {
        name: "store_var_result_alias".into(),
        ops,
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
    });
    assert!(
        relifted.blocks.values().any(|block| {
            block.ops.iter().any(|op| {
                op.opcode == OpCode::Copy
                    && matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == "store_var"
                    )
                    && matches!(op.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "slot")
            })
        }),
        "store_var lifetime marker must survive a SimpleIR relift"
    );
    let relifted_alias = relifted
        .blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .find(|op| {
            op.opcode == OpCode::Copy
                && op
                    .attrs
                    .get("_simple_out")
                    .is_some_and(|out| matches!(out, AttrValue::Str(out) if out == &stored_name))
        })
        .expect("store_var alias copy_var must survive a SimpleIR relift");
    assert_eq!(
        relifted_alias.operands.len(),
        1,
        "store_var alias relift must have exactly one source operand"
    );
}

#[test]
fn copy_var_reemission_prefers_preserved_source_local_name() {
    let mut func = TirFunction::new("copy_var_source_local_name".into(), vec![], TirType::None);
    let source = func.fresh_value();
    let copied = func.fresh_value();
    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![source],
            attrs: AttrDict::new(),
            source_span: None,
        });
        let mut attrs = AttrDict::new();
        attrs.insert("_var".into(), AttrValue::Str("x".into()));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![source],
            results: vec![copied],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return {
            values: vec![copied],
        };
    }

    let ops = lower_to_simple_ir(&func);
    let copied_name = value_var(copied);
    let copy = ops
        .iter()
        .find(|op| op.kind == "copy_var" && op.out.as_deref() == Some(copied_name.as_str()))
        .expect("copy_var must be re-emitted for the copied value");

    assert_eq!(copy.var.as_deref(), Some("x"));
    assert_eq!(
        copy.args, None,
        "copy_var source identity must use var metadata, not a duplicate args lane"
    );
}

#[test]
fn lower_shift_ops_use_runtime_simple_ir_names() {
    let mut func = TirFunction::new(
        "shift_names".into(),
        vec![TirType::I64, TirType::I64],
        TirType::DynBox,
    );
    let shl = ValueId(func.next_value);
    func.next_value += 1;
    let shr = ValueId(func.next_value);
    func.next_value += 1;
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Shl,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![shl],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Shr,
        operands: vec![shl, ValueId(1)],
        results: vec![shr],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![shr] };

    let ops = lower_to_simple_ir(&func);
    assert!(ops.iter().any(|op| op.kind == "lshift"));
    assert!(ops.iter().any(|op| op.kind == "rshift"));
    assert!(!ops.iter().any(|op| op.kind == "shl"));
    assert!(!ops.iter().any(|op| op.kind == "shr"));
}

#[test]
fn lower_import_with_operand_roundtrips_as_module_import() {
    let mut func = TirFunction::new("import_roundtrip".into(), vec![], TirType::DynBox);
    let name = ValueId(func.next_value);
    func.next_value += 1;
    let imported = ValueId(func.next_value);
    func.next_value += 1;
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![name],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("builtins".into()));
            attrs
        },
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Import,
        operands: vec![name],
        results: vec![imported],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![imported],
    };

    let ops = lower_to_simple_ir(&func);
    let import_op = ops
        .iter()
        .find(|op| op.kind == "module_import")
        .expect("expected module_import op");
    assert_eq!(import_op.args.as_ref().map(Vec::len), Some(1));
}

fn assert_module_mutation_roundtrips(opcode: OpCode, simple_kind: &str, arity: usize) {
    let mut func = TirFunction::new(
        format!("{simple_kind}_roundtrip"),
        std::iter::repeat_n(TirType::DynBox, arity).collect(),
        TirType::None,
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: (0..arity as u32).map(ValueId).collect(),
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let ops = lower_to_simple_ir(&func);
    let module_op = ops
        .iter()
        .find(|op| op.kind == simple_kind)
        .unwrap_or_else(|| panic!("expected {simple_kind} op, got {ops:?}"));

    assert_eq!(module_op.args.as_ref().map(Vec::len), Some(arity));
    assert_eq!(
        module_op.out.as_deref(),
        Some("none"),
        "{simple_kind} must preserve no-result mutation shape"
    );
}

#[test]
fn lower_module_cache_set_roundtrips() {
    assert_module_mutation_roundtrips(OpCode::ModuleCacheSet, "module_cache_set", 2);
}

#[test]
fn lower_module_cache_del_roundtrips() {
    assert_module_mutation_roundtrips(OpCode::ModuleCacheDel, "module_cache_del", 1);
}

#[test]
fn lower_module_set_attr_roundtrips() {
    assert_module_mutation_roundtrips(OpCode::ModuleSetAttr, "module_set_attr", 3);
}

#[test]
fn lower_module_del_global_roundtrips() {
    assert_module_mutation_roundtrips(OpCode::ModuleDelGlobal, "module_del_global", 2);
}

#[test]
fn lower_module_del_global_if_present_roundtrips() {
    assert_module_mutation_roundtrips(
        OpCode::ModuleDelGlobalIfPresent,
        "module_del_global_if_present",
        2,
    );
}

#[test]
fn empty_tir_return_preserves_original_ret_signature() {
    let mut func = TirFunction::new("ret_none".into(), vec![], TirType::DynBox);
    func.attrs
        .insert("_original_has_ret".into(), AttrValue::Bool(true));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Return { values: vec![] };

    let ops = lower_to_simple_ir(&func);

    assert!(
        !ops.iter().any(|op| op.kind == "ret_void"),
        "roundtrip must not downgrade original `ret` to `ret_void`: {ops:?}"
    );
    let ret_op = ops
        .iter()
        .find(|op| op.kind == "ret")
        .expect("roundtrip must synthesize `ret None`");
    let none_op = ops
        .iter()
        .find(|op| op.kind == "const_none")
        .expect("roundtrip must synthesize a const_none return value");
    let none_name = none_op
        .out
        .as_deref()
        .expect("const_none must define an output var");
    assert_eq!(
        ret_op.var.as_deref(),
        Some(none_name),
        "ret must use the synthesized None value"
    );
    assert_eq!(
        ret_op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str),
        Some(none_name),
        "ret args must also reference the synthesized None value"
    );
}

#[test]
fn ret_op_has_var_set() {
    let func = add_function();
    let ops = lower_to_simple_ir(&func);
    let ret_op = ops
        .iter()
        .find(|o| o.kind == "ret")
        .expect("expected a ret op");
    assert!(
        ret_op.var.is_some(),
        "ret op must have `var` set for the native backend; got: {:?}",
        ret_op
    );
}

/// Integration test: full TIR round-trip preserves `ret` var field.
/// This simulates the frontend's `def add(a,b): return a+b` IR.
#[test]
fn tir_round_trip_preserves_ret_var() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;
    use crate::tir::type_refine;

    let func_ir = FunctionIR {
        name: "add".into(),
        params: vec!["a".into(), "b".into()],
        ops: vec![
            OpIR {
                kind: "add".into(),
                args: Some(vec!["a".into(), "b".into()]),
                out: Some("v0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("v0".into()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let mut tir_func = lower_to_tir(&func_ir);
    type_refine::refine_types(&mut tir_func);
    let round_tripped = lower_to_simple_ir(&tir_func);

    let ret_op = round_tripped
        .iter()
        .find(|o| o.kind == "ret")
        .expect("TIR round-trip must preserve the ret op");
    assert!(
        ret_op.var.is_some(),
        "TIR round-trip must set `var` on ret op for native backend; got: {:?}",
        ret_op,
    );
}

/// The module phase re-lifts every function from post-pipeline SimpleIR
/// on each build, and the TIR content cache re-lifts on cache hits. An op
/// that doesn't round-trip falls to the `OpCode::Copy` fallback and
/// silently vanishes (the iterator-consumer bug class — see the
/// `exception_pending` precedent in ssa.rs). This pins the full
/// lift → type-refine → re-emit cycle for the 2-result `CheckedAdd`.
#[test]
fn checked_add_two_result_round_trip_survives_relift() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;
    use crate::tir::type_refine;

    // SimpleIR transport shape (the IterNextUnboxed convention):
    // var = wrapping sum (results[0]), out = overflow flag (results[1]).
    // Both results stay live: the flag feeds a br_if, the sum a ret.
    let func_ir = FunctionIR {
        name: "checked_add_roundtrip".into(),
        params: vec!["a".into(), "b".into()],
        ops: vec![
            OpIR {
                kind: "checked_add".into(),
                args: Some(vec!["a".into(), "b".into()]),
                var: Some("sum0".into()),
                out: Some("of0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "br_if".into(),
                args: Some(vec!["of0".into()]),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("sum0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".into(),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("a".into()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    // Lift (the module-phase path): the opcode must survive — NOT fall
    // to the Copy fallback (which would delete the overflow check).
    let mut tir_func = lower_to_tir(&func_ir);
    let ca_op = tir_func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .find(|op| op.opcode == OpCode::CheckedAdd)
        .expect("checked_add must lift to OpCode::CheckedAdd, not Copy")
        .clone();
    assert_eq!(ca_op.operands.len(), 2, "both operands must survive");
    assert_eq!(ca_op.results.len(), 2, "both results must survive");

    // Result types are intrinsic to the opcode (I64 sum, Bool flag) —
    // the WASM/LIR local types derive from these after every re-lift.
    type_refine::refine_types(&mut tir_func);
    assert_eq!(
        tir_func.value_types.get(&ca_op.results[0]),
        Some(&TirType::I64),
        "sum must refine to I64"
    );
    assert_eq!(
        tir_func.value_types.get(&ca_op.results[1]),
        Some(&TirType::Bool),
        "overflow flag must refine to Bool"
    );

    // Re-emit: same kind, same var/out convention, distinct outputs.
    let round_tripped = lower_to_simple_ir(&tir_func);
    let ca = round_tripped
        .iter()
        .find(|op| op.kind == "checked_add")
        .expect("re-emit must preserve checked_add");
    assert_eq!(ca.args.as_ref().map(Vec::len), Some(2));
    let sum_var = ca.var.as_deref().expect("sum must round-trip in var");
    let flag_var = ca.out.as_deref().expect("flag must round-trip in out");
    assert_ne!(sum_var, flag_var, "the two outputs must stay distinct");
}

#[test]
fn checked_mul_two_result_round_trip_survives_relift() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;
    use crate::tir::type_refine;

    let func_ir = FunctionIR {
        name: "checked_mul_roundtrip".into(),
        params: vec!["a".into(), "b".into()],
        ops: vec![
            OpIR {
                kind: "checked_mul".into(),
                args: Some(vec!["a".into(), "b".into()]),
                var: Some("product0".into()),
                out: Some("of0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "br_if".into(),
                args: Some(vec!["of0".into()]),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("product0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".into(),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("a".into()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let mut tir_func = lower_to_tir(&func_ir);
    let cm_op = tir_func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .find(|op| op.opcode == OpCode::CheckedMul)
        .expect("checked_mul must lift to OpCode::CheckedMul, not Copy")
        .clone();
    assert_eq!(cm_op.operands.len(), 2, "both operands must survive");
    assert_eq!(cm_op.results.len(), 2, "both results must survive");

    type_refine::refine_types(&mut tir_func);
    assert_eq!(
        tir_func.value_types.get(&cm_op.results[0]),
        Some(&TirType::I64),
        "product must refine to I64"
    );
    assert_eq!(
        tir_func.value_types.get(&cm_op.results[1]),
        Some(&TirType::Bool),
        "overflow flag must refine to Bool"
    );

    let round_tripped = lower_to_simple_ir(&tir_func);
    let cm = round_tripped
        .iter()
        .find(|op| op.kind == "checked_mul")
        .expect("re-emit must preserve checked_mul");
    assert_eq!(cm.args.as_ref().map(Vec::len), Some(2));
    let product_var = cm.var.as_deref().expect("product must round-trip in var");
    let flag_var = cm.out.as_deref().expect("flag must round-trip in out");
    assert_ne!(product_var, flag_var, "the two outputs must stay distinct");
}

#[test]
fn linearize_emits_add_op() {
    let func = add_function();
    let ops = lower_to_simple_ir(&func);
    let has_add = ops.iter().any(|o| o.kind == "add");
    assert!(has_add, "expected an 'add' op, got: {:?}", ops);
}

#[test]
fn linearize_multi_block_emits_labels() {
    // Build: func @branch(bool) -> i64 with two successor blocks.
    let mut func = TirFunction::new("branch".into(), vec![TirType::Bool], TirType::I64);

    let bb1 = func.fresh_block();
    let bb2 = func.fresh_block();
    let v1 = func.fresh_value();
    let v2 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: bb1,
        then_args: vec![],
        else_block: bb2,
        else_args: vec![],
    };

    let mut attrs1 = AttrDict::new();
    attrs1.insert("value".into(), AttrValue::Int(1));
    func.blocks.insert(
        bb1,
        TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![v1],
                attrs: attrs1,
                source_span: None,
            }],
            terminator: Terminator::Return { values: vec![v1] },
        },
    );

    let mut attrs2 = AttrDict::new();
    attrs2.insert("value".into(), AttrValue::Int(0));
    func.blocks.insert(
        bb2,
        TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![v2],
                attrs: attrs2,
                source_span: None,
            }],
            terminator: Terminator::Return { values: vec![v2] },
        },
    );

    let ops = lower_to_simple_ir(&func);
    let kinds: Vec<&str> = ops.iter().map(|o| o.kind.as_str()).collect();
    // Simple CondBranch with both successors returning is now emitted
    // as structured if/else/end_if instead of labels + jumps.
    assert!(
        kinds.contains(&"if"),
        "expected structured 'if' op for simple CondBranch, got: {:?}",
        kinds
    );
    assert!(
        kinds.contains(&"else"),
        "expected structured 'else' op for simple CondBranch, got: {:?}",
        kinds
    );
    assert!(
        kinds.contains(&"end_if"),
        "expected structured 'end_if' op for simple CondBranch, got: {:?}",
        kinds
    );
    // Both branches should have const + ret.
    let ret_count = kinds.iter().filter(|k| **k == "ret").count();
    assert!(
        ret_count >= 2,
        "expected >=2 ret ops (one per branch), got {}: {:?}",
        ret_count,
        kinds
    );
}
