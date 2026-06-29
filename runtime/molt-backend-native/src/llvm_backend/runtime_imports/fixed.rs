#[cfg(feature = "llvm")]
use super::attributes::{add_memory_read, add_nounwind, add_willreturn};
#[cfg(feature = "llvm")]
use crate::runtime_import_abi::{MOLT_DEC_REF_OBJ, MOLT_INC_REF_OBJ, RuntimeReturnAbi};
#[cfg(feature = "llvm")]
use inkwell::AddressSpace;
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::types::{BasicMetadataTypeEnum, FunctionType};
#[cfg(feature = "llvm")]
use inkwell::values::FunctionValue;

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FixedRuntimeParamAbi {
    I64,
    Ptr,
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FixedRuntimeReturnAbi {
    I64,
    I32,
    Void,
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FixedRuntimeSignature {
    Boxed {
        param_count: usize,
        return_abi: RuntimeReturnAbi,
    },
    Custom {
        params: &'static [FixedRuntimeParamAbi],
        return_abi: FixedRuntimeReturnAbi,
    },
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FixedRuntimeAttrs {
    pub(super) willreturn: bool,
    pub(super) memory_read: bool,
}

#[cfg(feature = "llvm")]
impl FixedRuntimeAttrs {
    const NONE: Self = Self {
        willreturn: false,
        memory_read: false,
    };
    const WILLRETURN: Self = Self {
        willreturn: true,
        memory_read: false,
    };
    const WILLRETURN_MEMORY_READ: Self = Self {
        willreturn: true,
        memory_read: true,
    };
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FixedRuntimeImportSpec {
    pub(super) name: &'static str,
    pub(super) signature: FixedRuntimeSignature,
    pub(super) attrs: FixedRuntimeAttrs,
}

#[cfg(feature = "llvm")]
const fn i64_ret(
    name: &'static str,
    param_count: usize,
    attrs: FixedRuntimeAttrs,
) -> FixedRuntimeImportSpec {
    FixedRuntimeImportSpec {
        name,
        signature: FixedRuntimeSignature::Boxed {
            param_count,
            return_abi: RuntimeReturnAbi::I64,
        },
        attrs,
    }
}

#[cfg(feature = "llvm")]
const fn void_ret(
    name: &'static str,
    param_count: usize,
    attrs: FixedRuntimeAttrs,
) -> FixedRuntimeImportSpec {
    FixedRuntimeImportSpec {
        name,
        signature: FixedRuntimeSignature::Boxed {
            param_count,
            return_abi: RuntimeReturnAbi::Void,
        },
        attrs,
    }
}

#[cfg(feature = "llvm")]
const fn custom(
    name: &'static str,
    params: &'static [FixedRuntimeParamAbi],
    return_abi: FixedRuntimeReturnAbi,
    attrs: FixedRuntimeAttrs,
) -> FixedRuntimeImportSpec {
    FixedRuntimeImportSpec {
        name,
        signature: FixedRuntimeSignature::Custom { params, return_abi },
        attrs,
    }
}

#[cfg(feature = "llvm")]
const ATTR_NONE: FixedRuntimeAttrs = FixedRuntimeAttrs::NONE;
#[cfg(feature = "llvm")]
const ATTR_WILLRETURN: FixedRuntimeAttrs = FixedRuntimeAttrs::WILLRETURN;
#[cfg(feature = "llvm")]
const ATTR_WILLRETURN_MEMORY_READ: FixedRuntimeAttrs = FixedRuntimeAttrs::WILLRETURN_MEMORY_READ;

#[cfg(feature = "llvm")]
const PTR_I64: &[FixedRuntimeParamAbi] = &[FixedRuntimeParamAbi::Ptr, FixedRuntimeParamAbi::I64];
#[cfg(feature = "llvm")]
const PTR_I64_PTR: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
];
#[cfg(feature = "llvm")]
const I64_PTR_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_PTR_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_PTR_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_PTR_I64_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_PTR_I64_I64_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_PTR_I64_I64_I64_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_I64_PTR_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_I64_PTR_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_I64_PTR_I64_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_I64_PTR_I64_I64_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];
#[cfg(feature = "llvm")]
const I64_I64_I64_PTR_I64_I64_I64_I64_I64: &[FixedRuntimeParamAbi] = &[
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::Ptr,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
    FixedRuntimeParamAbi::I64,
];

#[cfg(feature = "llvm")]
pub(super) const FIXED_RUNTIME_IMPORTS: &[FixedRuntimeImportSpec] = &[
    i64_ret("molt_add", 2, ATTR_NONE),
    i64_ret("molt_str_concat", 2, ATTR_NONE),
    i64_ret("molt_sub", 2, ATTR_NONE),
    i64_ret("molt_mul", 2, ATTR_NONE),
    i64_ret("molt_div", 2, ATTR_NONE),
    i64_ret("molt_floordiv", 2, ATTR_NONE),
    i64_ret("molt_mod", 2, ATTR_NONE),
    i64_ret("molt_pow", 2, ATTR_NONE),
    i64_ret("molt_inplace_add", 2, ATTR_NONE),
    i64_ret("molt_inplace_sub", 2, ATTR_NONE),
    i64_ret("molt_inplace_mul", 2, ATTR_NONE),
    i64_ret("molt_inplace_div", 2, ATTR_NONE),
    i64_ret("molt_inplace_floordiv", 2, ATTR_NONE),
    i64_ret("molt_inplace_mod", 2, ATTR_NONE),
    i64_ret("molt_inplace_pow", 2, ATTR_NONE),
    i64_ret("molt_inplace_lshift", 2, ATTR_NONE),
    i64_ret("molt_inplace_rshift", 2, ATTR_NONE),
    i64_ret("molt_neg", 1, ATTR_NONE),
    i64_ret("molt_not", 1, ATTR_NONE),
    i64_ret("molt_invert", 1, ATTR_NONE),
    i64_ret("molt_chan_new", 1, ATTR_NONE),
    i64_ret("molt_int_from_i64", 1, ATTR_WILLRETURN),
    i64_ret("molt_is_truthy", 1, ATTR_NONE),
    i64_ret("molt_is_function_obj", 1, ATTR_WILLRETURN),
    i64_ret("molt_is_truthy_int", 1, ATTR_WILLRETURN_MEMORY_READ),
    i64_ret("molt_is_truthy_bool", 1, ATTR_WILLRETURN_MEMORY_READ),
    i64_ret("molt_is_truthy_int_nogil", 1, ATTR_WILLRETURN_MEMORY_READ),
    i64_ret("molt_is_truthy_bool_nogil", 1, ATTR_WILLRETURN_MEMORY_READ),
    i64_ret("molt_eq", 2, ATTR_NONE),
    i64_ret("molt_ne", 2, ATTR_NONE),
    i64_ret("molt_lt", 2, ATTR_NONE),
    i64_ret("molt_le", 2, ATTR_NONE),
    i64_ret("molt_gt", 2, ATTR_NONE),
    i64_ret("molt_ge", 2, ATTR_NONE),
    i64_ret("molt_contains", 2, ATTR_NONE),
    i64_ret("molt_bit_and", 2, ATTR_NONE),
    i64_ret("molt_bit_or", 2, ATTR_NONE),
    i64_ret("molt_bit_xor", 2, ATTR_NONE),
    i64_ret("molt_lshift", 2, ATTR_NONE),
    i64_ret("molt_rshift", 2, ATTR_NONE),
    i64_ret("molt_inplace_bit_and", 2, ATTR_NONE),
    i64_ret("molt_inplace_bit_or", 2, ATTR_NONE),
    i64_ret("molt_inplace_bit_xor", 2, ATTR_NONE),
    void_ret(MOLT_INC_REF_OBJ.name, 1, ATTR_WILLRETURN),
    void_ret(MOLT_DEC_REF_OBJ.name, 1, ATTR_WILLRETURN),
    i64_ret("molt_alloc", 1, ATTR_WILLRETURN),
    i64_ret("molt_get_attr_name", 2, ATTR_NONE),
    custom(
        "molt_get_attr_object_ic",
        I64_PTR_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    i64_ret("molt_set_attr_name", 3, ATTR_NONE),
    i64_ret("molt_del_attr_name", 2, ATTR_NONE),
    i64_ret("molt_getitem_method", 2, ATTR_NONE),
    i64_ret("molt_getitem_unchecked", 2, ATTR_NONE),
    i64_ret("molt_setitem_method", 3, ATTR_NONE),
    i64_ret("molt_delitem_method", 2, ATTR_NONE),
    i64_ret("molt_slice_new", 3, ATTR_WILLRETURN),
    i64_ret("molt_iter_next", 1, ATTR_WILLRETURN),
    i64_ret("molt_classmethod_new", 1, ATTR_WILLRETURN),
    i64_ret("molt_staticmethod_new", 1, ATTR_WILLRETURN),
    i64_ret("molt_property_new", 3, ATTR_WILLRETURN),
    i64_ret("molt_raise", 1, ATTR_WILLRETURN),
    i64_ret("molt_exception_clear", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_last", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_last_pending", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_current", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_push", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_pop", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_stack_enter", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_stack_depth", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_stack_clear", 0, ATTR_WILLRETURN),
    i64_ret("molt_exception_enter_handler", 1, ATTR_WILLRETURN),
    i64_ret("molt_exception_stack_exit", 1, ATTR_WILLRETURN),
    i64_ret("molt_exception_stack_set_depth", 1, ATTR_WILLRETURN),
    i64_ret("molt_exception_resolve_captured", 1, ATTR_WILLRETURN),
    void_ret("molt_warn_stderr", 1, ATTR_WILLRETURN),
    void_ret("molt_print_newline", 0, ATTR_WILLRETURN),
    void_ret("molt_print_obj", 1, ATTR_WILLRETURN),
    i64_ret("molt_callargs_new", 2, ATTR_WILLRETURN),
    i64_ret("molt_callargs_push_pos", 2, ATTR_WILLRETURN),
    i64_ret("molt_call_bind", 2, ATTR_NONE),
    i64_ret("molt_call_bind_ic", 3, ATTR_NONE),
    i64_ret("molt_call_indirect_ic", 3, ATTR_NONE),
    i64_ret("molt_call_func_fast0", 1, ATTR_NONE),
    i64_ret("molt_call_func_fast1", 2, ATTR_NONE),
    i64_ret("molt_call_func_fast2", 3, ATTR_NONE),
    i64_ret("molt_call_func_fast3", 4, ATTR_NONE),
    i64_ret("molt_func_new", 3, ATTR_WILLRETURN),
    i64_ret("molt_func_new_builtin_named", 4, ATTR_WILLRETURN),
    i64_ret("molt_func_new_closure", 4, ATTR_WILLRETURN),
    i64_ret("molt_code_new", 9, ATTR_WILLRETURN),
    i64_ret("molt_code_slot_set", 2, ATTR_WILLRETURN),
    i64_ret("molt_code_slots_init", 1, ATTR_WILLRETURN),
    i64_ret("molt_trace_enter_slot", 1, ATTR_WILLRETURN),
    i64_ret("molt_frame_locals_set", 1, ATTR_WILLRETURN),
    i64_ret("molt_trace_set_line", 1, ATTR_WILLRETURN),
    i64_ret("molt_trace_exit", 0, ATTR_WILLRETURN),
    i64_ret("molt_fn_ptr_code_set", 2, ATTR_WILLRETURN),
    i64_ret("molt_function_defaults_version", 1, ATTR_WILLRETURN),
    custom(
        "molt_call_method_ic0",
        I64_I64_PTR_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_method_ic1",
        I64_I64_PTR_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_method_ic2",
        I64_I64_PTR_I64_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_method_ic3",
        I64_I64_PTR_I64_I64_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_method_ic4",
        I64_I64_PTR_I64_I64_I64_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_super_method_ic0",
        I64_I64_I64_PTR_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_super_method_ic1",
        I64_I64_I64_PTR_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_super_method_ic2",
        I64_I64_I64_PTR_I64_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_super_method_ic3",
        I64_I64_I64_PTR_I64_I64_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    custom(
        "molt_call_super_method_ic4",
        I64_I64_I64_PTR_I64_I64_I64_I64_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_NONE,
    ),
    i64_ret("molt_dict_new", 1, ATTR_WILLRETURN),
    i64_ret("molt_set_new", 1, ATTR_WILLRETURN),
    i64_ret("molt_module_import", 1, ATTR_NONE),
    i64_ret("molt_module_new", 1, ATTR_WILLRETURN),
    i64_ret("molt_module_cache_get", 1, ATTR_WILLRETURN),
    i64_ret("molt_module_cache_del", 1, ATTR_WILLRETURN),
    i64_ret("molt_module_cache_set", 2, ATTR_WILLRETURN),
    i64_ret("molt_module_get_attr", 2, ATTR_WILLRETURN),
    i64_ret("molt_module_import_from", 2, ATTR_WILLRETURN),
    i64_ret("molt_module_get_global", 2, ATTR_WILLRETURN),
    i64_ret("molt_module_get_name", 2, ATTR_WILLRETURN),
    i64_ret("molt_module_del_global", 2, ATTR_WILLRETURN),
    i64_ret("molt_module_del_global_if_present", 2, ATTR_WILLRETURN),
    i64_ret("molt_module_set_attr", 3, ATTR_WILLRETURN),
    i64_ret("molt_call_builtin", 2, ATTR_NONE),
    custom(
        "molt_string_from_bytes",
        PTR_I64_PTR,
        FixedRuntimeReturnAbi::I32,
        ATTR_WILLRETURN,
    ),
    custom(
        "molt_bigint_from_str",
        PTR_I64,
        FixedRuntimeReturnAbi::I64,
        ATTR_WILLRETURN,
    ),
    i64_ret("molt_list_builder_new", 1, ATTR_WILLRETURN),
    void_ret("molt_list_builder_append", 2, ATTR_WILLRETURN),
    i64_ret("molt_list_builder_finish", 1, ATTR_WILLRETURN),
    i64_ret("molt_tuple_builder_finish", 1, ATTR_WILLRETURN),
    i64_ret("molt_dict_builder_new", 1, ATTR_WILLRETURN),
    void_ret("molt_dict_builder_append", 3, ATTR_WILLRETURN),
    i64_ret("molt_dict_builder_finish", 1, ATTR_WILLRETURN),
    i64_ret("molt_set_builder_new", 1, ATTR_WILLRETURN),
    void_ret("molt_set_builder_append", 2, ATTR_WILLRETURN),
    i64_ret("molt_set_builder_finish", 1, ATTR_WILLRETURN),
    i64_ret("molt_exception_pending", 0, ATTR_WILLRETURN_MEMORY_READ),
];

#[cfg(feature = "llvm")]
pub(super) fn fixed_runtime_import(name: &str) -> Option<FixedRuntimeImportSpec> {
    FIXED_RUNTIME_IMPORTS
        .iter()
        .copied()
        .find(|spec| spec.name == name)
}

#[cfg(feature = "llvm")]
pub(super) fn fixed_runtime_import_return_abi(
    name: &str,
    param_count: usize,
) -> Option<RuntimeReturnAbi> {
    match fixed_runtime_import(name)?.signature {
        FixedRuntimeSignature::Boxed {
            param_count: fixed_param_count,
            return_abi,
        } if fixed_param_count == param_count => Some(return_abi),
        _ => None,
    }
}

#[cfg(feature = "llvm")]
pub(super) fn fixed_runtime_function_type<'ctx>(
    ctx: &'ctx Context,
    spec: FixedRuntimeImportSpec,
) -> FunctionType<'ctx> {
    let i64_ty = ctx.i64_type();
    let i32_ty = ctx.i32_type();
    match spec.signature {
        FixedRuntimeSignature::Boxed {
            param_count,
            return_abi,
        } => {
            let params: Vec<BasicMetadataTypeEnum<'ctx>> =
                (0..param_count).map(|_| i64_ty.into()).collect();
            match return_abi {
                RuntimeReturnAbi::I64 => i64_ty.fn_type(&params, false),
                RuntimeReturnAbi::Void => ctx.void_type().fn_type(&params, false),
            }
        }
        FixedRuntimeSignature::Custom { params, return_abi } => {
            let ptr_ty = ctx.ptr_type(AddressSpace::default());
            let param_types: Vec<BasicMetadataTypeEnum<'ctx>> = params
                .iter()
                .map(|param| match param {
                    FixedRuntimeParamAbi::I64 => i64_ty.into(),
                    FixedRuntimeParamAbi::Ptr => ptr_ty.into(),
                })
                .collect();
            match return_abi {
                FixedRuntimeReturnAbi::I64 => i64_ty.fn_type(&param_types, false),
                FixedRuntimeReturnAbi::I32 => i32_ty.fn_type(&param_types, false),
                FixedRuntimeReturnAbi::Void => ctx.void_type().fn_type(&param_types, false),
            }
        }
    }
}

#[cfg(feature = "llvm")]
pub(super) fn declare_fixed_runtime_import<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    spec: FixedRuntimeImportSpec,
) -> FunctionValue<'ctx> {
    if let Some(func) = module.get_function(spec.name) {
        return func;
    }
    let func = module.add_function(
        spec.name,
        fixed_runtime_function_type(ctx, spec),
        Some(inkwell::module::Linkage::External),
    );
    add_nounwind(ctx, func);
    if spec.attrs.willreturn {
        add_willreturn(ctx, func);
    }
    if spec.attrs.memory_read {
        add_memory_read(ctx, func);
    }
    func
}

#[cfg(feature = "llvm")]
pub(super) fn declare_fixed_runtime_function<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    name: &str,
) -> Option<FunctionValue<'ctx>> {
    fixed_runtime_import(name).map(|spec| declare_fixed_runtime_import(ctx, module, spec))
}

#[cfg(feature = "llvm")]
pub(super) fn declare_fixed_runtime_functions<'ctx>(ctx: &'ctx Context, module: &Module<'ctx>) {
    for &spec in FIXED_RUNTIME_IMPORTS {
        declare_fixed_runtime_import(ctx, module, spec);
    }
}
