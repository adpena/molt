//! Runtime calls (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "call lowering"): classified runtime entries called by their generated
//! ABI, and the dynamic call protocol (`molt_call_func_fast{N}`,
//! `molt_call_bind`, `molt_call_func_owned`), written once.

use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::{
    RuntimeBoxedAbi, RuntimeBoxedReturn, RuntimeCallForm, RuntimeFn, RuntimeReturnContract,
};

use super::builder::{CodegenBuilder, MachineKind};
use super::custody::{Custody, spill_words};
use super::function::{FunctionCx, ResultCustody};
use super::scalar;
use super::value_table::HeldCarrier;
use crate::tir::ops::TirOp;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_codegen_abi::word::WordComparison;

/// Call `function` with machine words, for its word.
fn call_word<B: CodegenBuilder>(
    b: &mut B,
    function: RuntimeFn,
    args: &[B::Word],
    name: &str,
) -> B::Word {
    let callee = b.runtime_callee(function);
    let args: Vec<B::Value> = args.iter().map(|&arg| b.word_value(arg)).collect();
    let value = b
        .call(callee, &args)
        .unwrap_or_else(|| panic!("{} returns a word", function.symbol()));
    b.set_name(value, name);
    b.expect_word(value)
}

/// A positional CallArgs for `args`, which `consume` (`molt_call_bind*`,
/// `molt_call_builtin`) owns and frees. The builder reserves exactly
/// `args.len()` positional slots, so no push can fail; each push retains its
/// argument. A failed allocation returns the raw word 0 with MemoryError
/// pending; every builder entry and consumer observes a pending exception
/// first, so the protocol needs no branch here.
pub(crate) fn with_callargs<B: CodegenBuilder>(
    b: &mut B,
    args: &[B::Word],
    consume: impl FnOnce(&mut B, B::Word) -> B::Word,
) -> B::Word {
    let count = b.constant(args.len() as u64);
    let zero = b.constant(0);
    let builder = call_word(b, RuntimeFn::CallargsNew, &[count, zero], "callargs");
    let push = b.runtime_callee(RuntimeFn::CallargsPushPos);
    for &arg in args {
        let words = [b.word_value(builder), b.word_value(arg)];
        let pushed = b.call(push, &words);
        if let Some(pushed) = pushed {
            b.set_name(pushed, "callargs_push");
        }
    }
    consume(b, builder)
}

/// `molt_call_bind(callable, CallArgs(args))`.
fn bind_call_words<B: CodegenBuilder>(b: &mut B, callable: B::Word, args: &[B::Word]) -> B::Word {
    let bind = b.runtime_callee(RuntimeFn::CallBind);
    with_callargs(b, args, |b, builder| {
        let words = [b.word_value(callable), b.word_value(builder)];
        let value = b.call(bind, &words).expect("molt_call_bind returns a word");
        b.set_name(value, "call_result");
        b.expect_word(value)
    })
}

