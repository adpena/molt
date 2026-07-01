use super::*;
use crate::llvm_backend::LlvmBackend;
use crate::llvm_backend::runtime_imports::declare_runtime_functions;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use inkwell::attributes::Attribute;
use inkwell::context::Context;
use inkwell::values::AnyValue;

fn make_backend(ctx: &Context) -> LlvmBackend<'_> {
    let backend = LlvmBackend::new(ctx, "test");
    declare_runtime_functions(ctx, &backend.module);
    backend
}

fn has_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
    let kind_id = Attribute::get_named_enum_kind_id(attr_name);
    kind_id == 0
        || func
            .get_enum_attribute(AttributeLoc::Function, kind_id)
            .is_some()
}

fn lacks_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
    let kind_id = Attribute::get_named_enum_kind_id(attr_name);
    kind_id == 0
        || func
            .get_enum_attribute(AttributeLoc::Function, kind_id)
            .is_none()
}

fn assert_lowering_error_contains(err: &LlvmLoweringError, needle: &str) {
    let joined = err.diagnostics().join("\n");
    assert!(
        joined.contains(needle),
        "expected lowering diagnostic containing {needle:?}, got:\n{joined}"
    );
}

fn const_none_def(result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn lowering_error_for_single_op(
    name: &str,
    dialect: Dialect,
    opcode: OpCode,
    operand_count: usize,
) -> (LlvmLoweringError, LlvmBackend<'static>) {
    let ctx = Box::leak(Box::new(Context::create()));
    let backend = make_backend(ctx);
    let mut func = TirFunction::new(name.into(), vec![], TirType::DynBox);
    let operands: Vec<ValueId> = (0..operand_count).map(|_| func.fresh_value()).collect();
    let result = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for &value in &operands {
            entry.ops.push(const_none_def(value));
        }
        entry.ops.push(TirOp {
            dialect,
            opcode,
            operands,
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
    }

    (
        try_lower_tir_to_llvm(&func, &backend)
            .expect_err("removed runtime delegate must fail LLVM lowering"),
        backend,
    )
}

#[test]
fn boxed_or_retains_selected_operand_result() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "boxed_or_selected_owner".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Or,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_is_truthy"), "{ir}");
    assert!(
        ir.contains("call void @molt_inc_ref_obj(i64 %bool_or)"),
        "{ir}"
    );
}

#[test]
fn removed_runtime_delegates_fail_before_phantom_imports() {
    for &(opcode, dialect, operands, symbol, message) in &[
        (
            OpCode::Yield,
            Dialect::Molt,
            1usize,
            "molt_yield",
            "explicit state-machine poll/resume",
        ),
        (
            OpCode::YieldFrom,
            Dialect::Molt,
            1,
            "molt_yield_from",
            "generator delegation",
        ),
        (OpCode::ScfIf, Dialect::Scf, 1, "molt_call_0", "LLVM CFG"),
        (
            OpCode::ScfFor,
            Dialect::Scf,
            4,
            "molt_scf_for",
            "loops into LLVM CFG",
        ),
        (
            OpCode::ScfWhile,
            Dialect::Scf,
            2,
            "molt_scf_while",
            "while regions",
        ),
        (
            OpCode::ScfYield,
            Dialect::Scf,
            1,
            "molt_scf_yield",
            "phi nodes",
        ),
    ] {
        let (err, backend) =
            lowering_error_for_single_op("removed_runtime_delegate", dialect, opcode, operands);
        assert_lowering_error_contains(&err, symbol);
        assert_lowering_error_contains(&err, message);
        assert!(
            backend.module.get_function(symbol).is_none(),
            "{symbol} must not be declared as a phantom runtime import"
        );
    }
}

#[test]
fn iterator_ops_lower_to_real_runtime_exports() {
    for &(opcode, expected, forbidden, call_name) in &[
        (
            OpCode::GetIter,
            "molt_iter_checked",
            "molt_get_iter",
            "iter_checked",
        ),
        (
            OpCode::ForIter,
            "molt_iter_next",
            "molt_for_iter",
            "for_iter_next",
        ),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(format!("iterator_{call_name}"), vec![], TirType::DynBox);
        let operand = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(operand));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![operand],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let ir = try_lower_tir_to_llvm(&func, &backend)
            .unwrap()
            .print_to_string()
            .to_string();
        assert!(ir.contains(expected), "{ir}");
        assert!(ir.contains(call_name), "{ir}");
        assert!(!ir.contains(forbidden), "{ir}");
    }
}

