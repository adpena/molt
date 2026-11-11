use molt_runtime_core::prelude::*;

#[cfg(not(feature = "stdlib_http"))]
use super::dec_ref_bits;

#[cfg(not(feature = "stdlib_http"))]
pub fn attr_name_bits_from_bytes(_py: &CoreGilToken, name: &[u8]) -> Option<u64> {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::builtins::attr::dynamic_attr_name_bits(py, name)
    }
}

#[cfg(not(feature = "stdlib_http"))]
pub fn call_callable0(_py: &CoreGilToken, call_bits: u64) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        unsafe { crate::call::dispatch::call_callable0(py, call_bits) }
    }
}

pub fn call_callable1(_py: &CoreGilToken, call_bits: u64, arg0: u64) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        unsafe { crate::call::dispatch::call_callable1(py, call_bits, arg0) }
    }
}

#[cfg(not(feature = "stdlib_itertools"))]
pub fn call_callable2(_py: &CoreGilToken, call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        unsafe { crate::call::dispatch::call_callable2(py, call_bits, arg0, arg1) }
    }
}

#[cfg(not(feature = "stdlib_http"))]
pub fn call_class_init_with_args(_py: &CoreGilToken, class_bits: u64, args: &[u64]) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let class_ptr = obj_from_bits(class_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        unsafe { crate::call_class_init_with_args(py, class_ptr, args) }
    }
}

pub fn missing_bits(_py: &CoreGilToken) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::missing_bits(py)
    }
}

#[cfg(not(feature = "stdlib_http"))]
pub fn molt_getattr_builtin(
    _py: &molt_runtime_core::PyToken,
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    let __root_py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };

    crate::molt_getattr_builtin_attached_body(__root_py, obj_bits, name_bits, default_bits)
}

#[cfg(not(feature = "stdlib_http"))]
pub fn attr_optional(_py: &CoreGilToken, obj_bits: u64, name: &[u8]) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let result = {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::lookup_attribute(py, obj_bits, name_bits).into_result()
    };
    dec_ref_bits(_py, name_bits);
    result.map_err(|()| MoltObject::none().bits())
}

#[cfg(not(feature = "stdlib_http"))]
pub fn builtin_classes(_py: &CoreGilToken, name: &str) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let classes = crate::builtin_classes(py);
        match name {
            "list" => classes.list,
            _ => MoltObject::none().bits(),
        }
    }
}

#[cfg(not(feature = "stdlib_http"))]
pub fn resolve_global_bits(_py: &CoreGilToken, module: &str, name: &str) -> Result<u64, u64> {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::builtins::functions_pickle::pickle_resolve_global_bits(py, module, name)
    }
}
