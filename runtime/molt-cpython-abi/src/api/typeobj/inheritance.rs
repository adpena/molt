//! Slot inheritance follows the completed C3 order and CPython's coupled rules.

use super::*;
use crate::abi_types::*;

pub(super) unsafe fn prepare_layout(tp: *mut PyTypeObject) {
    unsafe {
        if let Some(heap) = heap_type_storage(tp) {
            macro_rules! own_table {
                ($field:ident, $storage:ident) => {
                    if (*tp).$field.is_null() {
                        (*tp).$field = (&raw mut (*heap).$storage).cast();
                    }
                };
            }
            own_table!(tp_as_async, as_async);
            own_table!(tp_as_number, as_number);
            own_table!(tp_as_sequence, as_sequence);
            own_table!(tp_as_mapping, as_mapping);
            own_table!(tp_as_buffer, as_buffer);
        }
        let base = (*tp).tp_base;
        if base.is_null() {
            return;
        }
        (*tp).tp_flags |= (*base).tp_flags
            & (Py_TPFLAGS_MANAGED_DICT
                | Py_TPFLAGS_MANAGED_WEAKREF
                | Py_TPFLAGS_BASE_EXC_SUBCLASS
                | Py_TPFLAGS_TYPE_SUBCLASS
                | Py_TPFLAGS_LONG_SUBCLASS
                | Py_TPFLAGS_BYTES_SUBCLASS
                | Py_TPFLAGS_UNICODE_SUBCLASS
                | Py_TPFLAGS_TUPLE_SUBCLASS
                | Py_TPFLAGS_LIST_SUBCLASS
                | Py_TPFLAGS_DICT_SUBCLASS
                | _Py_TPFLAGS_MATCH_SELF
                | Py_TPFLAGS_ITEMS_AT_END);
        if (*tp).tp_flags & Py_TPFLAGS_HAVE_GC == 0
            && (*base).tp_flags & Py_TPFLAGS_HAVE_GC != 0
            && (*tp).tp_traverse.is_none()
            && (*tp).tp_clear.is_none()
        {
            (*tp).tp_flags |= Py_TPFLAGS_HAVE_GC;
            (*tp).tp_traverse = (*base).tp_traverse;
            (*tp).tp_clear = (*base).tp_clear;
        }
        macro_rules! inherit_value {
            ($field:ident) => {
                if (*tp).$field == 0 {
                    (*tp).$field = (*base).$field;
                }
            };
        }
        inherit_value!(tp_basicsize);
        inherit_value!(tp_itemsize);
        inherit_value!(tp_dictoffset);
        inherit_value!(tp_weaklistoffset);
    }
}