fn const_int_def(result: ValueId, value: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

fn make_dummy_lowering<'ctx, 'func>(
    backend: &'func LlvmBackend<'ctx>,
    func: &'func TirFunction,
    llvm_fn: FunctionValue<'ctx>,
) -> FunctionLowering<'ctx, 'func> {
    FunctionLowering {
        backend,
        func,
        llvm_fn,
        entry_trampoline_bb: None,
        block_map: HashMap::new(),
        values: HashMap::new(),
        value_types: HashMap::new(),
        pending_phis: Vec::new(),
        phi_edges: Vec::new(),
        pgo_branch_weights: None,
        pgo_weight_index: 0,
        const_str_counter: 0,
        synthetic_block_counter: 0,
        all_llvm_blocks: Vec::new(),
        llvm_pred_map: HashMap::new(),
        state_resume_blocks: HashMap::new(),
        try_stack_baselines: Vec::new(),
        call_site_counter: 0,
        diagnostics: RefCell::new(Vec::new()),
        repr_facts: crate::representation_plan::LlvmReprFacts::default(),
    }
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `same_name`")]
fn llvm_symbol_signature_mismatch_rejects_tir_forward_declaration() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    backend.module.add_function(
        "same_name",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    let func = TirFunction::new("same_name".into(), vec![], TirType::I64);

    let _ = declare_tir_function(&func, &backend);
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `molt_trace_exit`")]
fn llvm_symbol_signature_mismatch_rejects_runtime_i64_reuse() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    backend.module.add_function(
        "molt_trace_exit",
        ctx.void_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let dummy = TirFunction::new("dummy_runtime_symbol".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_symbol",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_runtime_i64_fn("molt_trace_exit", 0);
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `molt_inc_ref_obj`")]
fn llvm_symbol_signature_mismatch_rejects_runtime_void_reuse() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    backend.module.add_function(
        "molt_inc_ref_obj",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    let dummy = TirFunction::new("dummy_runtime_void_symbol".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_void_symbol",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
}

#[test]
fn on_demand_runtime_declaration_uses_conservative_attributes() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    let dummy = TirFunction::new("dummy_runtime_attrs".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_attrs",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let func = lowering.ensure_runtime_i64_fn("molt_abs_builtin", 1);

    assert!(has_fn_attr(func, "nounwind"));
    assert!(
        lacks_fn_attr(func, "willreturn"),
        "ad-hoc runtime declarations must not claim termination"
    );
}

#[test]
#[should_panic(
    expected = "LLVM runtime import `molt_unclassified_runtime_symbol` has no ABI classification"
)]
fn unclassified_runtime_declaration_rejects_new_symbol_drift() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    let dummy = TirFunction::new("dummy_runtime_reject".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_reject",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_runtime_i64_fn("molt_unclassified_runtime_symbol", 2);
}

#[test]
fn preserved_runtime_call_rejects_name_only_symbol_drift() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_intrinsic_symbols
        .insert("molt_unclassified_runtime_symbol".to_string());

    let err = lower_preserved_kind_ir(&backend, "unclassified_runtime_symbol", 2, true, None)
        .expect_err("name-only preserved runtime symbols must fail before LLVM declaration");
    assert_lowering_error_contains(&err, "has no LLVM ABI classification");
    assert_lowering_error_contains(&err, "molt_unclassified_runtime_symbol");
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `gen_fn`")]
fn llvm_symbol_signature_mismatch_rejects_function_symbol_reuse() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend.module.add_function(
        "gen_fn",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    backend
        .function_param_types
        .insert("gen_fn".to_string(), vec![TirType::DynBox, TirType::DynBox]);
    let dummy = TirFunction::new("dummy_function_symbol".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_function_symbol",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_function_symbol("gen_fn", 0, false);
}

#[test]
fn lower_const_and_return() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Build: fn f() -> i64 { return 42 }
    let mut func = TirFunction::new("const_ret".into(), vec![], TirType::I64);
    let v0 = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v0],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(42));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v0] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(ir.contains("const_ret"), "function name missing from IR");
    assert!(ir.contains("42"), "constant 42 missing from IR");
    assert!(ir.contains("ret "), "return instruction missing from IR");
}

#[test]
fn lowers_exception_pop_then_dec_ref_from_shared_drop_shape() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("exception_drop".into(), vec![], TirType::None);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(owned));
    let mut exception_pop = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    };
    exception_pop.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("exception_pop".into()),
    );
    entry.ops.push(exception_pop);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DecRef,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    let pop_pos = ir
        .find("molt_exception_pop")
        .unwrap_or_else(|| panic!("LLVM must call molt_exception_pop; IR:\n{ir}"));
    let dec_pos = ir
        .find("molt_dec_ref_obj")
        .unwrap_or_else(|| panic!("LLVM must call molt_dec_ref_obj; IR:\n{ir}"));
    assert!(
        pop_pos < dec_pos,
        "shared ExceptionRegion drops must lower after the owning exception_pop; IR:\n{ir}"
    );
}

#[test]
fn missing_value_id_is_fatal_lowering_error() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("missing_value".into(), vec![], TirType::I64);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Return {
        values: vec![ValueId(99)],
    };

    let err = match try_lower_tir_to_llvm(&func, &backend) {
        Ok(_) => panic!("malformed TIR unexpectedly lowered successfully"),
        Err(err) => err,
    };
    assert_lowering_error_contains(&err, "ValueId %99 was used before being defined");
}

#[test]
fn missing_phi_argument_is_fatal_lowering_error() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("missing_phi_arg".into(), vec![], TirType::I64);
    let join_id = func.fresh_block();
    let join_arg = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: join_id,
        args: vec![],
    };
    func.blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: join_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![join_arg],
            },
        },
    );

    let err = match try_lower_tir_to_llvm(&func, &backend) {
        Ok(_) => panic!("malformed phi unexpectedly lowered successfully"),
        Err(err) => err,
    };
    assert_lowering_error_contains(&err, "phi argument index 0 is required");
}

#[test]
fn unreachable_predecessor_does_not_feed_phi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("dead_phi_pred".into(), vec![], TirType::DynBox);
    let join_id = func.fresh_block();
    let dead_id = func.fresh_block();
    let live_value = func.fresh_value();
    let join_arg = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(live_value));
    entry.terminator = Terminator::Branch {
        target: join_id,
        args: vec![live_value],
    };
    func.blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: join_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![join_arg],
            },
        },
    );
    func.blocks.insert(
        dead_id,
        TirBlock {
            id: dead_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(999)],
            },
        },
    );

    try_lower_tir_to_llvm(&func, &backend)
        .expect("dead TIR predecessor must not contribute to LLVM phi incoming values");
    backend
        .module
        .verify()
        .expect("dead predecessor phi lowering should verify");
}

#[test]
fn check_exception_edge_feeds_handler_phi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("check_exception_phi".into(), vec![], TirType::DynBox);
    let exit_id = func.fresh_block();
    let handler_id = func.fresh_block();
    let live_value = func.fresh_value();
    let exit_value = func.fresh_value();
    let handler_arg = func.fresh_value();

    let mut handler_attrs = AttrDict::new();
    handler_attrs.insert("value".into(), AttrValue::Int(100));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(live_value));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![live_value],
        results: vec![],
        attrs: handler_attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Branch {
        target: exit_id,
        args: vec![],
    };
    func.blocks.insert(
        exit_id,
        TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![const_none_def(exit_value)],
            terminator: Terminator::Return {
                values: vec![exit_value],
            },
        },
    );
    func.blocks.insert(
        handler_id,
        TirBlock {
            id: handler_id,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![handler_arg],
            },
        },
    );
    func.has_exception_handling = true;
    func.label_id_map.insert(handler_id.0, 100);

    try_lower_tir_to_llvm(&func, &backend)
        .expect("check_exception operands must feed handler block phi args");
    backend
        .module
        .verify()
        .expect("check_exception handler phi lowering should verify");
}

/// Build the trivial `fn add(a: i64, b: i64) -> i64 { return a + b }` TIR
/// used by the overflow-safety gating tests below.
fn build_i64_add_func() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "add_i64".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
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
    (func, v_sum)
}

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
    backend
        .function_return_types
        .insert("helper_bool".to_string(), TirType::Bool);
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
    backend
        .function_return_types
        .insert("helper_f64".to_string(), TirType::F64);
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

