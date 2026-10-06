//! Native allocation, finalization and terminal release share one handshake.

use crate::abi_types::{Py_TPFLAGS_HAVE_GC, Py_TPFLAGS_HEAPTYPE, PyObject, PyTypeObject};
use crate::api::{errors, memory, refcount};
use std::ffi::c_void;
use std::ptr;

pub(crate) struct NativeDeallocation {
    object: *mut PyObject,
}

/// Retire an allocator result before the heap type's payload is admitted.
/// Only its initialized object header is readable; subtype/type destructors
/// would assume collector custody and a completely initialized heap tail.
pub(super) unsafe fn release_unconstructed_type(
    object: *mut PyObject,
    metaclass: *mut PyTypeObject,
    owns_metaclass: bool,
) {
    errors::with_preserved_error(|| unsafe {
        if let Some(storage) = NativeDeallocation::storage(object) {
            storage.finish();
        }
        if owns_metaclass {
            refcount::Py_DECREF(metaclass.cast());
        }
    });
}

impl NativeDeallocation {
    pub(crate) unsafe fn storage(object: *mut PyObject) -> Option<Self> {
        if object.is_null() {
            return None;
        }
        unsafe {
            let type_ = (*object).ob_type;
            if type_.is_null() {
                memory::Py_FatalError(c"native deallocation has no type".as_ptr());
            }
            if (*type_).tp_flags & Py_TPFLAGS_HAVE_GC != 0 {
                memory::PyObject_GC_UnTrack(object.cast());
            }
        }
        Some(Self { object })
    }
    /// Enter only after the last native reference has been consumed. A
    /// resurrecting finalizer retains both storage and its instance-type owner.
    pub(crate) unsafe fn begin(object: *mut PyObject) -> Option<Self> {
        if object.is_null() {
            return None;
        }
        let type_ = unsafe { (*object).ob_type };
        if type_.is_null() {
            unsafe { memory::Py_FatalError(c"native deallocation has no type".as_ptr()) };
        }
        let gc = unsafe { (*type_).tp_flags } & Py_TPFLAGS_HAVE_GC != 0;
        unsafe {
            if gc {
                memory::PyObject_GC_UnTrack(object.cast());
            }
            if memory::PyObject_CallFinalizerFromDealloc(object) < 0 {
                // A finalizer may transfer the instance to a compatible class
                // and retire the old class. Never retain an unowned old type.
                let current = (*object).ob_type;
                if (*current).tp_flags & Py_TPFLAGS_HAVE_GC != 0
                    && memory::PyObject_GC_IsTracked(object) == 0
                {
                    memory::PyObject_GC_Track(object.cast());
                }
                return None;
            }
        }
        Some(Self { object })
    }

    /// The caller has published every owned field empty before any release.
    /// Retire collector identity even when the extension supplies its own free.
    pub(crate) unsafe fn finish(self) {
        unsafe {
            // Field release can also reenter Python. Select the terminal owner
            // only after every such callback, as subtype_dealloc does.
            let type_ = (*self.object).ob_type;
            let gc = (*type_).tp_flags & Py_TPFLAGS_HAVE_GC != 0;
            let free = (*type_).tp_free.unwrap_or(if gc {
                memory::PyObject_GC_Del
            } else {
                memory::PyObject_Free
            });
            if !ptr::fn_addr_eq(
                free,
                memory::PyObject_GC_Del as unsafe extern "C" fn(*mut c_void),
            ) {
                // Arbitrary extension frees need not call a Molt memory API.
                // Revoke any physical type receipt before address reuse,
                // including rejection before type_dealloc can read a tail.
                if ![
                    memory::PyObject_Free as unsafe extern "C" fn(*mut c_void),
                    memory::PyMem_Free,
                    memory::PyMem_RawFree,
                ]
                .iter()
                .any(|known| ptr::fn_addr_eq(free, *known))
                {
                    super::unregister_type_address(self.object.addr());
                }
                if gc {
                    memory::native_gc_node_deallocate(self.object.addr());
                }
            }
            free(self.object.cast());
            // Heap subclass ownership belongs to subtype_dealloc, which wraps
            // both builtin and arbitrary extension-base destructors.
        }
    }
}

