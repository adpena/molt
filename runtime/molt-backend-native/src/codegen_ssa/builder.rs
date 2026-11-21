//! The instruction-selection trait every native backend implements for the
//! walker (design: `docs/design/foundation/codegen_walker.md`, W2).
//!
//! It extends the word and branch builders the `molt-codegen-abi` generators
//! already use, so one backend value spells both. A method names a machine
//! operation, never a TIR op: what an op means is written once in the walker
//! (`scalar` and the families that follow it). The trait grows only by what a
//! migrated family needs.

use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::list_item::ItemContainers;
use molt_codegen_abi::word::BranchBuilder;
use molt_ir::code_slots::ProgramSlotKind;
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
use molt_ir::native_callable_abi::NativeCallableMachineType;
use molt_ir::runtime_fn_generated::RuntimeFn;

/// The machine shape of a backend value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MachineKind {
    /// A one-bit truth value.
    Bit,
    /// An integer narrower than a word that is not a truth value (a runtime
    /// entry's 32-bit result).
    NarrowInt,
    /// A 64-bit integer: a raw integer or a boxed word.
    Word,
    /// A 64-bit float.
    Float,
    /// A machine address. Only LLVM has a pointer type; Cranelift carries
    /// addresses as words, so without LLVM nothing constructs this kind.
    #[cfg_attr(not(feature = "llvm"), allow(dead_code))]
    Address,
}

/// A comparison of two floats. Every ordered comparison is false when either
/// operand is NaN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloatComparison {
    OrderedEqual,
    /// True when the operands differ or either is NaN (`nan != nan`,
    /// `bool(nan)`).
    UnorderedOrNotEqual,
    OrderedLess,
    OrderedLessOrEqual,
    OrderedGreater,
    OrderedGreaterOrEqual,
}

/// A comparison of two words as signed integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntComparison {
    Equal,
    NotEqual,
    SignedLess,
    SignedLessOrEqual,
    SignedGreater,
    SignedGreaterOrEqual,
}

/// A two-operand machine integer instruction. Arithmetic wraps; a shift
/// count outside `[0, 63]` and a zero divisor are undefined, so the walker
/// proves them first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntBinary {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    ShiftLeft,
    ShiftRightArithmetic,
    /// The shift that fills with zeros.
    ShiftRightLogical,
    /// The quotient truncated toward zero.
    SignedQuotient,
    /// The remainder with the dividend's sign.
    SignedRemainder,
}

/// A machine integer instruction that also answers whether the signed
/// result overflowed 64 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckedBinary {
    Add,
    Mul,
}

/// A two-operand IEEE float instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloatBinary {
    Add,
    Sub,
    Mul,
    Divide,
    /// The remainder with the dividend's sign (C `fmod`).
    Remainder,
}