pub(super) unsafe fn prepare_new(tp: *mut PyTypeObject) {
    unsafe {
        let base = (*tp).tp_base;
        if (*tp).tp_new.is_none()
            && base == &raw mut PyBaseObject_Type
            && (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE == 0
        {
            (*tp).tp_flags |= Py_TPFLAGS_DISALLOW_INSTANTIATION;
        }
        if (*tp).tp_flags & Py_TPFLAGS_DISALLOW_INSTANTIATION != 0 {
            (*tp).tp_new = None;
        } else if (*tp).tp_new.is_none() && !base.is_null() {
            (*tp).tp_new = (*base).tp_new;
        }
    }
}

unsafe fn own_name(tp: *mut PyTypeObject, name: &std::ffi::CStr) -> Result<bool, ()> {
    let value = unsafe {
        crate::api::mapping::_PyDict_GetItemStringWithError((*tp).tp_dict, name.as_ptr())
    };
    if descriptors::pending() {
        Err(())
    } else {
        Ok(!value.is_null())
    }
}

unsafe fn copy_declared_slot(tp: *mut PyTypeObject, base: *mut PyTypeObject, slot: SlotWrapper) {
    unsafe {
        let destination = slot_wrapper_storage(tp, slot);
        if destination.is_null() || !destination.read().is_null() {
            return;
        }
        let value = slot_wrapper_ptr(base, slot);
        let parent = (*base).tp_base;
        if !value.is_null() && (parent.is_null() || value != slot_wrapper_ptr(parent, slot)) {
            destination.write(value);
        }
    }
}

pub(super) unsafe fn finish(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let mro = crate::api::refcount::OwnedPyObject::from_borrowed((*tp).tp_mro);
        let count = crate::api::sequences::PyTuple_Size(mro.as_ptr());
        if count < 0 {
            return -1;
        }
        let overrides_hash = match own_name(tp, c"__eq__") {
            Err(()) => return -1,
            Ok(true) => true,
            Ok(false) => match own_name(tp, c"__hash__") {
                Ok(value) => value,
                Err(()) => return -1,
            },
        };
        for index in 1..count {
            let base =
                crate::api::sequences::PyTuple_GetItem(mro.as_ptr(), index).cast::<PyTypeObject>();
            if base.is_null() {
                return -1;
            }
            if (*tp).tp_flags & (Py_TPFLAGS_SEQUENCE | Py_TPFLAGS_MAPPING) == 0 {
                (*tp).tp_flags |= (*base).tp_flags & (Py_TPFLAGS_SEQUENCE | Py_TPFLAGS_MAPPING);
            }
            if (*tp).tp_call.is_none() {
                (*tp).tp_flags |= (*base).tp_flags & Py_TPFLAGS_HAVE_VECTORCALL;
            }
            // The slot-number/storage schema already covers all protocol table
            // fields. Inheritance policy selects fields, never repeats offsets.
            for id in ts::Py_bf_getbuffer..=ts::Py_am_send {
                let Some(slot) = stable_slot_wrapper(id) else {
                    continue;
                };
                let inherit = match slot {
                    SlotWrapper::Direct(d) => matches!(
                        d,
                        DirectSlot::Dealloc
                            | DirectSlot::Repr
                            | DirectSlot::Call
                            | DirectSlot::Str
                            | DirectSlot::Iter
                            | DirectSlot::IterNext
                            | DirectSlot::DescrGet
                            | DirectSlot::DescrSet
                            | DirectSlot::Init
                            | DirectSlot::Alloc
                            | DirectSlot::IsGc
                            | DirectSlot::Finalize
                    ),
                    SlotWrapper::Async(AsyncSlot::Send) => false,
                    _ => true,
                };
                if inherit {
                    copy_declared_slot(tp, base, slot);
                }
            }
            if (*tp).tp_getattr.is_none() && (*tp).tp_getattro.is_none() {
                (*tp).tp_getattr = (*base).tp_getattr;
                (*tp).tp_getattro = (*base).tp_getattro;
            }
            if (*tp).tp_setattr.is_none() && (*tp).tp_setattro.is_none() {
                (*tp).tp_setattr = (*base).tp_setattr;
                (*tp).tp_setattro = (*base).tp_setattro;
            }
            if (*tp).tp_richcompare.is_none() && (*tp).tp_hash.is_none() && !overrides_hash {
                (*tp).tp_richcompare = (*base).tp_richcompare;
                (*tp).tp_hash = (*base).tp_hash;
            }
            let parent = (*base).tp_base;
            if (*tp).tp_vectorcall_offset == 0
                && (*base).tp_vectorcall_offset != 0
                && (parent.is_null()
                    || (*base).tp_vectorcall_offset != (*parent).tp_vectorcall_offset)
            {
                (*tp).tp_vectorcall_offset = (*base).tp_vectorcall_offset;
            }
            if (*tp).tp_descr_get.is_some()
                && (*tp).tp_descr_get.map(|f| f as *const ())
                    == (*base).tp_descr_get.map(|f| f as *const ())
                && (*tp).tp_flags & Py_TPFLAGS_IMMUTABLETYPE != 0
            {
                (*tp).tp_flags |= (*base).tp_flags & Py_TPFLAGS_METHOD_DESCRIPTOR;
            }
            if (*tp).tp_flags & Py_TPFLAGS_HAVE_GC == (*base).tp_flags & Py_TPFLAGS_HAVE_GC {
                copy_declared_slot(tp, base, SlotWrapper::Direct(DirectSlot::Free));
            } else if (*tp).tp_flags & Py_TPFLAGS_HAVE_GC != 0
                && (*tp).tp_free.is_none()
                && (*base).tp_free.map(|f| f as *const ())
                    == Some(crate::api::memory::PyObject_Free as *const ())
            {
                (*tp).tp_free = Some(crate::api::memory::PyObject_GC_Del);
            }
        }
        let base = (*tp).tp_base;
        if !base.is_null() {
            // A static type without its own table aliases only after per-slot
            // inheritance, so ancestor storage is never used as an output table.
            macro_rules! inherit_table {
                ($field:ident) => {
                    if (*tp).$field.is_null() {
                        (*tp).$field = (*base).$field;
                    }
                };
            }
            inherit_table!(tp_as_async);
            inherit_table!(tp_as_number);
            inherit_table!(tp_as_sequence);
            inherit_table!(tp_as_mapping);
            inherit_table!(tp_as_buffer);
        }
        if (*tp).tp_alloc.is_none() {
            (*tp).tp_alloc = Some(PyType_GenericAlloc);
        }
        if (*tp).tp_free.is_none() {
            (*tp).tp_free = Some(if (*tp).tp_flags & Py_TPFLAGS_HAVE_GC != 0 {
                crate::api::memory::PyObject_GC_Del
            } else {
                crate::api::memory::PyObject_Free
            });
        }
        if (*tp).tp_hash.is_none() {
            match own_name(tp, c"__hash__") {
                Err(()) => return -1,
                Ok(true) => {}
                Ok(false) => {
                    if crate::api::mapping::PyDict_SetItemString(
                        (*tp).tp_dict,
                        c"__hash__".as_ptr(),
                        &raw mut Py_None,
                    ) < 0
                    {
                        return -1;
                    }
                    (*tp).tp_hash = Some(PyObject_HashNotImplemented);
                }
            }
        }
        if (*tp).tp_flags & Py_TPFLAGS_HAVE_GC != 0 {
            if (*tp).tp_traverse.is_none() {
                crate::api::errors::PyErr_SetString(
                    (&raw mut PyExc_SystemError).cast(),
                    c"GC type has no traverse function".as_ptr(),
                );
                return -1;
            }
            if (*tp).tp_flags & Py_TPFLAGS_BASETYPE != 0
                && (*tp).tp_free.map(|f| f as *const ())
                    == Some(crate::api::memory::PyObject_Free as *const ())
            {
                return reject_type_layout(c"GC base type has a non-GC free slot");
            }
        }
        0
    }
}
