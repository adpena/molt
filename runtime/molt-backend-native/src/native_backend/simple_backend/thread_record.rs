//! Cranelift spelling of `molt_codegen_abi::thread_record`.

use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir::{InstBuilder, MemFlagsData, Value, types};
use cranelift_frontend::FunctionBuilder;
use molt_codegen_abi::thread_record::{ThreadRecordBuilder, generate_thread_record};

struct ClifThreadRecord<'a> {
    builder: FuncCursor<'a>,
    context: Value,
}

impl ThreadRecordBuilder for ClifThreadRecord<'_> {
    type Word = Value;

    fn attached_context(&mut self) -> Value {
        self.context
    }

    fn load_trusted_word(&mut self, address: Value, offset: i32) -> Value {
        self.builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), address, offset)
    }
}

/// Insert the immutable record read before the entry's first instruction
/// without moving the function builder off its current TIR block.
pub(super) fn entry_thread_record(builder: &mut FunctionBuilder<'_>) -> Value {
    let current = builder
        .current_block()
        .expect("a walker has a current block");
    let entry = builder.func.layout.entry_block().unwrap_or(current);
    let context = builder.func.dfg.block_params(entry)[0];
    let cursor = builder.cursor().at_first_insertion_point(entry);
    generate_thread_record(&mut ClifThreadRecord {
        builder: cursor,
        context,
    })
}
