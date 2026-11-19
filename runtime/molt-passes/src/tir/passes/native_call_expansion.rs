//! The late lowering of a source extension's native callable (an `invoke_ffi`
//! with `native_callable_binding = "direct_symbol"`), written once for every
//! backend that consumes final TIR. The backend selects instructions for two
//! primitives and calls runtime entries:
//!
//! ```text
//!   %r = Call[invoke_ffi @sym, molt.object_call_v1] %a, %b
//! becomes
//!   %r = CallNative[@sym, molt.object_call_v1] %a, %b
//!
//!   %m = Call[invoke_ffi @PyInit_x, molt.pyinit_module_v1] %name
//! becomes
//!   %f = NativeSymbolAddr[@PyInit_x]
//!   %m = CallRuntime[molt_cpython_abi_run_static_extension_init] %f, %name
//!
//!   %r = Call[invoke_ffi @sym, molt.forward_f32_v1] %bytes
//! becomes
//!   %f = NativeSymbolAddr[@sym]
//!   %r = CallRuntime[molt_native_forward_f32] %f, %bytes
//! ```
//!
//! The initializer and the forwarding callable never run from compiled code:
//! their addresses cross into runtime transactions, which own hook setup,
//! scratch buffers and result validation. `CallNative` calls the symbol with
//! the machine signature its ABI token names (`molt_ir::native_callable_abi`).
//! Each rewritten call keeps the original op's attributes (exception source,
//! location), so the `CheckException` after it still reads one source;
//! `NativeSymbolAddr` cannot raise. Both primitives carry the ABI token, whose
//! machine signature the backend declares the symbol with. Every operand stays borrowed, as on the
//! `invoke_ffi` it replaces, and each result is one owned word.

use molt_ir::native_callable_abi::{
    NativeCallableAbi, NativeCallableLowering, parse_native_callable_abi,
};
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, NATIVE_ABI_ATTR, NATIVE_SYMBOL_ATTR, OpCode, TirOp};
use crate::tir::types::TirType;

const INVOKE_FFI_ATTRS: [&str; 5] = [
    "native_callable_export",
    "native_callable_binding",
    "native_callable_symbol",
    "native_callable_abi",
    "s_value",
];

fn attr_str<'a>(op: &'a TirOp, key: &str) -> Option<&'a str> {
    match op.attrs.get(key) {
        Some(AttrValue::Str(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// The direct-symbol native callable an op invokes, if it is one.
fn direct_symbol(op: &TirOp) -> Option<(String, NativeCallableAbi)> {
    if op.opcode != OpCode::Call || op.origin != Some(molt_ir::OpKind::InvokeFfi) {
        return None;
    }
    if attr_str(op, "native_callable_binding") != Some("direct_symbol") {
        return None;
    }
    let symbol = attr_str(op, "native_callable_symbol")?.to_string();
    // The TIR verifier admitted the token and the arity.
    let abi = parse_native_callable_abi(attr_str(op, "native_callable_abi")?)?;
    Some((symbol, abi))
}

/// Rewrite every direct-symbol native call of `func`.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "native_call_expansion",
        ..Default::default()
    };
    let blocks: Vec<_> = func.blocks.keys().copied().collect();
    for block in blocks {
        if !func.blocks[&block]
            .ops
            .iter()
            .any(|op| direct_symbol(op).is_some())
        {
            continue;
        }
        let ops = std::mem::take(&mut func.blocks.get_mut(&block).unwrap().ops);
        let mut rewritten = Vec::with_capacity(ops.len() + 1);
        for mut op in ops {
            let Some((symbol, abi)) = direct_symbol(&op) else {
                rewritten.push(op);
                continue;
            };
            for key in INVOKE_FFI_ATTRS {
                op.attrs.remove(key);
            }
            op.origin = None;
            // Every native callable ABI produces an owned object. A discarded
            // source destination still needs an SSA definition for the drop pass.
            if op.results.is_empty() {
                let result = func.fresh_value();
                func.value_types.insert(result, TirType::DynBox);
                op.results.push(result);
            }
            let runtime = match abi.lowering() {
                NativeCallableLowering::ObjectValues | NativeCallableLowering::ObjectCallargs => {
                    op.opcode = OpCode::CallNative;
                    op.attrs
                        .insert(NATIVE_SYMBOL_ATTR.into(), AttrValue::Str(symbol));
                    op.attrs.insert(
                        NATIVE_ABI_ATTR.into(),
                        AttrValue::Str(abi.token().to_string()),
                    );
                    rewritten.push(op);
                    stats.values_changed += 1;
                    continue;
                }
                NativeCallableLowering::PyinitModule => RuntimeFn::CpythonAbiRunStaticExtensionInit,
                NativeCallableLowering::ForwardF32 => RuntimeFn::NativeForwardF32,
            };
            let address = func.fresh_value();
            func.value_types.insert(address, TirType::I64);
            let mut take_address = TirOp::new(OpCode::NativeSymbolAddr, Vec::new(), vec![address]);
            take_address.source_span = op.source_span;
            take_address
                .attrs
                .insert(NATIVE_SYMBOL_ATTR.into(), AttrValue::Str(symbol));
            // The ABI names the symbol's machine signature, which the backend
            // declares exactly.
            take_address.attrs.insert(
                NATIVE_ABI_ATTR.into(),
                AttrValue::Str(abi.token().to_string()),
            );
            rewritten.push(take_address);
            op.opcode = OpCode::CallRuntime;
            op.runtime = Some(runtime);
            op.operands.insert(0, address);
            rewritten.push(op);
            stats.ops_added += 1;
            stats.values_changed += 1;
        }
        func.blocks.get_mut(&block).unwrap().ops = rewritten;
    }
    stats
}

#[cfg(test)]
#[path = "native_call_expansion_tests.rs"]
mod tests;
