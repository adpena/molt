//! Python ModuleType construction shares storage and initialization with imports.

use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::builtins::methods::{builtin_func_bits, builtin_variadic_func_bits};
use crate::builtins::types::{call_vararg_args, call_vararg_kwargs};
use crate::object::layout::module_set_name_bits;
use crate::*;

crate::builtins::methods::native_method_table!(module_method_bits, publish_module_methods, py, name, [], {


}, {
        "__new__" => Some(builtin_variadic_func_bits(
                py,
            NativeCallableSpec::constructor(builtin_classes(py).module).with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(molt_module_type_new),
            )),
        "__init__" => Some(builtin_variadic_func_bits(
                py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).module, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_module_init),
            )),
        "__getattribute__" => Some(builtin_func_bits(
                py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).module, "__getattribute__").with_text_signature("($self, name, /)"),
                fn_addr!(molt_module_getattribute),
                2,
            )),
        "__setattr__" => Some(builtin_func_bits(
                py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).object, "__setattr__").with_text_signature("($self, name, value, /)"),
                fn_addr!(molt_module_setattr),
                3,
            )),
        "__delattr__" => Some(builtin_func_bits(
                py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).object, "__delattr__").with_text_signature("($self, name, /)"),
                fn_addr!(molt_module_delattr),
                2,
            )),
        "__dir__" => Some(builtin_func_bits(
                py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).module, "__dir__"),
                fn_addr!(molt_module_dir),
                1,
            )),
        "__repr__" => Some(builtin_func_bits(
                py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).module, "__repr__").with_text_signature("($self, /)"),
                fn_addr!(molt_module_repr),
                1,
            )),
});

/// Dictionary replacement may finalize a displaced value and reenter Python.
/// Keep all borrowed initializer inputs and the namespace alive until it ends.
struct ModuleInputs<'a, 'py> {
    py: &'a PyToken<'py>,
    values: [u64; 4],
}

impl Drop for ModuleInputs<'_, '_> {
    fn drop(&mut self) {
        for value in self.values {
            dec_ref_bits(self.py, value);
        }
    }
}

pub(crate) fn initialize_module_namespace(
    py: &PyToken<'_>,
    module_bits: u64,
    name_bits: u64,
    doc_bits: u64,
) -> u64 {
    let Some(module_ptr) = obj_from_bits(module_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_MODULE })
    else {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "descriptor '__init__' requires a 'module' object but received a '{}'",
                type_name(py, obj_from_bits(module_bits)),
            ),
        );
    };
    if !obj_from_bits(name_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
    {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "module() argument 'name' must be str, not {}",
                type_name(py, obj_from_bits(name_bits)),
            ),
        );
    }
    let namespace_bits = unsafe { module_dict_bits(module_ptr) };
    let Some(namespace_ptr) = obj_from_bits(namespace_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_DICT })
    else {
        return raise_exception::<_>(py, "SystemError", "module namespace is not a dictionary");
    };
    let inputs = ModuleInputs {
        py,
        values: [module_bits, name_bits, doc_bits, namespace_bits],
    };
    for value in inputs.values {
        inc_ref_bits(py, value);
    }
    let none = MoltObject::none().bits();
    for (name, value) in [
        (b"__name__".as_slice(), name_bits),
        (b"__doc__".as_slice(), doc_bits),
        (b"__package__".as_slice(), none),
        (b"__loader__".as_slice(), none),
        (b"__spec__".as_slice(), none),
    ] {
        let Some(key) = attr_name_bits_from_bytes(py, name) else {
            return none;
        };
        unsafe { dict_set_in_place(py, namespace_ptr, key, value) };
        dec_ref_bits(py, key);
        if exception_pending(py) {
            return none;
        }
    }
    unsafe { module_set_name_bits(py, module_ptr, name_bits) };
    none
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_type_new(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(args) = call_vararg_args(py, "__new__", args_bits) else {
            return MoltObject::none().bits();
        };
        if call_vararg_kwargs(py, "__new__", kwargs_bits).is_none() {
            return MoltObject::none().bits();
        }
        let Some((_, class_ptr)) = crate::builtins::type_ops::native_constructor_receiver(
            py,
            builtin_classes(py).module,
            args.first().copied(),
            "module",
        ) else {
            return MoltObject::none().bits();
        };
        // __new__ only allocates. Unlike import allocation it neither validates
        // a module name nor redirects through a loader's active identity.
        unsafe { alloc_instance_for_class(py, class_ptr) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_init(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = crate::builtins::native_arguments::NativeArguments::read(
            py,
            "__init__",
            args_bits,
            kwargs_bits,
        ) else {
            return MoltObject::none().bits();
        };
        let Some(&receiver) = call.positional.first() else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "descriptor '__init__' of 'module' object needs an argument",
            );
        };
        // Descriptor receiver validation precedes argument parsing.
        if !obj_from_bits(receiver)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_MODULE })
        {
            return raise_exception::<_>(
                py,
                "TypeError",
                &format!(
                    "descriptor '__init__' requires a 'module' object but received a '{}'",
                    type_name(py, obj_from_bits(receiver)),
                ),
            );
        }
        let Some(bound) = call.named(py, "module", ["name", "doc"], 1) else {
            return MoltObject::none().bits();
        };
        let [name, doc] = *bound;
        initialize_module_namespace(
            py,
            receiver,
            name.expect("required name admitted"),
            doc.unwrap_or_else(|| MoltObject::none().bits()),
        )
    })
}
