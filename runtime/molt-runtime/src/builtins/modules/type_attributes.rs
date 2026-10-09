//! Default ModuleType methods. Subclasses enter the shared attribute protocol.

use crate::*;

fn module_receiver(py: &PyToken<'_>, bits: u64, method: &str) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(bits).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_MODULE
    {
        return Some(ptr);
    }
    raise_exception::<_>(
        py,
        "TypeError",
        &format!(
            "descriptor '{method}' requires a 'module' object but received a '{}'",
            type_name(py, obj_from_bits(bits))
        ),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_getattribute(module: u64, name: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = module_receiver(py, module, "__getattribute__") else {
            return MoltObject::none().bits();
        };
        if string_obj_to_owned(obj_from_bits(name)).is_none() {
            return crate::builtins::attr::raise_attr_name_type_error(py, name);
        }
        if let Some(value) = unsafe { crate::builtins::attr::module_attr_lookup(py, ptr, name) } {
            return value;
        }
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let attribute = string_obj_to_owned(obj_from_bits(name)).unwrap();
        let module_name = unsafe { module_name_bits(ptr) };
        let message = match string_obj_to_owned(obj_from_bits(module_name)) {
            Some(label) => format!("module '{label}' has no attribute '{attribute}'"),
            None => format!("module has no attribute '{attribute}'"),
        };
        raise_exception::<_>(py, "AttributeError", &message)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_setattr(module: u64, name: u64, value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if module_receiver(py, module, "__setattr__").is_none() {
            return MoltObject::none().bits();
        }
        crate::builtins::attributes::generic_set_attr_name(module, name, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_delattr(module: u64, name: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if module_receiver(py, module, "__delattr__").is_none() {
            return MoltObject::none().bits();
        }
        crate::builtins::attributes::generic_del_attr_name(module, name)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_dir(module: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = module_receiver(py, module, "__dir__") else {
            return MoltObject::none().bits();
        };
        let dictionary = unsafe { module_dict_bits(ptr) };
        let Some(dict) = obj_from_bits(dictionary).as_ptr() else {
            return raise_exception::<_>(py, "TypeError", "module.__dict__ is not a dictionary");
        };
        let Some(name) = attr_name_bits_from_bytes(py, b"__dir__") else {
            return MoltObject::none().bits();
        };
        let hook = unsafe { dict_get_in_place(py, dict, name) };
        dec_ref_bits(py, name);
        if let Some(hook) = hook {
            inc_ref_bits(py, hook);
            let result = unsafe { call_callable0(py, hook) };
            dec_ref_bits(py, hook);
            return result;
        }
        let Some(keys) = (unsafe {
            crate::object::ops_dict::dict_snapshot(
                py,
                dict,
                crate::object::ops_dict::DictSnapshotKind::Keys,
            )
        }) else {
            return MoltObject::none().bits();
        };
        let list = alloc_list(py, &keys);
        if list.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_repr(module: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = module_receiver(py, module, "__repr__") else {
            return MoltObject::none().bits();
        };
        let text = unsafe { crate::object::ops_format::format_module_default(py, ptr) };
        text.into_bits(py)
    })
}
