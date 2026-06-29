use super::attributes::MEMORY_READ;
use super::declarations::runtime_function_type;
use super::*;
use crate::runtime_import_abi::{RuntimeReturnAbi, TRAMPOLINE_RUNTIME_IMPORTS};
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::context::Context;
use inkwell::values::FunctionValue;
use std::collections::HashSet;
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
    assert!(module.get_function("molt_code_new").is_some());
    assert!(module.get_function("molt_print_newline").is_some());
    assert!(module.get_function("molt_print_obj").is_some());
    assert!(module.get_function("molt_bigint_from_str").is_some());
    assert!(
        module
            .get_function("molt_function_defaults_version")
            .is_some()
    );
    // The augmented-assignment entries the boxed `emit_binary_arith` path
    // calls through `call_runtime_2` (which requires pre-declaration). A
    // TIR→LLVM-lowered function carrying `+=`/`-=`/`*=` (an inlined or
    // generator-fused caller) panics without these.
    assert!(module.get_function("molt_inplace_add").is_some());
    assert!(module.get_function("molt_inplace_sub").is_some());
    assert!(module.get_function("molt_inplace_mul").is_some());
}

#[test]
fn shared_runtime_helper_imports_have_llvm_abi_authority() {
    let ctx = Context::create();
    let module = ctx.create_module("test_shared_runtime_helpers");
    declare_runtime_functions(&ctx, &module);

    for signature in TRAMPOLINE_RUNTIME_IMPORTS {
        let func = module.get_function(signature.name).unwrap_or_else(|| {
            assert_eq!(
                runtime_import_return_abi(signature.name, signature.param_count),
                Some(signature.return_abi),
                "{} should be declared or classified",
                signature.name
            );
            declare_conservative_runtime_function(
                &ctx,
                &module,
                signature.name,
                runtime_function_type(&ctx, *signature),
            )
        });
        assert_eq!(func.count_params() as usize, signature.param_count);
        match signature.return_abi {
            RuntimeReturnAbi::I64 => {
                let ret_ty = func
                    .get_type()
                    .get_return_type()
                    .unwrap_or_else(|| panic!("{} should return i64", signature.name));
                assert!(
                    ret_ty.is_int_type(),
                    "{} should return an integer",
                    signature.name
                );
                assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
            }
            RuntimeReturnAbi::Void => {
                assert!(
                    func.get_type().get_return_type().is_none(),
                    "{} should return void",
                    signature.name
                );
            }
        }
    }
}

#[test]
fn fixed_runtime_imports_do_not_overlap_conservative_fallbacks() {
    let conservative: HashSet<&str> = CONSERVATIVE_RUNTIME_IMPORTS
        .iter()
        .map(|signature| signature.name)
        .collect();
    let mut fixed_names = HashSet::new();
    for spec in fixed::FIXED_RUNTIME_IMPORTS {
        assert!(
            fixed_names.insert(spec.name),
            "fixed LLVM runtime import `{}` appears more than once",
            spec.name,
        );
        assert!(
            !conservative.contains(spec.name),
            "fixed LLVM runtime import `{}` must not also live in the conservative fallback table",
            spec.name,
        );
    }
}

#[test]
fn custom_fixed_runtime_import_signatures_are_typed() {
    let ctx = Context::create();
    let module = ctx.create_module("test_custom_fixed_runtime_imports");
    declare_runtime_functions(&ctx, &module);

    let string_from_bytes = module
        .get_function("molt_string_from_bytes")
        .expect("molt_string_from_bytes should be fixed-declared");
    assert_eq!(string_from_bytes.count_params(), 3);
    let params = string_from_bytes.get_type().get_param_types();
    assert!(params[0].is_pointer_type());
    assert_eq!(params[1].into_int_type().get_bit_width(), 64);
    assert!(params[2].is_pointer_type());
    let ret_ty = string_from_bytes
        .get_type()
        .get_return_type()
        .expect("molt_string_from_bytes should return i32");
    assert!(ret_ty.is_int_type());
    assert_eq!(ret_ty.into_int_type().get_bit_width(), 32);
    assert!(has_fn_attr(string_from_bytes, "nounwind"));
    assert!(has_fn_attr(string_from_bytes, "willreturn"));

    let bigint_from_str = module
        .get_function("molt_bigint_from_str")
        .expect("molt_bigint_from_str should be fixed-declared");
    assert_eq!(bigint_from_str.count_params(), 2);
    let params = bigint_from_str.get_type().get_param_types();
    assert!(params[0].is_pointer_type());
    assert_eq!(params[1].into_int_type().get_bit_width(), 64);
    let ret_ty = bigint_from_str
        .get_type()
        .get_return_type()
        .expect("molt_bigint_from_str should return i64");
    assert!(ret_ty.is_int_type());
    assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
    assert!(has_fn_attr(bigint_from_str, "nounwind"));
    assert!(has_fn_attr(bigint_from_str, "willreturn"));

    let get_attr_object_ic = module
        .get_function("molt_get_attr_object_ic")
        .expect("molt_get_attr_object_ic should be fixed-declared");
    assert_eq!(get_attr_object_ic.count_params(), 4);
    let params = get_attr_object_ic.get_type().get_param_types();
    assert_eq!(params[0].into_int_type().get_bit_width(), 64);
    assert!(params[1].is_pointer_type());
    assert_eq!(params[2].into_int_type().get_bit_width(), 64);
    assert_eq!(params[3].into_int_type().get_bit_width(), 64);
    let ret_ty = get_attr_object_ic
        .get_type()
        .get_return_type()
        .expect("molt_get_attr_object_ic should return i64");
    assert!(ret_ty.is_int_type());
    assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
}

