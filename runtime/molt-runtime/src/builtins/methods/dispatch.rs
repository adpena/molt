use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, builtin_func_bits_with_signature,
};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

super::native_method_table!(builtin_special_method_bits, publish_special_methods, py, name, [class_bits], {
    let builtins = builtin_classes(py);
    let none = MoltObject::none().bits();
}, {
    "__buffer__" => builtin_buffer_method_bits(py, class_bits, false),
    "__release_buffer__" => builtin_buffer_method_bits(py, class_bits, true),
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
        } else if class_bits == builtins.function {
            Some(crate::builtins::methods::builtin_variadic_func_bits(
                py,
                spec.with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(crate::builtins::types::native_constructors::function_new),
            ))
        } else if class_bits == builtins.reference_type {
            Some(crate::builtins::methods::builtin_variadic_func_bits(py, spec, fn_addr!(crate::builtins::weakref_type::weakref_new_method)))
        } else { io_constructor_bits(py, class_bits, true) }
    },
    "__init__" => io_constructor_bits(py, class_bits, false),
});

// These runtime builtin namespaces have process-owned physical buffer slots.
// Array and extension classes already publish their declarations through their
// own readiness/explicit declaration transaction; inheritance uses normal MRO
// lookup. No namespace enumeration or C readiness is needed for one lazy name.
fn builtin_buffer_method_bits(py: &PyToken<'_>, class: u64, release: bool) -> Option<u64> {
    let builtins = builtin_classes(py);
    if ![builtins.bytes, builtins.bytearray, builtins.memoryview].contains(&class) {
        return None;
    }
    unsafe {
        use molt_cpython_abi::api::{refcount::OwnedPyObject, typeobj};
        let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let owner = bridge
            .handle_to_borrowed_pyobj(class)
            .cast::<molt_cpython_abi::abi_types::PyTypeObject>();
        if owner.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "buffer declaration owner");
            return Some(0);
        }
        let slot = if release {
            molt_cpython_abi::type_slots::Py_bf_releasebuffer
        } else {
            molt_cpython_abi::type_slots::Py_bf_getbuffer
        };
        // bytes has no release callback. Absence is a physical declaration
        // fact, not a synthetic callable that ignores an unsupported operation.
        if typeobj::PyType_GetSlot(owner, slot).is_null() {
            return None;
        }
        let name = if release {
            "__release_buffer__"
        } else {
            "__buffer__"
        };
        Some(super::common::init_native_callable(
            py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, name),
            || {
                let descriptor =
                    OwnedPyObject::from_owned(typeobj::buffer_slot_wrapper(owner, release));
                if descriptor.as_ptr().is_null() {
                    crate::cpython_abi_hooks::propagate_native_failure(py, "buffer declaration");
                    return 0;
                }
                match bridge.molt_value_for_pyobj(descriptor.as_ptr()) {
                    Some(bits) => bits,
                    None => {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            py,
                            "buffer declaration projection",
                        );
                        0
                    }
                }
            },
        ))
    }
}

