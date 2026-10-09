//! Native type metadata admission before callback-bearing namespace publication.

use super::*;
use crate::api::refcount::OwnedPyObject;
use crate::api::sequences::{PyTuple_GetItem, PyTuple_New, PyTuple_SetItem, PyTuple_Size};

pub(super) unsafe fn prepare(
    bridge: &crate::bridge::ObjectBridge,
    tp: *mut PyTypeObject,
    projected: bool,
) -> c_int {
    unsafe {
        let object = &raw mut crate::abi_types::PyBaseObject_Type;
        if (*tp).tp_base.is_null() && tp != object {
            (*tp).tp_base = object;
            if (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE != 0 {
                crate::api::refcount::Py_INCREF(object.cast());
            }
        }
        let base = (*tp).tp_base;
        let _base = OwnedPyObject::from_borrowed(base.cast());
        if !base.is_null() {
            if ready_type(bridge, base) < 0 {
                return -1;
            }
            if (*tp).ob_base.ob_base.ob_type.is_null() {
                (*tp).ob_base.ob_base.ob_type = crate::bridge::semantic_type(base.cast());
            }
        }
        if (*tp).ob_base.ob_base.ob_type.is_null() {
            return reject_type_readiness(c"type has no resolved metaclass");
        }
        if (*tp).tp_bases.is_null() {
            let bases = OwnedPyObject::from_owned(PyTuple_New(if base.is_null() { 0 } else { 1 }));
            if bases.as_ptr().is_null() {
                return -1;
            }
            if !base.is_null() {
                crate::api::refcount::Py_INCREF(base.cast());
                // SetItem consumes the new reference on either outcome.
                if PyTuple_SetItem(bases.as_ptr(), 0, base.cast()) < 0 {
                    return -1;
                }
            }
            (*tp).tp_bases = bases.into_ptr();
        }
        if projected {
            // Runtime-bound native shells still declare native descriptors and
            // slots. Ready their bases, while preserving the runtime's sealed
            // hierarchy instead of rebuilding a second MRO.
            let bases = OwnedPyObject::from_borrowed((*tp).tp_bases);
            let count = PyTuple_Size(bases.as_ptr());
            if count < 0 {
                return -1;
            }
            for index in 0..count {
                let base = PyTuple_GetItem(bases.as_ptr(), index);
                if base.is_null() || ready_type(bridge, base.cast()) < 0 {
                    return -1;
                }
            }
            0
        } else {
            compute_mro(tp)
        }
    }
}

unsafe fn compute_mro(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        // Pin snapshots before base readiness can allocate or invoke callbacks.
        let bases = OwnedPyObject::from_borrowed((*tp).tp_bases);
        let count = PyTuple_Size(bases.as_ptr());
        if count < 0 {
            return -1;
        }
        let mut direct = Vec::with_capacity(count as usize);
        let mut owners = Vec::with_capacity(count as usize);
        let mut seen = HashSet::with_capacity(count as usize);
        for index in 0..count {
            let base = PyTuple_GetItem(bases.as_ptr(), index);
            if base.is_null() {
                return -1;
            }
            if base == tp.cast() || !seen.insert(base) {
                return reject_type_layout(c"duplicate or cyclic base class");
            }
            // All tuple entries must already be type objects. Readying does not
            // turn an arbitrary object into a type by interpreting its payload.
            if PyType_Check(base) == 0 {
                return reject_type_layout(c"bases must contain only type objects");
            }
            owners.push(OwnedPyObject::from_borrowed(base));
            direct.push(base.cast::<PyTypeObject>());
        }
        let mut sequences = Vec::with_capacity(direct.len() + 1);
        let mut mro_owners = Vec::with_capacity(direct.len());
        for &base in &direct {
            if PyType_Ready(base) < 0 {
                return -1;
            }
            let mro = OwnedPyObject::from_borrowed((*base).tp_mro);
            if mro.as_ptr().is_null() {
                return reject_type_readiness(c"ready base has no method resolution order");
            }
            let len = PyTuple_Size(mro.as_ptr());
            if len < 0 {
                return -1;
            }
            let mut sequence = Vec::with_capacity(len as usize);
            for index in 0..len {
                let entry = PyTuple_GetItem(mro.as_ptr(), index);
                if entry.is_null() {
                    return -1;
                }
                if entry == tp.cast() || PyType_Check(entry) == 0 {
                    return reject_type_layout(c"invalid or cyclic base method resolution order");
                }
                sequence.push(entry.cast::<PyTypeObject>());
            }
            sequences.push(sequence);
            mro_owners.push(mro);
        }
        sequences.push(direct);
        let Some(merged) = molt_lang_obj_model::hierarchy::c3_merge(&sequences) else {
            return reject_type_layout(c"cannot create a consistent method resolution order (MRO)");
        };
        let mro = OwnedPyObject::from_owned(PyTuple_New((merged.len() + 1) as Py_ssize_t));
        if mro.as_ptr().is_null() {
            return -1;
        }
        for (index, entry) in std::iter::once(tp).chain(merged).enumerate() {
            crate::api::refcount::Py_INCREF(entry.cast());
            if PyTuple_SetItem(mro.as_ptr(), index as Py_ssize_t, entry.cast()) < 0 {
                return -1;
            }
        }
        let previous = std::mem::replace(&mut (*tp).tp_mro, mro.into_ptr());
        crate::api::errors::release_preserving_error(&[previous]);
        0
    }
}