#[test]
fn fused_method_dispatch_ic_runtime_functions_are_declared() {
    let ctx = Context::create();
    let module = ctx.create_module("test_method_dispatch_ic");
    declare_runtime_functions(&ctx, &module);

    // call_method_icN: site + recv + name_ptr + name_len + N args = 4 + N.
    // call_super_method_icN: site + class + self + name_ptr + name_len + N
    // args = 5 + N. `name_ptr` is a native pointer in the LLVM ABI; all other
    // values are boxed i64 carriers. These dispatch arbitrary user code, so
    // they carry `nounwind` (catch_unwind boundary) but must NOT carry
    // `willreturn`.
    let willreturn_kind = Attribute::get_named_enum_kind_id("willreturn");
    for (name, arity, ptr_index) in &[
        ("molt_call_method_ic0", 4usize, 2usize),
        ("molt_call_method_ic1", 5, 2),
        ("molt_call_method_ic2", 6, 2),
        ("molt_call_method_ic3", 7, 2),
        ("molt_call_method_ic4", 8, 2),
        ("molt_call_super_method_ic0", 5, 3),
        ("molt_call_super_method_ic1", 6, 3),
        ("molt_call_super_method_ic2", 7, 3),
        ("molt_call_super_method_ic3", 8, 3),
        ("molt_call_super_method_ic4", 9, 3),
    ] {
        let func = module
            .get_function(name)
            .unwrap_or_else(|| panic!("{name} should be declared"));
        assert_eq!(
            func.count_params() as usize,
            *arity,
            "{name} should have {arity} i64 parameters"
        );
        let params = func.get_type().get_param_types();
        for (idx, param) in params.iter().enumerate() {
            if idx == *ptr_index {
                assert!(param.is_pointer_type(), "{name} param {idx} should be ptr");
            } else {
                assert!(param.is_int_type(), "{name} param {idx} should be integer");
                assert_eq!(
                    param.into_int_type().get_bit_width(),
                    64,
                    "{name} param {idx} should be i64"
                );
            }
        }
        assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
        // Method dispatch runs arbitrary user code (may loop/suspend): the
        // declaration must not promise termination.
        if willreturn_kind != 0 {
            assert!(
                func.get_enum_attribute(AttributeLoc::Function, willreturn_kind)
                    .is_none(),
                "{name} must NOT have willreturn (dispatches arbitrary user code)"
            );
        }
    }
}

