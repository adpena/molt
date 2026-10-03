//! Indexed sequence reads shared by IMPORT_STAR and the boxed C-API.
//! Mapping subscription and iterator exhaustion have different contracts.
use crate::*;
use crate::object::class_storage::{ClassDeclaration, class_declares};

#[derive(Clone, Copy)]
pub(crate) enum SequenceReadSlot {
    Builtin,
    Special,
}

/// Managed sq_item admission is class-owned metadata. Native mapping methods
/// do not create a sequence slot; an ordinary heap class's explicit declaration
/// does, including an alias to dict.__getitem__. No descriptor is bound here.
pub(crate) fn sequence_read_slot(py: &PyToken<'_>, receiver: u64) -> Option<SequenceReadSlot> {
    let class = obj_from_bits(type_of_bits(py, receiver)).as_ptr()?;
    unsafe {
        for &base in crate::builtins::type_ops::class_mro_view(py, class).iter() {
            let Some(base) = obj_from_bits(base).as_ptr() else {
                continue;
            };
            if class_declares(base, ClassDeclaration::NativeSlotLayout) {
                if class_declares(base, ClassDeclaration::NativeSequenceItem) {
                    return Some(SequenceReadSlot::Builtin);
                }
            } else if obj_from_bits(class_dict_bits(base)).as_ptr().is_some_and(|namespace| {
                dict_get_str_bytes_borrowed(py, namespace, b"__getitem__").is_some()
            }) {
                return Some(SequenceReadSlot::Special);
            }
        }
    }
    None
}

/// New-reference result; a pending error returns None. Only callers implementing
/// sequence exhaustion may clear IndexError. This routine never consumes it.
pub(crate) fn sequence_item_at_index(py: &PyToken<'_>, receiver: u64, mut index: i64) -> u64 {
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    inc_ref_bits(py, receiver);
    let _receiver_owner = obj_from_bits(receiver).as_ptr().map(PtrDropGuard::new);
    if let Some(ptr) = obj_from_bits(receiver).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_FOREIGN
    {
        let Ok(native_index) = isize::try_from(index) else {
            return raise_exception::<_>(py, "OverflowError", "cannot fit 'int' into an index-sized integer");
        };
        let result = unsafe {
            molt_cpython_abi::bridge::molt_foreign_sequence_item(
                crate::object::foreign::foreign_ptr_from_obj(ptr), native_index,
            )
        };
        return match result.decode() {
            molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => bits,
            _ => {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native sequence read");
                MoltObject::none().bits()
            }
        };
    }
    let slot = sequence_read_slot(py, receiver);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let Some(slot) = slot else {
        let mapping = unsafe { crate::builtins::attr::has_special_method(py, receiver, b"__getitem__") };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let name = class_name_for_error(type_of_bits(py, receiver));
        let message = if mapping {
            format!("{name} is not a sequence")
        } else {
            format!("'{name}' object does not support indexing")
        };
        return raise_exception::<_>(py, "TypeError", &message);
    };
    if index < 0 {
        // Normalize exactly once, after sq_item admission. An inherited native
        // item slot still observes an overridden heap-class length slot.
        let has_length = matches!(slot, SequenceReadSlot::Builtin)
            || unsafe { crate::builtins::attr::has_special_method(py, receiver, b"__len__") };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if has_length {
            let length_bits = molt_len(receiver);
            let _length_owner = obj_from_bits(length_bits).as_ptr().map(PtrDropGuard::new);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let Some(length) = to_i64(obj_from_bits(length_bits)) else {
                return raise_exception::<_>(py, "OverflowError", "cannot fit 'int' into an index-sized integer");
            };
            if length < 0 {
                return raise_exception::<_>(py, "ValueError", "__len__() should return >= 0");
            }
            index += length;
        }
    }
    let index_bits = int_bits_from_i64(py, index);
    let _index_owner = obj_from_bits(index_bits).as_ptr().map(PtrDropGuard::new);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    match slot {
        SequenceReadSlot::Builtin => crate::object::ops::molt_sequence_item_builtin(receiver, index_bits),
        SequenceReadSlot::Special => {
            let method = unsafe {
                crate::builtins::attr::lookup_special_method(py, receiver, b"__getitem__")
            };
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let Some(method) = method else {
                let name = class_name_for_error(type_of_bits(py, receiver));
                return raise_exception::<_>(py, "TypeError", &format!("'{name}' object is not subscriptable"));
            };
            let value = unsafe { call_callable1(py, method, index_bits) };
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, method));
            value
        }
    }
}
