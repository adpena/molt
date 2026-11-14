//! Default ModuleType methods. Subclasses enter the shared attribute protocol.

use crate::BuiltinExc;
use crate::*;

fn module_receiver(py: &PyToken<'_>, bits: u64, method: &str) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(bits).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_MODULE
    {
        return Some(ptr);
    }
    raise_exception::<_>(
        py,
        BuiltinExc::TypeError,
        &format!(
            "descriptor '{method}' requires a 'module' object but received a '{}'",
            type_name(py, obj_from_bits(bits))
        ),
    )
}

pub(crate) fn molt_module_getattribute_attached_body(
    _py: &crate::PyToken<'_>,
    module: u64,
    name: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(ptr) = module_receiver(py, module, "__getattribute__") else {
            return MoltObject::none().bits();
        };
        if string_obj_to_owned(obj_from_bits(name)).is_none() {
            return crate::builtins::attr::raise_attr_name_type_error(py, name);
        }
        match unsafe { crate::builtins::attr::module_attr_lookup(py, ptr, name).into_result() } {
            Ok(Some(value)) => return value,
            Ok(None) => {}
            Err(()) => return MoltObject::none().bits(),
        }
        unsafe { raise_module_attribute_miss(py, ptr, name) }
    })
}

/// The AttributeError a module raises once lookup and its `__getattr__` both
/// miss (CPython `_Py_module_getattro_impl`, Objects/moduleobject.c). The label
/// is `__dict__['__name__']`, so a renamed module reports its new name, and a
/// non-str or absent name drops it. `__spec__` tells a module that is still
/// initializing, or a submodule its parent has not bound yet, from a plain
/// miss. Like `PyObject_GetAttr`, the error records `name` and `obj`.
pub(crate) unsafe fn raise_module_attribute_miss(
    py: &PyToken<'_>,
    module: *mut u8,
    name: u64,
) -> u64 {
    let none = MoltObject::none().bits();
    let attribute =
        string_obj_to_owned(obj_from_bits(name)).unwrap_or_else(|| "<attr>".to_string());
    let module_bits = MoltObject::from_ptr(module).bits();
    let label = match unsafe { module_dict_entry(py, module, crate::NameId::DunderName) } {
        Ok(Some(bits)) => {
            let label = string_obj_to_owned(obj_from_bits(bits));
            dec_ref_bits(py, bits);
            label
        }
        Ok(None) => None,
        Err(()) => return none,
    };
    let Some(label) = label else {
        let message = format!("module has no attribute '{attribute}'");
        return attr_error_with_obj_message(py, &message, &attribute, module_bits);
    };
    let spec = match unsafe { module_dict_entry(py, module, crate::NameId::DunderSpec) } {
        Ok(spec) => spec,
        Err(()) => return none,
    };
    let message = match spec {
        None => Ok(format!("module '{label}' has no attribute '{attribute}'")),
        Some(spec) => {
            let message = match crate::builtins::platform::spec_is_initializing(
                py,
                spec,
                crate::builtins::module_table::ModuleImportPolicy::CImport,
            )
            .map_err(|_| ())
            {
                Err(()) => Err(()),
                Ok(true) => Ok(format!(
                    "partially initialized module '{label}' has no attribute '{attribute}' \
                     (most likely due to a circular import)"
                )),
                Ok(false) => spec_lists_uninitialized_submodule(py, spec, name).map(|listed| {
                    if listed {
                        format!(
                            "cannot access submodule '{attribute}' of module '{label}' \
                             (most likely due to a circular import)"
                        )
                    } else {
                        format!("module '{label}' has no attribute '{attribute}'")
                    }
                }),
            };
            dec_ref_bits(py, spec);
            message
        }
    };
    match message {
        Ok(message) => attr_error_with_obj_message(py, &message, &attribute, module_bits),
        Err(()) => none,
    }
}

