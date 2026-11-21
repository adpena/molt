//! Cranelift's implementation of the walker's builder trait
//! (`codegen_ssa::CodegenBuilder`, design `codegen_walker.md` W3):
//! instruction selection only.
//!
//! Machine kinds map to Cranelift types: a truth bit is an `I8` holding 0 or
//! 1 (Cranelift has no `i1`), a narrow integer an `I32`, a word an `I64` and a
//! float an `F64`. An address is the target's `I64` pointer, so it reads back
//! as a word. Blocks are sealed together when the function is finalized; the
//! walker adds every edge before then.

use cranelift_codegen::ir::{StackSlot, TrapCode};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::{BranchBuilder, WordBuilder};
use target_lexicon::Architecture;

use super::*;
use crate::codegen_ssa::{
    CheckedBinary, CodegenBuilder, FloatBinary, FloatComparison, IntBinary, IntComparison,
    MachineKind, SiteBody,
};

/// The trap a path that never executes ends with.
const UNREACHABLE_TRAP: TrapCode = TrapCode::unwrap_user(3);

/// The C library's `fmod`: Cranelift has no float remainder instruction, and
/// LLVM's `frem` lowers to the same call.
const FMOD_SYMBOL: &str = "fmod";

/// The Cranelift function builder at its current position, with the module
/// that declares callees and this function's runtime references.
pub(crate) struct NativeCodegen<'a, 'b> {
    pub(crate) words: NativeWords<'a, 'b>,
    pub(crate) module: &'a mut dyn Module,
    pub(crate) import_ids: &'a mut RuntimeImportIds,
    pub(crate) imports: &'a mut NativeFunctionImports,
}

/// Logical signature of a compiled definition or import. The object and JIT
/// declaration boundary adds the attached execution context exactly once.
pub(super) fn linkage_signature(
    module: &dyn Module,
    abi: &molt_ir::linkage_abi::NativeFunctionLinkageAbi,
) -> cranelift_codegen::ir::Signature {
    let machine = |ty: &crate::tir::types::TirType| {
        NativeCodegen::machine_type(crate::codegen_ssa::control::carrier_kind(
            crate::codegen_ssa::HeldCarrier::of(ty),
        ))
    };
    let mut signature = module.make_signature();
    for ty in &abi.param_types {
        signature.params.push(AbiParam::new(machine(ty)));
    }
    if let Some(ty) = &abi.return_type {
        signature.returns.push(AbiParam::new(machine(ty)));
    }
    signature
}

impl<'b> NativeCodegen<'_, 'b> {
    fn ins(&mut self) -> cranelift_frontend::FuncInstBuilder<'_, 'b> {
        self.words.builder.ins()
    }

    fn frame_sites(&mut self) -> NativeFrameSites {
        NativeFrameSites {
            retain: self.refcount_site(RefcountOperation::Retain),
            release: self.refcount_site(RefcountOperation::Release),
            enter: self.runtime_callee(RuntimeFn::FrameRecordEnter),
            exit: self.runtime_callee(RuntimeFn::FrameRecordExit),
        }
    }

    fn value_type(&self, value: Value) -> types::Type {
        self.words.builder.func.dfg.value_type(value)
    }

    fn machine_type(kind: MachineKind) -> types::Type {
        match kind {
            MachineKind::Bit => types::I8,
            MachineKind::NarrowInt => types::I32,
            MachineKind::Word | MachineKind::Address => types::I64,
            MachineKind::Float => types::F64,
        }
    }

    fn block_args(args: &[Value]) -> Vec<BlockArg> {
        args.iter().map(|&arg| BlockArg::Value(arg)).collect()
    }

    /// The address of the writable data object `symbol` another object of
    /// the program defines.
    fn data_address(&mut self, symbol: &str) -> Value {
        let data = self
            .module
            .declare_data(symbol, Linkage::Import, true, false)
            .unwrap_or_else(|error| panic!("import {symbol}: {error}"));
        let local = self
            .module
            .declare_data_in_func(data, self.words.builder.func);
        let pointer = self.module.target_config().pointer_type();
        self.ins().symbol_value(pointer, local)
    }

    /// Whether the target lowers Cranelift's overflow-flag instructions
    /// (`sadd_overflow`, `smul_overflow`). RISC-V 64 does not in 0.137.
    fn lowers_overflow_flags(&self) -> bool {
        !matches!(
            self.module.isa().triple().architecture,
            Architecture::Riscv64(_)
        )
    }

    /// Checked arithmetic from plain instructions, for a target without the
    /// overflow-flag instructions: an add overflowed when both operands'
    /// signs differ from the sum's; a product when its high half is not the
    /// low half's sign extension.
    fn int_checked_portable(
        &mut self,
        op: CheckedBinary,
        left: Value,
        right: Value,
    ) -> (Value, Value) {
        match op {
            CheckedBinary::Add => {
                let sum = self.ins().iadd(left, right);
                let left_sign = self.ins().bxor(left, sum);
                let right_sign = self.ins().bxor(right, sum);
                let both = self.ins().band(left_sign, right_sign);
                let overflow = self.ins().icmp_imm_s(IntCC::SignedLessThan, both, 0);
                (sum, overflow)
            }
            CheckedBinary::Mul => {
                // Compare the high signed half to the sign extension of the low half.
                let low = self.ins().imul(left, right);
                let high = self.ins().smulhi(left, right);
                let shift = self.ins().iconst(types::I64, 63);
                let sign = self.ins().sshr(low, shift);
                let overflow = self.ins().icmp(IntCC::NotEqual, high, sign);
                (low, overflow)
            }
        }
    }
}

impl<'a, 'b> WordBuilder for NativeCodegen<'a, 'b> {
    molt_codegen_abi::forward_word_builder!(words: NativeWords<'a, 'b>);
}

impl<'b> CraneliftWords<'b> for NativeCodegen<'_, 'b> {
    fn cranelift(&mut self) -> &mut FunctionBuilder<'b> {
        self.words.builder
    }
}

