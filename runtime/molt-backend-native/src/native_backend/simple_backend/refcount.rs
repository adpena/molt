use super::*;
use molt_codegen_abi::inline_refcount::{
    RefcountBuilder, RefcountOperation, RefcountStorage, generate_refcount,
};
use molt_codegen_abi::word::{BranchBuilder, WordBuilder, WordComparison};

/// Function-local imports and the admission belonging to this exact body.
/// Keeping the invocation fact here makes helpers consume the entry's value
/// rather than rediscovering attachment or borrowing another function's fact.
pub(crate) struct NativeFunctionImports {
    /// This body's reference to each runtime entry, by dense `RuntimeFn` index.
    refs: Box<[cranelift_codegen::packed_option::PackedOption<FuncRef>]>,
    refcount_admission: Option<Value>,
}

impl Default for NativeFunctionImports {
    fn default() -> Self {
        Self {
            refs: vec![Default::default(); RuntimeFn::COUNT].into_boxed_slice(),
            refcount_admission: None,
        }
    }
}

impl NativeFunctionImports {
    pub(crate) fn get(&self, function: RuntimeFn) -> Option<FuncRef> {
        self.refs[function.index()].expand()
    }

    pub(crate) fn insert(&mut self, function: RuntimeFn, value: FuncRef) {
        self.refs[function.index()] = value.into();
    }

    pub(crate) fn set_refcount_admission(&mut self, value: Value) {
        assert!(self.refcount_admission.replace(value).is_none());
    }

    pub(crate) fn refcount_site(
        &self,
        slow: FuncRef,
        operation: RefcountOperation,
    ) -> NativeRefcountSite {
        NativeRefcountSite {
            slow,
            admission: self
                .refcount_admission
                .expect("refcount before invocation admission"),
            operation,
        }
    }
}

/// One invocation's immutable admission and its authoritative cold transition.
/// The entry's SSA value dominates all sites, including loop backedges. It is
/// never a Cranelift Variable, so SSA repair cannot invent another admission.
#[derive(Clone, Copy)]
pub(crate) struct NativeRefcountSite {
    pub(crate) slow: FuncRef,
    pub(crate) admission: Value,
    pub(crate) operation: RefcountOperation,
}

#[cfg(feature = "native-backend")]
pub(crate) fn emit_refcount(builder: &mut FunctionBuilder, value: Value, site: NativeRefcountSite) {
    let storage = RefcountStorage::for_build(molt_codegen_abi::MOLT_REFCOUNT_ATOMIC);
    let mut emitter = NativeRefcountBuilder {
        words: NativeWords { builder },
        site,
        slow_block: None,
    };
    generate_refcount(&mut emitter, value, site.admission, site.operation, storage);
    if let Some(slow) = emitter.slow_block {
        let continuation = emitter.words.builder.create_block();
        emitter.words.builder.ins().jump(continuation, &[]);
        emitter.words.builder.switch_to_block(slow);
        emitter.words.builder.seal_block(slow);
        emitter.words.builder.set_cold_block(slow);
        emitter.words.builder.ins().call(site.slow, &[value]);
        emitter.words.builder.ins().jump(continuation, &[]);
        emitter.words.builder.seal_block(continuation);
        emitter.words.builder.switch_to_block(continuation);
    }
}

/// A compiled read of module global slot `index` (call_anatomy.md 4.5): the
/// shared generator decides; this file only selects instructions. `base` is
/// the activation's boxed base word; an unbound slot calls `cold` with
/// `cold_args` (the site's by-name `molt_module_get_global`).
#[cfg(feature = "native-backend")]
pub(crate) fn emit_global_slot_read(
    builder: &mut FunctionBuilder,
    base: Value,
    index: u32,
    retain: NativeRefcountSite,
    cold: FuncRef,
    cold_args: &[Value],
) -> Value {
    let mut emitter = NativeRefcountBuilder {
        words: NativeWords { builder },
        site: retain,
        slow_block: None,
    };
    molt_codegen_abi::global_slot::generate_global_slot_read(&mut emitter, base, index, |b| {
        let call = b.words.builder.ins().call(cold, cold_args);
        b.words.builder.inst_results(call)[0]
    })
}

