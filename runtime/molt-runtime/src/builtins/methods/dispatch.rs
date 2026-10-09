use super::common::{builtin_func_bits, builtin_func_bits_with_defaults_tuple};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

super::native_method_table!(builtin_special_method_bits, publish_special_methods, py, name, [class_bits], {
    let builtins = builtin_classes(py);
    let none = MoltObject::none().bits();
}, {
    "__class_getitem__" if [builtins.list, builtins.dict, builtins.tuple, builtins.set, builtins.frozenset, builtins.type_obj].contains(&class_bits) => Some(builtin_func_bits(
        py, NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, class_bits, "__class_getitem__"), fn_addr!(molt_generic_alias_new), 2,
    )),
    "__get__" if [builtins.method_descriptor, builtins.wrapper_descriptor, builtins.classmethod_descriptor].contains(&class_bits) => Some(builtin_func_bits_with_defaults_tuple(
        py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class_bits, "__get__"),
        fn_addr!(molt_function_descriptor_get), 3, &[none],
    )),
    "__new__" => {
        let spec = NativeCallableSpec::constructor(class_bits);
        if class_bits == builtins.generic_alias {
            Some(builtin_func_bits(py, spec, fn_addr!(molt_generic_alias_type_new), 3))
        } else if class_bits == builtins.reference_type {
            Some(crate::builtins::methods::builtin_variadic_func_bits(py, spec, fn_addr!(crate::builtins::weakref_type::weakref_new_method)))
        } else if class_bits == builtins.file_io {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_file_io_new), 5, &[none, none, none]))
        } else if [builtins.buffered_reader, builtins.buffered_writer, builtins.buffered_random].contains(&class_bits) {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_buffered_new), 3, &[MoltObject::from_int(-1).bits()]))
        } else if class_bits == builtins.text_io_wrapper {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_text_io_wrapper_new), 7, &[none, none, none, MoltObject::from_bool(false).bits(), MoltObject::from_bool(false).bits()]))
        } else if class_bits == builtins.bytes_io {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_bytesio_new), 2, &[none]))
        } else if class_bits == builtins.string_io {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_stringio_new), 3, &[none, none]))
        } else { None }
    },
    "__init__" => {
        let spec = NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class_bits, "__init__");
        if class_bits == builtins.file_io {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_file_io_init), 5, &[none, none, none]))
        } else if [builtins.buffered_reader, builtins.buffered_writer, builtins.buffered_random].contains(&class_bits) {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_buffered_init), 3, &[MoltObject::from_int(-1).bits()]))
        } else if class_bits == builtins.text_io_wrapper {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_text_io_wrapper_init), 7, &[none, none, none, MoltObject::from_bool(false).bits(), MoltObject::from_bool(false).bits()]))
        } else if class_bits == builtins.bytes_io {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_bytesio_init), 2, &[none]))
        } else if class_bits == builtins.string_io {
            Some(builtin_func_bits_with_defaults_tuple(py, spec, fn_addr!(molt_stringio_init), 3, &[none, none]))
        } else { None }
    },
});

/// Routing is shared by direct lookup and requested-owner namespace publication.
/// The method arms, not a second list of spellings, define each public surface.
macro_rules! native_method_families {
    ($( $field:ident => $lookup:path, $publish:path; )*) => {
        fn declared_method(py: &PyToken<'_>, class: u64, name: &str) -> Option<u64> {
            let classes = builtin_classes(py);
            $(if class == classes.$field { return $lookup(py, name); })*
            None
        }
        fn publish_declared_methods(py: &PyToken<'_>, class: u64) -> bool {
            let classes = builtin_classes(py);
            $(if class == classes.$field { return $publish(py); })*
            true
        }
    };
}

native_method_families! {
    dict_keyiterator => super::specialized::dict_keyiterator_method_bits, super::specialized::publish_dict_keyiterator_methods;
    dict_valueiterator => super::specialized::dict_valueiterator_method_bits, super::specialized::publish_dict_valueiterator_methods;
    dict_itemiterator => super::specialized::dict_itemiterator_method_bits, super::specialized::publish_dict_itemiterator_methods;
    dict_reversekeyiterator => super::specialized::dict_reversekeyiterator_method_bits, super::specialized::publish_dict_reversekeyiterator_methods;
    dict_reversevalueiterator => super::specialized::dict_reversevalueiterator_method_bits, super::specialized::publish_dict_reversevalueiterator_methods;
    dict_reverseitemiterator => super::specialized::dict_reverseitemiterator_method_bits, super::specialized::publish_dict_reverseitemiterator_methods;
    set_iterator => super::specialized::set_iterator_method_bits, super::specialized::publish_set_iterator_methods;
    union_type => super::specialized::union_method_bits, super::specialized::publish_union_methods;
    dict_keys => super::specialized::dict_keys_method_bits, super::specialized::publish_dict_keys_methods;
    dict_items => super::specialized::dict_items_method_bits, super::specialized::publish_dict_items_methods;
    dict_values => super::specialized::dict_values_method_bits, super::specialized::publish_dict_values_methods;
    generic_alias => super::specialized::generic_alias_method_bits, super::specialized::publish_generic_alias_methods;
    object => super::core_types::object_method_bits, super::core_types::publish_object_methods;
    type_obj => super::core_types::type_method_bits, super::core_types::publish_type_methods;
    module => crate::builtins::modules::module_method_bits, crate::builtins::modules::publish_module_methods;
    int => super::numeric::int_method_bits, super::numeric::publish_int_methods;
    bool => super::numeric::bool_method_bits, super::numeric::publish_bool_methods;
    float => super::numeric::float_method_bits, super::numeric::publish_float_methods;
    complex => super::numeric::complex_method_bits, super::numeric::publish_complex_methods;
    dict => crate::builtins::containers::dict_method_bits, crate::builtins::containers::publish_dict_methods;
    tuple => crate::builtins::containers::tuple_method_bits, crate::builtins::containers::publish_tuple_methods;
    list => crate::builtins::containers::list_method_bits, crate::builtins::containers::publish_list_methods;
    set => crate::builtins::containers::set_method_bits, crate::builtins::containers::publish_set_methods;
    frozenset => crate::builtins::containers::frozenset_method_bits, crate::builtins::containers::publish_frozenset_methods;
    str => super::sequence::string_method_bits, super::sequence::publish_string_methods;
    bytes => super::sequence::bytes_method_bits, super::sequence::publish_bytes_methods;
    bytearray => super::sequence::bytearray_method_bits, super::sequence::publish_bytearray_methods;
    slice => super::sequence::slice_method_bits, super::sequence::publish_slice_methods;
    memoryview => super::core_types::memoryview_method_bits, super::core_types::publish_memoryview_methods;
    range => super::core_types::range_method_bits, super::core_types::publish_range_methods;
    staticmethod => super::specialized::staticmethod_method_bits, super::specialized::publish_staticmethod_methods;
    classmethod => super::specialized::classmethod_method_bits, super::specialized::publish_classmethod_methods;
    property => super::specialized::property_method_bits, super::specialized::publish_property_methods;
    reference_type => super::specialized::weakref_method_bits, super::specialized::publish_weakref_methods;
    generator => super::specialized::generator_method_bits, super::specialized::publish_generator_methods;
    coroutine => super::specialized::coroutine_method_bits, super::specialized::publish_coroutine_methods;
    coroutine_wrapper => super::specialized::coroutine_wrapper_method_bits, super::specialized::publish_coroutine_wrapper_methods;
    async_generator => super::specialized::asyncgen_method_bits, super::specialized::publish_asyncgen_methods;
}