impl BranchBuilder for NativeCodegen<'_, '_> {
    fn choose(
        &mut self,
        condition: Value,
        yes: impl FnOnce(&mut Self) -> Value,
        no: impl FnOnce(&mut Self) -> Value,
    ) -> Value {
        cranelift_choose(self, condition, yes, no)
    }
}

impl CodegenBuilder for NativeCodegen<'_, '_> {
    type Value = Value;
    type Block = Block;
    type Slot = StackSlot;
    type Callee = FuncRef;
    type RefcountSite = NativeRefcountSite;

    fn kind(&self, value: Value) -> MachineKind {
        match self.value_type(value) {
            types::I8 => MachineKind::Bit,
            types::I32 => MachineKind::NarrowInt,
            types::I64 => MachineKind::Word,
            types::F64 => MachineKind::Float,
            other => panic!("no lowered value is a Cranelift {other}"),
        }
    }
    fn word_value(&self, word: Value) -> Value {
        word
    }
    fn bit_value(&self, bit: Value) -> Value {
        bit
    }
    fn expect_word(&self, value: Value) -> Value {
        debug_assert_eq!(self.value_type(value), types::I64, "a word is an I64");
        value
    }
    fn expect_bit(&self, value: Value) -> Value {
        debug_assert_eq!(self.value_type(value), types::I8, "a truth bit is an I8");
        value
    }

    fn zero_extend(&mut self, value: Value) -> Value {
        self.ins().uextend(types::I64, value)
    }
    fn low_bit(&mut self, word: Value) -> Value {
        let byte = self.ins().ireduce(types::I8, word);
        self.ins().band_imm_s(byte, 1)
    }
    fn float_to_bits(&mut self, value: Value) -> Value {
        self.ins().bitcast(types::I64, MemFlagsData::new(), value)
    }
    fn bits_to_float(&mut self, word: Value) -> Value {
        self.ins().bitcast(types::F64, MemFlagsData::new(), word)
    }
    fn address_to_word(&mut self, value: Value) -> Value {
        value
    }
    fn word_to_address(&mut self, word: Value) -> Value {
        word
    }

    fn int_binary(&mut self, op: IntBinary, left: Value, right: Value, _exact: bool) -> Value {
        let ins = self.ins();
        match op {
            IntBinary::Add => ins.iadd(left, right),
            IntBinary::Sub => ins.isub(left, right),
            IntBinary::Mul => ins.imul(left, right),
            IntBinary::And => ins.band(left, right),
            IntBinary::Or => ins.bor(left, right),
            IntBinary::Xor => ins.bxor(left, right),
            IntBinary::ShiftLeft => ins.ishl(left, right),
            IntBinary::ShiftRightArithmetic => ins.sshr(left, right),
            IntBinary::ShiftRightLogical => ins.ushr(left, right),
            IntBinary::SignedQuotient => ins.sdiv(left, right),
            IntBinary::SignedRemainder => ins.srem(left, right),
        }
    }
    fn int_compare(&mut self, comparison: IntComparison, left: Value, right: Value) -> Value {
        let condition = match comparison {
            IntComparison::Equal => IntCC::Equal,
            IntComparison::NotEqual => IntCC::NotEqual,
            IntComparison::SignedLess => IntCC::SignedLessThan,
            IntComparison::SignedLessOrEqual => IntCC::SignedLessThanOrEqual,
            IntComparison::SignedGreater => IntCC::SignedGreaterThan,
            IntComparison::SignedGreaterOrEqual => IntCC::SignedGreaterThanOrEqual,
        };
        self.ins().icmp(condition, left, right)
    }
    fn int_checked(&mut self, op: CheckedBinary, left: Value, right: Value) -> (Value, Value) {
        if !self.lowers_overflow_flags() {
            return self.int_checked_portable(op, left, right);
        }
        match op {
            CheckedBinary::Add => self.ins().sadd_overflow(left, right),
            CheckedBinary::Mul => self.ins().smul_overflow(left, right),
        }
    }
    fn is_zero(&mut self, value: Value) -> Value {
        self.ins().icmp_imm_s(IntCC::Equal, value, 0)
    }
    fn int_to_float(&mut self, word: Value) -> Value {
        self.ins().fcvt_from_sint(types::F64, word)
    }
    fn select(&mut self, condition: Value, yes: Value, no: Value) -> Value {
        self.ins().select(condition, yes, no)
    }
    fn bit_xor(&mut self, left: Value, right: Value) -> Value {
        self.ins().bxor(left, right)
    }
    fn int_negate(&mut self, word: Value) -> Value {
        self.ins().ineg(word)
    }
    fn int_invert(&mut self, word: Value) -> Value {
        self.ins().bnot(word)
    }
    fn bit_not(&mut self, bit: Value) -> Value {
        // `bnot` would turn 1 into 0xFE, which a later `band` misreads.
        self.ins().bxor_imm_s(bit, 1)
    }

    fn float_binary(&mut self, op: FloatBinary, left: Value, right: Value, _fast: bool) -> Value {
        match op {
            FloatBinary::Add => self.ins().fadd(left, right),
            FloatBinary::Sub => self.ins().fsub(left, right),
            FloatBinary::Mul => self.ins().fmul(left, right),
            FloatBinary::Divide => self.ins().fdiv(left, right),
            FloatBinary::Remainder => {
                let mut signature = self.module.make_signature();
                signature.params.push(AbiParam::new(types::F64));
                signature.params.push(AbiParam::new(types::F64));
                signature.returns.push(AbiParam::new(types::F64));
                let id = self
                    .module
                    .declare_function(FMOD_SYMBOL, Linkage::Import, &signature)
                    .unwrap_or_else(|error| panic!("declare {FMOD_SYMBOL}: {error}"));
                let fmod = self
                    .module
                    .declare_func_in_func(id, self.words.builder.func);
                let call = self.ins().call(fmod, &[left, right]);
                self.words.builder.inst_results(call)[0]
            }
        }
    }
    fn float_negate(&mut self, value: Value) -> Value {
        self.ins().fneg(value)
    }
    fn float_constant(&mut self, value: f64) -> Value {
        self.ins().f64const(value)
    }
    fn float_compare(&mut self, comparison: FloatComparison, left: Value, right: Value) -> Value {
        // Cranelift's float conditions other than `NotEqual` are ordered;
        // `NotEqual` is unordered-or-not-equal.
        let condition = match comparison {
            FloatComparison::OrderedEqual => FloatCC::Equal,
            FloatComparison::UnorderedOrNotEqual => FloatCC::NotEqual,
            FloatComparison::OrderedLess => FloatCC::LessThan,
            FloatComparison::OrderedLessOrEqual => FloatCC::LessThanOrEqual,
            FloatComparison::OrderedGreater => FloatCC::GreaterThan,
            FloatComparison::OrderedGreaterOrEqual => FloatCC::GreaterThanOrEqual,
        };
        self.ins().fcmp(condition, left, right)
    }

