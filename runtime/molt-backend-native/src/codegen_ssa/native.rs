//! A source extension's native callable in the walker: the two late-TIR
//! primitives `native_call_expansion` writes (`op_kinds.toml`
//! `NativeSymbolAddr`, `CallNative`). The meaning lives in that expansion.
//! Here the walker takes the symbol's address, or calls the symbol under the
//! machine signature its ABI token names; a backend only declares the symbol
//! (`CodegenBuilder::native_callee`).

use molt_ir::native_callable_abi::{
    NativeCallableMachineSignature, NativeCallableMachineType, parse_native_callable_abi,
};
use molt_ir::runtime_fn_generated::RuntimeBoxedReturn;

use super::builder::CodegenBuilder;
use super::function::{FunctionCx, ResultCustody};
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, NATIVE_ABI_ATTR, NATIVE_SYMBOL_ATTR, TirOp};

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Dynamic FFI calls borrow their TIR operands. The runtime consumes one
    /// builder reference for both capability denial and ordinary dispatch.
    pub(crate) fn invoke_ffi_op(&mut self, b: &mut B, op: &TirOp) {
        use molt_codegen_abi::inline_refcount::RefcountOperation;
        use molt_ir::runtime_fn_generated::RuntimeFn;

        if matches!(op.attrs.get("native_callable_binding"), Some(AttrValue::Str(binding)) if binding == "direct_symbol")
        {
            self.fatal("direct_symbol invoke_ffi survived native_call_expansion");
            return;
        }
        let prebuilt = matches!(op.attrs.get("native_callable_abi"), Some(AttrValue::Str(token))
            if parse_native_callable_abi(token).is_some_and(|abi| abi.uses_callargs()));
        if op.operands.is_empty() || (prebuilt && op.operands.len() != 2) {
            self.fatal("invoke_ffi requires a callable and the payload specified by its ABI");
            return;
        }
        let bridge =
            matches!(op.attrs.get("s_value"), Some(AttrValue::Str(mode)) if mode == "bridge");
        let lane = if bridge {
            "invoke_ffi_bridge"
        } else {
            "invoke_ffi_deopt"
        };
        let site = self.next_inline_cache_site(b, lane);
        let result = self
            .operands()
            .with_words(b, &op.operands, lane, |b, words, name| {
                let invoke = |b: &mut B, builder: B::Word| {
                    let entry = b.runtime_callee(RuntimeFn::InvokeFfiIc);
                    let required =
                        b.constant(molt_codegen_abi::box_bool_bits(i64::from(bridge)) as u64);
                    let args = [site, words[0], builder, required].map(|word| b.word_value(word));
                    let result = b.call(entry, &args).expect("invoke_ffi returns one word");
                    b.set_name(result, name);
                    b.expect_word(result)
                };
                if prebuilt {
                    let retain = b.refcount_site(RefcountOperation::Retain);
                    b.refcount(retain, words[1]);
                    invoke(b, words[1])
                } else {
                    super::calls::with_callargs(b, &words[1..], invoke)
                }
            });
        let result = b.word_value(result);
        self.bind_owned(b, op, result);
    }

    /// The extension transactions borrow a Python payload alongside a raw
    /// code address. Boxing the address or passing a raw payload violates
    /// the runtime's mixed machine ABI.
    pub(crate) fn native_transaction(
        &mut self,
        b: &mut B,
        op: &TirOp,
        runtime: molt_ir::runtime_fn_generated::RuntimeFn,
    ) {
        use super::custody::{CallTarget, RuntimeArg};
        let [address, payload] = op.operands.as_slice() else {
            self.fatal(format!(
                "{} requires a code address and a payload",
                runtime.symbol()
            ));
            return;
        };
        if !self.runtime.contains(runtime) {
            self.fatal(format!(
                "runtime symbol `{}` is unavailable in the selected runtime",
                runtime.symbol()
            ));
            return;
        }
        let address = self.operands().resolve(b, *address);
        let callee = b.runtime_callee(runtime);
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(callee),
            &[RuntimeArg::Word(address), RuntimeArg::Operand(*payload)],
            ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
            "native_transaction",
            runtime.symbol(),
        );
    }

    /// The op's symbol and its machine signature for `payload_arity`, or a
    /// fatal diagnostic when the expansion left either malformed.
    fn native_target(
        &self,
        op: &TirOp,
        payload_arity: Option<usize>,
    ) -> Option<(String, NativeCallableMachineSignature)> {
        let attr = |key: &str| match op.attrs.get(key) {
            Some(AttrValue::Str(value)) if !value.is_empty() => Some(value.as_str()),
            _ => None,
        };
        let (Some(symbol), Some(token)) = (attr(NATIVE_SYMBOL_ATTR), attr(NATIVE_ABI_ATTR)) else {
            self.fatal(format!(
                "{:?} lacks the symbol or ABI token native_call_expansion gives it",
                op.opcode
            ));
            return None;
        };
        let Some(abi) = parse_native_callable_abi(token) else {
            self.fatal(format!(
                "`{symbol}` names unknown native callable ABI `{token}`"
            ));
            return None;
        };
        let Some(signature) = payload_arity
            .or(abi.fixed_arity())
            .and_then(|arity| abi.native_machine_signature(arity))
        else {
            self.fatal(format!("`{symbol}` does not fit ABI `{token}`'s arity"));
            return None;
        };
        if signature.results.len() != 1 {
            self.fatal(format!("`{symbol}` must return one value: {signature:?}"));
            return None;
        }
        Some((symbol.to_owned(), signature))
    }

    /// The extension symbol's code address as a raw word. Only the address
    /// crosses into the runtime transaction, which calls it with the ABI's
    /// signature; the declaration still carries that exact signature.
    pub(crate) fn native_symbol_addr_op(&mut self, b: &mut B, op: &TirOp) {
        let Some((symbol, signature)) = self.native_target(op, None) else {
            return;
        };
        let callee = b.native_callee(&symbol, &signature.params, signature.results[0]);
        let address = b.function_address(callee);
        if let Some(&result) = op.results.first() {
            let address = b.word_value(address);
            self.bind(result, address, HeldCarrier::RawI64);
        }
    }

    /// A direct call of the extension symbol: Molt values in, borrowed, and
    /// one owned Molt value out, as `molt.object_call_v1` defines.
    pub(crate) fn call_native_op(&mut self, b: &mut B, op: &TirOp) {
        let Some((symbol, signature)) = self.native_target(op, Some(op.operands.len())) else {
            return;
        };
        let molt_values = |types: &[NativeCallableMachineType]| {
            types
                .iter()
                .all(|&ty| ty == NativeCallableMachineType::MoltValue)
        };
        if signature.params.len() != op.operands.len()
            || !molt_values(&signature.params)
            || !molt_values(&signature.results)
        {
            self.fatal(format!(
                "CallNative of `{symbol}` passes and returns Molt values only: {signature:?}"
            ));
            return;
        }
        let callee = b.native_callee(&symbol, &signature.params, signature.results[0]);
        self.positional_runtime_call(
            b,
            op,
            callee,
            ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
            "native_call",
            &symbol,
        );
    }
}

