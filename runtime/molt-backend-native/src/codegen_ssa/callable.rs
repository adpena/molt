//! The walker's callable family (design: `docs/design/foundation/codegen_walker.md`,
//! W2): the callable ops that are runtime calls, and function construction
//! (`builtin_func`, `func_new`, `func_new_closure`) with the callable and
//! task trampolines as helper bodies, written once for every native backend.
//! `callable_preserved_op` is the family's table for preserved `Copy` ops.

use molt_codegen_abi::task_site::TaskConstructorLayout;
use molt_ir::OpKind;
use molt_ir::code_slots::ProgramSlotKind;
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
use molt_ir::runtime_fn_generated::{RuntimeBoxedReturn, RuntimeFn};

use super::builder::{CodegenBuilder, HelperBody, MachineKind};
use super::custody::{CallTarget, RuntimeArg, release_call_inputs};
use super::function::{FunctionCx, ResultCustody};
use super::task::{TaskPlacement, task_trampoline_word};
use super::value_table::HeldCarrier;
use super::{control, scalar};
use crate::ir::ParameterCustody;
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::types::TirType;
use molt_tir::trampolines::TrampolineTaskKind;

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// A preserved callable op that is a runtime call: `function_closure_bits`,
    /// `asyncgen_new`, `code_slot_set`, `code_slots_init`, `callargs_new`,
    /// `locals()` and its setter, and the `line` marker. Answers false for any other kind, or for a
    /// malformed op the caller then refuses.
    pub(crate) fn callable_runtime_op(&mut self, b: &mut B, op: &TirOp, kind: OpKind) -> bool {
        let unary = |op: &TirOp| op.operands.len() == 1 && op.results.len() <= 1;
        match kind {
            // The function owns this edge: only a kept result gains an owner,
            // and a discarded borrowed edge is never released.
            OpKind::FunctionClosureBits if unary(op) => self.borrowed_call(
                b,
                op,
                RuntimeFn::FunctionClosureBits,
                &[RuntimeArg::Operand(op.operands[0])],
                ResultCustody::Boxed(RuntimeBoxedReturn::BorrowedValue),
                "function_closure_bits",
            ),
            // The dedicated task-wrapper ABI.
            OpKind::AsyncgenNew if unary(op) => self.borrowed_call(
                b,
                op,
                RuntimeFn::AsyncgenNew,
                &[RuntimeArg::Operand(op.operands[0])],
                ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                "asyncgen_new",
            ),
            OpKind::FunctionClosureBits | OpKind::AsyncgenNew => return false,
            OpKind::CodeSlotSet => {
                // The slot id is a raw ABI word; the code object and the
                // globals are borrowed. Only the exception state reports
                // failure.
                let owner = match op.attrs.get("s_value") {
                    Some(AttrValue::Str(owner)) => owner.as_str(),
                    _ => panic!("admitted program-slot owner"),
                };
                let code = b.program_slot(ProgramSlotKind::Code, owner);
                let code = b.word_value(code);
                self.borrowed_call(
                    b,
                    op,
                    RuntimeFn::CodeSlotSet,
                    &[
                        RuntimeArg::Word(code),
                        RuntimeArg::Operand(op.operands[0]),
                        RuntimeArg::Operand(op.operands[1]),
                    ],
                    ResultCustody::SideEffect,
                    "code_slot_set",
                );
            }
            OpKind::CodeSlotsInit => {
                let Some(&AttrValue::Int(count)) = op.attrs.get("value") else {
                    panic!("admitted code_slots_init count");
                };
                let count = b.constant(count as u64);
                b.call_runtime(RuntimeFn::CodeSlotsInit, &[count]);
            }
            OpKind::CallargsNew => {
                // The source call form picks the builder: a CALL_FUNCTION_EX
                // site's arguments are its own tuple and mapping.
                let constructor = op
                    .call_argument_form()
                    .expect("validated callargs_new call form")
                    .runtime_constructor();
                let zero = b.constant(0);
                let builder = b
                    .call_runtime(constructor, &[zero, zero])
                    .expect("a CallArgs constructor answers a word");
                let builder = b.word_value(builder);
                b.set_name(builder, "callargs_new");
                self.bind_owned(b, op, builder);
            }
            // `locals()`: the runtime is the one authority over the executing
            // frame's bindings.
            OpKind::FrameLocals if op.operands.is_empty() => self.borrowed_call(
                b,
                op,
                RuntimeFn::LocalsBuiltin,
                &[],
                ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                "frame_locals",
            ),
            OpKind::FrameLocalsSet if op.operands.len() == 1 => self.borrowed_call(
                b,
                op,
                RuntimeFn::FrameLocalsSet,
                &[RuntimeArg::Operand(op.operands[0])],
                ResultCustody::SideEffect,
                "frame_locals_set",
            ),
            OpKind::FrameLocals | OpKind::FrameLocalsSet => return false,
            // A statement marker: it emits no code. Tracebacks read the
            // location `frame_set_location` publishes before each raising op.
            OpKind::Line => {}
            _ => return false,
        }
        true
    }

    /// A preserved callable op: a `Copy` whose origin names a callable
    /// protocol with its own lowering. Answers false for a kind this family
    /// does not own (the frame family's homes, entry and exit), or for a
    /// malformed op the caller then refuses.
    pub(crate) fn callable_preserved_op(&mut self, b: &mut B, op: &TirOp, kind: OpKind) -> bool {
        match kind {
            OpKind::FunctionClosureBits
            | OpKind::AsyncgenNew
            | OpKind::CodeSlotSet
            | OpKind::CodeSlotsInit
            | OpKind::CallargsNew
            | OpKind::FrameLocals
            | OpKind::FrameLocalsSet
            | OpKind::Line => self.callable_runtime_op(b, op, kind),
            OpKind::BuiltinFunc | OpKind::FuncNew | OpKind::FuncNewClosure => {
                self.function_construction_op(b, op, kind)
            }
            _ => false,
        }
    }

    /// One borrowed runtime call of `function` over `args`, bound by
    /// `custody`, labelled by `name`.
    fn borrowed_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        function: RuntimeFn,
        args: &[RuntimeArg<B::Value>],
        custody: ResultCustody,
        name: &str,
    ) {
        let callee = b.runtime_callee(function);
        let label = op.origin.map_or(name, |kind| kind.as_str());
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(callee),
            args,
            custody,
            label,
            name,
        );
    }
}

