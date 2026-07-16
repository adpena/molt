//! Runtime-native `_weakref.ReferenceType` object-model authority.
//!
//! Construction, exact callback-free interning, subtype allocation, and the
//! core protocol live here.  The Python `weakref` module builds containers and
//! proxy/finalize policy on top; it does not manufacture the heap kind.

use molt_obj_model::MoltObject;

use crate::builtins::attributes::attr_lookup_ptr;
use crate::builtins::classes::builtin_classes;
use crate::builtins::type_ops::{issubclass_bits, type_of_bits};
use crate::call::class_init::alloc_instance_for_class;
use crate::{
    TYPE_ID_STRING, TYPE_ID_TYPE, TYPE_ID_WEAKREF, alloc_string, attr_name_bits_from_bytes,
    dec_ref_bits, exception_pending, inc_ref_bits, int_bits_from_i64, molt_eq,
    molt_weakref_callback, molt_weakref_find_nocallback, molt_weakref_get, molt_weakref_peek,
    molt_weakref_register, obj_from_bits, object_type_id, raise_exception, string_obj_to_owned,
    type_name,
};

fn referent_name(_py: &crate::PyToken<'_>, target_ptr: *mut u8) -> Option<String> {
    let name_attr = attr_name_bits_from_bytes(_py, b"__name__")?;
    let name_bits = unsafe { attr_lookup_ptr(_py, target_ptr, name_attr) };
    dec_ref_bits(_py, name_attr);
    let name_bits = name_bits?;
    let name = obj_from_bits(name_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_STRING })
        .and_then(|_| string_obj_to_owned(obj_from_bits(name_bits)));
    dec_ref_bits(_py, name_bits);
    name
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_reference_type() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits = builtin_classes(_py).reference_type;
        inc_ref_bits(_py, bits);
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_new(class_bits: u64, target_bits: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref.__new__ expects type");
        };
        if unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
            || !issubclass_bits(class_bits, builtins.reference_type)
        {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "weakref.__new__(type): type is not a subtype of ReferenceType",
            );
        }
        if class_bits == builtins.reference_type && obj_from_bits(callback_bits).is_none() {
            let cached = molt_weakref_find_nocallback(target_bits);
            if !obj_from_bits(cached).is_none() || exception_pending(_py) {
                return cached;
            }
        }
        let instance_bits = unsafe { alloc_instance_for_class(_py, class_ptr) };
        let Some(instance_ptr) = obj_from_bits(instance_bits).as_ptr() else {
            return instance_bits;
        };
        if unsafe { object_type_id(instance_ptr) } != TYPE_ID_WEAKREF {
            dec_ref_bits(_py, instance_bits);
            return raise_exception::<_>(
                _py,
                "SystemError",
                "ReferenceType allocation did not produce the WEAKREF heap kind",
            );
        }
        let registered = molt_weakref_register(instance_bits, target_bits, callback_bits);
        if exception_pending(_py) || obj_from_bits(registered).as_bool() != Some(true) {
            dec_ref_bits(_py, instance_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(_py, "SystemError", "weakref registration was not unique");
        }
        instance_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_call(self_bits: u64) -> u64 {
    molt_weakref_get(self_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_init(self_bits: u64, _target_bits: u64, _callback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let valid = obj_from_bits(self_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_WEAKREF });
        if !valid {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "weakref.__init__ expects ReferenceType",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_callback_get(self_bits: u64) -> u64 {
    molt_weakref_callback(self_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let other_is_weakref = obj_from_bits(other_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_WEAKREF });
        if !other_is_weakref {
            return MoltObject::from_bool(false).bits();
        }
        let left = molt_weakref_peek(self_bits);
        let right = molt_weakref_peek(other_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, left);
            dec_ref_bits(_py, right);
            return MoltObject::none().bits();
        }
        let result = if obj_from_bits(left).is_none() || obj_from_bits(right).is_none() {
            MoltObject::from_bool(self_bits == other_bits).bits()
        } else {
            molt_eq(left, right)
        };
        dec_ref_bits(_py, left);
        dec_ref_bits(_py, right);
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_hash(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hash = crate::object::weakref::weakref_cached_hash_or_compute(_py, self_bits);
        if exception_pending(_py) {
            MoltObject::none().bits()
        } else {
            int_bits_from_i64(_py, hash)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "weakref repr expects ReferenceType");
        };
        let target = molt_weakref_peek(self_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let text = if let Some(target_ptr) = obj_from_bits(target).as_ptr() {
            let target_type = type_name(_py, obj_from_bits(target));
            let target_class = type_of_bits(_py, target);
            let target_name = if target_class == builtin_classes(_py).type_obj {
                "type".to_string()
            } else {
                target_type.into_owned()
            };
            let mut text = format!(
                "<weakref at 0x{:x}; to '{}' at 0x{:x}>",
                self_ptr as usize, target_name, target_ptr as usize
            );
            if let Some(name) = referent_name(_py, target_ptr) {
                text.pop();
                text.push_str(" (");
                text.push_str(&name);
                text.push_str(")>");
            } else if exception_pending(_py) {
                dec_ref_bits(_py, target);
                return MoltObject::none().bits();
            }
            text
        } else {
            format!("<weakref at 0x{:x}; dead>", self_ptr as usize)
        };
        dec_ref_bits(_py, target);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}