    fn runtime_callee(&mut self, function: RuntimeFn) -> FuncRef {
        import_runtime_func_ref(
            self.module,
            self.import_ids,
            self.words.builder,
            self.imports,
            function,
        )
    }
    /// Imported by its attached execution symbol: the module merges the
    /// declaration with the body's definition when the body is in it.
    fn function_callee(
        &mut self,
        symbol: &str,
        abi: &molt_ir::linkage_abi::NativeFunctionLinkageAbi,
    ) -> FuncRef {
        let signature = linkage_signature(self.module, abi);
        let id =
            declare_compiled_execution_function(self.module, symbol, Linkage::Import, &signature)
                .unwrap_or_else(|error| panic!("declare compiled function `{symbol}`: {error}"));
        self.module
            .declare_func_in_func(id, self.words.builder.func)
    }
    /// A plain import, never the attached-execution symbol: finalization
    /// prepends the context only to names with the attached suffix.
    fn native_callee(
        &mut self,
        symbol: &str,
        params: &[molt_ir::native_callable_abi::NativeCallableMachineType],
        result: molt_ir::native_callable_abi::NativeCallableMachineType,
    ) -> FuncRef {
        use molt_ir::native_callable_abi::NativeCallableMachineType as Machine;
        let pointer = self.module.target_config().pointer_type();
        let machine = |ty: Machine| match ty {
            Machine::MoltValue | Machine::U64 => types::I64,
            Machine::Pointer => pointer,
            Machine::I32 => types::I32,
        };
        let mut signature = self.module.make_signature();
        signature
            .params
            .extend(params.iter().map(|&ty| AbiParam::new(machine(ty))));
        signature.returns.push(AbiParam::new(machine(result)));
        let id = self
            .module
            .declare_function(symbol, Linkage::Import, &signature)
            .unwrap_or_else(|error| panic!("declare extension symbol `{symbol}`: {error}"));
        self.module
            .declare_func_in_func(id, self.words.builder.func)
    }
    fn function_address(&mut self, callee: FuncRef) -> Value {
        let pointer = self.module.target_config().pointer_type();
        self.ins().func_addr(pointer, callee)
    }
    /// An attached callee takes the logical arguments here; the function's
    /// finalization prepends the execution context (`complete_attached_calls`).
    fn call(&mut self, callee: FuncRef, args: &[Value]) -> Option<Value> {
        let call = self.ins().call(callee, args);
        self.words.builder.inst_results(call).first().copied()
    }

    /// Cranelift's atomic_load is sequentially consistent (its only atomic
    /// load); a plain load would let GVN or LICM reuse it across a request.
    fn async_work_pending(&mut self) -> Value {
        let address = self.imports.pending_work(self.words.builder);
        let word = self
            .ins()
            .atomic_load(types::I32, MemFlagsData::trusted(), address);
        self.ins().icmp_imm_s(IntCC::NotEqual, word, 0)
    }
    fn recursion_charge(
        &mut self,
        charge: crate::codegen_ssa::RecursionCharge,
        thread: Value,
    ) -> Option<Value> {
        use crate::codegen_ssa::RecursionCharge;
        use molt_codegen_abi::thread_record::{
            generate_recursion_admits, generate_recursion_count, generate_recursion_release,
        };
        match charge {
            RecursionCharge::Admits => Some(generate_recursion_admits(&mut self.words, thread)),
            RecursionCharge::Count => {
                generate_recursion_count(&mut self.words, thread);
                None
            }
            RecursionCharge::Release => {
                generate_recursion_release(&mut self.words, thread);
                None
            }
        }
    }
    fn thread_record_word(&mut self) -> Value {
        self.imports.thread_record(self.words.builder)
    }
    fn load_byte(&mut self, address: Value, offset: i32) -> Value {
        self.ins()
            .load(types::I8, MemFlagsData::trusted(), address, offset)
    }
    fn call_cold(&mut self, callee: FuncRef, args: &[Value]) -> Option<Value> {
        let block = self
            .words
            .builder
            .current_block()
            .expect("cold call has a block");
        self.words.builder.set_cold_block(block);
        self.call(callee, args)
    }

    fn create_block(&mut self, _name: &str) -> Block {
        self.words.builder.create_block()
    }
    fn set_name(&mut self, _value: Value, _name: &str) {}
    fn switch_to_block(&mut self, block: Block) {
        self.words.builder.switch_to_block(block);
    }

