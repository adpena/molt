//! Runtime calls (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "call lowering"): classified runtime entries called by their generated
//! ABI, and the dynamic call protocol (`molt_call_func_fast{N}`,
//! `molt_call_bind`, `molt_call_func_owned`), written once.

use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::{
    RuntimeBoxedAbi, RuntimeBoxedReturn, RuntimeCallForm, RuntimeFn, RuntimeReturnContract,
};

use super::builder::{CodegenBuilder, IntBinary, IntComparison, MachineKind, SiteBody};
use super::control;
use super::custody::{CallTarget, Custody, RuntimeArg, release_call_inputs, spill_words};
use super::function::{FunctionCx, ResultCustody};
use super::object::{raw_name_pointer, with_name};
use super::scalar;
use super::value_table::HeldCarrier;
use crate::ir::ParameterCustody;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::method_site::{method_site_record_size, write_method_site_record_image};
use molt_codegen_abi::word::WordComparison;
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
use molt_ir::tir::ops::BuiltinCallTarget;

/// The runtime's fixed-arity loaded-method calls, the cold path of a method
/// site call with that many arguments. A call with more keeps the runtime
/// call.
const LOADED_METHOD_CALLS: [RuntimeFn; 5] = [
    RuntimeFn::CallLoadedMethod0,
    RuntimeFn::CallLoadedMethod1,
    RuntimeFn::CallLoadedMethod2,
    RuntimeFn::CallLoadedMethod3,
    RuntimeFn::CallLoadedMethod4,
];

