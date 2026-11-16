use super::*;
use molt_ir::native_callable_abi::{
    NATIVE_CALLABLE_ABI_FORWARD_F32_V1, NATIVE_CALLABLE_ABI_OBJECT_CALL_V1,
    NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
};

/// `return invoke_ffi[@symbol, abi](params...)`, run through the production
/// late stage (`native_call_expansion`) and lowered.
fn lower_native_call(abi: &str, symbol: &str, params: usize) -> String {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "native_call".into(),
        vec![TirType::DynBox; params],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    let mut op = TirOp::new(
        OpCode::Call,
        (0..params as u32).map(ValueId).collect(),
        vec![result],
    );
    op.origin = Some(OpKind::InvokeFfi);
    for (key, value) in [
        ("native_callable_export", "ext.f"),
        ("native_callable_binding", "direct_symbol"),
        ("native_callable_abi", abi),
        ("native_callable_symbol", symbol),
    ] {
        op.attrs.insert(key.into(), AttrValue::Str(value.into()));
    }
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(op);
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    for function in [
        RuntimeFn::CpythonAbiRunStaticExtensionInit,
        RuntimeFn::NativeForwardF32,
    ] {
        backend.runtime_callable_symbols.insert(function);
    }
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    crate::tir::type_refine::refine_types(&mut func);
    assert!(crate::tir::pass_manager::run_late_lowering(
        &mut func, &tti, None
    ));
    backend.function_repr_facts.insert(
        func.name.clone(),
        crate::representation_plan::ProducerCarrierPlan::build(&func),
    );
    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend
        .module
        .verify()
        .expect("native call lowering verifies in LLVM");
    let declaration = backend
        .module
        .get_function(symbol)
        .expect("the extension symbol is declared")
        .print_to_string()
        .to_string();
    format!("{declaration}\n{}", llvm_fn.print_to_string())
}

#[test]
fn object_call_calls_the_extension_symbol_with_molt_values() {
    let ir = lower_native_call(NATIVE_CALLABLE_ABI_OBJECT_CALL_V1, "ext_f", 2);
    assert!(
        ir.contains("declare i64 @ext_f(i64, i64)"),
        "machine signature of molt.object_call_v1: {ir}"
    );
    assert!(ir.contains("call i64 @ext_f("), "direct call: {ir}");
}

#[test]
fn pyinit_passes_the_raw_initializer_address_to_the_runtime() {
    let ir = lower_native_call(NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1, "PyInit_ext", 1);
    let pointer = if cfg!(target_pointer_width = "64") {
        "i64"
    } else {
        "i32"
    };
    assert!(
        ir.contains(&format!("declare {pointer} @PyInit_ext()")),
        "the initializer keeps its own signature: {ir}"
    );
    assert!(
        ir.contains("ptrtoint (ptr @PyInit_ext to i64)"),
        "the address is a machine word: {ir}"
    );
    assert!(
        ir.contains_execution("molt_cpython_abi_run_static_extension_init"),
        "runtime transaction: {ir}"
    );
    assert!(
        !ir.contains("call i64 @PyInit_ext"),
        "compiled code never calls the initializer: {ir}"
    );
}

#[test]
fn forward_f32_hands_its_callable_to_the_runtime_transaction() {
    let ir = lower_native_call(NATIVE_CALLABLE_ABI_FORWARD_F32_V1, "ext_scale", 1);
    assert!(
        ir.contains("@ext_scale(i"),
        "declared with molt.forward_f32_v1's machine signature: {ir}"
    );
    assert!(
        ir.contains_execution("molt_native_forward_f32"),
        "runtime transaction: {ir}"
    );
}
