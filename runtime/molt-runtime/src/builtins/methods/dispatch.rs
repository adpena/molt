use super::common::{
    builtin_classmethod_bits, builtin_func_bits, builtin_func_bits_with_defaults_tuple,
};
use super::core_types::{
    memoryview_method_bits, object_method_bits, range_method_bits, type_method_bits,
};
use super::io::file_method_bits;
use super::numeric::{
    complex_class_method_bits, complex_method_bits, float_class_method_bits, float_method_bits,
    int_class_method_bits, int_method_bits,
};
use super::sequence::{
    bytearray_method_bits, bytes_method_bits, slice_method_bits, string_method_bits,
};
use super::singletons::missing_bits;
use super::specialized::property_method_bits;
use crate::PyToken;
use crate::builtins::containers::tuple_method_bits;
use crate::*;

pub(crate) fn builtin_class_method_bits(
    _py: &PyToken<'_>,
    class_bits: u64,
    name: &str,
) -> Option<u64> {
    let builtins = builtin_classes(_py);
    if name == "__class_getitem__"
        && (class_bits == builtins.list
            || class_bits == builtins.dict
            || class_bits == builtins.tuple
            || class_bits == builtins.set
            || class_bits == builtins.frozenset
            || class_bits == builtins.type_obj)
    {
        return Some(builtin_classmethod_bits(
            _py,
            &runtime_state(_py).method_cache.generic_alias_class_getitem,
            fn_addr!(molt_generic_alias_new),
            2,
        ));
    }
    if class_bits == builtins.tuple && name == "__new__" {
        return Some(builtin_func_bits_with_defaults_tuple(
            _py,
            &runtime_state(_py).method_cache.tuple_new,
            fn_addr!(molt_tuple_new_bound),
            2,
            &[missing_bits(_py)],
        ));
    }
    if class_bits == builtins.generic_alias && name == "__new__" {
        return Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generic_alias_new,
            fn_addr!(molt_generic_alias_type_new),
            3,
        ));
    }
    if class_bits == builtins.reference_type && name == "__new__" {
        return Some(builtin_func_bits_with_defaults_tuple(
            _py,
            &runtime_state(_py).method_cache.weakref_new,
            fn_addr!(molt_weakref_new),
            3,
            &[MoltObject::none().bits()],
        ));
    }
    if issubclass_bits(class_bits, builtins.reference_type) {
        if name == "__new__" {
            return Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.weakref_new,
                fn_addr!(molt_weakref_new),
                3,
                &[MoltObject::none().bits()],
            ));
        }
        if let Some(bits) = crate::builtins::methods::weakref_method_bits(_py, name) {
            return Some(bits);
        }
    }
    if class_bits == builtins.object {
        return object_method_bits(_py, name);
    }
    if class_bits == builtins.type_obj {
        return type_method_bits(_py, name);
    }
    if class_bits == builtins.int {
        if let Some(bits) = int_method_bits(_py, name) {
            return Some(bits);
        }
        if let Some(bits) = int_class_method_bits(_py, name) {
            return Some(bits);
        }
    }
    if class_bits == builtins.float {
        if let Some(bits) = float_method_bits(_py, name) {
            return Some(bits);
        }
        if let Some(bits) = float_class_method_bits(_py, name) {
            return Some(bits);
        }
    }
    if class_bits == builtins.complex {
        if let Some(bits) = complex_method_bits(_py, name) {
            return Some(bits);
        }
        if let Some(bits) = complex_class_method_bits(_py, name) {
            return Some(bits);
        }
    }
    if class_bits == builtins.base_exception || class_bits == builtins.exception {
        return exception_method_bits(_py, name);
    }
    if class_bits == builtins.base_exception_group || class_bits == builtins.exception_group {
        return exception_group_method_bits(_py, name);
    }
    if class_bits == builtins.dict {
        return dict_method_bits(_py, name);
    }
    if class_bits == builtins.tuple {
        return tuple_method_bits(_py, name);
    }
    if class_bits == builtins.list {
        return list_method_bits(_py, name);
    }
    if class_bits == builtins.set {
        return set_method_bits(_py, name);
    }
    if class_bits == builtins.frozenset {
        return frozenset_method_bits(_py, name);
    }
    if class_bits == builtins.str {
        return string_method_bits(_py, name);
    }
    if class_bits == builtins.bytes {
        return bytes_method_bits(_py, name);
    }
    if class_bits == builtins.bytearray {
        return bytearray_method_bits(_py, name);
    }
    if class_bits == builtins.slice {
        return slice_method_bits(_py, name);
    }
    if class_bits == builtins.memoryview {
        return memoryview_method_bits(_py, name);
    }
    if class_bits == builtins.range {
        return range_method_bits(_py, name);
    }
    if class_bits == builtins.property {
        return property_method_bits(_py, name);
    }
    if class_bits == builtins.file_io {
        // FileIO(name, mode='r', closefd=True, opener=None)
        // __defaults__ = (None, None, None) for the last 3 params
        let none = MoltObject::none().bits();
        match name {
            "__new__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.file_io_new,
                    fn_addr!(molt_file_io_new),
                    5,
                    &[none, none, none],
                ));
            }
            "__init__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.file_io_init,
                    fn_addr!(molt_file_io_init),
                    5,
                    &[none, none, none],
                ));
            }
            _ => {}
        }
    }
    if class_bits == builtins.buffered_reader
        || class_bits == builtins.buffered_writer
        || class_bits == builtins.buffered_random
    {
        // BufferedReader(raw, buffer_size=-1)
        let neg_one = MoltObject::from_int(-1).bits();
        match name {
            "__new__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.buffered_new,
                    fn_addr!(molt_buffered_new),
                    3,
                    &[neg_one],
                ));
            }
            "__init__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.buffered_init,
                    fn_addr!(molt_buffered_init),
                    3,
                    &[neg_one],
                ));
            }
            _ => {}
        }
    }
    if class_bits == builtins.text_io_wrapper {
        // TextIOWrapper(buffer, encoding=None, errors=None, newline=None,
        //               line_buffering=False, write_through=False)
        let none = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        match name {
            "__new__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.text_io_wrapper_new,
                    fn_addr!(molt_text_io_wrapper_new),
                    7,
                    &[none, none, none, false_bits, false_bits],
                ));
            }
            "__init__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.text_io_wrapper_init,
                    fn_addr!(molt_text_io_wrapper_init),
                    7,
                    &[none, none, none, false_bits, false_bits],
                ));
            }
            _ => {}
        }
    }
    if class_bits == builtins.bytes_io {
        // BytesIO(initial_bytes=None)
        let none = MoltObject::none().bits();
        match name {
            "__new__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.bytes_io_new,
                    fn_addr!(molt_bytesio_new),
                    2,
                    &[none],
                ));
            }
            "__init__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.bytes_io_init,
                    fn_addr!(molt_bytesio_init),
                    2,
                    &[none],
                ));
            }
            _ => {}
        }
    }
    if class_bits == builtins.string_io {
        // StringIO(initial_value='', newline=None)
        let none = MoltObject::none().bits();
        match name {
            "__new__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.string_io_new,
                    fn_addr!(molt_stringio_new),
                    3,
                    &[none, none],
                ));
            }
            "__init__" => {
                return Some(builtin_func_bits_with_defaults_tuple(
                    _py,
                    &runtime_state(_py).method_cache.string_io_init,
                    fn_addr!(molt_stringio_init),
                    3,
                    &[none, none],
                ));
            }
            _ => {}
        }
    }
    if class_bits == builtins.file
        || class_bits == builtins.file_io
        || class_bits == builtins.buffered_reader
        || class_bits == builtins.buffered_writer
        || class_bits == builtins.buffered_random
        || class_bits == builtins.text_io_wrapper
        || class_bits == builtins.bytes_io
        || class_bits == builtins.string_io
    {
        if name == "reconfigure" && class_bits != builtins.text_io_wrapper {
            return None;
        }
        return file_method_bits(_py, name);
    }
    if is_builtin_class_bits(_py, class_bits) {
        return object_method_bits(_py, name);
    }
    None
}