#[test]
fn function_and_code_runtime_functions_are_declared() {
    let ctx = Context::create();
    let module = ctx.create_module("test_function_code_runtime");
    declare_runtime_functions(&ctx, &module);

    let memory_kind = Attribute::get_named_enum_kind_id("memory");
    for (name, arity) in &[
        ("molt_func_new", 3usize),
        ("molt_func_new_builtin_named", 4),
        ("molt_func_new_closure", 4),
        ("molt_code_new", 9),
        ("molt_code_slot_set", 2),
        ("molt_code_slots_init", 1),
        ("molt_trace_enter_slot", 1),
        ("molt_trace_exit", 0),
        ("molt_frame_locals_set", 1),
        ("molt_trace_set_line", 1),
        ("molt_fn_ptr_code_set", 2),
        ("molt_function_defaults_version", 1),
    ] {
        let func = module
            .get_function(name)
            .unwrap_or_else(|| panic!("{name} should be declared"));
        assert_eq!(
            func.count_params() as usize,
            *arity,
            "{name} should have {arity} i64 parameters"
        );
        let ret_ty = func
            .get_type()
            .get_return_type()
            .unwrap_or_else(|| panic!("{name} should return i64"));
        assert!(ret_ty.is_int_type(), "{name} should return an integer");
        assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
        assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
        assert!(
            has_fn_attr(func, "willreturn"),
            "{name} should have willreturn"
        );
        if memory_kind != 0 {
            assert!(
                func.get_enum_attribute(AttributeLoc::Function, memory_kind)
                    .is_none(),
                "{name} should not claim a read-only/no-memory effect"
            );
        }
    }
}

#[test]
fn diagnostics_and_constant_runtime_functions_are_declared() {
    let ctx = Context::create();
    let module = ctx.create_module("test_diagnostics_constants_runtime");
    declare_runtime_functions(&ctx, &module);

    let print_newline = module
        .get_function("molt_print_newline")
        .expect("molt_print_newline should be declared");
    assert_eq!(print_newline.count_params(), 0);
    assert!(
        print_newline.get_type().get_return_type().is_none(),
        "molt_print_newline should return void"
    );
    assert!(has_fn_attr(print_newline, "nounwind"));
    assert!(has_fn_attr(print_newline, "willreturn"));

    let print_obj = module
        .get_function("molt_print_obj")
        .expect("molt_print_obj should be declared");
    assert_eq!(print_obj.count_params(), 1);
    assert!(
        print_obj.get_type().get_return_type().is_none(),
        "molt_print_obj should return void"
    );
    assert!(has_fn_attr(print_obj, "nounwind"));
    assert!(has_fn_attr(print_obj, "willreturn"));

    let bigint_from_str = module
        .get_function("molt_bigint_from_str")
        .expect("molt_bigint_from_str should be declared");
    assert_eq!(bigint_from_str.count_params(), 2);
    let ret_ty = bigint_from_str
        .get_type()
        .get_return_type()
        .expect("molt_bigint_from_str should return i64");
    assert!(ret_ty.is_int_type());
    assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
    assert!(has_fn_attr(bigint_from_str, "nounwind"));
    assert!(has_fn_attr(bigint_from_str, "willreturn"));
}

#[test]
fn dynamic_call_runtime_functions_are_declared_without_willreturn() {
    let ctx = Context::create();
    let module = ctx.create_module("test_dynamic_call_runtime");
    declare_runtime_functions(&ctx, &module);

    let willreturn_kind = Attribute::get_named_enum_kind_id("willreturn");
    for (name, arity) in &[
        ("molt_call_bind", 2usize),
        ("molt_call_bind_ic", 3),
        ("molt_call_indirect_ic", 3),
        ("molt_call_func_fast0", 1),
        ("molt_call_func_fast1", 2),
        ("molt_call_func_fast2", 3),
        ("molt_call_func_fast3", 4),
    ] {
        let func = module
            .get_function(name)
            .unwrap_or_else(|| panic!("{name} should be declared"));
        assert_eq!(
            func.count_params() as usize,
            *arity,
            "{name} should have {arity} i64 parameters"
        );
        assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
        if willreturn_kind != 0 {
            assert!(
                func.get_enum_attribute(AttributeLoc::Function, willreturn_kind)
                    .is_none(),
                "{name} must NOT have willreturn (dispatches arbitrary user code)"
            );
        }
    }
}

fn parse_literal_ensure_runtime_calls(source: &str) -> Vec<(String, usize, RuntimeReturnAbi)> {
    let production = source
        .split("#[cfg(all(test, feature = \"llvm\"))]")
        .next()
        .unwrap_or(source);
    let mut calls = Vec::new();
    for (needle, return_abi) in [
        ("ensure_runtime_i64_fn(\"", RuntimeReturnAbi::I64),
        ("ensure_runtime_void_fn(\"", RuntimeReturnAbi::Void),
    ] {
        let mut rest = production;
        while let Some(start) = rest.find(needle) {
            rest = &rest[start + needle.len()..];
            let Some(end_name) = rest.find('"') else {
                break;
            };
            let name = &rest[..end_name];
            let after_name = &rest[end_name + 1..];
            let Some(comma) = after_name.find(',') else {
                rest = after_name;
                continue;
            };
            let mut digits = String::new();
            for ch in after_name[comma + 1..].chars() {
                if ch.is_ascii_digit() {
                    digits.push(ch);
                } else if !digits.is_empty() {
                    break;
                } else if !ch.is_whitespace() {
                    break;
                }
            }
            if let Ok(param_count) = digits.parse::<usize>() {
                calls.push((name.to_string(), param_count, return_abi));
            }
            rest = after_name;
        }
    }
    calls
}

