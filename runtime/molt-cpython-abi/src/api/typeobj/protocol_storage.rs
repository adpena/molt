//! Compact runtime types own all protocol destinations. Physical ownership is
//! declared once; readiness and mutation fill slots from SLOT_WRAPPER_DEFS.
use crate::abi_types::*;
use std::cell::UnsafeCell;

pub struct TypeProtocolTables {
    as_async: UnsafeCell<PyAsyncMethods>,
    as_number: UnsafeCell<PyNumberMethods>,
    as_sequence: UnsafeCell<PySequenceMethods>,
    as_mapping: UnsafeCell<PyMappingMethods>,
    as_buffer: UnsafeCell<PyBufferProcs>,
}

// Slot reads/writes have the same runtime GIL/bootstrap custody as PyTypeObject.
// The process owners outlive their exported compact type objects. Managed
// compact allocations keep this exact storage inside their existing owner.
unsafe impl Sync for TypeProtocolTables {}

impl TypeProtocolTables {
    pub const fn new() -> Self {
        Self {
            as_async: UnsafeCell::new(unsafe { std::mem::zeroed() }),
            as_number: UnsafeCell::new(unsafe { std::mem::zeroed() }),
            as_sequence: UnsafeCell::new(unsafe { std::mem::zeroed() }),
            as_mapping: UnsafeCell::new(unsafe { std::mem::zeroed() }),
            as_buffer: UnsafeCell::new(unsafe { std::mem::zeroed() }),
        }
    }

    /// Attach only at allocation/bootstrap, before the type can be observed.
    /// `self` must retain its address and outlive every read of the type slots.
    pub unsafe fn attach(&self, tp: *mut PyTypeObject) {
        unsafe {
            (*tp).tp_as_async = self.as_async.get().cast();
            (*tp).tp_as_number = self.as_number.get().cast();
            (*tp).tp_as_sequence = self.as_sequence.get().cast();
            (*tp).tp_as_mapping = self.as_mapping.get().cast();
            (*tp).tp_as_buffer = self.as_buffer.get().cast();
        }
    }
}

static MAPPINGPROXY_PROTOCOLS: TypeProtocolTables = TypeProtocolTables::new();

/// Exact process storage/authority declaration, never a class-name or mutable
/// flag classifier. mappingproxy's existing runtime method declarations own its
/// behavior. Exported native builtin shells keep their declared C slots.
pub(crate) fn process_runtime_protocols(
    tp: *mut PyTypeObject,
) -> Option<&'static TypeProtocolTables> {
    std::ptr::eq(tp, &raw mut PyDictProxy_Type).then_some(&MAPPINGPROXY_PROTOCOLS)
}

/// Physical C member metadata derived from the runtime's sealed specification.
/// The retained exact spec projection owns all UTF-8 caches used by member
/// names/docs. It contains no class edge, registry or mutable semantic facts.
pub struct StructSequenceTypeProjection {
    _spec: crate::api::refcount::OwnedPyObject,
    members: Vec<PyMemberDef>,
    basicsize: Py_ssize_t,
}

impl StructSequenceTypeProjection {
    /// Missing is an ordinary type; Error preserves actual projection failure.
    pub unsafe fn prepare(bits: u64) -> Result<Option<Self>, ()> {
        use crate::api::{errors, numbers, sequences, strings};
        use crate::hooks::{DecodedHandleResult, TypeMetadataField, hooks_or_stubs};
        unsafe {
            let spec_bits =
                match (hooks_or_stubs().type_metadata)(bits, TypeMetadataField::StructSequence)
                    .decode()
                {
                    DecodedHandleResult::Missing => return Ok(None),
                    DecodedHandleResult::Error => return Err(()),
                    DecodedHandleResult::Ok(bits) => bits,
                };
            let spec = crate::api::refcount::OwnedPyObject::from_owned(
                crate::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(spec_bits),
            );
            if spec.as_ptr().is_null() {
                return Err(());
            }
            let names = sequences::PyTuple_GetItem(spec.as_ptr(), 0);
            let docs = sequences::PyTuple_GetItem(spec.as_ptr(), 1);
            let visible = numbers::PyLong_AsSsize_t(sequences::PyTuple_GetItem(spec.as_ptr(), 2));
            let count = sequences::PyTuple_Size(names);
            if visible < 0 || count < visible || sequences::PyTuple_Size(docs) != count {
                errors::PyErr_BadInternalCall();
                return Err(());
            }
            let mut members = Vec::new();
            if members.try_reserve_exact(count as usize + 1).is_err() {
                errors::PyErr_NoMemory();
                return Err(());
            }
            for index in 0..count {
                let name = sequences::PyTuple_GetItem(names, index);
                if std::ptr::eq(name, &raw mut Py_None) {
                    continue;
                }
                let name = strings::PyUnicode_AsUTF8(name);
                if name.is_null() {
                    return Err(());
                }
                let doc = sequences::PyTuple_GetItem(docs, index);
                let doc = if std::ptr::eq(doc, &raw mut Py_None) {
                    std::ptr::null()
                } else {
                    strings::PyUnicode_AsUTF8(doc)
                };
                if !errors::PyErr_Occurred().is_null() {
                    return Err(());
                }
                let Some(offset) = (index as usize)
                    .checked_mul(std::mem::size_of::<*mut PyObject>())
                    .and_then(|bytes| {
                        bytes.checked_add(std::mem::offset_of!(PyTupleObject, ob_item))
                    })
                    .and_then(|bytes| Py_ssize_t::try_from(bytes).ok())
                else {
                    errors::PyErr_NoMemory();
                    return Err(());
                };
                members.push(PyMemberDef {
                    name,
                    type_: super::PY_T_OBJECT,
                    offset,
                    flags: super::PY_READONLY,
                    doc,
                });
            }
            members.push(PyMemberDef {
                name: std::ptr::null(),
                type_: 0,
                offset: 0,
                flags: 0,
                doc: std::ptr::null(),
            });
            let Some(basicsize) = ((count - visible) as usize)
                .checked_mul(std::mem::size_of::<*mut PyObject>())
                .and_then(|bytes| bytes.checked_add(std::mem::offset_of!(PyTupleObject, ob_item)))
                .and_then(|bytes| Py_ssize_t::try_from(bytes).ok())
            else {
                errors::PyErr_NoMemory();
                return Err(());
            };
            Ok(Some(Self {
                _spec: spec,
                members,
                basicsize,
            }))
        }
    }