/// A site body: one access the `molt_codegen_abi` generators write, whose
/// miss calls a runtime entry. Its words are listed per variant; the refcount
/// admission follows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SiteBody {
    /// `object.<field at offset>`, owned (`generate_field_slot_get`); words
    /// `[object]`; the miss is `molt_object_field_get_ptr`.
    FieldGet { offset: i32 },
    /// `object.<field at offset> = value`, answering None
    /// (`generate_field_slot_set`); words `[object, value]`; the miss is
    /// `molt_object_field_set_ptr`.
    FieldSet { offset: i32 },
    /// A proven builtin exception member, owned
    /// (`molt_codegen_abi::builtin_field::generate_builtin_field_get`); words
    /// `[object]`; the miss is `molt_exception_field_get(object, tag)`.
    BuiltinFieldGet {
        field: molt_codegen_abi::exception_layout::ExceptionTypedField,
    },
    /// `sequence[index]`, owned, over the `containers` a proof admits
    /// (`generate_list_item_get`); words `[sequence, index]`; the miss is
    /// `cold`.
    ItemGet {
        containers: ItemContainers,
        cold: RuntimeFn,
    },
    /// `list[index] = value` (`generate_list_item_set`); words `[list,
    /// index, value]`; the miss is `molt_setitem_method`.
    ListItemSet,
    /// A module global's slot `index`, owned (`generate_global_slot_read`);
    /// words `[base, module, name]`; the miss is `molt_module_get_global`.
    GlobalSlotRead { index: u32 },
    /// Store into a module's static cell `index`, answering None
    /// (`generate_static_cell_store`); words `[base, value]`.
    StaticCellStore { index: u32 },
    /// A closed import of the module `id` names, owned
    /// (`generate_module_import_closed`); words `[id]`; the miss is
    /// `molt_module_ensure`.
    ModuleImportClosed,
    /// A method site's load (`generate_method_site_load`), owned; words
    /// `[record, receiver, self_out]`; the miss is `molt_method_site_load`.
    MethodSiteLoad,
    /// A method site's call with `arguments` arguments after `self`
    /// (`generate_method_site_call`), adopting every word; words `[thread,
    /// record, callable, self_or_null, args...]`; the miss is `cold`, the
    /// runtime's fixed-arity loaded-method call.
    MethodSiteCall { arguments: usize, cold: RuntimeFn },
    /// The owned word in a closure payload (`generate_closure_load`); words
    /// `[owner_address, offset]`; it never misses.
    ClosureLoad,
    /// Store a borrowed word in a closure payload, answering None
    /// (`generate_closure_store`); words `[owner_address, offset, value]`;
    /// the miss is `molt_closure_store`.
    ClosureStore,
    /// A new instance of a class allocated from its lifecycle plan, owned
    /// (`object_lifecycle::generate_object_alloc`); words `[class, profile]`.
    /// The miss is `molt_object_new` for the root `object` (`root`), whose
    /// class word may still be zero, else `molt_object_new_bound(class)`.
    /// `plan` is where the site finds the class's lifecycle plan.
    ObjectAlloc {
        root: bool,
        plan: molt_codegen_abi::object_lifecycle::PlanSource,
    },
    /// The release of an object whose count read 1
    /// (`object_lifecycle::generate_release_terminal`), answering None;
    /// words `[bits]`; the miss is `molt_dec_ref_obj`. Only the backend's
    /// release refcount sites reach it, through their slow call.
    ReleaseTerminal,
    /// `type(value)`, owned (`object_lifecycle::generate_type_of`); words
    /// `[value]`; the miss is `molt_type_of`.
    TypeOf,
    /// `BUILTIN_TYPE(tag)`, owned (`object_lifecycle::generate_builtin_type`);
    /// words `[tag]`; the miss is `molt_builtin_type`.
    BuiltinType,
}

/// One step of the inline recursion charge of a direct call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecursionCharge {
    Admits,
    Count,
    Release,
}

/// A helper function's body, written once for every builder
/// ([`CodegenBuilder::define_helper`]). A helper is another function, so its
/// body is generic over the builder the backend makes for it, never the
/// caller's. Each backend's helper builder places tasks too, so a task
/// trampoline is a helper body like any other.
pub(crate) trait HelperBody {
    /// Build the body over the helper's word parameters and answer its word.
    fn build<H: super::task::TaskPlacement>(&self, h: &mut H, params: &[H::Word]) -> H::Word;
}

pub(crate) trait CodegenBuilder: BranchBuilder {
    /// Any machine value the backend holds for a TIR value.
    type Value: Copy;
    /// A block of the function being built.
    type Block: Copy;
    /// A word or word array of the function's entry storage.
    type Slot: Copy;
    /// A function the walker calls: a runtime entry or a backend helper.
    type Callee: Copy;
    /// See [`CodegenBuilder::refcount_site`].
    type RefcountSite: Copy;

    fn kind(&self, value: Self::Value) -> MachineKind;
    /// A word as a general value.
    fn word_value(&self, word: Self::Word) -> Self::Value;
    /// A truth bit as a general value.
    fn bit_value(&self, bit: Self::Predicate) -> Self::Value;
    /// The word a `MachineKind::Word` value is.
    fn expect_word(&self, value: Self::Value) -> Self::Word;
    /// The bit a `MachineKind::Bit` value is.
    fn expect_bit(&self, value: Self::Value) -> Self::Predicate;