#[test]
fn lower_call_guarded_uses_runtime_callable_dispatch_even_with_known_target() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let _target = backend.module.add_function(
        "guarded_target",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    backend
        .function_param_types
        .insert("guarded_target".to_string(), vec![TirType::DynBox]);
    backend
        .function_return_types
        .insert("guarded_target".to_string(), TirType::DynBox);

    let mut func = TirFunction::new("guarded_call_abi".into(), vec![], TirType::DynBox);
    let callable = func.fresh_value();
    let arg0 = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(callable), const_none_def(arg0)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![callable, arg0],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("call_guarded".into()),
            );
            attrs.insert("s_value".into(), AttrValue::Str("guarded_target".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(ir.contains("molt_call_func_fast1"), "{ir}");
    assert!(!ir.contains("call i64 @guarded_target"), "{ir}");
}

#[test]
fn lower_import_uses_var_attr_fallback_for_module_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("import_var_fallback".into(), vec![], TirType::DynBox);
    let imported = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut attrs = AttrDict::new();
    attrs.insert("_var".into(), AttrValue::Str("pathlib".into()));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Import,
        operands: vec![],
        results: vec![imported],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![imported],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_module_import"), "{ir}");
}

#[test]
fn lower_direct_container_builders_box_raw_i64_elements() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("container_builder_boxing".into(), vec![], TirType::DynBox);
    let raw = func.fresh_value();
    let key = func.fresh_value();
    let list = func.fresh_value();
    let tuple = func.fresh_value();
    let set = func.fresh_value();
    let dict = func.fresh_value();
    let ret = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_def(raw, 2));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![key],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("k".into()));
            attrs
        },
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildList,
        operands: vec![raw],
        results: vec![list],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildTuple,
        operands: vec![raw],
        results: vec![tuple],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildSet,
        operands: vec![raw],
        results: vec![set],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildDict,
        operands: vec![key, raw],
        results: vec![dict],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(const_none_def(ret));
    entry.terminator = Terminator::Return { values: vec![ret] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    let boxed_two = "9221401712017801218";
    assert!(
        ir.matches(boxed_two).count() >= 4,
        "each direct container builder must append boxed int bits; IR:\n{ir}"
    );
    assert!(
        !ir.contains("molt_list_builder_append(i64 %list, i64 2)"),
        "{ir}"
    );
    assert!(
        !ir.contains("molt_set_builder_append(i64 %set_builder, i64 2)"),
        "{ir}"
    );
    assert!(
        !ir.contains("molt_dict_builder_append(i64 %dict_builder, i64 %str_bits, i64 2)"),
        "{ir}"
    );
}

#[test]
fn lower_preserved_container_builders_use_void_append_abi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "preserved_container_builder_append_abi".into(),
        vec![],
        TirType::DynBox,
    );
    let raw = func.fresh_value();
    let key = func.fresh_value();
    let list = func.fresh_value();
    let tuple = func.fresh_value();
    let set = func.fresh_value();
    let dict = func.fresh_value();
    let ret = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_def(raw, 2));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![key],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("k".into()));
            attrs
        },
        source_span: None,
    });
    for (kind, operands, result) in [
        ("list_new", vec![raw], list),
        ("tuple_new", vec![raw], tuple),
        ("set_new", vec![raw], set),
        ("dict_new", vec![key, raw], dict),
    ] {
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands,
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
                attrs
            },
            source_span: None,
        });
    }
    entry.ops.push(const_none_def(ret));
    entry.terminator = Terminator::Return { values: vec![ret] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().expect("module should verify");
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call void @molt_list_builder_append"), "{ir}");
    assert!(ir.contains("call void @molt_dict_builder_append"), "{ir}");
    assert!(ir.contains("call void @molt_set_builder_append"), "{ir}");
}

#[test]
#[should_panic(expected = "call_method_ic supports at most 4 positional args")]
fn lower_call_method_ic_rejects_over_ic4_arity() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "call_method_ic_too_many_args".into(),
        vec![],
        TirType::DynBox,
    );
    let mut operands = Vec::new();
    for _ in 0..6 {
        let value = func.fresh_value();
        func.blocks
            .get_mut(&func.entry_block)
            .unwrap()
            .ops
            .push(const_none_def(value));
        operands.push(value);
    }
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethodIc,
        operands,
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("method".into(), AttrValue::Str("m".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let _ = lower_tir_to_llvm(&func, &backend);
}

#[test]
fn lower_call_method_ic_preserves_central_no_willreturn_declaration() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("call_method_ic_attr_reuse".into(), vec![], TirType::DynBox);
    let recv = func.fresh_value();
    let arg = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(recv));
    entry.ops.push(const_none_def(arg));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethodIc,
        operands: vec![recv, arg],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("method".into(), AttrValue::Str("m".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_call_method_ic1"), "{ir}");
    let runtime_fn = backend
        .module
        .get_function("molt_call_method_ic1")
        .expect("central method IC runtime import should exist");
    assert!(has_fn_attr(runtime_fn, "nounwind"));
    assert!(
        lacks_fn_attr(runtime_fn, "willreturn"),
        "method IC dispatch executes arbitrary user code"
    );
}

#[test]
#[should_panic(expected = "call_super_method_ic supports at most 4 positional args")]
fn lower_call_super_method_ic_rejects_over_ic4_arity() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "call_super_method_ic_too_many_args".into(),
        vec![],
        TirType::DynBox,
    );
    let mut operands = Vec::new();
    for _ in 0..7 {
        let value = func.fresh_value();
        func.blocks
            .get_mut(&func.entry_block)
            .unwrap()
            .ops
            .push(const_none_def(value));
        operands.push(value);
    }
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallSuperMethodIc,
        operands,
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("method".into(), AttrValue::Str("m".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let _ = lower_tir_to_llvm(&func, &backend);
}

#[test]
fn lower_class_def_boxes_raw_i64_attribute_values() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("class_def_boxed_attrs".into(), vec![], TirType::DynBox);
    let name = func.fresh_value();
    let base = func.fresh_value();
    let attr_key = func.fresh_value();
    let attr_value = func.fresh_value();
    let class_obj = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![name],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("C".into()));
            attrs
        },
        source_span: None,
    });
    entry.ops.push(const_none_def(base));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![attr_key],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("y".into()));
            attrs
        },
        source_span: None,
    });
    entry.ops.push(const_int_def(attr_value, 2));
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str("class_def".into()));
    attrs.insert("s_value".into(), AttrValue::Str("1,1,0,0,0".into()));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![name, base, attr_key, attr_value],
        results: vec![class_obj],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![class_obj],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_guarded_class_def"), "{ir}");
    assert!(
        ir.contains("9221401712017801218"),
        "class_def attr values must be boxed before array storage; IR:\n{ir}"
    );
    assert!(!ir.contains("store i64 2, ptr %class_attr_ptr_1"), "{ir}");
}