/// The boxed word of `None`.
const NONE_WORD: u64 = molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE;

/// What a callable trampoline calls.
#[derive(Clone, Copy)]
enum TrampolineTarget<'a> {
    /// A compiled function, declared by its frozen linkage row.
    Compiled {
        symbol: &'a str,
        abi: &'a NativeFunctionLinkageAbi,
    },
    /// A runtime entry whose callable ABI unpacks its arguments.
    Runtime(RuntimeFn),
}

/// The entry the runtime calls a function object through: `(closure, argv,
/// argc)` after the execution context. It decodes each boxed argument in
/// `argv` into the representation its parameter declares (a raw integer
/// param takes the full-range integer, not the boxed bits), calls the
/// target, ends the references an adopting entry kept only as extracted
/// values, and boxes the result for the word ABI.
struct Trampoline<'a> {
    target: TrampolineTarget<'a>,
    has_closure: bool,
    /// One type per target parameter; the closure is first when present.
    params: &'a [TirType],
    result: Option<&'a TirType>,
    custody: &'a [ParameterCustody],
}

impl Trampoline<'_> {
    /// The helper's name: one per target, arity and closure shape.
    fn name(symbol: &str, params: usize, has_closure: bool) -> String {
        let arity = params - usize::from(has_closure);
        let closure = if has_closure { "_closure" } else { "" };
        format!("{symbol}__molt_callable_trampoline_{arity}{closure}")
    }
}