    /// Zero-extend a `Bit` or `NarrowInt` value to a word.
    fn zero_extend(&mut self, value: Self::Value) -> Self::Word;
    /// The low bit of a word as a truth bit.
    fn low_bit(&mut self, word: Self::Word) -> Self::Predicate;
    /// The 64 bits of a `Float` value, unchanged.
    fn float_to_bits(&mut self, value: Self::Value) -> Self::Word;
    /// The float whose 64 bits are `word`.
    fn bits_to_float(&mut self, word: Self::Word) -> Self::Value;
    /// An `Address` value as a word.
    fn address_to_word(&mut self, value: Self::Value) -> Self::Word;
    /// The address whose bits are `word`, as a `MachineKind::Address` value.
    fn word_to_address(&mut self, word: Self::Word) -> Self::Value;

    /// `no_signed_wrap` is a hint that the walker proved the signed result
    /// exact; a backend with no use for it ignores it.
    fn int_binary(
        &mut self,
        op: IntBinary,
        left: Self::Word,
        right: Self::Word,
        no_signed_wrap: bool,
    ) -> Self::Word;
    fn int_compare(
        &mut self,
        comparison: IntComparison,
        left: Self::Word,
        right: Self::Word,
    ) -> Self::Predicate;
    /// The wrapping result and its signed-overflow bit.
    fn int_checked(
        &mut self,
        op: CheckedBinary,
        left: Self::Word,
        right: Self::Word,
    ) -> (Self::Word, Self::Predicate);
    /// Whether an integer value of any width (a word, a narrow int or a
    /// bit) is zero, compared in its own width.
    fn is_zero(&mut self, value: Self::Value) -> Self::Predicate;
    /// The float nearest a signed integer.
    fn int_to_float(&mut self, word: Self::Word) -> Self::Value;
    /// `yes` when `condition` holds, else `no`; both are of one kind.
    fn select(
        &mut self,
        condition: Self::Predicate,
        yes: Self::Value,
        no: Self::Value,
    ) -> Self::Value;
    fn bit_xor(&mut self, left: Self::Predicate, right: Self::Predicate) -> Self::Predicate;
    /// Two's-complement negation.
    fn int_negate(&mut self, word: Self::Word) -> Self::Word;
    /// Every bit flipped.
    fn int_invert(&mut self, word: Self::Word) -> Self::Word;
    fn bit_not(&mut self, bit: Self::Predicate) -> Self::Predicate;

    /// `fast_math` is a hint that the op carries the fast-math annotation; a
    /// backend with no relaxed float mode ignores it.
    fn float_binary(
        &mut self,
        op: FloatBinary,
        left: Self::Value,
        right: Self::Value,
        fast_math: bool,
    ) -> Self::Value;
    fn float_negate(&mut self, value: Self::Value) -> Self::Value;
    fn float_constant(&mut self, value: f64) -> Self::Value;
    fn float_compare(
        &mut self,
        comparison: FloatComparison,
        left: Self::Value,
        right: Self::Value,
    ) -> Self::Predicate;

    /// The callee that is runtime entry `function`, by its typed identity;
    /// the generated record supplies symbol, signature and context.
    fn runtime_callee(&mut self, function: RuntimeFn) -> Self::Callee;
    /// The compiled function `symbol` as a callee, declared by its frozen
    /// linkage row `abi`: one machine parameter per row parameter, each in
    /// its carrier's machine kind, and the row's result. The execution
    /// context is the backend's, as for a runtime entry.
    fn function_callee(&mut self, symbol: &str, abi: &NativeFunctionLinkageAbi) -> Self::Callee;
    /// A source extension's native symbol as a callee, declared once with the
    /// exact machine signature its ABI token names
    /// (`molt_ir::native_callable_abi`). It takes no execution context: the
    /// final link resolves it from the extension's archive or object, which
    /// knows nothing of Molt's attached calls.
    fn native_callee(
        &mut self,
        symbol: &str,
        params: &[NativeCallableMachineType],
        result: NativeCallableMachineType,
    ) -> Self::Callee;
    /// The code address of a compiled function's callee, as a word.
    fn function_address(&mut self, callee: Self::Callee) -> Self::Word;
    /// Call `callee` with one machine value per parameter. The result is
    /// `None` when the callee returns nothing; otherwise it has the kind of
    /// the callee's result (a 32-bit runtime status is `NarrowInt`).
    fn call(&mut self, callee: Self::Callee, args: &[Self::Value]) -> Option<Self::Value>;
    /// Call a runtime entry whose parameters and result are words.
    fn call_runtime(&mut self, function: RuntimeFn, args: &[Self::Word]) -> Option<Self::Word> {
        let callee = self.runtime_callee(function);
        let args: Vec<Self::Value> = args.iter().map(|&arg| self.word_value(arg)).collect();
        let result = self.call(callee, &args)?;
        Some(self.expect_word(result))
    }

