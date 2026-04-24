//! Declares runtime functions that compiled LLVM code calls into.
//!
//! These correspond to `extern "C"` functions in `molt-runtime/src/object/ops.rs`
//! and related modules. All use the NaN-boxed u64 ABI.
//!
//! ## LLVM function attributes
//!
//! Every declared function is annotated with LLVM attributes that enable
//! interprocedural optimization:
//!
//! - **`nounwind`**: All molt runtime functions use explicit error return
//!   values (NaN-boxed sentinels) and `catch_unwind` at FFI boundaries.
//!   Panics never escape as C++ exceptions, so LLVM can omit landing pads
//!   and exception handling tables entirely.
//!
//! - **`willreturn`**: Applied to functions that always terminate (no
//!   infinite loops, no coroutine suspension). Enables more aggressive
//!   dead code elimination and code motion.
//!
//! - **`memory(read)`** (= `readonly`): Applied to functions that read
//!   memory but never write to it. Enables CSE and LICM of repeated
//!   calls with the same arguments.
//!
//! - **`memory(none)`** (= `readnone`): Applied to functions that
//!   neither read nor write memory — pure functions of their arguments.
//!   Enables full redundancy elimination.

#[cfg(feature = "llvm")]
use inkwell::attributes::{Attribute, AttributeLoc};
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
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
const MEMORY_READ: u64 = 0b01_01_01;

/// Apply `nounwind` to a function declaration.
///
/// Safe for all molt runtime functions: panics are caught by `catch_unwind`
/// in `with_gil_entry!` and converted to pending exceptions with zeroed
/// return values.  No C++ exceptions are ever thrown.
#[cfg(feature = "llvm")]
fn add_nounwind(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("nounwind");
    func.add_attribute(AttributeLoc::Function, ctx.create_enum_attribute(kind, 0));
}

