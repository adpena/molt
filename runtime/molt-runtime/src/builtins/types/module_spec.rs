//! Canonical `importlib.machinery.ModuleSpec` class.
//!
//! One runtime-owned class serves the Python facade, runtime importlib
//! consumers and extension initialization. Runtime construction runs the
//! same initializer as `ModuleSpec.__init__` without importing
//! `importlib.machinery`, so static initializers and direct dynamic loaders
//! need no application bootstrap. The class is an ordinary mutable,
//! subclassable heap type cached in the `types` runtime state; shutdown
//! releases it with the other mutable runtime classes.

use super::*;

const MODULE_SPEC_INIT_ARGUMENT_NAMES: &[&[u8]] =
    &[b"self", b"name", b"loader", b"origin", b"is_package"];

/// Return the borrowed canonical class, or zero with an exception pending.
fn module_spec_class(py: &PyToken<'_>) -> u64 {
    let state = types_state(py);
    init_cached_runtime_class_configured(
        py,
        &state.module_spec_class,
        "ModuleSpec",
        8,
        None,
        |_class_bits, dict_ptr| configure_module_spec_class(py, state, dict_ptr),
    )
}

/// Construct one owned spec as `ModuleSpec(name, loader, origin, is_package)`
/// does. The canonical initializer runs directly; neither the facade module
/// nor a replaced `ModuleSpec.__init__` is consulted. Returns `None` with an
/// exception pending on failure.
pub(crate) fn alloc_module_spec(
    py: &PyToken<'_>,
    name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
) -> Option<u64> {
    if exception_pending(py) {
        return None;
    }
    let class_bits = module_spec_class(py);
    if class_bits == 0 {
        return None;
    }
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    let spec_bits = unsafe { alloc_instance_for_class(py, class_ptr) };
    if exception_pending(py) || obj_from_bits(spec_bits).as_ptr().is_none() {
        dec_ref_bits(py, spec_bits);
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(py, "MemoryError", "ModuleSpec allocation failed");
        }
        return None;
    }
    if !initialize_module_spec(
        py,
        spec_bits,
        name_bits,
        loader_bits,
        origin_bits,
        is_package_bits,
    ) {
        dec_ref_bits(py, spec_bits);
        return None;
    }
    Some(spec_bits)
}

fn configure_module_spec_class(
    py: &PyToken<'_>,
    state: &TypesRuntimeState,
    dict_ptr: *mut u8,
) -> bool {
    let module_ptr = alloc_string(py, b"importlib.machinery");
    if module_ptr.is_null() {
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(py, "MemoryError", "class module allocation failed");
        }
        return false;
    }
    let module_bits = MoltObject::from_ptr(module_ptr).bits();
    let published = set_class_method(py, dict_ptr, "__module__", module_bits);
    dec_ref_bits(py, module_bits);
    if !published {
        return false;
    }
    let init_bits = module_spec_init_bits(py, state);
    if !set_class_method(py, dict_ptr, "__init__", init_bits) {
        return false;
    }
    let repr_bits = builtin_func_bits(
        py,
        &state.module_spec_repr_fn,
        molt_importlib_module_spec_repr as *const () as usize as u64,
        1,
    );
    if !set_class_method(py, dict_ptr, "__repr__", repr_bits) {
        return false;
    }
    let parent_bits = builtin_func_bits(
        py,
        &state.module_spec_parent_fn,
        molt_importlib_module_spec_parent as *const () as usize as u64,
        1,
    );
    if parent_bits == 0 || exception_pending(py) {
        return false;
    }
    let none = MoltObject::none().bits();
    let property_ptr = alloc_property_obj(py, parent_bits, none, none);
    if property_ptr.is_null() {
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(py, "MemoryError", "property allocation failed");
        }
        return false;
    }
    let property_bits = MoltObject::from_ptr(property_ptr).bits();
    let published = set_class_method(py, dict_ptr, "parent", property_bits);
    dec_ref_bits(py, property_bits);
    published
}

/// `__init__(self, name, loader=None, origin=None, is_package=None)`: the
/// positional-or-keyword signature every existing runtime and compiled
/// consumer binds against.
fn module_spec_init_bits(py: &PyToken<'_>, state: &TypesRuntimeState) -> u64 {
    if exception_pending(py) {
        return 0;
    }
    init_atomic_bits(py, &state.module_spec_init_fn, || {
        let none = MoltObject::none().bits();
        let bits = crate::builtins::methods::alloc_builtin_function_with_defaults(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "molt_importlib_module_spec_init",
                molt_importlib_module_spec_init as *const (),
            ),
            5,
            &[none, none, none],
        );
        if bits == 0 {
            return 0;
        }
        if !crate::builtins::methods::configure_builtin_signature(
            py,
            bits,
            MODULE_SPEC_INIT_ARGUMENT_NAMES,
            false,
            false,
        ) {
            dec_ref_bits(py, bits);
            return 0;
        }
        bits
    })
}

