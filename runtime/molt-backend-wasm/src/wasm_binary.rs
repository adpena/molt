mod code_remap;
mod emit;
mod import_strip;
mod leb;
mod reloc;
mod types;
mod validation;

pub(crate) use emit::{
    const_expr_i32_const_padded, emit_call, emit_call_indirect, emit_i32_const, emit_ref_func,
    emit_return_call, emit_simple_call, emit_table_index_i64, encode_u32_leb128_padded,
};
pub(crate) use import_strip::strip_unused_imports;
pub(crate) use reloc::add_reloc_sections;
pub(crate) use validation::validate_wasm_sections;
