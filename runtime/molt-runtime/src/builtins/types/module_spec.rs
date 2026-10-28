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
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};

const MODULE_SPEC_INIT_ARGUMENT_NAMES: &[&[u8]] =
    &[b"self", b"name", b"loader", b"origin", b"is_package"];

/// Return the borrowed canonical class, or zero with an exception pending.
fn module_spec_class(py: &PyToken<'_>) -> u64 {
    let state = types_state(py);
    init_cached_runtime_class_configured(
        py,
        &state.module_spec_class,
        "ModuleSpec",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(false, true),
            native_module: None,
            layout_size: 8,
            instance_shape: None,
            native_slots: None,
        },
        |class_bits, dict_ptr| configure_module_spec_class(py, state, class_bits, dict_ptr),
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
    class_bits: u64,
    dict_ptr: *mut u8,
) -> bool {
    let module_ptr = alloc_string(py, b"_frozen_importlib");
    if module_ptr.is_null() {
        return false;
    }
    let module_bits = MoltObject::from_ptr(module_ptr).bits();
    let published = set_class_method(py, dict_ptr, "__module__", module_bits);
    dec_ref_bits(py, module_bits);
    if !published {
        return false;
    }
    let init_bits = module_spec_init_bits(py, state, class_bits);
    if !set_class_method(py, dict_ptr, "__init__", init_bits) {
        return false;
    }
    let repr_bits = builtin_func_bits(
        py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, class_bits, "__repr__"),
        crate::builtins::functions::runtime_fn_addr(
            "molt_importlib_module_spec_repr",
            molt_importlib_module_spec_repr as *const (),
        ),
        1,
    );
    if !set_class_method(py, dict_ptr, "__repr__", repr_bits) {
        return false;
    }
    let parent_bits = builtin_func_bits(
        py,
        NativeCallableSpec::function(&state.module_spec_parent_fn),
        crate::builtins::functions::runtime_fn_addr(
            "molt_importlib_module_spec_parent",
            molt_importlib_module_spec_parent as *const (),
        ),
        1,
    );
    if parent_bits == 0 || exception_pending(py) {
        return false;
    }
    let none = MoltObject::none().bits();
    let property_ptr = alloc_property_obj(py, parent_bits, none, none);
    if property_ptr.is_null() {
        return false;
    }
    let property_bits = MoltObject::from_ptr(property_ptr).bits();
    let published = set_class_method(py, dict_ptr, "parent", property_bits);
    dec_ref_bits(py, property_bits);
    if !published {
        return false;
    }
    for (name, get_slot, get_name, get_fn, set_slot, set_name, set_fn) in [
        (
            "cached",
            &state.module_spec_cached_get_fn,
            "molt_importlib_module_spec_cached_get",
            molt_importlib_module_spec_cached_get as *const (),
            &state.module_spec_cached_set_fn,
            "molt_importlib_module_spec_cached_set",
            molt_importlib_module_spec_cached_set as *const (),
        ),
        (
            "has_location",
            &state.module_spec_has_location_get_fn,
            "molt_importlib_module_spec_has_location_get",
            molt_importlib_module_spec_has_location_get as *const (),
            &state.module_spec_has_location_set_fn,
            "molt_importlib_module_spec_has_location_set",
            molt_importlib_module_spec_has_location_set as *const (),
        ),
    ] {
        let getter = builtin_func_bits(
            py,
            NativeCallableSpec::function(get_slot),
            crate::builtins::functions::runtime_fn_addr(get_name, get_fn),
            1,
        );
        if getter == 0 || exception_pending(py) {
            return false;
        }
        let setter = builtin_func_bits(
            py,
            NativeCallableSpec::function(set_slot),
            crate::builtins::functions::runtime_fn_addr(set_name, set_fn),
            2,
        );
        if setter == 0 || exception_pending(py) {
            return false;
        }
        let property_ptr = alloc_property_obj(py, getter, setter, none);
        if property_ptr.is_null() {
            return false;
        }
        let property_bits = MoltObject::from_ptr(property_ptr).bits();
        let published = set_class_method(py, dict_ptr, name, property_bits);
        dec_ref_bits(py, property_bits);
        if !published {
            return false;
        }
    }
    true
}

