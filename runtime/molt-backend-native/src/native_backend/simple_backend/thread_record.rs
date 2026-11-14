//! Cranelift spelling of `molt_codegen_abi::thread_record`.

use cranelift_codegen::ir::{InstBuilder, MemFlagsData, Value, types};
use cranelift_frontend::FunctionBuilder;
use molt_codegen_abi::thread_record::{
    ThreadRecordBuilder, ThreadRecordWord, generate_thread_record_word,
};

struct ClifThreadRecord<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    context: Value,
}

impl ThreadRecordBuilder for ClifThreadRecord<'_, '_> {
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
    generate_thread_record_word(&mut ClifThreadRecord { builder, context }, word)
}