    /// The thread record lent by this invocation. Its entry-block value
    /// dominates every request, including a first request in a cold block.
    fn thread_record_word(&mut self) -> Self::Word;
    /// Whether async work waits for a poll: the thread's pending-work word
    /// (its address lent by the thread record, read once per invocation) is
    /// nonzero. The read is a relaxed atomic load no pass merges with another
    /// read or hoists out of a loop.
    fn async_work_pending(&mut self) -> Self::Predicate;
    /// Apply one step of a direct call's inline recursion charge to this
    /// invocation's `thread` record (the shared
    /// `molt_codegen_abi::thread_record` generators): whether the limit
    /// admits one more frame, or count or release one. Only `Admits`
    /// answers.
    fn recursion_charge(
        &mut self,
        charge: RecursionCharge,
        thread: Self::Word,
    ) -> Option<Self::Predicate>;
    /// Load one unsigned thread-local byte at a fixed byte offset.
    fn load_byte(&mut self, address: Self::Word, offset: i32) -> Self::Value;
    /// A call in a cold block, with the usual execution-context ABI.
    fn call_cold(&mut self, callee: Self::Callee, args: &[Self::Value]) -> Option<Self::Value>;

    /// A new block of the function being built, after every existing one.
    /// `name` is a debug name; a backend may ignore it.
    fn create_block(&mut self, name: &str) -> Self::Block;
    /// Give `value` the debug name `name`; a backend may ignore it.
    fn set_name(&mut self, value: Self::Value, name: &str);
    /// Build at the end of `block` from now on.
    fn switch_to_block(&mut self, block: Self::Block);

    /// A word of stack storage the function owns for its whole invocation,
    /// allocated once in its entry: an operation that runs in a loop reuses
    /// it instead of growing the stack. `name` is a debug name.
    fn entry_slot(&mut self, name: &str) -> Self::Slot;
    /// `len` consecutive words of entry storage, as [`Self::entry_slot`].
    fn entry_slot_array(&mut self, len: u32, name: &str) -> Self::Slot;
    fn load_slot(&mut self, slot: Self::Slot) -> Self::Word;
    fn store_slot(&mut self, slot: Self::Slot, word: Self::Word);
    /// Store `word` into word `index` of an entry array.
    fn store_slot_at(&mut self, slot: Self::Slot, index: u32, word: Self::Word);
    /// Load word `index` of an entry array.
    fn load_slot_at(&mut self, slot: Self::Slot, index: u32) -> Self::Word;
    /// The address of an entry slot, as a `MachineKind::Address` value.
    fn slot_pointer(&mut self, slot: Self::Slot) -> Self::Value;