/// Apply `willreturn` to a function declaration.
///
/// Valid for functions that always terminate: no infinite loops, no
/// coroutine suspension, no `longjmp`-style control transfer.
#[cfg(feature = "llvm")]
fn add_willreturn(ctx: &Context, func: FunctionValue<'_>) {
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
fn add_memory_read(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("memory");
    func.add_attribute(
        AttributeLoc::Function,
        ctx.create_enum_attribute(kind, MEMORY_READ),
    );
}

/// Declare all runtime functions that lowered code may call.
///
/// We declare them as external linkage — the linker will resolve them
/// against the Molt runtime shared library or static archive.
///
/// Each function is annotated with LLVM attributes to enable
/// interprocedural optimization.  See module-level documentation.
#[cfg(feature = "llvm")]
pub fn declare_runtime_functions<'ctx>(ctx: &'ctx Context, module: &Module<'ctx>) {
    let i64_ty = ctx.i64_type();
    let i32_ty = ctx.i32_type();
    let void_ty = ctx.void_type();

    // ── Arithmetic (DynBox dispatch: (u64, u64) -> u64) ──
    // These may allocate (e.g. bigint overflow) so no memory attribute.
    for name in &[
        "molt_add",
        "molt_str_concat",
        "molt_sub",
        "molt_mul",
        "molt_div",
        "molt_floordiv",
        "molt_mod",
        "molt_pow",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Unary (u64) -> u64 ──
    //
    // Unary ops that may allocate (neg/not/invert produce new heap objects):
    for name in &["molt_neg", "molt_not", "molt_invert"] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // Truthy / type-check functions: GIL-wrapped, may read heap objects
    // and env vars, but never allocate or mutate user-visible state.
    for name in &["molt_is_truthy", "molt_is_function_obj"] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // Fast-path truthy checks: these inspect NaN-box bits and only
    // fall back to the GIL-wrapped path for unexpected types.  They
    // are read-only from LLVM's perspective (no allocation, no visible
    // mutation).
    for name in &[
        "molt_is_truthy_int",
        "molt_is_truthy_bool",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        add_memory_read(ctx, func);
    }

    // GIL-free truthy checks: no GIL acquisition, no catch_unwind, no
    // signal checks.  Pure reads of the NaN-boxed value bits with a
    // fallback to the fast-path check above.  Read-only: they inspect
    // memory but never write.
    //
    // Note: molt_is_truthy_* return i64 (0 or 1), not u64.
    for name in &[
        "molt_is_truthy_int_nogil",
        "molt_is_truthy_bool_nogil",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        add_memory_read(ctx, func);
    }

    // ── Comparison (u64, u64) -> u64 ──
    // Comparisons read heap objects but may invoke user-defined __eq__,
    // __lt__, etc. that allocate.  No memory attribute is safe here.
    for name in &[
        "molt_eq",
        "molt_ne",
        "molt_lt",
        "molt_le",
        "molt_gt",
        "molt_ge",
        "molt_contains",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Bitwise (u64, u64) -> u64 ──
    // Bitwise ops may allocate (bigint promotion on shift overflow).
    for name in &[
        "molt_bit_and",
        "molt_bit_or",
        "molt_bit_xor",
        "molt_lshift",
        "molt_rshift",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Refcount ──
    // molt_inc_ref_obj(bits: u64)  (void return)
    // molt_dec_ref_obj(bits: u64)  (void return)
    // These mutate refcount fields in heap objects.  No memory attribute.
    // dec_ref may trigger deallocation chains but always returns.
    {
        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
        let inc = module.add_function(
            "molt_inc_ref_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, inc);
        add_willreturn(ctx, inc);
        let dec = module.add_function(
            "molt_dec_ref_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, dec);
        add_willreturn(ctx, dec);
    }

    // ── Allocation ──
    // molt_alloc(size_bits: u64) -> u64
    // Allocates heap memory — no memory restriction attribute.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_alloc",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Attribute access ──
    // molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64
    // molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64
    // molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64
    // These may invoke descriptors (__get__, __set__, __delete__) which
    // can have arbitrary side effects.  No memory attribute.
    {
        let get_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_get_attr_name",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let get_obj_ic_ty = i64_ty.fn_type(
            &[i64_ty.into(), i64_ty.into(), i64_ty.into(), i64_ty.into()],
            false,
        );
        let func = module.add_function(
            "molt_get_attr_object_ic",
            get_obj_ic_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_set_attr_name",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_del_attr_name",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Indexing ──
    // molt_getitem_method(obj_bits: u64, key_bits: u64) -> u64
    // molt_setitem_method(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64
    // molt_delitem_method(obj_bits: u64, key_bits: u64) -> u64
    // May invoke __getitem__/__setitem__/__delitem__ with side effects.
    {
        let get_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_getitem_method",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        // molt_getitem_unchecked(obj_bits: u64, key_bits: u64) -> u64
        // Same signature as molt_getitem_method but skips bounds checking.
        // Used when the BCE pass has proven the index is in-range.
        // Read-only: proven in-bounds index access never mutates the container.
        let func = module.add_function(
            "molt_getitem_unchecked",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        add_memory_read(ctx, func);
        let set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_setitem_method",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_delitem_method",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Slice ──
    // molt_slice_new(start: u64, stop: u64, step: u64) -> u64
    // Allocates a new slice object.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_slice_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Iteration ──
    // molt_iter_next(iter_bits: u64) -> u64
    // Advances an iterator — mutates the iterator's internal state.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_iter_next",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Descriptor wrappers ──
    // Allocate new descriptor objects.
    {
        let unary_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_classmethod_new",
            unary_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_staticmethod_new",
            unary_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let property_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_property_new",
            property_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Exception handling ──
    // molt_raise(exc_bits: u64) -> u64
    // Sets the pending exception flag — mutates thread-local state.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_raise",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // exception handler stack helpers — all mutate thread-local exception state.
    {
        let noarg_ret_i64 = i64_ty.fn_type(&[], false);
        let unary_ret_i64 = i64_ty.fn_type(&[i64_ty.into()], false);
        for name in &[
            "molt_exception_clear",
            "molt_exception_last",
            "molt_exception_current",
            "molt_exception_push",
            "molt_exception_pop",
            "molt_exception_stack_enter",
            "molt_exception_stack_depth",
            "molt_exception_stack_clear",
        ] {
            let func = module.add_function(
                name,
                noarg_ret_i64,
                Some(inkwell::module::Linkage::External),
            );
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }
        for name in &[
            "molt_exception_enter_handler",
            "molt_exception_stack_exit",
            "molt_exception_stack_set_depth",
            "molt_exception_resolve_captured",
        ] {
            let func = module.add_function(
                name,
                unary_ret_i64,
                Some(inkwell::module::Linkage::External),
            );
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }
    }

    // ── Diagnostics ──
    // molt_warn_stderr(msg_bits: u64) -> void
    // Writes to stderr — side-effecting but always returns.
    {
        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_warn_stderr",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Call infrastructure ──
    // Allocate and populate call-argument builders.  All allocate heap memory.
    // molt_callargs_new(pos_capacity: u64, kw_capacity: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_callargs_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_callargs_push_pos",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // molt_call_bind(call_bits: u64, builder_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_bind",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Containers ──
    // molt_dict_new(capacity_bits: u64) -> u64
    // molt_set_new(capacity_bits: u64) -> u64
    // Allocate new container objects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_dict_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_set_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Import ──
    // molt_module_import(name_bits: u64) -> u64
    // May execute module-level code with arbitrary side effects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_module_import",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        // No willreturn: import may execute arbitrary module init code.
    }
    // molt_module_get_attr(module_bits: u64, attr_bits: u64) -> u64
    // Reads module namespace — may invoke __getattr__.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_module_get_attr",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Method / Builtin call ──
    // molt_call_method(receiver: u64, name: u64, args_builder: u64) -> u64
    // Invokes arbitrary user code — no memory or willreturn attribute.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_method",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // molt_call_builtin(name: u64, args_builder: u64) -> u64
    // Invokes a builtin function — may have arbitrary side effects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_builtin",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── String construction ──
    // molt_string_from_bytes(ptr: *const u8, len: u64, out: *mut u64) -> i32
    // Allocates a new string object from a byte buffer.
    {
        let ptr_ty = ctx.ptr_type(inkwell::AddressSpace::default());
        let fn_ty = i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
        let func = module.add_function(
            "molt_string_from_bytes",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // molt_call_0(callable: u64) -> u64
    // Invoke a callable with zero arguments. Used by SCF desugaring.
    // Executes arbitrary user code — no memory or willreturn attribute.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_0",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── Container builders ──
    // All builder functions allocate / mutate heap builder state.
    // list uses builder_new/append/finish
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_list_builder_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let fn_ty2 = void_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_list_builder_append",
            fn_ty2,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_list_builder_finish",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // tuple reuses the list-shaped builder payload, but finalizes to tuple
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_tuple_builder_finish",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // dict builder
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_dict_builder_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let fn_ty2 = void_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_dict_builder_append",
            fn_ty2,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_dict_builder_finish",
            i64_ty.fn_type(&[i64_ty.into()], false),
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // set builder
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_set_builder_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let fn_ty2 = void_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_set_builder_append",
            fn_ty2,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_set_builder_finish",
            i64_ty.fn_type(&[i64_ty.into()], false),
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Iteration (GetIter / ForIter) ──
    // molt_get_iter(obj: u64) -> u64
    // molt_for_iter(iter: u64) -> u64  (returns sentinel on exhaustion)
    // Both may invoke __iter__/__next__ with side effects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_get_iter",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_for_iter",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Generator support ──
    // molt_yield(value: u64) -> u64
    // molt_yield_from(subiter: u64) -> u64
    // These suspend coroutine execution — NOT willreturn.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_yield",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let func = module.add_function(
            "molt_yield_from",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── Exception pending check ──
    // molt_exception_pending() -> u64  (returns nonzero if exception pending)
    // Reads thread-local exception flag — read-only, always returns.
    {
        let fn_ty = i64_ty.fn_type(&[], false);
        let func = module.add_function(
            "molt_exception_pending",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        add_memory_read(ctx, func);
    }

    // ── Deopt ──
    // molt_deopt_transfer(frame: u64) -> u64
    // Transfers control to the interpreter — NOT willreturn (may not
    // return to this compiled code path).
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_deopt_transfer",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── SCF dialect runtime helpers ──
    // These are called when SCF ops survive lowering (not yet fully desugared).
    // All execute user-provided closures with arbitrary side effects.
    // molt_scf_if(cond: u64, then_fn: u64, else_fn: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_scf_if",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        // willreturn: if-expressions always evaluate one branch and return.
        add_willreturn(ctx, func);
    }
    // molt_scf_for(lb: u64, ub: u64, step: u64, body_fn: u64) -> u64
    // Executes a loop — NOT willreturn (body may execute indefinitely
    // if step is zero, or the trip count may be very large).
    {
        let fn_ty = i64_ty.fn_type(
            &[i64_ty.into(), i64_ty.into(), i64_ty.into(), i64_ty.into()],
            false,
        );
        let func = module.add_function(
            "molt_scf_for",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // molt_scf_while(cond_fn: u64, body_fn: u64) -> u64
    // Executes a while loop — NOT willreturn (may be infinite).
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_scf_while",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // molt_scf_yield(value: u64) -> u64
    // Suspends coroutine execution — NOT willreturn.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_scf_yield",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
}

#[cfg(all(test, feature = "llvm"))]
mod tests {
    use super::*;
    use inkwell::attributes::Attribute;
    use inkwell::context::Context;

    /// Check that a function has an enum attribute with the given name.
    fn has_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
        let kind_id = Attribute::get_named_enum_kind_id(attr_name);
        if kind_id == 0 {
            // Unknown attribute name in this LLVM version — skip check.
            return true;
        }
        func.get_enum_attribute(AttributeLoc::Function, kind_id)
            .is_some()
    }

    #[test]
    fn runtime_functions_are_declared() {
        let ctx = Context::create();
        let module = ctx.create_module("test_rt");
        declare_runtime_functions(&ctx, &module);

        // Spot-check a few key functions exist
        assert!(module.get_function("molt_add").is_some());
        assert!(module.get_function("molt_sub").is_some());
        assert!(module.get_function("molt_eq").is_some());
        assert!(module.get_function("molt_inc_ref_obj").is_some());
        assert!(module.get_function("molt_dec_ref_obj").is_some());
        assert!(module.get_function("molt_alloc").is_some());
        assert!(module.get_function("molt_get_attr_name").is_some());
        assert!(module.get_function("molt_raise").is_some());
        assert!(module.get_function("molt_is_truthy").is_some());
    }

    #[test]
    fn all_functions_have_nounwind() {
        let ctx = Context::create();
        let module = ctx.create_module("test_nounwind");
        declare_runtime_functions(&ctx, &module);

        let mut func = module.get_first_function();
        while let Some(f) = func {
            assert!(
                has_fn_attr(f, "nounwind"),
                "Function {} is missing nounwind attribute",
                f.get_name().to_str().unwrap()
            );
            func = f.get_next_function();
        }
    }

    #[test]
    fn willreturn_on_simple_functions() {
        let ctx = Context::create();
        let module = ctx.create_module("test_willreturn");
        declare_runtime_functions(&ctx, &module);

        // These functions always terminate — must have willreturn.
        for name in &[
            "molt_add",
            "molt_sub",
            "molt_eq",
            "molt_alloc",
            "molt_inc_ref_obj",
            "molt_dec_ref_obj",
            "molt_slice_new",
            "molt_exception_pending",
        ] {
            let f = module.get_function(name).expect(name);
            assert!(
                has_fn_attr(f, "willreturn"),
                "Function {} is missing willreturn attribute",
                name
            );
        }
    }

    #[test]
    fn no_willreturn_on_control_flow_functions() {
        let ctx = Context::create();
        let module = ctx.create_module("test_no_willreturn");
        declare_runtime_functions(&ctx, &module);

        let willreturn_kind = Attribute::get_named_enum_kind_id("willreturn");
        if willreturn_kind == 0 {
            return; // LLVM version doesn't recognize this attribute.
        }

        // These functions may not return (coroutine suspension, loops,
        // deopt transfer, arbitrary user code execution).
        for name in &[
            "molt_yield",
            "molt_yield_from",
            "molt_deopt_transfer",
            "molt_scf_for",
            "molt_scf_while",
            "molt_scf_yield",
            "molt_call_method",
            "molt_call_builtin",
            "molt_call_0",
            "molt_module_import",
        ] {
            let f = module.get_function(name).expect(name);
            assert!(
                f.get_enum_attribute(AttributeLoc::Function, willreturn_kind)
                    .is_none(),
                "Function {} should NOT have willreturn (may not terminate)",
                name
            );
        }
    }

    #[test]
    fn memory_read_on_pure_readers() {
        let ctx = Context::create();
        let module = ctx.create_module("test_memory_read");
        declare_runtime_functions(&ctx, &module);

        let memory_kind = Attribute::get_named_enum_kind_id("memory");
        if memory_kind == 0 {
            return; // LLVM version doesn't support memory attribute.
        }

        // These functions only read memory.
        for name in &[
            "molt_is_truthy_int",
            "molt_is_truthy_bool",
            "molt_is_truthy_int_nogil",
            "molt_is_truthy_bool_nogil",
            "molt_exception_pending",
            "molt_getitem_unchecked",
        ] {
            let f = module.get_function(name).expect(name);
            let attr = f
                .get_enum_attribute(AttributeLoc::Function, memory_kind)
                .unwrap_or_else(|| panic!("{} missing memory attribute", name));
            assert_eq!(
                attr.get_enum_value(),
                MEMORY_READ,
                "Function {} should have memory(read) = {}",
                name,
                MEMORY_READ
            );
        }
    }
}
