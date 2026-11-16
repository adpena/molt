//! Instruction selection for a source extension's native callable, the two
//! late-TIR primitives `native_call_expansion` writes: the address of the
//! extension's symbol, and a direct call of the symbol under the machine
//! signature its ABI token names. The meaning lives in that expansion; this
//! file only declares the symbol and emits the instruction.

use super::*;
use crate::tir::ops::{NATIVE_ABI_ATTR, NATIVE_SYMBOL_ATTR};
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum};
use inkwell::values::FunctionValue;
use molt_ir::native_callable_abi::{NativeCallableMachineType, parse_native_callable_abi};

fn native_attr<'a>(op: &'a TirOp, key: &str) -> &'a str {
    match op.attrs.get(key) {
        Some(AttrValue::Str(value)) if !value.is_empty() => value.as_str(),
        _ => panic!("native_call_expansion gives {:?} its `{key}`", op.opcode),
    }
}

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    fn native_machine_type(&self, machine: NativeCallableMachineType) -> BasicTypeEnum<'ctx> {
        match machine {
            NativeCallableMachineType::MoltValue | NativeCallableMachineType::U64 => {
                self.backend.context.i64_type().into()
            }
            NativeCallableMachineType::Pointer => self.backend.pointer_sized_int_type().into(),
            NativeCallableMachineType::I32 => self.backend.context.i32_type().into(),
        }
    }

    /// The extension's symbol, declared once per module. The final link
    /// resolves it from the extension's archive or object.
    fn native_symbol(
        &self,
        symbol: &str,
        params: &[NativeCallableMachineType],
        result: NativeCallableMachineType,
    ) -> FunctionValue<'ctx> {
        if let Some(function) = self.backend.module.get_function(symbol) {
            return function;
        }
        let params: Vec<BasicMetadataTypeEnum> = params
            .iter()
            .map(|&machine| self.native_machine_type(machine).into())
            .collect();
        let ty = self.native_machine_type(result).fn_type(&params, false);
        self.backend
            .module
            .add_function(symbol, ty, Some(inkwell::module::Linkage::External))
    }

    pub(super) fn emit_native_symbol_addr(&mut self, op: &TirOp) {
        // Only the address crosses into the runtime transaction, which calls
        // it with the ABI's signature. The declaration carries that exact
        // signature: wasm-ld replaces a function whose declared and defined
        // signatures differ with a trapping stub.
        let abi = parse_native_callable_abi(native_attr(op, NATIVE_ABI_ATTR))
            .expect("the TIR verifier admitted the native callable ABI");
        let signature = abi
            .fixed_arity()
            .and_then(|arity| abi.native_machine_signature(arity))
            .expect("an address ABI has a fixed payload arity");
        let [result] = signature.results[..] else {
            panic!("a native callable returns one value: {signature:?}");
        };
        let function = self.native_symbol(
            native_attr(op, NATIVE_SYMBOL_ATTR),
            &signature.params,
            result,
        );
        let address = self
            .backend
            .builder
            .build_ptr_to_int(
                function.as_global_value().as_pointer_value(),
                self.backend.context.i64_type(),
                "native_symbol_addr",
            )
            .unwrap();
        self.values.set_value(op.results[0], address.into());
        self.values.set_repr(
            op.results[0],
            crate::codegen_ssa::HeldCarrier::of(&TirType::I64),
        );
    }

    pub(super) fn emit_call_native(&mut self, op: &TirOp) {
        let abi = parse_native_callable_abi(native_attr(op, NATIVE_ABI_ATTR))
            .expect("the TIR verifier admitted the native callable ABI");
        let signature = abi
            .native_machine_signature(op.operands.len())
            .expect("the TIR verifier admitted the native callable arity");
        assert!(
            signature.params.len() == op.operands.len()
                && signature
                    .params
                    .iter()
                    .all(|&machine| machine == NativeCallableMachineType::MoltValue)
                && signature.results == [NativeCallableMachineType::MoltValue],
            "CallNative passes and returns Molt values: {signature:?}"
        );
        let function = self.native_symbol(
            native_attr(op, NATIVE_SYMBOL_ATTR),
            &signature.params,
            NativeCallableMachineType::MoltValue,
        );
        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
            .operands
            .iter()
            .map(|&operand| self.ensure_i64(self.resolve(operand)).into())
            .collect();
        let call = self
            .backend
            .builder
            .build_call(function, &args, "native_call")
            .unwrap();
        self.bind_owned_runtime_result(op, call.try_as_basic_value().unwrap_basic());
    }
}