impl HelperBody for Trampoline<'_> {
    fn build<H: TaskPlacement>(&self, h: &mut H, params: &[H::Word]) -> H::Word {
        let &[closure, argv, _argc] = params else {
            panic!("a callable trampoline takes (closure, argv, argc)")
        };
        let callee = match self.target {
            TrampolineTarget::Compiled { symbol, abi } => h.function_callee(symbol, abi),
            TrampolineTarget::Runtime(function) => h.runtime_callee(function),
        };
        let first = usize::from(self.has_closure);
        let mut args = Vec::with_capacity(self.params.len());
        if self.has_closure {
            args.push(h.word_value(closure));
        }
        let argv = h.word_to_address(argv);
        // A raw parameter of an adopting entry keeps only the value extracted
        // from its box; the trampoline ends that reference after the call.
        let mut extracted = Vec::new();
        for (index, ty) in self.params.iter().enumerate().skip(first) {
            let boxed = h.load_word(argv, 8 * (index - first) as i32);
            let carrier = HeldCarrier::of(ty);
            if self.custody.get(index) == Some(&ParameterCustody::Transferred)
                && carrier != HeldCarrier::Boxed
            {
                extracted.push(boxed);
            }
            let word = scalar::unbox_to_param_word(h, boxed, ty);
            args.push(match control::carrier_kind(carrier) {
                MachineKind::Bit => {
                    let bit = h.low_bit(word);
                    h.bit_value(bit)
                }
                MachineKind::Float => h.bits_to_float(word),
                _ => h.word_value(word),
            });
        }
        let result = h.call(callee, &args).unwrap_or_else(|| {
            let none = h.constant(NONE_WORD);
            h.word_value(none)
        });
        release_call_inputs(h, None, &extracted);
        scalar::box_value(h, result, self.result.unwrap_or(&TirType::DynBox))
    }
}

/// A compiled function's plain trampoline (its target's frozen linkage row
/// declares every parameter, the closure first when present).
pub(crate) fn plain_trampoline<B: CodegenBuilder>(
    b: &mut B,
    symbol: &str,
    abi: &NativeFunctionLinkageAbi,
    has_closure: bool,
) -> B::Callee {
    assert!(
        !abi.checked_raw_return(),
        "a callable trampoline needs an every-exit return"
    );
    let name = Trampoline::name(symbol, abi.param_types.len(), has_closure);
    b.define_helper(
        &name,
        3,
        &Trampoline {
            target: TrampolineTarget::Compiled { symbol, abi },
            has_closure,
            params: &abi.param_types,
            result: abi.return_type.as_ref(),
            custody: &abi.parameter_custody,
        },
    )
}

/// The entry the runtime calls a task constructor through: `(closure, argv,
/// argc)` after the execution context, like any callable trampoline. It
/// allocates the task its typed metadata selects, with the poll's frame
/// size, and retains each argument into it (`task::task_trampoline_word`).
/// Python constructor arguments and the poll's one raw frame parameter are
/// different ABIs; the shared task kind and layout own that distinction.
struct TaskTrampoline<'a> {
    /// The poll: a compiled function of one frame parameter.
    symbol: &'a str,
    abi: &'a NativeFunctionLinkageAbi,
    kind: TrampolineTaskKind,
    layout: TaskConstructorLayout,
    closure_size: i64,
    arity: usize,
    has_closure: bool,
}

impl<'a> TaskTrampoline<'a> {
    /// The task trampoline `op` constructs through, from its typed task
    /// metadata; `None` for a plain callable. Partial metadata, an unknown
    /// kind or a closure size its layout refuses is a compiler error.
    fn of(
        op: &TirOp,
        symbol: &'a str,
        abi: &'a NativeFunctionLinkageAbi,
        arity: usize,
        has_closure: bool,
    ) -> Option<Self> {
        if !op.attrs.contains_key("task_kind") && !op.attrs.contains_key("task_closure_size") {
            return None;
        }
        let (kind, closure_size) =
            match (op.attrs.get("task_kind"), op.attrs.get("task_closure_size")) {
                (Some(AttrValue::Str(kind)), Some(AttrValue::Int(size))) => (
                    TrampolineTaskKind::from_constructor_kind(kind).unwrap_or_else(|| {
                        panic!("unknown callable task_kind `{kind}` for {symbol}")
                    }),
                    *size,
                ),
                _ => panic!(
                    "{symbol} requires complete callable task_kind and task_closure_size metadata"
                ),
            };
        let layout = crate::trampoline_task_layout(kind);
        layout.validate_closure_size(
            closure_size,
            arity,
            has_closure,
            crate::GENERATOR_CONTROL_BYTES,
        );
        Some(Self {
            symbol,
            abi,
            kind,
            layout,
            closure_size,
            arity,
            has_closure,
        })
    }

