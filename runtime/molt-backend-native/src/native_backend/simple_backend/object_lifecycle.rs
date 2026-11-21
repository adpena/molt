//! Cranelift's spelling of the shared plain-instance allocation and terminal
//! free (`molt_codegen_abi::object_lifecycle`), inline over the frame
//! builder's control and word primitives, as `field_slot.rs` is.

use super::*;
use molt_codegen_abi::inline_refcount::RefcountStorage;
use molt_codegen_abi::list_item::ListLocking;
use molt_codegen_abi::object_lifecycle::{
    LifecycleWords, PlanSource, generate_builtin_type, generate_object_alloc,
    generate_release_fields, generate_release_terminal, generate_type_of,
};

fn lifecycle_builder<'a, 'b>(
    builder: &'a mut FunctionBuilder<'b>,
    sites: NativeFrameSites,
) -> NativeFrameBuilder<'a, 'b> {
    NativeFrameBuilder {
        words: NativeWords { builder },
        sites,
        slow_blocks: Vec::new(),
        tasks: None,
    }
}

/// A new instance of the class `class` from its lifecycle plan, owned.
/// `allocate` is `molt_heap_alloc`; `cold(builder)` is the runtime
/// constructor.
pub(crate) fn emit_object_alloc(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    class: Value,
    plan: PlanSource,
    words: LifecycleWords<Value>,
    allocate: FuncRef,
    cold: impl FnOnce(&mut FunctionBuilder<'_>) -> Value,
) -> Value {
    let mut b = lifecycle_builder(builder, sites);
    generate_object_alloc(
        &mut b,
        class,
        plan,
        words,
        ListLocking::BUILD,
        |b, bytes| {
            let call = b.words.builder.ins().call(allocate, &[bytes]);
            b.words.builder.inst_results(call)[0]
        },
        |b| cold(&mut *b.words.builder),
    )
}

/// `type(value)`, owned; `cold(builder)` is `molt_type_of(value)`.
pub(crate) fn emit_type_of(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    value: Value,
    cold: impl FnOnce(&mut FunctionBuilder<'_>) -> Value,
) -> Value {
    let mut b = lifecycle_builder(builder, sites);
    generate_type_of(&mut b, value, |b| cold(&mut *b.words.builder))
}

/// `BUILTIN_TYPE(tag)`, owned; `object_class` is the thread record's
/// `ObjectClass` word and `cold(builder)` is `molt_builtin_type(tag)`.
pub(crate) fn emit_builtin_type(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    tag: Value,
    object_class: Value,
    cold: impl FnOnce(&mut FunctionBuilder<'_>) -> Value,
) -> Value {
    let mut b = lifecycle_builder(builder, sites);
    generate_builtin_type(&mut b, tag, object_class, |b| cold(&mut *b.words.builder))
}

/// The release of `bits` after its count read 1. `free` is
/// `molt_heap_free`; `release_fields` is the program's field-release helper
/// (`ReleaseFieldsBody`); `cold(builder)` is `molt_dec_ref_obj` of `bits`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_release_terminal(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    bits: Value,
    admission: Value,
    words: LifecycleWords<Value>,
    terminal_release: Value,
    free: FuncRef,
    release_fields: FuncRef,
    cold: impl FnOnce(&mut FunctionBuilder<'_>),
) {
    let mut b = lifecycle_builder(builder, sites);
    generate_release_terminal(
        &mut b,
        bits,
        admission,
        words,
        terminal_release,
        ListLocking::BUILD,
        RefcountStorage::for_build(molt_codegen_abi::MOLT_REFCOUNT_ATOMIC),
        |b, object| {
            b.words.builder.ins().call(free, &[object]);
        },
        |b, pointer, fields| {
            b.words
                .builder
                .ins()
                .call(release_fields, &[pointer, fields]);
        },
        |b| cold(&mut *b.words.builder),
    );
}

/// Drop each field reference of the dying instance at `pointer`; `dec_ref`
/// is `molt_dec_ref_obj`, which takes a last reference.
pub(crate) fn emit_release_fields(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    pointer: Value,
    fields: Value,
    dec_ref: FuncRef,
) {
    let mut b = lifecycle_builder(builder, sites);
    generate_release_fields(&mut b, pointer, fields, |b, child| {
        b.words.builder.ins().call(dec_ref, &[child]);
    });
}
