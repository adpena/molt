//! Native test declarations use the production readiness and allocation owners.

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{mapping, memory, refcount, typeobj};
use std::ffi::CStr;
use std::ops::{Deref, DerefMut};
use std::os::raw::c_int;
use std::ptr::{self, NonNull};

pub(crate) trait TypeStorage {
    unsafe fn allocate() -> *mut Self;
    unsafe fn free(allocation: *mut Self);
    fn type_object(&mut self) -> &mut PyTypeObject;
}

impl TypeStorage for PyTypeObject {
    unsafe fn allocate() -> *mut Self {
        unsafe { memory::PyObject_Calloc(1, std::mem::size_of::<Self>()).cast() }
    }

    unsafe fn free(allocation: *mut Self) {
        unsafe { memory::PyObject_Free(allocation.cast()) };
    }

    fn type_object(&mut self) -> &mut PyTypeObject {
        self
    }
}

impl TypeStorage for PyHeapTypeObject {
    unsafe fn allocate() -> *mut Self {
        unsafe { memory::_PyObject_GC_New(&raw mut PyType_Type).cast() }
    }

    unsafe fn free(allocation: *mut Self) {
        unsafe { memory::PyObject_GC_Del(allocation.cast()) };
    }

    fn type_object(&mut self) -> &mut PyTypeObject {
        &mut self.ht_type
    }
}

/// Own stable C storage, not a runtime substitute. Types drop in reverse local
/// order, while their bases and declaration tables still live. The public free
/// path also unregisters the type's non-owning subclass identity.
pub(crate) struct NativeType<T: TypeStorage>(NonNull<T>);

impl<T: TypeStorage> NativeType<T> {
    pub(crate) fn new() -> Self {
        let mut allocation =
            NonNull::new(unsafe { T::allocate() }).expect("native type fixture allocation");
        unsafe { allocation.as_mut() }
            .type_object()
            .ob_base
            .ob_base
            .ob_refcnt = 1;
        Self(allocation)
    }

    /// Run after the caller has populated its own declarations. Inheritance,
    /// namespace publication and GC admission belong to public readiness.
    pub(crate) unsafe fn ready(&mut self) -> c_int {
        unsafe { typeobj::PyType_Ready(self.0.as_mut().type_object()) }
    }
}

impl NativeType<PyTypeObject> {
    /// Declare an unready static subtype. Bootstrap-negative tests can retain
    /// this declaration without pretending inherited slots or READY state exist.
    pub(crate) fn subtype(base: *mut PyTypeObject, name: &'static CStr) -> Self {
        let mut class = Self::new();
        class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        class.tp_name = name.as_ptr();
        class.tp_base = base;
        class.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE;
        class
    }
}

impl<T: TypeStorage> Deref for NativeType<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}

impl<T: TypeStorage> DerefMut for NativeType<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut() }
    }
}

impl<T: TypeStorage> Drop for NativeType<T> {
    fn drop(&mut self) {
        unsafe {
            let ty = self.0.as_mut().type_object();
            // Retain addresses only, never an extra object owner: clearing the
            // namespace and MRO must retire every native GC allocation they own.
            let mut native_nodes = Vec::new();
            for root in [ty.tp_dict, ty.tp_mro, ty.tp_bases, ty.tp_cache] {
                if crate::object::gc::native_gc_is_enrolled(root.addr()) {
                    native_nodes.push(root.addr());
                }
            }
            if !ty.tp_dict.is_null() {
                let mut position = 0;
                let mut value = ptr::null_mut();
                while mapping::PyDict_Next(
                    ty.tp_dict,
                    &raw mut position,
                    ptr::null_mut(),
                    &raw mut value,
                ) != 0
                {
                    if crate::object::gc::native_gc_is_enrolled(value.addr()) {
                        native_nodes.push(value.addr());
                    }
                }
            }
            refcount::Py_CLEAR(&raw mut ty.tp_mro);
            refcount::Py_CLEAR(&raw mut ty.tp_dict);
            refcount::Py_CLEAR(&raw mut ty.tp_bases);
            refcount::Py_CLEAR(&raw mut ty.tp_cache);
            assert_eq!(
                ty.ob_base.ob_base.ob_refcnt, 1,
                "native type retained a descriptor, MRO or instance owner"
            );
            for address in native_nodes {
                assert!(
                    !crate::object::gc::native_gc_is_enrolled(address),
                    "native readiness leaked GC identity {address:#x}"
                );
            }
            T::free(self.0.as_ptr());
        }
    }
}