#[cfg(test)]
mod tests {
    //! The two ops on the interpreting builder and as Cranelift machine code:
    //! the call reaches the extension symbol with the borrowed operand words
    //! and binds its owned result; the address is the symbol's own code
    //! address; a malformed or non-Molt-value op is refused before any code.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{Run, both, every_runtime_entry, kernel, machine};
    use super::super::scalar::tests::{Value, native_result};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    fn inline(value: i64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | ((value as u64) & ((1 << 47) - 1))
    }

    fn native_op(opcode: OpCode, operands: u32, symbol: &str, abi: &str) -> TirOp {
        let mut attrs = AttrDict::default();
        attrs.insert(NATIVE_SYMBOL_ATTR.into(), AttrValue::Str(symbol.into()));
        attrs.insert(NATIVE_ABI_ATTR.into(), AttrValue::Str(abi.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operands).map(ValueId).collect(),
            results: vec![ValueId(operands)],
            attrs,
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        }
    }

    /// Lower `op` over boxed inputs %0.., and return its result value (none
    /// when it was refused) with the diagnostics.
    fn lower(op: TirOp, inputs: &[u64], machine_only: bool) -> Run<Vec<String>> {
        let lowering = kernel!(op: TirOp = op; |b, inputs| -> Vec<String> {
            let func = TirFunction::new(
                "native".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(8);
            for (index, &input) in inputs.iter().enumerate() {
                values.set_value(ValueId(index as u32), input);
                values.set_repr(ValueId(index as u32), HeldCarrier::Boxed);
            }
            let diagnostics = RefCell::new(Vec::new());
            let runtime = every_runtime_entry();
            let sites = Cell::new(0);
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &runtime,
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &sites,
            };
            match op.opcode {
                OpCode::CallNative => cx.call_native_op(b, op),
                OpCode::NativeSymbolAddr => cx.native_symbol_addr_op(b, op),
                other => panic!("{other:?} is no native op"),
            }
            let result = values.value(op.results[0]);
            (diagnostics.into_inner(), result.into_iter().collect())
        });
        let inputs: Vec<Value> = inputs.iter().copied().map(Value::Word).collect();
        if machine_only {
            machine(&lowering, &inputs, false)
        } else {
            both(&lowering, &inputs)
        }
    }