    fn entry_slot(&mut self, name: &str) -> StackSlot {
        self.entry_slot_array(1, name)
    }
    fn entry_slot_array(&mut self, len: u32, _name: &str) -> StackSlot {
        self.words
            .builder
            .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8 * len, 3))
    }
    fn load_slot(&mut self, slot: StackSlot) -> Value {
        let pointer = self.module.target_config().pointer_type();
        self.ins().stack_load(pointer, types::I64, slot, 0)
    }
    fn store_slot(&mut self, slot: StackSlot, word: Value) {
        let pointer = self.module.target_config().pointer_type();
        self.ins().stack_store(pointer, word, slot, 0);
    }
    fn store_slot_at(&mut self, slot: StackSlot, index: u32, word: Value) {
        let offset = i32::try_from(8 * index).expect("an entry array fits a frame");
        let pointer = self.module.target_config().pointer_type();
        self.ins().stack_store(pointer, word, slot, offset);
    }
    fn load_slot_at(&mut self, slot: StackSlot, index: u32) -> Value {
        let offset = i32::try_from(8 * index).expect("an entry array fits a frame");
        let pointer = self.module.target_config().pointer_type();
        self.ins().stack_load(pointer, types::I64, slot, offset)
    }
    fn slot_pointer(&mut self, slot: StackSlot) -> Value {
        let pointer = self.module.target_config().pointer_type();
        self.ins().stack_addr(pointer, slot, 0)
    }

    fn bit_constant(&mut self, value: bool) -> Value {
        self.ins().iconst(types::I8, i64::from(value))
    }
    /// Anonymous read-only data; the names are LLVM's only.
    fn bytes_constant(&mut self, bytes: &[u8], _prefix: &str, _suffix: &str) -> Value {
        let id = self
            .module
            .declare_anonymous_data(false, false)
            .unwrap_or_else(|error| panic!("declare a byte constant: {error}"));
        let mut description = DataDescription::new();
        description.define(bytes.to_vec().into_boxed_slice());
        self.module
            .define_data(id, &description)
            .unwrap_or_else(|error| panic!("define a byte constant: {error}"));
        let local = self
            .module
            .declare_data_in_func(id, self.words.builder.func);
        let pointer = self.module.target_config().pointer_type();
        self.ins().symbol_value(pointer, local)
    }
    fn site_record(&mut self, symbol: &str, image: &[u8]) -> Value {
        let data = self
            .module
            .declare_data(symbol, Linkage::Local, true, false)
            .unwrap_or_else(|error| panic!("site record `{symbol}`: {error}"));
        let mut description = DataDescription::new();
        description.set_align(8);
        description.define(image.to_vec().into_boxed_slice());
        match self.module.define_data(data, &description) {
            Ok(()) | Err(cranelift_module::ModuleError::DuplicateDefinition(_)) => {}
            Err(error) => panic!("site record `{symbol}`: {error}"),
        }
        let local = self
            .module
            .declare_data_in_func(data, self.words.builder.func);
        let pointer = self.module.target_config().pointer_type();
        self.ins().symbol_value(pointer, local)
    }
    fn static_cell_block(&mut self, module: &str) -> Value {
        self.data_address(&molt_ir::code_slots::static_cell_block_symbol(module))
    }
    fn load_word(&mut self, address: Value, offset: i32) -> Value {
        self.ins()
            .load(types::I64, MemFlagsData::trusted(), address, offset)
    }
    fn store_word(&mut self, address: Value, offset: i32, word: Value) {
        self.ins()
            .store(MemFlagsData::trusted(), word, address, offset);
    }
    fn program_slot(&mut self, kind: molt_ir::code_slots::ProgramSlotKind, owner: &str) -> Value {
        emit_program_slot_id(kind, owner, self.module, self.words.builder)
    }

    /// A second codegen context and function builder, defined into the
    /// module as a local attached function, then referenced from the caller.
    fn define_helper(
        &mut self,
        name: &str,
        params: usize,
        body: &impl crate::codegen_ssa::HelperBody,
    ) -> FuncRef {
        let symbol = compiled_execution_symbol(name);
        let id = match self.module.get_name(&symbol) {
            Some(cranelift_module::FuncOrDataId::Func(id)) => id,
            Some(cranelift_module::FuncOrDataId::Data(_)) => {
                panic!("helper {name} names program data")
            }
            None => {
                let mut logical = self.module.make_signature();
                for _ in 0..params {
                    logical.params.push(AbiParam::new(types::I64));
                }
                logical.returns.push(AbiParam::new(types::I64));
                let id = declare_compiled_execution_function(
                    self.module,
                    name,
                    Linkage::Local,
                    &logical,
                )
                .unwrap_or_else(|error| panic!("declare helper {name}: {error}"));
                let mut context = self.module.make_context();
                context.func.signature = self
                    .module
                    .declarations()
                    .get_function_decl(id)
                    .signature
                    .clone();
                let mut builder_context = FunctionBuilderContext::new();
                let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
                let entry = builder.create_block();
                builder.append_block_params_for_function_params(entry);
                builder.switch_to_block(entry);
                let execution = builder.block_params(entry)[0];
                let words = builder.block_params(entry)[1..].to_vec();
                let mut imports = NativeFunctionImports::default();
                {
                    let mut helper = NativeCodegen {
                        words: NativeWords {
                            builder: &mut builder,
                        },
                        module: &mut *self.module,
                        import_ids: &mut *self.import_ids,
                        imports: &mut imports,
                    };
                    let result = body.build(&mut helper, &words);
                    helper.words.builder.ins().return_(&[result]);
                }
                builder.seal_all_blocks();
                builder.finalize(self.module.target_config());
                complete_attached_calls(&*self.module, &mut context.func, execution);
                self.module
                    .define_function(id, &mut context)
                    .unwrap_or_else(|error| panic!("define helper {name}: {error:?}"));
                id
            }
        };
        self.module
            .declare_func_in_func(id, self.words.builder.func)
    }

    fn refcount_site(&mut self, operation: RefcountOperation) -> NativeRefcountSite {
        // A release whose count read 1 reaches the program's terminal-release
        // helper, which frees an unobserved plain instance inline.
        let slow = match operation {
            RefcountOperation::Retain => self.runtime_callee(RuntimeFn::IncRefObj),
            RefcountOperation::Release => self.define_helper(
                crate::codegen_ssa::RELEASE_TERMINAL_HELPER,
                1,
                &crate::codegen_ssa::ReleaseTerminalBody,
            ),
        };
        NativeRefcountSite {
            slow,
            operation,
            admission: self.imports.refcount_admission(self.words.builder),
        }
    }
    fn refcount(&mut self, site: NativeRefcountSite, word: Value) {
        emit_refcount(self.words.builder, word, site);
    }
    fn refcount_admission(&mut self) -> Value {
        self.imports.refcount_admission(self.words.builder)
    }
    fn profile_enabled(&mut self) -> Value {
        let record = self.thread_record_word();
        let offset = molt_codegen_abi::thread_record::ThreadRecordWord::ProfileEnabled.offset();
        self.ins()
            .load(types::I64, MemFlagsData::trusted(), record, offset)
    }

    /// Inline in the caller: Cranelift blocks may split anywhere, so a site
    /// body needs no out-of-line copy. Its sites carry the admission, which
    /// is the trailing word.
    fn site(&mut self, body: SiteBody, args: &[Value]) -> Value {
        let sites = self.frame_sites();
        match (body, args) {
            (SiteBody::FieldGet { offset }, &[object, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ObjectFieldGetPtr);
                emit_field_slot_get(
                    self.words.builder,
                    sites,
                    object,
                    offset,
                    |builder, pointer| {
                        let offset = builder.ins().iconst(types::I64, i64::from(offset));
                        let call = builder.ins().call(cold, &[pointer, offset]);
                        builder.inst_results(call)[0]
                    },
                )
            }
            (SiteBody::BuiltinFieldGet { field }, &[object, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ExceptionFieldGet);
                emit_builtin_field_get(
                    self.words.builder,
                    sites,
                    object,
                    field,
                    |builder, object| {
                        let tag = builder.ins().iconst(types::I64, i64::from(field as u8));
                        let call = builder.ins().call(cold, &[object, tag]);
                        builder.inst_results(call)[0]
                    },
                )
            }
            (SiteBody::FieldSet { offset }, &[object, value, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ObjectFieldSetPtr);
                emit_field_slot_set(
                    self.words.builder,
                    sites,
                    object,
                    offset,
                    value,
                    |builder, pointer| {
                        let offset = builder.ins().iconst(types::I64, i64::from(offset));
                        builder.ins().call(cold, &[pointer, offset, value]);
                    },
                );
                self.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
            }
            (SiteBody::ItemGet { containers, cold }, &[sequence, index, _]) => {
                let cold = self.runtime_callee(cold);
                emit_list_item_get(
                    self.words.builder,
                    sites,
                    sequence,
                    index,
                    containers,
                    |builder| {
                        let call = builder.ins().call(cold, &[sequence, index]);
                        builder.inst_results(call)[0]
                    },
                )
            }
            (SiteBody::ObjectAlloc { root, plan }, &[class, profile, _]) => {
                let allocate = self.runtime_callee(RuntimeFn::HeapAlloc);
                let cold = self.runtime_callee(if root {
                    RuntimeFn::ObjectNew
                } else {
                    RuntimeFn::ObjectNewBound
                });
                emit_object_alloc(
                    self.words.builder,
                    sites,
                    class,
                    plan,
                    profile,
                    allocate,
                    |builder| {
                        let call = if root {
                            builder.ins().call(cold, &[])
                        } else {
                            builder.ins().call(cold, &[class])
                        };
                        builder.inst_results(call)[0]
                    },
                )
            }
            (SiteBody::ReleaseTerminal, &[bits, _]) => {
                let free = self.runtime_callee(RuntimeFn::HeapFree);
                let cold = self.runtime_callee(RuntimeFn::DecRefObj);
                let profile = self.profile_enabled();
                let record = self.thread_record_word();
                let terminal_release = self.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    record,
                    molt_codegen_abi::thread_record::ThreadRecordWord::TerminalRelease.offset(),
                );
                emit_release_terminal(
                    self.words.builder,
                    sites,
                    bits,
                    profile,
                    terminal_release,
                    free,
                    |builder| {
                        builder.ins().call(cold, &[bits]);
                    },
                );
                self.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
            }
            (SiteBody::ClosureLoad, &[owner, offset, _]) => {
                emit_closure_load(self.words.builder, sites, owner, offset)
            }
            (SiteBody::ClosureStore, &[owner, offset, value, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ClosureStore);
                emit_closure_store(self.words.builder, sites, owner, offset, value, |builder| {
                    builder.ins().call(cold, &[owner, offset, value]);
                });
                self.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
            }
            (SiteBody::ListItemSet, &[list, index, value, _]) => {
                let cold = self.runtime_callee(RuntimeFn::SetitemMethod);
                let none = self.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE);
                emit_list_item_set(
                    self.words.builder,
                    sites,
                    list,
                    index,
                    value,
                    none,
                    |builder| {
                        let call = builder.ins().call(cold, &[list, index, value]);
                        builder.inst_results(call)[0]
                    },
                )
            }
            (SiteBody::GlobalSlotRead { index }, &[base, module, name, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ModuleGetGlobal);
                emit_global_slot_read(
                    self.words.builder,
                    base,
                    index,
                    sites.retain,
                    cold,
                    &[module, name],
                )
            }
            (SiteBody::StaticCellStore { index }, &[base, value, _]) => {
                emit_static_cell_store(
                    self.words.builder,
                    base,
                    index,
                    value,
                    sites.retain,
                    sites.release,
                );
                self.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
            }
            (SiteBody::ModuleImportClosed, &[id, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ModuleEnsure);
                let block = self.data_address(molt_ir::MODULE_READY_SYMBOL);
                emit_module_import_closed(self.words.builder, block, id, sites.retain, cold)
            }
            (SiteBody::MethodSiteCall { cold, .. }, &[thread, record, ref words @ .., _]) => {
                let cold = self.runtime_callee(cold);
                emit_method_site_call(self.words.builder, sites, thread, record, words, cold)
            }
            (SiteBody::MethodSiteLoad, &[record, receiver, self_out, _]) => {
                let cold = self.runtime_callee(RuntimeFn::MethodSiteLoad);
                emit_method_site_load(self.words.builder, sites, record, receiver, self_out, cold)
            }
            (body, args) => panic!("{body:?} takes other words than {}", args.len()),
        }
    }

    fn placeholder(&mut self, kind: MachineKind) -> Value {
        match kind {
            MachineKind::Float => self.ins().f64const(0.0),
            kind => self.ins().iconst(Self::machine_type(kind), 0),
        }
    }

    fn append_block_param(&mut self, block: Block, kind: MachineKind) -> Value {
        self.words
            .builder
            .append_block_param(block, Self::machine_type(kind))
    }
    fn jump(&mut self, target: Block, args: &[Value]) {
        let args = Self::block_args(args);
        self.ins().jump(target, &args);
    }
    fn branch_if(&mut self, condition: Value, yes: (Block, &[Value]), no: (Block, &[Value])) {
        let (yes_args, no_args) = (Self::block_args(yes.1), Self::block_args(no.1));
        self.ins().brif(condition, yes.0, &yes_args, no.0, &no_args);
    }
    /// `cranelift_frontend::Switch` picks a jump table or a compare tree; its
    /// targets take no arguments, so an edge that passes some goes through a
    /// block of its own that jumps with them.
    fn switch(
        &mut self,
        selector: Value,
        cases: &[(i64, Block, Vec<Value>)],
        default: (Block, &[Value]),
    ) {
        let edges = cases
            .iter()
            .map(|(value, target, args)| (Some(*value), *target, args.as_slice()))
            .chain(std::iter::once((None, default.0, default.1)));
        let mut switch = Switch::new();
        let mut otherwise = default.0;
        let mut forwarded: Vec<(Block, Block, &[Value])> = Vec::new();
        for (value, target, args) in edges {
            let entry = if args.is_empty() {
                target
            } else {
                let block = self.words.builder.create_block();
                forwarded.push((block, target, args));
                block
            };
            match value {
                Some(value) => switch.set_entry(u128::from(value as u64), entry),
                None => otherwise = entry,
            }
        }
        switch.emit(self.words.builder, selector, otherwise);
        for (block, target, args) in forwarded {
            self.words.builder.switch_to_block(block);
            self.jump(target, args);
        }
    }
    fn frame_homes(&self) -> Option<Self::Slot> {
        self.imports.frame_homes
    }
    fn set_frame_homes(&mut self, slot: Option<Self::Slot>) {
        self.imports.frame_homes = slot;
    }

    fn pop_frame(
        &mut self,
        base: Value,
        kind: Option<molt_codegen_abi::frame_record::FrameEntryKind>,
    ) {
        let sites = self.frame_sites();
        let record = kind.map(|_| self.thread_record_word());
        emit_frame_record_pop(self.words.builder, sites, record, base, kind);
    }
    fn return_value(&mut self, value: Option<Value>) {
        self.ins().return_(value.as_slice());
    }
    fn unreachable(&mut self) {
        self.ins().trap(UNREACHABLE_TRAP);
    }
}

