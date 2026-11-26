//! Checked packed Buffer2D access for Cranelift and LLVM, including Wasm.

use molt_codegen_abi::boxed_word::{
    BoxTag, generate_box_inline_int, generate_has_tag, generate_inline_int_fits,
    generate_unbox_inline_int,
};
use molt_codegen_abi::word::WordComparison;
use molt_codegen_abi::{
    BUFFER2D_COLS_OFFSET, BUFFER2D_ROWS_OFFSET, BUFFER2D_STORAGE_DATA_OFFSET,
    BUFFER2D_STORAGE_PACKED, BUFFER2D_STORAGE_TAG_OFFSET, HEADER_TYPE_ID_OFFSET,
    MOLT_REFCOUNT_ATOMIC, POINTER_MASK, THREAD_RECORD_EXCEPTION_PENDING_OFFSET, TYPE_ID_BUFFER2D,
};
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, IntBinary, MachineKind};
use super::custody::Operands;
use super::function::FunctionCx;
use super::value_table::HeldCarrier;
use crate::tir::ops::TirOp;
use crate::tir::values::ValueId;

fn require<B: CodegenBuilder>(b: &mut B, condition: B::Predicate, slow: B::Block, name: &str) {
    let next = b.create_block(name);
    b.branch_if(condition, (next, &[]), (slow, &[]));
    b.switch_to_block(next);
}

/// Consume an already raw i64 directly. For a boxed value, a successful
/// tag test proves the integer conversion has no Python effects.
fn exact_i64<B: CodegenBuilder>(
    b: &mut B,
    operands: &Operands<'_, B>,
    value: ValueId,
    slow: B::Block,
) -> B::Word {
    let value_word = operands.resolve(b, value);
    let word = b.expect_word(value_word);
    if operands.held(value) == HeldCarrier::RawI64 {
        return word;
    }
    let inline = generate_has_tag(b, word, BoxTag::Int);
    require(b, inline, slow, "buffer_inline_integer");
    generate_unbox_inline_int(b, word)
}

/// Normalize a negative index once. The unsigned comparison then checks
/// both ends, including i64::MIN, because the extent is at most i64::MAX.
fn normalize<B: CodegenBuilder>(b: &mut B, index: B::Word, extent: B::Word) -> B::Word {
    let sign = b.shift_right_arithmetic(index, 63);
    let correction = b.int_binary(IntBinary::And, sign, extent, false);
    b.int_binary(IntBinary::Add, index, correction, false)
}

fn can_access<B: CodegenBuilder>(operands: &Operands<'_, B>, op: &TirOp) -> bool {
    !MOLT_REFCOUNT_ATOMIC
        && operands.held(op.operands[0]) == HeldCarrier::Boxed
        && op.operands[1..]
            .iter()
            .all(|&id| matches!(operands.held(id), HeldCarrier::Boxed | HeldCarrier::RawI64))
}

fn checked_buffer<B: CodegenBuilder>(
    b: &mut B,
    operands: &Operands<'_, B>,
    owner: ValueId,
    cold: B::Block,
) -> B::Value {
    let object = operands.resolve(b, owner);
    let object = b.expect_word(object);
    let is_pointer = generate_has_tag(b, object, BoxTag::Ptr);
    require(b, is_pointer, cold, "buffer_pointer");
    let pointer = b.and(object, POINTER_MASK);
    let kind = b.load_u32(pointer, HEADER_TYPE_ID_OFFSET);
    let buffer_kind = b.constant(u64::from(TYPE_ID_BUFFER2D));
    let exact = b.compare(WordComparison::Equal, kind, buffer_kind);
    require(b, exact, cold, "buffer_exact");

    let thread = b.thread_record_word();
    let hint = b.load_byte(thread, THREAD_RECORD_EXCEPTION_PENDING_OFFSET);
    let clear = b.is_zero(hint);
    require(b, clear, cold, "buffer_exception_clear");
    b.word_to_address(pointer)
}

/// The slot address exists only within this access. Promotion and reentry
/// cannot intervene between checking storage and the load or store.
fn checked_slot<B: CodegenBuilder>(
    b: &mut B,
    operands: &Operands<'_, B>,
    buffer: B::Value,
    row: ValueId,
    col: ValueId,
    cold: B::Block,
) -> B::Value {
    let tag = b.load_word(buffer, BUFFER2D_STORAGE_TAG_OFFSET);
    let packed_tag = b.constant(BUFFER2D_STORAGE_PACKED);
    let packed = b.compare(WordComparison::Equal, tag, packed_tag);
    require(b, packed, cold, "buffer_packed");

    let row = exact_i64(b, operands, row, cold);
    let col = exact_i64(b, operands, col, cold);
    let rows = b.load_word(buffer, BUFFER2D_ROWS_OFFSET);
    let cols = b.load_word(buffer, BUFFER2D_COLS_OFFSET);
    let row = normalize(b, row, rows);
    let col = normalize(b, col, cols);
    let row_in = b.compare(WordComparison::UnsignedLessThan, row, rows);
    let col_in = b.compare(WordComparison::UnsignedLessThan, col, cols);
    let in_bounds = b.both(row_in, col_in);
    require(b, in_bounds, cold, "buffer_in_bounds");

    // The constructor checks rows * cols and allocates that many i64s.
    // Together the two bounds establish that this address is in storage.
    let row_base = b.int_binary(IntBinary::Mul, row, cols, false);
    let index = b.int_binary(IntBinary::Add, row_base, col, false);
    let offset = b.shift_left(index, 3);
    let data = b.load_word(buffer, BUFFER2D_STORAGE_DATA_OFFSET);
    let address = b.int_binary(IntBinary::Add, data, offset, false);
    b.word_to_address(address)
}

