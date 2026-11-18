//! Cranelift's spelling of the shared list item generators
//! (`molt_codegen_abi::list_item`): `xs[i]` and `xs[i] = v` on an exact list
//! compile inline, over the frame builder's control and word primitives.

use super::*;
use molt_codegen_abi::list_item::{
    ItemContainers, ListLocking, generate_list_item_get, generate_list_item_set,
};

fn list_item_builder<'a, 'b>(
    builder: &'a mut FunctionBuilder<'b>,
    sites: NativeFrameSites,
) -> NativeFrameBuilder<'a, 'b> {
    NativeFrameBuilder {
        words: NativeWords { builder },
        sites,
        slow_blocks: Vec::new(),
        sealed: Vec::new(),
        tasks: None,
    }
}

/// `sequence[index]` as an owned word: the shared read of the `containers`
/// the site's proof admits, whose miss calls `cold`. The generator's blocks
/// join the caller's sealed set.
pub(crate) fn emit_list_item_get(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    sites: NativeFrameSites,
    sequence: Value,
    index: Value,
    containers: ItemContainers,
    cold: impl FnOnce(&mut FunctionBuilder<'_>) -> Value,
) -> Value {
    let mut b = list_item_builder(builder, sites);
    let item = generate_list_item_get(
        &mut b,
        sequence,
        index,
        containers,
        ListLocking::BUILD,
        |b| cold(&mut *b.words.builder),
    );
    sealed_blocks.extend(b.sealed);
    item
}

/// `list[index] = value`: the shared store, whose miss calls `cold`.
pub(crate) fn emit_list_item_set(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    sites: NativeFrameSites,
    list: Value,
    index: Value,
    value: Value,
    success: Value,
    cold: impl FnOnce(&mut FunctionBuilder<'_>) -> Value,
) -> Value {
    let mut b = list_item_builder(builder, sites);
    let stored = generate_list_item_set(
        &mut b,
        list,
        index,
        value,
        ListLocking::BUILD,
        success,
        |b| cold(&mut *b.words.builder),
    );
    sealed_blocks.extend(b.sealed);
    stored
}

/// The owned word in `owner`'s closure payload at `offset`: the shared
/// closure slot read (`molt_codegen_abi::closure_slot`), which never misses.
pub(crate) fn emit_closure_load(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    sites: NativeFrameSites,
    owner: Value,
    offset: Value,
) -> Value {
    let mut b = list_item_builder(builder, sites);
    let value = molt_codegen_abi::closure_slot::generate_closure_load(&mut b, owner, offset);
    sealed_blocks.extend(b.sealed);
    value
}

/// Store the borrowed `value` in `owner`'s closure payload at `offset`; the
/// miss calls `cold`, the runtime entry.
pub(crate) fn emit_closure_store(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    sites: NativeFrameSites,
    owner: Value,
    offset: Value,
    value: Value,
    cold: impl FnOnce(&mut FunctionBuilder<'_>),
) {
    let mut b = list_item_builder(builder, sites);
    molt_codegen_abi::closure_slot::generate_closure_store(
        &mut b,
        owner,
        offset,
        value,
        ListLocking::BUILD,
        |b| cold(&mut *b.words.builder),
    );
    sealed_blocks.extend(b.sealed);
}