/// A compiled store into static cell `index` (call_anatomy.md 4.5a): the
/// shared generator decides; this file only selects instructions. `base` is
/// the module's boxed block base; `value` is retained into the cell and the
/// displaced word released.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_static_cell_store(
    builder: &mut FunctionBuilder,
    base: Value,
    index: u32,
    value: Value,
    retain: NativeRefcountSite,
    release: NativeRefcountSite,
) {
    let mut emitter = NativeCellBuilder {
        inner: NativeRefcountBuilder {
            words: NativeWords { builder },
            site: retain,
            slow_block: None,
        },
        release,
    };
    molt_codegen_abi::static_cell::generate_static_cell_store(
        &mut emitter,
        base,
        index,
        value,
        molt_codegen_abi::MOLT_REFCOUNT_ATOMIC,
    );
}

/// The slot builder plus the release site a cell store needs.
#[cfg(feature = "native-backend")]
struct NativeCellBuilder<'a, 'b> {
    inner: NativeRefcountBuilder<'a, 'b>,
    release: NativeRefcountSite,
}

#[cfg(feature = "native-backend")]
impl<'a, 'b> WordBuilder for NativeCellBuilder<'a, 'b> {
    molt_codegen_abi::forward_word_builder!(inner: NativeRefcountBuilder<'a, 'b>);
}

#[cfg(feature = "native-backend")]
impl BranchBuilder for NativeCellBuilder<'_, '_> {
    fn choose(
        &mut self,
        condition: Value,
        yes: impl FnOnce(&mut Self) -> Value,
        no: impl FnOnce(&mut Self) -> Value,
    ) -> Value {
        cranelift_choose(self, condition, yes, no)
    }
}

#[cfg(feature = "native-backend")]
impl<'b> CraneliftWords<'b> for NativeCellBuilder<'_, 'b> {
    fn cranelift(&mut self) -> &mut FunctionBuilder<'b> {
        self.inner.words.builder
    }
}

#[cfg(feature = "native-backend")]
impl molt_codegen_abi::global_slot::SlotBuilder for NativeCellBuilder<'_, '_> {
    fn load_word(&mut self, address: Value, offset: i32) -> Value {
        self.inner.load_word(address, offset)
    }
    fn retain(&mut self, word: Value) {
        self.inner.retain(word);
    }
}

#[cfg(feature = "native-backend")]
impl molt_codegen_abi::static_cell::CellBuilder for NativeCellBuilder<'_, '_> {
    fn swap_word(&mut self, address: Value, offset: i32, value: Value, atomic: bool) -> Value {
        let builder = &mut *self.inner.words.builder;
        if atomic {
            let at = builder.ins().iadd_imm_s(address, i64::from(offset));
            return builder.ins().atomic_rmw(
                types::I64,
                MemFlagsData::new(),
                cranelift_codegen::ir::AtomicRmwOp::Xchg,
                at,
                value,
            );
        }
        let old = builder
            .ins()
            .load(types::I64, MemFlagsData::new(), address, offset);
        builder
            .ins()
            .store(MemFlagsData::new(), value, address, offset);
        old
    }
    fn release(&mut self, word: Value) {
        emit_refcount(&mut *self.inner.words.builder, word, self.release);
    }
}

/// The Cranelift builder a native word builder spells into.
trait CraneliftWords<'b> {
    fn cranelift(&mut self) -> &mut FunctionBuilder<'b>;
}

impl<'b> CraneliftWords<'b> for NativeWords<'_, 'b> {
    fn cranelift(&mut self) -> &mut FunctionBuilder<'b> {
        self.builder
    }
}

