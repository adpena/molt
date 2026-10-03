//! Published builtin constructors own argument contracts and native allocation.
//! type.__call__ uses the same __new__/__init__ lifecycle for exact classes and
//! heap subtypes. Explicit descriptors run only their named constructor phase.

use crate::builtins::native_arguments::NativeArguments as Arguments;
use crate::object::native_instance::NativePayload;
use crate::*;

/// These classes publish both phases through ordinary native descriptors.
/// The binder uses this publication boundary, never a second argument parser.
pub(crate) fn owns_constructor_descriptors(py: &PyToken<'_>, class: u64) -> bool {
    let b = builtin_classes(py);
    [
        b.list,
        b.dict,
        b.set,
        b.frozenset,
        b.tuple,
        b.str,
        b.bytes,
        b.bytearray,
        b.int,
        b.float,
        b.complex,
        b.memoryview,
    ]
    .contains(&class)
}

unsafe fn owner(py: &PyToken<'_>, call: &Arguments<'_, '_>, base: u64) -> Option<(u64, *mut u8)> {
    unsafe {
        let (class, ptr) = crate::builtins::type_ops::native_constructor_receiver(
            py,
            base,
            call.positional.first().copied(),
            &class_name_for_error(base),
        )?;
        crate::object::class_finish_definition(py, ptr).ok()?;
        Some((class, ptr))
    }
}

/// tuple/float/frozenset tp_new tolerate keywords only when a subtype's custom
/// tp_init consumes them (CPython clinic wrappers and frozenset_new).
unsafe fn custom_init(py: &PyToken<'_>, class: u64, base: u64) -> bool {
    unsafe {
        if class == base {
            return false;
        }
        let name = intern_static_name(py, &runtime_state(py).interned.init_name, b"__init__");
        let class = obj_from_bits(class).as_ptr().unwrap();
        let base = obj_from_bits(base).as_ptr().unwrap();
        class_attr_lookup_raw_mro(py, class, name) != class_attr_lookup_raw_mro(py, base, name)
    }
}

fn pointer_bits(ptr: *mut u8) -> u64 {
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn codec_arguments(py: &PyToken<'_>, label: &str, bound: &[Option<u64>; 3]) -> bool {
    for (index, name) in [(1, "encoding"), (2, "errors")] {
        let Some(bits) = bound[index] else { continue };
        let Some(ptr) = obj_from_bits(bits)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        else {
            raise_exception::<()>(
                py,
                "TypeError",
                &format!(
                    "{label}() argument '{name}' must be str, not {}",
                    type_name(py, obj_from_bits(bits))
                ),
            );
            return false;
        };
        let value = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        // Argument Clinic exports strict UTF-8 before checking embedded NUL.
        if !crate::object::ops_string::require_strict_utf8(py, bits) {
            return false;
        }
        if value.contains(&0) {
            raise_exception::<()>(py, "ValueError", "embedded null character");
            return false;
        }
    }
    true
}

pub(crate) extern "C" fn list_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some((_, class)) = (unsafe { owner(py, &call, builtin_classes(py).list) }) else {
            return MoltObject::none().bits();
        };
        unsafe { crate::call::class_init::alloc_instance_for_class(py, class) }
    })
}

pub(crate) extern "C" fn dict_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some((bits, class)) = (unsafe { owner(py, &call, builtin_classes(py).dict) }) else {
            return MoltObject::none().bits();
        };
        if bits == builtin_classes(py).dict {
            molt_dict_new(0)
        } else {
            unsafe { crate::call::class_init::alloc_instance_for_class(py, class) }
        }
    })
}

pub(crate) extern "C" fn set_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = call.new_receiver(py, builtin_classes(py).set) else {
            return MoltObject::none().bits();
        };
        pointer_bits(unsafe {
            crate::object::builders::alloc_native_set(py, class, NativePayload::Set, None)
        })
    })
}