#[test]
fn lower_preserved_dict_update_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("dict_update_preserved".into(), vec![], TirType::DynBox);
    let dict_bits = func.fresh_value();
    let other_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(dict_bits), const_none_def(other_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![dict_bits, other_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("dict_update".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_dict_update"), "{ir}");
}

/// Build a single-block function whose only op is a preserved `Copy`
/// carrying `_original_kind = kind` with `n_operands` ConstNone operands and
/// (optionally) a result, then lower it and return the printed IR. Shared by
/// the preserved-op passthrough-class regressions below.
#[cfg(feature = "llvm")]
fn lower_preserved_kind_ir(
    backend: &LlvmBackend<'_>,
    kind: &str,
    n_operands: usize,
    with_result: bool,
    s_value: Option<&str>,
) -> Result<String, LlvmLoweringError> {
    let mut func = TirFunction::new(format!("preserved_{kind}"), vec![], TirType::DynBox);
    let operands: Vec<ValueId> = (0..n_operands).map(|_| func.fresh_value()).collect();
    let result = with_result.then(|| func.fresh_value());
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for &o in &operands {
        entry.ops.push(const_none_def(o));
    }
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
    if let Some(s) = s_value {
        attrs.insert("s_value".into(), AttrValue::Str(s.to_string()));
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands,
        results: result.into_iter().collect(),
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: result.into_iter().collect(),
    };
    try_lower_tir_to_llvm(&func, backend).map(|f| f.print_to_string().to_string())
}

/// The preserved-op passthrough-class closure: each kind that previously
/// fell to the `Copy` operand-0 passthrough (a silent miscompile / dropped
/// side effect) must now lower to its dedicated runtime call. This pins the
/// specific dedicated arms whose runtime symbol DIFFERS from `molt_<kind>`
/// (so the generic fallback would have declined) or which are result-less.
#[test]
fn lower_preserved_passthrough_class_routes_to_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    // (kind, n_operands, with_result, s_value, expected runtime symbol)
    let cases: &[(&str, usize, bool, Option<&str>, &str)] = &[
        ("abs", 1, true, None, "molt_abs_builtin"),
        ("const_ellipsis", 0, true, None, "molt_ellipsis"),
        (
            "const_not_implemented",
            0,
            true,
            None,
            "molt_not_implemented",
        ),
        ("gen_throw", 2, true, None, "molt_generator_throw"),
        ("gen_close", 1, true, None, "molt_generator_close"),
        (
            "exception_set_cause",
            2,
            false,
            None,
            "molt_exception_set_cause",
        ),
        (
            "get_attr_special_obj",
            1,
            true,
            Some("__class__"),
            "molt_get_attr_special",
        ),
        ("borrow", 1, true, None, "molt_inc_ref_obj"),
        ("identity_alias", 1, true, None, "molt_inc_ref_obj"),
        ("binding_alias", 1, true, None, "molt_inc_ref_obj"),
        ("release", 1, true, None, "molt_dec_ref_obj"),
        ("guard_tag", 2, false, None, "molt_guard_type"),
        ("guard_layout", 3, true, None, "molt_guard_layout_ptr"),
        ("guard_dict_shape", 3, true, None, "molt_guard_layout_ptr"),
        ("dataclass_new", 4, true, None, "molt_dataclass_new"),
        ("json_parse", 1, true, None, "molt_json_parse_scalar_obj"),
        (
            "msgpack_parse",
            1,
            true,
            None,
            "molt_msgpack_parse_scalar_obj",
        ),
        ("cbor_parse", 1, true, None, "molt_cbor_parse_scalar_obj"),
        (
            "gen_locals_register",
            2,
            false,
            Some("gen_fn"),
            "molt_gen_locals_register",
        ),
    ];
    for &(kind, nops, with_result, s_value, sym) in cases {
        let ir = lower_preserved_kind_ir(&backend, kind, nops, with_result, s_value)
            .unwrap_or_else(|e| {
                panic!(
                    "preserved `{kind}` must lower, got error: {:?}",
                    e.diagnostics()
                )
            });
        assert!(
            ir.contains(sym),
            "preserved `{kind}` must lower to `{sym}` (not an operand-0 \
                 passthrough); IR:\n{ir}"
        );
    }
}

/// Repr-identity preserved ops (`cast`, `widen`, `store_var`, `copy_var`) are the
/// explicit exception to the terminal preserved-op fail-loud rule: they
/// carry no runtime semantics and must alias operand 0 exactly, matching
/// native/WASM identity lowering over the NaN-boxed value format.
#[test]
fn lower_preserved_repr_identity_ops_pass_operand_through() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    for kind in ["cast", "widen", "store_var", "copy_var"] {
        let mut func = TirFunction::new(
            format!("preserved_{kind}_identity"),
            vec![TirType::DynBox],
            TirType::DynBox,
        );
        let src = func
            .blocks
            .get(&func.entry_block)
            .and_then(|block| block.args.first())
            .map(|arg| arg.id)
            .expect("identity test function must have one entry argument");
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![src],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let ir = try_lower_tir_to_llvm(&func, &backend)
            .map(|f| f.print_to_string().to_string())
            .unwrap_or_else(|e| {
                panic!(
                    "repr-identity preserved `{kind}` must lower as operand-0 \
                         passthrough, got error: {:?}",
                    e.diagnostics()
                )
            });
        assert!(
            !ir.contains("call "),
            "repr-identity preserved `{kind}` must not lower through a runtime call:\n{ir}"
        );
        assert!(
            ir.contains("ret i64 %0"),
            "repr-identity preserved `{kind}` must return operand 0 exactly:\n{ir}"
        );
    }
}

