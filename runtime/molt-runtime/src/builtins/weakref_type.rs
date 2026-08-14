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
    dec_ref_bits, exception_pending, inc_ref_bits, int_bits_from_i64, molt_weakref_find_nocallback,
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

fn weakref_receiver(_py: &crate::PyToken<'_>, self_bits: u64, method: &str) -> Option<*mut u8> {
    let self_obj = obj_from_bits(self_bits);
    if let Some(ptr) = self_obj.as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_WEAKREF
    {
        return Some(ptr);
    }
    let self_type = type_name(_py, self_obj);
    let message = format!(
        "descriptor '{method}' requires a 'weakref.ReferenceType' object but received a '{self_type}'"
    );
    raise_exception::<()>(_py, "TypeError", &message);
    None
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
    crate::with_gil_entry_nopanic!(_py, {
        if weakref_receiver(_py, self_bits, "__call__").is_none() {
            return MoltObject::none().bits();
        }
        crate::object::weakref::weakref_peek_owned(_py, self_bits)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_init(self_bits: u64, _target_bits: u64, _callback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if weakref_receiver(_py, self_bits, "__init__").is_none() {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_callback_get(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if weakref_receiver(_py, self_bits, "__callback__").is_none() {
            return MoltObject::none().bits();
        }
        crate::object::weakref::weakref_callback_owned(_py, self_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        weakref_richcompare(_py, self_bits, other_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_ne(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        weakref_richcompare(_py, self_bits, other_bits, true)
    })
}

fn weakref_richcompare(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
    is_ne: bool,
) -> u64 {
    let method = if is_ne { "__ne__" } else { "__eq__" };
    if weakref_receiver(_py, self_bits, method).is_none() {
        return MoltObject::none().bits();
    }
    let other_is_weakref = obj_from_bits(other_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_WEAKREF);
    if !other_is_weakref {
        return crate::builtins::methods::not_implemented_bits(_py);
    }
    let left = crate::object::weakref::weakref_peek_owned(_py, self_bits);
    let right = crate::object::weakref::weakref_peek_owned(_py, other_bits);
    let result = if let (Some(left), Some(right)) = (left, right) {
        let name_bits = if is_ne {
            crate::intern_static_name(_py, &crate::runtime_state(_py).interned.ne_name, b"__ne__")
        } else {
            crate::intern_static_name(_py, &crate::runtime_state(_py).interned.eq_name, b"__eq__")
        };
        match crate::object::ops_compare::rich_compare_value(
            _py,
            obj_from_bits(left),
            obj_from_bits(right),
            name_bits,
            name_bits,
        ) {
            crate::object::ops_compare::CompareValueOutcome::Value(bits) => bits,
            crate::object::ops_compare::CompareValueOutcome::Error => MoltObject::none().bits(),
            crate::object::ops_compare::CompareValueOutcome::NotComparable => {
                MoltObject::from_bool(if is_ne { left != right } else { left == right }).bits()
            }
        }
    } else {
        MoltObject::from_bool(if is_ne {
            self_bits != other_bits
        } else {
            self_bits == other_bits
        })
        .bits()
    };
    if let Some(left) = left {
        dec_ref_bits(_py, left);
    }
    if let Some(right) = right {
        dec_ref_bits(_py, right);
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_weakref_hash(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if weakref_receiver(_py, self_bits, "__hash__").is_none() {
            return MoltObject::none().bits();
        }
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
        let Some(self_ptr) = weakref_receiver(_py, self_bits, "__repr__") else {
            return MoltObject::none().bits();
        };
        let target = crate::object::weakref::weakref_peek_owned(_py, self_bits);
        let text = if let Some(target_bits) = target {
            let target_ptr = obj_from_bits(target_bits)
                .as_ptr()
                .unwrap_or_else(|| std::process::abort());
            let target_type = type_name(_py, obj_from_bits(target_bits));
            let target_class = type_of_bits(_py, target_bits);
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
                dec_ref_bits(_py, target_bits);
                return MoltObject::none().bits();
            }
            text
        } else {
            format!("<weakref at 0x{:x}; dead>", self_ptr as usize)
        };
        if let Some(target_bits) = target {
            dec_ref_bits(_py, target_bits);
        }
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}