/// Native solid owners are defined by physical layout changes, not names or
/// direct subtype comparability. Equal-layout sibling bases are compatible.
unsafe fn solid_owner(
    mut tp: *mut PyTypeObject,
    owners: &mut HashMap<*mut PyTypeObject, *mut PyTypeObject>,
    owner_pins: &mut Vec<OwnedPyObject>,
) -> Option<*mut PyTypeObject> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    unsafe {
        while !tp.is_null() && !owners.contains_key(&tp) {
            if let Some(value) =
                GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(tp.cast())
                    .filter(|value| {
                        crate::hooks::hooks_or_stubs().classify_heap(value.bits())
                            == crate::abi_types::MoltTypeTag::Type as u8
                    })
            {
                // Stop at every representation boundary, including managed
                // ancestors behind a native direct base. Facade sizes cannot
                // answer whether the runtime class introduced physical slots.
                let result = (crate::hooks::hooks_or_stubs().type_metadata)(
                    value.bits(),
                    crate::hooks::TypeMetadataField::SolidOwner,
                );
                let pin = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_result_to_pyobj(result));
                if pin.as_ptr().is_null() {
                    return None;
                }
                owners.insert(tp, pin.as_ptr().cast::<PyTypeObject>());
                owner_pins.push(pin);
                break;
            }
            if !seen.insert(tp) {
                reject_type_layout(c"cyclic physical base chain");
                return None;
            }
            chain.push(tp);
            tp = (*tp).tp_base;
        }
        let mut entries = chain.into_iter().rev();
        let mut owner = if tp.is_null() {
            entries.next()?
        } else {
            owners[&tp]
        };
        owners.insert(owner, owner);
        for candidate in entries {
            if (*candidate).tp_basicsize != (*owner).tp_basicsize
                || (*candidate).tp_itemsize != (*owner).tp_itemsize
            {
                owner = candidate;
            }
            owners.insert(candidate, owner);
        }
        Some(owner)
    }
}

pub(super) unsafe fn best_base(bases: *mut PyObject) -> *mut PyTypeObject {
    unsafe {
        let bases = OwnedPyObject::from_borrowed(bases);
        let count = PyTuple_Size(bases.as_ptr());
        if count < 0 {
            return ptr::null_mut();
        }
        let mut candidates = Vec::with_capacity(count as usize);
        for index in 0..count {
            let candidate = PyTuple_GetItem(bases.as_ptr(), index);
            if candidate.is_null() {
                return ptr::null_mut();
            }
            if PyType_Check(candidate) == 0 {
                reject_type_layout(c"bases must contain only type objects");
                return ptr::null_mut();
            }
            candidates.push(OwnedPyObject::from_borrowed(candidate));
        }
        let mut selected = None;
        let mut owners = HashMap::new();
        let mut owner_pins = Vec::with_capacity(candidates.len());
        for candidate in &candidates {
            let direct = candidate.as_ptr().cast::<PyTypeObject>();
            if PyType_Ready(direct) < 0 {
                return ptr::null_mut();
            }
            if (*direct).tp_flags & crate::abi_types::Py_TPFLAGS_BASETYPE == 0 {
                reject_type_layout(c"type is not an acceptable base type");
                return ptr::null_mut();
            }
            let Some(owner) = solid_owner(direct, &mut owners, &mut owner_pins) else {
                return ptr::null_mut();
            };
            match molt_lang_obj_model::hierarchy::dominant_layout_base(
                selected,
                (direct, owner),
                |a, b| PyType_IsSubtype(a, b) != 0,
            ) {
                Ok(winner) => selected = Some(winner),
                Err(_) => {
                    reject_type_layout(c"multiple bases have instance lay-out conflict");
                    return ptr::null_mut();
                }
            }
        }
        selected.map_or(ptr::null_mut(), |(direct, _)| direct)
    }
}

