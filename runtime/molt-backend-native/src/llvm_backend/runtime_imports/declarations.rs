#[cfg(feature = "llvm")]
use super::attributes::{add_memory_read, add_nounwind, add_willreturn};
#[cfg(feature = "llvm")]
use crate::runtime_import_abi::{
    MOLT_DEC_REF_OBJ, MOLT_INC_REF_OBJ, RuntimeImportSignature, RuntimeReturnAbi,
};
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::types::FunctionType;
#[cfg(feature = "llvm")]
use inkwell::values::FunctionValue;
#[cfg(feature = "llvm")]
pub(super) fn runtime_function_type<'ctx>(
    ctx: &'ctx Context,
    signature: RuntimeImportSignature,
) -> FunctionType<'ctx> {
    let i64_ty = ctx.i64_type();
    let params = vec![i64_ty.into(); signature.param_count];
    match signature.return_abi {
        RuntimeReturnAbi::I64 => i64_ty.fn_type(&params, false),
        RuntimeReturnAbi::Void => ctx.void_type().fn_type(&params, false),
    }
}

/// Declare a runtime symbol that is not in the fixed import table yet.
///
/// This is the only on-demand declaration path lowering may use. It deliberately
/// applies the weakest globally valid runtime attribute set: `nounwind` only.
/// Stronger facts such as `willreturn` must be encoded by adding the symbol to
/// `declare_runtime_functions` so the central table owns that proof.
#[cfg(feature = "llvm")]
pub(crate) fn declare_conservative_runtime_function<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    name: &str,
    fn_ty: FunctionType<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(func) = module.get_function(name) {
        return func;
    }
    let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
    add_nounwind(ctx, func);
    func
}