/// Terminal fail-loud state: a preserved `Copy` carrying an `_original_kind`
/// that NO arm and NO `molt_<kind>` runtime intrinsic claims must be a hard
/// `record_fatal` lowering error — never a silent operand-0 passthrough.
/// `__ppaudit_unmapped__` is a synthetic kind that cannot resolve to any
/// `molt_*` symbol, so it must reach the terminal guard.
#[test]
fn lower_preserved_unmapped_kind_fails_loud() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let err = lower_preserved_kind_ir(&backend, "__ppaudit_unmapped__", 1, true, None).expect_err(
        "an unhandled preserved op must fail the lowering, not silently \
                 pass operand 0 through",
    );
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "__ppaudit_unmapped__");
}

/// RESULT-LESS preserved side-effect ops (`print_newline`, `set_update`,
/// `dict_str_int_inc`, …) whose `molt_<kind>` symbol IS in the linked
/// intrinsic surface must lower to that runtime call via the generic
/// fallback — NOT be dropped as a `Copy` "0 results → no-op". The
/// passthrough enumeration found these reaching the no-op branch (a missing
/// newline / a set or dict mutation that never happened). This pins the
/// result-less generic-fallback path; the symbols are injected because the
/// unit-test backend has an empty intrinsic surface by default.
#[test]
fn lower_preserved_resultless_side_effect_routes_to_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    // (kind, n_operands, expected runtime symbol). All result-less (res=0).
    let cases: &[(&str, usize, &str)] = &[
        ("print_newline", 0, "molt_print_newline"),
        ("set_update", 2, "molt_set_update"),
        ("dict_str_int_inc", 3, "molt_dict_str_int_inc"),
        ("spawn", 1, "molt_spawn"),
    ];
    for &(_, _, sym) in cases {
        backend.runtime_intrinsic_symbols.insert(sym.to_string());
    }
    for &(kind, nops, sym) in cases {
        let ir = lower_preserved_kind_ir(&backend, kind, nops, false, None).unwrap_or_else(|e| {
            panic!(
                "result-less preserved `{kind}` must lower, got error: {:?}",
                e.diagnostics()
            )
        });
        assert!(
            ir.contains(sym),
            "result-less preserved `{kind}` must lower to `{sym}` (not a \
                 dropped no-op); IR:\n{ir}"
        );
        if sym == "molt_print_newline" {
            assert!(
                ir.contains("call void @molt_print_newline()"),
                "print_newline must use the runtime's void ABI; IR:\n{ir}"
            );
        }
    }
}

#[test]
fn lower_preserved_chan_new_uses_dedicated_handle_lowering() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let ir = lower_preserved_kind_ir(&backend, "chan_new", 1, true, None).unwrap_or_else(|e| {
        panic!(
            "chan_new returns an opaque channel handle and must lower through \
             its dedicated LLVM arm, got error: {:?}",
            e.diagnostics()
        )
    });
    assert!(
        ir.contains("call i64 @molt_chan_new(i64"),
        "chan_new must call the centrally declared handle constructor; IR:\n{ir}"
    );
}

#[test]
fn lower_preserved_void_runtime_result_shape_fails_loud() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_intrinsic_symbols
        .insert("molt_spawn".to_string());
    let err = lower_preserved_kind_ir(&backend, "spawn", 1, true, None)
        .expect_err("void preserved runtime ops must not bind a boxed result");
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "spawn");
}

/// The dual safety check: a result-less preserved op whose `molt_<kind>`
/// symbol is ABSENT from the intrinsic surface must STILL fail loud (never a
/// silent dropped side effect). Without the symbol the generic fallback
/// declines and the terminal guard must fire.
#[test]
fn lower_preserved_resultless_unmapped_fails_loud() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let err = lower_preserved_kind_ir(&backend, "__ppaudit_resultless__", 2, false, None)
        .expect_err("an unhandled result-less preserved op must fail the lowering");
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "__ppaudit_resultless__");
}

/// A bare `Copy` (no `_original_kind` — a genuine SSA value copy such as
/// `copy`/`load_var`/`store_var`) must STILL take the benign operand-0
/// passthrough. The terminal fail-loud guard keys on `_original_kind`, so it
/// must not fire here.
#[test]
fn lower_bare_copy_without_original_kind_passes_through() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("bare_copy".into(), vec![], TirType::DynBox);
    let src = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(src));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![src],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    // Must lower cleanly (no fatal); the result aliases the source.
    let ir = try_lower_tir_to_llvm(&func, &backend)
        .map(|f| f.print_to_string().to_string())
        .expect("a bare Copy without _original_kind must lower as a passthrough");
    assert!(
        !ir.contains("unhandled preserved"),
        "bare Copy must not trigger the preserved-op fail-loud: {ir}"
    );
}

#[test]
fn lower_preserved_len_ignores_transport_container_type() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("len_preserved".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(obj));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
            attrs.insert("container_type".into(), AttrValue::Str("tuple".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call i64 @molt_len("), "{ir}");
    assert!(!ir.contains("call i64 @molt_len_tuple("), "{ir}");
}

#[test]
fn lower_preserved_len_uses_tir_tuple_fact() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "len_typed_tuple".into(),
        vec![TirType::Tuple(vec![TirType::DynBox, TirType::DynBox])],
        TirType::DynBox,
    );
    let obj = ValueId(0);
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call i64 @molt_len_tuple("), "{ir}");
}

#[test]
fn lower_preserved_list_append_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("list_append_preserved".into(), vec![], TirType::DynBox);
    let list_bits = func.fresh_value();
    let item_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(list_bits), const_none_def(item_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits, item_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("list_append".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_list_append"), "{ir}");
}

#[test]
fn lower_del_boundary_calls_dec_ref_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::DynBox);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(owned));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelBoundary,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_dec_ref_obj"), "{ir}");
}

#[test]
fn lower_preserved_list_pop_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_intrinsic_symbols
        .insert("molt_list_pop".to_string());
    let ir = lower_preserved_kind_ir(&backend, "list_pop", 2, true, None)
        .expect("list_pop must lower through the boxed runtime call");
    assert!(ir.contains("molt_list_pop"), "{ir}");
}