/// `molt_call_func_fast{N}` for at most three positional arguments (the
/// callee trampoline borrows them), else the CallArgs route.
fn call_func_words<B: CodegenBuilder>(b: &mut B, callable: B::Word, args: &[B::Word]) -> B::Word {
    if args.len() > 3 {
        return bind_call_words(b, callable, args);
    }
    let fast = match args.len() {
        0 => RuntimeFn::CallFuncFast0,
        1 => RuntimeFn::CallFuncFast1,
        2 => RuntimeFn::CallFuncFast2,
        _ => RuntimeFn::CallFuncFast3,
    };
    let mut words = Vec::with_capacity(args.len() + 1);
    words.push(callable);
    words.extend_from_slice(args);
    call_word(b, fast, &words, "call_func")
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Call a staged runtime contract by its boxed ABI.
    pub(crate) fn contract_runtime_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        call: molt_ir::runtime_calls::RuntimeCall,
    ) {
        self.boxed_runtime_call(b, op, call.boxed_abi());
    }

    /// Lower a preserved op through its admitted boxed runtime ABI. Both
    /// bound and discarded calls execute: owned results are bound or
    /// released, void calls produce no value. Generated semantic facts, not
    /// symbol spelling, authorize the route; an unclassified entry is
    /// refused (`false`), and an entry with no positional boxed ABI at this
    /// arity is a fatal diagnostic.
    pub(crate) fn preserved_runtime_call(&mut self, b: &mut B, op: &TirOp, kind: OpKind) -> bool {
        if let Some(call) = molt_ir::runtime_calls::RuntimeCall::from_op(kind) {
            self.contract_runtime_call(b, op, call);
            return true;
        }
        if let Some(abi) = molt_ir::runtime_fn_generated::runtime_boxed_op_abi(kind) {
            self.boxed_runtime_call(b, op, abi);
            return true;
        }
        let Some(function) = molt_ir::runtime_fn_generated::preserved_op_runtime_fn(kind) else {
            return false;
        };
        let Some(abi) = function
            .boxed_abi()
            .filter(|abi| abi.arity() == op.operands.len())
        else {
            if !self.runtime.contains(function) {
                return false;
            }
            self.fatal(format!(
                "preserved SimpleIR op `{kind}` maps to runtime entry `{}`, \
                 but that entry has no positional boxed-value ABI classification at this arity",
                function.symbol()
            ));
            return true;
        };
        self.boxed_runtime_call(b, op, abi);
        true
    }

    /// A classified positional boxed-ABI call, for a direct runtime call or
    /// a preserved op: one admission, borrowed-operand custody and the
    /// canonical result custody.
    pub(crate) fn boxed_runtime_call(&mut self, b: &mut B, op: &TirOp, abi: RuntimeBoxedAbi) {
        let symbol = abi.symbol();
        // Semantic classification is not availability in the selected
        // runtime; admit before declaring symbols or minting owners.
        if !self.runtime.contains(abi.function) {
            self.fatal(format!(
                "boxed runtime symbol `{symbol}` is unavailable in the selected runtime"
            ));
            return;
        }
        if op.operands.len() != abi.arity() {
            self.fatal(format!(
                "boxed runtime symbol `{symbol}` has mismatched arity"
            ));
            return;
        }
        if op.results.len() > 1 {
            self.fatal(format!(
                "boxed runtime symbol `{symbol}` has multiple result values"
            ));
            return;
        }
        if abi.result == RuntimeBoxedReturn::Void && !op.results.is_empty() {
            self.fatal(format!(
                "call to void runtime symbol `{symbol}` has result values"
            ));
            return;
        }
        let callee = b.runtime_callee(abi.function);
        self.positional_runtime_call(
            b,
            op,
            callee,
            ResultCustody::Boxed(abi.result),
            "boxed_call",
            symbol,
        );
    }

    /// A direct runtime call whose result is raw bits (the generated raw
    /// ABI): operands pass as borrowed words, and the result binds without
    /// an owner. A machine helper takes and returns machine words.
    pub(crate) fn raw_runtime_call(&mut self, b: &mut B, op: &TirOp, function: RuntimeFn) {
        let symbol = function.symbol();
        if !self.runtime.contains(function) {
            self.fatal(format!(
                "raw runtime symbol `{symbol}` is unavailable in the selected runtime"
            ));
            return;
        }
        if op.operands.len() != function.params() {
            self.fatal(format!(
                "raw runtime symbol `{symbol}` has mismatched arity"
            ));
            return;
        }
        if op.results.len() > 1 {
            self.fatal(format!(
                "raw runtime symbol `{symbol}` has multiple result values"
            ));
            return;
        }
        if function.form() == RuntimeCallForm::Helper {
            let callee = b.runtime_callee(function);
            let operands = self.operands();
            let args: Vec<B::Value> = op
                .operands
                .iter()
                .map(|&operand| {
                    let value = operands.resolve(b, operand);
                    let word = scalar::word_of(b, value);
                    b.word_value(word)
                })
                .collect();
            let result = b.call(callee, &args);
            if let Some(value) = result {
                b.set_name(value, "helper_call");
            }
            if let (Some(&id), Some(value)) = (op.results.first(), result) {
                let ty = match function.result() {
                    RuntimeReturnContract::RawBits => TirType::I64,
                    _ => TirType::DynBox,
                };
                self.bind(id, value, HeldCarrier::of(&ty));
            }
            return;
        }
        let callee = b.runtime_callee(function);
        self.positional_runtime_call(b, op, callee, ResultCustody::Unowned, "raw_call", symbol);
    }

    /// Borrow a dynamic call's callable and positional arguments through one
    /// custody. The dynamic-call ABI carries every argument as a boxed word,
    /// which the callee trampoline decodes into its parameter's carrier, so
    /// each value is boxed once here and a minted box is released after the
    /// call returns.
    fn borrow_dynamic_call_operands(
        &self,
        b: &mut B,
        callable: ValueId,
        args: &[ValueId],
    ) -> (Custody<B>, B::Word, Vec<B::Word>) {
        let mut operands = Vec::with_capacity(args.len() + 1);
        operands.push(callable);
        operands.extend_from_slice(args);
        let view = self.operands();
        let mut custody = view.begin(b, &operands, "dynamic_call");
        let callable = view.operand(b, &mut custody, callable);
        let args = args
            .iter()
            .map(|&arg| view.operand(b, &mut custody, arg))
            .collect();
        (custody, callable, args)
    }

    /// `molt_call_bind(callable, CallArgs(args))` over borrowed operands.
    pub(crate) fn call_bind_runtime(
        &self,
        b: &mut B,
        callable: ValueId,
        args: &[ValueId],
    ) -> B::Word {
        let (custody, callable, args) = self.borrow_dynamic_call_operands(b, callable, args);
        let result = bind_call_words(b, callable, &args);
        self.operands()
            .finish(b, custody, result, "dynamic_call_result", |_| {})
    }

    /// `molt_call_func_fast{N}` (or the CallArgs route) over borrowed operands.
    pub(crate) fn call_func_runtime(
        &self,
        b: &mut B,
        callable: ValueId,
        args: &[ValueId],
    ) -> B::Word {
        let (custody, callable, args) = self.borrow_dynamic_call_operands(b, callable, args);
        let result = call_func_words(b, callable, &args);
        self.operands()
            .finish(b, custody, result, "dynamic_call_result", |_| {})
    }

    /// A function object takes the fast call; any other callable binds.
    pub(crate) fn call_func_or_bind_runtime(
        &self,
        b: &mut B,
        callable: ValueId,
        args: &[ValueId],
    ) -> B::Word {
        let (custody, callable, args) = self.borrow_dynamic_call_operands(b, callable, args);
        let is_function = call_word(b, RuntimeFn::IsFunctionObj, &[callable], "is_function_obj");
        let truthy = call_word(b, RuntimeFn::IsTruthy, &[is_function], "is_function_truthy");
        let zero = b.constant(0);
        let fast = b.compare(WordComparison::NotEqual, truthy, zero);
        let named = b.bit_value(fast);
        b.set_name(named, "call_func_fast_guard");
        let fast_block = b.create_block("call_func_fast");
        let bind_block = b.create_block("call_func_bind");
        let merge = b.create_block("call_func_merge");
        let joined = b.append_block_param(merge, MachineKind::Word);
        b.set_name(joined, "call_func_or_bind_phi");
        b.branch_if(fast, (fast_block, &[]), (bind_block, &[]));
        b.switch_to_block(fast_block);
        let fast_result = call_func_words(b, callable, &args);
        let fast_result = b.word_value(fast_result);
        b.jump(merge, &[fast_result]);
        b.switch_to_block(bind_block);
        let bind_result = bind_call_words(b, callable, &args);
        let bind_result = b.word_value(bind_result);
        b.jump(merge, &[bind_result]);
        b.switch_to_block(merge);
        let result = b.expect_word(joined);
        self.operands()
            .finish(b, custody, result, "dynamic_call_result", |_| {})
    }

    /// An ordinary source call that adopted its callable and arguments:
    /// `molt_call_func_owned(callable, args, nargs, 0)`. The runtime's owned
    /// lane ends a temporary bound method before its function runs, moves
    /// the arguments into an adopting frame or releases them after a
    /// borrowing callee, and releases the callable last; nothing is released
    /// here on success. A failed materialization skips entry and releases the
    /// instruction's inputs through operation-local custody.
    pub(crate) fn call_func_owned_runtime(&self, b: &mut B, op: &TirOp) -> B::Word {
        let view = self.operands();
        let mut custody = view.begin_call(
            b,
            &op.operands,
            &op.operands[1..],
            Some(op.operands[0]),
            "owned_call",
        );
        let words: Vec<B::Word> = op
            .operands
            .iter()
            .map(|&operand| view.operand(b, &mut custody, operand))
            .collect();
        let (args, nargs) = spill_words(b, &words[1..], "owned_call_args");
        let owned = b.runtime_callee(RuntimeFn::CallFuncOwned);
        let no_site = b.constant(0);
        view.surrender(b, &custody, &op.operands, &[]);
        let call_args = [
            b.word_value(words[0]),
            b.word_value(args),
            b.word_value(nargs),
            b.word_value(no_site),
        ];
        let result = b
            .call(owned, &call_args)
            .expect("molt_call_func_owned returns a word");
        b.set_name(result, "call_func_owned");
        let result = b.expect_word(result);
        view.finish(b, custody, result, "owned_call_result", |_| {})
    }
}