/// Declare all runtime functions that lowered code may call.
///
/// We declare them as external linkage — the linker will resolve them
/// against the Molt runtime shared library or static archive.
///
/// Each function is annotated with LLVM attributes to enable
/// interprocedural optimization.  See module-level documentation.
#[cfg(feature = "llvm")]
pub(crate) fn declare_runtime_functions<'ctx>(ctx: &'ctx Context, module: &Module<'ctx>) {
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
        // In-place augmented-assignment runtime entries. The boxed slow paths of
        // emit_binary_arith / emit_bitwise dispatch `x //= y` etc. through these
        // (they try the `__i<op>__` dunder before the binary protocol). They
        // must be declared here or call_runtime_2's get_function lookup panics
        // ("Runtime function not declared"). matmul rides the preserved-Copy
        // lane and is declared by the classified runtime-import table below.
        //
        // add/sub/mul: the first-class OpCode::InplaceAdd/Sub/Mul share
        // emit_binary_arith with their binary opcode and previously dispatched
        // the boxed fallback to molt_add/sub/mul (skipping __iadd__/etc. — a
        // latent LLVM-only parity bug); the boxed slow path now correctly calls
        // molt_inplace_add/sub/mul, which therefore must be declared here.
        "molt_inplace_add",
        "molt_inplace_sub",
        "molt_inplace_mul",
        "molt_inplace_div",
        "molt_inplace_floordiv",
        "molt_inplace_mod",
        "molt_inplace_pow",
        "molt_inplace_lshift",
        "molt_inplace_rshift",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Unary (u64) -> u64 ──
    // Unary ops may invoke user protocol methods (__neg__, __bool__, __invert__).
    for name in &["molt_neg", "molt_not", "molt_invert"] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Integer boxing (i64 -> u64 NaN-boxed) ──
    //
    // Channel constructor: returns an opaque runtime handle encoded in Molt
    // object bits. It is not a generic boxed preserved-Copy fallback because the
    // Rust ABI advertises the semantic `ChanHandle` type; LLVM owns it through a
    // dedicated `chan_new` arm.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_chan_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // `molt_int_from_i64` boxes a raw i64 into a tagged integer handle. Used by
    // the overflow-safe integer box path (`box_i64_overflow_safe`) so the LLVM
    // backend boxes integers through the same runtime entry point the native
    // backend uses, instead of an unconditional 47-bit truncating mask.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_int_from_i64",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // Truthiness may invoke user __bool__/__len__; it must not promise
    // termination.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_is_truthy",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // Type-check helper: reads object layout only and always returns.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_is_function_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // Fast-path truthy checks: these inspect NaN-box bits and only
    // fall back to the GIL-wrapped path for unexpected types.  They
    // are read-only from LLVM's perspective (no allocation, no visible
    // mutation).
    for name in &["molt_is_truthy_int", "molt_is_truthy_bool"] {
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
    for name in &["molt_is_truthy_int_nogil", "molt_is_truthy_bool_nogil"] {
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
    }

    // ── Bitwise (u64, u64) -> u64 ──
    // Bitwise ops may invoke user numeric protocol methods.
    for name in &[
        "molt_bit_and",
        "molt_bit_or",
        "molt_bit_xor",
        "molt_lshift",
        "molt_rshift",
        // In-place bitwise: the Copy-carried `inplace_bit_*` arms in
        // lower_preserved_simpleir_op route through emit_bitwise, whose boxed
        // fallback now dispatches `|=`/`&=`/`^=` to molt_inplace_bit_* (trying
        // `__ior__`/etc. first — previously they wrongly used molt_bit_*).
        "molt_inplace_bit_and",
        "molt_inplace_bit_or",
        "molt_inplace_bit_xor",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Refcount ──
    // molt_inc_ref_obj(bits: u64)  (void return)
    // molt_dec_ref_obj(bits: u64)  (void return)
    // These mutate refcount fields in heap objects.  No memory attribute.
    // dec_ref may trigger deallocation chains but always returns.
    {
        let inc = module.add_function(
            MOLT_INC_REF_OBJ.name,
            runtime_function_type(ctx, MOLT_INC_REF_OBJ),
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, inc);
        add_willreturn(ctx, inc);
        let dec = module.add_function(
            MOLT_DEC_REF_OBJ.name,
            runtime_function_type(ctx, MOLT_DEC_REF_OBJ),
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
        let set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_set_attr_name",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_del_attr_name",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
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
        // molt_getitem_unchecked(obj_bits: u64, key_bits: u64) -> u64
        // Same signature as molt_getitem_method. The current runtime delegates to
        // the generic index path, so it cannot claim readonly/willreturn yet.
        let func = module.add_function(
            "molt_getitem_unchecked",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_setitem_method",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_delitem_method",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
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
            "molt_exception_last_pending",
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
    {
        let fn_ty = void_ty.fn_type(&[], false);
        let func = module.add_function(
            "molt_print_newline",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);

        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_print_obj",
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
        // No willreturn: dynamic callable dispatch may execute arbitrary user code.
    }
    // Fast dynamic-call entries execute the target callable directly.
    {
        for (name, arity) in &[
            ("molt_call_bind_ic", 3usize),
            ("molt_call_indirect_ic", 3),
            ("molt_call_func_fast0", 1),
            ("molt_call_func_fast1", 2),
            ("molt_call_func_fast2", 3),
            ("molt_call_func_fast3", 4),
        ] {
            let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                (0..*arity).map(|_| i64_ty.into()).collect();
            let fn_ty = i64_ty.fn_type(&params, false);
            let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
        }
    }

    // ── Function and code-object construction ──
    {
        for (name, arity) in &[
            ("molt_func_new", 3usize),
            ("molt_func_new_builtin_named", 4),
            ("molt_func_new_closure", 4),
            ("molt_code_new", 9),
        ] {
            let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                (0..*arity).map(|_| i64_ty.into()).collect();
            let fn_ty = i64_ty.fn_type(&params, false);
            let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let code_slot_set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_code_slot_set",
            code_slot_set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);

        let unary_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        for name in &[
            "molt_code_slots_init",
            "molt_trace_enter_slot",
            "molt_frame_locals_set",
            "molt_trace_set_line",
        ] {
            let func =
                module.add_function(name, unary_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let noarg_ty = i64_ty.fn_type(&[], false);
        let func = module.add_function(
            "molt_trace_exit",
            noarg_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);

        let fn_ptr_code_set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_fn_ptr_code_set",
            fn_ptr_code_set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_function_defaults_version",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Fused method-dispatch ICs ──
    // molt_call_method_icN(site, recv, name_ptr, name_len, a0..a{N-1}) -> u64
    //   N positional args, N ∈ 0..=4. All u64 — the name pointer is a real
    //   native pointer cast to i64 (every value NaN-boxed). These dispatch the
    //   resolved method, which executes arbitrary user code: NO `willreturn`
    //   (a method may loop forever or suspend), but `nounwind` holds (the
    //   `with_gil_entry_nopanic!` catch_unwind boundary converts every panic to
    //   a pending exception). 4 + N parameters.
    for (name, n) in &[
        ("molt_call_method_ic0", 0usize),
        ("molt_call_method_ic1", 1),
        ("molt_call_method_ic2", 2),
        ("molt_call_method_ic3", 3),
        ("molt_call_method_ic4", 4),
    ] {
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..(4 + *n)).map(|_| i64_ty.into()).collect();
        let fn_ty = i64_ty.fn_type(&params, false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }
    // molt_call_super_method_icN(site, class, self, name_ptr, name_len, a0..) -> u64
    //   5 + N parameters; same effect profile (nounwind, no willreturn).
    for (name, n) in &[
        ("molt_call_super_method_ic0", 0usize),
        ("molt_call_super_method_ic1", 1),
        ("molt_call_super_method_ic2", 2),
        ("molt_call_super_method_ic3", 3),
        ("molt_call_super_method_ic4", 4),
    ] {
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..(5 + *n)).map(|_| i64_ty.into()).collect();
        let fn_ty = i64_ty.fn_type(&params, false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
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

    // ── Import / module namespace helpers ──
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
    {
        let unary_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        for name in &[
            "molt_module_new",
            "molt_module_cache_get",
            "molt_module_cache_del",
        ] {
            let func =
                module.add_function(name, unary_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let binary_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        for name in &[
            "molt_module_cache_set",
            "molt_module_get_attr",
            "molt_module_import_from",
            "molt_module_get_global",
            "molt_module_get_name",
            "molt_module_del_global",
            "molt_module_del_global_if_present",
        ] {
            let func =
                module.add_function(name, binary_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let ternary_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_module_set_attr",
            ternary_ty,
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
    {
        let ptr_ty = ctx.ptr_type(inkwell::AddressSpace::default());
        let fn_ty = i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_bigint_from_str",
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
        let func = module.add_function(
            "molt_for_iter",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
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
