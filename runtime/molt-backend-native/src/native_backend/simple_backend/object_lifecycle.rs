//! Cranelift's spelling of the shared plain-instance allocation and terminal
//! free (`molt_codegen_abi::object_lifecycle`), inline over the frame
//! builder's control and word primitives, as `field_slot.rs` is.

use super::*;
use molt_codegen_abi::list_item::ListLocking;
use molt_codegen_abi::object_lifecycle::{
    LifecycleWords, PlanSource, generate_object_alloc, generate_release_terminal,
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
    profile_enabled: Value,
    allocate: FuncRef,
    cold: impl FnOnce(&mut FunctionBuilder<'_>) -> Value,
) -> Value {
    let mut b = lifecycle_builder(builder, sites);
    generate_object_alloc(
        &mut b,
        class,
        plan,
        LifecycleWords { profile_enabled },
        ListLocking::BUILD,
        |b, bytes| {
            let call = b.words.builder.ins().call(allocate, &[bytes]);
            b.words.builder.inst_results(call)[0]
        },
        |b| cold(&mut *b.words.builder),
    )
}

/// The release of `bits` after its count read 1. `free` is
/// `molt_heap_free`; `cold(builder)` is `molt_dec_ref_obj`.
pub(crate) fn emit_release_terminal(
    builder: &mut FunctionBuilder<'_>,
    sites: NativeFrameSites,
    bits: Value,
    profile_enabled: Value,
    terminal_release: Value,
    free: FuncRef,
    cold: impl FnOnce(&mut FunctionBuilder<'_>),
) {
    let mut b = lifecycle_builder(builder, sites);
    generate_release_terminal(
        &mut b,
        bits,
        LifecycleWords { profile_enabled },
        terminal_release,
        ListLocking::BUILD,
        |b, base, bytes| {
            b.words.builder.ins().call(free, &[base, bytes]);
        },
        |b| cold(&mut *b.words.builder),
    );
}