pub(crate) extern "C" fn bytearray_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = call.new_receiver(py, builtin_classes(py).bytearray) else {
            return MoltObject::none().bits();
        };
        pointer_bits(unsafe { crate::object::builders::alloc_native_bytearray(py, class) })
    })
}

pub(crate) extern "C" fn tuple_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = call.new_receiver(py, builtin_classes(py).tuple) else {
            return MoltObject::none().bits();
        };
        if unsafe { NativePayload::Tuple.admit(py, class) }.is_none() {
            return MoltObject::none().bits();
        }
        let Some(value) = call.positional_only(py, "tuple", unsafe {
            custom_init(py, class, builtin_classes(py).tuple)
        }) else {
            return MoltObject::none().bits();
        };
        crate::molt_tuple_new_bound(class, value)
    })
}

pub(crate) extern "C" fn frozenset_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = call.new_receiver(py, builtin_classes(py).frozenset) else {
            return MoltObject::none().bits();
        };
        if unsafe { NativePayload::Frozenset.admit(py, class) }.is_none() {
            return MoltObject::none().bits();
        }
        let Some(value) = call.positional_only(py, "frozenset", unsafe {
            custom_init(py, class, builtin_classes(py).frozenset)
        }) else {
            return MoltObject::none().bits();
        };
        let result = if value == missing_bits(py) {
            molt_frozenset_new(0)
        } else {
            unsafe { frozenset_from_iter_bits(py, value) }
                .unwrap_or_else(|| MoltObject::none().bits())
        };
        if exception_pending(py) {
            dec_ref_bits(py, result);
            return MoltObject::none().bits();
        }
        if class == builtin_classes(py).frozenset {
            return result;
        }
        let Some(source) = obj_from_bits(result).as_ptr() else {
            return result;
        };
        let ptr = unsafe {
            crate::object::builders::alloc_native_set(
                py,
                class,
                NativePayload::Frozenset,
                Some(source),
            )
        };
        dec_ref_bits(py, result);
        pointer_bits(ptr)
    })
}

pub(crate) extern "C" fn int_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some((class, _)) = (unsafe { owner(py, &call, builtin_classes(py).int) }) else {
            return MoltObject::none().bits();
        };
        let Some(bound) = call.named(py, "int", ["", "base"], 0) else {
            return MoltObject::none().bits();
        };
        let [value, base] = *bound;
        if value.is_none() && base.is_some() {
            return raise_exception::<_>(py, "TypeError", "int() missing string argument");
        }
        molt_int_new(
            class,
            value.unwrap_or_else(|| MoltObject::from_int(0).bits()),
            base.unwrap_or_else(|| missing_bits(py)),
        )
    })
}

pub(crate) extern "C" fn float_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some((class, _)) = (unsafe { owner(py, &call, builtin_classes(py).float) }) else {
            return MoltObject::none().bits();
        };
        let Some(value) = call.positional_only(py, "float", unsafe {
            custom_init(py, class, builtin_classes(py).float)
        }) else {
            return MoltObject::none().bits();
        };
        molt_float_new(
            class,
            if value == missing_bits(py) {
                MoltObject::from_float(0.0).bits()
            } else {
                value
            },
        )
    })
}

pub(crate) extern "C" fn memoryview_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        if unsafe { owner(py, &call, builtin_classes(py).memoryview) }.is_none() {
            return MoltObject::none().bits();
        }
        let Some(bound) = call.named(py, "memoryview", ["object"], 1) else {
            return MoltObject::none().bits();
        };
        let [value] = *bound;
        crate::molt_memoryview_new(value.expect("required constructor argument"))
    })
}