fn load_packed_i64<B: CodegenBuilder>(
    b: &mut B,
    operands: &Operands<'_, B>,
    op: &TirOp,
    miss: B::Block,
) -> B::Word {
    let owner = checked_buffer(b, operands, op.operands[0], miss);
    let slot = checked_slot(b, operands, owner, op.operands[1], op.operands[2], miss);
    b.load_word(slot, 0)
}

fn commit_packed_store<B: CodegenBuilder>(
    b: &mut B,
    operands: &Operands<'_, B>,
    op: &TirOp,
    miss: B::Block,
) {
    let owner = checked_buffer(b, operands, op.operands[0], miss);
    let slot = checked_slot(b, operands, owner, op.operands[1], op.operands[2], miss);
    let value = exact_i64(b, operands, op.operands[3], miss);
    b.store_word(slot, 0, value);
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    pub(super) fn try_buffer_load(&mut self, b: &mut B, op: &TirOp) {
        let operands = self.operands();
        let zero = b.constant(0);
        let miss = b.bit_constant(false);
        let (word, success) = if can_access(&operands, op) {
            let cold = b.create_block("buffer_load_miss");
            let done = b.create_block("buffer_load_done");
            let word = b.append_block_param(done, MachineKind::Word);
            let success = b.append_block_param(done, MachineKind::Bit);
            let value = load_packed_i64(b, &operands, op, cold);
            let hit = b.bit_constant(true);
            b.jump(done, &[b.word_value(value), b.bit_value(hit)]);
            b.switch_to_block(cold);
            b.jump(done, &[b.word_value(zero), b.bit_value(miss)]);
            b.switch_to_block(done);
            (word, success)
        } else {
            (b.word_value(zero), b.bit_value(miss))
        };
        self.bind(op.results[0], word, HeldCarrier::RawI64);
        self.bind(op.results[1], success, HeldCarrier::RawBool);
    }

    pub(super) fn try_buffer_store(&mut self, b: &mut B, op: &TirOp) {
        let operands = self.operands();
        let miss_value = b.bit_constant(false);
        let success = if can_access(&operands, op) {
            let miss = b.create_block("buffer_store_miss");
            let done = b.create_block("buffer_store_done");
            let result = b.append_block_param(done, MachineKind::Bit);
            commit_packed_store(b, &operands, op, miss);
            let hit = b.bit_constant(true);
            b.jump(done, &[b.bit_value(hit)]);
            b.switch_to_block(miss);
            b.jump(done, &[b.bit_value(miss_value)]);
            b.switch_to_block(done);
            result
        } else {
            b.bit_value(miss_value)
        };
        self.bind(op.results[0], success, HeldCarrier::RawBool);
    }

    pub(super) fn try_buffer_branch(
        &mut self,
        b: &mut B,
        op: &TirOp,
        hit: B::Block,
        miss: B::Block,
        raw: Option<ValueId>,
    ) -> bool {
        let operands = self.operands();
        if !can_access(&operands, op) {
            return false;
        }
        if let Some(raw) = raw {
            let value = load_packed_i64(b, &operands, op, miss);
            self.bind(raw, b.word_value(value), HeldCarrier::RawI64);
        } else {
            commit_packed_store(b, &operands, op, miss);
        }
        b.jump(hit, &[]);
        true
    }

    pub(super) fn buffer_dimension_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        entry: RuntimeFn,
    ) -> bool {
        let dimension = match entry {
            RuntimeFn::Buffer2dRows => BUFFER2D_ROWS_OFFSET,
            RuntimeFn::Buffer2dCols => BUFFER2D_COLS_OFFSET,
            _ => return false,
        };
        let operands = self.operands();
        if !can_access(&operands, op) {
            return false;
        }
        let cold = b.create_block("buffer_access_cold");
        let done = b.create_block("buffer_access_done");
        let result = b.append_block_param(done, MachineKind::Word);

        let buffer = checked_buffer(b, &operands, op.operands[0], cold);
        let answer = b.load_word(buffer, dimension);
        let inline = generate_inline_int_fits(b, answer);
        require(b, inline, cold, "buffer_inline_result");
        let hot = generate_box_inline_int(b, answer);
        b.jump(done, &[b.word_value(hot)]);

        b.switch_to_block(cold);
        let mut custody = operands.begin(b, &op.operands, "buffer_access");
        let args: Vec<_> = op
            .operands
            .iter()
            .map(|&id| {
                let word = operands.operand(b, &mut custody, id);
                b.word_value(word)
            })
            .collect();
        let callee = b.runtime_callee(entry);
        let fallback = b
            .call_cold(callee, &args)
            .expect("Buffer2D entry returns a word");
        let fallback = operands.finish(
            b,
            custody,
            b.expect_word(fallback),
            "buffer_fallback",
            |_| {},
        );
        b.jump(done, &[b.word_value(fallback)]);
        b.switch_to_block(done);
        self.bind_owned(b, op, result);
        true
    }
}