#[cfg(test)]
mod tests {
    //! The call family on the interpreting builder and as Cranelift machine
    //! code. The oracle is the dynamic call protocol: arity picks the fast
    //! entry or CallArgs, a function object takes the fast call and any other
    //! callable binds, an owned call hands every word to the runtime.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{Run, both, every_runtime_entry, kernel, machine};
    use super::super::scalar::tests::{FUNCTION_WORD, Value, literal_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    fn inline(value: i64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | ((value as u64) & ((1 << 47) - 1))
    }

    fn key(words: &[u64]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[derive(Clone, Copy, Debug)]
    enum Route {
        Func,
        Bind,
        FuncOrBind,
        Owned,
    }

    /// Call %0 with arguments %1.. (all boxed inputs) by `route`.
    fn call(route: Route, inputs: &[u64], machine_only: bool) -> Run<()> {
        let call = kernel!(route: Route = route; |b, inputs| -> () {
            let func = TirFunction::new(
                "calls".into(),
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
            let cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &runtime,
                call_sites: &sites,
            };
            let args: Vec<ValueId> = (1..inputs.len() as u32).map(ValueId).collect();
            let word = match *route {
                Route::Func => cx.call_func_runtime(b, ValueId(0), &args),
                Route::Bind => cx.call_bind_runtime(b, ValueId(0), &args),
                Route::FuncOrBind => cx.call_func_or_bind_runtime(b, ValueId(0), &args),
                Route::Owned => {
                    let op = TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Call,
                        operands: (0..inputs.len() as u32).map(ValueId).collect(),
                        results: vec![],
                        attrs: AttrDict::default(),
                        source_span: None,
                        origin: None,
                        runtime: None,
                        callee: None,
                    };
                    cx.call_func_owned_runtime(b, &op)
                }
            };
            ((), vec![b.word_value(word)])
        });
        let inputs: Vec<Value> = inputs.iter().copied().map(Value::Word).collect();
        if machine_only {
            machine(&call, &inputs, false)
        } else {
            both(&call, &inputs)
        }
    }

    #[test]
    fn arity_picks_the_fast_entry_or_callargs() {
        let fast = [
            RuntimeFn::CallFuncFast0,
            RuntimeFn::CallFuncFast1,
            RuntimeFn::CallFuncFast2,
            RuntimeFn::CallFuncFast3,
        ];
        for count in 0..=4 {
            let mut inputs = vec![OBJECT];
            inputs.extend((0..count).map(|index| inline(index as i64)));
            let run = call(Route::Func, &inputs, false);
            let expected = if count <= 3 {
                literal_word(fast[count], &key(&inputs))
            } else {
                literal_word(RuntimeFn::CallBind, &key(&inputs))
            };
            assert_eq!(run.results, [Value::Word(expected)], "{count} arguments");
        }
        let inputs = [OBJECT, inline(1), inline(2)];
        let run = call(Route::Bind, &inputs, false);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::CallBind,
                &key(&inputs)
            ))]
        );
        assert_eq!(
            run.runtime_calls,
            [
                RuntimeFn::CallargsNew,
                RuntimeFn::CallargsPushPos,
                RuntimeFn::CallargsPushPos,
                RuntimeFn::CallBind
            ]
        );
    }

    #[test]
    fn a_function_takes_the_fast_call_and_any_other_callable_binds() {
        let run = call(Route::FuncOrBind, &[FUNCTION_WORD, inline(7)], true);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::CallFuncFast1,
                &key(&[FUNCTION_WORD, inline(7)])
            ))]
        );
        let run = call(Route::FuncOrBind, &[OBJECT, inline(7)], true);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::CallBind,
                &key(&[OBJECT, inline(7)])
            ))]
        );
    }

    #[test]
    fn an_owned_call_hands_the_callable_and_every_argument_to_the_runtime() {
        let inputs = [OBJECT, inline(1), inline(2)];
        let run = call(Route::Owned, &inputs, false);
        let mut expected = key(&inputs);
        expected.extend(0u64.to_le_bytes());
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::CallFuncOwned,
                &expected
            ))]
        );
        assert!(run.refcounts.is_empty(), "nothing is released on success");
    }
}