/// `__init__(self, name, loader=None, origin=None, is_package=None)`: the
/// positional-or-keyword signature every existing runtime and compiled
/// consumer binds against.
fn module_spec_init_bits(py: &PyToken<'_>, state: &TypesRuntimeState, class_bits: u64) -> u64 {
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
        if !unsafe {
            crate::builtins::functions::native_callable::configure_native_callable(
                py,
                obj_from_bits(bits).as_ptr().unwrap(),
                NativeCallableSpec::declared(
                    NativeCallableKind::MethodDescriptor,
                    class_bits,
                    "__init__",
                ),
            )
        } {
            dec_ref_bits(py, bits);
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

/// `self.<name> = value` through the full attribute protocol.
fn set_spec_attr(py: &PyToken<'_>, spec_bits: u64, name: &[u8], value_bits: u64) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(py, name) else {
        return false;
    };
    let _ = crate::molt_set_attr_name(spec_bits, name_bits, value_bits);
    dec_ref_bits(py, name_bits);
    !exception_pending(py)
}

/// Owned `obj.<name>` through the full attribute protocol.
fn get_spec_attr(py: &PyToken<'_>, obj_bits: u64, name: &[u8]) -> Option<u64> {
    let name_bits = attr_name_bits_from_bytes(py, name)?;
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
            return false;
        }
        MoltObject::from_ptr(list_ptr).bits()
    } else {
        none
    };
    let located = set_spec_attr(py, spec_bits, b"submodule_search_locations", locations_bits);
    dec_ref_bits(py, locations_bits);
    // ModuleSpec's origin is descriptive. File factories separately admit
    // has_location from their actual file provenance, as CPython _set_fileattr.
    let has_location = MoltObject::from_bool(false).bits();
    located
        && set_spec_attr(py, spec_bits, b"_set_fileattr", has_location)
        && set_spec_attr(py, spec_bits, b"_cached", none)
}

