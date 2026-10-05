//! Compact runtime types own all protocol destinations. Physical ownership is
//! declared once; readiness and mutation fill slots from SLOT_WRAPPER_DEFS.
use crate::abi_types::*;
use std::cell::UnsafeCell;

pub(crate) struct TypeProtocolTables {
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
    pub(crate) const fn new() -> Self {
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
    pub(crate) unsafe fn attach(&self, tp: *mut PyTypeObject) {
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
