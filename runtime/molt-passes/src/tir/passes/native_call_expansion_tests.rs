use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, NATIVE_ABI_ATTR, NATIVE_SYMBOL_ATTR, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::native_callable_abi::{
    NATIVE_CALLABLE_ABI_FORWARD_F32_V1, NATIVE_CALLABLE_ABI_OBJECT_CALL_V1,
    NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
};
use molt_ir::runtime_fn_generated::RuntimeFn;

/// `return invoke_ffi[@symbol, abi](params...)`, as `backend_ir.py` emits a
/// source extension's direct-symbol call.
fn native_call(abi: &str, symbol: &str, params: usize) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "f".into(),
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
    op.origin = Some(molt_ir::OpKind::InvokeFfi);
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
    block.terminator = crate::tir::blocks::Terminator::Return {
        values: vec![result],
    };
    (func, result)
}

fn entry_ops(func: &TirFunction) -> &[TirOp] {
    &func.blocks[&func.entry_block].ops
}

fn str_attr<'a>(op: &'a TirOp, key: &str) -> Option<&'a str> {
    match op.attrs.get(key) {
        Some(AttrValue::Str(value)) => Some(value),
        _ => None,
    }
}

fn assert_no_invoke_ffi_metadata(op: &TirOp) {
    assert_eq!(op.origin, None);
    for key in [
        "native_callable_export",
        "native_callable_binding",
        "native_callable_abi",
        "native_callable_symbol",
    ] {
        assert!(!op.attrs.contains_key(key), "{key} left on {:?}", op.opcode);
    }
}

#[test]
fn object_call_becomes_a_direct_native_call_of_its_symbol() {
    let (mut func, result) = native_call(NATIVE_CALLABLE_ABI_OBJECT_CALL_V1, "ext_f", 2);
    assert!(super::run(&mut func).total_changes() > 0);
    crate::tir::verify::verify_function(&func).expect("expanded function verifies");
    let [call] = entry_ops(&func) else {
        panic!("one op: {:?}", entry_ops(&func));
    };
    assert_eq!(call.opcode, OpCode::CallNative);
    assert_eq!(call.operands, vec![ValueId(0), ValueId(1)]);
    assert_eq!(call.results, vec![result]);
    assert_eq!(str_attr(call, NATIVE_SYMBOL_ATTR), Some("ext_f"));
    assert_eq!(
        str_attr(call, NATIVE_ABI_ATTR),
        Some(NATIVE_CALLABLE_ABI_OBJECT_CALL_V1)
    );
    assert_no_invoke_ffi_metadata(call);
}

#[test]
fn address_abis_pass_the_symbol_address_to_their_runtime_transaction() {
    for (abi, runtime) in [
        (
            NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
            RuntimeFn::CpythonAbiRunStaticExtensionInit,
        ),
        (
            NATIVE_CALLABLE_ABI_FORWARD_F32_V1,
            RuntimeFn::NativeForwardF32,
        ),
    ] {
        let (mut func, result) = native_call(abi, "PyInit_ext", 1);
        assert!(super::run(&mut func).total_changes() > 0);
        crate::tir::verify::verify_function(&func).expect("expanded function verifies");
        let [address, call] = entry_ops(&func) else {
            panic!("{abi}: two ops: {:?}", entry_ops(&func));
        };
        assert_eq!(address.opcode, OpCode::NativeSymbolAddr, "{abi}");
        assert!(address.operands.is_empty());
        assert_eq!(str_attr(address, NATIVE_SYMBOL_ATTR), Some("PyInit_ext"));
        assert_eq!(str_attr(address, NATIVE_ABI_ATTR), Some(abi));
        assert_eq!(func.value_types[&address.results[0]], TirType::I64);
        assert_eq!(call.opcode, OpCode::CallRuntime, "{abi}");
        assert_eq!(call.runtime, Some(runtime), "{abi}");
        assert_eq!(call.operands, vec![address.results[0], ValueId(0)]);
        assert_eq!(call.results, vec![result]);
        assert_no_invoke_ffi_metadata(call);
    }
}

#[test]
fn a_function_without_native_calls_is_unchanged() {
    let (mut func, _) = native_call(NATIVE_CALLABLE_ABI_OBJECT_CALL_V1, "ext_f", 1);
    let call = &mut func.blocks.get_mut(&func.entry_block).unwrap().ops[0];
    call.attrs.insert(
        "native_callable_binding".into(),
        AttrValue::Str("module_attr".into()),
    );
    assert_eq!(super::run(&mut func).total_changes(), 0);
    let [call] = entry_ops(&func) else {
        panic!("one op: {:?}", entry_ops(&func));
    };
    assert_eq!(call.opcode, OpCode::Call);
    assert_eq!(call.origin, Some(molt_ir::OpKind::InvokeFfi));
}