pub(crate) extern "C" fn complex_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = call.new_receiver(py, builtin_classes(py).complex) else {
            return MoltObject::none().bits();
        };
        if unsafe { NativePayload::Complex.admit(py, class) }.is_none() {
            return MoltObject::none().bits();
        }
        let Some(bound) = call.named(py, "complex", ["real", "imag"], 0) else {
            return MoltObject::none().bits();
        };
        let [real, imag] = *bound;
        let result = molt_complex_from_obj(
            real.unwrap_or_else(|| MoltObject::from_int(0).bits()),
            imag.unwrap_or_else(|| MoltObject::from_int(0).bits()),
            MoltObject::from_int(i64::from(imag.is_some())).bits(),
        );
        if exception_pending(py) {
            dec_ref_bits(py, result);
            return MoltObject::none().bits();
        }
        if class == builtin_classes(py).complex {
            return result;
        }
        let Some(source) = complex_ptr_from_bits(result) else {
            dec_ref_bits(py, result);
            return MoltObject::none().bits();
        };
        let parts = unsafe { *complex_ref(source) };
        let ptr = unsafe {
            crate::object::native_instance::alloc_unpublished(py, class, NativePayload::Complex, 0)
        };
        if !ptr.is_null() {
            unsafe {
                ptr.cast::<crate::builtins::numbers::ComplexParts>()
                    .write(parts);
            }
        }
        let ptr = unsafe { crate::object::native_instance::publish(py, ptr, class) };
        dec_ref_bits(py, result);
        pointer_bits(ptr)
    })
}

pub(crate) extern "C" fn str_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = call.new_receiver(py, builtin_classes(py).str) else {
            return MoltObject::none().bits();
        };
        if unsafe { NativePayload::String.admit(py, class) }.is_none() {
            return MoltObject::none().bits();
        }
        let Some(bound) = call.named(py, "str", ["object", "encoding", "errors"], 0) else {
            return MoltObject::none().bits();
        };
        if !codec_arguments(py, "str", &bound) {
            return MoltObject::none().bits();
        }
        let result = match bound[0] {
            None => pointer_bits(alloc_string(py, b"")),
            Some(value) if bound[1].is_none() && bound[2].is_none() => molt_str_from_obj(value),
            Some(value) => decode_string(py, value, bound[1], bound[2]),
        };
        native_text_result(py, class, NativePayload::String, result)
    })
}

pub(crate) extern "C" fn bytes_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(class) = call.new_receiver(py, builtin_classes(py).bytes) else {
            return MoltObject::none().bits();
        };
        if unsafe { NativePayload::Bytes.admit(py, class) }.is_none() {
            return MoltObject::none().bits();
        }
        let Some(bound) = call.named(py, "bytes", ["source", "encoding", "errors"], 0) else {
            return MoltObject::none().bits();
        };
        if !codec_arguments(py, "bytes", &bound) {
            return MoltObject::none().bits();
        }
        let result = bytes_value(py, *bound);
        native_text_result(py, class, NativePayload::Bytes, result)
    })
}

fn bytes_value(py: &PyToken<'_>, bound: [Option<u64>; 3]) -> u64 {
    match bound {
        [None, None, None] => pointer_bits(alloc_bytes(py, &[])),
        [Some(value), None, None] => molt_bytes_from_obj(value),
        [Some(value), Some(encoding), errors] => molt_bytes_from_str(
            value,
            encoding,
            errors.unwrap_or_else(|| MoltObject::none().bits()),
        ),
        [source, encoding, _] => {
            let message = if encoding.is_some() {
                "encoding without a string argument"
            } else if source.is_some_and(|source| {
                obj_from_bits(source)
                    .as_ptr()
                    .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
            }) {
                "string argument without an encoding"
            } else {
                "errors without a string argument"
            };
            raise_exception::<_>(py, "TypeError", message)
        }
    }
}

fn native_text_result(py: &PyToken<'_>, class: u64, kind: NativePayload, result: u64) -> u64 {
    if exception_pending(py) {
        dec_ref_bits(py, result);
        return MoltObject::none().bits();
    }
    if class == kind.owner(py) {
        return result;
    }
    let Some(source) = obj_from_bits(result).as_ptr() else {
        return result;
    };
    let out = unsafe {
        let bytes = if kind == NativePayload::String {
            std::slice::from_raw_parts(string_bytes(source), string_len(source))
        } else {
            bytes_like_slice_raw(source).unwrap()
        };
        crate::object::builders::alloc_native_inline_bytes(py, class, kind, bytes)
    };
    dec_ref_bits(py, result);
    pointer_bits(out)
}