impl crate::codegen_ssa::frame::RecordPlacement for NativeCodegen<'_, '_> {
    fn push_frame(
        &mut self,
        code: Value,
        kind: Option<molt_codegen_abi::frame_record::FrameEntryKind>,
        payload: Option<Value>,
    ) -> Value {
        let sites = self.frame_sites();
        let record = kind.map(|_| self.thread_record_word());
        emit_frame_record_push(self.words.builder, sites, record, code, kind, payload)
    }
}

impl crate::codegen_ssa::task::TaskPlacement for NativeCodegen<'_, '_> {
    fn poll_parameter(&self) -> Option<Value> {
        let function = &self.words.builder.func;
        let params = function.dfg.block_params(function.layout.entry_block()?);
        match params {
            [_, frame] => Some(*frame),
            _ => None,
        }
    }
    fn poll_frame_bytes(&mut self, symbol: &str) -> Self::Word {
        load_stateful_frame_size(self.module, self.words.builder, symbol)
    }
    fn place_task(
        &mut self,
        layout: molt_codegen_abi::task_site::TaskConstructorLayout,
        poll: Value,
        frame_bytes: Value,
        context: molt_codegen_abi::task_site::TaskFrameContext<Value>,
        payload: molt_codegen_abi::task_site::TaskPayload<'_, Value>,
        _name: &str,
    ) -> Value {
        let frames = self.frame_sites();
        let tasks = NativeTaskSites::with_imports(|entry| self.runtime_callee(entry));
        emit_task_construction(
            self.words.builder,
            frames,
            tasks,
            layout,
            poll,
            frame_bytes,
            context,
            payload,
        )
    }
}