#[test]
fn lowering_literal_runtime_imports_are_declared_or_classified() {
    let ctx = Context::create();
    let module = ctx.create_module("test_lowering_literal_runtime_imports");
    declare_runtime_functions(&ctx, &module);
    let source = include_str!("lowering.rs");

    let mut missing = Vec::new();
    for (name, param_count, return_abi) in parse_literal_ensure_runtime_calls(source) {
        if let Some(func) = module.get_function(&name) {
            assert_eq!(
                func.count_params() as usize,
                param_count,
                "{name} central declaration arity must match lowering call"
            );
            match return_abi {
                RuntimeReturnAbi::I64 => {
                    let ret_ty = func
                        .get_type()
                        .get_return_type()
                        .unwrap_or_else(|| panic!("{name} should return i64"));
                    assert!(ret_ty.is_int_type(), "{name} should return an integer");
                    assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
                }
                RuntimeReturnAbi::Void => {
                    assert!(
                        func.get_type().get_return_type().is_none(),
                        "{name} should return void"
                    );
                }
            }
            continue;
        }
        if !is_runtime_import_abi(&name, param_count, return_abi) {
            missing.push(format!("{name}/{param_count}/{return_abi:?}"));
        }
    }

    assert!(
        missing.is_empty(),
        "lowering literal runtime imports must be centrally declared or classified: {}",
        missing.join(", ")
    );
}

#[test]
fn module_namespace_runtime_functions_are_declared() {
    let ctx = Context::create();
    let module = ctx.create_module("test_module_runtime");
    declare_runtime_functions(&ctx, &module);

    for (name, arity) in &[
        ("molt_module_new", 1usize),
        ("molt_module_cache_get", 1),
        ("molt_module_cache_del", 1),
        ("molt_module_cache_set", 2),
        ("molt_module_get_attr", 2),
        ("molt_module_import_from", 2),
        ("molt_module_get_global", 2),
        ("molt_module_get_name", 2),
        ("molt_module_del_global", 2),
        ("molt_module_del_global_if_present", 2),
        ("molt_module_set_attr", 3),
    ] {
        let func = module
            .get_function(name)
            .unwrap_or_else(|| panic!("{name} should be declared"));
        assert_eq!(
            func.count_params() as usize,
            *arity,
            "{name} should have {arity} i64 parameters"
        );
        assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
        assert!(
            has_fn_attr(func, "willreturn"),
            "{name} should have willreturn"
        );
    }
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
        "molt_add",
        "molt_str_concat",
        "molt_sub",
        "molt_mul",
        "molt_div",
        "molt_floordiv",
        "molt_mod",
        "molt_pow",
        "molt_inplace_add",
        "molt_inplace_sub",
        "molt_inplace_mul",
        "molt_inplace_div",
        "molt_inplace_floordiv",
        "molt_inplace_mod",
        "molt_inplace_pow",
        "molt_inplace_lshift",
        "molt_inplace_rshift",
        "molt_neg",
        "molt_not",
        "molt_invert",
        "molt_is_truthy",
        "molt_eq",
        "molt_ne",
        "molt_lt",
        "molt_le",
        "molt_gt",
        "molt_ge",
        "molt_contains",
        "molt_bit_and",
        "molt_bit_or",
        "molt_bit_xor",
        "molt_lshift",
        "molt_rshift",
        "molt_inplace_bit_and",
        "molt_inplace_bit_or",
        "molt_inplace_bit_xor",
        "molt_get_attr_name",
        "molt_get_attr_object_ic",
        "molt_set_attr_name",
        "molt_del_attr_name",
        "molt_getitem_method",
        "molt_getitem_unchecked",
        "molt_setitem_method",
        "molt_delitem_method",
        "molt_call_builtin",
        "molt_call_bind",
        "molt_call_bind_ic",
        "molt_call_indirect_ic",
        "molt_call_func_fast0",
        "molt_call_func_fast1",
        "molt_call_func_fast2",
        "molt_call_func_fast3",
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
