pub(crate) mod bind;
pub(crate) mod class_init;
pub(crate) mod dispatch;
pub(crate) mod function;
pub(crate) mod type_policy;

use crate::builtins::attr::{class_attr_lookup_raw_mro, descriptor_bind};
use crate::{
    MoltObject, PyToken, TYPE_ID_STATICMETHOD, TYPE_ID_TYPE, builtin_classes, dec_ref_bits,
    exception_pending, inc_ref_bits, intern_static_name, obj_from_bits, object_class_bits,
    object_type_id, raise_exception, raise_not_callable, runtime_state, staticmethod_func_bits,
};

/// Consume the owned result of a call whose value is intentionally ignored.
///
/// All Python-call boundaries return a new owning reference, including the
/// `None` success sentinel.  Keeping the discard at a named boundary prevents
/// statement-like consumers (`__set__`, `__delete__`, and lifecycle hooks)
/// from silently leaking successful heap-valued returns.
#[inline]
pub(crate) fn discard_owned_call_result(_py: &PyToken<'_>, result_bits: u64) {
    dec_ref_bits(_py, result_bits);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallAttrLookup {
    Found(u64),
    Missing,
    Raised,
}

/// A builtin `staticmethod.__call__` transparently forwards to its wrapped
/// object. The owned target keeps the wrapper chain's recursion charge alive
/// through the target operation. Subclasses use it only while their resolved
/// __call__ is the builtin method; overrides retain ordinary Python dispatch.
pub(crate) enum StaticmethodCallTarget<'guard, 'py> {
    NotStaticmethod,
    Owned(OwnedStaticmethodCallTarget<'guard, 'py>),
    Raised,
}

struct StaticmethodResolutionDepth {
    entered: usize,
}

impl StaticmethodResolutionDepth {
    fn new() -> Self {
        Self { entered: 0 }
    }

    fn enter(&mut self, py: &PyToken<'_>) -> bool {
        if crate::state::recursion::recursion_guard_enter() {
            self.entered += 1;
            true
        } else {
            let _ = raise_exception::<u64>(
                py,
                "RecursionError",
                "maximum recursion depth exceeded while resolving staticmethod call target",
            );
            false
        }
    }
}

impl Drop for StaticmethodResolutionDepth {
    fn drop(&mut self) {
        for _ in 0..self.entered {
            crate::state::recursion::recursion_guard_exit();
        }
    }
}

pub(crate) struct OwnedStaticmethodCallTarget<'guard, 'py> {
    py: &'guard PyToken<'py>,
    bits: u64,
    _depth: StaticmethodResolutionDepth,
}

impl OwnedStaticmethodCallTarget<'_, '_> {
    #[inline]
    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }
}

impl Drop for OwnedStaticmethodCallTarget<'_, '_> {
    fn drop(&mut self) {
        // Release the retained target while the wrapper recursion charge is
        // still active. `_depth` unwinds immediately after this drop body.
        dec_ref_bits(self.py, self.bits);
    }
}

#[inline]
pub(crate) unsafe fn is_exact_staticmethod_wrapper(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STATICMETHOD {
            return false;
        }
        let class_bits = object_class_bits(ptr);
        class_bits == 0 || class_bits == builtin_classes(py).staticmethod
    }
}

unsafe fn has_builtin_staticmethod_call(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    unsafe {
        object_type_id(ptr) == TYPE_ID_STATICMETHOD
            && crate::builtins::types::wrappers::wrapper_uses_default_method(
                py,
                ptr,
                b"__call__",
                fn_addr!(crate::molt_staticmethod_call),
            )
    }
}

/// Flatten a builtin-staticmethod wrapper chain without recursive Rust calls.
/// Each successor is retained before its predecessor is released, so future
/// mutable wrapper storage cannot invalidate the next edge during resolution.
pub(crate) unsafe fn resolve_staticmethod_call_target<'guard, 'py>(
    py: &'guard PyToken<'py>,
    call_bits: u64,
) -> StaticmethodCallTarget<'guard, 'py> {
    unsafe {
        let Some(call_ptr) = obj_from_bits(call_bits).as_ptr() else {
            return StaticmethodCallTarget::NotStaticmethod;
        };
        if !has_builtin_staticmethod_call(py, call_ptr) {
            return if exception_pending(py) {
                StaticmethodCallTarget::Raised
            } else {
                StaticmethodCallTarget::NotStaticmethod
            };
        }

        let mut depth = StaticmethodResolutionDepth::new();
        let mut current_bits = call_bits;
        let mut current_owned = None;
        loop {
            let Some(current_ptr) = obj_from_bits(current_bits).as_ptr() else {
                return StaticmethodCallTarget::Owned(OwnedStaticmethodCallTarget {
                    py,
                    bits: current_owned.expect("staticmethod target must be retained"),
                    _depth: depth,
                });
            };
            if !has_builtin_staticmethod_call(py, current_ptr) {
                if exception_pending(py) {
                    if let Some(bits) = current_owned {
                        dec_ref_bits(py, bits);
                    }
                    return StaticmethodCallTarget::Raised;
                }
                return StaticmethodCallTarget::Owned(OwnedStaticmethodCallTarget {
                    py,
                    bits: current_owned.expect("staticmethod target must be retained"),
                    _depth: depth,
                });
            }
            if !depth.enter(py) {
                if let Some(bits) = current_owned {
                    dec_ref_bits(py, bits);
                }
                return StaticmethodCallTarget::Raised;
            }

            let next_bits = staticmethod_func_bits(current_ptr);
            if crate::builtins::methods::is_missing_bits(py, next_bits) {
                if let Some(bits) = current_owned {
                    dec_ref_bits(py, bits);
                }
                let _ =
                    raise_exception::<u64>(py, "RuntimeError", "uninitialized staticmethod object");
                return StaticmethodCallTarget::Raised;
            }
            inc_ref_bits(py, next_bits);
            if let Some(previous_bits) = current_owned.replace(next_bits) {
                dec_ref_bits(py, previous_bits);
            }
            current_bits = next_bits;
        }
    }
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
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        // Raw special-method lookup returns a borrowed descriptor. Keep both
        // arguments alive through descriptor binding because `__get__` may run
        // arbitrary user code and drop their original owners.
        inc_ref_bits(_py, class_bits);
        inc_ref_bits(_py, obj_bits);
        let call_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
        let result = match class_attr_lookup_raw_mro(_py, class_ptr, call_name_bits) {
            Some(_) if exception_pending(_py) => CallAttrLookup::Raised,
            Some(raw_bits) => {
                match descriptor_bind(_py, raw_bits, Some(class_bits), Some(obj_bits)) {
                    Some(bits) if exception_pending(_py) => {
                        dec_ref_bits(_py, bits);
                        CallAttrLookup::Raised
                    }
                    Some(bits) => CallAttrLookup::Found(bits),
                    None if exception_pending(_py) => CallAttrLookup::Raised,
                    None => CallAttrLookup::Missing,
                }
            }
            None if exception_pending(_py) => CallAttrLookup::Raised,
            None => CallAttrLookup::Missing,
        };
        dec_ref_bits(_py, obj_bits);
        dec_ref_bits(_py, class_bits);
        result
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
