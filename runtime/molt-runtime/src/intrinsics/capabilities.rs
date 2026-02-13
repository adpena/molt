use crate::{MoltObject, has_capability, is_trusted, raise_exception, string_obj_to_owned};

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_trusted() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(is_trusted(_py)).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_has(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(crate::obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "capability name must be str"),
        };
        MoltObject::from_bool(has_capability(_py, &name)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_require(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(crate::obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "capability name must be str"),
        };
        if !has_capability(_py, &name) {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                &format!("missing {name} capability"),
            );
        }
        MoltObject::none().bits()
    })
}
