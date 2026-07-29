use super::*;

#[test]
fn lower_i64_add_overflow_safe_uses_native_add() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    // Build: fn add(a: i64, b: i64) -> i64 { return a + b }, with the result
    // marked overflow-safe by the representation plan. The backend may then
    // emit a raw machine `add` instead of routing through the runtime.
    let (func, v_sum) = build_i64_add_func();
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    // A native machine `add` is sound only when BOTH operands and the result
    // are value-range-proven exact-i64 carriers. The two `i64` parameters
    // (entry args %0/%1) carry as boxed `DynBox` unless proven overflow-safe
    // (the parameter-ABI carrier rule), so prove all three here — the
    // realistic shape under which the plan admits raw machine arithmetic.
    for v in [ValueId(0), ValueId(1), v_sum] {
        facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("add i64"),
        "expected native i64 add for an overflow-safe result: {}",
        ir
    );
    assert!(
        !ir.contains("call") || !ir.contains("molt_add"),
        "overflow-safe i64+i64 add must NOT call the runtime: {}",
        ir
    );
}

#[test]
fn lower_i64_add_not_overflow_safe_routes_to_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Same add, but with NO overflow-safety proof (empty plan facts). The
    // structural fix for the LLVM int-overflow miscompile requires this to
    // route through `molt_add` (BigInt-correct) rather than emit a raw
    // machine `add` that would silently wrap and truncate at box time.
    let (func, _v_sum) = build_i64_add_func();

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("call i64 @molt_add"),
        "non-overflow-safe i64+i64 add must route through molt_add: {}",
        ir
    );
}

#[test]
fn lower_f64_add() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Build: fn fadd(a: f64, b: f64) -> f64 { return a + b }
    let mut func = TirFunction::new(
        "add_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
    );
    let v_sum = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![v_sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_sum],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("fadd double"),
        "expected native f64 add in IR: {}",
        ir
    );
}

#[test]
fn lower_dynbox_add_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Build: fn dyn_add(a: DynBox, b: DynBox) -> DynBox { return a + b }
    let mut func = TirFunction::new(
        "dyn_add".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let v_sum = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![v_sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_sum],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("molt_add"),
        "expected runtime call to molt_add in IR: {}",
        ir
    );
}

#[test]
fn lower_conditional_branch() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Build: fn cond(flag: Bool) -> i64 { if flag: return 1 else: return 0 }
    let mut func = TirFunction::new("cond_branch".into(), vec![TirType::Bool], TirType::I64);

    let then_id = func.fresh_block();
    let else_id = func.fresh_block();
    let v_one = func.fresh_value();
    let v_zero = func.fresh_value();

    // Entry: cond branch on param 0
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_id,
        then_args: vec![],
        else_block: else_id,
        else_args: vec![],
    };

    // Then block: return 1
    func.blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![v_one],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(1));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![v_one],
            },
        },
    );

    // Else block: return 0
    func.blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![v_zero],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(0));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![v_zero],
            },
        },
    );

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    // Should have 3 blocks and a conditional branch
    assert!(
        ir.contains("br i1"),
        "expected conditional branch in IR: {}",
        ir
    );
    assert!(ir.contains("bb1"), "expected then block in IR: {}", ir);
    assert!(ir.contains("bb2"), "expected else block in IR: {}", ir);
}

#[test]
fn plain_trampoline_boxes_bool_return_into_i64_abi() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let _target = backend.module.add_function(
        "helper_bool",
        ctx.bool_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    backend.function_linkage_abis.insert(
        "helper_bool".to_string(),
        test_native_linkage_abi(vec![], Some(TirType::Bool)),
    );
    let dummy = TirFunction::new("dummy".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);
    let trampoline = lowering.ensure_plain_trampoline("helper_bool", 0, false);

    assert_eq!(
        trampoline.get_type().get_return_type(),
        Some(ctx.i64_type().into())
    );
    backend.module.verify().expect("llvm module should verify");
    let ir = trampoline.print_to_string().to_string();
    assert!(ir.contains("box_bool") || ir.contains("zext_bool"), "{ir}");
    assert!(ir.contains("ret i64"), "{ir}");
}

#[test]
fn plain_trampoline_boxes_f64_return_into_i64_abi() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let _target = backend.module.add_function(
        "helper_f64",
        ctx.f64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    backend.function_linkage_abis.insert(
        "helper_f64".to_string(),
        test_native_linkage_abi(vec![], Some(TirType::F64)),
    );
    let dummy = TirFunction::new("dummy".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);
    let trampoline = lowering.ensure_plain_trampoline("helper_f64", 0, false);

    assert_eq!(
        trampoline.get_type().get_return_type(),
        Some(ctx.i64_type().into())
    );
    backend.module.verify().expect("llvm module should verify");
    let ir = trampoline.print_to_string().to_string();
    assert!(
        ir.contains("f64_to_i64") || ir.contains("bitcast double"),
        "{ir}"
    );
    assert!(ir.contains("fcmp uno"), "{ir}");
    assert!(ir.contains("ret i64"), "{ir}");
}
