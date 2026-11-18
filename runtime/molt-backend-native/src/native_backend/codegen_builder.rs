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

impl<'b> NativeCodegen<'_, 'b> {
    fn ins(&mut self) -> cranelift_frontend::FuncInstBuilder<'_, 'b> {
        self.words.builder.ins()
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
            CheckedBinary::Mul => imul_overflow64(self.words.builder, left, right),
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
    /// An attached callee takes the logical arguments here; the function's
    /// finalization prepends the execution context (`complete_attached_calls`).
    fn call(&mut self, callee: FuncRef, args: &[Value]) -> Option<Value> {
        let call = self.ins().call(callee, args);
        self.words.builder.inst_results(call).first().copied()
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
    fn program_slot(&mut self, kind: molt_ir::code_slots::ProgramSlotKind, owner: &str) -> Value {
        emit_program_slot_id(kind, owner, self.module, self.words.builder)
    }

    fn refcount_site(&mut self, operation: RefcountOperation) -> NativeRefcountSite {
        let slow = self.runtime_callee(match operation {
            RefcountOperation::Retain => RuntimeFn::IncRefObj,
            RefcountOperation::Release => RuntimeFn::DecRefObj,
        });
        self.imports.refcount_site(slow, operation)
    }
    fn refcount(&mut self, site: NativeRefcountSite, word: Value) {
        emit_refcount(self.words.builder, word, site);
    }
    fn refcount_admission(&mut self) -> Value {
        self.refcount_site(RefcountOperation::Retain).admission
    }

    /// Inline in the caller: Cranelift blocks may split anywhere, so a site
    /// body needs no out-of-line copy. Its sites carry the admission, which
    /// is the trailing word.
    fn site(&mut self, body: SiteBody, args: &[Value]) -> Value {
        let sites = NativeFrameSites {
            retain: self.refcount_site(RefcountOperation::Retain),
            release: self.refcount_site(RefcountOperation::Release),
            enter: self.runtime_callee(RuntimeFn::FrameRecordEnter),
            exit: self.runtime_callee(RuntimeFn::FrameRecordExit),
        };
        let mut sealed = BTreeSet::new();
        match (body, args) {
            (SiteBody::FieldGet { offset }, &[object, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ObjectFieldGetPtr);
                emit_field_slot_get(
                    self.words.builder,
                    &mut sealed,
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
            (SiteBody::FieldSet { offset }, &[object, value, _]) => {
                let cold = self.runtime_callee(RuntimeFn::ObjectFieldSetPtr);
                emit_field_slot_set(
                    self.words.builder,
                    &mut sealed,
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
                    &mut sealed,
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
            (SiteBody::ListItemSet, &[list, index, value, _]) => {
                let cold = self.runtime_callee(RuntimeFn::SetitemMethod);
                let none = self.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE);
                emit_list_item_set(
                    self.words.builder,
                    &mut sealed,
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
    fn unreachable(&mut self) {
        self.ins().trap(UNREACHABLE_TRAP);
    }
}

/// Walker kernels as Cranelift machine code (`codegen_ssa::differential`):
/// each kernel is built through [`NativeCodegen`] into one function, which
/// the JIT compiles and the test calls. The runtime entries the walker calls
/// are stubs over the model tests' runtime model.
#[cfg(test)]
pub(crate) mod machine {
    use std::cell::RefCell;

    use cranelift_jit::{JITBuilder, JITModule};
    use molt_codegen_abi::inline_refcount::CompiledRefcountAdmission;

    use super::*;
    use crate::codegen_ssa::differential::{Kernel, Run};
    use crate::codegen_ssa::scalar::tests::{Memory, RuntimeModel, Value, program_slot_word};

    // Inputs and results travel as 64-bit words; a narrower value is read
    // from and written to the word's low bytes.
    const _: () = assert!(cfg!(target_endian = "little"));

    /// What the stubs saw while the kernel ran on this thread.
    #[derive(Default)]
    struct Stubs {
        model: RuntimeModel,
        calls: Vec<RuntimeFn>,
        refcounts: Vec<(RefcountOperation, u64)>,
    }

    thread_local! {
        static STUBS: RefCell<Stubs> = RefCell::default();
    }

    /// The process memory the kernel's pointer arguments name.
    struct ProcessMemory;

    impl Memory for ProcessMemory {
        fn bytes(&self, address: u64, len: usize) -> Vec<u8> {
            // SAFETY: the kernel passed `len` readable bytes at `address`.
            unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec()
        }
        fn word(&self, address: u64) -> u64 {
            // SAFETY: the kernel passed an aligned word at `address`.
            unsafe { *(address as *const u64) }
        }
        fn store(&mut self, address: u64, word: u64) {
            // SAFETY: the kernel passed a writable aligned word at `address`.
            unsafe { *(address as *mut u64) = word }
        }
    }

    fn runtime(function: RuntimeFn, args: &[u64]) -> u64 {
        STUBS.with_borrow_mut(|stubs| {
            stubs.calls.push(function);
            stubs
                .model
                .run(function, args, &mut ProcessMemory)
                .unwrap_or(0)
        })
    }

    fn refcount(operation: RefcountOperation, word: u64) {
        STUBS.with_borrow_mut(|stubs| stubs.refcounts.push((operation, word)));
    }

    // Each runtime entry is attached: its first parameter is the execution
    // context, which the stubs ignore.
    extern "C" fn int_from_i64(_: u64, raw: u64) -> u64 {
        runtime(RuntimeFn::IntFromI64, &[raw])
    }
    extern "C" fn int_as_i64(_: u64, word: u64) -> u64 {
        runtime(RuntimeFn::IntAsI64, &[word])
    }
    extern "C" fn is_truthy(_: u64, word: u64) -> u64 {
        runtime(RuntimeFn::IsTruthy, &[word])
    }
    extern "C" fn div(_: u64, left: u64, right: u64) -> u64 {
        runtime(RuntimeFn::Div, &[left, right])
    }
    extern "C" fn floordiv(_: u64, left: u64, right: u64) -> u64 {
        runtime(RuntimeFn::Floordiv, &[left, right])
    }
    extern "C" fn modulo(_: u64, left: u64, right: u64) -> u64 {
        runtime(RuntimeFn::Mod, &[left, right])
    }
    extern "C" fn call_inputs_release(_: u64, callable: u64, args: u64, count: u64) {
        runtime(RuntimeFn::CallInputsRelease, &[callable, args, count]);
    }
    extern "C" fn string_from_bytes(_: u64, data: u64, len: u64, out: u64) -> u32 {
        runtime(RuntimeFn::StringFromBytes, &[data, len, out]) as u32
    }
    extern "C" fn bytes_from_bytes(_: u64, data: u64, len: u64, out: u64) -> u32 {
        runtime(RuntimeFn::BytesFromBytes, &[data, len, out]) as u32
    }
    extern "C" fn bigint_from_bytes(_: u64, data: u64, len: u64) -> u64 {
        runtime(RuntimeFn::BigintFromBytes, &[data, len])
    }
    extern "C" fn two(function: RuntimeFn, a: u64, b: u64) -> u64 {
        runtime(function, &[a, b])
    }
    extern "C" fn get_attr_name(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::GetAttrName, a, b)
    }
    extern "C" fn del_attr_name(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::DelAttrName, a, b)
    }
    extern "C" fn callargs_new(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::CallargsNew, a, b)
    }
    extern "C" fn callargs_push_pos(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::CallargsPushPos, a, b)
    }
    extern "C" fn call_bind(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::CallBind, a, b)
    }
    extern "C" fn call_func_fast0(_: u64, a: u64) -> u64 {
        runtime(RuntimeFn::CallFuncFast0, &[a])
    }
    extern "C" fn call_func_fast1(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::CallFuncFast1, a, b)
    }
    extern "C" fn call_func_fast2(_: u64, a: u64, b: u64, c: u64) -> u64 {
        runtime(RuntimeFn::CallFuncFast2, &[a, b, c])
    }
    extern "C" fn call_func_fast3(_: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
        runtime(RuntimeFn::CallFuncFast3, &[a, b, c, d])
    }
    extern "C" fn is_function_obj(_: u64, a: u64) -> u64 {
        runtime(RuntimeFn::IsFunctionObj, &[a])
    }
    extern "C" fn call_func_owned(_: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
        runtime(RuntimeFn::CallFuncOwned, &[a, b, c, d])
    }
    extern "C" fn contains(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::Contains, a, b)
    }
    extern "C" fn add(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::Add, a, b)
    }
    extern "C" fn eq(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::Eq, a, b)
    }
    extern "C" fn not(_: u64, a: u64) -> u64 {
        runtime(RuntimeFn::Not, &[a])
    }
    extern "C" fn getitem_method(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::GetitemMethod, a, b)
    }
    extern "C" fn getitem_unchecked(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::GetitemUnchecked, a, b)
    }
    extern "C" fn delitem_method(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::DelitemMethod, a, b)
    }
    extern "C" fn set_attr_name(_: u64, a: u64, b: u64, c: u64) -> u64 {
        runtime(RuntimeFn::SetAttrName, &[a, b, c])
    }
    extern "C" fn setitem_method(_: u64, a: u64, b: u64, c: u64) -> u64 {
        runtime(RuntimeFn::SetitemMethod, &[a, b, c])
    }
    extern "C" fn get_attr_object_ic(_: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
        runtime(RuntimeFn::GetAttrObjectIc, &[a, b, c, d])
    }
    extern "C" fn set_attr_object_ic(_: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> u64 {
        runtime(RuntimeFn::SetAttrObjectIc, &[a, b, c, d, e])
    }
    extern "C" fn guarded_field_get(_: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
        runtime(RuntimeFn::GuardedFieldGet, &[a, b, c, d, e, f])
    }
    #[allow(clippy::too_many_arguments)]
    extern "C" fn guarded_field_set(
        _: u64,
        a: u64,
        b: u64,
        c: u64,
        d: u64,
        e: u64,
        f: u64,
        g: u64,
    ) -> u64 {
        runtime(RuntimeFn::GuardedFieldSet, &[a, b, c, d, e, f, g])
    }
    // A site body's misses for an object pointer; the model has no objects,
    // so a test never reaches them.
    extern "C" fn object_field_get_ptr(_: u64, a: u64, b: u64) -> u64 {
        two(RuntimeFn::ObjectFieldGetPtr, a, b)
    }
    extern "C" fn object_field_set_ptr(_: u64, a: u64, b: u64, c: u64) -> u64 {
        runtime(RuntimeFn::ObjectFieldSetPtr, &[a, b, c])
    }
    extern "C" fn inc_ref_obj(_: u64, word: u64) {
        refcount(RefcountOperation::Retain, word);
    }
    extern "C" fn dec_ref_obj(_: u64, word: u64) {
        refcount(RefcountOperation::Release, word);
    }
    extern "C" fn fmod(left: f64, right: f64) -> f64 {
        // Rust's `%` on floats is C's `fmod`.
        left % right
    }

    fn lookup(name: &str) -> Option<*const u8> {
        if name == FMOD_SYMBOL {
            return Some(fmod as *const u8);
        }
        if name.starts_with("__molt_module_slot_") || name.starts_with("__molt_code_slot_") {
            // The binding the linker would resolve; it lives as long as the
            // code that reads it, which the test never frees before the read.
            let word: &'static u64 = Box::leak(Box::new(program_slot_word(name)));
            return Some(std::ptr::from_ref(word).cast());
        }
        let function = RuntimeFn::ALL
            .iter()
            .copied()
            .find(|function| function.linker_symbol() == name)?;
        Some(match function {
            RuntimeFn::IntFromI64 => int_from_i64 as *const u8,
            RuntimeFn::IntAsI64 => int_as_i64 as *const u8,
            RuntimeFn::IsTruthy => is_truthy as *const u8,
            RuntimeFn::Div => div as *const u8,
            RuntimeFn::Floordiv => floordiv as *const u8,
            RuntimeFn::Mod => modulo as *const u8,
            RuntimeFn::CallInputsRelease => call_inputs_release as *const u8,
            RuntimeFn::StringFromBytes => string_from_bytes as *const u8,
            RuntimeFn::BytesFromBytes => bytes_from_bytes as *const u8,
            RuntimeFn::BigintFromBytes => bigint_from_bytes as *const u8,
            RuntimeFn::GetAttrName => get_attr_name as *const u8,
            RuntimeFn::DelAttrName => del_attr_name as *const u8,
            RuntimeFn::SetAttrName => set_attr_name as *const u8,
            RuntimeFn::CallargsNew => callargs_new as *const u8,
            RuntimeFn::CallargsPushPos => callargs_push_pos as *const u8,
            RuntimeFn::CallBind => call_bind as *const u8,
            RuntimeFn::CallFuncFast0 => call_func_fast0 as *const u8,
            RuntimeFn::CallFuncFast1 => call_func_fast1 as *const u8,
            RuntimeFn::CallFuncFast2 => call_func_fast2 as *const u8,
            RuntimeFn::CallFuncFast3 => call_func_fast3 as *const u8,
            RuntimeFn::IsFunctionObj => is_function_obj as *const u8,
            RuntimeFn::CallFuncOwned => call_func_owned as *const u8,
            RuntimeFn::Contains => contains as *const u8,
            RuntimeFn::Add => add as *const u8,
            RuntimeFn::Eq => eq as *const u8,
            RuntimeFn::Not => not as *const u8,
            RuntimeFn::GetitemMethod => getitem_method as *const u8,
            RuntimeFn::GetitemUnchecked => getitem_unchecked as *const u8,
            RuntimeFn::DelitemMethod => delitem_method as *const u8,
            RuntimeFn::SetitemMethod => setitem_method as *const u8,
            RuntimeFn::GetAttrObjectIc => get_attr_object_ic as *const u8,
            RuntimeFn::SetAttrObjectIc => set_attr_object_ic as *const u8,
            RuntimeFn::GuardedFieldGet => guarded_field_get as *const u8,
            RuntimeFn::GuardedFieldSet => guarded_field_set as *const u8,
            RuntimeFn::ObjectFieldGetPtr => object_field_get_ptr as *const u8,
            RuntimeFn::ObjectFieldSetPtr => object_field_set_ptr as *const u8,
            RuntimeFn::IncRefObj => inc_ref_obj as *const u8,
            RuntimeFn::DecRefObj => dec_ref_obj as *const u8,
            _ => return None,
        })
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
        run_with(inputs, fail_allocation, |b, args| kernel.build(b, args))
    }

    /// [`run`] over a body that builds with `NativeCodegen` itself.
    fn run_with<F>(
        inputs: &[Value],
        fail_allocation: bool,
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
        STUBS.with_borrow_mut(|stubs| {
            *stubs = Stubs::default();
            stubs.model.fail_allocation = fail_allocation;
        });
        kernel_code(0, input_words.as_ptr(), result_words.as_mut_ptr());
        let stubs = STUBS.take();
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
        }
    }

    /// A Cranelift SSA value (the walker tests name their model value `Value`).
    type Value_ = cranelift_codegen::ir::Value;

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
                let run = run_with(&inputs, false, |b, args| {
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