fn io_class(py: &PyToken<'_>, class: u64) -> bool {
    let b = builtin_classes(py);
    [
        b.file,
        b.file_io,
        b.buffered_reader,
        b.buffered_writer,
        b.buffered_random,
        b.text_io_wrapper,
        b.bytes_io,
        b.string_io,
    ]
    .contains(&class)
}

pub(crate) fn builtin_class_method_bits(py: &PyToken<'_>, class: u64, name: &str) -> Option<u64> {
    super::method_dispatch(py, || {
        if let Some(bits) = builtin_special_method_bits(py, class, name) {
            return Some(bits);
        }
        if exception_pending(py) {
            return None;
        }
        if let Some(bits) = declared_method(py, class, name) {
            return Some(bits);
        }
        if exception_pending(py) {
            return None;
        }
        let b = builtin_classes(py);
        let extra = if class == b.int {
            super::numeric::int_class_method_bits(py, name)
        } else if class == b.float {
            super::numeric::float_class_method_bits(py, name)
        } else if class == b.complex {
            super::numeric::complex_class_method_bits(py, name)
        } else if issubclass_bits(class, b.base_exception) {
            if name == "__init__" {
                let owner = if class == b.exception_group {
                    b.base_exception_group
                } else {
                    class
                };
                crate::builtins::exceptions::exception_method_bits_for_owner(py, owner, name)
            } else if class == b.base_exception_group || class == b.exception_group {
                crate::builtins::exceptions::exception_group_method_bits(py, name)
            } else if class == b.base_exception {
                crate::builtins::exceptions::exception_method_bits(py, name)
            } else {
                None
            }
        } else if io_class(py, class) {
            super::io::file_method_bits(py, class, name)
        } else {
            None
        };
        if extra.is_some() || exception_pending(py) {
            return extra;
        }
        None
    })
}

pub(crate) fn publish_builtin_class_methods(py: &PyToken<'_>, class: u64) -> bool {
    if !is_builtin_class_bits(py, class)
        && !crate::builtins::exceptions::is_builtin_exception_class_bits(py, class)
    {
        return true;
    }
    let Some(pointer) = obj_from_bits(class).as_ptr() else {
        return false;
    };
    use crate::object::class_storage::{ClassDeclaration, class_declare, class_declares};
    if unsafe { class_declares(pointer, ClassDeclaration::NativeNamespacePublished) } {
        return true;
    }
    if !publish_special_methods(py, class) || !publish_declared_methods(py, class) {
        return false;
    }
    let b = builtin_classes(py);
    let complete = if class == b.int {
        super::numeric::publish_int_class_methods(py)
    } else if class == b.float {
        super::numeric::publish_float_class_methods(py)
    } else if class == b.complex {
        super::numeric::publish_complex_class_methods(py)
    } else if issubclass_bits(class, b.base_exception) {
        if !crate::builtins::exceptions::publish_exception_fields(py, class) {
            return false;
        }
        let owner = if class == b.exception_group {
            b.base_exception_group
        } else {
            class
        };
        let _ = crate::builtins::exceptions::exception_method_bits_for_owner(py, owner, "__init__");
        if exception_pending(py) {
            return false;
        }
        if class == b.base_exception {
            crate::builtins::exceptions::publish_exception_methods(py)
        } else if class == b.base_exception_group || class == b.exception_group {
            crate::builtins::exceptions::publish_exception_group_methods(py)
        } else {
            true
        }
    } else if io_class(py, class) {
        super::io::publish_file_methods(py, class)
    } else {
        true
    };
    if complete && !exception_pending(py) {
        unsafe { class_declare(pointer, ClassDeclaration::NativeNamespacePublished) };
        true
    } else {
        false
    }
}