/// The native hierarchy owner has no descendant rebase transaction. Preserve
/// that admission boundary rather than publishing a tuple without updating
/// descendant MROs, inherited slots and subclass identities.
pub(super) unsafe fn set_bases(tp: *mut PyTypeObject, value: *mut PyObject) -> c_int {
    unsafe {
        if value.is_null() {
            return reject_type_layout(c"cannot delete __bases__ attribute");
        }
        if crate::api::sequences::PyTuple_Check(value) == 0 {
            return reject_type_layout(c"__bases__ must be a tuple of classes");
        }
        let _receiver = OwnedPyObject::from_borrowed(tp.cast());
        let _incoming = OwnedPyObject::from_borrowed(value);
        reject_type_layout(c"native class bases are immutable after type readiness")
    }
}

/// Reassign only equal native storage/deallocation contracts. Managed classes
/// use object_set_class and can never be installed as a physical ob_type.
pub(super) unsafe fn assign_class(object: *mut PyObject, target: *mut PyTypeObject) -> c_int {
    unsafe {
        let original = (*object).ob_type;
        let _receiver = OwnedPyObject::from_borrowed(object);
        let old = OwnedPyObject::from_borrowed(original.cast());
        let new = OwnedPyObject::from_borrowed(target.cast());
        if GLOBAL_BRIDGE
            .observed_handle_for_pyobj(target.cast())
            .is_some_and(|value| {
                crate::hooks::hooks_or_stubs().classify_heap(value.bits())
                    == crate::abi_types::MoltTypeTag::Type as u8
            })
        {
            return reject_type_layout(
                c"__class__ assignment cannot change native storage into managed storage",
            );
        }
        if descriptors::pending() {
            return -1;
        }
        if (*original).tp_flags & Py_TPFLAGS_HEAPTYPE == 0
            || (*target).tp_flags & Py_TPFLAGS_HEAPTYPE == 0
            || ((*original).tp_flags | (*target).tp_flags)
                & crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE
                != 0
        {
            return reject_type_layout(c"__class__ assignment requires mutable heap types");
        }
        if PyType_Ready(target) < 0 {
            return -1;
        }
        let layout_flags = Py_TPFLAGS_HAVE_GC
            | crate::abi_types::Py_TPFLAGS_MANAGED_DICT
            | crate::abi_types::Py_TPFLAGS_MANAGED_WEAKREF
            | crate::abi_types::Py_TPFLAGS_ITEMS_AT_END;
        if (*original).tp_basicsize != (*target).tp_basicsize
            || (*original).tp_itemsize != (*target).tp_itemsize
            || (*original).tp_dictoffset != (*target).tp_dictoffset
            || (*original).tp_weaklistoffset != (*target).tp_weaklistoffset
            || (*original).tp_vectorcall_offset != (*target).tp_vectorcall_offset
            || ((*original).tp_flags ^ (*target).tp_flags) & layout_flags != 0
            || (*original).tp_dealloc.map(|f| f as usize)
                != (*target).tp_dealloc.map(|f| f as usize)
            || (*original).tp_free.map(|f| f as usize) != (*target).tp_free.map(|f| f as usize)
            || (*original).tp_traverse.map(|f| f as usize)
                != (*target).tp_traverse.map(|f| f as usize)
            || (*original).tp_clear.map(|f| f as usize) != (*target).tp_clear.map(|f| f as usize)
        {
            return reject_type_layout(c"__class__ assignment has incompatible native layout");
        }
        let mut owners = HashMap::new();
        let mut pins = Vec::new();
        let Some(left) = solid_owner(original, &mut owners, &mut pins) else {
            return -1;
        };
        let Some(right) = solid_owner(target, &mut owners, &mut pins) else {
            return -1;
        };
        if left != right {
            return reject_type_layout(c"__class__ assignment has different native solid owners");
        }
        if (*object).ob_type != original {
            return reject_type_readiness(c"object class changed during assignment");
        }
        crate::api::refcount::Py_INCREF(target.cast());
        (*object).ob_type = target;
        crate::api::errors::release_preserving_error(&[original.cast()]);
        drop(new);
        drop(old);
        0
    }
}