pub(crate) unsafe extern "C" fn object_dealloc(object: *mut PyObject) {
    errors::with_preserved_error(|| unsafe {
        if let Some(deallocation) = NativeDeallocation::storage(object) {
            deallocation.finish();
        }
    });
}

/// The default heap boundary owns instance finalization and the instance's
/// heap-class edge. Both FromSpec and managed projections use this before
/// inheritance, so no facade can terminate the wrapper walk with a NULL slot.
pub(super) unsafe fn prepare_heap_defaults(tp: *mut PyTypeObject, managed: bool) {
    unsafe {
        if (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE == 0 {
            return;
        }
        if (*tp).tp_dealloc.is_none() {
            (*tp).tp_dealloc = Some(subtype_dealloc);
        }
        let base = (*tp).tp_base;
        if managed {
            // Python class construction owns this allocation strategy; a
            // foreign base's allocator must not bypass the heap boundary.
            (*tp).tp_alloc = Some(super::PyType_GenericAlloc);
            (*tp).tp_free = Some(memory::PyObject_GC_Del);
        }
        // Python-created heap classes are GC types even over object: an
        // instance and one of its classes can form a cycle (type_new_alloc).
        if managed
            || (*tp).tp_flags & Py_TPFLAGS_HAVE_GC != 0
            || (!base.is_null() && (*base).tp_flags & Py_TPFLAGS_HAVE_GC != 0)
        {
            (*tp).tp_flags |= Py_TPFLAGS_HAVE_GC;
            if (*tp).tp_traverse.is_none() {
                (*tp).tp_traverse = Some(subtype_traverse);
            }
            if (*tp).tp_clear.is_none() {
                (*tp).tp_clear = Some(subtype_clear);
            }
        }
    }
}

type Dealloc = unsafe extern "C" fn(*mut PyObject);
type Traverse =
    unsafe extern "C" fn(*mut PyObject, *mut c_void, *mut c_void) -> std::os::raw::c_int;
type Clear = unsafe extern "C" fn(*mut PyObject) -> std::os::raw::c_int;
type Visit = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> std::os::raw::c_int;

/// Resolve the effective inherited storage callbacks behind default wrappers.
/// Allocation admission and execution must inspect the same base authority.
pub(crate) unsafe fn gc_uses_managed_storage(type_: *mut PyTypeObject) -> bool {
    unsafe {
        let mut traversal = type_;
        while (*traversal)
            .tp_traverse
            .is_some_and(|slot| ptr::fn_addr_eq(slot, subtype_traverse as Traverse))
        {
            traversal = (*traversal).tp_base;
            if traversal.is_null() {
                break;
            }
        }
        if !traversal.is_null()
            && (*traversal).tp_traverse.is_some_and(|slot| {
                ptr::fn_addr_eq(slot, memory::molt_managed_gc_traverse as Traverse)
                    || ptr::fn_addr_eq(slot, memory::molt_list_gc_traverse as Traverse)
            })
        {
            return true;
        }
        let mut clearing = type_;
        while (*clearing)
            .tp_clear
            .is_some_and(|slot| ptr::fn_addr_eq(slot, subtype_clear as Clear))
        {
            clearing = (*clearing).tp_base;
            if clearing.is_null() {
                break;
            }
        }
        !clearing.is_null()
            && (*clearing)
                .tp_clear
                .is_some_and(|slot| ptr::fn_addr_eq(slot, memory::molt_managed_gc_clear as Clear))
    }
}

unsafe fn clear_members(type_: *mut PyTypeObject, object: *mut PyObject) {
    unsafe {
        let members = (*type_).tp_members;
        if (*type_).tp_flags & Py_TPFLAGS_HEAPTYPE == 0 || members.is_null() {
            return;
        }
        for index in 0..(*type_).ob_base.ob_size {
            let member = &*members.offset(index);
            if member.type_ == super::PY_T_OBJECT_EX && member.flags & super::PY_READONLY == 0 {
                refcount::Py_CLEAR(object.cast::<u8>().offset(member.offset).cast());
            }
        }
    }
}

unsafe fn clear_subtype_dict(object: *mut PyObject, base: *mut PyTypeObject) {
    unsafe {
        let type_ = (*object).ob_type;
        if (*type_).tp_dictoffset != 0 && (*base).tp_dictoffset == 0 {
            let dict = crate::api::object::_PyObject_GetDictPtr(object);
            if !dict.is_null() {
                refcount::Py_CLEAR(dict);
            }
        }
    }
}

/// One default heap-subtype boundary owns finalization, member/dict release and
/// the instance's heap-class reference. Its base may be an arbitrary extension.
pub(crate) unsafe extern "C" fn subtype_dealloc(object: *mut PyObject) {
    errors::with_preserved_error(|| unsafe {
        let Some(_terminal) = NativeDeallocation::begin(object) else {
            return;
        };
        let type_ = (*object).ob_type;
        let _type_pin = refcount::OwnedPyObject::from_borrowed(type_.cast());
        let mut base = type_;
        while (*base)
            .tp_dealloc
            .is_some_and(|slot| ptr::fn_addr_eq(slot, subtype_dealloc as Dealloc))
        {
            clear_members(base, object);
            base = (*base).tp_base;
            if base.is_null() {
                memory::Py_FatalError(c"heap subtype has no base destructor".as_ptr());
            }
        }
        let _base_pin = refcount::OwnedPyObject::from_borrowed(base.cast());
        clear_subtype_dict(object, base);
        // Member/dict callbacks and finalization can change the actual class.
        // The base callback can free that class, so cache no borrowed fields
        // after calling it. A heap base destructor consumes its own class owner.
        let current = (*object).ob_type;
        let release_type = (*current).tp_flags & Py_TPFLAGS_HEAPTYPE != 0
            && (*base).tp_flags & Py_TPFLAGS_HEAPTYPE == 0;
        let Some(dealloc) = (*base).tp_dealloc else {
            memory::Py_FatalError(c"heap subtype base has no destructor".as_ptr());
        };
        if (*base).tp_flags & Py_TPFLAGS_HAVE_GC != 0 && memory::PyObject_GC_IsTracked(object) == 0
        {
            memory::PyObject_GC_Track(object.cast());
        }
        dealloc(object);
        if release_type {
            refcount::Py_DECREF(current.cast());
        }
    });
}

pub(super) unsafe extern "C" fn subtype_traverse(
    object: *mut PyObject,
    visit_raw: *mut c_void,
    arg: *mut c_void,
) -> std::os::raw::c_int {
    if object.is_null() || visit_raw.is_null() {
        return 0;
    }
    unsafe {
        let visit: Visit = std::mem::transmute(visit_raw);
        let type_ = (*object).ob_type;
        let mut base = type_;
        while (*base)
            .tp_traverse
            .is_some_and(|slot| ptr::fn_addr_eq(slot, subtype_traverse as Traverse))
        {
            let members = (*base).tp_members;
            if !members.is_null() {
                for index in 0..(*base).ob_base.ob_size {
                    let member = &*members.offset(index);
                    if member.type_ == super::PY_T_OBJECT_EX {
                        let edge = object
                            .cast::<u8>()
                            .offset(member.offset)
                            .cast::<*mut PyObject>()
                            .read();
                        if !edge.is_null() {
                            let rc = visit(edge, arg);
                            if rc != 0 {
                                return rc;
                            }
                        }
                    }
                }
            }
            base = (*base).tp_base;
            if base.is_null() {
                return 0;
            }
        }
        if (*type_).tp_dictoffset != 0 && (*base).tp_dictoffset == 0 {
            let dict = crate::api::object::_PyObject_GetDictPtr(object);
            if !dict.is_null() && !(*dict).is_null() {
                let rc = visit(*dict, arg);
                if rc != 0 {
                    return rc;
                }
            }
        }
        if (*type_).tp_flags & Py_TPFLAGS_HEAPTYPE != 0
            && ((*base).tp_traverse.is_none() || (*base).tp_flags & Py_TPFLAGS_HEAPTYPE == 0)
        {
            let rc = visit(type_.cast(), arg);
            if rc != 0 {
                return rc;
            }
        }
        (*base)
            .tp_traverse
            .map_or(0, |traverse| traverse(object, visit_raw, arg))
    }
}

pub(super) unsafe extern "C" fn subtype_clear(object: *mut PyObject) -> std::os::raw::c_int {
    if object.is_null() {
        return 0;
    }
    unsafe {
        let mut base = (*object).ob_type;
        let _type_pin = refcount::OwnedPyObject::from_borrowed(base.cast());
        while (*base)
            .tp_clear
            .is_some_and(|slot| ptr::fn_addr_eq(slot, subtype_clear as Clear))
        {
            clear_members(base, object);
            base = (*base).tp_base;
            if base.is_null() {
                return 0;
            }
        }
        clear_subtype_dict(object, base);
        (*base).tp_clear.map_or(0, |clear| clear(object))
    }
}

pub(crate) unsafe extern "C" fn type_dealloc(object: *mut PyObject) {
    if object.is_null() {
        return;
    }
    let type_ = object.cast::<PyTypeObject>();
    let Some(heap) = super::heap_type_storage(type_) else {
        unsafe {
            memory::Py_FatalError(c"native type deallocation requires proven heap storage".as_ptr())
        };
    };
    errors::with_preserved_error(|| unsafe {
        let Some(deallocation) = NativeDeallocation::storage(object) else {
            return;
        };
        super::unregister_type_address(object.addr());
        // Detach the entire inventory before DECREF can invoke callbacks.
        let edges = [
            std::mem::replace(&mut (*type_).tp_base, ptr::null_mut()).cast(),
            std::mem::replace(&mut (*type_).tp_dict, ptr::null_mut()),
            std::mem::replace(&mut (*type_).tp_bases, ptr::null_mut()),
            std::mem::replace(&mut (*type_).tp_mro, ptr::null_mut()),
            std::mem::replace(&mut (*type_).tp_cache, ptr::null_mut()),
            std::mem::replace(&mut (*heap).ht_name, ptr::null_mut()),
            std::mem::replace(&mut (*heap).ht_qualname, ptr::null_mut()),
            std::mem::replace(&mut (*heap).ht_slots, ptr::null_mut()),
            std::mem::replace(&mut (*heap).ht_module, ptr::null_mut()),
        ];
        let doc = std::mem::replace(&mut (*type_).tp_doc, ptr::null());
        let name = std::mem::replace(&mut (*heap)._ht_tpname, ptr::null_mut());
        (*type_).tp_name = ptr::null();
        errors::release_preserving_error(&edges);
        memory::PyMem_Free(doc.cast_mut().cast());
        memory::PyMem_Free(name.cast());
        deallocation.finish();
    });
}

/// A failed spec construction can already own a self-referential MRO or
/// descriptors. Cut those construction cycles before releasing its first owner.
pub(super) struct HeapTypeConstruction(pub(super) refcount::OwnedPyObject);

impl HeapTypeConstruction {
    pub(super) fn into_ptr(self) -> *mut PyObject {
        let this = std::mem::ManuallyDrop::new(self);
        unsafe { ptr::read(&this.0) }.into_ptr()
    }
}

impl Drop for HeapTypeConstruction {
    fn drop(&mut self) {
        errors::with_preserved_error(|| unsafe {
            let type_ = self.0.as_ptr().cast::<PyTypeObject>();
            if !(*type_).tp_dict.is_null() {
                crate::api::mapping::PyDict_Clear((*type_).tp_dict);
            }
            refcount::Py_CLEAR(&raw mut (*type_).tp_mro);
        });
    }
}
