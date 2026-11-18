//! Scalar carriers: how a raw machine value becomes a boxed word and back.
//!
//! Written once for every native backend. The NaN-box encoding itself comes
//! from the `molt-codegen-abi` generators; this module owns the choice of
//! generator or runtime entry per type.

use molt_codegen_abi as nanbox;
use molt_codegen_abi::word::WordComparison;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, FloatComparison, MachineKind};
use super::value_table::HeldCarrier;
use crate::tir::types::TirType;

fn runtime_word<B: CodegenBuilder>(b: &mut B, function: RuntimeFn, args: &[B::Word]) -> B::Word {
    b.call_runtime(function, args)
        .unwrap_or_else(|| panic!("runtime entry {function:?} returns a word"))
}

/// `value` as one 64-bit word, bits preserved: a truth bit zero-extends, a
/// float keeps its bits (a float is its own boxed word, runtime spec 2.1.1).
pub(crate) fn word_of<B: CodegenBuilder>(b: &mut B, value: B::Value) -> B::Word {
    match b.kind(value) {
        MachineKind::Word => b.expect_word(value),
        MachineKind::Bit | MachineKind::NarrowInt => b.zero_extend(value),
        MachineKind::Float => b.float_to_bits(value),
        MachineKind::Address => b.address_to_word(value),
    }
}

/// Box a raw integer through the runtime (`molt_int_from_i64`): the inline
/// box when it fits, otherwise one fresh heap BigInt owner, or None with
/// MemoryError pending when that allocation fails. One value, no control
/// flow, so it is valid where the block may not be split.
pub(crate) fn box_int_by_runtime<B: CodegenBuilder>(b: &mut B, raw: B::Word) -> B::Word {
    runtime_word(b, RuntimeFn::IntFromI64, &[raw])
}

/// Box a raw integer of any width: the inline box on the hot arm when the
/// value fits the 47-bit payload, the runtime on the cold arm. A borrowing
/// consumer must not box through this directly: the heap box it may mint has
/// no owner (operation-local custody records one).
pub(crate) fn box_int<B: CodegenBuilder>(b: &mut B, raw: B::Word) -> B::Word {
    let fits = nanbox::boxed_word::generate_inline_int_fits(b, raw);
    b.choose(
        fits,
        |b| nanbox::boxed_word::generate_box_inline_int(b, raw),
        |b| box_int_by_runtime(b, raw),
    )
}

/// The boxed word of `value`, held as `ty`'s carrier.
pub(crate) fn box_value<B: CodegenBuilder>(b: &mut B, value: B::Value, ty: &TirType) -> B::Word {
    let word = word_of(b, value);
    match HeldCarrier::of(ty) {
        HeldCarrier::RawI64 => box_int(b, word),
        HeldCarrier::RawBool => b.or(word, nanbox::QNAN | nanbox::TAG_BOOL),
        // A float is its own word; a boxed word (a None annotation included:
        // only a ConstNone producer supplies the singleton) already is one.
        HeldCarrier::RawF64 | HeldCarrier::Boxed | HeldCarrier::Never => word,
    }
}

/// The raw integer of a boxed word the caller proved an integer representable
/// in 64 bits. The runtime decodes inline integers and heap BigInts and
/// preserves a pending exception. Branchless: a block-parameter coercion
/// places it before an existing terminator and cannot split the block.
pub(crate) fn unbox_int<B: CodegenBuilder>(b: &mut B, word: B::Word) -> B::Word {
    runtime_word(b, RuntimeFn::IntAsI64, &[word])
}

/// A boxed word decoded to the word a parameter typed `ty` travels as in the
/// direct calling convention: the integer, a bool's payload bit, or the word
/// itself (a float's bits, a reference).
pub(crate) fn unbox_to_param_word<B: CodegenBuilder>(
    b: &mut B,
    word: B::Word,
    ty: &TirType,
) -> B::Word {
    match HeldCarrier::of(ty) {
        HeldCarrier::RawI64 => unbox_int(b, word),
        HeldCarrier::RawBool => b.and(word, 1),
        HeldCarrier::RawF64 | HeldCarrier::Boxed | HeldCarrier::Never => word,
    }
}