#[test]
fn lower_preserved_dataclass_new_values_calls_runtime_with_value_slice() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let ir = lower_preserved_kind_ir(&backend, "dataclass_new_values", 5, true, None)
        .expect("dataclass_new_values must lower through its value-slice runtime call");
    assert!(ir.contains("molt_dataclass_new_from_values"), "{ir}");
    assert!(ir.contains("alloca i64, i64 2"), "{ir}");
}

#[test]
fn lower_preserved_tuple_from_list_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("tuple_from_list_preserved".into(), vec![], TirType::DynBox);
    let list_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(list_bits));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("tuple_from_list".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_tuple_from_list"), "{ir}");
}

#[test]
fn lower_preserved_set_add_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("set_add_preserved".into(), vec![], TirType::DynBox);
    let set_bits = func.fresh_value();
    let item_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(set_bits), const_none_def(item_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![set_bits, item_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("set_add".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_set_add"), "{ir}");
}

#[test]
fn lower_preserved_list_extend_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("list_extend_preserved".into(), vec![], TirType::DynBox);
    let list_bits = func.fresh_value();
    let other_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(list_bits), const_none_def(other_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits, other_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("list_extend".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_list_extend"), "{ir}");
}

#[test]
fn lower_preserved_aiter_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("aiter_preserved".into(), vec![], TirType::DynBox);
    let obj_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(obj_bits));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("aiter".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_aiter"), "{ir}");
}

#[test]
fn lower_preserved_gen_send_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("gen_send_preserved".into(), vec![], TirType::DynBox);
    let gen_bits = func.fresh_value();
    let send_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(gen_bits), const_none_def(send_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![gen_bits, send_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("gen_send".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_generator_send"), "{ir}");
}

#[test]
fn lower_preserved_context_exit_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("context_exit_preserved".into(), vec![], TirType::DynBox);
    let ctx_bits = func.fresh_value();
    let exc_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(ctx_bits), const_none_def(exc_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![ctx_bits, exc_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("context_exit".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_context_exit"), "{ir}");
}

#[test]
fn lower_preserved_super_new_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("super_new_preserved".into(), vec![], TirType::DynBox);
    let type_bits = func.fresh_value();
    let obj_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(type_bits), const_none_def(obj_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![type_bits, obj_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("super_new".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_super_new"), "{ir}");
}

#[test]
fn lower_dynamic_get_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_get_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::LoadAttr,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("get_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_get_attr_name"), "{ir}");
    assert!(ir.contains("i64 %0, i64 %1"), "{ir}");
}

#[test]
fn lower_dynamic_set_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_set_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreAttr,
        operands: vec![ValueId(0), ValueId(1), ValueId(2)],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("set_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_set_attr_name"), "{ir}");
    assert!(ir.contains("i64 %0, i64 %1, i64 %2"), "{ir}");
}

#[test]
fn lower_dynamic_del_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_del_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelAttr,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("del_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_del_attr_name"), "{ir}");
    assert!(ir.contains("i64 %0, i64 %1"), "{ir}");
}

#[test]
fn lower_preserved_has_attr_name_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("has_attr_name_preserved".into(), vec![], TirType::DynBox);
    let obj_bits = func.fresh_value();
    let name_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(obj_bits), const_none_def(name_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj_bits, name_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("has_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_has_attr_name"), "{ir}");
}

#[test]
fn lower_call_method_uses_call_bind_ic_abi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("call_method_abi".into(), vec![], TirType::DynBox);
    let callable = func.fresh_value();
    let arg0 = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(callable), const_none_def(arg0)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethod,
        operands: vec![callable, arg0],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_call_bind_ic"), "{ir}");
    assert!(!ir.contains("molt_call_method"), "{ir}");
}

#[test]
fn lower_call_bind_preserves_callargs_builder_abi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("call_bind_abi".into(), vec![], TirType::DynBox);
    let callable = func.fresh_value();
    let builder = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(callable), const_none_def(builder)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![callable, builder],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_call_bind_ic"), "{ir}");
}

#[test]
fn lower_i64_comparison() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    // Build: fn lt(a: i64, b: i64) -> bool { return a < b }
    let mut func = TirFunction::new(
        "cmp_lt".into(),
        vec![TirType::I64, TirType::I64],
        TirType::Bool,
    );
    let v_result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Lt,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![v_result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_result],
    };

    // The raw `icmp slt` path needs both operands proven exact-i64 carriers;
    // an unproven `i64` parameter carries boxed (`DynBox`) and dispatches the
    // comparison through the runtime. Prove the two parameters here.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    for v in [ValueId(0), ValueId(1)] {
        facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("icmp slt"),
        "expected signed less-than comparison in IR: {}",
        ir
    );
}

#[test]
fn lower_box_i64() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    // Build: fn box_it(x: i64) -> DynBox { return box(x) }
    let mut func = TirFunction::new("box_i64".into(), vec![TirType::I64], TirType::DynBox);
    let v_boxed = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BoxVal,
        operands: vec![ValueId(0)],
        results: vec![v_boxed],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_boxed],
    };

    // `box(x)` emits the NaN-boxing arithmetic only when `x` is a RAW i64.
    // An unproven `i64` parameter carries already-boxed (`DynBox`), for which
    // `box` is a no-op; prove the parameter raw so the box path is exercised.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    facts
        .repr_by_value
        .insert(ValueId(0), crate::Repr::RawI64Safe);
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    // Should contain the NaN-boxing OR operations
    assert!(
        ir.contains("or i64"),
        "expected NaN-boxing OR in IR: {}",
        ir
    );
    assert!(
        ir.contains("and i64"),
        "expected NaN-boxing AND mask in IR: {}",
        ir
    );
}

