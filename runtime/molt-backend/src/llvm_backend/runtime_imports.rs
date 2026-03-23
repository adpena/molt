//! Declares runtime functions that compiled LLVM code calls into.
//!
//! These correspond to `extern "C"` functions in `molt-runtime/src/object/ops.rs`
//! and related modules. All use the NaN-boxed u64 ABI.

#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::context::Context;

/// Declare all runtime functions that lowered code may call.
///
/// We declare them as external linkage — the linker will resolve them
/// against the Molt runtime shared library or static archive.
#[cfg(feature = "llvm")]
pub fn declare_runtime_functions<'ctx>(ctx: &'ctx Context, module: &Module<'ctx>) {
    let i64_ty = ctx.i64_type();
    let i32_ty = ctx.i32_type();
    let void_ty = ctx.void_type();

    // ── Arithmetic (DynBox dispatch: (u64, u64) -> u64) ──
    for name in &[
        "molt_add",
        "molt_sub",
        "molt_mul",
        "molt_div",
        "molt_floordiv",
        "molt_mod",
        "molt_pow",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
    }

    // ── Unary (u64) -> u64 ──
    for name in &["molt_neg", "molt_not", "molt_invert", "molt_is_truthy"] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
    }

    // Note: molt_is_truthy returns i64 (0 or 1), not u64.

    // ── Comparison (u64, u64) -> u64 ──
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
        module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
    }

    // ── Bitwise (u64, u64) -> u64 ──
    for name in &[
        "molt_bit_and",
        "molt_bit_or",
        "molt_bit_xor",
        "molt_lshift",
        "molt_rshift",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
    }

    // ── Refcount ──
    // molt_inc_ref_obj(bits: u64)  (void return)
    // molt_dec_ref_obj(bits: u64)  (void return)
    {
        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(
            "molt_inc_ref_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        module.add_function(
            "molt_dec_ref_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Allocation ──
    // molt_alloc(size_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(
            "molt_alloc",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Attribute access ──
    // molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64
    // molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64
    // molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64
    {
        let get_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_get_attr_name",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        let set_ty =
            i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_set_attr_name",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_del_attr_name",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Indexing ──
    // molt_getitem_method(obj_bits: u64, key_bits: u64) -> u64
    // molt_setitem_method(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64
    // molt_delitem_method(obj_bits: u64, key_bits: u64) -> u64
    {
        let get_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_getitem_method",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        let set_ty =
            i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_setitem_method",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_delitem_method",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Slice ──
    // molt_slice_new(start: u64, stop: u64, step: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(
            &[i64_ty.into(), i64_ty.into(), i64_ty.into()],
            false,
        );
        module.add_function(
            "molt_slice_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Iteration ──
    // molt_iter_next(iter_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(
            "molt_iter_next",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Exception handling ──
    // molt_raise(exc_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(
            "molt_raise",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Call infrastructure ──
    // molt_callargs_new(pos_capacity: u64, kw_capacity: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_callargs_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }
    // molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_callargs_push_pos",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }
    // molt_call_bind(call_bits: u64, builder_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_call_bind",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Containers ──
    // molt_dict_new(capacity_bits: u64) -> u64
    // molt_set_new(capacity_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(
            "molt_dict_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        module.add_function(
            "molt_set_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }

    // ── Import ──
    // molt_module_import(name_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(
            "molt_module_import",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }
    // molt_module_get_attr(module_bits: u64, attr_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        module.add_function(
            "molt_module_get_attr",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
    }
}

#[cfg(all(test, feature = "llvm"))]
mod tests {
    use super::*;
    use inkwell::context::Context;

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
}