    /// A truth-bit constant.
    fn bit_constant(&mut self, value: bool) -> Self::Predicate;
    /// The address of a private, read-only copy of `bytes` in the program's
    /// data, as a `MachineKind::Address` value. `prefix` and `suffix` name
    /// it where the backend names data; a backend may ignore them.
    fn bytes_constant(&mut self, bytes: &[u8], prefix: &str, suffix: &str) -> Self::Value;
    /// The address of the program's private, writable, 8-byte-aligned data
    /// object `symbol`, as a `MachineKind::Address` value: a site's record,
    /// which the site's code updates. The first request creates it with
    /// `image`; a later one answers the same object.
    fn site_record(&mut self, symbol: &str, image: &[u8]) -> Self::Value;
    /// The address of `module`'s static cell block (one word per layout
    /// name), resolved when the program links.
    fn static_cell_block(&mut self, module: &str) -> Self::Value;
    /// The word at byte `offset` from `address`, an `Address` value.
    fn load_word(&mut self, address: Self::Value, offset: i32) -> Self::Word;
    /// Store `word` at byte `offset` from `address`, an `Address` value.
    fn store_word(&mut self, address: Self::Value, offset: i32, word: Self::Word);
    /// The immutable word the program binds to `owner`'s program slot of
    /// `kind` (`molt_ir::code_slots::program_slot_symbol`), resolved when the
    /// program links.
    fn program_slot(&mut self, kind: ProgramSlotKind, owner: &str) -> Self::Word;
    /// The program's private helper function `name`: `params` word
    /// parameters after the execution context, answering a word. The first
    /// request defines it from `body`, which the backend runs over a builder
    /// positioned in the helper's entry; a later request answers the same
    /// function. The helper takes the execution context as a compiled
    /// function does.
    fn define_helper(&mut self, name: &str, params: usize, body: &impl HelperBody) -> Self::Callee;

    /// One reference-count site: the transition `operation` that each
    /// [`Self::refcount`] through it applies. The transition itself is the
    /// shared `molt_codegen_abi::inline_refcount` generator; a backend
    /// chooses only where its body lives (inline, or one out-of-line copy
    /// once a function holds many sites).
    fn refcount_site(&mut self, operation: RefcountOperation) -> Self::RefcountSite;
    /// Apply `site`'s transition to the boxed word `word`.
    fn refcount(&mut self, site: Self::RefcountSite, word: Self::Word);
    /// The invocation's refcount admission word, which a site body passes to
    /// its retains and releases.
    fn refcount_admission(&mut self) -> Self::Word;
    /// The thread record's profile-enabled word (1 when the process collects
    /// runtime profile counters), read at most once per invocation: a
    /// profile-only guard tests it before its runtime call.
    fn profile_enabled(&mut self) -> Self::Word;

    /// Run the shared generator's `body` on `args` (its words, the refcount
    /// admission last) and answer its word. A backend chooses only where the
    /// body lives: inline, or one out-of-line copy per body.
    fn site(&mut self, body: SiteBody, args: &[Self::Value]) -> Self::Value;

    /// A value of machine kind `kind` that stands where the lowering has
    /// already refused the function: it keeps the function well formed for
    /// the next diagnostic and never executes.
    fn placeholder(&mut self, kind: MachineKind) -> Self::Value;

    /// The active record's homes slot, also used for cold traceback capture.
    fn frame_homes(&self) -> Option<Self::Slot>;
    fn set_frame_homes(&mut self, slot: Option<Self::Slot>);

    /// Append a parameter of machine kind `kind` to `block`, before the first
    /// branch to it. The result is the parameter's value inside the block;
    /// every branch to `block` passes one argument per parameter, in order.
    /// LLVM builds a phi; Cranelift has block parameters natively.
    fn append_block_param(&mut self, block: Self::Block, kind: MachineKind) -> Self::Value;
    /// End the current block with a jump that passes `args` to `target`.
    fn jump(&mut self, target: Self::Block, args: &[Self::Value]);
    /// End the current block with a two-way branch on `condition`.
    fn branch_if(
        &mut self,
        condition: Self::Predicate,
        yes: (Self::Block, &[Self::Value]),
        no: (Self::Block, &[Self::Value]),
    );
    /// End the current block with a branch on `selector`: the case equal to
    /// it, else `default`.
    fn switch(
        &mut self,
        selector: Self::Word,
        cases: &[(i64, Self::Block, Vec<Self::Value>)],
        default: (Self::Block, &[Self::Value]),
    );
    /// End the current block on a path that never executes.
    fn unreachable(&mut self);
    /// Retire the frame whose base was taken from the invocation's entry slot.
    fn pop_frame(
        &mut self,
        base: Self::Word,
        kind: Option<molt_codegen_abi::frame_record::FrameEntryKind>,
    );
    /// Return a value already converted to the function's declared machine ABI.
    fn return_value(&mut self, value: Option<Self::Value>);
}