#[test]
fn masked_shift_loop_phi_promoted_to_raw_i64_lane() {
    // #43 end-to-end (the perf payoff the value-range phi narrowing exists
    // for): a `DynBox`-declared loop-header phi that the representation plan
    // proves `RawI64Safe` must be carried as a raw `I64` so the in-loop
    // `<<`/`&` emit raw machine `shl`/`and` instead of the boxed
    // `molt_lshift`/`molt_bit_and` runtime. `type_refine` leaves the masked
    // accumulator `DynBox` (its inline-window fit is a value-range-only fact),
    // so without `effective_block_arg_type`'s DynBox->I64 promotion the phi
    // carries boxed and every iteration round-trips through the runtime — the
    // exact regression this guards.
    //
    // Shape:  s_phi: DynBox = phi[ 1 (preheader), band (back-edge) ]
    //         shl  = s_phi << 1
    //         band = shl & MASK            (MASK = 2**32 - 1)
    //         -> header(band)
    // with the plan proving s_phi / shl / band all RawI64Safe.
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    let mut func = TirFunction::new("masked_shift".into(), vec![], TirType::None);
    let s_start = func.fresh_value(); // ConstInt 1
    let mask_c = func.fresh_value(); // ConstInt (2**32 - 1)
    let one_c = func.fresh_value(); // ConstInt 1 (shift count)
    let s_phi = func.fresh_value(); // header phi (DynBox-declared)
    let shl = func.fresh_value(); // s_phi << 1
    let band = func.fresh_value(); // shl & MASK

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();

    let mk_int = |result: ValueId, v: i64| TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(v));
            m
        },
        source_span: None,
    };
    let mk_bin = |opcode: OpCode, a: ValueId, b: ValueId, r: ValueId| TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![a, b],
        results: vec![r],
        attrs: AttrDict::new(),
        source_span: None,
    };
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            mk_int(s_start, 1),
            mk_int(mask_c, (1i64 << 32) - 1),
            mk_int(one_c, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![s_start],
        };
    }
    // The phi is DECLARED DynBox (as type_refine leaves the masked accumulator).
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: s_phi,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: body,
                args: vec![],
            },
        },
    );
    func.loop_roles
        .insert(header, crate::tir::blocks::LoopRole::LoopHeader);
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                mk_bin(OpCode::Shl, s_phi, one_c, shl),
                mk_bin(OpCode::BitAnd, shl, mask_c, band),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![band],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles
        .insert(exit, crate::tir::blocks::LoopRole::LoopEnd);

    // The plan proves the masked accumulator chain RawI64Safe (what the
    // value-range phi narrowing yields end to end). The ConstInts are I64 by
    // their own lowering; the proof here is for the phi + the two op results.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    for v in [s_phi, shl, band] {
        facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("shl i64"),
        "masked accumulator shift must lower to a RAW machine `shl i64`, not \
             the boxed runtime. IR:\n{ir}"
    );
    assert!(
        !ir.contains("@molt_lshift"),
        "a RawI64Safe-proven masked shift must NOT call the boxed `molt_lshift`. \
             IR:\n{ir}"
    );
    assert!(
        !ir.contains("@molt_bit_and"),
        "a RawI64Safe-proven masked `& MASK` must NOT call the boxed \
             `molt_bit_and`. IR:\n{ir}"
    );
    // The header phi must be a raw `i64` phi (promoted from its DynBox
    // declaration) so the back-edge carries the raw masked value.
    assert!(
        ir.contains("phi i64"),
        "the RawI64Safe masked accumulator phi must be a raw `i64` phi. IR:\n{ir}"
    );
}

// ── RPO algorithm tests ──
//
// The RPO algorithm is exercised end-to-end by the integration tests in
// `runtime/molt-backend/tests/llvm_rpo.rs`, which call into
// [`super::compute_function_rpo`] directly with synthetic CFGs covering
// diamonds, loops, switches, deep chains, self-loops, and unreachable
// blocks. Those tests live in a separate test binary and so are not
// blocked by drift in the wider lib test suite.

/// Helper: build a function with `num_blocks` empty blocks (terminators
/// initialized to `Unreachable`; tests overwrite them).
fn make_func_with_blocks(name: &str, num_blocks: u32) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], TirType::I64);
    for _ in 1..num_blocks {
        let bid = func.fresh_block();
        func.blocks.insert(
            bid,
            TirBlock {
                id: bid,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );
    }
    func
}

fn set_term(func: &mut TirFunction, b: BlockId, term: Terminator) {
    func.blocks.get_mut(&b).unwrap().terminator = term;
}

fn position_of(rpo: &[BlockId], b: BlockId) -> usize {
    rpo.iter()
        .position(|x| *x == b)
        .unwrap_or_else(|| panic!("BlockId {:?} not present in RPO {:?}", b, rpo))
}

