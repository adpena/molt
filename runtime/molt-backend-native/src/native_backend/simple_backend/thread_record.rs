//! Cranelift spelling of `molt_codegen_abi::thread_record`.

use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir::{InstBuilder, MemFlagsData, Value, types};
use cranelift_frontend::FunctionBuilder;
use molt_codegen_abi::thread_record::{
    ThreadRecordBuilder, ThreadRecordWord, generate_thread_record, generate_thread_record_word,
};

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

/// `word` of the thread record `context` lends; dead when nothing uses it.
pub(crate) fn thread_record_word(
    builder: &mut FunctionBuilder<'_>,
    context: Value,
    word: ThreadRecordWord,
) -> Value {
    generate_thread_record_word(
        &mut ClifThreadRecord {
            builder: builder.cursor(),
            context,
        },
        word,
    )
}

// The recursion charge of a compiled call, spelled once for every Cranelift
// call site (`molt_codegen_abi::thread_record::generate_recursion_*`). A site
// branches on `emit_recursion_admits`, counts the frame on the admitted arm,
// and releases it after the call returns.

/// Whether the recursion limit admits one more Python frame on `thread`.
pub(crate) fn emit_recursion_admits(
    builder: &mut super::FunctionBuilder<'_>,
    thread: Value,
) -> Value {
    molt_codegen_abi::thread_record::generate_recursion_admits(
        &mut super::NativeWords { builder },
        thread,
    )
}

/// Count the frame `emit_recursion_admits` admitted.
pub(crate) fn emit_recursion_count(builder: &mut super::FunctionBuilder<'_>, thread: Value) {
    molt_codegen_abi::thread_record::generate_recursion_count(
        &mut super::NativeWords { builder },
        thread,
    );
}

/// Release the frame `emit_recursion_count` counted.
pub(crate) fn emit_recursion_release(builder: &mut super::FunctionBuilder<'_>, thread: Value) {
    molt_codegen_abi::thread_record::generate_recursion_release(
        &mut super::NativeWords { builder },
        thread,
    );
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