impl<'b> CraneliftWords<'b> for NativeRefcountBuilder<'_, 'b> {
    fn cranelift(&mut self) -> &mut FunctionBuilder<'b> {
        self.words.builder
    }
}

/// `BranchBuilder::choose` in Cranelift: a branch to a hot block and a cold
/// block that join with one i64 parameter. Both native builders use it.
fn cranelift_choose<'b, S: CraneliftWords<'b>>(
    s: &mut S,
    condition: Value,
    yes: impl FnOnce(&mut S) -> Value,
    no: impl FnOnce(&mut S) -> Value,
) -> Value {
    let yes_block = s.cranelift().create_block();
    let no_block = s.cranelift().create_block();
    let join = s.cranelift().create_block();
    s.cranelift().append_block_param(join, types::I64);
    s.cranelift()
        .ins()
        .brif(condition, yes_block, &[], no_block, &[]);
    s.cranelift().seal_block(yes_block);
    s.cranelift().seal_block(no_block);
    s.cranelift().set_cold_block(no_block);
    s.cranelift().switch_to_block(yes_block);
    let taken = yes(s);
    s.cranelift().ins().jump(join, &[taken.into()]);
    s.cranelift().switch_to_block(no_block);
    let fallback = no(s);
    s.cranelift().ins().jump(join, &[fallback.into()]);
    s.cranelift().seal_block(join);
    s.cranelift().switch_to_block(join);
    s.cranelift().block_params(join)[0]
}

impl BranchBuilder for NativeWords<'_, '_> {
    fn choose(
        &mut self,
        condition: Value,
        yes: impl FnOnce(&mut Self) -> Value,
        no: impl FnOnce(&mut Self) -> Value,
    ) -> Value {
        cranelift_choose(self, condition, yes, no)
    }
}

impl BranchBuilder for NativeRefcountBuilder<'_, '_> {
    fn choose(
        &mut self,
        condition: Value,
        yes: impl FnOnce(&mut Self) -> Value,
        no: impl FnOnce(&mut Self) -> Value,
    ) -> Value {
        cranelift_choose(self, condition, yes, no)
    }
}

#[cfg(feature = "native-backend")]
impl molt_codegen_abi::global_slot::SlotBuilder for NativeRefcountBuilder<'_, '_> {
    fn load_word(&mut self, address: Value, offset: i32) -> Value {
        self.words
            .builder
            .ins()
            .load(types::I64, MemFlagsData::new(), address, offset)
    }
    fn retain(&mut self, word: Value) {
        emit_refcount(&mut *self.words.builder, word, self.site);
    }
}

struct NativeRefcountBuilder<'a, 'b> {
    words: NativeWords<'a, 'b>,
    site: NativeRefcountSite,
    slow_block: Option<Block>,
}

/// Cranelift's one spelling of 64-bit word instructions.
pub(crate) struct NativeWords<'a, 'b> {
    pub(crate) builder: &'a mut FunctionBuilder<'b>,
}

impl WordBuilder for NativeWords<'_, '_> {
    type Word = Value;
    type Predicate = Value;

    fn constant(&mut self, value: u64) -> Value {
        self.builder.ins().iconst(types::I64, value as i64)
    }
    fn and(&mut self, value: Value, mask: u64) -> Value {
        self.builder.ins().band_imm_s(value, mask as i64)
    }
    fn or(&mut self, value: Value, bits: u64) -> Value {
        self.builder.ins().bor_imm_s(value, bits as i64)
    }
    fn shift_left(&mut self, value: Value, amount: u32) -> Value {
        self.builder.ins().ishl_imm_s(value, i64::from(amount))
    }
    fn shift_right_arithmetic(&mut self, value: Value, amount: u32) -> Value {
        self.builder.ins().sshr_imm_s(value, i64::from(amount))
    }
    fn add_immediate(&mut self, value: Value, delta: i64) -> Value {
        self.builder.ins().iadd_imm_s(value, delta)
    }
    fn compare(&mut self, comparison: WordComparison, left: Value, right: Value) -> Value {
        use cranelift_codegen::ir::condcodes::IntCC;
        let condition = match comparison {
            WordComparison::Equal => IntCC::Equal,
            WordComparison::NotEqual => IntCC::NotEqual,
            WordComparison::UnsignedLessThan => IntCC::UnsignedLessThan,
        };
        self.builder.ins().icmp(condition, left, right)
    }
    fn both(&mut self, left: Value, right: Value) -> Value {
        self.builder.ins().band(left, right)
    }
}

