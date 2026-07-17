pub(crate) mod bind;
pub(crate) mod class_init;
pub(crate) mod dispatch;
pub(crate) mod function;
pub(crate) mod type_policy;

use crate::builtins::attr::{class_attr_lookup, class_attr_lookup_raw_mro};
use crate::{
    MoltObject, PyToken, TYPE_ID_TYPE, exception_pending, intern_static_name, obj_from_bits,
    object_class_bits, object_type_id, raise_not_callable, runtime_state,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallAttrLookup {
    Found(u64),
    Missing,
    Raised,
}

unsafe fn call_class_ptr(obj_ptr: *mut u8) -> Option<*mut u8> {
    let class_bits = unsafe { object_class_bits(obj_ptr) };
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    (unsafe { object_type_id(class_ptr) } == TYPE_ID_TYPE).then_some(class_ptr)
}

pub(crate) unsafe fn lookup_call_attr(_py: &PyToken<'_>, obj_ptr: *mut u8) -> CallAttrLookup {
    unsafe {
        let Some(class_ptr) = call_class_ptr(obj_ptr) else {
            return CallAttrLookup::Missing;
        };
        let call_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
        match class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), call_name_bits) {
            Some(bits) if exception_pending(_py) => {
                crate::dec_ref_bits(_py, bits);
                CallAttrLookup::Raised
            }
            Some(bits) => CallAttrLookup::Found(bits),
            None if exception_pending(_py) => CallAttrLookup::Raised,
            None => CallAttrLookup::Missing,
        }
    }
}

pub(crate) unsafe fn require_call_attr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    obj: MoltObject,
) -> Result<u64, u64> {
    unsafe {
        match lookup_call_attr(_py, obj_ptr) {
            CallAttrLookup::Found(bits) => Ok(bits),
            CallAttrLookup::Missing => Err(raise_not_callable(_py, obj)),
            CallAttrLookup::Raised => Err(MoltObject::none().bits()),
        }
    }
}

pub(crate) unsafe fn has_type_call_attr(_py: &PyToken<'_>, obj_ptr: *mut u8) -> bool {
    unsafe {
        let Some(class_ptr) = call_class_ptr(obj_ptr) else {
            return false;
        };
        let call_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
        class_attr_lookup_raw_mro(_py, class_ptr, call_name_bits).is_some()
    }
}
