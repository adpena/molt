use super::*;

/// Emit the canonical owned-reference transition.
///
/// The runtime call is deliberately opaque to generated code: GIL/default,
/// native free-threaded, and future biased/deferred storage select different
/// representations and zero-edge protocols behind the same ABI. The deleted
/// inline lane hard-coded an unconditional `AtomicU32`, skipped overflow and
/// terminal-death checks, and was permanently disabled after corrupting SSA
/// across its hidden control-flow blocks.
#[cfg(feature = "native-backend")]
#[inline(always)]
pub(crate) fn emit_inc_ref_obj(builder: &mut FunctionBuilder, value: Value, retain: FuncRef) {
    builder.ins().call(retain, &[value]);
}
