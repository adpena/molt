use molt_runtime_core::prelude::*;

use super::{dec_ref_bits, exception_pending};

pub fn attr_name_bits_from_bytes(_py: &CoreGilToken, name: &[u8]) -> Option<u64> {
    crate::with_gil_entry_nopanic!(py, {
        crate::builtins::attr::attr_name_bits_from_bytes(py, name)
    })
}

pub fn call_callable0(_py: &CoreGilToken, call_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::call::dispatch::call_callable0(py, call_bits) }
    })
}

pub fn call_callable1(_py: &CoreGilToken, call_bits: u64, arg0: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::call::dispatch::call_callable1(py, call_bits, arg0) }
    })
}

pub fn call_callable2(_py: &CoreGilToken, call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::call::dispatch::call_callable2(py, call_bits, arg0, arg1) }
    })
}

pub fn call_class_init_with_args(_py: &CoreGilToken, class_bits: u64, args: &[u64]) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let class_ptr = obj_from_bits(class_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        unsafe { crate::call_class_init_with_args(py, class_ptr, args) }
    })
}

pub fn missing_bits(_py: &CoreGilToken) -> u64 {
    crate::with_gil_entry_nopanic!(py, { crate::missing_bits(py) })
}

pub fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    crate::molt_getattr_builtin(obj_bits, name_bits, default_bits)
}

pub fn attr_optional(_py: &CoreGilToken, obj_bits: u64, name: &[u8]) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if crate::with_gil_entry_nopanic!(py, {
            crate::builtins::attr::clear_attribute_error_if_pending(py)
        }) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

pub fn molt_repr_from_obj(bits: u64) -> u64 {
    crate::molt_repr_from_obj(bits)
}

pub fn molt_str_from_obj(bits: u64) -> u64 {
    crate::molt_str_from_obj(bits)
}

pub fn builtin_classes(_py: &CoreGilToken, name: &str) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let classes = crate::builtin_classes(py);
        match name {
            "list" => classes.list,
            _ => MoltObject::none().bits(),
        }
    })
}

pub fn resolve_global_bits(_py: &CoreGilToken, module: &str, name: &str) -> Result<u64, u64> {
    crate::with_gil_entry_nopanic!(py, {
        crate::builtins::functions_pickle::pickle_resolve_global_bits(py, module, name)
    })
}