/// Walker kernels as Cranelift machine code (`codegen_ssa::differential`):
/// each kernel is built through [`NativeCodegen`] into one function, which
/// the JIT compiles and the test calls. The runtime entries the walker calls
/// are stubs over the model tests' runtime model.
#[cfg(test)]
pub(crate) mod machine {
    use cranelift_jit::{JITBuilder, JITModule};
    use molt_codegen_abi::inline_refcount::CompiledRefcountAdmission;

    use super::*;
    use crate::codegen_ssa::differential::{Kernel, Run};
    use crate::codegen_ssa::runtime_stubs;
    use crate::codegen_ssa::scalar::tests::Value;

    // Inputs and results travel as 64-bit words; a narrower value is read
    // from and written to the word's low bytes.
    const _: () = assert!(cfg!(target_endian = "little"));

    extern "C" fn fmod(left: f64, right: f64) -> f64 {
        // Rust's `%` on floats is C's `fmod`.
        left % right
    }

    fn lookup(name: &str) -> Option<*const u8> {
        if name == FMOD_SYMBOL {
            return Some(fmod as *const u8);
        }
        runtime_stubs::lookup(name)
    }

    /// The host ISA at the optimization level release code uses.
    fn host_isa() -> isa::OwnedTargetIsa {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").unwrap();
        flags.set("is_pic", "false").unwrap();
        cranelift_native::builder()
            .expect("the host is a Cranelift target")
            .finish(settings::Flags::new(flags))
            .expect("the host ISA builds")
    }

    fn input_type(value: Value) -> types::Type {
        match value {
            Value::Bit(_) => types::I8,
            Value::Narrow(_) => types::I32,
            Value::Word(_) | Value::Address(_) => types::I64,
            Value::Float(_) => types::F64,
        }
    }

    fn input_word(value: Value) -> u64 {
        match value {
            Value::Bit(bit) => u64::from(bit),
            Value::Narrow(narrow) => u64::from(narrow),
            Value::Word(word) | Value::Address(word) => word,
            Value::Float(float) => float.to_bits(),
        }
    }

