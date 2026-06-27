mod emit;
mod reloc;
mod rewrite;

pub(crate) use emit::{
    const_expr_i32_const_padded, emit_call, emit_call_indirect, emit_i32_const, emit_ref_func,
    emit_return_call, emit_simple_call, emit_table_index_i64, encode_u32_leb128_padded,
};
pub(crate) use reloc::add_reloc_sections;
pub(crate) use rewrite::{strip_unused_imports, validate_wasm_sections};