    /// The helper's name: one per poll, arity, closure shape, task kind and
    /// closure size.
    fn name(&self) -> String {
        format!(
            "{}__molt_task_trampoline_{}_{}{}_{}",
            self.symbol,
            self.arity,
            usize::from(self.has_closure),
            self.kind.trampoline_kind().symbol_suffix(),
            self.closure_size,
        )
    }
}

impl HelperBody for TaskTrampoline<'_> {
    fn build<H: TaskPlacement>(&self, h: &mut H, params: &[H::Word]) -> H::Word {
        let &[closure, arguments, _argc] = params else {
            panic!("a task trampoline takes (closure, argv, argc)")
        };
        let poll = h.function_callee(self.symbol, self.abi);
        let poll = h.function_address(poll);
        let frame_bytes = h.poll_frame_bytes(self.symbol);
        task_trampoline_word(
            h,
            self.layout,
            poll,
            frame_bytes,
            self.has_closure.then_some(closure),
            arguments,
            self.arity,
        )
    }
}

/// The integer attribute `value` (a construction's Python arity), or 0.
fn arity_attr(op: &TirOp) -> usize {
    match op.attrs.get("value") {
        Some(AttrValue::Int(value)) => usize::try_from(*value).unwrap_or(0),
        _ => 0,
    }
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Function construction: `builtin_func` (a runtime callable),
    /// `func_new` and `func_new_closure` (a compiled function). Each passes
    /// the target's code address, its trampoline's and the arity to the
    /// runtime constructor. Every trampoline, a task constructor's too, is a
    /// walker helper body. Answers false for a malformed op the caller then
    /// refuses.
    pub(crate) fn function_construction_op(&mut self, b: &mut B, op: &TirOp, kind: OpKind) -> bool {
        match kind {
            OpKind::BuiltinFunc => self.builtin_function(b, op),
            OpKind::FuncNew | OpKind::FuncNewClosure => {
                self.compiled_function(b, op, kind == OpKind::FuncNewClosure)
            }
            _ => false,
        }
    }

    /// `builtin_func`: the catalog key is the callable's identity (HF-881);
    /// its code address is only the target. Runtime function objects use
    /// the manifest's callable ABI, never a compiled function's
    /// representation or parameter custody.
    fn builtin_function(&mut self, b: &mut B, op: &TirOp) -> bool {
        use molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi;
        let Some(AttrValue::Str(name)) = op.attrs.get("s_value") else {
            return false;
        };
        let arity = arity_attr(op);
        // The typed runtime entry decoded at TIR ingest (HF-821 B1).
        let Some((function, key, abi)) = op.runtime.and_then(|function| {
            let callable = function.callable()?;
            Some((function, callable.key, callable.trampoline?))
        }) else {
            self.fatal(format!(
                "builtin_func target `{name}` has no runtime callable ABI; use func_new for compiled functions"
            ));
            return true;
        };
        if function.params() != arity {
            self.fatal(format!(
                "builtin_func arity mismatch for `{name}`: manifest requires {}, got {arity}",
                function.params()
            ));
            return true;
        }
        let target = b.runtime_callee(function);
        let trampoline = match abi {
            // This runtime entry already consumes (closure, argv, argc).
            RuntimeCallableTrampolineAbi::CallFrame => target,
            RuntimeCallableTrampolineAbi::UnpackArgs => {
                let params = vec![TirType::DynBox; arity];
                let custody = vec![ParameterCustody::Borrowed; arity];
                b.define_helper(
                    &Trampoline::name(name, arity, false),
                    3,
                    &Trampoline {
                        target: TrampolineTarget::Runtime(function),
                        has_closure: false,
                        params: &params,
                        result: Some(&TirType::DynBox),
                        custody: &custody,
                    },
                )
            }
        };
        let code = b.function_address(target);
        let trampoline = b.function_address(trampoline);
        let (constructor, mut args) = match op.operands.first() {
            Some(&name) => (
                RuntimeFn::FuncNewBuiltinNamed,
                vec![RuntimeArg::Operand(name)],
            ),
            None => (RuntimeFn::FuncNewBuiltin, Vec::new()),
        };
        let key = b.constant(key);
        let arity = b.constant(arity as u64);
        args.extend(
            [key, code, trampoline, arity].map(|word| RuntimeArg::Word(b.word_value(word))),
        );
        self.borrowed_call(
            b,
            op,
            constructor,
            &args,
            ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
            "builtin_func_new",
        );
        true
    }

    /// `func_new` and `func_new_closure`: the closure object is borrowed
    /// (the function retains it); code addresses, the arity and the entry
    /// custody are raw ABI words.
    fn compiled_function(&mut self, b: &mut B, op: &TirOp, has_closure: bool) -> bool {
        let symbol = match op.callee {
            Some(callee) => self.functions.symbol(callee).to_owned(),
            None => match op.attrs.get("s_value") {
                Some(AttrValue::Str(symbol)) if !symbol.is_empty() => symbol.clone(),
                _ => return false,
            },
        };
        let arity = arity_attr(op);
        let closure = match (has_closure, op.operands.first()) {
            (true, Some(&closure)) => Some(closure),
            (true, None) => return false,
            (false, _) => None,
        };
        let Some(abi) = self.linkage.get(&symbol) else {
            self.fatal(format!(
                "function symbol `{symbol}` has no exact native linkage ABI; refusing to invent one"
            ));
            return true;
        };
        let task = TaskTrampoline::of(op, &symbol, abi, arity, has_closure);
        let (target, trampoline, custody) = match task {
            // A task constructor's arguments enter through its task
            // trampoline, which retains each into the new task: it borrows.
            // Its target is the poll, whose one parameter is the task's frame.
            Some(task) => {
                assert_eq!(
                    abi.param_types.len(),
                    1,
                    "task poll `{symbol}` takes its frame alone"
                );
                let poll = b.function_callee(&symbol, abi);
                let trampoline = b.define_helper(&task.name(), 3, &task);
                (poll, trampoline, 0)
            }
            None => {
                assert_eq!(
                    arity + usize::from(has_closure),
                    abi.param_types.len(),
                    "caller/linkage arity mismatch for `{symbol}`"
                );
                let target = b.function_callee(&symbol, abi);
                let trampoline = plain_trampoline(b, &symbol, abi, has_closure);
                (
                    target,
                    trampoline,
                    entry_custody(&symbol, abi, has_closure, arity),
                )
            }
        };
        let code = b.function_address(target);
        let trampoline = b.function_address(trampoline);
        let arity = b.constant(arity as u64);
        let custody = b.constant(custody);
        match closure {
            None => {
                let result = b
                    .call_runtime(RuntimeFn::FuncNew, &[code, trampoline, arity, custody])
                    .expect("func_new answers a word");
                let result = b.word_value(result);
                b.set_name(result, "func_new");
                self.bind_owned(b, op, result);
            }
            Some(closure) => {
                let words =
                    [code, trampoline, arity].map(|word| RuntimeArg::Word(b.word_value(word)));
                let custody = RuntimeArg::Word(b.word_value(custody));
                self.borrowed_call(
                    b,
                    op,
                    RuntimeFn::FuncNewClosure,
                    &[
                        words[0],
                        words[1],
                        words[2],
                        RuntimeArg::Operand(closure),
                        custody,
                    ],
                    ResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    "func_new_closure",
                );
            }
        }
        true
    }
}