// Before the defining frozen-external body runs, this dictionary owns its
// initial list objects. Binding transfers those objects into the verified
// defining globals and retires the bootstrap dictionary; later reads use
// those globals, never a rediscovered sys.modules entry.
fn new_module_spec_suffix_namespace(py: &PyToken<'_>) -> u64 {
    use crate::builtins::exceptions::ExceptionValue;
    let mut owners = Vec::new();
    let mut pairs = Vec::new();
    for (name, suffixes) in [
        (
            b"SOURCE_SUFFIXES".as_slice(),
            if crate::builtins::platform::sys_platform_str() == "win32" {
                &[".py", ".pyw"][..]
            } else {
                &[".py"][..]
            },
        ),
        (b"BYTECODE_SUFFIXES".as_slice(), &[".pyc"][..]),
    ] {
        let Some(key) = attr_name_bits_from_bytes(py, name) else {
            return 0;
        };
        owners.push(ExceptionValue::adopt(py, key));
        let mut values = Vec::new();
        for suffix in suffixes {
            let ptr = alloc_string(py, suffix.as_bytes());
            if ptr.is_null() {
                return 0;
            }
            let value = MoltObject::from_ptr(ptr).bits();
            owners.push(ExceptionValue::adopt(py, value));
            values.push(value);
        }
        let ptr = alloc_list(py, &values);
        if ptr.is_null() {
            return 0;
        }
        let list = MoltObject::from_ptr(ptr).bits();
        owners.push(ExceptionValue::adopt(py, list));
        pairs.extend_from_slice(&[key, list]);
    }
    let ptr = alloc_dict_with_pairs(py, &pairs);
    if ptr.is_null() {
        0
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn module_spec_suffix_namespace(py: &PyToken<'_>) -> u64 {
    let state = types_state(py);
    init_atomic_bits(py, &state.module_spec_suffix_namespace, || {
        state
            .module_spec_suffix_bound
            .store(MoltObject::from_bool(false).bits(), AtomicOrdering::Release);
        new_module_spec_suffix_namespace(py)
    })
}

pub(crate) fn module_spec_suffix_value_from_namespace(
    py: &PyToken<'_>,
    namespace: u64,
    name: &[u8],
) -> Option<u64> {
    let key = attr_name_bits_from_bytes(py, name)?;
    let ptr = obj_from_bits(namespace).as_ptr()?;
    let result = unsafe { dict_get_in_place(py, ptr, key) };
    dec_ref_bits(py, key);
    if exception_pending(py) {
        return None;
    }
    let Some(result) = result.ok()? else {
        let _ = raise_exception::<u64>(
            py,
            "NameError",
            &format!("name '{}' is not defined", String::from_utf8_lossy(name)),
        );
        return None;
    };
    inc_ref_bits(py, result);
    Some(result)
}

pub(crate) fn module_spec_suffix_value(py: &PyToken<'_>, name: &[u8]) -> Option<u64> {
    let namespace = module_spec_suffix_namespace(py);
    if namespace == 0 {
        return None;
    }
    module_spec_suffix_value_from_namespace(py, namespace, name)
}

/// Owned staging namespace. No defining globals or canonical root are changed.
pub(crate) fn stage_module_spec_suffix_namespace(py: &PyToken<'_>) -> u64 {
    let state = types_state(py);
    let current = module_spec_suffix_namespace(py);
    if current == 0 {
        return 0;
    }
    // Re-executing the defining body creates fresh defaults, as its Python
    // assignments do; machinery keeps any previously exported list objects.
    if state.module_spec_suffix_bound.load(AtomicOrdering::Acquire)
        == MoltObject::from_bool(true).bits()
    {
        new_module_spec_suffix_namespace(py)
    } else {
        inc_ref_bits(py, current);
        current
    }
}

/// The verified defining body calls this only after its assignments succeed.
/// Reload assignments already modify the bound dictionary incrementally; a
/// different later defining module cannot replace the first successful owner.
pub(crate) fn commit_module_spec_suffix_namespace(py: &PyToken<'_>, globals: u64) -> bool {
    use crate::builtins::exceptions::ExceptionValue;
    let state = types_state(py);
    if state.module_spec_suffix_bound.load(AtomicOrdering::Acquire)
        == MoltObject::from_bool(true).bits()
    {
        return true;
    }
    for name in [
        b"SOURCE_SUFFIXES".as_slice(),
        b"BYTECODE_SUFFIXES".as_slice(),
    ] {
        let Some(value) = module_spec_suffix_value_from_namespace(py, globals, name) else {
            return false;
        };
        let _value = ExceptionValue::adopt(py, value);
    }
    if state.module_spec_suffix_bound.load(AtomicOrdering::Acquire)
        == MoltObject::from_bool(true).bits()
    {
        return true;
    }
    inc_ref_bits(py, globals);
    let displaced = state
        .module_spec_suffix_namespace
        .swap(globals, AtomicOrdering::AcqRel);
    state
        .module_spec_suffix_bound
        .store(MoltObject::from_bool(true).bits(), AtomicOrdering::Release);
    if displaced != 0 {
        dec_ref_bits(py, displaced);
    }
    !exception_pending(py)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_suffixes() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::ExceptionValue;
        let Some(source) = module_spec_suffix_value(py, b"SOURCE_SUFFIXES") else {
            return MoltObject::none().bits();
        };
        let source = ExceptionValue::adopt(py, source);
        let Some(bytecode) = module_spec_suffix_value(py, b"BYTECODE_SUFFIXES") else {
            return MoltObject::none().bits();
        };
        let bytecode = ExceptionValue::adopt(py, bytecode);
        let ptr = alloc_tuple(py, &[source.bits(), bytecode.bits()]);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// CPython's `_get_cached`: suffix dispatch is a Python method call, while
/// source cache paths use the same runtime policy as cache_from_source.
fn module_spec_cached_from_origin(py: &PyToken<'_>, origin: u64) -> Option<u64> {
    use crate::builtins::exceptions::ExceptionValue;
    for name in [
        b"SOURCE_SUFFIXES".as_slice(),
        b"BYTECODE_SUFFIXES".as_slice(),
    ] {
        let endswith = ExceptionValue::adopt(py, get_spec_attr(py, origin, b"endswith")?);
        let policy = ExceptionValue::adopt(py, module_spec_suffix_value(py, name)?);
        let suffixes =
            ExceptionValue::adopt(py, unsafe { tuple_from_iter_bits(py, policy.bits()) }?);
        let result = ExceptionValue::adopt(py, unsafe {
            call_callable1(py, endswith.bits(), suffixes.bits())
        });
        if exception_pending(py) {
            return None;
        }
        if !crate::object::ops::truth_value(py, obj_from_bits(result.bits())).ok()? {
            continue;
        }
        if name == b"BYTECODE_SUFFIXES" {
            inc_ref_bits(py, origin);
            return Some(origin);
        }
        let cached = crate::builtins::platform::molt_importlib_cache_from_source(origin);
        if exception_pending(py) {
            dec_ref_bits(py, cached);
            if crate::builtins::platform::clear_pending_if_kind(py, &["NotImplementedError"]) {
                return Some(MoltObject::none().bits());
            }
            return None;
        }
        return Some(cached);
    }
    Some(MoltObject::none().bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_cached_get(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::exceptions::ExceptionValue;
        let result = (|| -> Option<u64> {
            let cached = ExceptionValue::adopt(py, get_spec_attr(py, self_bits, b"_cached")?);
            if obj_from_bits(cached.bits()).is_none() {
                let origin = ExceptionValue::adopt(py, get_spec_attr(py, self_bits, b"origin")?);
                if !obj_from_bits(origin.bits()).is_none() {
                    let located =
                        ExceptionValue::adopt(py, get_spec_attr(py, self_bits, b"_set_fileattr")?);
                    if crate::object::ops::truth_value(py, obj_from_bits(located.bits())).ok()? {
                        let origin =
                            ExceptionValue::adopt(py, get_spec_attr(py, self_bits, b"origin")?);
                        let cached = ExceptionValue::adopt(
                            py,
                            module_spec_cached_from_origin(py, origin.bits())?,
                        );
                        if !set_spec_attr(py, self_bits, b"_cached", cached.bits()) {
                            return None;
                        }
                    }
                }
            }
            get_spec_attr(py, self_bits, b"_cached")
        })();
        result.unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_cached_set(self_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = set_spec_attr(py, self_bits, b"_cached", value_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_has_location_get(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        get_spec_attr(py, self_bits, b"_set_fileattr").unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_has_location_set(
    self_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Ok(value) = crate::object::ops::truth_value(py, obj_from_bits(value_bits)) {
            let _ = set_spec_attr(
                py,
                self_bits,
                b"_set_fileattr",
                MoltObject::from_bool(value).bits(),
            );
        }
        MoltObject::none().bits()
    })
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
        // CPython's field order is observable through attribute getters and
        // __repr__/__format__: read each optional field for the condition,
        // then read it again only when the field is included.
        let mut fields = Vec::new();
        for (attr, optional, use_repr) in [
            (b"name".as_slice(), false, true),
            (b"loader".as_slice(), false, true),
            (b"origin".as_slice(), true, true),
            (b"submodule_search_locations".as_slice(), true, false),
        ] {
            if optional {
                let Some(probe) = get_spec_attr(py, self_bits, attr) else {
                    return MoltObject::none().bits();
                };
                let include = !obj_from_bits(probe).is_none();
                dec_ref_bits(py, probe);
                if !include {
                    continue;
                }
            }
            if !fields.is_empty() {
                fields.extend_from_slice(b", ");
            }
            fields.extend_from_slice(attr);
            fields.push(b'=');
            let Some(value) = get_spec_attr(py, self_bits, attr) else {
                return MoltObject::none().bits();
            };
            if !append_spec_formatted_value(py, value, use_repr, &mut fields) {
                return MoltObject::none().bits();
            }
        }
        let Some(class_bits) = get_spec_attr(py, self_bits, b"__class__") else {
            return MoltObject::none().bits();
        };
        let class_name = get_spec_attr(py, class_bits, b"__name__");
        dec_ref_bits(py, class_bits);
        let Some(class_name) = class_name else {
            return MoltObject::none().bits();
        };
        let mut out = Vec::new();
        if !append_spec_formatted_value(py, class_name, false, &mut out) {
            return MoltObject::none().bits();
        }
        out.push(b'(');
        out.extend_from_slice(&fields);
        out.push(b')');
        let out_ptr = alloc_string(py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

fn append_spec_formatted_value(
    py: &PyToken<'_>,
    value_bits: u64,
    use_repr: bool,
    out: &mut Vec<u8>,
) -> bool {
    let formatted = if use_repr {
        molt_repr_from_obj(value_bits)
    } else {
        let empty_ptr = alloc_string(py, b"");
        if empty_ptr.is_null() {
            dec_ref_bits(py, value_bits);
            return false;
        }
        let empty = MoltObject::from_ptr(empty_ptr).bits();
        let formatted = crate::molt_format_builtin(value_bits, empty);
        dec_ref_bits(py, empty);
        formatted
    };
    dec_ref_bits(py, value_bits);
    if exception_pending(py) {
        dec_ref_bits(py, formatted);
        return false;
    }
    // Both public renderer entrypoints own callback validation and cleanup.
    let ptr = obj_from_bits(formatted)
        .as_ptr()
        .expect("validated renderer string");
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(crate::string_bytes(ptr), crate::string_len(ptr))
    });
    dec_ref_bits(py, formatted);
    true
}

/// The canonical class for the `importlib.machinery` facade (owned).
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_type() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::state::cache::retain_cached_result(py, module_spec_class(py))
    })
}

#[cfg(test)]
#[path = "module_spec/tests.rs"]
mod tests;