fn exact_str_ptr(py: &PyToken<'_>, bits: u64) -> Option<*mut u8> {
    obj_from_bits(bits).as_ptr().filter(|&ptr| {
        (unsafe { object_type_id(ptr) }) == TYPE_ID_STRING
            && type_of_bits(py, bits) == builtin_classes(py).str
    })
}

fn spec_attr_name(py: &PyToken<'_>, name: &[u8]) -> Option<u64> {
    let bits = attr_name_bits_from_bytes(py, name);
    if bits.is_none() && !exception_pending(py) {
        let _ = raise_exception::<u64>(py, "MemoryError", "attribute name allocation failed");
    }
    bits
}

/// `self.<name> = value` through the full attribute protocol.
fn set_spec_attr(py: &PyToken<'_>, spec_bits: u64, name: &[u8], value_bits: u64) -> bool {
    let Some(name_bits) = spec_attr_name(py, name) else {
        return false;
    };
    let _ = crate::molt_set_attr_name(spec_bits, name_bits, value_bits);
    dec_ref_bits(py, name_bits);
    !exception_pending(py)
}

/// Owned `obj.<name>` through the full attribute protocol.
fn get_spec_attr(py: &PyToken<'_>, obj_bits: u64, name: &[u8]) -> Option<u64> {
    let name_bits = spec_attr_name(py, name)?;
    let value_bits = crate::molt_get_attr_name(obj_bits, name_bits);
    dec_ref_bits(py, name_bits);
    if exception_pending(py) {
        dec_ref_bits(py, value_bits);
        return None;
    }
    Some(value_bits)
}

/// The one initializer shared by `ModuleSpec.__init__` and runtime
/// construction. Statement order, `str(name)` coercion and truthiness of
/// `is_package` follow the facade class this replaces.
fn initialize_module_spec(
    py: &PyToken<'_>,
    spec_bits: u64,
    name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
) -> bool {
    if exception_pending(py) {
        return false;
    }
    let name = if exact_str_ptr(py, name_bits).is_some() {
        inc_ref_bits(py, name_bits);
        name_bits
    } else {
        let bits = unsafe { call_callable1(py, builtin_classes(py).str, name_bits) };
        if exception_pending(py) {
            dec_ref_bits(py, bits);
            return false;
        }
        bits
    };
    let named = set_spec_attr(py, spec_bits, b"name", name);
    dec_ref_bits(py, name);
    let none = MoltObject::none().bits();
    if !named
        || !set_spec_attr(py, spec_bits, b"loader", loader_bits)
        || !set_spec_attr(py, spec_bits, b"origin", origin_bits)
        || !set_spec_attr(py, spec_bits, b"loader_state", none)
        || !set_spec_attr(py, spec_bits, b"cached", none)
    {
        return false;
    }
    let is_package = is_truthy(py, obj_from_bits(is_package_bits));
    if exception_pending(py) {
        return false;
    }
    let locations_bits = if is_package {
        let list_ptr = alloc_list(py, &[]);
        if list_ptr.is_null() {
            if !exception_pending(py) {
                let _ = raise_exception::<u64>(py, "MemoryError", "list allocation failed");
            }
            return false;
        }
        MoltObject::from_ptr(list_ptr).bits()
    } else {
        none
    };
    let located = set_spec_attr(py, spec_bits, b"submodule_search_locations", locations_bits);
    dec_ref_bits(py, locations_bits);
    let has_location = MoltObject::from_bool(!obj_from_bits(origin_bits).is_none()).bits();
    located && set_spec_attr(py, spec_bits, b"has_location", has_location)
}

/// Owned `name.rpartition(".")[0]`.
fn module_parent_of_name(py: &PyToken<'_>, name_bits: u64) -> u64 {
    if let Some(name_ptr) = exact_str_ptr(py, name_bits) {
        let bytes = unsafe {
            std::slice::from_raw_parts(crate::string_bytes(name_ptr), crate::string_len(name_ptr))
        };
        let head = bytes
            .iter()
            .rposition(|&byte| byte == b'.')
            .map_or(&bytes[..0], |index| &bytes[..index]);
        let head_ptr = alloc_string(py, head);
        if head_ptr.is_null() {
            if !exception_pending(py) {
                let _ = raise_exception::<u64>(py, "MemoryError", "string allocation failed");
            }
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(head_ptr).bits();
    }
    let Some(rpartition_bits) = get_spec_attr(py, name_bits, b"rpartition") else {
        return MoltObject::none().bits();
    };
    let dot_ptr = alloc_string(py, b".");
    if dot_ptr.is_null() {
        dec_ref_bits(py, rpartition_bits);
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(py, "MemoryError", "string allocation failed");
        }
        return MoltObject::none().bits();
    }
    let dot_bits = MoltObject::from_ptr(dot_ptr).bits();
    let parts_bits = unsafe { call_callable1(py, rpartition_bits, dot_bits) };
    dec_ref_bits(py, dot_bits);
    dec_ref_bits(py, rpartition_bits);
    if exception_pending(py) {
        dec_ref_bits(py, parts_bits);
        return MoltObject::none().bits();
    }
    let head_bits = crate::molt_index(parts_bits, MoltObject::from_int(0).bits());
    dec_ref_bits(py, parts_bits);
    head_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_init(
    self_bits: u64,
    name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = initialize_module_spec(
            py,
            self_bits,
            name_bits,
            loader_bits,
            origin_bits,
            is_package_bits,
        );
        MoltObject::none().bits()
    })
}