/// A boxed `value` decoded to `ty`'s carrier; a value whose carrier is boxed
/// is returned as it is.
pub(crate) fn unbox_value<B: CodegenBuilder>(b: &mut B, value: B::Value, ty: &TirType) -> B::Value {
    match HeldCarrier::of(ty) {
        HeldCarrier::RawI64 => {
            let word = word_of(b, value);
            let raw = unbox_int(b, word);
            b.word_value(raw)
        }
        HeldCarrier::RawF64 => {
            let word = word_of(b, value);
            b.bits_to_float(word)
        }
        HeldCarrier::RawBool => {
            let word = word_of(b, value);
            let payload = b.and(word, 1);
            let bit = b.low_bit(payload);
            b.bit_value(bit)
        }
        HeldCarrier::Boxed | HeldCarrier::Never => value,
    }
}

/// The truth of `value`, held as `carrier`. A raw bool, integer or float is
/// tested inline; a boxed word asks the runtime (the late lowering stage
/// expands the bool and integer words inline before codegen).
pub(crate) fn truth<B: CodegenBuilder>(
    b: &mut B,
    value: B::Value,
    carrier: HeldCarrier,
) -> B::Predicate {
    match carrier {
        HeldCarrier::RawBool => b.expect_bit(value),
        HeldCarrier::RawI64 => {
            let zero = b.constant(0);
            let word = b.expect_word(value);
            b.compare(WordComparison::NotEqual, word, zero)
        }
        HeldCarrier::RawF64 => {
            let zero = b.float_constant(0.0);
            b.float_compare(FloatComparison::UnorderedOrNotEqual, value, zero)
        }
        HeldCarrier::Boxed | HeldCarrier::Never => {
            let word = word_of(b, value);
            let truthy = runtime_word(b, RuntimeFn::IsTruthy, &[word]);
            let zero = b.constant(0);
            b.compare(WordComparison::NotEqual, truthy, zero)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! The scalar carriers on an interpreting builder. The oracle is the
    //! runtime's word layout written out here by hand (a quiet NaN, a 3-bit
    //! tag at bit 48, a 47-bit two's-complement integer payload), not the
    //! generators under test.

    use super::*;
    use crate::codegen_ssa::builder::{CheckedBinary, FloatBinary, IntBinary, IntComparison};
    use molt_codegen_abi::inline_refcount::RefcountOperation;
    use molt_codegen_abi::word::{BranchBuilder, WordBuilder};

    const QUIET_NAN: u64 = 0x7ff8 << 48;
    const INT_TAG: u64 = 1 << 48;
    const BOOL_TAG: u64 = 2 << 48;
    pub(crate) const NONE_WORD: u64 = QUIET_NAN | (3 << 48);
    /// A stand-in heap BigInt word: a pointer tag and an address.
    pub(crate) const HEAP_INT: u64 = QUIET_NAN | (4 << 48) | 0x1000;
    /// Where the stand-in entry storage lives.
    const SLOT_BASE: u64 = 0x10_0000;
    /// Where the stand-in byte constants live, 1 MiB apart.
    const BYTES_BASE: u64 = 0x100_0000_0000;
    /// The contents of entry storage no store has written.
    const UNINITIALIZED: u64 = 0xdead_beef;

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) enum Value {
        Bit(bool),
        Narrow(u32),
        Word(u64),
        Float(f64),
        Address(u64),
    }

    /// The edge a block ends with: each target and the values it passes.
    #[derive(Clone, Debug, PartialEq)]
    pub(in crate::codegen_ssa) enum Edge {
        Jump(u32, Vec<Value>),
        Branch(bool, (u32, Vec<Value>), (u32, Vec<Value>)),
        Switch(u64, Vec<(i64, u32, Vec<Value>)>, (u32, Vec<Value>)),
        Unreachable,
    }

    /// Runs each instruction as it is built; `choose` runs the taken arm only.
    /// A block ends in a recorded edge; nothing follows it.
    #[derive(Default)]
    pub(crate) struct Interpreter {
        pub(crate) runtime: RuntimeModel,
        pub(in crate::codegen_ssa) runtime_calls: Vec<RuntimeFn>,
        /// Each block's parameter kinds, by block number.
        pub(in crate::codegen_ssa) params: Vec<Vec<MachineKind>>,
        pub(in crate::codegen_ssa) edges: Vec<Edge>,
        /// The block being built.
        pub(in crate::codegen_ssa) current: u32,
        /// Entry storage, one word per slot.
        pub(in crate::codegen_ssa) slots: Vec<u64>,
        /// Each reference-count transition applied, in order.
        pub(in crate::codegen_ssa) refcounts: Vec<(RefcountOperation, u64)>,
        /// The byte constants built so far, by number.
        bytes: Vec<Vec<u8>>,
    }

    impl Interpreter {
        pub(in crate::codegen_ssa) fn block(&mut self) -> u32 {
            self.params.push(Vec::new());
            (self.params.len() - 1) as u32
        }

        fn check_arguments(&self, target: u32, args: &[Value]) {
            let kinds: Vec<MachineKind> = args.iter().map(|&arg| self.kind(arg)).collect();
            assert_eq!(
                kinds, self.params[target as usize],
                "an edge passes one argument of each parameter's kind"
            );
        }
    }

    impl WordBuilder for Interpreter {
        type Word = u64;
        type Predicate = bool;
        fn constant(&mut self, value: u64) -> u64 {
            value
        }
        fn and(&mut self, value: u64, mask: u64) -> u64 {
            value & mask
        }
        fn or(&mut self, value: u64, bits: u64) -> u64 {
            value | bits
        }
        fn shift_left(&mut self, value: u64, amount: u32) -> u64 {
            value << amount
        }
        fn shift_right_arithmetic(&mut self, value: u64, amount: u32) -> u64 {
            ((value as i64) >> amount) as u64
        }
        fn add_immediate(&mut self, value: u64, delta: i64) -> u64 {
            value.wrapping_add(delta as u64)
        }
        fn compare(&mut self, comparison: WordComparison, left: u64, right: u64) -> bool {
            match comparison {
                WordComparison::Equal => left == right,
                WordComparison::NotEqual => left != right,
                WordComparison::UnsignedLessThan => left < right,
            }
        }
        fn both(&mut self, left: bool, right: bool) -> bool {
            left && right
        }
    }

    impl BranchBuilder for Interpreter {
        fn choose(
            &mut self,
            condition: bool,
            yes: impl FnOnce(&mut Self) -> u64,
            no: impl FnOnce(&mut Self) -> u64,
        ) -> u64 {
            if condition { yes(self) } else { no(self) }
        }
    }

    impl CodegenBuilder for Interpreter {
        type Value = Value;
        type Block = u32;
        type Slot = usize;
        type Callee = RuntimeFn;
        type RefcountSite = RefcountOperation;
        fn create_block(&mut self, _name: &str) -> u32 {
            self.block()
        }
        fn set_name(&mut self, _value: Value, _name: &str) {}
        fn switch_to_block(&mut self, block: u32) {
            self.current = block;
        }
        fn entry_slot(&mut self, name: &str) -> usize {
            self.entry_slot_array(1, name)
        }
        fn entry_slot_array(&mut self, len: u32, _name: &str) -> usize {
            let first = self.slots.len();
            // Uninitialized entry storage reads as a word no producer makes.
            self.slots
                .extend(std::iter::repeat_n(UNINITIALIZED, len as usize));
            first
        }
        fn load_slot(&mut self, slot: usize) -> u64 {
            self.slots[slot]
        }
        fn store_slot(&mut self, slot: usize, word: u64) {
            self.slots[slot] = word;
        }
        fn store_slot_at(&mut self, slot: usize, index: u32, word: u64) {
            self.slots[slot + index as usize] = word;
        }
        fn slot_pointer(&mut self, slot: usize) -> Value {
            Value::Address(SLOT_BASE + 8 * slot as u64)
        }
        fn bit_constant(&mut self, value: bool) -> bool {
            value
        }
        fn bytes_constant(&mut self, bytes: &[u8], _prefix: &str, _suffix: &str) -> Value {
            self.bytes.push(bytes.to_vec());
            Value::Address(BYTES_BASE + ((self.bytes.len() as u64 - 1) << 20))
        }
        fn program_slot(&mut self, kind: molt_ir::code_slots::ProgramSlotKind, owner: &str) -> u64 {
            program_slot_word(&molt_ir::code_slots::program_slot_symbol(kind, owner))
        }
        fn refcount_site(&mut self, operation: RefcountOperation) -> RefcountOperation {
            operation
        }
        fn refcount(&mut self, site: RefcountOperation, word: u64) {
            self.refcounts.push((site, word));
        }
        fn runtime_callee(&mut self, function: RuntimeFn) -> RuntimeFn {
            function
        }
        fn call(&mut self, function: RuntimeFn, args: &[Value]) -> Option<Value> {
            let args: Vec<u64> = args.iter().map(|&arg| word_of(self, arg)).collect();
            let result = self.run_runtime(function, &args)?;
            Some(match function.result_type() {
                Some(molt_ir::runtime_fn_generated::RuntimeMachineType::I32) => {
                    Value::Narrow(result as u32)
                }
                _ => Value::Word(result),
            })
        }
        fn placeholder(&mut self, kind: MachineKind) -> Value {
            match kind {
                MachineKind::Bit => Value::Bit(false),
                MachineKind::NarrowInt => Value::Narrow(0),
                MachineKind::Word => Value::Word(0),
                MachineKind::Float => Value::Float(0.0),
                MachineKind::Address => Value::Address(0),
            }
        }
        fn append_block_param(&mut self, block: u32, kind: MachineKind) -> Value {
            self.params[block as usize].push(kind);
            self.placeholder(kind)
        }
        fn jump(&mut self, target: u32, args: &[Value]) {
            self.check_arguments(target, args);
            self.edges.push(Edge::Jump(target, args.to_vec()));
        }
        fn branch_if(&mut self, condition: bool, yes: (u32, &[Value]), no: (u32, &[Value])) {
            self.check_arguments(yes.0, yes.1);
            self.check_arguments(no.0, no.1);
            self.edges.push(Edge::Branch(
                condition,
                (yes.0, yes.1.to_vec()),
                (no.0, no.1.to_vec()),
            ));
        }
        fn switch(
            &mut self,
            selector: u64,
            cases: &[(i64, u32, Vec<Value>)],
            default: (u32, &[Value]),
        ) {
            for (_, target, args) in cases {
                self.check_arguments(*target, args);
            }
            self.check_arguments(default.0, default.1);
            self.edges.push(Edge::Switch(
                selector,
                cases.to_vec(),
                (default.0, default.1.to_vec()),
            ));
        }
        fn unreachable(&mut self) {
            self.edges.push(Edge::Unreachable);
        }
        fn kind(&self, value: Value) -> MachineKind {
            match value {
                Value::Bit(_) => MachineKind::Bit,
                Value::Narrow(_) => MachineKind::NarrowInt,
                Value::Word(_) => MachineKind::Word,
                Value::Float(_) => MachineKind::Float,
                Value::Address(_) => MachineKind::Address,
            }
        }
        fn word_value(&self, word: u64) -> Value {
            Value::Word(word)
        }
        fn bit_value(&self, bit: bool) -> Value {
            Value::Bit(bit)
        }
        fn expect_word(&self, value: Value) -> u64 {
            match value {
                Value::Word(word) => word,
                other => panic!("{other:?} is not a word"),
            }
        }
        fn expect_bit(&self, value: Value) -> bool {
            match value {
                Value::Bit(bit) => bit,
                other => panic!("{other:?} is not a bit"),
            }
        }
        fn zero_extend(&mut self, value: Value) -> u64 {
            match value {
                Value::Bit(bit) => u64::from(bit),
                Value::Narrow(narrow) => u64::from(narrow),
                other => panic!("{other:?} is not narrower than a word"),
            }
        }
        fn low_bit(&mut self, word: u64) -> bool {
            word & 1 == 1
        }
        fn float_to_bits(&mut self, value: Value) -> u64 {
            match value {
                Value::Float(float) => float.to_bits(),
                other => panic!("{other:?} is not a float"),
            }
        }
        fn bits_to_float(&mut self, word: u64) -> Value {
            Value::Float(f64::from_bits(word))
        }
        fn address_to_word(&mut self, value: Value) -> u64 {
            match value {
                Value::Address(address) => address,
                other => panic!("{other:?} is not an address"),
            }
        }
        fn float_constant(&mut self, value: f64) -> Value {
            Value::Float(value)
        }
        fn float_compare(
            &mut self,
            comparison: FloatComparison,
            left: Value,
            right: Value,
        ) -> bool {
            let (Value::Float(left), Value::Float(right)) = (left, right) else {
                panic!("a float comparison takes floats");
            };
            match comparison {
                FloatComparison::OrderedEqual => left == right,
                // `!=` on IEEE floats is true for unequal or unordered operands.
                FloatComparison::UnorderedOrNotEqual => left != right,
                FloatComparison::OrderedLess => left < right,
                FloatComparison::OrderedLessOrEqual => left <= right,
                FloatComparison::OrderedGreater => left > right,
                FloatComparison::OrderedGreaterOrEqual => left >= right,
            }
        }
        fn int_binary(&mut self, op: IntBinary, left: u64, right: u64, _exact: bool) -> u64 {
            let (l, r) = (left as i64, right as i64);
            (match op {
                IntBinary::Add => l.wrapping_add(r),
                IntBinary::Sub => l.wrapping_sub(r),
                IntBinary::Mul => l.wrapping_mul(r),
                IntBinary::And => l & r,
                IntBinary::Or => l | r,
                IntBinary::Xor => l ^ r,
                IntBinary::ShiftLeft => l << u32::try_from(r).expect("the walker proved the count"),
                IntBinary::ShiftRightArithmetic => {
                    l >> u32::try_from(r).expect("the walker proved the count")
                }
                IntBinary::SignedQuotient => l
                    .checked_div(r)
                    .expect("the walker proved the divisor nonzero and the quotient exact"),
                IntBinary::SignedRemainder => l
                    .checked_rem(r)
                    .expect("the walker proved the divisor nonzero and the quotient exact"),
            }) as u64
        }
        fn int_checked(&mut self, op: CheckedBinary, left: u64, right: u64) -> (u64, bool) {
            let (l, r) = (left as i64, right as i64);
            let (value, overflow) = match op {
                CheckedBinary::Add => l.overflowing_add(r),
                CheckedBinary::Mul => l.overflowing_mul(r),
            };
            (value as u64, overflow)
        }
        fn int_to_float(&mut self, word: u64) -> Value {
            Value::Float(word as i64 as f64)
        }
        fn select(&mut self, condition: bool, yes: Value, no: Value) -> Value {
            assert_eq!(
                std::mem::discriminant(&yes),
                std::mem::discriminant(&no),
                "a select takes two values of one kind"
            );
            if condition { yes } else { no }
        }
        fn bit_xor(&mut self, left: bool, right: bool) -> bool {
            left ^ right
        }
        fn int_compare(&mut self, comparison: IntComparison, left: u64, right: u64) -> bool {
            let (l, r) = (left as i64, right as i64);
            match comparison {
                IntComparison::Equal => l == r,
                IntComparison::NotEqual => l != r,
                IntComparison::SignedLess => l < r,
                IntComparison::SignedLessOrEqual => l <= r,
                IntComparison::SignedGreater => l > r,
                IntComparison::SignedGreaterOrEqual => l >= r,
            }
        }
        fn int_negate(&mut self, word: u64) -> u64 {
            (word as i64).wrapping_neg() as u64
        }
        fn int_invert(&mut self, word: u64) -> u64 {
            !word
        }
        fn bit_not(&mut self, bit: bool) -> bool {
            !bit
        }
        fn float_binary(
            &mut self,
            op: FloatBinary,
            left: Value,
            right: Value,
            _fast: bool,
        ) -> Value {
            let (Value::Float(l), Value::Float(r)) = (left, right) else {
                panic!("a float instruction takes floats");
            };
            Value::Float(match op {
                FloatBinary::Add => l + r,
                FloatBinary::Sub => l - r,
                FloatBinary::Mul => l * r,
                FloatBinary::Divide => l / r,
                // Rust's `%` on floats is C's `fmod`.
                FloatBinary::Remainder => l % r,
            })
        }
        fn float_negate(&mut self, value: Value) -> Value {
            let Value::Float(float) = value else {
                panic!("a float negation takes a float");
            };
            Value::Float(-float)
        }
    }

    impl Interpreter {
        fn run_runtime(&mut self, function: RuntimeFn, args: &[u64]) -> Option<u64> {
            self.runtime_calls.push(function);
            let mut runtime = std::mem::take(&mut self.runtime);
            let result = runtime.run(function, args, self);
            self.runtime = runtime;
            result
        }
    }

    impl Memory for Interpreter {
        fn bytes(&self, address: u64, len: usize) -> Vec<u8> {
            let constant = &self.bytes[((address - BYTES_BASE) >> 20) as usize];
            let offset = ((address - BYTES_BASE) & ((1 << 20) - 1)) as usize;
            constant[offset..offset + len].to_vec()
        }
        fn word(&self, address: u64) -> u64 {
            self.slots[((address - SLOT_BASE) / 8) as usize]
        }
        fn store(&mut self, address: u64, word: u64) {
            self.slots[((address - SLOT_BASE) / 8) as usize] = word;
        }
    }

    /// The memory a runtime entry reads its pointer arguments through.
    pub(crate) trait Memory {
        fn bytes(&self, address: u64, len: usize) -> Vec<u8>;
        fn word(&self, address: u64) -> u64;
        fn store(&mut self, address: u64, word: u64);
    }

    /// The word the model binds to the program slot `symbol` names: a raw
    /// index derived from that identity, so a wrong slot reads a wrong word.
    pub(crate) fn program_slot_word(symbol: &str) -> u64 {
        fnv(symbol.as_bytes()) & 0xffff
    }

    /// The object word the model's constructor returns for a literal: a
    /// pointer-tagged word derived from the constructor and every byte, so
    /// a literal that reached the runtime with other bytes reads differently.
    pub(crate) fn literal_word(function: RuntimeFn, bytes: &[u8]) -> u64 {
        let mut key = function.symbol().as_bytes().to_vec();
        key.extend_from_slice(bytes);
        QUIET_NAN | (4 << 48) | ((fnv(&key) & 0xffff_fff0) << 3) | 0x8
    }

    fn fnv(bytes: &[u8]) -> u64 {
        bytes
            .iter()
            .fold(0xcbf2_9ce4_8422_2325, |hash: u64, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            })
    }

    /// The runtime entries the walker calls, as the model tests define them.
    /// The interpreting builder and the machine-code stubs both run this one
    /// model, so the two machines see the same runtime.
    #[derive(Default)]
    pub(crate) struct RuntimeModel {
        /// The raw integer the stand-in heap BigInt holds.
        heap_int: Option<i64>,
        /// When set, `molt_int_from_i64` fails as an allocation failure does:
        /// it returns None with MemoryError pending.
        pub(crate) fail_mint: bool,
        /// The words each `molt_call_inputs_release` read from its spill.
        pub(crate) released: Vec<Vec<u64>>,
    }

    impl RuntimeModel {
        pub(crate) fn run(
            &mut self,
            function: RuntimeFn,
            args: &[u64],
            memory: &mut dyn Memory,
        ) -> Option<u64> {
            Some(match function {
                // Releases the adopted inputs; returns nothing.
                RuntimeFn::CallInputsRelease => {
                    let words = (0..args[2]).map(|index| memory.word(args[1] + 8 * index));
                    self.released.push(words.collect());
                    return None;
                }
                // Writes the new object through the out-pointer; status 0.
                RuntimeFn::StringFromBytes | RuntimeFn::BytesFromBytes => {
                    let bytes = memory.bytes(args[0], args[1] as usize);
                    memory.store(args[2], literal_word(function, &bytes));
                    0
                }
                RuntimeFn::BigintFromBytes => {
                    literal_word(function, &memory.bytes(args[0], args[1] as usize))
                }
                RuntimeFn::IntFromI64 if self.fail_mint => NONE_WORD,
                RuntimeFn::IntFromI64 => {
                    self.heap_int = Some(args[0] as i64);
                    HEAP_INT
                }
                RuntimeFn::IntAsI64 => {
                    if args[0] == HEAP_INT {
                        self.heap_int.expect("a heap integer was boxed") as u64
                    } else {
                        assert_eq!(args[0] >> 47, (QUIET_NAN | INT_TAG) >> 47, "an inline int");
                        // Sign-extend the 47-bit payload.
                        (((args[0] << 17) as i64) >> 17) as u64
                    }
                }
                RuntimeFn::IsTruthy => u64::from(args[0] != NONE_WORD),
                // A division by zero raises; its word is dead.
                RuntimeFn::Div | RuntimeFn::Floordiv | RuntimeFn::Mod => NONE_WORD,
                other => panic!("the walker never calls {other:?}"),
            })
        }
    }

    const MAX_INLINE: i64 = (1 << 46) - 1;
    const MIN_INLINE: i64 = -(1 << 46);

    use crate::codegen_ssa::differential::{both, kernel};

    #[test]
    fn an_integer_boxes_inline_exactly_when_it_fits_47_bits_and_round_trips() {
        // Box a raw int, then decode it through both decoders.
        let round_trip = kernel!(; |b, inputs| -> () {
            let boxed = box_value(b, inputs[0], &TirType::I64);
            let boxed_value = b.word_value(boxed);
            let back = unbox_value(b, boxed_value, &TirType::I64);
            let param = unbox_to_param_word(b, boxed, &TirType::I64);
            let param = b.word_value(param);
            ((), vec![boxed_value, back, param])
        });
        for (raw, inline) in [
            (0, true),
            (-1, true),
            (MAX_INLINE, true),
            (MIN_INLINE, true),
            (MAX_INLINE + 1, false),
            (MIN_INLINE - 1, false),
            (i64::MAX, false),
            (i64::MIN, false),
        ] {
            let run = both(&round_trip, &[Value::Word(raw as u64)]);
            let decodes = [RuntimeFn::IntAsI64, RuntimeFn::IntAsI64];
            if inline {
                let payload = (raw as u64) & ((1 << 47) - 1);
                assert_eq!(
                    run.results[0],
                    Value::Word(QUIET_NAN | INT_TAG | payload),
                    "{raw}"
                );
                assert_eq!(
                    run.runtime_calls, decodes,
                    "{raw} boxes without the runtime"
                );
            } else {
                assert_eq!(run.results[0], Value::Word(HEAP_INT), "{raw}");
                assert_eq!(run.runtime_calls[0], RuntimeFn::IntFromI64, "{raw}");
                assert_eq!(run.runtime_calls[1..], decodes, "{raw}");
            }
            assert_eq!(run.results[1..], [Value::Word(raw as u64); 2], "{raw}");
        }
    }

    #[test]
    fn a_bool_and_a_float_box_without_the_runtime_and_round_trip() {
        for ty in [TirType::Bool, TirType::F64] {
            let round_trip = kernel!(ty: TirType = ty.clone(); |b, inputs| -> () {
                let boxed = box_value(b, inputs[0], ty);
                let boxed_value = b.word_value(boxed);
                let back = unbox_value(b, boxed_value, ty);
                let param = unbox_to_param_word(b, boxed, ty);
                let param = b.word_value(param);
                ((), vec![boxed_value, back, param])
            });
            let inputs: Vec<Value> = if ty == TirType::Bool {
                vec![Value::Bit(false), Value::Bit(true)]
            } else {
                [0.0, -0.0, 1.5, f64::INFINITY, f64::NAN]
                    .map(Value::Float)
                    .to_vec()
            };
            for input in inputs {
                let run = both(&round_trip, &[input]);
                let (boxed, param) = match input {
                    Value::Bit(bit) => (QUIET_NAN | BOOL_TAG | u64::from(bit), u64::from(bit)),
                    Value::Float(float) => (float.to_bits(), float.to_bits()),
                    other => unreachable!("{other:?}"),
                };
                assert_eq!(run.results[0], Value::Word(boxed), "{input:?}");
                match (run.results[1], input) {
                    (Value::Float(back), Value::Float(float)) => {
                        assert_eq!(back.to_bits(), float.to_bits());
                    }
                    (back, input) => assert_eq!(back, input),
                }
                assert_eq!(run.results[2], Value::Word(param), "{input:?}");
                assert!(run.runtime_calls.is_empty());
            }
        }
    }

    #[test]
    fn a_boxed_carrier_passes_through_unchanged() {
        for ty in [
            TirType::DynBox,
            TirType::None,
            TirType::Str,
            TirType::BigInt,
        ] {
            let pass = kernel!(ty: TirType = ty.clone(); |b, inputs| -> () {
                let boxed = box_value(b, inputs[0], ty);
                let boxed = b.word_value(boxed);
                let unboxed = unbox_value(b, inputs[0], ty);
                ((), vec![boxed, unboxed])
            });
            let run = both(&pass, &[Value::Word(NONE_WORD)]);
            assert_eq!(run.results, [Value::Word(NONE_WORD); 2], "{ty:?}");
            assert!(run.runtime_calls.is_empty());
        }
        let words = kernel!(; |b, inputs| -> () {
            let words = inputs.iter().map(|&input| {
                let word = word_of(b, input);
                b.word_value(word)
            }).collect();
            ((), words)
        });
        let run = both(&words, &[Value::Address(0x2000), Value::Narrow(7)]);
        assert_eq!(run.results, [Value::Word(0x2000), Value::Word(7)]);
        assert!(run.runtime_calls.is_empty());
    }

    #[test]
    fn truth_follows_python_for_each_carrier() {
        for (value, carrier, expected) in [
            (Value::Bit(true), HeldCarrier::RawBool, true),
            (Value::Bit(false), HeldCarrier::RawBool, false),
            (Value::Word(0), HeldCarrier::RawI64, false),
            (Value::Word(u64::MAX), HeldCarrier::RawI64, true),
            (Value::Float(0.0), HeldCarrier::RawF64, false),
            (Value::Float(-0.0), HeldCarrier::RawF64, false),
            (Value::Float(0.5), HeldCarrier::RawF64, true),
            // bool(float("nan")) is True.
            (Value::Float(f64::NAN), HeldCarrier::RawF64, true),
            (Value::Word(NONE_WORD), HeldCarrier::Boxed, false),
            (Value::Word(HEAP_INT), HeldCarrier::Boxed, true),
        ] {
            let test = kernel!(carrier: HeldCarrier = carrier; |b, inputs| -> () {
                let bit = truth(b, inputs[0], *carrier);
                ((), vec![b.bit_value(bit)])
            });
            let run = both(&test, &[value]);
            assert_eq!(run.results, [Value::Bit(expected)], "{value:?}");
            let runtime: &[RuntimeFn] = if carrier == HeldCarrier::Boxed {
                &[RuntimeFn::IsTruthy]
            } else {
                &[]
            };
            assert_eq!(run.runtime_calls, runtime, "raw carriers test inline");
        }
    }
}
