//! Runtime access to the existing managed and CPython tuple storage owners.
//! Admission is physical; explicit base slots never redispatch subclass methods.

use crate::*;
use molt_cpython_abi::abi_types::{PyMappingMethods, PyObject, PySequenceMethods, PyTuple_Type};
use molt_cpython_abi::api::abstract_sequence::{BinaryFunc, ObjObjProc, SsizeArgFunc};
use molt_cpython_abi::api::{errors, refcount::OwnedPyObject, sequences};
use molt_cpython_abi::bridge::{GLOBAL_BRIDGE, owned_native_result_to_runtime};
use molt_cpython_abi::hooks::DecodedHandleResult;
use molt_obj_model::sequence_compare::RichCompareOp;

#[cfg(test)]
mod tests;

/// Typed view of the immutable root tuple's published physical operations.
/// The public ABI tables store opaque C pointers; all signature admission lives
/// here, using the same aliases as the canonical PySequence_* dispatcher.
struct TupleSlots {
    subscript: BinaryFunc,
    concat: BinaryFunc,
    item: SsizeArgFunc,
    repeat: SsizeArgFunc,
    contains: ObjObjProc,
}

impl TupleSlots {
    fn published(py: &PyToken<'_>) -> Option<Self> {
        unsafe {
            let root = &raw const PyTuple_Type;
            let sequence = (*root).tp_as_sequence.cast::<PySequenceMethods>();
            let mapping = (*root).tp_as_mapping.cast::<PyMappingMethods>();
            if sequence.is_null() || mapping.is_null() {
                return raise_exception(
                    py,
                    "SystemError",
                    "tuple physical slot tables are missing",
                );
            }
            let (subscript, concat, item, repeat, contains) = (
                (*mapping).mp_subscript,
                (*sequence).sq_concat,
                (*sequence).sq_item,
                (*sequence).sq_repeat,
                (*sequence).sq_contains,
            );
            if [subscript, concat, item, repeat, contains]
                .iter()
                .any(|slot| slot.is_null())
            {
                return raise_exception(py, "SystemError", "tuple physical slot is missing");
            }
            // CPython binaryfunc, ssizeargfunc, and objobjproc respectively.
            // These exact fields are populated by the immutable tuple root's
            // initializer in abi_types; no receiver-provided pointer enters.
            Some(Self {
                subscript: std::mem::transmute::<*mut std::ffi::c_void, BinaryFunc>(subscript),
                concat: std::mem::transmute::<*mut std::ffi::c_void, BinaryFunc>(concat),
                item: std::mem::transmute::<*mut std::ffi::c_void, SsizeArgFunc>(item),
                repeat: std::mem::transmute::<*mut std::ffi::c_void, SsizeArgFunc>(repeat),
                contains: std::mem::transmute::<*mut std::ffi::c_void, ObjObjProc>(contains),
            })
        }
    }
}

#[derive(Clone, Copy)]
enum Storage {
    Managed(*mut u8),
    Native(*mut PyObject),
}

/// A retained tuple receiver. Native storage remains owned by the ABI; this
/// adapter never reads its fields or creates a second tuple projection.
pub(crate) struct TupleStorage<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
    storage: Storage,
}

pub(crate) fn native_tuple(bits: u64) -> Option<*mut PyObject> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_FOREIGN {
        return None;
    }
    let native = std::ptr::with_exposed_provenance_mut(unsafe {
        crate::object::foreign::foreign_ptr_from_obj(ptr)
    });
    unsafe { sequences::tuple_layout_object(native) }.map(|_| native)
}

pub(crate) fn is_tuple(bits: u64) -> bool {
    obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE)
        || native_tuple(bits).is_some()
}

/// Managed tuple items borrow from the retained immutable receiver during
/// comparison. Native projection owns a runtime handle and releases it here.
pub(crate) struct TupleComparisonItem<'tuple, 'a, 'py> {
    receiver: &'tuple TupleStorage<'a, 'py>,
    bits: u64,
    owned: bool,
}

impl TupleComparisonItem<'_, '_, '_> {
    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }
}

impl Drop for TupleComparisonItem<'_, '_, '_> {
    fn drop(&mut self) {
        if self.owned {
            errors::with_preserved_error(|| dec_ref_bits(self.receiver.py, self.bits));
        }
    }
}