    fn result_value(kind: MachineKind, word: u64) -> Value {
        match kind {
            MachineKind::Bit => {
                assert!(word <= 1, "a truth bit is 0 or 1, not {word:#x}");
                Value::Bit(word == 1)
            }
            MachineKind::NarrowInt => Value::Narrow(word as u32),
            MachineKind::Word | MachineKind::Address => Value::Word(word),
            MachineKind::Float => Value::Float(f64::from_bits(word)),
        }
    }

    /// Compile `kernel` as `fn(context, inputs, results)` and run it once on
    /// `inputs`.
    pub(crate) fn run<K: Kernel>(
        kernel: &K,
        inputs: &[Value],
        fail_allocation: bool,
    ) -> Run<K::Fact> {
        run_with(inputs, fail_allocation, 0, false, |b, args| {
            kernel.build(b, args)
        })
    }

    pub(crate) fn run_with_pending_byte<K: Kernel>(
        kernel: &K,
        inputs: &[Value],
        fail_allocation: bool,
        pending: u8,
    ) -> Run<K::Fact> {
        run_with(inputs, fail_allocation, pending, false, |b, args| {
            kernel.build(b, args)
        })
    }

    /// [`run`] while the process collects runtime profile counters: the
    /// thread record's profile-enabled word is 1.
    pub(crate) fn run_with_profile<K: Kernel>(kernel: &K, inputs: &[Value]) -> Run<K::Fact> {
        run_with(inputs, false, 0, true, |b, args| kernel.build(b, args))
    }

    /// [`run`] over a body that builds with `NativeCodegen` itself.
    fn run_with<F>(
        inputs: &[Value],
        fail_allocation: bool,
        pending: u8,
        profile_enabled: bool,
        body: impl FnOnce(&mut NativeCodegen<'_, '_>, &[Value_]) -> (F, Vec<Value_>),
    ) -> Run<F> {
        let mut jit = JITBuilder::with_isa(host_isa(), cranelift_module::default_libcall_names());
        jit.symbol_lookup_fn(Box::new(lookup));
        let mut module = JITModule::new(jit);
        let pointer = module.target_config().pointer_type();
        let mut signature = module.make_signature();
        for _ in 0..3 {
            signature.params.push(AbiParam::new(pointer));
        }
        let id = module
            .declare_function("walker_kernel", Linkage::Local, &signature)
            .unwrap();
        let mut context = module.make_context();
        context.func.signature = signature;
        let mut builder_context = FunctionBuilderContext::new();
        let mut import_ids = RuntimeImportIds::default();
        let mut imports = NativeFunctionImports::default();
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let &[execution, input_words, result_words] = builder.block_params(entry) else {
            unreachable!("the kernel takes three parameters");
        };
        let admission = builder
            .ins()
            .iconst(types::I64, CompiledRefcountAdmission::Runtime as i64);
        imports.set_refcount_admission(admission);
        let args: Vec<Value_> = inputs
            .iter()
            .enumerate()
            .map(|(index, &input)| {
                builder.ins().load(
                    input_type(input),
                    MemFlagsData::trusted(),
                    input_words,
                    8 * index as i32,
                )
            })
            .collect();
        let (fact, kinds) = {
            let mut b = NativeCodegen {
                words: NativeWords {
                    builder: &mut builder,
                },
                module: &mut module,
                import_ids: &mut import_ids,
                imports: &mut imports,
            };
            let (fact, results) = body(&mut b, &args);
            let kinds: Vec<MachineKind> = results.iter().map(|&value| b.kind(value)).collect();
            for (index, (&value, &kind)) in results.iter().zip(&kinds).enumerate() {
                let word = match kind {
                    MachineKind::Bit | MachineKind::NarrowInt => b.zero_extend(value),
                    _ => value,
                };
                b.words.builder.ins().store(
                    MemFlagsData::trusted(),
                    word,
                    result_words,
                    8 * index as i32,
                );
            }
            b.words.builder.ins().return_(&[]);
            (fact, kinds)
        };
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
        complete_attached_calls(&module, &mut context.func, execution);
        module
            .define_function(id, &mut context)
            .unwrap_or_else(|error| panic!("the kernel compiles: {error:?}"));
        module.finalize_definitions().unwrap();
        let code = module.get_finalized_function(id);
        // SAFETY: `code` is the function defined above, whose signature is
        // `extern "C" fn(context, inputs, results)` in the host convention.
        let kernel_code: extern "C" fn(u64, *const u64, *mut u64) =
            unsafe { std::mem::transmute(code) };
        let input_words: Vec<u64> = inputs.iter().copied().map(input_word).collect();
        let mut result_words = vec![0u64; kinds.len()];
        let mut record =
            [0u64; (molt_codegen_abi::THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET as usize / 8) + 1];
        runtime_stubs::begin(fail_allocation, record.as_mut_ptr() as u64);
        // The exception hint is one byte at its ABI offset, on either byte order.
        let first = &mut record[0];
        let mut bytes = first.to_ne_bytes();
        bytes[molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET as usize] = pending;
        *first = u64::from_ne_bytes(bytes);
        let profile =
            molt_codegen_abi::thread_record::ThreadRecordWord::ProfileEnabled.offset() as usize / 8;
        record[profile] = u64::from(profile_enabled);
        let attached = molt_codegen_abi::AttachedExecutionContextStorage::new();
        attached.bind_thread_record(record.as_mut_ptr() as u64);
        kernel_code(
            std::ptr::from_ref(&attached) as u64,
            input_words.as_ptr(),
            result_words.as_mut_ptr(),
        );
        let stubs = runtime_stubs::finish();
        // SAFETY: the kernel has returned; nothing refers to its code.
        unsafe { module.free_memory() };
        Run {
            fact,
            results: kinds
                .iter()
                .zip(result_words)
                .map(|(&kind, word)| result_value(kind, word))
                .collect(),
            runtime_calls: stubs.calls,
            refcounts: stubs.refcounts,
            released: stubs.model.released,
            inserted: stubs.model.inserted,
            origins: stubs.model.origins,
        }
    }

    /// A Cranelift SSA value (the walker tests name their model value `Value`).
    type Value_ = cranelift_codegen::ir::Value;

