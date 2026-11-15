//! Cranelift's spelling of the shared proven field read and store
//! (`molt_codegen_abi::field_slot`): `obj.x` and `obj.x = v` at a fixed offset
//! compile inline, over the frame builder's control and word primitives.

use super::*;
use molt_codegen_abi::field_slot::{generate_field_slot_get, generate_field_slot_set};
use molt_codegen_abi::list_item::ListLocking;

fn field_slot_builder<'a, 'b>(
    builder: &'a mut FunctionBuilder<'b>,
    sites: NativeFrameSites,
) -> NativeFrameBuilder<'a, 'b> {
    NativeFrameBuilder {
        words: NativeWords { builder },
        sites,
        slow_blocks: Vec::new(),
        sealed: Vec::new(),
    }
}

/// `object.<field at offset>` as an owned word, whose miss calls
/// `cold(builder, pointer)`. The generator's blocks join the caller's sealed
/// set.
pub(crate) fn emit_field_slot_get(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    sites: NativeFrameSites,
    object: Value,
    offset: i32,
    cold: impl FnOnce(&mut FunctionBuilder<'_>, Value) -> Value,
) -> Value {
    let mut b = field_slot_builder(builder, sites);
    let value =
        generate_field_slot_get(&mut b, object, offset, ListLocking::BUILD, |b, pointer| {
            cold(&mut *b.words.builder, pointer)
        });
    sealed_blocks.extend(b.sealed);
    value
}

/// `object.<field at offset> = value`, whose miss calls
/// `cold(builder, pointer)`.
pub(crate) fn emit_field_slot_set(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    sites: NativeFrameSites,
    object: Value,
    offset: i32,
    value: Value,
    cold: impl FnOnce(&mut FunctionBuilder<'_>, Value),
) {
    let mut b = field_slot_builder(builder, sites);
    generate_field_slot_set(
        &mut b,
        object,
        offset,
        value,
        ListLocking::BUILD,
        |b, pointer| cold(&mut *b.words.builder, pointer),
    );
    sealed_blocks.extend(b.sealed);
}