fn decode_string(py: &PyToken<'_>, value: u64, encoding: Option<u64>, errors: Option<u64>) -> u64 {
    use crate::object::buffer_exports::ScopedBuffer;
    let buffer = match ScopedBuffer::new(py, value) {
        Ok(buffer) => buffer,
        Err(error) => return error.raise(py, "decoding to str: need a bytes-like object"),
    };
    let len = match buffer.contiguous_len() {
        Ok(len) => len,
        Err(crate::object::buffer_exports::BufferAccessError::Pending) => {
            return MoltObject::none().bits();
        }
        Err(_) => {
            return raise_exception::<_>(
                py,
                "BufferError",
                "underlying buffer is not C-contiguous",
            );
        }
    };
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(buffer.view().data, len) }
    };
    let copied = pointer_bits(alloc_bytes(py, bytes));
    drop(buffer);
    if exception_pending(py) {
        dec_ref_bits(py, copied);
        return MoltObject::none().bits();
    }
    let out = molt_bytes_decode(
        copied,
        encoding.unwrap_or_else(|| MoltObject::none().bits()),
        errors.unwrap_or_else(|| MoltObject::none().bits()),
    );
    dec_ref_bits(py, copied);
    out
}

pub(crate) extern "C" fn list_init(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(receiver) = call.receiver(py) else {
            return MoltObject::none().bits();
        };
        if crate::object::ops_list::list_receiver(py, receiver, "__init__").is_none() {
            return MoltObject::none().bits();
        }
        let Some(value) = call.positional_only(py, "list", false) else {
            return MoltObject::none().bits();
        };
        molt_list_init_method(receiver, value)
    })
}

pub(crate) extern "C" fn dict_init(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(receiver) = call.receiver(py) else {
            return MoltObject::none().bits();
        };
        let Some(storage) = obj_from_bits(receiver)
            .as_ptr()
            .and_then(|ptr| unsafe { crate::object::ops::dict_like_bits_from_ptr(py, ptr) })
        else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "descriptor '__init__' requires a 'dict' object",
            );
        };
        if call.values().len() > 1 {
            return raise_exception::<_>(py, "TypeError", "dict expected at most 1 argument");
        }
        inc_ref_bits(py, storage);
        let _storage = PtrDropGuard::new(obj_from_bits(storage).as_ptr().unwrap());
        if let Some(&value) = call.values().first() {
            unsafe {
                dict_update_apply(py, storage, dict_update_set_in_place, value);
            }
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
        }
        for (key, value) in call.keyword_pairs() {
            unsafe {
                dict_update_set_in_place(py, storage, key, value);
            }
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

pub(crate) extern "C" fn set_init(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(receiver) = call.receiver(py) else {
            return MoltObject::none().bits();
        };
        if !obj_from_bits(receiver)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SET })
        {
            return raise_exception::<_>(
                py,
                "TypeError",
                "descriptor '__init__' requires a 'set' object",
            );
        }
        let Some(value) = call.positional_only(py, "set", false) else {
            return MoltObject::none().bits();
        };
        molt_set_clear(receiver);
        if !exception_pending(py) && value != missing_bits(py) {
            molt_set_update(receiver, value);
        }
        MoltObject::none().bits()
    })
}

pub(crate) extern "C" fn bytearray_init(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) = Arguments::read(py, "constructor", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(receiver) = call.receiver(py) else {
            return MoltObject::none().bits();
        };
        let Some(bound) = call.named(py, "bytearray", ["source", "encoding", "errors"], 0) else {
            return MoltObject::none().bits();
        };
        if !codec_arguments(py, "bytearray", &bound) {
            return MoltObject::none().bits();
        }
        crate::object::ops_bytes::bytearray_init_from_arguments(py, receiver, *bound)
    })
}
