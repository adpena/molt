#[cfg(feature = "llvm")]
use inkwell::attributes::{Attribute, AttributeLoc};
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::values::FunctionValue;
// ── LLVM memory effect encoding (for the `memory` enum attribute) ──
//
// The `memory` attribute in LLVM 16+ replaces the legacy `readnone`,
// `readonly`, and `writeonly` function attributes.  Its value is a
// 6-bit bitmask encoding read/write permissions for three memory
// location classes:
//
//   bits [1:0] = Default (everything not ArgMem or InaccessibleMem)
//   bits [3:2] = ArgMem  (memory pointed to by pointer arguments)
//   bits [5:4] = InaccessibleMem (e.g. errno, thread-locals)
//
// Each 2-bit field: 0 = None, 1 = Read, 2 = Write, 3 = ReadWrite.

/// `memory(none)` — the function does not access any memory.
/// Currently unused: all molt runtime functions dereference NaN-boxed
/// heap pointers in at least their fallback paths.  Retained for future
/// use when we add inline NaN-box tag extraction intrinsics.
#[cfg(feature = "llvm")]
#[allow(dead_code)]
const MEMORY_NONE: u64 = 0;

/// `memory(read)` — the function may read any memory but never writes.
/// All three location classes set to Read (01): 0b01_01_01 = 21.
#[cfg(feature = "llvm")]
pub(super) const MEMORY_READ: u64 = 0b01_01_01;

/// Apply `nounwind` to a function declaration.
///
/// Safe for all molt runtime functions: panics are caught by `catch_unwind`
/// in `with_gil_entry!` and converted to pending exceptions with zeroed
/// return values.  No C++ exceptions are ever thrown.
#[cfg(feature = "llvm")]
pub(super) fn add_nounwind(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("nounwind");
    func.add_attribute(AttributeLoc::Function, ctx.create_enum_attribute(kind, 0));
}

/// Apply `willreturn` to a function declaration.
///
/// Valid for functions that always terminate: no infinite loops, no
/// coroutine suspension, no `longjmp`-style control transfer.
#[cfg(feature = "llvm")]
pub(super) fn add_willreturn(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("willreturn");
    func.add_attribute(AttributeLoc::Function, ctx.create_enum_attribute(kind, 0));
}

/// Apply `memory(none)` to a function — it neither reads nor writes memory.
/// See `MEMORY_NONE` for why this is currently unused.
#[cfg(feature = "llvm")]
#[allow(dead_code)]
fn add_memory_none(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("memory");
    func.add_attribute(
        AttributeLoc::Function,
        ctx.create_enum_attribute(kind, MEMORY_NONE),
    );
}

/// Apply `memory(read)` to a function — it may read memory but never writes.
#[cfg(feature = "llvm")]
pub(super) fn add_memory_read(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("memory");
    func.add_attribute(
        AttributeLoc::Function,
        ctx.create_enum_attribute(kind, MEMORY_READ),
    );
}