#[test]
fn rpo_diamond_cfg_orders_entry_first_then_arms_then_merge() {
    // CFG:
    //   entry -> A, B   (cond branch)
    //   A     -> merge
    //   B     -> merge
    //   merge -> return
    //
    // Valid RPOs: [entry, A, B, merge] OR [entry, B, A, merge].
    let mut func = make_func_with_blocks("diamond", 4);
    let entry = func.entry_block; // BlockId(0)
    let a = BlockId(1);
    let b = BlockId(2);
    let merge = BlockId(3);

    // We allocate ValueId(0) as the conditional value. We never actually
    // evaluate it — RPO walks terminators, not ops.
    let cond = func.fresh_value();
    set_term(
        &mut func,
        entry,
        Terminator::CondBranch {
            cond,
            then_block: a,
            then_args: vec![],
            else_block: b,
            else_args: vec![],
        },
    );
    set_term(
        &mut func,
        a,
        Terminator::Branch {
            target: merge,
            args: vec![],
        },
    );
    set_term(
        &mut func,
        b,
        Terminator::Branch {
            target: merge,
            args: vec![],
        },
    );
    set_term(&mut func, merge, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(
        rpo.len(),
        4,
        "all four blocks must appear in RPO: {:?}",
        rpo
    );
    assert_eq!(rpo[0], entry, "entry must be first: {:?}", rpo);
    assert_eq!(rpo[3], merge, "merge must be last: {:?}", rpo);

    let pos_entry = position_of(&rpo, entry);
    let pos_a = position_of(&rpo, a);
    let pos_b = position_of(&rpo, b);
    let pos_merge = position_of(&rpo, merge);

    assert!(pos_entry < pos_a, "entry must precede A: {:?}", rpo);
    assert!(pos_entry < pos_b, "entry must precede B: {:?}", rpo);
    assert!(pos_a < pos_merge, "A must precede merge: {:?}", rpo);
    assert!(pos_b < pos_merge, "B must precede merge: {:?}", rpo);

    // The two valid orderings are exactly these two.
    let valid_a_first = rpo == vec![entry, a, b, merge];
    let valid_b_first = rpo == vec![entry, b, a, merge];
    assert!(
        valid_a_first || valid_b_first,
        "RPO must be one of the two valid diamond orderings, got {:?}",
        rpo
    );
}

#[test]
fn rpo_simple_loop_orders_entry_before_header_before_body() {
    // CFG:
    //   entry  -> header
    //   header -> body, exit  (cond branch)
    //   body   -> header      (back-edge — does NOT change RPO order)
    //   exit   -> return
    //
    // Required: entry < header < body in RPO. The back-edge body->header
    // is the only edge that runs "backwards" in the resulting layout.
    let mut func = make_func_with_blocks("loop", 4);
    let entry = func.entry_block; // BlockId(0)
    let header = BlockId(1);
    let body = BlockId(2);
    let exit = BlockId(3);

    let cond = func.fresh_value();
    set_term(
        &mut func,
        entry,
        Terminator::Branch {
            target: header,
            args: vec![],
        },
    );
    set_term(
        &mut func,
        header,
        Terminator::CondBranch {
            cond,
            then_block: body,
            then_args: vec![],
            else_block: exit,
            else_args: vec![],
        },
    );
    set_term(
        &mut func,
        body,
        Terminator::Branch {
            target: header,
            args: vec![],
        },
    );
    set_term(&mut func, exit, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(
        rpo.len(),
        4,
        "all four blocks must appear in RPO: {:?}",
        rpo
    );

    let pos_entry = position_of(&rpo, entry);
    let pos_header = position_of(&rpo, header);
    let pos_body = position_of(&rpo, body);
    let pos_exit = position_of(&rpo, exit);

    assert_eq!(pos_entry, 0, "entry must be first: {:?}", rpo);
    assert!(
        pos_entry < pos_header,
        "entry must precede header: {:?}",
        rpo
    );
    assert!(
        pos_header < pos_body,
        "header must precede body (back-edge does not flip order): {:?}",
        rpo
    );
    assert!(
        pos_header < pos_exit,
        "header must precede exit (then is forward edge): {:?}",
        rpo
    );
}

#[test]
fn rpo_unreachable_blocks_are_excluded() {
    // CFG:
    //   entry -> exit (return)
    //   dead  -> return  (no predecessor — unreachable)
    let mut func = make_func_with_blocks("dead_block", 3);
    let entry = func.entry_block;
    let exit = BlockId(1);
    let dead = BlockId(2);

    set_term(
        &mut func,
        entry,
        Terminator::Branch {
            target: exit,
            args: vec![],
        },
    );
    set_term(&mut func, exit, Terminator::Return { values: vec![] });
    set_term(&mut func, dead, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo, vec![entry, exit]);
    assert!(
        !rpo.contains(&dead),
        "unreachable block must be excluded from RPO: {:?}",
        rpo
    );
}

#[test]
fn rpo_switch_terminator_visits_all_cases_and_default() {
    // CFG:
    //   entry -> switch on v: case 0 -> A, case 1 -> B, default -> C
    //   A, B, C -> merge -> return
    let mut func = make_func_with_blocks("switch_cfg", 5);
    let entry = func.entry_block;
    let a = BlockId(1);
    let b = BlockId(2);
    let c = BlockId(3);
    let merge = BlockId(4);

    let v = func.fresh_value();
    set_term(
        &mut func,
        entry,
        Terminator::Switch {
            value: v,
            cases: vec![(0, a, vec![]), (1, b, vec![])],
            default: c,
            default_args: vec![],
        },
    );
    for case_block in [a, b, c] {
        set_term(
            &mut func,
            case_block,
            Terminator::Branch {
                target: merge,
                args: vec![],
            },
        );
    }
    set_term(&mut func, merge, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo.len(), 5, "all five blocks must appear: {:?}", rpo);
    assert_eq!(rpo[0], entry);
    assert_eq!(rpo[4], merge);
    for case_block in [a, b, c] {
        let p = position_of(&rpo, case_block);
        assert!(p > 0, "case block must follow entry");
        assert!(p < 4, "case block must precede merge");
    }
}

#[test]
fn rpo_deeply_chained_cfg_does_not_overflow_stack() {
    // Build a chain of 5,000 blocks: entry -> b1 -> b2 -> ... -> b4999 -> return.
    // The original recursive implementation overflowed at this depth on
    // default thread stack sizes; the iterative version handles it
    // without issue.
    const N: u32 = 5_000;
    let mut func = make_func_with_blocks("deep_chain", N);
    for i in 0..N - 1 {
        set_term(
            &mut func,
            BlockId(i),
            Terminator::Branch {
                target: BlockId(i + 1),
                args: vec![],
            },
        );
    }
    set_term(
        &mut func,
        BlockId(N - 1),
        Terminator::Return { values: vec![] },
    );

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo.len(), N as usize);
    for (i, bid) in rpo.iter().enumerate() {
        assert_eq!(
            *bid,
            BlockId(i as u32),
            "deep chain RPO must be entry, b1, b2, ... in order"
        );
    }
}

#[test]
fn rpo_terminator_successor_helper_preserves_order() {
    // The order in which `append_terminator_successors` records successors
    // is part of the algorithm's contract: it determines tie-breaking
    // when multiple valid RPOs exist. Pin it explicitly.
    let mut buf = Vec::new();

    buf.clear();
    append_terminator_successors(
        &Terminator::Branch {
            target: BlockId(7),
            args: vec![],
        },
        &mut buf,
    );
    assert_eq!(buf, vec![BlockId(7)]);

    buf.clear();
    append_terminator_successors(
        &Terminator::CondBranch {
            cond: ValueId(0),
            then_block: BlockId(11),
            then_args: vec![],
            else_block: BlockId(13),
            else_args: vec![],
        },
        &mut buf,
    );
    assert_eq!(
        buf,
        vec![BlockId(11), BlockId(13)],
        "then must precede else"
    );

    buf.clear();
    append_terminator_successors(
        &Terminator::Switch {
            value: ValueId(0),
            cases: vec![(0, BlockId(20), vec![]), (1, BlockId(21), vec![])],
            default: BlockId(22),
            default_args: vec![],
        },
        &mut buf,
    );
    assert_eq!(
        buf,
        vec![BlockId(20), BlockId(21), BlockId(22)],
        "switch cases in declaration order, then default"
    );

    buf.clear();
    append_terminator_successors(&Terminator::Return { values: vec![] }, &mut buf);
    assert!(buf.is_empty(), "Return has no successors");

    buf.clear();
    append_terminator_successors(&Terminator::Unreachable, &mut buf);
    assert!(buf.is_empty(), "Unreachable has no successors");
}