impl<'a, 'b> WordBuilder for NativeRefcountBuilder<'a, 'b> {
    molt_codegen_abi::forward_word_builder!(words: NativeWords<'a, 'b>);
}

impl RefcountBuilder for NativeRefcountBuilder<'_, '_> {
    fn pointer_width(&self) -> molt_codegen_abi::PointerWidth {
        molt_codegen_abi::PointerWidth::Bits64
    }
    fn load_header_u32(&mut self, pointer: Value, offset: i32) -> Value {
        let value = self
            .words
            .builder
            .ins()
            .load(types::I32, MemFlagsData::new(), pointer, offset);
        self.words.builder.ins().uextend(types::I64, value)
    }
    fn store_header_u32(&mut self, pointer: Value, offset: i32, value: Value) {
        let value = self.words.builder.ins().ireduce(types::I32, value);
        self.words
            .builder
            .ins()
            .store(MemFlagsData::new(), value, pointer, offset);
    }
    fn slow_call(&mut self, operation: RefcountOperation, _bits: Value) {
        assert_eq!(operation, self.site.operation);
        let block = *self
            .slow_block
            .get_or_insert_with(|| self.words.builder.create_block());
        self.words.builder.ins().jump(block, &[]);
    }
    fn if_else(
        &mut self,
        condition: Value,
        yes: impl FnOnce(&mut Self),
        no: impl FnOnce(&mut Self),
    ) {
        let yes_block = self.words.builder.create_block();
        let no_block = self.words.builder.create_block();
        let continuation = self.words.builder.create_block();
        self.words
            .builder
            .ins()
            .brif(condition, yes_block, &[], no_block, &[]);
        self.words.builder.seal_block(yes_block);
        self.words.builder.seal_block(no_block);
        self.words.builder.switch_to_block(yes_block);
        yes(self);
        if !self.terminated() {
            self.words.builder.ins().jump(continuation, &[]);
        }
        self.words.builder.switch_to_block(no_block);
        no(self);
        if !self.terminated() {
            self.words.builder.ins().jump(continuation, &[]);
        }
        self.words.builder.seal_block(continuation);
        self.words.builder.switch_to_block(continuation);
    }
}

impl NativeRefcountBuilder<'_, '_> {
    fn terminated(&self) -> bool {
        self.words
            .builder
            .current_block()
            .and_then(|block| self.words.builder.func.layout.last_inst(block))
            .is_some_and(|inst| {
                self.words.builder.func.dfg.insts[inst]
                    .opcode()
                    .is_terminator()
            })
    }
}

/// Resolve an entry SSA placeholder only if a generated transition consumes it.
/// This is code-emission state, not a runtime/fact cache: a scalar-only body
/// leaves no admission call. Replacement preserves the exact entry SSA value.
pub(crate) fn materialize_refcount_admission(
    builder: &mut FunctionBuilder<'_>,
    admission: Value,
    import: impl FnOnce(&mut FunctionBuilder<'_>) -> FuncRef,
) {
    if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC {
        return;
    }
    let used = builder.func.layout.blocks().any(|block| {
        builder
            .func
            .layout
            .block_insts(block)
            .any(|inst| builder.func.dfg.inst_args(inst).contains(&admission))
    });
    if used {
        let callee = import(builder);
        let inst = builder
            .func
            .dfg
            .value_def(admission)
            .inst()
            .expect("entry admission placeholder");
        builder.func.replace(inst).call(callee, &[]);
    }
}
