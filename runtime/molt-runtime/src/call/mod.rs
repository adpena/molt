pub(crate) mod bind;
pub(crate) mod class_init;
pub(crate) mod dispatch;
pub(crate) mod function;

use crate::{PyToken, attr_lookup_ptr, intern_static_name, runtime_state};

pub(crate) unsafe fn lookup_call_attr(_py: &PyToken<'_>, obj_ptr: *mut u8) -> Option<u64> {
    unsafe {
        let call_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
        attr_lookup_ptr(_py, obj_ptr, call_name_bits)
    }
}