impl<'a, 'py> TupleStorage<'a, 'py> {
    pub(crate) fn from_bits(py: &'a PyToken<'py>, bits: u64) -> Option<Self> {
        let ptr = obj_from_bits(bits).as_ptr()?;
        let storage = if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
            Storage::Managed(ptr)
        } else {
            Storage::Native(native_tuple(bits)?)
        };
        inc_ref_bits(py, bits);
        Some(Self { py, bits, storage })
    }

    pub(crate) fn admit(py: &'a PyToken<'py>, bits: u64, method: &str) -> Option<Self> {
        if Self::pending(py) {
            return None;
        }
        Self::from_bits(py, bits).or_else(|| {
            raise_exception(
                py,
                "TypeError",
                &format!(
                    "descriptor '{method}' requires a 'tuple' object but received a '{}'",
                    type_name(py, obj_from_bits(bits)),
                ),
            )
        })
    }

    fn pending(py: &PyToken<'_>) -> bool {
        if !unsafe { errors::PyErr_Occurred() }.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "tuple operation");
        }
        exception_pending(py)
    }

    pub(crate) fn len(&self) -> Option<usize> {
        if Self::pending(self.py) {
            return None;
        }
        match self.storage {
            Storage::Managed(ptr) => Some(unsafe { crate::object::seq_access::len(ptr) }),
            Storage::Native(ptr) => {
                let len = unsafe { sequences::PyTuple_GET_SIZE(ptr) };
                if len < 0 || Self::pending(self.py) {
                    crate::cpython_abi_hooks::propagate_native_failure(self.py, "tuple length");
                    None
                } else {
                    Some(len as usize)
                }
            }
        }
    }

    /// An in-bounds element transfers one owned runtime reference. Missing or
    /// uninitialized storage is an error, never clean iterator exhaustion.
    pub(crate) fn item(&self, index: usize) -> Option<u64> {
        if Self::pending(self.py) {
            return None;
        }
        match self.storage {
            Storage::Managed(ptr) => {
                unsafe { crate::object::seq_access::pin_item(self.py, ptr, index) }
                    .map(|item| item.into_bits())
                    .or_else(|| {
                        raise_exception(self.py, "SystemError", "invalid tuple item storage")
                    })
            }
            Storage::Native(ptr) => {
                let borrowed = unsafe { sequences::PyTuple_GetItem(ptr, index as isize) };
                let owned = unsafe { molt_cpython_abi::api::object::Py_XNewRef(borrowed) };
                self.result(owned)
            }
        }
    }

    pub(crate) fn comparison_item(&self, index: usize) -> Option<TupleComparisonItem<'_, 'a, 'py>> {
        if Self::pending(self.py) {
            return None;
        }
        let (bits, owned) = match self.storage {
            Storage::Managed(ptr) => (
                unsafe { crate::object::seq_access::item(ptr, index) }.or_else(|| {
                    raise_exception(self.py, "SystemError", "invalid tuple comparison storage")
                })?,
                false,
            ),
            Storage::Native(_) => (self.item(index)?, true),
        };
        Some(TupleComparisonItem {
            receiver: self,
            bits,
            owned,
        })
    }

    fn project(&self, bits: u64) -> Option<OwnedPyObject> {
        if Self::pending(self.py) {
            return None;
        }
        let object =
            unsafe { OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)) };
        if object.as_ptr().is_null() || Self::pending(self.py) {
            crate::cpython_abi_hooks::propagate_native_failure(self.py, "tuple operand projection");
            None
        } else {
            Some(object)
        }
    }

    fn result(&self, object: *mut PyObject) -> Option<u64> {
        match unsafe { owned_native_result_to_runtime(object) }.decode() {
            DecodedHandleResult::Ok(bits) => Some(bits),
            _ => {
                crate::cpython_abi_hooks::propagate_native_failure(self.py, "tuple slot result");
                None
            }
        }
    }

    pub(crate) fn getitem(&self, key: u64, normalize_negative: bool) -> u64 {
        let invoke = || -> Option<u64> {
            let tuple = self.project(self.bits)?;
            let key = self.project(key)?;
            let slots = TupleSlots::published(self.py)?;
            let result = unsafe {
                if normalize_negative {
                    (slots.subscript)(tuple.as_ptr(), key.as_ptr())
                } else {
                    let index = molt_cpython_abi::api::abstract_number::PyNumber_AsSsize_t(
                        key.as_ptr(),
                        (&raw mut molt_cpython_abi::abi_types::PyExc_IndexError).cast(),
                    );
                    if Self::pending(self.py) {
                        return None;
                    }
                    (slots.item)(tuple.as_ptr(), index)
                }
            };
            self.result(result)
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }

    pub(crate) fn contains(&self, value: u64) -> u64 {
        let invoke = || -> Option<u64> {
            let tuple = self.project(self.bits)?;
            let value = self.project(value)?;
            let slots = TupleSlots::published(self.py)?;
            let status = unsafe { (slots.contains)(tuple.as_ptr(), value.as_ptr()) };
            if status < 0 || Self::pending(self.py) {
                crate::cpython_abi_hooks::propagate_native_failure(self.py, "tuple containment");
                None
            } else {
                Some(MoltObject::from_bool(status != 0).bits())
            }
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }

    pub(crate) fn concat(&self, other: u64) -> u64 {
        if !is_tuple(other) {
            return crate::object::ops_arith::native_slots::SequenceConcatKind::Tuple
                .raise(self.py, self.bits, other);
        }
        let invoke = || -> Option<u64> {
            let tuple = self.project(self.bits)?;
            let other = self.project(other)?;
            let slots = TupleSlots::published(self.py)?;
            self.result(unsafe { (slots.concat)(tuple.as_ptr(), other.as_ptr()) })
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }

    pub(crate) fn repeat(&self, count: isize) -> u64 {
        let invoke = || -> Option<u64> {
            let tuple = self.project(self.bits)?;
            let slots = TupleSlots::published(self.py)?;
            self.result(unsafe { (slots.repeat)(tuple.as_ptr(), count) })
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }

    pub(crate) fn compare(&self, other: u64, op: RichCompareOp) -> u64 {
        if !is_tuple(other) {
            return crate::builtins::methods::not_implemented_bits(self.py);
        }
        let invoke = || -> Option<u64> {
            let tuple = self.project(self.bits)?;
            let other = self.project(other)?;
            self.result(unsafe {
                ((*(&raw const PyTuple_Type))
                    .tp_richcompare
                    .expect("tuple comparison slot"))(
                    tuple.as_ptr(), other.as_ptr(), op as i32
                )
            })
        };
        invoke().unwrap_or_else(|| MoltObject::none().bits())
    }
}

impl Drop for TupleStorage<'_, '_> {
    fn drop(&mut self) {
        errors::with_preserved_error(|| dec_ref_bits(self.py, self.bits));
    }
}