/// The boxed word of `None`.
const NONE_WORD: u64 = molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE;

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
        if self.packed_buffer_call(b, op, abi.function) {
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
    /// `molt_call_func_owned(callable, args, nargs, candidate_key_or_zero)`. The runtime's owned
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
        let candidate = (op.opcode == OpCode::CallClassCandidate)
            .then(|| self.linked_target(op))
            .flatten();
        view.surrender(b, &custody, &op.operands, &[]);
        let callable = b.word_value(words[0]);
        let args = b.word_value(args);
        let nargs = b.word_value(nargs);
        let result = if let Some((target, abi)) = candidate {
            let init = b.function_callee(&target, &abi);
            let code = b.function_address(init);
            let slot = super::object::class_definition_slot(b, op);
            let slot = b.word_value(slot);
            let code = b.word_value(code);
            let entry = b.runtime_callee(RuntimeFn::CallClassCandidateOwned);
            b.call(entry, &[callable, args, nargs, slot, code])
        } else {
            let key = molt_ir::tir::call_targets::runtime_candidate(op).map_or(0, |candidate| {
                candidate
                    .callable()
                    .expect("verified runtime candidate")
                    .key
            });
            let key = b.constant(key);
            let key = b.word_value(key);
            let entry = b.runtime_callee(RuntimeFn::CallFuncOwned);
            b.call(entry, &[callable, args, nargs, key])
        }
        .expect("owned call returns a word");
        b.set_name(result, "call_func_owned");
        let result = b.expect_word(result);
        view.finish(b, custody, result, "owned_call_result", |_| {})
    }

    /// `Call`: an ordinary source call, a builder-consuming call, a guarded
    /// call, a direct call of linked compiled code, a call of a runtime entry
    /// by its role, or a dynamic call of operand 0.
    pub(crate) fn call_op(&mut self, b: &mut B, op: &TirOp) {
        let origin = op.origin;
        if op.opcode == OpCode::CallClassCandidate && op.operands.is_empty() {
            self.fatal("class call candidate requires its captured callable");
            return;
        }
        if op.opcode == OpCode::CallClassCandidate
            || (matches!(origin, Some(OpKind::CallFunc | OpKind::CallFunction))
                && !op.operands.is_empty())
        {
            // An ordinary source call adopted its callable and arguments.
            let result = if op.argument_custody().is_some() {
                self.call_func_owned_runtime(b, op)
            } else {
                self.call_func_or_bind_runtime(b, op.operands[0], &op.operands[1..])
            };
            let result = b.word_value(result);
            self.bind_owned(b, op, result);
            return;
        }
        if matches!(origin, Some(OpKind::CallBind | OpKind::CallIndirect)) && op.operands.len() >= 2
        {
            let (lane, function) = if origin == Some(OpKind::CallIndirect) {
                ("call_indirect", RuntimeFn::CallIndirectIc)
            } else {
                ("call_bind", RuntimeFn::CallBindIc)
            };
            // An ordinary call (a stack-form builder) adopted its callable as
            // well as the builder; one owned entry serves both spellings.
            let owned_callable = op.operand_custody(0) == ParameterCustody::Transferred;
            let function = if owned_callable {
                RuntimeFn::CallBindIcOwned
            } else {
                function
            };
            let result = self.builder_consuming_call(
                b,
                op.operands[0],
                op.operands[1],
                lane,
                function,
                owned_callable,
            );
            let result = b.word_value(result);
            self.bind_owned(b, op, result);
            return;
        }
        if origin == Some(OpKind::CallGuarded) && !op.operands.is_empty() {
            // A guarded call adopts as an instruction, whatever it resolves
            // to: its callable and arguments take the owned lane.
            let result = if op.argument_custody().is_some() {
                self.call_func_owned_runtime(b, op)
            } else {
                self.call_func_runtime(b, op.operands[0], &op.operands[1..])
            };
            let result = b.word_value(result);
            self.bind_owned(b, op, result);
            return;
        }
        let Some(target) = self.call_target(op) else {
            if op.operands.is_empty() {
                // No operands and no target: None.
                self.bind_none(b, op);
            } else {
                let result = self.call_bind_runtime(b, op.operands[0], &op.operands[1..]);
                let result = b.word_value(result);
                self.bind_owned(b, op, result);
            }
            return;
        };
        if let Some(abi) = self.linkage.get(&target) {
            let abi = abi.clone();
            self.compiled_call(b, op, &target, &abi, &op.operands, 0);
            return;
        }
        // Source functions use exact native linkage. Only the external or
        // runtime CALL role, or a first-class TIR `Call` whose own identity
        // names the target (`runtime_call_symbol`), may use a classified
        // runtime ABI; neither a symbol prefix nor an operand count invents a
        // signature.
        let runtime_role = match origin {
            Some(kind) => kind == OpKind::Call,
            None => crate::tir::call_targets::runtime_call_symbol(op) == Some(target.as_str()),
        };
        // The runtime entry decoded at TIR ingest (HF-821 B1), admitted only
        // at its exact arity.
        let runtime_target = op
            .runtime
            .filter(|function| runtime_role && function.params() == op.operands.len());
        if let Some(function @ (RuntimeFn::SplicedFrameLink | RuntimeFn::SplicedFrameFinish)) =
            runtime_target
        {
            self.spliced_frame_call(b, op, function);
        } else if let Some(abi) = runtime_target.and_then(RuntimeFn::boxed_abi) {
            self.boxed_runtime_call(b, op, abi);
        } else if let Some(function) =
            runtime_target.filter(|function| function.form() == RuntimeCallForm::RawCall)
        {
            self.raw_runtime_call(b, op, function);
        } else {
            self.fatal(format!(
                "direct call target `{target}` has no exact native linkage ABI; refusing to invent an i64 signature"
            ));
            self.bind_none(b, op);
        }
    }

    /// The identity place is an address in caller storage. Its payload is
    /// transferred to the runtime only on an observed activation's exit.
    pub(crate) fn spliced_frame_call(&mut self, b: &mut B, op: &TirOp, function: RuntimeFn) {
        if !self.runtime.contains(function) {
            self.fatal(format!("missing runtime entry '{}'", function.symbol()));
            return;
        }
        let slot = *op.operands.last().expect("spliced frame call with a slot");
        debug_assert_eq!(self.operands().held(slot), HeldCarrier::RawI64);
        let slot = self.operands().resolve(b, slot);
        let slot = b.expect_word(slot);
        let callee = b.runtime_callee(function);
        if function == RuntimeFn::SplicedFrameLink {
            let slot = b.word_value(slot);
            self.runtime_call(
                b,
                op,
                CallTarget::Callee(callee),
                &[
                    RuntimeArg::Operand(op.operands[0]),
                    RuntimeArg::Operand(op.operands[1]),
                    RuntimeArg::Word(slot),
                ],
                ResultCustody::SideEffect,
                "spliced_frame_link",
                function.symbol(),
            );
            return;
        }
        let address = b.word_to_address(slot);
        let bindings = b.load_word(address, 0);
        let zero = b.constant(0);
        let observed = b.int_compare(IntComparison::NotEqual, bindings, zero);
        let release = b.create_block("spliced_frame_finish");
        let next = b.create_block("spliced_frame_finished");
        b.branch_if(observed, (release, &[]), (next, &[]));
        b.switch_to_block(release);
        b.store_word(address, 0, zero);
        b.call_cold(callee, &[b.word_value(bindings)]);
        b.jump(next, &[]);
        b.switch_to_block(next);
        self.bind_none(b, op);
    }

    /// The linker symbol a call names: its typed callee's row (HF-847), else
    /// the symbol an optional reference keeps (`s_value`, HF-821 B1), else
    /// `_var`.
    pub(crate) fn call_target(&self, op: &TirOp) -> Option<String> {
        if let Some(callee) = op.callee {
            return Some(self.functions.symbol(callee).to_owned());
        }
        ["s_value", "_var"]
            .iter()
            .find_map(|key| match op.attrs.get(*key) {
                Some(AttrValue::Str(symbol)) if !symbol.is_empty() => Some(symbol.clone()),
                _ => None,
            })
    }

    /// `value`, held as `source`, converted for a destination of type
    /// `target` (`control::coerce`); a refused conversion is a fatal
    /// diagnostic and a placeholder.
    pub(crate) fn coerce(
        &self,
        b: &mut B,
        value: B::Value,
        source: HeldCarrier,
        source_id: Option<ValueId>,
        target: &TirType,
    ) -> B::Value {
        control::coerce(b, self.plan, value, source, source_id, target).unwrap_or_else(|| {
            self.fatal(format!(
                "no conversion of a value held as {source:?} to {target:?}"
            ));
            b.placeholder(control::carrier_kind(HeldCarrier::of(target)))
        })
    }

    /// `CodeIs` (HF-821 D4): the function word's code pointer, the first
    /// word of the function object, equals the compiled target's address.
    pub(crate) fn code_is_op(&mut self, b: &mut B, op: &TirOp) {
        let Some((target, abi)) = self.linked_target(op) else {
            self.fatal("CodeIs names no compiled target");
            return;
        };
        let callee = b.function_callee(&target, &abi);
        let operand = self.operands().resolve(b, op.operands[0]);
        let word = scalar::word_of(b, operand);
        let mask = b.constant(molt_codegen_abi::POINTER_MASK);
        let address = b.int_binary(IntBinary::And, word, mask, false);
        let object = b.word_to_address(address);
        let code = b.load_word(object, 0);
        let expected = b.function_address(callee);
        let same = b.int_compare(IntComparison::Equal, code, expected);
        let same = b.bit_value(same);
        b.set_name(same, "code_is");
        self.bind(op.results[0], same, HeldCarrier::RawBool);
    }

    /// `FunctionClosure` (design 78 5.10): the function word's closure tuple,
    /// one load at `FUNCTION_CLOSURE_OFFSET`. The result is a view: the
    /// function object owns the tuple, and ownership facts give the view no
    /// release of its own.
    pub(crate) fn function_closure_op(&mut self, b: &mut B, op: &TirOp) {
        let operand = self.operands().resolve(b, op.operands[0]);
        let word = scalar::word_of(b, operand);
        let mask = b.constant(molt_codegen_abi::POINTER_MASK);
        let address = b.int_binary(IntBinary::And, word, mask, false);
        let object = b.word_to_address(address);
        let closure = b.load_word(object, molt_codegen_abi::FUNCTION_CLOSURE_OFFSET);
        let closure = b.word_value(closure);
        b.set_name(closure, "function_closure");
        self.bind(op.results[0], closure, HeldCarrier::of(&TirType::DynBox));
    }

    /// The compiled target `op` names and its frozen linkage row, when the
    /// program has one.
    pub(crate) fn linked_target(&self, op: &TirOp) -> Option<(String, NativeFunctionLinkageAbi)> {
        let target = self.call_target(op)?;
        let abi = self.linkage.get(&target)?.clone();
        Some((target, abi))
    }

    /// A direct call into compiled code `target` with `operands`, the op's
    /// operands from `first_operand` on. Each argument is converted from its
    /// held carrier to the callee's parameter type (`DynBox` is the boxed
    /// ABI): a raw value passed raw into a boxed parameter would decode as a
    /// garbage float. Compiled callees borrow their arguments and return an
    /// owned result, so a scalar boxed for a boxed parameter is a temporary
    /// of this call: the operation's custody mints it (skipping the call when
    /// a box cannot be allocated) and releases it after the call returns. A
    /// position the instruction adopted (`argument_custody`) belongs to the
    /// entry once it runs.
    pub(crate) fn compiled_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        target: &str,
        abi: &NativeFunctionLinkageAbi,
        operands: &[ValueId],
        first_operand: usize,
    ) {
        let callee = b.function_callee(target, abi);
        let return_type = abi.return_type.clone().unwrap_or(TirType::DynBox);
        let view = self.operands();
        let param_types: Vec<TirType> = (0..operands.len())
            .map(|index| {
                abi.param_types
                    .get(index)
                    .cloned()
                    .unwrap_or(TirType::DynBox)
            })
            .collect();
        let boxed_param = |index: usize| HeldCarrier::of(&param_types[index]) == HeldCarrier::Boxed;
        let boxed_source = |id: ValueId| view.held(id) == HeldCarrier::Boxed;
        let boxes = |index: usize| boxed_param(index) && !boxed_source(operands[index]);
        let adopted = |index: usize| {
            op.operand_custody(index + first_operand) == ParameterCustody::Transferred
        };
        let pick = |keep: &dyn Fn(usize) -> bool| -> Vec<ValueId> {
            (0..operands.len())
                .filter(|&index| keep(index))
                .map(|index| operands[index])
                .collect()
        };
        let borrowed = pick(&boxes);
        let adopted_inputs = pick(&adopted);
        let mut custody = view.begin_call(b, &borrowed, &adopted_inputs, None, "direct_call");
        let mut args = Vec::with_capacity(operands.len());
        // Adopted object references a raw parameter does not keep: the raw
        // entry extracted its value, so the reference ends after the call.
        let mut extracted = Vec::new();
        for (index, &id) in operands.iter().enumerate() {
            let value = if boxes(index) {
                let word = view.operand(b, &mut custody, id);
                b.word_value(word)
            } else {
                let value = view.resolve(b, id);
                if adopted(index) && boxed_source(id) && !boxed_param(index) {
                    extracted.push(scalar::word_of(b, value));
                }
                self.coerce(b, value, view.held(id), Some(id), &param_types[index])
            };
            args.push(value);
        }
        // Boxes minted for adopted positions belong to the entry once it runs.
        let adopted_boxes = pick(&|index| adopted(index) && boxes(index));
        let borrowed_boxes = pick(&|index| !adopted(index) && boxes(index));
        view.surrender(b, &custody, &adopted_boxes, &borrowed_boxes);
        let returned = b.call(callee, &args);
        if let Some(value) = returned {
            b.set_name(value, "direct_call");
        }
        release_call_inputs(b, None, &extracted);
        let none = b.constant(NONE_WORD);
        if let Some(&result) = op.results.first() {
            let raw = returned.unwrap_or_else(|| b.word_value(none));
            let word = if return_type == TirType::DynBox {
                b.expect_word(raw)
            } else {
                scalar::box_value(b, raw, &return_type)
            };
            let word = view.finish(b, custody, word, "direct_call_result", |_| {});
            let word = b.word_value(word);
            self.bind(result, word, HeldCarrier::Boxed);
        } else if let Some(returned) = returned.filter(|_| !return_type.is_unboxed()) {
            // A semantic object return is a boxed handle; a raw scalar has no
            // owner and needs no boxing just to be discarded.
            let word = b.expect_word(returned);
            let word = view.finish(b, custody, word, "direct_call_result", |_| {});
            let word = b.word_value(word);
            self.bind_owned(b, op, word);
        } else {
            view.finish(b, custody, none, "direct_call_result", |_| {});
        }
    }

    /// CPython's CALL after a method load: `[callable, self_or_null, args...]`,
    /// a source call that adopts every operand. At a method site with at most
    /// four arguments the shared site call body takes every word over;
    /// otherwise `molt_call_loaded_method{N}` (or the spilled entry above four
    /// arguments) does: it enters the selected function with `self` prepended,
    /// or calls the attribute when `self_or_null` is the null sentinel. Failed
    /// boxing skips the entry and releases the instruction's inputs.
    pub(crate) fn call_loaded_method_op(&mut self, b: &mut B, op: &TirOp) {
        let adopts_all = op.argument_custody().is_some_and(|custody| {
            custody
                .iter()
                .all(|&position| position == ParameterCustody::Transferred)
        });
        if op.operands.len() < 2 || !adopts_all {
            self.fatal(format!(
                "malformed {op:?}: a loaded method call adopts its pair and every argument"
            ));
            return;
        }
        let lane = "call_loaded_method";
        let view = self.operands();
        let mut custody = view.begin_call(
            b,
            &op.operands,
            &op.operands[1..],
            Some(op.operands[0]),
            lane,
        );
        let words: Vec<B::Word> = op
            .operands
            .iter()
            .map(|&operand| view.operand(b, &mut custody, operand))
            .collect();
        let arguments = words.len() - 2;
        let cold = LOADED_METHOD_CALLS.get(arguments).copied();
        let result = match (op.method_site(), cold) {
            (Some(site), Some(cold)) => {
                view.surrender(b, &custody, &op.operands, &[]);
                let Some(record) = self.method_site_record(b, site) else {
                    self.fatal(format!("{op:?}: method site {site} has no record"));
                    return;
                };
                let thread = b.thread_record_word();
                let admission = b.refcount_admission();
                let mut site_words = vec![thread, record];
                site_words.extend_from_slice(&words);
                site_words.push(admission);
                let args: Vec<B::Value> =
                    site_words.iter().map(|&word| b.word_value(word)).collect();
                let result = b.site(SiteBody::MethodSiteCall { arguments, cold }, &args);
                b.set_name(result, "method_site_call");
                b.expect_word(result)
            }
            _ => {
                let (function, call_words) = match cold {
                    Some(function) => (function, words.clone()),
                    None => {
                        let (args, count) = spill_words(b, &words[2..], "call_loaded_method_args");
                        (
                            RuntimeFn::CallLoadedMethodSpilled,
                            vec![words[0], words[1], args, count],
                        )
                    }
                };
                let callee = b.runtime_callee(function);
                view.surrender(b, &custody, &op.operands, &[]);
                let args: Vec<B::Value> =
                    call_words.iter().map(|&word| b.word_value(word)).collect();
                let result = b
                    .call(callee, &args)
                    .expect("a loaded-method call answers a word");
                b.set_name(result, lane);
                b.expect_word(result)
            }
        };
        let result =
            self.operands()
                .finish(b, custody, result, "call_loaded_method_result", |_| {});
        let result = b.word_value(result);
        self.bind_owned(b, op, result);
    }

    /// `DirectCall` (HF-821 D4): hand the callable to the callee's frame entry
    /// through the thread record's invocation word, call the compiled target
    /// with the operands after the callable (`compiled_call`), and put the
    /// outer handoff back.
    pub(crate) fn guarded_direct_call_op(&mut self, b: &mut B, op: &TirOp) {
        let Some((target, abi)) = self.linked_target(op) else {
            self.fatal("DirectCall names no compiled target");
            return;
        };
        let callee = self.operands().resolve(b, op.operands[0]);
        let callee = scalar::word_of(b, callee);
        let thread = b.thread_record_word();
        let record = b.word_to_address(thread);
        let offset = molt_codegen_abi::THREAD_RECORD_PENDING_INVOCATION_OFFSET;
        let outer = b.load_word(record, offset);
        b.store_word(record, offset, callee);
        self.compiled_call(b, op, &target, &abi, &op.operands[1..], 1);
        b.store_word(record, offset, outer);
    }

    /// Bind None as `op`'s first result.
    fn bind_none(&mut self, b: &mut B, op: &TirOp) {
        if let Some(&result) = op.results.first() {
            let none = b.constant(NONE_WORD);
            let none = b.word_value(none);
            self.bind(result, none, HeldCarrier::Boxed);
        }
    }

    /// A call consuming a frontend-built CallArgs builder: `call_bind` and
    /// `call_indirect` free their builder (the generated `[[consuming_kind]]`
    /// facts), and on entry they observe an exception left pending by an
    /// earlier builder step, releasing the builder and returning None without
    /// running the callee. The callable is borrowed. The builder is an object
    /// carrier and is requested first, so its word dominates the failure
    /// block, which releases it when a failed callable box skips the call.
    ///
    /// With `owned_callable` the entry consumes the callable as well (an
    /// ordinary call that adopted it), so a box custody minted for it
    /// belongs to the entry once the call runs.
    fn builder_consuming_call(
        &self,
        b: &mut B,
        callable: ValueId,
        builder: ValueId,
        lane: &str,
        function: RuntimeFn,
        owned_callable: bool,
    ) -> B::Word {
        let view = self.operands();
        assert_eq!(
            view.held(builder),
            HeldCarrier::Boxed,
            "{lane} builder %{} must be an object carrier",
            builder.0
        );
        let mut custody = view.begin_call(
            b,
            &[builder, callable],
            &[builder],
            owned_callable.then_some(callable),
            lane,
        );
        let builder_word = view.operand(b, &mut custody, builder);
        let callable_word = view.operand(b, &mut custody, callable);
        if owned_callable {
            view.surrender(b, &custody, &[callable], &[]);
        }
        let site = self.next_call_site(b, lane);
        let bound = call_word(
            b,
            function,
            &[site, callable_word, builder_word],
            function.symbol(),
        );
        view.finish(b, custody, bound, &format!("{lane}_result"), |_| {})
    }

    /// `CallMethod`: receiver.method(args...) through
    /// `molt_call_bind_ic(site, bound_method, CallArgs(args))`, or the owned
    /// lane for an ordinary call that adopted its bound method and arguments
    /// (it ends a temporary bound method of a Python function before that
    /// function runs). The bound method and arguments are borrowed: the
    /// trampoline decodes each boxed word into its parameter's carrier, so a
    /// raw argument is boxed once and a minted box released after the call.
    pub(crate) fn call_method_op(&mut self, b: &mut B, op: &TirOp) {
        if op.operands.is_empty() {
            return;
        }
        if op.argument_custody().is_some() {
            let result = self.call_func_owned_runtime(b, op);
            let result = b.word_value(result);
            self.bind_owned(b, op, result);
            return;
        }
        let view = self.operands();
        let mut custody = view.begin(b, &op.operands, "call_method");
        let method = view.operand(b, &mut custody, op.operands[0]);
        let args: Vec<B::Word> = op.operands[1..]
            .iter()
            .map(|&arg| view.operand(b, &mut custody, arg))
            .collect();
        let site = self.next_call_site(b, "call_method");
        let bind = b.runtime_callee(RuntimeFn::CallBindIc);
        let bound = with_callargs(b, &args, |b, builder| {
            let words = [
                b.word_value(site),
                b.word_value(method),
                b.word_value(builder),
            ];
            let value = b
                .call(bind, &words)
                .expect("molt_call_bind_ic returns a word");
            b.set_name(value, "call_method_bind");
            b.expect_word(value)
        });
        let result = self
            .operands()
            .finish(b, custody, bound, "call_method_result", |_| {});
        let result = b.word_value(result);
        self.bind_owned(b, op, result);
    }

    /// The address of method site `site`'s record in this function, as a
    /// word: one writable record per site, created with the image of the
    /// site's load (its attribute name). None when no load names the site.
    pub(crate) fn method_site_record(&self, b: &mut B, site: u32) -> Option<B::Word> {
        let name = self
            .func
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .find_map(|op| match op.attrs.get("name") {
                Some(AttrValue::Str(name))
                    if op.opcode == OpCode::LoadMethod && op.method_site() == Some(site) =>
                {
                    Some(name.as_str())
                }
                _ => None,
            })?;
        let mut image = vec![0u8; method_site_record_size(name.len())];
        write_method_site_record_image(name.as_bytes(), &mut image);
        let symbol = format!("__molt_method_site_{}_{site}", self.func.name);
        let record = b.site_record(&symbol, &image);
        let word = b.address_to_word(record);
        let named = b.word_value(word);
        b.set_name(named, "method_site_record");
        Some(word)
    }

    /// CPython's LOAD_ATTR / LOAD_SUPER_ATTR in method form: a method site's
    /// load body, whose miss is `molt_method_site_load(record, receiver,
    /// self_out)`, or `molt_load_super_method(site, class, first_arg, name,
    /// len, self_out)`. The load returns the callable and writes
    /// `self_or_null` (the receiver or the null sentinel) to an entry slot.
    /// Operands are borrowed; both results are owned.
    pub(crate) fn load_method_op(&mut self, b: &mut B, op: &TirOp) {
        let (lane, operand_count) = if op.opcode == OpCode::LoadSuperMethod {
            ("load_super_method", 2)
        } else {
            ("load_method", 1)
        };
        let Some(AttrValue::Str(name)) = op.attrs.get("name") else {
            self.fatal(format!(
                "malformed {op:?}: a method load names its attribute"
            ));
            return;
        };
        let &[callable_id, self_id] = op.results.as_slice() else {
            self.fatal(format!("malformed {op:?}: a method load has two results"));
            return;
        };
        if op.operands.len() != operand_count {
            self.fatal(format!(
                "malformed {op:?}: {operand_count} operand(s) expected"
            ));
            return;
        }
        let null = b.constant(molt_codegen_abi::loaded_method_null_bits() as u64);
        let self_slot = b.entry_slot(&format!("{lane}_self"));
        // The slot holds the null sentinel until the runtime writes it, so a
        // call that raises before its write leaves a releasable word.
        b.store_slot(self_slot, null);
        let view = self.operands();
        let mut custody = view.begin(b, &op.operands, lane);
        let operands: Vec<B::Word> = op
            .operands
            .iter()
            .map(|&operand| view.operand(b, &mut custody, operand))
            .collect();
        let self_out = b.slot_pointer(self_slot);
        let self_out = b.address_to_word(self_out);
        let named = b.word_value(self_out);
        b.set_name(named, &format!("{lane}_self_ptr"));
        let callable = if op.opcode == OpCode::LoadMethod {
            let Some(site) = op.method_site() else {
                self.fatal(format!("{op:?}: a method load is a method site"));
                return;
            };
            let Some(record) = self.method_site_record(b, site) else {
                self.fatal(format!("{op:?}: method site {site} has no record"));
                return;
            };
            let admission = b.refcount_admission();
            let words = [record, operands[0], self_out, admission].map(|word| b.word_value(word));
            let callable = b.site(SiteBody::MethodSiteLoad, &words);
            b.expect_word(callable)
        } else {
            let callee = b.runtime_callee(RuntimeFn::LoadSuperMethod);
            let site = self.next_call_site(b, lane);
            let (name_pointer, name_len) = raw_name_pointer(b, name);
            let mut args = vec![b.word_value(site)];
            args.extend(operands.iter().map(|&operand| b.word_value(operand)));
            args.extend([name_pointer, b.word_value(name_len), b.word_value(self_out)]);
            let callable = b
                .call(callee, &args)
                .expect("molt_load_super_method returns a word");
            b.set_name(callable, lane);
            b.expect_word(callable)
        };
        // A receiver that could not be boxed was never loaded: its pair is
        // None and the null sentinel, never a previous load's words.
        let callable =
            self.operands()
                .finish(b, custody, callable, &format!("{lane}_callable"), |b| {
                    b.store_slot(self_slot, null)
                });
        let self_or_null = b.load_slot(self_slot);
        let self_or_null = b.word_value(self_or_null);
        b.set_name(self_or_null, &format!("{lane}_self_or_null"));
        let callable = b.word_value(callable);
        self.bind(callable_id, callable, HeldCarrier::Boxed);
        self.bind(self_id, self_or_null, HeldCarrier::Boxed);
    }

    /// Consume a loaded receiver before evaluating user arguments. The
    /// builder borrows its existing carrier; push_pos retains a real
    /// receiver, whose original credit ends even when the push fails. The
    /// null sentinel never crosses a Python-object or refcount boundary.
    pub(crate) fn push_method_receiver_op(&mut self, b: &mut B, op: &TirOp) {
        let &[builder, receiver] = op.operands.as_slice() else {
            self.fatal(format!(
                "malformed {op:?}: method receiver prefix takes two operands"
            ));
            return;
        };
        let operands = self.operands();
        let builder = operands.resolve(b, builder);
        let builder = scalar::word_of(b, builder);
        let receiver = operands.resolve(b, receiver);
        let receiver = scalar::word_of(b, receiver);
        let null = b.constant(molt_codegen_abi::loaded_method_null_bits() as u64);
        let present = b.compare(WordComparison::NotEqual, receiver, null);
        let named = b.bit_value(present);
        b.set_name(named, "method_receiver_present");
        let push = b.create_block("method_receiver_push");
        let join = b.create_block("method_receiver_done");
        b.branch_if(present, (push, &[]), (join, &[]));
        b.switch_to_block(push);
        let callee = b.runtime_callee(RuntimeFn::CallargsPushPos);
        let words = [b.word_value(builder), b.word_value(receiver)];
        let result = b
            .call(callee, &words)
            .expect("molt_callargs_push_pos returns a word");
        b.set_name(result, "method_receiver_push_result");
        self.bind_owned(b, op, result);
        let release = b.refcount_site(RefcountOperation::Release);
        b.refcount(release, receiver);
        b.jump(join, &[]);
        b.switch_to_block(join);
    }

    /// `CallBuiltin`. IR owns executable identity and argument roles: a
    /// named call carries arguments only; dynamic lookup alone takes operand
    /// zero as the name.
    pub(crate) fn call_builtin_op(&mut self, b: &mut B, op: &TirOp) {
        let Some(call) = op.builtin_call() else {
            self.fatal("malformed CallBuiltin identity or argument contract");
            return;
        };
        if matches!(call.wire_kind, OpKind::Print | OpKind::BuiltinPrint) {
            // The frontend joined a multi-argument print into one display
            // string with explicit newline behavior. Each argument is
            // borrowed by its print call; a failed box skips every later
            // print.
            let print = b.runtime_callee(RuntimeFn::PrintObj);
            let view = self.operands();
            let mut custody = view.begin(b, call.arguments, "print");
            for &argument in call.arguments {
                let word = view.operand(b, &mut custody, argument);
                let word = b.word_value(word);
                b.call(print, &[word]);
            }
            let none = b.constant(NONE_WORD);
            view.finish(b, custody, none, "print_result", |_| {});
            self.bind_none(b, op);
        } else if call.wire_kind == OpKind::RangeNew {
            // `range(...)` is its own frontend op, never a builtin lookup:
            // `molt_call_builtin` would reach the builtins module cache, which
            // a call before its population fails. The frontend materializes
            // all three bounds, which the constructor borrows (it copies
            // their values).
            let callee = b.runtime_callee(RuntimeFn::RangeNew);
            let bounds: Vec<RuntimeArg<B::Value>> = call
                .arguments
                .iter()
                .map(|&bound| RuntimeArg::Operand(bound))
                .collect();
            self.runtime_call(
                b,
                op,
                CallTarget::Callee(callee),
                &bounds,
                ResultCustody::canonical(RuntimeFn::RangeNew, 3),
                "range_new",
                "range_new",
            );
        } else {
            // Build arguments only after a synthesized name is admitted. The
            // dynamic-name lane borrows its existing SSA operand instead.
            let result = match call.target {
                BuiltinCallTarget::Named(name) => with_name(b, name, |b, name| {
                    self.builtin_with_name(b, RuntimeArg::Word(name), call.arguments)
                }),
                BuiltinCallTarget::Dynamic(&name) => {
                    self.builtin_with_name(b, RuntimeArg::Operand(name), call.arguments)
                }
            };
            let result = b.word_value(result);
            self.bind_owned(b, op, result);
        }
    }

    /// `molt_call_builtin(name, CallArgs(arguments))`, which consumes the
    /// builder. The arguments, and a dynamic name operand, are borrowed
    /// through one custody.
    fn builtin_with_name(
        &self,
        b: &mut B,
        name: RuntimeArg<B::Word>,
        arguments: &[ValueId],
    ) -> B::Word {
        let mut operands = Vec::with_capacity(arguments.len() + 1);
        if let RuntimeArg::Operand(name) = name {
            operands.push(name);
        }
        operands.extend_from_slice(arguments);
        let view = self.operands();
        let mut custody = view.begin(b, &operands, "builtin_call");
        let name = match name {
            RuntimeArg::Operand(name) => view.operand(b, &mut custody, name),
            RuntimeArg::Word(word) => word,
        };
        let args: Vec<B::Word> = arguments
            .iter()
            .map(|&argument| view.operand(b, &mut custody, argument))
            .collect();
        let call_builtin = b.runtime_callee(RuntimeFn::CallBuiltin);
        let bound = with_callargs(b, &args, |b, builder| {
            let words = [b.word_value(name), b.word_value(builder)];
            let value = b
                .call(call_builtin, &words)
                .expect("molt_call_builtin returns a word");
            b.set_name(value, "call_builtin");
            b.expect_word(value)
        });
        view.finish(b, custody, bound, "builtin_call_result", |_| {})
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
    use super::super::scalar::tests::{FUNCTION_WORD, Value, compiled_result, literal_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};
    use std::collections::BTreeMap;

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
        Owned(Option<RuntimeFn>),
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
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &sites,
            };
            let args: Vec<ValueId> = (1..inputs.len() as u32).map(ValueId).collect();
            let word = match *route {
                Route::Func => cx.call_func_runtime(b, ValueId(0), &args),
                Route::Bind => cx.call_bind_runtime(b, ValueId(0), &args),
                Route::FuncOrBind => cx.call_func_or_bind_runtime(b, ValueId(0), &args),
                Route::Owned(candidate) => {
                    let mut op = TirOp {
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
                    if let Some(candidate) = candidate {
                        op.attrs.insert(molt_ir::tir::call_targets::RUNTIME_CANDIDATE_ATTR.into(),
                            AttrValue::RuntimeCandidate(candidate));
                        op.origin = Some(OpKind::CallFunc);
                        op.set_argument_custody(&vec![ParameterCustody::Transferred; inputs.len()]);
                    }
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
        let run = call(Route::Owned(None), &inputs, false);
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

    #[test]
    fn positional_runtime_candidate_is_passed_with_the_original_inputs() {
        let candidate = RuntimeFn::Buffer2dGet;
        let inputs = [OBJECT, inline(91), inline(2), inline(7)];
        let run = call(Route::Owned(Some(candidate)), &inputs, false);
        let mut expected = key(&inputs);
        expected.extend(candidate.callable().unwrap().key.to_le_bytes());
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::CallFuncOwned,
                &expected
            ))]
        );
        assert!(run.refcounts.is_empty());
    }

    /// A second boxed object (a builder, a class).
    const OTHER: u64 = (0x7ff8 << 48) | (4 << 48) | 0x3000;

    /// What a test lowers.
    #[derive(Clone, Copy, Debug)]
    enum Family {
        Call,
        CallMethod,
        LoadMethod,
        PushReceiver,
        CallBuiltin,
        /// `DirectCall`, between a store of [`OUTER_INVOCATION`] into the thread
        /// record's invocation word and a read of it, which follows the results.
        DirectCall,
    }

    /// The invocation word an enclosing call left in the thread record.
    const OUTER_INVOCATION: u64 = 0x5eed_0000_0001;

    fn op(
        opcode: OpCode,
        origin: Option<OpKind>,
        operands: &[u32],
        results: &[u32],
        name: Option<&str>,
    ) -> TirOp {
        let mut attrs = AttrDict::default();
        if let Some(name) = name {
            attrs.insert("name".into(), AttrValue::Str(name.into()));
        }
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: operands.iter().map(|&id| ValueId(id)).collect(),
            results: results.iter().map(|&id| ValueId(id)).collect(),
            attrs,
            source_span: None,
            origin,
            runtime: None,
            callee: None,
        }
    }

    /// Lower `op` by `family` over boxed inputs %0.. in a program whose
    /// compiled functions are `linkage`; the fact is the diagnostics, the
    /// results the op's bound results.
    fn lower_linked(
        family: Family,
        op: TirOp,
        inputs: &[u64],
        linkage: BTreeMap<String, NativeFunctionLinkageAbi>,
        machine_only: bool,
    ) -> Run<Vec<String>> {
        let lowering = kernel!(family: Family = family, op: TirOp = op, linkage: BTreeMap<String, NativeFunctionLinkageAbi> = linkage; |b, inputs| -> Vec<String> {
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
            let mut invocation = None;
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &runtime,
                linkage,
                functions: &Default::default(),
                call_sites: &sites,
            };
            match *family {
                Family::Call => cx.call_op(b, op),
                Family::CallMethod => cx.call_method_op(b, op),
                Family::LoadMethod => cx.load_method_op(b, op),
                Family::PushReceiver => cx.push_method_receiver_op(b, op),
                Family::CallBuiltin => cx.call_builtin_op(b, op),
                Family::DirectCall => {
                    let thread = b.thread_record_word();
                    let record = b.word_to_address(thread);
                    let offset = molt_codegen_abi::THREAD_RECORD_PENDING_INVOCATION_OFFSET;
                    let outer = b.constant(OUTER_INVOCATION);
                    b.store_word(record, offset, outer);
                    cx.guarded_direct_call_op(b, op);
                    invocation = Some(b.load_word(record, offset));
                }
            }
            let mut results: Vec<B::Value> = op
                .results
                .iter()
                .filter_map(|&result| cx.values.value(result))
                .collect();
            results.extend(invocation.map(|word| b.word_value(word)));
            (diagnostics.borrow().clone(), results)
        });
        let inputs: Vec<Value> = inputs.iter().copied().map(Value::Word).collect();
        if machine_only {
            machine(&lowering, &inputs, false)
        } else {
            both(&lowering, &inputs)
        }
    }

    /// [`lower_linked`] in a program with no compiled functions.
    fn lower(family: Family, op: TirOp, inputs: &[u64], machine_only: bool) -> Run<Vec<String>> {
        lower_linked(family, op, inputs, BTreeMap::new(), machine_only)
    }

    /// The boxed id of the first inline-cache site of `lane` in `calls`.
    fn site(lane: &str) -> u64 {
        let site = molt_codegen_abi::stable_ic_site_id("calls", 0, lane);
        molt_codegen_abi::box_int_bits(site) as u64
    }

    /// The model's CallArgs builder for `count` positional arguments.
    fn callargs(count: u64) -> u64 {
        literal_word(RuntimeFn::CallargsNew, &count.to_le_bytes())
    }

    #[test]
    fn a_builder_consuming_call_passes_its_site_callable_and_builder() {
        let call = op(OpCode::Call, Some(OpKind::CallBind), &[0, 1], &[2], None);
        let run = lower(Family::Call, call, &[OBJECT, OTHER], false);
        assert!(run.fact.is_empty());
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::CallBindIc,
                &key(&[site("call_bind"), OBJECT, OTHER])
            ))]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::CallBindIc]);
    }

    /// A direct call of `target` with operands %0.. and result %`result`.
    fn direct_call(target: &str, operands: u32, result: u32) -> TirOp {
        let mut call = op(
            OpCode::Call,
            None,
            &(0..operands).collect::<Vec<_>>(),
            &[result],
            None,
        );
        call.attrs
            .insert("_var".into(), AttrValue::Str(target.into()));
        call
    }

    /// One compiled function `symbol` of `params` returning `result`.
    fn linked(
        symbol: &str,
        params: Vec<TirType>,
        result: Option<TirType>,
    ) -> BTreeMap<String, NativeFunctionLinkageAbi> {
        let abi = NativeFunctionLinkageAbi {
            source_signature: crate::ir::ExternFunctionSignature {
                arity: params.len(),
                has_closure: false,
                returns_value: result.is_some(),
                execution_context: crate::ir::ExecutionContextPolicy::None,
            },
            parameter_custody: Vec::new(),
            param_types: params,
            return_type: result,
        };
        BTreeMap::from([(symbol.to_owned(), abi)])
    }

    #[test]
    fn a_direct_call_passes_each_argument_in_its_parameter_carrier() {
        // %0 stays boxed for a boxed parameter; %1 is decoded for a raw one.
        let symbol = "walker_compiled_boxed";
        let program = linked(
            symbol,
            vec![TirType::DynBox, TirType::I64],
            Some(TirType::DynBox),
        );
        let call = direct_call(symbol, 2, 2);
        let run = lower_linked(Family::Call, call, &[OBJECT, inline(5)], program, false);
        assert!(run.fact.is_empty(), "{:?}", run.fact);
        assert_eq!(
            run.results,
            [Value::Word(compiled_result(symbol, &[OBJECT, 5]))]
        );
    }

    #[test]
    fn a_raw_direct_call_result_is_boxed_for_its_boxed_result() {
        let symbol = "walker_compiled_raw";
        let program = linked(symbol, vec![TirType::I64, TirType::I64], Some(TirType::I64));
        let call = direct_call(symbol, 2, 2);
        let run = lower_linked(Family::Call, call, &[inline(2), inline(3)], program, false);
        assert!(run.fact.is_empty(), "{:?}", run.fact);
        assert_eq!(
            run.results,
            [Value::Word(inline(compiled_result(symbol, &[2, 3]) as i64))]
        );
    }

    #[test]
    fn a_direct_call_hands_its_callable_to_the_callee_and_restores_the_outer_word() {
        // %0 is the callable, %1 the argument. The callee's frame entry reads
        // the invocation word on entry; the enclosing call's word comes back.
        let symbol = "walker_compiled_handoff";
        let program = linked(symbol, vec![TirType::DynBox], Some(TirType::DynBox));
        let mut call = direct_call(symbol, 2, 2);
        call.opcode = OpCode::DirectCall;
        // Both machines: the interpreter and Cranelift machine code.
        let run = lower_linked(
            Family::DirectCall,
            call,
            &[FUNCTION_WORD, OBJECT],
            program,
            false,
        );
        assert!(run.fact.is_empty(), "{:?}", run.fact);
        assert_eq!(
            run.results,
            [
                Value::Word(compiled_result(symbol, &[OBJECT, FUNCTION_WORD])),
                Value::Word(OUTER_INVOCATION),
            ]
        );
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn a_direct_call_of_an_unlinked_target_is_refused() {
        let run = lower(Family::Call, direct_call("f", 1, 1), &[OBJECT], false);
        assert!(
            run.fact.len() == 1 && run.fact[0].contains("`f` has no exact native linkage ABI"),
            "{:?}",
            run.fact
        );
        assert_eq!(run.results, [Value::Word(NONE_WORD)]);
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn a_method_call_binds_its_arguments_through_its_site() {
        let call = op(OpCode::CallMethod, None, &[0, 1], &[2], None);
        let run = lower(Family::CallMethod, call, &[OBJECT, inline(5)], false);
        let mut expected = key(&[site("call_method"), OBJECT, callargs(1)]);
        expected.extend(key(&[inline(5)]));
        assert_eq!(
            run.results,
            [Value::Word(literal_word(RuntimeFn::CallBindIc, &expected))]
        );
        assert_eq!(
            run.runtime_calls,
            [
                RuntimeFn::CallargsNew,
                RuntimeFn::CallargsPushPos,
                RuntimeFn::CallBindIc
            ]
        );
    }

    #[test]
    fn a_super_method_load_answers_the_callable_and_writes_self() {
        let load = op(OpCode::LoadSuperMethod, None, &[0, 1], &[2, 3], Some("m"));
        let run = lower(Family::LoadMethod, load, &[OTHER, OBJECT], false);
        let mut expected = key(&[site("load_super_method"), OTHER, OBJECT]);
        expected.extend(b"m");
        assert_eq!(
            run.results,
            [
                Value::Word(literal_word(RuntimeFn::LoadSuperMethod, &expected)),
                Value::Word(OBJECT),
            ]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::LoadSuperMethod]);
    }

    #[test]
    fn only_a_present_receiver_is_pushed_and_its_credit_ended() {
        let push = || op(OpCode::CallargsPushMethodReceiver, None, &[0, 1], &[], None);
        let null = molt_codegen_abi::loaded_method_null_bits() as u64;
        let run = lower(Family::PushReceiver, push(), &[OTHER, null], true);
        assert!(run.runtime_calls.is_empty() && run.refcounts.is_empty());
        let run = lower(Family::PushReceiver, push(), &[OTHER, OBJECT], true);
        assert_eq!(run.runtime_calls, [RuntimeFn::CallargsPushPos]);
        assert_eq!(run.refcounts, [(RefcountOperation::Release, OBJECT)]);
    }

    #[test]
    fn a_named_builtin_call_borrows_its_arguments_and_its_immortal_name() {
        let call = op(OpCode::CallBuiltin, None, &[0], &[1], Some("len"));
        let run = lower(Family::CallBuiltin, call, &[OBJECT], true);
        let name = literal_word(RuntimeFn::StringFromBytes, b"len");
        let mut expected = key(&[name, callargs(1)]);
        expected.extend(key(&[OBJECT]));
        assert_eq!(
            run.results,
            [Value::Word(literal_word(RuntimeFn::CallBuiltin, &expected))]
        );
        assert_eq!(
            run.runtime_calls,
            [
                RuntimeFn::LiteralCellFill,
                RuntimeFn::CallargsNew,
                RuntimeFn::CallargsPushPos,
                RuntimeFn::CallBuiltin
            ]
        );
        assert!(run.refcounts.is_empty(), "{:?}", run.refcounts);
    }
}