/// The entry-custody word for the function object `func_new` (or
/// `func_new_closure`) creates for `symbol`, from the parameter declaration
/// in its linkage row. A row the runtime cannot encode in one bit is a
/// compiler error: guessing "borrowed" would let a transferring body release
/// its caller's references.
fn entry_custody(
    symbol: &str,
    abi: &NativeFunctionLinkageAbi,
    has_closure: bool,
    arity: usize,
) -> u64 {
    let transferred: Vec<bool> = abi
        .parameter_custody
        .iter()
        .map(|custody| matches!(custody, ParameterCustody::Transferred))
        .collect();
    molt_codegen_abi::EntryCustodyDeclaration::declare(
        abi.source_signature.has_closure,
        abi.source_signature.arity,
        &transferred,
    )
    .encode(has_closure, arity)
    .unwrap_or_else(|error| {
        panic!("func_new target `{symbol}` has no runtime entry custody: {error:?}")
    })
}

#[cfg(test)]
mod tests {
    //! The callable family's runtime calls on the interpreting builder and as
    //! Cranelift machine code. The oracle is each call's contract: a closure
    //! read is borrowed and binds with its own reference; the line marker
    //! emits nothing.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{Run, both, every_runtime_entry, kernel};
    use super::super::scalar::tests::{Value, compiled_result, literal_word};
    use super::super::value_table::{HeldCarrier, ValueTable};
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use molt_codegen_abi::inline_refcount::RefcountOperation;