    #[test]
    fn a_native_call_passes_borrowed_words_and_binds_the_owned_result() {
        for inputs in [vec![], vec![OBJECT], vec![OBJECT, inline(-3), inline(7)]] {
            let symbol = format!("walker_native_object_call{}", inputs.len());
            let op = native_op(
                OpCode::CallNative,
                inputs.len() as u32,
                &symbol,
                "molt.object_call_v1",
            );
            let run = lower(op, &inputs, false);
            assert_eq!(run.fact, Vec::<String>::new());
            assert_eq!(
                run.results,
                [Value::Word(native_result(&symbol, &inputs))],
                "{} arguments",
                inputs.len()
            );
            // Borrowed operands: no reference count moves for the call.
            assert_eq!(run.refcounts, [], "{} arguments", inputs.len());
            assert_eq!(
                run.runtime_calls,
                [],
                "an extension call is no runtime call"
            );
        }
    }

    #[test]
    fn a_native_symbol_address_is_the_symbols_own_code() {
        let op = native_op(
            OpCode::NativeSymbolAddr,
            0,
            "walker_native_pyinit",
            "molt.pyinit_module_v1",
        );
        let run = lower(op, &[], true);
        assert_eq!(run.fact, Vec::<String>::new());
        let code = super::super::runtime_stubs::lookup("walker_native_pyinit")
            .expect("the test links the extension symbol");
        assert_eq!(run.results, [Value::Word(code as u64)]);
    }

    #[test]
    fn a_malformed_or_non_molt_value_native_op_is_refused() {
        let mut no_symbol = native_op(OpCode::CallNative, 1, "x", "molt.object_call_v1");
        no_symbol.attrs.remove(NATIVE_SYMBOL_ATTR);
        let unknown = native_op(OpCode::CallNative, 1, "x", "molt.unknown_v1");
        // One payload argument, but a pointer/length/pointer machine call.
        let forward = native_op(OpCode::CallNative, 1, "x", "molt.forward_f32_v1");
        let wrong_arity = native_op(OpCode::CallNative, 2, "x", "molt.object_callargs_v1");
        for (op, refusal) in [
            (no_symbol, "lacks the symbol or ABI token"),
            (unknown, "unknown native callable ABI"),
            (forward, "passes and returns Molt values only"),
            (wrong_arity, "does not fit ABI"),
        ] {
            let inputs = vec![OBJECT; op.operands.len()];
            let run = lower(op, &inputs, false);
            assert!(
                run.fact.len() == 1 && run.fact[0].contains(refusal),
                "{refusal}: {:?}",
                run.fact
            );
            assert_eq!(run.results, [], "{refusal}: a refused op binds nothing");
        }
    }
}