    /// Compile a whole function, `(context, words...) -> word` with one word
    /// parameter per `args`, whose body `build` emits from the entry block
    /// through its own returns, and call it once on `args`. The result is
    /// the function's answer.
    pub(crate) fn run_function<F>(
        args: &[u64],
        build: impl FnOnce(&mut NativeCodegen<'_, '_>, &[Value_]) -> F,
    ) -> Run<F> {
        run_function_in(args, ThreadState::default(), build)
    }

    /// The thread state a run lends its code: the record's exception hint,
    /// whether the runtime holds a current exception (a set hint without
    /// one is stale), and the pending-work word.
    #[derive(Clone, Copy, Default)]
    pub(crate) struct ThreadState {
        pub(crate) hint: u8,
        pub(crate) raised: bool,
        pub(crate) work: u32,
        pub(crate) frame_top: u64,
        /// The thread's recursion depth and the runtime's recursion limit.
        pub(crate) depth: u64,
        pub(crate) limit: u64,
    }

    /// [`run_function`] in the thread state `state`.
    pub(crate) fn run_function_in<F>(
        args: &[u64],
        state: ThreadState,
        build: impl FnOnce(&mut NativeCodegen<'_, '_>, &[Value_]) -> F,
    ) -> Run<F> {
        let mut jit = JITBuilder::with_isa(host_isa(), cranelift_module::default_libcall_names());
        jit.symbol_lookup_fn(Box::new(lookup));
        let mut module = JITModule::new(jit);
        let mut signature = module.make_signature();
        for _ in 0..=args.len() {
            signature.params.push(AbiParam::new(types::I64));
        }
        signature.returns.push(AbiParam::new(types::I64));
        let id = module
            .declare_function("walker_function", Linkage::Local, &signature)
            .unwrap();
        let mut context = module.make_context();
        context.func.signature = signature;
        let mut builder_context = FunctionBuilderContext::new();
        let mut import_ids = RuntimeImportIds::default();
        let mut imports = NativeFunctionImports::default();
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let execution = builder.block_params(entry)[0];
        let params = builder.block_params(entry)[1..].to_vec();
        let admission = builder
            .ins()
            .iconst(types::I64, CompiledRefcountAdmission::Runtime as i64);
        imports.set_refcount_admission(admission);
        let fact = build(
            &mut NativeCodegen {
                words: NativeWords {
                    builder: &mut builder,
                },
                module: &mut module,
                import_ids: &mut import_ids,
                imports: &mut imports,
            },
            &params,
        );
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
        complete_attached_calls(&module, &mut context.func, execution);
        module
            .define_function(id, &mut context)
            .unwrap_or_else(|error| panic!("the function compiles: {error:?}"));
        module.finalize_definitions().unwrap();
        let code = module.get_finalized_function(id);
        let mut record =
            [0u64; (molt_codegen_abi::THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET as usize / 8) + 1];
        record[molt_codegen_abi::THREAD_RECORD_FRAME_TOP_OFFSET as usize / 8] = state.frame_top;
        let work = Box::new(state.work);
        let limit = Box::new(state.limit);
        record[molt_codegen_abi::THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET as usize / 8] =
            std::ptr::from_ref(&*limit) as u64;
        record[molt_codegen_abi::THREAD_RECORD_RECURSION_DEPTH_OFFSET as usize / 8] = state.depth;
        record[molt_codegen_abi::THREAD_RECORD_PENDING_WORK_PTR_OFFSET as usize / 8] =
            std::ptr::from_ref(&*work) as u64;
        let mut bytes = record[0].to_ne_bytes();
        bytes[molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET as usize] = state.hint;
        record[0] = u64::from_ne_bytes(bytes);
        runtime_stubs::begin(false, record.as_mut_ptr() as u64);
        runtime_stubs::raise_when(state.raised);
        let attached = molt_codegen_abi::AttachedExecutionContextStorage::new();
        attached.bind_thread_record(record.as_mut_ptr() as u64);
        let context = std::ptr::from_ref(&attached) as u64;
        // SAFETY: `code` is the function defined above, in the host
        // convention, with one word parameter per argument after the context.
        let answer = unsafe {
            match *args {
                [] => std::mem::transmute::<*const u8, extern "C" fn(u64) -> u64>(code)(context),
                [a] => std::mem::transmute::<*const u8, extern "C" fn(u64, u64) -> u64>(code)(
                    context, a,
                ),
                [a, b] => std::mem::transmute::<*const u8, extern "C" fn(u64, u64, u64) -> u64>(
                    code,
                )(context, a, b),
                _ => panic!("run_function takes at most two arguments"),
            }
        };
        let stubs = runtime_stubs::finish();
        // SAFETY: the function has returned; nothing refers to its code.
        unsafe { module.free_memory() };
        Run {
            fact,
            results: vec![Value::Word(answer)],
            runtime_calls: stubs.calls,
            refcounts: stubs.refcounts,
            released: stubs.model.released,
            inserted: stubs.model.inserted,
            origins: stubs.model.origins,
        }
    }

    #[test]
    fn checked_arithmetic_without_overflow_flags_matches_rust() {
        // RISC-V 64 selects this sequence; run it on the host too.
        let cases = [
            (0, 0),
            (40, 2),
            (i64::MAX, 1),
            (i64::MIN, -1),
            (-5, i64::MIN + 5),
            (1 << 32, 1 << 31),
            (-(1 << 32), 1 << 31),
            (i64::MIN, 1),
            (-1, -1),
        ];
        for (op, oracle) in [
            (
                CheckedBinary::Add,
                i64::overflowing_add as fn(i64, i64) -> (i64, bool),
            ),
            (CheckedBinary::Mul, i64::overflowing_mul),
        ] {
            for (left, right) in cases {
                let inputs = [Value::Word(left as u64), Value::Word(right as u64)];
                let run = run_with(&inputs, false, 0, false, |b, args| {
                    let (value, overflow) = b.int_checked_portable(op, args[0], args[1]);
                    ((), vec![value, overflow])
                });
                let (value, overflow) = oracle(left, right);
                assert_eq!(
                    run.results,
                    [Value::Word(value as u64), Value::Bit(overflow)],
                    "{op:?} {left} {right}"
                );
            }
        }
    }
}