/// Getter of the `ModuleSpec.parent` property.
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_parent(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(locations_bits) = get_spec_attr(py, self_bits, b"submodule_search_locations")
        else {
            return MoltObject::none().bits();
        };
        let is_package = !obj_from_bits(locations_bits).is_none();
        dec_ref_bits(py, locations_bits);
        let Some(name_bits) = get_spec_attr(py, self_bits, b"name") else {
            return MoltObject::none().bits();
        };
        if is_package {
            return name_bits;
        }
        let parent_bits = module_parent_of_name(py, name_bits);
        dec_ref_bits(py, name_bits);
        parent_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let mut out = b"ModuleSpec(".to_vec();
        for (index, (attr, label)) in [
            (b"name".as_slice(), b"name=".as_slice()),
            (b"loader".as_slice(), b"loader=".as_slice()),
            (b"origin".as_slice(), b"origin=".as_slice()),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                out.extend_from_slice(b", ");
            }
            out.extend_from_slice(label);
            let Some(value_bits) = get_spec_attr(py, self_bits, attr) else {
                return MoltObject::none().bits();
            };
            let repr_bits = molt_repr_from_obj(value_bits);
            dec_ref_bits(py, value_bits);
            if exception_pending(py) {
                dec_ref_bits(py, repr_bits);
                return MoltObject::none().bits();
            }
            let Some(repr_ptr) = obj_from_bits(repr_bits)
                .as_ptr()
                .filter(|&ptr| unsafe { object_type_id(ptr) } == TYPE_ID_STRING)
            else {
                dec_ref_bits(py, repr_bits);
                return raise_exception::<_>(py, "TypeError", "__repr__ returned non-string");
            };
            out.extend_from_slice(unsafe {
                std::slice::from_raw_parts(
                    crate::string_bytes(repr_ptr),
                    crate::string_len(repr_ptr),
                )
            });
            dec_ref_bits(py, repr_bits);
        }
        out.push(b')');
        let out_ptr = alloc_string(py, &out);
        if out_ptr.is_null() {
            if !exception_pending(py) {
                let _ = raise_exception::<u64>(py, "MemoryError", "string allocation failed");
            }
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

/// The canonical class for the `importlib.machinery` facade (owned).
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_type() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let class_bits = module_spec_class(py);
        if class_bits == 0 {
            return MoltObject::none().bits();
        }
        inc_ref_bits(py, class_bits);
        class_bits
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn str_bits(py: &PyToken<'_>, value: &[u8]) -> u64 {
        let ptr = alloc_string(py, value);
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn attr(py: &PyToken<'_>, obj_bits: u64, name: &[u8]) -> u64 {
        let value = get_spec_attr(py, obj_bits, name).expect("ModuleSpec attribute");
        assert!(!exception_pending(py));
        value
    }

    fn attr_text(py: &PyToken<'_>, obj_bits: u64, name: &[u8]) -> String {
        let value = attr(py, obj_bits, name);
        let text = string_obj_to_owned(obj_from_bits(value)).expect("str attribute");
        dec_ref_bits(py, value);
        text
    }

    extern "C" fn observed_setattr(self_bits: u64, name_bits: u64, value_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            if string_obj_to_owned(obj_from_bits(name_bits)).as_deref() == Some("name") {
                let observed = str_bits(py, b"observed.leaf");
                let result = molt_object_setattr(self_bits, name_bits, observed);
                dec_ref_bits(py, observed);
                result
            } else {
                molt_object_setattr(self_bits, name_bits, value_bits)
            }
        })
    }

    #[test]
    fn runtime_factory_obeys_mutable_class_attribute_protocol() {
        struct RemoveAttribute(u64, u64);
        impl Drop for RemoveAttribute {
            fn drop(&mut self) {
                crate::with_gil_entry_nopanic!(py, {
                    crate::molt_del_attr_name(self.0, self.1);
                    dec_ref_bits(py, self.1);
                    dec_ref_bits(py, self.0);
                });
            }
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = module_spec_class(py);
            let setter = crate::builtins::methods::alloc_builtin_function(
                py,
                observed_setattr as *const () as usize as u64,
                3,
            );
            let setter_name = str_bits(py, b"__setattr__");
            inc_ref_bits(py, class);
            let _restore_class = RemoveAttribute(class, setter_name);
            crate::molt_set_attr_name(class, setter_name, setter);
            dec_ref_bits(py, setter);
            assert!(!exception_pending(py));
            let name = str_bits(py, b"original");
            let none = MoltObject::none().bits();
            let spec = alloc_module_spec(py, name, none, none, none).expect("observed spec");
            assert_eq!(attr_text(py, spec, b"name"), "observed.leaf");
            assert_eq!(attr_text(py, spec, b"parent"), "observed");
            for bits in [spec, name] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn runtime_factory_and_facade_type_share_one_canonical_class() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class_bits = module_spec_class(py);
            assert_ne!(class_bits, 0);
            let facade_bits = molt_importlib_module_spec_type();
            assert_eq!(facade_bits, class_bits);
            dec_ref_bits(py, facade_bits);

            let none = MoltObject::none().bits();
            let name = str_bits(py, b"pkg.leaf");
            let origin = str_bits(py, b"/tmp/leaf.so");
            let not_package = MoltObject::from_bool(false).bits();
            let spec = alloc_module_spec(py, name, none, origin, not_package).expect("module spec");
            assert_eq!(type_of_bits(py, spec), class_bits);
            assert_eq!(attr_text(py, spec, b"name"), "pkg.leaf");
            assert_eq!(attr_text(py, spec, b"origin"), "/tmp/leaf.so");
            assert_eq!(attr_text(py, spec, b"parent"), "pkg");
            for (field, expected) in [
                (b"loader".as_slice(), none),
                (b"loader_state".as_slice(), none),
                (b"cached".as_slice(), none),
                (b"submodule_search_locations".as_slice(), none),
                (
                    b"has_location".as_slice(),
                    MoltObject::from_bool(true).bits(),
                ),
            ] {
                assert_eq!(attr(py, spec, field), expected);
            }
            let repr = molt_importlib_module_spec_repr(spec);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(repr)).as_deref(),
                Some("ModuleSpec(name='pkg.leaf', loader=None, origin='/tmp/leaf.so')")
            );
            dec_ref_bits(py, repr);

            let is_package = MoltObject::from_bool(true).bits();
            let package =
                alloc_module_spec(py, name, none, none, is_package).expect("package spec");
            assert_eq!(attr_text(py, package, b"parent"), "pkg.leaf");
            assert_eq!(
                attr(py, package, b"has_location"),
                MoltObject::from_bool(false).bits()
            );
            let locations = attr(py, package, b"submodule_search_locations");
            assert_eq!(molt_len(locations), MoltObject::from_int(0).bits());
            dec_ref_bits(py, locations);

            for bits in [package, spec, origin, name] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn class_call_binds_positional_and_keyword_initializer_arguments() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class_bits = module_spec_class(py);
            assert_ne!(class_bits, 0);
            let none = MoltObject::none().bits();
            let name = str_bits(py, b"top");
            let origin = str_bits(py, b"built-in");
            let positional = unsafe { call_callable2(py, class_bits, name, none) };
            assert!(!exception_pending(py));
            assert_eq!(type_of_bits(py, positional), class_bits);
            assert_eq!(attr_text(py, positional, b"parent"), "");
            assert_eq!(attr(py, positional, b"origin"), none);

            let origin_key = str_bits(py, b"origin");
            let package_key = str_bits(py, b"is_package");
            let builder = molt_callargs_new(1, 2);
            unsafe {
                let _ = molt_callargs_push_pos(builder, name);
                let _ = molt_callargs_push_kw(builder, origin_key, origin);
                let _ = molt_callargs_push_kw(builder, package_key, MoltObject::from_int(1).bits());
            }
            let keyword = molt_call_bind(class_bits, builder);
            assert!(!exception_pending(py));
            assert_eq!(attr_text(py, keyword, b"origin"), "built-in");
            assert_eq!(attr(py, keyword, b"loader"), none);
            assert_eq!(
                attr(py, keyword, b"has_location"),
                MoltObject::from_bool(true).bits()
            );
            assert_eq!(attr_text(py, keyword, b"parent"), "top");

            let parent_name = str_bits(py, b"parent");
            let _ = molt_object_setattr(keyword, parent_name, name);
            assert!(exception_pending(py), "parent is a read-only property");
            clear_exception(py);

            for bits in [
                parent_name,
                keyword,
                package_key,
                origin_key,
                positional,
                origin,
                name,
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }
}