    const OBJECT: u64 = (0x7ff8 << 48) | (4 << 48) | 0x2000;

    fn lower(kind: OpKind) -> Run<(bool, Option<HeldCarrier>)> {
        let lowering = kernel!(kind: OpKind = kind; |b, inputs| -> (bool, Option<HeldCarrier>) {
            let func = TirFunction::new(
                "callable".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(4);
            values.set_value(ValueId(0), inputs[0]);
            values.set_repr(ValueId(0), HeldCarrier::Boxed);
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &every_runtime_entry(),
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &Cell::new(0),
            };
            let op = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![ValueId(0)],
                results: vec![ValueId(1)],
                attrs: AttrDict::default(),
                source_span: None,
                origin: Some(*kind),
                runtime: None,
                callee: None,
            };
            let lowered = cx.callable_runtime_op(b, &op, *kind);
            let results = cx.values.value(ValueId(1)).into_iter().collect();
            ((lowered, cx.values.repr(ValueId(1))), results)
        });
        both(&lowering, &[Value::Word(OBJECT)])
    }

    #[test]
    fn a_closure_read_is_borrowed_and_its_kept_result_retained() {
        let run = lower(OpKind::FunctionClosureBits);
        assert_eq!(run.fact, (true, Some(HeldCarrier::Boxed)));
        assert_eq!(run.runtime_calls, [RuntimeFn::FunctionClosureBits]);
        let Value::Word(closure) = run.results[0] else {
            panic!("a closure is a word")
        };
        assert_eq!(run.refcounts, [(RefcountOperation::Retain, closure)]);
    }

    #[test]
    fn the_line_marker_emits_nothing_and_other_kinds_are_left() {
        let run = lower(OpKind::Line);
        assert_eq!(run.fact, (true, None));
        assert!(run.runtime_calls.is_empty());
        let run = lower(OpKind::FuncNew);
        assert_eq!(run.fact, (false, None));
        // The frame family's homes, entry and exit are not this family's.
        for kind in [
            OpKind::FrameHomeLoad,
            OpKind::TraceEnterSlot,
            OpKind::ListNew,
        ] {
            assert_eq!(lower(kind).fact, (false, None), "{kind}");
        }
    }

    /// The preserved op `kind` through the family's table, over `operands`
    /// boxed %0.. and with result %9 when `result`.
    fn preserved(kind: OpKind, operands: usize, result: bool) -> Run<bool> {
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: (0..operands as u32).map(ValueId).collect(),
            results: if result { vec![ValueId(9)] } else { vec![] },
            attrs: AttrDict::default(),
            source_span: None,
            origin: Some(kind),
            runtime: None,
            callee: None,
        };
        let lowering = kernel!(kind: OpKind = kind, op: TirOp = op; |b, inputs| -> bool {
            let func = TirFunction::new(
                "callable".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(10);
            for (index, &input) in inputs.iter().enumerate() {
                values.set_value(ValueId(index as u32), input);
                values.set_repr(ValueId(index as u32), HeldCarrier::Boxed);
            }
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &every_runtime_entry(),
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &Cell::new(0),
            };
            let lowered = cx.callable_preserved_op(b, op, *kind);
            assert!(diagnostics.borrow().is_empty(), "{:?}", diagnostics.borrow());
            let results = cx.values.value(ValueId(9)).into_iter().collect();
            (lowered, results)
        });
        let inputs: Vec<Value> = (0..operands).map(|_| Value::Word(OBJECT)).collect();
        both(&lowering, &inputs)
    }

    #[test]
    fn locals_reads_and_replaces_the_frame_bindings_through_the_runtime() {
        let run = preserved(OpKind::FrameLocals, 0, true);
        assert!(run.fact);
        assert_eq!(run.runtime_calls, [RuntimeFn::LocalsBuiltin]);
        assert_eq!(
            run.results,
            [Value::Word(literal_word(RuntimeFn::LocalsBuiltin, &[]))]
        );
        // The owned mapping binds as is: no retain.
        assert!(run.refcounts.is_empty());
        let run = preserved(OpKind::FrameLocalsSet, 1, false);
        assert!(run.fact);
        assert_eq!(run.runtime_calls, [RuntimeFn::FrameLocalsSet]);
        assert!(run.results.is_empty());
        // A malformed op is refused before any code.
        for (kind, operands) in [(OpKind::FrameLocals, 1), (OpKind::FrameLocalsSet, 0)] {
            let run = preserved(kind, operands, false);
            assert!(!run.fact, "{kind}");
            assert!(run.runtime_calls.is_empty());
        }
    }

    /// A function construction of `kind` (`s_value` = `symbol`, `value` =
    /// `arity`) over boxed %0.. in a program whose compiled functions are
    /// `linkage`, as Cranelift machine code: the trampoline is a helper body,
    /// which the interpreter cannot hold. The runtime's constructor stub
    /// enters the trampoline with the inline ints 2, 3, ... as arguments.
    #[cfg(feature = "native-backend")]
    fn construct(
        kind: OpKind,
        symbol: &str,
        arity: usize,
        runtime: Option<RuntimeFn>,
        linkage: std::collections::BTreeMap<String, NativeFunctionLinkageAbi>,
        inputs: &[u64],
    ) -> Run<(bool, Vec<String>)> {
        let mut attrs = AttrDict::default();
        attrs.insert("s_value".into(), AttrValue::Str(symbol.into()));
        attrs.insert("value".into(), AttrValue::Int(arity as i64));
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: (0..inputs.len() as u32).map(ValueId).collect(),
            results: vec![ValueId(9)],
            attrs,
            source_span: None,
            origin: Some(kind),
            runtime,
            callee: None,
        };
        let lowering = kernel!(kind: OpKind = kind, op: TirOp = op, linkage: std::collections::BTreeMap<String, NativeFunctionLinkageAbi> = linkage; |b, inputs| -> (bool, Vec<String>) {
            let func = TirFunction::new(
                "callable".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(10);
            for (index, &input) in inputs.iter().enumerate() {
                values.set_value(ValueId(index as u32), input);
                values.set_repr(ValueId(index as u32), HeldCarrier::Boxed);
            }
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &every_runtime_entry(),
                linkage,
                functions: &Default::default(),
                call_sites: &Cell::new(0),
            };
            let lowered = cx.callable_preserved_op(b, op, *kind);
            let results = cx.values.value(ValueId(9)).into_iter().collect();
            ((lowered, diagnostics.borrow().clone()), results)
        });
        let inputs: Vec<Value> = inputs.iter().copied().map(Value::Word).collect();
        super::super::differential::machine(&lowering, &inputs, false)
    }

    /// One compiled function `symbol`: its parameter types (the closure
    /// first when `has_closure`), custody and result.
    #[cfg(feature = "native-backend")]
    fn linked(
        symbol: &str,
        has_closure: bool,
        params: Vec<TirType>,
        custody: Vec<ParameterCustody>,
        result: Option<TirType>,
    ) -> std::collections::BTreeMap<String, NativeFunctionLinkageAbi> {
        let abi = NativeFunctionLinkageAbi {
            source_signature: crate::ir::ExternFunctionSignature {
                arity: params.len(),
                has_closure,
                returns_value: result.is_some(),
                execution_context: crate::ir::ExecutionContextPolicy::None,
            },
            parameter_custody: custody,
            param_types: params,
            return_type: result,
            return_availability: Default::default(),
        };
        std::collections::BTreeMap::from([(symbol.to_owned(), abi)])
    }

    #[cfg(feature = "native-backend")]
    fn inline(value: u64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | value
    }

    /// The entry custody the runtime contract assigns `abi`.
    #[cfg(feature = "native-backend")]
    fn custody_word(abi: &NativeFunctionLinkageAbi, has_closure: bool) -> u64 {
        let transferred: Vec<bool> = abi
            .parameter_custody
            .iter()
            .map(|custody| *custody == ParameterCustody::Transferred)
            .collect();
        molt_codegen_abi::EntryCustodyDeclaration::declare(
            abi.source_signature.has_closure,
            abi.source_signature.arity,
            &transferred,
        )
        .encode(
            has_closure,
            abi.source_signature.arity - usize::from(has_closure),
        )
        .unwrap()
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn a_function_trampoline_decodes_raw_parameters_and_boxes_the_raw_result() {
        use ParameterCustody::{Borrowed, Transferred};
        let symbol = "walker_compiled_raw";
        let identity = super::super::scalar::tests::program_slot_word(
            &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(symbol),
        );
        // Borrowed, then an adopting entry whose raw parameters keep only the
        // extracted values: the trampoline releases both boxes after the call.
        for (custody, released) in [
            (vec![Borrowed, Borrowed], vec![]),
            (
                vec![Transferred, Transferred],
                vec![vec![inline(2), inline(3)]],
            ),
        ] {
            let linkage = linked(
                symbol,
                false,
                vec![TirType::I64, TirType::I64],
                custody,
                Some(TirType::I64),
            );
            let entry = custody_word(&linkage[symbol], false);
            let run = construct(OpKind::FuncNew, symbol, 2, None, linkage, &[]);
            assert_eq!(run.fact, (true, vec![]));
            let entered = inline(compiled_result(symbol, &[2, 3]));
            let mut key = Vec::new();
            for word in [identity, entered, 2, entry] {
                key.extend(word.to_le_bytes());
            }
            assert_eq!(
                run.results,
                [Value::Word(literal_word(RuntimeFn::FuncNew, &key))]
            );
            assert_eq!(run.released, released);
        }
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn a_closure_trampoline_passes_the_closure_first() {
        let symbol = "walker_compiled_boxed";
        let identity = super::super::scalar::tests::program_slot_word(
            &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(symbol),
        );
        let linkage = linked(
            symbol,
            true,
            vec![TirType::DynBox, TirType::DynBox],
            vec![ParameterCustody::Borrowed; 2],
            Some(TirType::DynBox),
        );
        let entry = custody_word(&linkage[symbol], true);
        let run = construct(OpKind::FuncNewClosure, symbol, 1, None, linkage, &[OBJECT]);
        assert_eq!(run.fact, (true, vec![]));
        let entered = compiled_result(symbol, &[OBJECT, inline(2)]);
        let mut key = Vec::new();
        for word in [identity, entered, 1, OBJECT, entry] {
            key.extend(word.to_le_bytes());
        }
        assert_eq!(
            run.results,
            [Value::Word(literal_word(RuntimeFn::FuncNewClosure, &key))]
        );
        assert_eq!(run.runtime_calls, [RuntimeFn::FuncNewClosure]);
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn a_builtin_trampoline_unpacks_its_arguments_into_the_runtime_entry() {
        use molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi;
        // A stubbed binary entry whose callable ABI unpacks its arguments.
        let function = RuntimeFn::ALL
            .iter()
            .copied()
            .filter(|function| {
                super::super::runtime_stubs::lookup(function.linker_symbol()).is_some()
            })
            .find(|function| {
                function.params() == 2
                    && function.callable().and_then(|callable| callable.trampoline)
                        == Some(RuntimeCallableTrampolineAbi::UnpackArgs)
            })
            .expect("a binary runtime callable that unpacks its arguments");
        let key = function.callable().unwrap().key;
        let identity = super::super::scalar::tests::program_slot_word(function.linker_symbol());
        let run = construct(
            OpKind::BuiltinFunc,
            function.symbol(),
            2,
            Some(function),
            Default::default(),
            &[],
        );
        assert_eq!(run.fact, (true, vec![]));
        let mut operands = Vec::new();
        for word in [inline(2), inline(3)] {
            operands.extend(word.to_le_bytes());
        }
        let entered = literal_word(function, &operands);
        let mut model = Vec::new();
        for word in [key, identity, entered, 2] {
            model.extend(word.to_le_bytes());
        }
        assert_eq!(
            run.results,
            [Value::Word(literal_word(RuntimeFn::FuncNewBuiltin, &model))]
        );
        assert_eq!(run.runtime_calls, [function, RuntimeFn::FuncNewBuiltin]);
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn a_builtin_of_the_wrong_arity_is_refused() {
        let run = construct(
            OpKind::BuiltinFunc,
            "add",
            1,
            Some(RuntimeFn::Add),
            Default::default(),
            &[],
        );
        assert!(run.fact.0);
        assert!(
            run.fact.1.len() == 1 && run.fact.1[0].contains("builtin_func"),
            "{:?}",
            run.fact.1
        );
        assert!(run.runtime_calls.is_empty());
    }
}