// IO phases enter the ordinary type-call binder. BufferedWriter allocation
// accepts construction arguments without converting them; its initializer owns
// the declared raw/size contract and all user callbacks.
fn io_constructor_bits(py: &PyToken<'_>, class: u64, is_new: bool) -> Option<u64> {
    let b = builtin_classes(py);
    let none = MoltObject::none().bits();
    let receiver: &[u8] = if is_new { b"cls" } else { b"self" };
    let spec = if is_new {
        NativeCallableSpec::constructor(class)
    } else {
        NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, "__init__")
    };
    let bind = |new, init, names: &[&[u8]], defaults: &[u64]| {
        Some(builtin_func_bits_with_signature(
            py,
            spec,
            if is_new { new } else { init },
            names.len() as u64,
            defaults,
            1,
            names,
            false,
            false,
        ))
    };
    if class == b.buffered_reader && is_new {
        return Some(crate::builtins::methods::builtin_variadic_func_bits(
            py,
            spec,
            fn_addr!(crate::builtins::io::buffered_reader_new_method),
        ));
    }
    if class == b.buffered_writer && is_new {
        return Some(crate::builtins::methods::builtin_variadic_func_bits(
            py,
            spec,
            fn_addr!(crate::builtins::io::buffered_writer_new_method),
        ));
    }
    if class == b.file_io {
        // The internal missing singleton distinguishes omission from explicit None.
        let mode = missing_bits(py);
        if obj_from_bits(mode).is_none() {
            return Some(0);
        }
        bind(
            fn_addr!(molt_file_io_new),
            fn_addr!(molt_file_io_init),
            &[receiver, b"file", b"mode", b"closefd", b"opener"],
            &[mode, MoltObject::from_bool(true).bits(), none],
        )
    } else if [b.buffered_reader, b.buffered_writer, b.buffered_random].contains(&class) {
        bind(
            fn_addr!(molt_buffered_new),
            fn_addr!(molt_buffered_init),
            &[receiver, b"raw", b"buffer_size"],
            &[MoltObject::from_int(crate::builtins::io::default_buffer_size(py)).bits()],
        )
    } else if class == b.text_io_wrapper {
        bind(
            fn_addr!(molt_text_io_wrapper_new),
            fn_addr!(molt_text_io_wrapper_init),
            &[
                receiver,
                b"buffer",
                b"encoding",
                b"errors",
                b"newline",
                b"line_buffering",
                b"write_through",
            ],
            &[
                none,
                none,
                none,
                MoltObject::from_bool(false).bits(),
                MoltObject::from_bool(false).bits(),
            ],
        )
    } else if class == b.bytes_io {
        bind(
            fn_addr!(molt_bytesio_new),
            fn_addr!(molt_bytesio_init),
            &[receiver, b"initial_bytes"],
            &[none],
        )
    } else if class == b.string_io {
        bind(
            fn_addr!(molt_stringio_new),
            fn_addr!(molt_stringio_init),
            &[receiver, b"initial_value", b"newline"],
            &[none, none],
        )
    } else {
        None
    }
}

/// Routing is shared by direct lookup and requested-owner namespace publication.
/// The method arms, not a second list of spellings, define each public surface.
macro_rules! native_method_families {
    (@lookup $py:ident, $name:ident) => { Ok(None) };
    (@lookup $py:ident, $name:ident, $lookup:path) => { $lookup($py, $name) };
    (@publish $py:ident) => { true };
    (@publish $py:ident, $publish:path) => { $publish($py) };
    (iterators { $( $iterator:ident $(=> $iter_lookup:path, $iter_publish:path)?; )* }
     ordinary { $( $field:ident => $lookup:path, $publish:path; )* }) => {
        fn declared_method(py: &PyToken<'_>, class: u64, name: &str) -> Result<Option<u64>, ()> {
            let classes = builtin_classes(py);
            $(if class == classes.$iterator {
                if let Some(method) = native_iterator_method_bits(py, class, name)? {
                    return Ok(Some(method));
                }
                return native_method_families!(@lookup py, name $(, $iter_lookup)?);
            })*
            $(if class == classes.$field { return $lookup(py, name); })*
            Ok(None)
        }
        fn publish_declared_methods(py: &PyToken<'_>, class: u64) -> bool {
            let classes = builtin_classes(py);
            $(if class == classes.$iterator {
                if !publish_native_iterator_methods(py, class) { return false; }
                return native_method_families!(@publish py $(, $iter_publish)?);
            })*
            $(if class == classes.$field { return $publish(py); })*
            true
        }
    };
}

// A single declaration owns both lookup and class namespace publication. The
// descriptor owner is the routed class; admission uses NativeCallableSpec's
// ordinary receiver check before entering the shared native iteration engine.
super::native_method_table!(native_iterator_method_bits, publish_native_iterator_methods,
    py, name, [class], {}, {
    "__iter__" => Some(super::builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, "__iter__")
            .with_text_signature("($self, /)"),
        fn_addr!(crate::object::ops_iter::builtin_iter_slot), 1)),
    "__next__" => Some(super::builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, "__next__")
            .with_text_signature("($self, /)"),
        fn_addr!(crate::object::ops_iter::builtin_next_slot), 1)),
});