/// An owned reference to one entry of the module's own `__dict__`.
unsafe fn module_dict_entry(
    py: &PyToken<'_>,
    module: *mut u8,
    key: crate::NameId,
) -> Result<Option<u64>, ()> {
    let Some(dict) = obj_from_bits(unsafe { module_dict_bits(module) }).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(dict) } != TYPE_ID_DICT {
        return Ok(None);
    }
    let key = key.owned_bits(py).ok_or(())?;
    let entry = unsafe { dict_get_in_place(py, dict, key) };
    dec_ref_bits(py, key);
    let entry = entry?;
    if let Some(bits) = entry {
        inc_ref_bits(py, bits);
    }
    Ok(entry)
}

/// CPython `_PyModuleSpec_IsUninitializedSubmodule`: a missing attribute is a
/// plain miss, and any other lookup or membership error propagates.
fn spec_lists_uninitialized_submodule(py: &PyToken<'_>, spec: u64, name: u64) -> Result<bool, ()> {
    let key = crate::NameId::PrivateUninitializedSubmodules
        .owned_bits(py)
        .ok_or(())?;
    let value = crate::builtins::platform::getattr_optional_bits(py, spec, key);
    dec_ref_bits(py, key);
    let Some(submodules) = value.map_err(|_| ())? else {
        return Ok(false);
    };
    let contains = crate::molt_contains_attached_body(py, submodules, name);
    dec_ref_bits(py, submodules);
    if exception_pending(py) {
        return Err(());
    }
    Ok(obj_from_bits(contains).as_bool().unwrap_or(false))
}

pub(crate) fn molt_module_setattr_attached_body(
    _py: &crate::PyToken<'_>,
    module: u64,
    name: u64,
    value: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if module_receiver(py, module, "__setattr__").is_none() {
            return MoltObject::none().bits();
        }
        let _ = crate::builtins::attributes::generic_set_attr_name(module, name, value);
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_module_delattr_attached_body(
    _py: &crate::PyToken<'_>,
    module: u64,
    name: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if module_receiver(py, module, "__delattr__").is_none() {
            return MoltObject::none().bits();
        }
        let _ = crate::builtins::attributes::generic_del_attr_name(module, name);
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_module_dir_attached_body(_py: &crate::PyToken<'_>, module: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(ptr) = module_receiver(py, module, "__dir__") else {
            return MoltObject::none().bits();
        };
        let dictionary = unsafe { module_dict_bits(ptr) };
        let Some(dict) = obj_from_bits(dictionary).as_ptr() else {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "module.__dict__ is not a dictionary",
            );
        };
        let Some(name) = crate::NameId::DunderDir.owned_bits(py) else {
            return MoltObject::none().bits();
        };
        let hook = unsafe { dict_get_in_place(py, dict, name) };
        dec_ref_bits(py, name);
        let hook = match hook {
            Ok(value) => value,
            Err(()) => return MoltObject::none().bits(),
        };
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
        // A cell-eligible module's bound globals are its static cells
        // (call_anatomy.md 4.5a), not dictionary keys.
        let mut cell_names = Vec::new();
        for (cell, _) in crate::builtins::namespace_slots::static_cell_items(dict) {
            let Some(key) = crate::dynamic_attr_name_bits(py, cell) else {
                for key in cell_names {
                    dec_ref_bits(py, key);
                }
                return MoltObject::none().bits();
            };
            cell_names.push(key);
        }
        let all: Vec<u64> = keys
            .iter()
            .copied()
            .chain(cell_names.iter().copied())
            .collect();
        let list = alloc_list(py, &all);
        for key in cell_names {
            dec_ref_bits(py, key);
        }
        if list.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list).bits()
        }
    })
}

pub(crate) fn molt_module_repr_attached_body(_py: &crate::PyToken<'_>, module: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(ptr) = module_receiver(py, module, "__repr__") else {
            return MoltObject::none().bits();
        };
        let text = unsafe { crate::object::ops_format::format_module_default(py, ptr) };
        text.into_bits(py)
    })
}

include!("../../attached_exports_generated/builtins/modules/type_attributes.rs");