    /// Participate in the existing physical owner inventory; the spec is an
    /// ordinary C reference, not a mirrored namespace edge.
    pub unsafe fn owned_spec(&mut self, detach: bool) -> *mut PyObject {
        if detach {
            let empty =
                unsafe { crate::api::refcount::OwnedPyObject::from_owned(std::ptr::null_mut()) };
            std::mem::replace(&mut self._spec, empty).into_ptr()
        } else {
            self._spec.as_ptr()
        }
    }

    /// The containing projection/static-binding owner outlives the C shell's
    /// admission. Table storage stays stable when this Rust owner moves.
    pub unsafe fn attach(&mut self, ty: *mut PyTypeObject) {
        unsafe {
            (*ty).tp_members = self.members.as_mut_ptr();
            (*ty).tp_basicsize = self.basicsize;
            (*ty).tp_itemsize = std::mem::size_of::<*mut PyObject>() as Py_ssize_t;
            (*ty).tp_flags |= Py_TPFLAGS_HAVE_GC;
            // Struct-sequence instances are runtime tuples, including their
            // hidden fields and class edge. Heap and caller-owned static type
            // shells must publish that same storage traversal before Ready.
            // This also lets existing native-allocation admission reject raw
            // C storage for a managed instance layout. Tuple immutability does
            // not introduce a tp_clear callback.
            (*ty).tp_traverse = Some(crate::api::memory::molt_managed_gc_traverse);
        }
    }
}

/// Extra physical storage only for a caller-owned compact static shell. The
/// runtime's existing static binding owns this stable allocation, never a
/// process-wide side registry. Heap type projections already own their tables.
pub struct ExternalStaticType {
    protocols: TypeProtocolTables,
    metadata: StructSequenceTypeProjection,
}
impl ExternalStaticType {
    pub unsafe fn prepare(bits: u64) -> Result<Box<Self>, ()> {
        let Some(metadata) = (unsafe { StructSequenceTypeProjection::prepare(bits)? }) else {
            unsafe {
                crate::api::errors::PyErr_BadInternalCall();
            }
            return Err(());
        };
        let layout = std::alloc::Layout::new::<Self>();
        let pointer = unsafe { std::alloc::alloc(layout).cast::<Self>() };
        if pointer.is_null() {
            unsafe {
                crate::api::errors::PyErr_NoMemory();
            }
            return Err(());
        }
        unsafe {
            pointer.write(Self {
                protocols: TypeProtocolTables::new(),
                metadata,
            });
            Ok(Box::from_raw(pointer))
        }
    }
    pub unsafe fn attach(&mut self, ty: *mut PyTypeObject) {
        unsafe {
            self.protocols.attach(ty);
            self.metadata.attach(ty);
        }
    }
    /// Call only after C roots/readiness are retired while the binding is live.
    pub unsafe fn detach(&mut self, ty: *mut PyTypeObject) {
        unsafe {
            (*ty).tp_as_async = std::ptr::null_mut();
            (*ty).tp_as_number = std::ptr::null_mut();
            (*ty).tp_as_sequence = std::ptr::null_mut();
            (*ty).tp_as_mapping = std::ptr::null_mut();
            (*ty).tp_as_buffer = std::ptr::null_mut();
            (*ty).tp_members = std::ptr::null_mut();
        }
    }
}