native_method_families! {
  iterators {
    iterator;
    callable_iterator;
    bytes_iterator;
    bytearray_iterator;
    range_iterator;
    longrange_iterator;
    str_iterator;
    str_ascii_iterator;
    tuple_iterator;
    enumerate;
    reversed;
    zip;
    map;
    filter;
    list_iterator => super::specialized::list_iterator_method_bits, super::specialized::publish_list_iterator_methods;
    list_reverseiterator => super::specialized::list_reverseiterator_method_bits, super::specialized::publish_list_reverseiterator_methods;
    dict_keyiterator => super::specialized::dict_keyiterator_method_bits, super::specialized::publish_dict_keyiterator_methods;
    dict_valueiterator => super::specialized::dict_valueiterator_method_bits, super::specialized::publish_dict_valueiterator_methods;
    dict_itemiterator => super::specialized::dict_itemiterator_method_bits, super::specialized::publish_dict_itemiterator_methods;
    dict_reversekeyiterator => super::specialized::dict_reversekeyiterator_method_bits, super::specialized::publish_dict_reversekeyiterator_methods;
    dict_reversevalueiterator => super::specialized::dict_reversevalueiterator_method_bits, super::specialized::publish_dict_reversevalueiterator_methods;
    dict_reverseitemiterator => super::specialized::dict_reverseitemiterator_method_bits, super::specialized::publish_dict_reverseitemiterator_methods;
    set_iterator => super::specialized::set_iterator_method_bits, super::specialized::publish_set_iterator_methods;
  }
  ordinary {
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
    async_generator_asend => super::specialized::asyncgen_asend_method_bits, super::specialized::publish_asyncgen_asend_methods;
    async_generator_athrow => super::specialized::asyncgen_athrow_method_bits, super::specialized::publish_asyncgen_athrow_methods;
  }
}

fn io_class(py: &PyToken<'_>, class: u64) -> bool {
    let b = builtin_classes(py);
    [
        b.io_base,
        b.raw_io_base,
        b.file,
        b.file_io,
        b.buffered_io_base,
        b.text_io_base,
        b.buffered_reader,
        b.buffered_writer,
        b.buffered_random,
        b.text_io_wrapper,
        b.bytes_io,
        b.string_io,
    ]
    .contains(&class)
}

pub(crate) fn builtin_class_method_bits(
    py: &PyToken<'_>,
    class: u64,
    name: &str,
) -> Result<Option<u64>, ()> {
    {
        if let Some(bits) = builtin_special_method_bits(py, class, name)? {
            return Ok(Some(bits));
        }
        if let Some(bits) = declared_method(py, class, name)? {
            return Ok(Some(bits));
        }
        let b = builtin_classes(py);
        if class == b.int {
            super::numeric::int_class_method_bits(py, name)
        } else if class == b.float {
            super::numeric::float_class_method_bits(py, name)
        } else if class == b.complex {
            super::numeric::complex_class_method_bits(py, name)
        } else if issubclass_bits(class, b.base_exception) {
            if matches!(name, "__str__" | "__repr__") {
                crate::builtins::exceptions::exception_render_method_bits(py, class, name)
            } else if name == "__init__" {
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
                Ok(None)
            }
        } else if io_class(py, class) {
            super::io::file_method_bits(py, class, name)
        } else {
            Ok(None)
        }
    }
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
    crate::builtins::exceptions::with_saved_raised_exception(py, || {
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
            if !crate::builtins::exceptions::publish_exception_fields(py, class)
                || !crate::builtins::exceptions::publish_exception_render_methods(py, class)
            {
                return false;
            }
            let owner = if class == b.exception_group {
                b.base_exception_group
            } else {
                class
            };
            if crate::builtins::exceptions::exception_method_bits_for_owner(py, owner, "__init__")
                .is_err()
            {
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
    })
}
