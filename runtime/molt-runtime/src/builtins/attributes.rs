use crate::BuiltinExc;
use crate::PyToken;
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use molt_obj_model::MoltObject;
#[cfg(test)]
pub(crate) use mutation::{
    molt_del_attr_generic_attached_body, molt_del_attr_object_attached_body,
    molt_del_attr_ptr_attached_body, molt_set_attr_generic_attached_body,
    molt_set_attr_object_ic_attached_body, molt_set_attr_ptr_attached_body,
};
use std::sync::atomic::Ordering;

use crate::async_rt::generators::generator_yieldfrom_bits;
use crate::builtins::annotations::pep649_enabled;
#[cfg(test)]
use crate::builtins::attr::exception_is_attribute_error;
use crate::builtins::attr::{
    AttributeLookupMode, DescriptorCallPolicy, class_slots_info, clear_attribute_error_if_pending,
};
use crate::builtins::containers::tuple_method_bits;
#[cfg(test)]
use crate::builtins::exceptions::exception_matches_kind;
use crate::builtins::frames::suspended_frame_bits;
use crate::builtins::methods::{
    asyncgen_method_bits, complex_method_bits, generator_method_bits, object_method_bits,
    range_method_bits, type_method_bits,
};
use crate::*;

pub(crate) mod callable_metadata;
mod class_lookup;
mod field_sites;
#[cfg(test)]
mod field_sites_tests;
mod generic_parameters;
mod mutation;
#[cfg(test)]
mod native_dictionary_tests;
mod scalar_attrs;
mod state;
pub(crate) mod type_metadata;
mod wrapper_attrs;

pub(crate) use wrapper_attrs::{
    prepare_wrapper_members, property_name_value, wrapper_copy_metadata, wrapper_publish_members,
};
pub use {
    wrapper_attrs::molt_wrapper_member_delete, wrapper_attrs::molt_wrapper_member_get,
    wrapper_attrs::molt_wrapper_member_set,
};

pub(crate) use class_lookup::{type_attr_lookup_ptr, type_attr_lookup_ptr_default};
#[cfg(feature = "molt_gpu_primitives")]
pub(crate) use mutation::object_setattr_raw;
#[cfg(test)]
use mutation::type_mutate_attr_name;
pub(crate) use mutation::{
    TypeMutation, explicit_object_mutation_admitted, explicit_type_mutate_attr_name,
    generic_del_attr_name, generic_set_attr_name, mutate_attr_name,
};
pub(crate) use scalar_attrs::{
    is_numeric_scalar_attr_receiver, numeric_publish_members, resolve_scalar_attr,
};
use state::{
    ATTR_LOOKUP_TRACE_LINES, AttrLookupTraceGuard, attr_site_name_cache, trace_attr_lookup_enabled,
};
pub(crate) use state::{
    AttributesRuntimeState, attributes_clear_runtime_state, debug_bound_method_enabled,
};
pub use {
    mutation::molt_del_attr_generic, mutation::molt_del_attr_name, mutation::molt_del_attr_object,
    mutation::molt_del_attr_ptr, mutation::molt_set_attr_generic, mutation::molt_set_attr_name,
    mutation::molt_set_attr_object, mutation::molt_set_attr_object_ic, mutation::molt_set_attr_ptr,
};
pub(crate) use {
    mutation::molt_del_attr_name_attached_body, mutation::molt_set_attr_name_attached_body,
    mutation::molt_set_attr_object_attached_body,
};

fn native_descriptor_metadata_field(
    name: crate::NameId,
) -> Option<crate::builtins::types::NativeDescriptorMetadata> {
    use crate::builtins::types::NativeDescriptorMetadata;
    match name {
        crate::NameId::DunderName => Some(NativeDescriptorMetadata::Name),
        crate::NameId::DunderQualname => Some(NativeDescriptorMetadata::Qualname),
        crate::NameId::DunderObjclass => Some(NativeDescriptorMetadata::Owner),
        crate::NameId::DunderDoc => Some(NativeDescriptorMetadata::Doc),
        _ => None,
    }
}

fn ic_site_from_bits(site_bits: u64) -> Option<u64> {
    let site = obj_from_bits(site_bits);
    if let Some(i) = site.as_int() {
        return u64::try_from(i).ok();
    }
    if site.is_bool() {
        return Some(if site.as_bool().unwrap_or(false) {
            1
        } else {
            0
        });
    }
    if site.is_ptr() || site.is_none() || site.is_pending() {
        return None;
    }
    Some(site_bits)
}

unsafe fn attr_name_bits_for_site(_py: &PyToken<'_>, site_id: u64, slice: &[u8]) -> Option<u64> {
    unsafe {
        let mut cache = attr_site_name_cache(_py)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(bits) = cache.get(&site_id).copied() {
            if let Some(ptr) = obj_from_bits(bits).as_ptr()
                && object_type_id(ptr) == TYPE_ID_STRING
            {
                let cached = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                if cached == slice {
                    profile_hit_unchecked(&ATTR_SITE_NAME_CACHE_HIT_COUNT);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
            }
            dec_ref_bits(_py, bits);
            cache.remove(&site_id);
        }
        profile_hit_unchecked(&ATTR_SITE_NAME_CACHE_MISS_COUNT);
        let bits = dynamic_attr_name_bits(_py, slice)?;
        inc_ref_bits(_py, bits);
        cache.insert(site_id, bits);
        Some(bits)
    }
}

pub(crate) fn molt_code_positions_attached_body(_py: &crate::PyToken<'_>, code_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let code_obj = obj_from_bits(code_bits);
        let Some(code_ptr) = code_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "code.co_positions() requires code",
            );
        };
        unsafe {
            if object_type_id(code_ptr) != TYPE_ID_CODE {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "code.co_positions() requires code",
                );
            }
        }

        let positions = unsafe {
            let table = obj_from_bits(code_linetable_bits(code_ptr));
            if table.is_none() {
                Vec::new()
            } else if let Some(ptr) = table
                .as_ptr()
                .filter(|&ptr| object_type_id(ptr) == TYPE_ID_BYTES)
            {
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
                match crate::builtins::frames::locations::LocationRows::new(
                    bytes,
                    code_firstlineno(code_ptr),
                )
                .collect::<Result<Vec<_>, _>>()
                {
                    Ok(rows) => rows,
                    Err(()) => {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::ValueError,
                            "invalid code location table",
                        );
                    }
                }
            } else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "code location table is not bytes",
                );
            }
        };
        let mut rows = Vec::with_capacity(positions.len());
        for position in positions {
            let bits = |value| {
                if value < 0 {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_int(value).bits()
                }
            };
            let ptr = alloc_tuple(
                _py,
                &[
                    bits(position.line),
                    bits(position.end_line),
                    bits(position.column),
                    bits(position.end_column),
                ],
            );
            if ptr.is_null() {
                for row in rows {
                    dec_ref_bits(_py, row);
                }
                return MoltObject::none().bits();
            }
            rows.push(MoltObject::from_ptr(ptr).bits());
        }
        let table = alloc_tuple(_py, &rows);
        for row in rows {
            dec_ref_bits(_py, row);
        }
        if table.is_null() {
            return MoltObject::none().bits();
        }
        let bits = MoltObject::from_ptr(table).bits();
        let result = molt_iter_attached_body(_py, bits);
        dec_ref_bits(_py, bits);
        result
    })
}

/// Attribute lookup for a `TYPE_ID_FOREIGN` wrapper: route through the wrapped
/// C object's normal or explicit generic protocol via the ABI bridge. Type names
/// use the same type observers as every C API caller, including distinct qualnames.
/// Returns an owned `Found`, a clean `Missing`, or `Error` with the native
/// failure pending. Only the required Python boundary raises for a plain miss.
///
/// # Safety
/// `obj_ptr` must be a live `TYPE_ID_FOREIGN` object.
pub(crate) unsafe fn foreign_attr_lookup(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    access: molt_cpython_abi::hooks::AttributeAccess,
) -> AttributeLookup {
    let c_ptr = unsafe { crate::object::foreign::foreign_ptr_from_obj(obj_ptr) };
    let val = unsafe { molt_cpython_abi::bridge::molt_foreign_getattr(c_ptr, attr_bits, access) };
    match val.decode() {
        molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => AttributeLookup::Found(bits),
        molt_cpython_abi::hooks::DecodedHandleResult::Missing => AttributeLookup::Missing,
        molt_cpython_abi::hooks::DecodedHandleResult::Error => {
            crate::cpython_abi_hooks::propagate_native_failure(_py, "native attribute lookup");
            AttributeLookup::Error
        }
    }
}

pub(crate) unsafe fn attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> AttributeLookup {
    unsafe { attr_lookup_ptr_with_mode(_py, obj_ptr, attr_bits, AttributeLookupMode::Value) }
}

pub(crate) unsafe fn attr_lookup_ptr_with_mode(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    mode: AttributeLookupMode<'_>,
) -> AttributeLookup {
    unsafe {
        let type_id = object_type_id(obj_ptr);
        if type_id == TYPE_ID_TYPE {
            return type_attr_lookup_ptr(_py, obj_ptr, attr_bits);
        }
        if type_id == TYPE_ID_SUPER {
            return crate::builtins::attr::super_attr_lookup(_py, obj_ptr, attr_bits);
        }
        if !matches!(type_id, TYPE_ID_FOREIGN | TYPE_ID_FUNCTION)
            && let Some(class) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
            && object_type_id(class) == TYPE_ID_TYPE
        {
            return class_lookup::attribute_lookup_transaction(
                _py,
                obj_ptr,
                class,
                attr_bits,
                if type_id == TYPE_ID_MODULE {
                    class_lookup::DefaultGetattribute::Module
                } else {
                    class_lookup::DefaultGetattribute::Object
                },
                mode,
                |mode| {
                    if type_id == TYPE_ID_MODULE {
                        module_attr_lookup(_py, obj_ptr, attr_bits)
                    } else {
                        attr_lookup_ptr_default_with_mode(_py, obj_ptr, attr_bits, false, mode)
                    }
                },
            );
        }
        attr_lookup_ptr_default_with_mode(_py, obj_ptr, attr_bits, false, mode)
    }
}

/// Complete builtin/default lookup without user __getattribute__ or
/// __getattr__. Normal lookup wraps this entire operation in one transaction,
/// including the trailing builtin members of dictless receiver kinds.
pub(crate) unsafe fn attr_lookup_ptr_default_with_suppression(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    suppress: bool,
) -> AttributeLookup {
    unsafe {
        attr_lookup_ptr_default_with_mode(
            _py,
            obj_ptr,
            attr_bits,
            suppress,
            AttributeLookupMode::Value,
        )
    }
}

pub(crate) unsafe fn attr_lookup_ptr_default_with_mode(
    py: &PyToken<'_>,
    object: *mut u8,
    name: u64,
    suppress: bool,
    mode: AttributeLookupMode<'_>,
) -> AttributeLookup {
    unsafe {
        AttributeLookup::from_result(attr_lookup_ptr_default_inner(
            py, object, name, suppress, mode,
        ))
    }
}

unsafe fn attr_lookup_ptr_default_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    suppress: bool,
    mode: AttributeLookupMode<'_>,
) -> Result<Option<u64>, ()> {
    unsafe {
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let trace_attr_lookup = trace_attr_lookup_enabled();
        let trace_guard = AttrLookupTraceGuard::new(trace_attr_lookup);
        if trace_attr_lookup {
            let line_no = ATTR_LOOKUP_TRACE_LINES.fetch_add(1, Ordering::Relaxed);
            if line_no < 400 {
                let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                    .unwrap_or_else(|| "<non-str>".to_string());
                eprintln!(
                    "MOLT_TRACE_ATTR_LOOKUP depth={} type={} owner=0x{:x} attr={}",
                    trace_guard.depth(),
                    type_name(_py, obj_from_bits(obj_bits)),
                    obj_bits,
                    attr_name,
                );
            }
        }
        profile_hit(_py, &ATTR_LOOKUP_COUNT);
        let type_id = object_type_id(obj_ptr);
        // Foreign (C-extension) object: route ALL attribute access here — this is
        // the shared lookup every getattr entry point (`molt_get_attr_object`,
        // `_ic`, `_generic`, `_name`, …) funnels through, so foreign dispatch must
        // live here, not in a single entry point.
        if type_id == crate::TYPE_ID_FOREIGN {
            return foreign_attr_lookup(
                _py,
                obj_ptr,
                attr_bits,
                molt_cpython_abi::hooks::AttributeAccess::Normal,
            )
            .into_result();
        }
        if type_id == crate::TYPE_ID_NATIVE_DESCRIPTOR
            && let Some(field) =
                crate::NameId::from_bits(_py, attr_bits).and_then(native_descriptor_metadata_field)
        {
            return crate::builtins::types::native_descriptor_metadata(_py, obj_bits, field)
                .map(Some);
        }
        // A heap numeric payload is not an exact builtin identity. Subtypes
        // enter the shared logical lookup before scalar or synthetic members.
        if matches!(type_id, TYPE_ID_BIGINT | TYPE_ID_FLOAT)
            && !is_numeric_scalar_attr_receiver(_py, obj_bits)
        {
            return crate::builtins::attr::object_attr_lookup_with_mode(
                _py, obj_ptr, attr_bits, suppress, mode,
            )
            .into_result();
        }
        if matches!(type_id, TYPE_ID_BIGINT | TYPE_ID_FLOAT) {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            if let Some(bits) =
                scalar_attrs::resolve_scalar_attr_with_mode(_py, self_bits, attr_bits, mode)
                    .into_result()?
            {
                return Ok(Some(bits));
            }
        }
        if !crate::object::heap_kind_has_class_shape(type_id)
            && !matches!(type_id, TYPE_ID_TYPE | TYPE_ID_EXCEPTION)
        {
            let class_bits = type_of_bits(_py, obj_bits);
            if let Some(class) = obj_from_bits(class_bits).as_ptr()
                && let Some(descriptor) = class_attr_lookup_raw_mro(_py, class, attr_bits)?
                && descriptor_is_data(_py, descriptor)?
            {
                return mode.bind(
                    _py,
                    descriptor,
                    Some(class_bits),
                    Some(obj_bits),
                    DescriptorCallPolicy::Required,
                );
            }
        }
        if type_id == TYPE_ID_MODULE {
            return crate::builtins::attr::module_attr_lookup_default(_py, obj_ptr, attr_bits)
                .into_result();
        }
        if type_id == TYPE_ID_BOUND_METHOD {
            let func_bits = bound_method_func_bits(obj_ptr);
            if let Some(func_ptr) = obj_from_bits(func_bits).as_ptr()
                && let Some(kind) = NativeCallableKind::from_class(_py, object_class_bits(obj_ptr))
            {
                return native_callable_attr(_py, obj_ptr, func_ptr, kind, attr_bits);
            }
            // Method type descriptors precede function attributes (CPython
            // method_getattro). __doc__ alone delegates to the function.
            let name = crate::NameId::from_bits(_py, attr_bits);
            match name {
                Some(crate::NameId::DunderFunc) => {
                    inc_ref_bits(_py, func_bits);
                    return Ok(Some(func_bits));
                }
                Some(crate::NameId::DunderSelf) => {
                    let value = bound_method_self_bits(obj_ptr);
                    inc_ref_bits(_py, value);
                    return Ok(Some(value));
                }
                _ => {}
            }
            if name != Some(crate::NameId::DunderDoc) {
                let class_bits = type_of_bits(_py, obj_bits);
                if let Some(class) = obj_from_bits(class_bits).as_ptr()
                    && let Some(descriptor) = class_attr_lookup_raw_mro(_py, class, attr_bits)?
                {
                    return mode.bind(
                        _py,
                        descriptor,
                        Some(class_bits),
                        Some(obj_bits),
                        DescriptorCallPolicy::Required,
                    );
                }
            }
            // C PyMethod_New accepts a noncallable inline function value too.
            return if let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() {
                attr_lookup_ptr(_py, func_ptr, attr_bits).into_result()
            } else {
                resolve_scalar_attr(_py, func_bits, attr_bits).into_result()
            };
        }
        if type_id == TYPE_ID_EXCEPTION {
            return crate::builtins::attr::object_attr_lookup_with_mode(
                _py, obj_ptr, attr_bits, suppress, mode,
            )
            .into_result();
        }
        if type_id == TYPE_ID_GENERATOR
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            match name {
                crate::NameId::GiRunning => {
                    return Ok(Some(
                        MoltObject::from_bool(generator_running(obj_ptr)).bits(),
                    ));
                }
                crate::NameId::GiSuspended => {
                    return Ok(Some(
                        MoltObject::from_bool(generator_suspended(obj_ptr)).bits(),
                    ));
                }
                crate::NameId::GiCode => {
                    let code_bits = crate::object::aux_header::object_frame_code_bits(obj_ptr);
                    if code_bits != 0 {
                        inc_ref_bits(_py, code_bits);
                        return Ok(Some(code_bits));
                    }
                    return Ok(Some(MoltObject::none().bits()));
                }
                crate::NameId::GiFrame => {
                    if generator_closed(obj_ptr) {
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    let value = suspended_frame_bits(_py, obj_ptr);
                    return if obj_from_bits(value).is_none() {
                        Err(())
                    } else {
                        Ok(Some(value))
                    };
                }
                crate::NameId::GiYieldfrom => {
                    if generator_closed(obj_ptr) {
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    let bits = generator_yieldfrom_bits(obj_ptr);
                    if !obj_from_bits(bits).is_none() {
                        inc_ref_bits(_py, bits);
                    }
                    return Ok(Some(bits));
                }
                _ => {}
            }
            if let Some(func_bits) = generator_method_bits(_py, name)? {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                return mode.bind(
                    _py,
                    func_bits,
                    Some(type_of_bits(_py, self_bits)),
                    Some(self_bits),
                    DescriptorCallPolicy::Required,
                );
            }
        }
        // Fixed-shape builtins also inherit ordinary class attributes even
        // when their semantic type is derived rather than stored in a header.
        if !crate::object::heap_kind_has_class_shape(type_id)
            && !matches!(type_id, TYPE_ID_EXCEPTION | TYPE_ID_FUNCTION | TYPE_ID_TYPE)
        {
            let result = crate::builtins::attr::object_attr_lookup_with_mode(
                _py, obj_ptr, attr_bits, suppress, mode,
            )
            .into_result();
            if let Some(value) = result? {
                return Ok(Some(value));
            }
        }
        if type_id == TYPE_ID_ASYNC_GENERATOR
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            match name {
                crate::NameId::AgRunning => {
                    let gen_bits = asyncgen_gen_bits(obj_ptr);
                    let gen_running = if let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) {
                        object_type_id(gen_ptr) == TYPE_ID_GENERATOR && generator_running(gen_ptr)
                    } else {
                        false
                    };
                    let running = asyncgen_running(obj_ptr) || gen_running;
                    return Ok(Some(MoltObject::from_bool(running).bits()));
                }
                crate::NameId::AgSuspended => {
                    let suspended =
                        maybe_ptr_from_bits(asyncgen_gen_bits(obj_ptr)).is_some_and(|gen_ptr| {
                            object_type_id(gen_ptr) == TYPE_ID_GENERATOR
                                && generator_suspended(gen_ptr)
                        });
                    return Ok(Some(MoltObject::from_bool(suspended).bits()));
                }
                crate::NameId::AgAwait => {
                    let await_bits = asyncgen_await_bits(_py, obj_ptr);
                    return Ok(Some(await_bits));
                }
                crate::NameId::AgCode => {
                    let code_bits = asyncgen_code_bits(_py, obj_ptr);
                    return Ok(Some(code_bits));
                }
                crate::NameId::AgFrame => {
                    let gen_bits = asyncgen_gen_bits(obj_ptr);
                    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
                        return Ok(Some(MoltObject::none().bits()));
                    };
                    if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    if generator_closed(gen_ptr) {
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    let value = suspended_frame_bits(_py, gen_ptr);
                    return if obj_from_bits(value).is_none() {
                        Err(())
                    } else {
                        Ok(Some(value))
                    };
                }
                _ => {}
            }
            if let Some(func_bits) = asyncgen_method_bits(_py, name)? {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                return mode.bind(
                    _py,
                    func_bits,
                    Some(type_of_bits(_py, self_bits)),
                    Some(self_bits),
                    DescriptorCallPolicy::Required,
                );
            }
        }
        if type_id == TYPE_ID_MEMORYVIEW
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            match name {
                crate::NameId::Obj => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    if let Some(lease) = (*crate::memoryview_ptr(obj_ptr)).native_lease.as_ref() {
                        let owner = lease.owner();
                        if owner.is_null() {
                            return Err(());
                        }
                        let result =
                            molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(owner);
                        if result.is_none() {
                            crate::cpython_abi_hooks::propagate_native_failure(
                                _py,
                                "memoryview exporter identity",
                            );
                        }
                        return result.map(Some).ok_or(());
                    }
                    let base = crate::memoryview_base_bits(obj_ptr);
                    if base == 0 {
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    inc_ref_bits(_py, base);
                    return Ok(Some(base));
                }
                crate::NameId::Format => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    let bits = memoryview_format_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::Itemsize => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    return Ok(Some(
                        MoltObject::from_int(memoryview_itemsize(obj_ptr) as i64).bits(),
                    ));
                }
                crate::NameId::Ndim => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    return Ok(Some(
                        MoltObject::from_int(memoryview_ndim(obj_ptr) as i64).bits(),
                    ));
                }
                crate::NameId::Shape => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    let shape = memoryview_shape(obj_ptr).unwrap_or(&[]);
                    let value = tuple_from_isize_slice(_py, shape);
                    return if obj_from_bits(value).is_none() {
                        Err(())
                    } else {
                        Ok(Some(value))
                    };
                }
                crate::NameId::Strides => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    let strides = memoryview_strides(obj_ptr).unwrap_or(&[]);
                    let value = tuple_from_isize_slice(_py, strides);
                    return if obj_from_bits(value).is_none() {
                        Err(())
                    } else {
                        Ok(Some(value))
                    };
                }
                crate::NameId::Readonly => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    return Ok(Some(
                        MoltObject::from_bool(memoryview_readonly(obj_ptr)).bits(),
                    ));
                }
                crate::NameId::Nbytes => {
                    if memoryview_released(obj_ptr) {
                        raise_released_memoryview::<()>(_py);
                        return Err(());
                    }
                    return Ok(Some(
                        MoltObject::from_int(memoryview_nbytes(obj_ptr) as i64).bits(),
                    ));
                }
                _ => {}
            }
            if let Some(func_bits) = memoryview_method_bits(_py, name)? {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                return mode.bind(
                    _py,
                    func_bits,
                    Some(type_of_bits(_py, self_bits)),
                    Some(self_bits),
                    DescriptorCallPolicy::Required,
                );
            }
        }
        if type_id == TYPE_ID_RANGE
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            match name {
                crate::NameId::Start => {
                    let bits = range_start_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::Stop => {
                    let bits = range_stop_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::Step => {
                    let bits = range_step_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                _ => {}
            }
            if let Some(func_bits) = range_method_bits(_py, name)? {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                return mode.bind(
                    _py,
                    func_bits,
                    Some(type_of_bits(_py, self_bits)),
                    Some(self_bits),
                    DescriptorCallPolicy::Required,
                );
            }
        }
        if type_id == TYPE_ID_SLICE
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            match name {
                crate::NameId::Start => {
                    let bits = slice_start_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::Stop => {
                    let bits = slice_stop_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::Step => {
                    let bits = slice_step_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                _ => {}
            }
            if let Some(func_bits) = slice_method_bits(_py, name)? {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                return mode.bind(
                    _py,
                    func_bits,
                    Some(type_of_bits(_py, self_bits)),
                    Some(self_bits),
                    DescriptorCallPolicy::Required,
                );
            }
        }
        if type_id == TYPE_ID_GENERIC_ALIAS
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            match name {
                crate::NameId::DunderOrigin => {
                    let bits = generic_alias_origin_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::DunderArgs => {
                    let bits = generic_alias_args_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::DunderParameters => {
                    return generic_parameters::alias_parameters(_py, obj_ptr).map(Some);
                }
                crate::NameId::DunderUnpacked => {
                    return Ok(Some(MoltObject::from_bool(false).bits()));
                }
                _ => {}
            }
        }
        if type_id == TYPE_ID_UNION
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            match name {
                crate::NameId::DunderOrigin => {
                    let bits = builtin_classes(_py).union_type;
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::DunderArgs => {
                    let bits = union_type_args_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Ok(Some(bits));
                }
                crate::NameId::DunderParameters => {
                    return generic_parameters::alias_parameters(_py, obj_ptr).map(Some);
                }
                _ => {}
            }
        }
        if type_id == TYPE_ID_FILE_HANDLE
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
        {
            let handle_ptr = file_handle_ptr(obj_ptr);
            if handle_ptr.is_null() {
                return Ok(None);
            }
            let handle = &*handle_ptr;
            if handle.text && (name == crate::NameId::Readinto || name == crate::NameId::Readinto1)
            {
                return Ok(None);
            }
            if !handle.text && name == crate::NameId::Reconfigure {
                return Ok(None);
            }
            if let Some(func_bits) = file_method_bits(_py, type_of_bits(_py, obj_bits), name)? {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                return mode.bind(
                    _py,
                    func_bits,
                    Some(type_of_bits(_py, self_bits)),
                    Some(self_bits),
                    DescriptorCallPolicy::Required,
                );
            }
        }
        if type_id == TYPE_ID_SET
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(func_bits) = set_method_bits(_py, name)?
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            return mode.bind(
                _py,
                func_bits,
                Some(type_of_bits(_py, self_bits)),
                Some(self_bits),
                DescriptorCallPolicy::Required,
            );
        }
        if type_id == TYPE_ID_FROZENSET
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(func_bits) = frozenset_method_bits(_py, name)?
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            return mode.bind(
                _py,
                func_bits,
                Some(type_of_bits(_py, self_bits)),
                Some(self_bits),
                DescriptorCallPolicy::Required,
            );
        }
        if type_id == TYPE_ID_LIST
            && crate::object::iterable::builtin_receiver(_py, obj_ptr)
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(descriptor) = list_method_bits(_py, name)?
        {
            return descriptor_bind(
                _py,
                descriptor,
                Some(builtin_classes(_py).list),
                Some(MoltObject::from_ptr(obj_ptr).bits()),
            );
        }
        if type_id == TYPE_ID_TUPLE
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(func_bits) = tuple_method_bits(_py, name)?
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            return mode.bind(
                _py,
                func_bits,
                Some(type_of_bits(_py, self_bits)),
                Some(self_bits),
                DescriptorCallPolicy::Required,
            );
        }
        if type_id == TYPE_ID_STRING
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(func_bits) = string_method_bits(_py, name)?
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            return mode.bind(
                _py,
                func_bits,
                Some(type_of_bits(_py, self_bits)),
                Some(self_bits),
                DescriptorCallPolicy::Required,
            );
        }
        if type_id == TYPE_ID_BYTES
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(func_bits) = bytes_method_bits(_py, name)?
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            return mode.bind(
                _py,
                func_bits,
                Some(type_of_bits(_py, self_bits)),
                Some(self_bits),
                DescriptorCallPolicy::Required,
            );
        }
        if type_id == TYPE_ID_BYTEARRAY
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(func_bits) = bytearray_method_bits(_py, name)?
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            return mode.bind(
                _py,
                func_bits,
                Some(type_of_bits(_py, self_bits)),
                Some(self_bits),
                DescriptorCallPolicy::Required,
            );
        }
        if type_id == TYPE_ID_COMPLEX
            && let Some(name) = crate::NameId::from_bits(_py, attr_bits)
            && let Some(func_bits) = complex_method_bits(_py, name)?
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            return mode.bind(
                _py,
                func_bits,
                Some(type_of_bits(_py, self_bits)),
                Some(self_bits),
                DescriptorCallPolicy::Required,
            );
        }
        if type_id == TYPE_ID_TYPE {
            return type_attr_lookup_ptr_default(_py, obj_ptr, attr_bits).into_result();
        }
        if type_id == TYPE_ID_FUNCTION {
            if let Some(kind) = NativeCallableKind::from_class(_py, object_class_bits(obj_ptr)) {
                return native_callable_attr(_py, obj_ptr, obj_ptr, kind, attr_bits);
            }
            if let Some(name) = crate::NameId::from_bits(_py, attr_bits) {
                if name == crate::NameId::DunderGet {
                    // Managed functions implement the descriptor protocol.
                    let none = MoltObject::none().bits();
                    let func_bits = crate::builtins::methods::builtin_func_bits_with_defaults_tuple(
                        _py,
                        NativeCallableSpec::function(
                            &runtime_state(_py).method_cache.function_descriptor_get,
                        ),
                        fn_addr!(molt_function_descriptor_get),
                        3, // (self, instance, owner)
                        &[none],
                    );
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    if func_bits == 0 {
                        return Err(());
                    }
                    let bound = molt_bound_method_new_attached_body(_py, func_bits, self_bits);
                    return if obj_from_bits(bound).is_none() {
                        Err(())
                    } else {
                        Ok(Some(bound))
                    };
                }
                if name == crate::NameId::DunderCode {
                    // CPython parity: builtin_function_or_method objects do not expose __code__.
                    if builtin_classes(_py).is_builtin_callable_class(object_class_bits(obj_ptr)) {
                        return Ok(None);
                    }
                    let code_bits = ensure_function_code_bits(_py, obj_ptr);
                    if !obj_from_bits(code_bits).is_none() {
                        inc_ref_bits(_py, code_bits);
                        return Ok(Some(code_bits));
                    }
                    return Ok(None);
                }
                if name == crate::NameId::DunderClosure {
                    if builtin_classes(_py).is_builtin_callable_class(object_class_bits(obj_ptr)) {
                        return Ok(None);
                    }
                    if function_call_abi(obj_ptr) == FunctionCallAbi::OpaqueContextFirst {
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    let closure_bits = function_closure_bits(obj_ptr);
                    if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
                        inc_ref_bits(_py, closure_bits);
                        return Ok(Some(closure_bits));
                    }
                    return Ok(Some(MoltObject::none().bits()));
                }
            }
            let annotate_name_bits = crate::NameId::DunderAnnotate.bits(_py);
            if crate::object::ops_compare::string_storage_equal(attr_bits, annotate_name_bits)
                && pep649_enabled(_py)
            {
                let mut annotate_bits = function_annotate_bits(obj_ptr);
                if annotate_bits == 0 {
                    annotate_bits = MoltObject::none().bits();
                }
                inc_ref_bits(_py, annotate_bits);
                return Ok(Some(annotate_bits));
            }
            let annotations_bits = crate::NameId::DunderAnnotations.bits(_py);
            if crate::object::ops_compare::string_storage_equal(attr_bits, annotations_bits) {
                let cached = function_annotations_bits(obj_ptr);
                if cached != 0 {
                    inc_ref_bits(_py, cached);
                    return Ok(Some(cached));
                }
                let annotate_bits = function_annotate_bits(obj_ptr);
                let res_bits = if pep649_enabled(_py)
                    && annotate_bits != 0
                    && !obj_from_bits(annotate_bits).is_none()
                {
                    let format_bits = MoltObject::from_int(1).bits();
                    let res_bits = call_callable1(_py, annotate_bits, format_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, res_bits);
                        return Err(());
                    }
                    let res_obj = obj_from_bits(res_bits);
                    let Some(res_ptr) = res_obj.as_ptr() else {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        raise_exception::<()>(_py, BuiltinExc::TypeError, &msg);
                        return Err(());
                    };
                    if object_type_id(res_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        raise_exception::<()>(_py, BuiltinExc::TypeError, &msg);
                        return Err(());
                    }
                    res_bits
                } else {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return Ok(None);
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                };
                function_set_annotations_bits(_py, obj_ptr, res_bits);
                return Ok(Some(res_bits));
            }
            // The shared resolver gives real class data descriptors priority
            // over the user dictionary, then binds ordinary class attributes.
            return crate::builtins::attr::object_attr_lookup_with_mode(
                _py, obj_ptr, attr_bits, suppress, mode,
            )
            .into_result();
        }
        if type_id == TYPE_ID_CODE {
            // Keep basic CPython-compatible code metadata available for inspect/types consumers.
            let Some(name) = crate::NameId::from_bits(_py, attr_bits) else {
                return Ok(None);
            };
            if let Some(spec) = crate::object::layout::CODE_REPLACE_FIELDS
                .iter()
                .find(|spec| spec.name == name)
            {
                return spec.field.read(_py, obj_ptr).map(Some);
            }
            if name == crate::NameId::CoPositions {
                let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::molt_code_positions as *const () as usize as u64,
                    1,
                );
                if func_ptr.is_null() {
                    return Err(());
                }
                let func_bits = MoltObject::from_ptr(func_ptr).bits();
                let _ = crate::molt_function_set_builtin_attached_body(_py, func_bits);
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
                dec_ref_bits(_py, func_bits);
                if bound_ptr.is_null() {
                    return Err(());
                }
                return Ok(Some(MoltObject::from_ptr(bound_ptr).bits()));
            }
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            if let Some(method) = crate::builtins::methods::builtin_class_method_bits(
                _py,
                builtin_classes(_py).code,
                name,
            )? {
                return mode.bind(
                    _py,
                    method,
                    Some(builtin_classes(_py).code),
                    Some(self_bits),
                    DescriptorCallPolicy::Required,
                );
            }
            return Ok(None);
        }

        if crate::object::heap_kind_has_class_shape(type_id) {
            return crate::builtins::attr::object_attr_lookup_with_mode(
                _py, obj_ptr, attr_bits, suppress, mode,
            )
            .into_result();
        }
        Ok(None)
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
pub(crate) unsafe fn molt_get_attr_generic_attached_body(
    _py: &crate::PyToken<'_>,
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "attribute name is too large",
                );
            };
            if obj_ptr.is_null() {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::AttributeError,
                    "object has no attribute",
                );
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let Some(attr_bits) = dynamic_attr_name_bits(_py, slice) else {
                return MoltObject::none().bits();
            };
            let found = attr_lookup_ptr(_py, obj_ptr, attr_bits);
            if let AttributeLookup::Missing = found {
                raise_attribute_miss(_py, obj_ptr, attr_bits, attr_name);
            }
            dec_ref_bits(_py, attr_bits);
            match found {
                AttributeLookup::Found(value) => value,
                AttributeLookup::Missing | AttributeLookup::Error => MoltObject::none().bits(),
            }
        })
    }
}

/// The AttributeError a required lookup raises once every protocol step has
/// missed. As in CPython's `PyObject_GetAttr`, it records `name` and `obj` for
/// every receiver; a module reports its own message family.
pub(crate) unsafe fn raise_attribute_miss(
    py: &PyToken<'_>,
    obj_ptr: *mut u8,
    name_bits: u64,
    attr_name: &str,
) {
    unsafe {
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        match object_type_id(obj_ptr) {
            TYPE_ID_MODULE => {
                crate::builtins::modules::raise_module_attribute_miss(py, obj_ptr, name_bits);
            }
            TYPE_ID_TYPE => {
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let message = format!("type object '{class_name}' has no attribute '{attr_name}'");
                attr_error_with_obj_message(py, &message, attr_name, obj_bits);
            }

            _ => {
                attr_error_with_obj(
                    py,
                    type_name(py, MoltObject::from_ptr(obj_ptr)),
                    attr_name,
                    obj_bits,
                );
            }
        }
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
pub(crate) unsafe fn molt_get_attr_ptr_attached_body(
    _py: &crate::PyToken<'_>,
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            molt_get_attr_generic_attached_body(_py, obj_ptr, attr_name_ptr, attr_name_len_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
pub(crate) unsafe fn molt_get_attr_object_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "attribute name is too large",
                );
            };
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
                return molt_get_attr_generic_attached_body(
                    _py,
                    ptr,
                    attr_name_ptr,
                    attr_name_len_bits,
                );
            }
            let Some(name_bits) = dynamic_attr_name_bits(_py, slice) else {
                return MoltObject::none().bits();
            };
            let resolved = resolve_scalar_attr(_py, obj_bits, name_bits).into_result();
            dec_ref_bits(_py, name_bits);
            match resolved {
                Ok(Some(value)) => return value,
                Err(()) => return MoltObject::none().bits(),
                Ok(None) => {}
            }
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
pub(crate) unsafe fn molt_get_attr_object_ic_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    site_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            if ic_site_from_bits(site_bits).is_none() {
                return molt_get_attr_object_attached_body(
                    _py,
                    obj_bits,
                    attr_name_ptr,
                    attr_name_len_bits,
                );
            }
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "attribute name is too large",
                );
            };
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            get_attr_object_ic_with_mode(
                _py,
                obj_bits,
                slice,
                site_bits,
                AttributeLookupMode::Value,
            )
            .unwrap_or_else(|()| MoltObject::none().bits())
        })
    }
}

/// Share the field and name caches between value and method loads. A cached
/// instance field is already the actual attribute value, so it never publishes
/// a method receiver. Only the generic descriptor transaction can do that.
pub(crate) unsafe fn get_attr_object_ic_with_mode(
    py: &PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    site_bits: u64,
    mode: AttributeLookupMode<'_>,
) -> Result<u64, ()> {
    unsafe {
        let site_id = ic_site_from_bits(site_bits);
        if let Some(site_id) = site_id
            && let Some(bits) = field_sites::read_cached_field(py, site_id, obj_bits)
        {
            if exception_pending(py) {
                dec_ref_bits(py, bits);
                return Err(());
            }
            return Ok(bits);
        }
        let name_bits = match site_id {
            Some(site_id) => attr_name_bits_for_site(py, site_id, name),
            None => dynamic_attr_name_bits(py, name),
        }
        .ok_or(())?;
        if let Some(site_id) = site_id
            && field_sites::remember_field_site(
                py,
                field_sites::FieldAccess::Read,
                site_id,
                obj_bits,
                name_bits,
            )
            .is_err()
        {
            dec_ref_bits(py, name_bits);
            return Err(());
        }
        let out = get_attr_name_with_mode(py, obj_bits, name_bits, mode);
        dec_ref_bits(py, name_bits);
        out
    }
}

/// # Safety
/// Dereferences `attr_name_ptr`. Caller must ensure it points to valid UTF-8
/// of length `attr_name_len_bits`.
pub(crate) unsafe fn molt_get_attr_special_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "attribute name is too large",
                );
            };
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
                return attr_error_with_obj(_py, type_name(_py, obj), attr_name, obj_bits);
            };
            let name_ptr = alloc_string(_py, slice);
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let class_bits = object_class_bits(obj_ptr);
            let class_ptr = obj_from_bits(class_bits).as_ptr();
            let res = if let Some(class_ptr) = class_ptr {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), name_bits)
                        .into_result()
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            };
            dec_ref_bits(_py, name_bits);
            match res {
                Ok(Some(value)) => return value,
                Err(()) => return MoltObject::none().bits(),
                Ok(None) => {}
            }
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

/// Native methods delegate only their public metadata to their implementation;
/// private binder dictionaries and annotation slots never enter public lookup.
unsafe fn native_callable_attr(
    _py: &PyToken<'_>,
    public_ptr: *mut u8,
    _obj_ptr: *mut u8,
    kind: NativeCallableKind,
    attr_bits: u64,
) -> Result<Option<u64>, ()> {
    unsafe {
        if let Some(field) = crate::NameId::from_bits(_py, attr_bits)
            .and_then(crate::builtins::functions::native_callable::CallableMetadata::from_id)
            && !kind.metadata_fields().contains(&field)
        {
            // The class's own metadata must not widen this callable role.
            return Ok(None);
        }
        let class = type_of_bits(_py, MoltObject::from_ptr(public_ptr).bits());
        let Some(class_ptr) = obj_from_bits(class).as_ptr() else {
            return Ok(None);
        };
        let Some(value) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)? else {
            return Ok(None);
        };
        descriptor_bind(
            _py,
            value,
            Some(class),
            Some(MoltObject::from_ptr(public_ptr).bits()),
        )
    }
}

/// Descriptor binding shares receiver admission with implicit attribute access.
pub(crate) fn molt_function_descriptor_get_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    instance_bits: u64,
    owner_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(pointer) = obj_from_bits(self_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "descriptor requires a function",
            );
        };
        unsafe {
            if object_type_id(pointer) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "descriptor requires a function",
                );
            }
            let owner = (!obj_from_bits(owner_bits).is_none()).then_some(owner_bits);
            let instance = (!obj_from_bits(instance_bits).is_none()).then_some(instance_bits);
            if let Some(kind) = NativeCallableKind::from_class(_py, object_class_bits(pointer))
                && !kind.is_descriptor()
            {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "native function is not a descriptor",
                );
            }
            match descriptor_bind(_py, self_bits, owner, instance) {
                Ok(Some(value)) => value,
                Ok(None) | Err(()) => MoltObject::none().bits(),
            }
        }
    })
}

pub(crate) fn molt_get_attr_name_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        get_attr_name(py, obj_bits, name_bits).unwrap_or_else(|()| MoltObject::none().bits())
    })
}

/// Required named lookup. Missing and failed internal operations remain distinct
/// until this boundary selects the public AttributeError for an actual miss.
pub(crate) fn get_attr_name(_py: &PyToken<'_>, obj_bits: u64, name_bits: u64) -> Result<u64, ()> {
    get_attr_name_with_mode(_py, obj_bits, name_bits, AttributeLookupMode::Value)
}

pub(crate) fn get_attr_name_with_mode(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
    mode: AttributeLookupMode<'_>,
) -> Result<u64, ()> {
    match lookup_attribute_with_mode(_py, obj_bits, name_bits, mode) {
        AttributeLookup::Found(value) => Ok(value),
        AttributeLookup::Error => Err(()),
        AttributeLookup::Missing => unsafe {
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                raise_attribute_miss(_py, obj_ptr, name_bits, &attr_name);
            } else {
                attr_error_with_obj(
                    _py,
                    type_name(_py, obj_from_bits(obj_bits)),
                    &attr_name,
                    obj_bits,
                );
            }
            Err(())
        },
    }
}

pub(crate) fn lookup_attribute_with_mode(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
    mode: AttributeLookupMode<'_>,
) -> AttributeLookup {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        let _ = raise_attr_name_type_error(_py, name_bits);
        return AttributeLookup::Error;
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            let _ = raise_attr_name_type_error(_py, name_bits);
            return AttributeLookup::Error;
        }
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            // Foreign (C-extension) objects are handled inside the shared
            // `attr_lookup_ptr` (below) so every getattr entry point routes
            // them uniformly — do NOT special-case here.
            return attr_lookup_ptr_with_mode(_py, obj_ptr, name_bits, mode);
        }
        scalar_attrs::resolve_scalar_attr_with_mode(_py, obj_bits, name_bits, mode)
    }
}

pub(crate) fn molt_get_attr_name_default_attached_body(
    _py: &crate::PyToken<'_>,
    object: u64,
    name: u64,
    default: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if is_missing_bits(py, default) {
            return molt_get_attr_name_attached_body(py, object, name);
        }
        match lookup_attribute(py, object, name) {
            AttributeLookup::Found(value) => value,
            AttributeLookup::Missing => {
                inc_ref_bits(py, default);
                default
            }
            AttributeLookup::Error => MoltObject::none().bits(),
        }
    })
}

pub(crate) fn molt_has_attr_name_attached_body(
    _py: &crate::PyToken<'_>,
    object: u64,
    name: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        match lookup_attribute(py, object, name) {
            AttributeLookup::Found(value) => {
                dec_ref_bits(py, value);
                MoltObject::from_bool(true).bits()
            }
            AttributeLookup::Missing | AttributeLookup::Error => {
                MoltObject::from_bool(false).bits()
            }
        }
    })
}

include!("../attached_exports_generated/builtins/attributes.rs");

#[cfg(test)]
mod tests {
    use super::{attr_name_bits_for_site, attributes_clear_runtime_state};
    use crate::{
        MoltObject, PyToken, alloc_dict_with_pairs, alloc_string, dec_ref_bits, inc_ref_bits,
        obj_from_bits, runtime_state,
    };
    use num_bigint::BigInt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CUSTOM_GETATTRIBUTE_CALLS: AtomicU64 = AtomicU64::new(0);

    extern "C" fn visibility_probe(__context: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        MoltObject::none().bits()
    }

    fn string_bits(_py: &PyToken<'_>, label: &[u8]) -> u64 {
        let ptr = alloc_string(_py, label);
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn refcount(bits: u64) -> u32 {
        let ptr = MoltObject::from_bits(bits).as_ptr().unwrap();
        unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    fn runtime_function_bits(
        _py: &PyToken<'_>,
        name: &'static str,
        target: *const (),
        arity: u64,
    ) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(name, target),
            arity,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn test_class_bits(_py: &PyToken<'_>, name: &[u8], attrs: &[(&[u8], u64)]) -> u64 {
        let builtins = crate::builtin_classes(_py);
        let name_bits = string_bits(_py, name);
        let namespace_bits = crate::molt_dict_new_attached_body(_py, attrs.len() as u64);
        assert!(!obj_from_bits(namespace_bits).is_none());
        for &(attr_name, value_bits) in attrs {
            let attr_bits = string_bits(_py, attr_name);
            assert_eq!(
                crate::c_api::molt_mapping_setitem_attached_body(
                    _py,
                    namespace_bits,
                    attr_bits,
                    value_bits
                ),
                0
            );
            dec_ref_bits(_py, attr_bits);
        }
        let class_bits = crate::builtins::types::molt_type_new_attached_body(
            _py,
            builtins.type_obj,
            name_bits,
            MoltObject::none().bits(),
            namespace_bits,
            MoltObject::none().bits(),
        );
        assert!(!obj_from_bits(class_bits).is_none());
        assert!(!crate::exception_pending(_py));
        dec_ref_bits(_py, namespace_bits);
        dec_ref_bits(_py, name_bits);
        class_bits
    }

    unsafe fn test_instance_bits(_py: &PyToken<'_>, class_bits: u64) -> u64 {
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("test class");
        let instance_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        assert!(!obj_from_bits(instance_bits).is_none());
        instance_bits
    }

    extern "C" fn changing_getattribute(__context: u64, _self_bits: u64, _name_bits: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        let call = CUSTOM_GETATTRIBUTE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        MoltObject::from_int(call as i64).bits()
    }

    #[test]
    fn class_attribute_errors_return_boxed_none_across_entrypoints() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let declared_name = string_bits(_py, b"BoxedAttributeErrors");
            let class_bits = crate::molt_class_new_attached_body(_py, declared_name);
            let class_ptr = MoltObject::from_bits(class_bits).as_ptr().unwrap();
            let invalid_name = MoltObject::from_int(17).bits();
            for (operation, name, expected_error, routes) in [
                (
                    "get",
                    "__missing_boxed_attribute__",
                    crate::BuiltinExc::AttributeError,
                    8,
                ),
                ("set", "__name__", crate::BuiltinExc::TypeError, 5),
                (
                    "del",
                    "__missing_boxed_attribute__",
                    crate::BuiltinExc::AttributeError,
                    4,
                ),
            ] {
                let name_bits = string_bits(_py, name.as_bytes());
                for route in 0..routes {
                    assert!(!crate::exception_pending(_py));
                    let result: u64 = unsafe {
                        match (operation, route) {
                            ("get", 0) => {
                                super::molt_get_attr_name_attached_body(_py, class_bits, name_bits)
                            }
                            ("get", 1) => super::molt_get_attr_generic_attached_body(
                                _py,
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 2) => super::molt_get_attr_ptr_attached_body(
                                _py,
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 3) => super::molt_get_attr_object_attached_body(
                                _py,
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 4) => super::molt_get_attr_object_ic_attached_body(
                                _py,
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                                MoltObject::from_int(91).bits(),
                            ),
                            ("get", 5) => super::molt_get_attr_special_attached_body(
                                _py,
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 6) => crate::molt_object_getattribute_attached_body(
                                _py, class_bits, name_bits,
                            ),
                            ("get", 7) => crate::molt_type_getattribute_attached_body(
                                _py, class_bits, name_bits,
                            ),
                            ("set", 0) => super::molt_set_attr_name_attached_body(
                                _py,
                                class_bits,
                                name_bits,
                                invalid_name,
                            ),
                            ("set", 1) => {
                                crate::builtins::attributes::molt_set_attr_generic_attached_body(
                                    _py,
                                    class_ptr,
                                    name.as_ptr(),
                                    name.len() as u64,
                                    invalid_name,
                                )
                            }
                            ("set", 2) => {
                                crate::builtins::attributes::molt_set_attr_ptr_attached_body(
                                    _py,
                                    class_ptr,
                                    name.as_ptr(),
                                    name.len() as u64,
                                    invalid_name,
                                )
                            }
                            ("set", 3) => super::molt_set_attr_object_attached_body(
                                _py,
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                                invalid_name,
                            ),
                            ("set", 4) => {
                                assert!(
                                    super::type_mutate_attr_name(
                                        class_bits,
                                        name_bits,
                                        Some(invalid_name)
                                    )
                                    .is_err()
                                );
                                MoltObject::none().bits()
                            }
                            ("del", 0) => {
                                super::molt_del_attr_name_attached_body(_py, class_bits, name_bits)
                            }
                            ("del", 1) => {
                                crate::builtins::attributes::molt_del_attr_generic_attached_body(
                                    _py,
                                    class_ptr,
                                    name.as_ptr(),
                                    name.len() as u64,
                                )
                            }
                            ("del", 2) => {
                                crate::builtins::attributes::molt_del_attr_ptr_attached_body(
                                    _py,
                                    class_ptr,
                                    name.as_ptr(),
                                    name.len() as u64,
                                )
                            }
                            ("del", 3) => {
                                crate::builtins::attributes::molt_del_attr_object_attached_body(
                                    _py,
                                    class_bits,
                                    name.as_ptr(),
                                    name.len() as u64,
                                )
                            }
                            _ => unreachable!(),
                        }
                    };
                    assert_eq!(
                        result,
                        MoltObject::none().bits(),
                        "{operation} route {route}"
                    );
                    assert!(crate::exception_pending(_py), "{operation} route {route}");
                    let exception = crate::exception_last_bits_noinc(_py).unwrap();
                    assert!(
                        crate::builtins::exceptions::exception_matches_kind(
                            _py,
                            exception,
                            expected_error,
                        ),
                        "{operation} route {route}"
                    );
                    crate::clear_exception(_py);
                }
                dec_ref_bits(_py, name_bits);
            }
            let name_key = string_bits(_py, b"__name__");
            let actual_name = super::molt_get_attr_name_attached_body(_py, class_bits, name_key);
            assert_eq!(
                crate::string_obj_to_owned(MoltObject::from_bits(actual_name)).as_deref(),
                Some("BoxedAttributeErrors")
            );
            assert!(!crate::exception_pending(_py));
            dec_ref_bits(_py, actual_name);
            dec_ref_bits(_py, name_key);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, declared_name);
        });
    }

    #[test]
    #[cfg(feature = "molt_gpu_primitives")]
    fn gpu_attribute_bridge_errors_return_failure_status() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name_bits = string_bits(_py, b"attribute");
            let receiver = alloc_string(_py, b"receiver");
            assert!(!receiver.is_null());
            for invalid_receiver in [true, false] {
                let result = crate::gpu_bridge::__molt_gpu_object_setattr_raw(
                    _py.attached_context(),
                    if invalid_receiver {
                        std::ptr::null_mut()
                    } else {
                        receiver
                    },
                    name_bits,
                    if invalid_receiver {
                        b"attribute".as_ptr()
                    } else {
                        b"\xff".as_ptr()
                    },
                    if invalid_receiver { 9 } else { 1 },
                    MoltObject::none().bits(),
                );
                assert_eq!(result, -1);
                assert!(crate::exception_pending(_py));
                let exception = crate::exception_last_bits_noinc(_py).unwrap();
                assert!(crate::builtins::exceptions::exception_matches_kind(
                    _py,
                    exception,
                    crate::BuiltinExc::TypeError,
                ));
                crate::clear_exception(_py);
            }
            dec_ref_bits(_py, MoltObject::from_ptr(receiver).bits());
            dec_ref_bits(_py, name_bits);
        });
    }

    #[test]
    fn builtin_code_visibility_is_identical_for_name_and_generic_entrypoints() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let builtin = crate::builtins::methods::alloc_builtin_function(
                _py,
                visibility_probe as *const () as usize as u64,
                0,
            );
            assert_ne!(builtin, 0);
            let builtin_ptr = MoltObject::from_bits(builtin).as_ptr().unwrap();
            let code_name = string_bits(_py, b"__code__");

            let by_name = super::molt_get_attr_name_attached_body(_py, builtin, code_name);
            assert!(MoltObject::from_bits(by_name).is_none());
            assert!(crate::exception_pending(_py));
            crate::clear_exception(_py);

            let by_generic = unsafe {
                super::molt_get_attr_generic_attached_body(
                    _py,
                    builtin_ptr,
                    b"__code__".as_ptr(),
                    8,
                )
            };
            assert!(MoltObject::from_bits(by_generic).is_none());
            assert!(crate::exception_pending(_py));
            crate::clear_exception(_py);

            let closure_name = string_bits(_py, b"__closure__");
            let closure_by_name =
                super::molt_get_attr_name_attached_body(_py, builtin, closure_name);
            assert!(MoltObject::from_bits(closure_by_name).is_none());
            assert!(crate::exception_pending(_py));
            crate::clear_exception(_py);

            let closure_by_generic = unsafe {
                super::molt_get_attr_generic_attached_body(
                    _py,
                    builtin_ptr,
                    b"__closure__".as_ptr(),
                    11,
                )
            };
            assert!(MoltObject::from_bits(closure_by_generic).is_none());
            assert!(crate::exception_pending(_py));
            crate::clear_exception(_py);

            let opaque_ptr =
                crate::alloc_function_obj(_py, visibility_probe as *const () as usize as u64, 0);
            let opaque_context = crate::alloc_tuple(_py, &[MoltObject::from_int(5).bits()]);
            assert!(!opaque_ptr.is_null() && !opaque_context.is_null());
            let opaque = MoltObject::from_ptr(opaque_ptr).bits();
            let opaque_context_bits = MoltObject::from_ptr(opaque_context).bits();
            unsafe {
                crate::function_set_closure_bits(
                    _py,
                    opaque_ptr,
                    opaque_context_bits,
                    crate::FunctionCallAbi::OpaqueContextFirst,
                );
            }
            let opaque_by_name = super::molt_get_attr_name_attached_body(_py, opaque, closure_name);
            assert!(MoltObject::from_bits(opaque_by_name).is_none());
            assert!(!crate::exception_pending(_py));
            let opaque_by_generic = unsafe {
                super::molt_get_attr_generic_attached_body(
                    _py,
                    opaque_ptr,
                    b"__closure__".as_ptr(),
                    11,
                )
            };
            assert!(MoltObject::from_bits(opaque_by_generic).is_none());
            assert!(!crate::exception_pending(_py));

            let ordinary_ptr =
                crate::alloc_function_obj(_py, visibility_probe as *const () as usize as u64, 0);
            assert!(!ordinary_ptr.is_null());
            let ordinary = MoltObject::from_ptr(ordinary_ptr).bits();
            let ordinary_code = super::molt_get_attr_name_attached_body(_py, ordinary, code_name);
            assert!(!MoltObject::from_bits(ordinary_code).is_none());
            assert!(!crate::exception_pending(_py));

            dec_ref_bits(_py, ordinary_code);
            dec_ref_bits(_py, ordinary);
            dec_ref_bits(_py, opaque_context_bits);
            dec_ref_bits(_py, opaque);
            dec_ref_bits(_py, closure_name);
            dec_ref_bits(_py, code_name);
            dec_ref_bits(_py, builtin);
        });
    }

    #[test]
    fn function_type_builtins_member_preserves_capture_and_returns_owned_reference() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let name = string_bits(py, b"__builtins__");
                let left = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
                let right = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
                let globals_ptr = alloc_dict_with_pairs(py, &[name, left]);
                let globals = MoltObject::from_ptr(globals_ptr).bits();
                let source_ptr =
                    crate::alloc_function_obj(py, visibility_probe as *const () as usize as u64, 0);
                assert!(!globals_ptr.is_null() && !source_ptr.is_null());
                let source = MoltObject::from_ptr(source_ptr).bits();
                crate::function_set_globals_bits(py, source_ptr, globals).unwrap();
                assert!(
                    crate::object::field_storage::current_dictionary(py, source_ptr)
                        .unwrap()
                        .is_none()
                );
                let before = refcount(left);
                let retained = super::molt_get_attr_name_attached_body(py, source, name);
                assert_eq!(retained, left);
                assert_eq!(refcount(left), before + 1);
                assert!(!crate::exception_pending(py));
                assert!(
                    crate::object::field_storage::current_dictionary(py, source_ptr)
                        .unwrap()
                        .is_none()
                );
                let generic = super::molt_get_attr_generic_attached_body(
                    py,
                    source_ptr,
                    b"__builtins__".as_ptr(),
                    12,
                );
                assert_eq!(generic, left);
                dec_ref_bits(py, generic);

                let code_name = string_bits(py, b"__code__");
                let code = super::molt_get_attr_name_attached_body(py, source, code_name);
                assert!(!obj_from_bits(code).is_none() && !crate::exception_pending(py));
                assert!(crate::dict_set_in_place(py, globals_ptr, name, right));
                let rebuilt =
                    crate::builtins::functions::function_type_new_from_args(py, &[code, globals]);
                assert!(!obj_from_bits(rebuilt).is_none() && !crate::exception_pending(py));
                assert!(crate::dict_set_in_place(py, globals_ptr, name, left));
                for (function, expected) in [(source, left), (rebuilt, right)] {
                    let captured = super::molt_get_attr_name_attached_body(py, function, name);
                    assert_eq!(captured, expected);
                    dec_ref_bits(py, captured);
                }

                let dictionary_name = string_bits(py, b"__dict__");
                let dictionary =
                    super::molt_get_attr_name_attached_body(py, source, dictionary_name);
                assert!(!crate::exception_pending(py));
                assert!(crate::dict_set_in_place(
                    py,
                    obj_from_bits(dictionary).as_ptr().unwrap(),
                    name,
                    right
                ));
                let shadowed = super::molt_get_attr_name_attached_body(py, source, name);
                assert_eq!(shadowed, left, "the member precedes the public dictionary");
                dec_ref_bits(py, shadowed);
                dec_ref_bits(py, dictionary);
                let method = crate::builtins::functions::molt_bound_method_new_attached_body(
                    py,
                    source,
                    MoltObject::from_int(7).bits(),
                );
                let delegated = super::molt_get_attr_name_attached_body(py, method, name);
                assert_eq!(
                    delegated, left,
                    "Python bound methods delegate to their function"
                );
                dec_ref_bits(py, delegated);
                dec_ref_bits(py, method);

                for value in [
                    source,
                    rebuilt,
                    code,
                    globals,
                    left,
                    right,
                    code_name,
                    dictionary_name,
                    name,
                ] {
                    dec_ref_bits(py, value);
                }
                assert_eq!(
                    refcount(retained),
                    1,
                    "the returned reference outlives both functions and globals"
                );
                dec_ref_bits(py, retained);
            }
        });
    }

    #[test]
    fn function_type_builtins_member_is_readonly_and_role_specific() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let name = string_bits(py, b"__builtins__");
                let none = MoltObject::none().bits();
                let globals = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[name, none])).bits();
                let function_ptr =
                    crate::alloc_function_obj(py, visibility_probe as *const () as usize as u64, 0);
                assert!(!function_ptr.is_null());
                let function = MoltObject::from_ptr(function_ptr).bits();
                crate::function_set_globals_bits(py, function_ptr, globals).unwrap();
                let owner = crate::builtin_classes(py).function;
                let descriptor = super::molt_get_attr_name_attached_body(py, owner, name);
                assert!(!obj_from_bits(descriptor).is_none() && !crate::exception_pending(py));
                assert_eq!(
                    crate::object::layout::native_descriptor_flavor(
                        obj_from_bits(descriptor).as_ptr().unwrap()
                    ),
                    Some(crate::object::layout::NativeDescriptorFlavor::CallableMember)
                );
                let class_access =
                    crate::builtins::types::native_descriptors::native_descriptor_get(
                        py,
                        descriptor,
                        None,
                        Some(owner),
                    )
                    .unwrap();
                assert_eq!(class_access, descriptor);
                dec_ref_bits(py, class_access);
                let captured = crate::builtins::types::native_descriptors::native_descriptor_get(
                    py,
                    descriptor,
                    Some(function),
                    Some(owner),
                )
                .unwrap();
                assert_eq!(
                    captured, none,
                    "an explicitly captured None is a present value"
                );
                dec_ref_bits(py, captured);
                for value in [Some(MoltObject::from_int(42).bits()), None] {
                    assert!(
                        crate::builtins::types::native_descriptors::native_descriptor_mutate(
                            py, descriptor, function, value
                        )
                        .is_err()
                    );
                    let exception = crate::exception_last_bits_noinc(py).unwrap();
                    assert!(crate::builtins::exceptions::exception_matches_kind(
                        py,
                        exception,
                        crate::BuiltinExc::AttributeError
                    ));
                    crate::clear_exception(py);
                }
                for delete in [false, true] {
                    let mutation = if delete {
                        super::molt_del_attr_name_attached_body(py, function, name)
                    } else {
                        super::molt_set_attr_name_attached_body(
                            py,
                            function,
                            name,
                            MoltObject::from_int(42).bits(),
                        )
                    };
                    assert!(obj_from_bits(mutation).is_none());
                    assert!(crate::exception_pending(py));
                    crate::clear_exception(py);
                }
                let still_none = super::molt_get_attr_name_attached_body(py, function, name);
                assert_eq!(still_none, none);
                assert!(!crate::exception_pending(py));
                dec_ref_bits(py, still_none);

                let builtin = crate::builtins::methods::alloc_builtin_function(
                    py,
                    visibility_probe as *const () as usize as u64,
                    0,
                );
                let wrapper_ptr = crate::alloc_staticmethod_obj(py, function);
                assert!(!wrapper_ptr.is_null());
                let wrapper = MoltObject::from_ptr(wrapper_ptr).bits();
                for (object, expected) in [(function, true), (builtin, false), (wrapper, false)] {
                    let directory = crate::molt_dir_builtin_attached_body(py, object);
                    assert!(!crate::exception_pending(py));
                    let contains = crate::molt_contains_attached_body(py, directory, name);
                    assert!(!crate::exception_pending(py));
                    assert_eq!(crate::is_truthy(py, obj_from_bits(contains)), expected);
                    dec_ref_bits(py, contains);
                    dec_ref_bits(py, directory);
                }
                for other in [builtin, wrapper] {
                    let missing = super::molt_get_attr_name_attached_body(py, other, name);
                    assert!(obj_from_bits(missing).is_none());
                    assert!(crate::exception_pending(py));
                    crate::clear_exception(py);
                    assert!(
                        crate::builtins::types::native_descriptors::native_descriptor_get(
                            py,
                            descriptor,
                            Some(other),
                            Some(owner)
                        )
                        .is_err()
                    );
                    let exception = crate::exception_last_bits_noinc(py).unwrap();
                    assert!(crate::builtins::exceptions::exception_matches_kind(
                        py,
                        exception,
                        crate::BuiltinExc::TypeError
                    ));
                    crate::clear_exception(py);
                }
                for value in [builtin, wrapper, descriptor, function, globals, name] {
                    dec_ref_bits(py, value);
                }
            }
        });
    }

    #[test]
    fn function_type_builtins_member_keeps_one_physical_gc_owner() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                assert_eq!(crate::object::function_metadata::FUNCTION_PAYLOAD_WORDS, 28);
                use crate::builtins::functions::native_callable::CallableMetadata as Declared;
                use crate::object::function_metadata::FunctionMetadataField as Stored;
                assert_eq!(Declared::Builtins.operation(), 15);
                assert_eq!(
                    Declared::from_operation(0),
                    Some(Declared::Stored(Stored::Name))
                );
                assert_eq!(
                    Declared::from_operation(14),
                    Some(Declared::Stored(Stored::BindKind))
                );
                assert_eq!(Declared::from_operation(15), Some(Declared::Builtins));
                assert!(Declared::from_operation(16).is_none());
                assert!(
                    Stored::from_operation(15).is_none(),
                    "the projected attribute has no mutable storage operation"
                );
                let name = string_bits(py, b"__builtins__");
                let captured = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
                let replacement = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
                let globals_ptr = alloc_dict_with_pairs(py, &[name, captured]);
                let globals = MoltObject::from_ptr(globals_ptr).bits();
                let function_ptr =
                    crate::alloc_function_obj(py, visibility_probe as *const () as usize as u64, 0);
                assert!(!function_ptr.is_null());
                let function = MoltObject::from_ptr(function_ptr).bits();
                crate::function_set_globals_bits(py, function_ptr, globals).unwrap();
                assert!(crate::dict_set_in_place(py, globals_ptr, name, replacement));
                let mut captured_visits = 0;
                crate::object::heap_lifecycle::visit_payload_owned_values(
                    py,
                    function_ptr,
                    &mut |value| {
                        if value == captured {
                            captured_visits += 1;
                        }
                    },
                );
                assert_eq!(
                    captured_visits, 1,
                    "the declared member borrows the prefix owner"
                );
                let before = refcount(captured);
                crate::object::heap_lifecycle::clear_cycle_edges(py, function_ptr);
                assert_eq!(refcount(captured), before - 1);
                crate::object::heap_lifecycle::clear_cycle_edges(py, function_ptr);
                assert_eq!(
                    refcount(captured),
                    before - 1,
                    "repeated clear cannot detach the owner twice"
                );
                for value in [function, globals, captured, replacement, name] {
                    dec_ref_bits(py, value);
                }
            }
        });
    }

    fn assert_callable_attr(_py: &PyToken<'_>, bits: u64) {
        assert!(!MoltObject::from_bits(bits).is_none());
        assert!(crate::builtins::callable::is_callable_impl(_py, bits));
        dec_ref_bits(_py, bits);
    }

    fn assert_numeric_scalar_attr_entrypoints(_py: &PyToken<'_>, receiver_bits: u64, name: &[u8]) {
        let name_bits = string_bits(_py, name);
        let default_bits = string_bits(_py, b"default-sentinel");

        let resolved = super::resolve_scalar_attr(_py, receiver_bits, name_bits)
            .into_result()
            .expect("numeric scalar lookup succeeds")
            .expect("numeric scalar resolver should find attribute");
        assert_callable_attr(_py, resolved);

        let getattr_bits = super::molt_get_attr_name_attached_body(_py, receiver_bits, name_bits);
        assert_eq!(
            crate::builtins::exceptions::molt_exception_pending_attached_body(_py),
            0
        );
        assert_callable_attr(_py, getattr_bits);

        let default_getattr_bits = super::molt_get_attr_name_default_attached_body(
            _py,
            receiver_bits,
            name_bits,
            default_bits,
        );
        assert_eq!(
            crate::builtins::exceptions::molt_exception_pending_attached_body(_py),
            0
        );
        assert_ne!(default_getattr_bits, default_bits);
        assert_callable_attr(_py, default_getattr_bits);

        let direct_bits =
            crate::molt_object_getattribute_attached_body(_py, receiver_bits, name_bits);
        assert_eq!(
            crate::builtins::exceptions::molt_exception_pending_attached_body(_py),
            0
        );
        assert_callable_attr(_py, direct_bits);

        let has_bits = super::molt_has_attr_name_attached_body(_py, receiver_bits, name_bits);
        assert_eq!(MoltObject::from_bits(has_bits).as_bool(), Some(true));

        let missing_name_bits = string_bits(_py, b"definitely_missing_scalar_attr");
        let missing_has_bits =
            super::molt_has_attr_name_attached_body(_py, receiver_bits, missing_name_bits);
        assert_eq!(
            MoltObject::from_bits(missing_has_bits).as_bool(),
            Some(false)
        );

        let missing_default_bits = super::molt_get_attr_name_default_attached_body(
            _py,
            receiver_bits,
            missing_name_bits,
            default_bits,
        );
        assert_eq!(missing_default_bits, default_bits);
        dec_ref_bits(_py, missing_default_bits);

        dec_ref_bits(_py, missing_name_bits);
        dec_ref_bits(_py, default_bits);
        dec_ref_bits(_py, name_bits);
    }

    #[test]
    fn attributes_runtime_state_is_owned_and_clearable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            attributes_clear_runtime_state(_py, state);
            let attributes = &state.attributes;

            attributes
                .wrapper_members_version
                .store(17, Ordering::Release);

            attributes
                .attr_site_name_cache
                .lock()
                .unwrap()
                .insert(17, string_bits(_py, b"site-name"));

            for (idx, slot) in attributes.object_slots().iter().enumerate() {
                let label = format!("attributes-slot-{idx}");
                slot.store(string_bits(_py, label.as_bytes()), Ordering::Release);
            }

            attributes_clear_runtime_state(_py, state);

            assert!(attributes.attr_site_name_cache.lock().unwrap().is_empty());
            assert_eq!(
                attributes.wrapper_members_version.load(Ordering::Acquire),
                0
            );
            for slot in attributes.object_slots() {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    }

    #[test]
    fn numeric_scalar_attrs_share_runtime_resolver() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let inline_int = MoltObject::from_int(42).bits();
            let inline_bool = MoltObject::from_bool(true).bits();
            let inline_float = MoltObject::from_float(3.0).bits();
            let heap_bigint =
                crate::builtins::numbers::int_bits_from_bigint(_py, BigInt::from(1u64) << 100usize);
            let heap_nan = crate::object::ops::float_result_bits(_py, f64::NAN);

            assert_numeric_scalar_attr_entrypoints(_py, inline_int, b"bit_length");
            assert_numeric_scalar_attr_entrypoints(_py, inline_bool, b"bit_length");
            assert_numeric_scalar_attr_entrypoints(_py, heap_bigint, b"bit_length");
            assert_numeric_scalar_attr_entrypoints(_py, inline_float, b"is_integer");
            assert_numeric_scalar_attr_entrypoints(_py, heap_nan, b"is_integer");

            dec_ref_bits(_py, heap_bigint);
            dec_ref_bits(_py, heap_nan);
        });
    }

    #[test]
    fn hasattr_discards_owned_bound_method_result() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let dict_bits = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[])).bits();
            let name_bits = string_bits(_py, b"items");
            let before = refcount(dict_bits);

            let has_bits = super::molt_has_attr_name_attached_body(_py, dict_bits, name_bits);

            assert_eq!(MoltObject::from_bits(has_bits).as_bool(), Some(true));
            assert_eq!(
                crate::builtins::exceptions::molt_exception_pending_attached_body(_py),
                0
            );
            assert_eq!(
                refcount(dict_bits),
                before,
                "discarding hasattr(dict, 'items') must release the temporary bound method"
            );

            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, dict_bits);
        });
    }

    #[test]
    fn type_data_descriptors_precede_shadowing_class_namespace_entries() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let declared_name = string_bits(_py, b"ActualName");
            let class_bits = crate::molt_class_new_attached_body(_py, declared_name);
            let class_ptr = MoltObject::from_bits(class_bits)
                .as_ptr()
                .expect("class allocation");
            let name_key = string_bits(_py, b"__name__");
            let shadow_name = string_bits(_py, b"ShadowName");
            let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
            let dict_ptr = MoltObject::from_bits(dict_bits)
                .as_ptr()
                .expect("class namespace");
            unsafe {
                assert!(crate::dict_set_in_place(
                    _py,
                    dict_ptr,
                    name_key,
                    shadow_name
                ))
            };

            let resolved = super::molt_get_attr_name_attached_body(_py, class_bits, name_key);
            assert_eq!(
                crate::string_obj_to_owned(MoltObject::from_bits(resolved)).as_deref(),
                Some("ActualName")
            );

            dec_ref_bits(_py, resolved);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, declared_name);
            dec_ref_bits(_py, name_key);
            dec_ref_bits(_py, shadow_name);
        });
    }

    #[test]
    fn attr_site_name_cache_releases_replaced_and_cleared_names() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            attributes_clear_runtime_state(_py, state);
            let first = unsafe { attr_name_bits_for_site(_py, 42, b"first-name") }.unwrap();
            inc_ref_bits(_py, first);
            dec_ref_bits(_py, first);
            let first_with_cache = refcount(first);

            let second = unsafe { attr_name_bits_for_site(_py, 42, b"second-name") }.unwrap();
            assert_eq!(refcount(first) + 1, first_with_cache);
            assert_eq!(
                crate::string_obj_to_owned(MoltObject::from_bits(second)).as_deref(),
                Some("second-name")
            );
            inc_ref_bits(_py, second);
            dec_ref_bits(_py, second);
            let second_with_cache = refcount(second);

            attributes_clear_runtime_state(_py, state);
            assert_eq!(refcount(second) + 1, second_with_cache);

            dec_ref_bits(_py, first);
            dec_ref_bits(_py, second);
        });
    }

    #[test]
    fn attr_object_ic_uses_requested_name_when_a_site_is_reused() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = test_class_bits(_py, b"SiteCollisionOwner", &[]);
            let instance_bits = unsafe { test_instance_bits(_py, class_bits) };
            let first_name = string_bits(_py, b"first");
            let second_name = string_bits(_py, b"second");
            assert_eq!(
                super::molt_set_attr_name_attached_body(
                    _py,
                    instance_bits,
                    first_name,
                    MoltObject::from_int(11).bits()
                ),
                MoltObject::none().bits()
            );
            assert_eq!(
                super::molt_set_attr_name_attached_body(
                    _py,
                    instance_bits,
                    second_name,
                    MoltObject::from_int(22).bits(),
                ),
                MoltObject::none().bits()
            );
            let site = MoltObject::from_int(700).bits();
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic_attached_body(
                        _py,
                        instance_bits,
                        b"first".as_ptr(),
                        5,
                        site,
                    )
                },
                MoltObject::from_int(11).bits()
            );
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic_attached_body(
                        _py,
                        instance_bits,
                        b"second".as_ptr(),
                        6,
                        site,
                    )
                },
                MoltObject::from_int(22).bits()
            );
            assert!(!crate::exception_pending(_py));

            attributes_clear_runtime_state(_py, runtime_state(_py));
            dec_ref_bits(_py, second_name);
            dec_ref_bits(_py, first_name);
            dec_ref_bits(_py, instance_bits);
            dec_ref_bits(_py, class_bits);
        });
    }

    #[test]
    fn attr_object_ic_observes_each_receivers_instance_shadow() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = test_class_bits(
                _py,
                b"InstanceShadowOwner",
                &[(b"value", MoltObject::from_int(3).bits())],
            );
            let first = unsafe { test_instance_bits(_py, class_bits) };
            let second = unsafe { test_instance_bits(_py, class_bits) };
            let name_bits = string_bits(_py, b"value");
            assert_eq!(
                super::molt_set_attr_name_attached_body(
                    _py,
                    first,
                    name_bits,
                    MoltObject::from_int(31).bits()
                ),
                MoltObject::none().bits()
            );
            assert_eq!(
                super::molt_set_attr_name_attached_body(
                    _py,
                    second,
                    name_bits,
                    MoltObject::from_int(32).bits()
                ),
                MoltObject::none().bits()
            );
            let site = MoltObject::from_int(701).bits();
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic_attached_body(
                        _py,
                        first,
                        b"value".as_ptr(),
                        5,
                        site,
                    )
                },
                MoltObject::from_int(31).bits()
            );
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic_attached_body(
                        _py,
                        second,
                        b"value".as_ptr(),
                        5,
                        site,
                    )
                },
                MoltObject::from_int(32).bits()
            );
            assert!(!crate::exception_pending(_py));

            attributes_clear_runtime_state(_py, runtime_state(_py));
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, second);
            dec_ref_bits(_py, first);
            dec_ref_bits(_py, class_bits);
        });
    }

    #[test]
    fn attr_object_ic_runs_custom_getattribute_on_every_lookup() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let hook_bits = runtime_function_bits(
                _py,
                "changing_getattribute",
                changing_getattribute as *const (),
                2,
            );
            let class_bits = test_class_bits(
                _py,
                b"ChangingGetattributeOwner",
                &[(b"__getattribute__", hook_bits)],
            );
            dec_ref_bits(_py, hook_bits);
            let instance_bits = unsafe { test_instance_bits(_py, class_bits) };
            CUSTOM_GETATTRIBUTE_CALLS.store(0, Ordering::SeqCst);
            let site = MoltObject::from_int(702).bits();

            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic_attached_body(
                        _py,
                        instance_bits,
                        b"probe".as_ptr(),
                        5,
                        site,
                    )
                },
                MoltObject::from_int(1).bits()
            );
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic_attached_body(
                        _py,
                        instance_bits,
                        b"probe".as_ptr(),
                        5,
                        site,
                    )
                },
                MoltObject::from_int(2).bits()
            );
            assert_eq!(CUSTOM_GETATTRIBUTE_CALLS.load(Ordering::SeqCst), 2);
            assert!(!crate::exception_pending(_py));

            attributes_clear_runtime_state(_py, runtime_state(_py));
            dec_ref_bits(_py, instance_bits);
            dec_ref_bits(_py, class_bits);
        });
    }

    #[test]
    fn generic_alias_parameters_are_protocol_owned_cached_gc_edges() {
        use crate::{alloc_generic_alias, alloc_tuple, builtin_classes, object_type_id};
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = test_class_bits(
                py,
                b"RenamedParameter",
                &[(b"__typing_subst__", MoltObject::none().bits())],
            );
            let first = unsafe { test_instance_bits(py, class) };
            let second = unsafe { test_instance_bits(py, class) };
            let arguments = alloc_tuple(py, &[second, first, second]);
            assert!(!arguments.is_null());
            let arguments = MoltObject::from_ptr(arguments).bits();
            let before = refcount(first);
            let alias = alloc_generic_alias(py, builtin_classes(py).tuple, arguments);
            assert!(!alias.is_null());
            let alias_bits = MoltObject::from_ptr(alias).bits();
            let name = string_bits(py, b"__parameters__");
            let parameters = super::molt_get_attr_name_attached_body(py, alias_bits, name);
            assert!(!crate::exception_pending(py));
            let tuple = obj_from_bits(parameters).as_ptr().unwrap();
            assert_eq!(unsafe { object_type_id(tuple) }, crate::TYPE_ID_TUPLE);
            let values = unsafe { crate::object::seq_access::pin_tuple(py, tuple) }.unwrap();
            assert_eq!(&*values, &[second, first]);
            drop(values);
            assert_eq!(refcount(first), before + 1);
            let again = super::molt_get_attr_name_attached_body(py, alias_bits, name);
            assert_eq!(again, parameters, "success must cache the actual tuple");
            let mut edges = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_payload_owned_values(py, alias, &mut |bits| {
                    edges.push(bits)
                });
            }
            assert!(
                edges.contains(&parameters),
                "cache must participate in canonical GC traversal"
            );
            dec_ref_bits(py, again);
            dec_ref_bits(py, parameters);
            dec_ref_bits(py, alias_bits);
            assert_eq!(
                refcount(first),
                before,
                "alias retirement must release its cache owner"
            );
            for bits in [name, arguments, second, first, class] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn generic_alias_c_api_constructs_the_same_parameter_owner() {
        use molt_cpython_abi::api::{object, refcount};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            let class = test_class_bits(
                py,
                b"CParameter",
                &[(b"__typing_subst__", MoltObject::none().bits())],
            );
            let parameter = unsafe { test_instance_bits(py, class) };
            // The transaction and live GIL entry keep both borrowed runtime
            // handles valid while the bridge creates their independent C owners.
            let (origin, argument) = unsafe {
                (
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(crate::builtin_classes(py).list),
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(parameter),
                )
            };
            assert!(!origin.is_null() && !argument.is_null());
            unsafe {
                let alias = object::Py_GenericAlias(origin, argument);
                assert!(!alias.is_null());
                let managed = GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(alias)
                    .expect("canonical runtime alias");
                assert_eq!(
                    crate::object_type_id(obj_from_bits(managed.bits()).as_ptr().unwrap()),
                    crate::TYPE_ID_GENERIC_ALIAS
                );
                let parameters = object::PyObject_GetAttrString(alias, c"__parameters__".as_ptr());
                assert!(!parameters.is_null());
                let again = object::PyObject_GetAttrString(alias, c"__parameters__".as_ptr());
                assert_eq!(parameters, again, "C consumers must share the cached tuple");
                assert_eq!(
                    molt_cpython_abi::api::sequences::PyTuple_Size(parameters),
                    1
                );
                let actual = molt_cpython_abi::api::sequences::PyTuple_GetItem(parameters, 0);
                assert_eq!(actual, argument);
                for value in [again, parameters, alias, argument, origin] {
                    refcount::Py_DECREF(value);
                }
            }
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, parameter);
            dec_ref_bits(py, class);
        });
    }

    #[test]
    fn union_parameter_projection_reuses_the_native_collector_and_gc_owner() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = test_class_bits(
                py,
                b"UnionParameter",
                &[(b"__typing_subst__", MoltObject::none().bits())],
            );
            let parameter = unsafe { test_instance_bits(py, class) };
            let arguments = crate::alloc_tuple(py, &[parameter, parameter]);
            assert!(!arguments.is_null());
            let arguments = MoltObject::from_ptr(arguments).bits();
            let union = crate::alloc_union_type(py, arguments);
            assert!(!union.is_null());
            let union_bits = MoltObject::from_ptr(union).bits();
            let name = string_bits(py, b"__parameters__");
            let parameters = super::molt_get_attr_name_attached_body(py, union_bits, name);
            assert!(!crate::exception_pending(py));
            let values = unsafe {
                crate::object::seq_access::pin_tuple(
                    py,
                    obj_from_bits(parameters).as_ptr().unwrap(),
                )
            }
            .unwrap();
            assert_eq!(&*values, &[parameter]);
            drop(values);
            let mut edges = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_payload_owned_values(py, union, &mut |bits| {
                    edges.push(bits)
                });
            }
            assert!(edges.contains(&parameters));
            for bits in [parameters, name, union_bits, arguments, parameter, class] {
                dec_ref_bits(py, bits);
            }
        });
    }

    static PARAMETER_REENTRY_ALIAS: AtomicU64 = AtomicU64::new(0);
    static PARAMETER_REENTRY_CALLS: AtomicU64 = AtomicU64::new(0);

    extern "C" fn reentrant_parameter_getter(__context: u64, _receiver: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        {
            let py = __py;
            let call = PARAMETER_REENTRY_CALLS.fetch_add(1, Ordering::Relaxed);
            let value = if call == 0 {
                let name = string_bits(py, b"__parameters__");
                let inner = super::molt_get_attr_name_attached_body(
                    py,
                    PARAMETER_REENTRY_ALIAS.load(Ordering::Acquire),
                    name,
                );
                assert!(!crate::exception_pending(py));
                dec_ref_bits(py, inner);
                dec_ref_bits(py, name);
                22
            } else {
                11
            };
            let tuple = crate::alloc_tuple(py, &[MoltObject::from_int(value).bits()]);
            assert!(!tuple.is_null());
            MoltObject::from_ptr(tuple).bits()
        }
    }

    #[test]
    fn reentrant_parameter_cache_uses_outer_completion_order() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            PARAMETER_REENTRY_CALLS.store(0, Ordering::Relaxed);
            let getter = runtime_function_bits(
                py,
                "reentrant_parameter_getter",
                reentrant_parameter_getter as *const (),
                1,
            );
            let descriptor = crate::molt_property_new_attached_body(
                py,
                getter,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            );
            let class = test_class_bits(
                py,
                b"ReentrantParameterProvider",
                &[(b"__parameters__", descriptor)],
            );
            let provider = unsafe { test_instance_bits(py, class) };
            let arguments = crate::alloc_tuple(py, &[provider]);
            assert!(!arguments.is_null());
            let arguments = MoltObject::from_ptr(arguments).bits();
            let alias = crate::alloc_generic_alias(py, crate::builtin_classes(py).list, arguments);
            assert!(!alias.is_null());
            let alias = MoltObject::from_ptr(alias).bits();
            PARAMETER_REENTRY_ALIAS.store(alias, Ordering::Release);
            let name = string_bits(py, b"__parameters__");
            let first = super::molt_get_attr_name_attached_body(py, alias, name);
            assert!(!crate::exception_pending(py));
            let second = super::molt_get_attr_name_attached_body(py, alias, name);
            assert_eq!(first, second);
            let values = unsafe {
                crate::object::seq_access::pin_tuple(py, obj_from_bits(first).as_ptr().unwrap())
            }
            .unwrap();
            assert_eq!(
                &*values,
                &[MoltObject::from_int(22).bits()],
                "matches actual CPython 3.12/3.14 reentry oracle"
            );
            assert_eq!(PARAMETER_REENTRY_CALLS.load(Ordering::Relaxed), 2);
            drop(values);
            PARAMETER_REENTRY_ALIAS.store(0, Ordering::Release);
            for bits in [
                second, first, name, alias, arguments, provider, class, descriptor, getter,
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn parameter_cache_cycle_is_actually_reclaimed_after_metadata_replacement() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::state::metrics::enable_profile_for_test();
        crate::with_gil_entry_nopanic!(py, {
            let parameter = crate::alloc_list(py, &[]);
            assert!(!parameter.is_null());
            let parameter_bits = MoltObject::from_ptr(parameter).bits();
            let metadata = crate::alloc_tuple(py, &[parameter_bits]);
            assert!(!metadata.is_null());
            let metadata = MoltObject::from_ptr(metadata).bits();
            let class = test_class_bits(
                py,
                b"CacheOnlyCycleProvider",
                &[(b"__parameters__", metadata)],
            );
            let provider = unsafe { test_instance_bits(py, class) };
            let arguments = crate::alloc_tuple(py, &[provider]);
            assert!(!arguments.is_null());
            let arguments = MoltObject::from_ptr(arguments).bits();
            let alias = crate::alloc_generic_alias(py, crate::builtin_classes(py).list, arguments);
            assert!(!alias.is_null());
            let alias_bits = MoltObject::from_ptr(alias).bits();
            let name = string_bits(py, b"__parameters__");
            let cached = super::molt_get_attr_name_attached_body(py, alias_bits, name);
            assert!(!crate::exception_pending(py));
            let cache_pointer = obj_from_bits(cached).as_ptr().unwrap();
            let values =
                unsafe { crate::object::seq_access::pin_tuple(py, cache_pointer) }.unwrap();
            assert_eq!(&*values, &[parameter_bits]);
            drop(values);
            // Remove the original provider edge. The only remaining path to
            // parameter is alias -> cached tuple -> parameter -> alias.
            let empty = crate::alloc_tuple(py, &[]);
            assert!(!empty.is_null());
            let empty = MoltObject::from_ptr(empty).bits();
            let result = super::molt_set_attr_name_attached_body(py, class, name, empty);
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, result);
            // The setup lookup's descriptor snapshot separately owns the old
            // metadata. Retire that TLS root before isolating the object cache;
            // this does not clear the GenericAlias parameter cache.
            crate::builtins::attr::clear_attr_tls_caches(py);
            let result = crate::molt_list_append_attached_body(py, parameter_bits, alias_bits);
            dec_ref_bits(py, result);
            for bits in [
                cached,
                metadata,
                arguments,
                provider,
                alias_bits,
                parameter_bits,
            ] {
                dec_ref_bits(py, bits);
            }
            for pointer in [alias, cache_pointer, parameter] {
                assert!(unsafe { crate::object::gc::gc_is_tracked(pointer) });
            }
            assert_eq!(
                refcount(alias_bits),
                1,
                "only the parameter list owns the alias"
            );
            assert_eq!(refcount(cached), 1, "only the alias owns the cached tuple");
            assert_eq!(
                refcount(parameter_bits),
                1,
                "only the cached tuple owns the list"
            );
            let before = crate::DEALLOC_COUNT.load(Ordering::Relaxed);
            let stats = unsafe { crate::object::gc::collect_cycles(py) };
            assert!(
                stats.collected >= 3,
                "cache-only cycle must actually collect"
            );
            assert!(crate::DEALLOC_COUNT.load(Ordering::Relaxed) >= before + 3);
            for pointer in [alias, cache_pointer, parameter] {
                assert!(!unsafe { crate::object::gc::gc_is_tracked(pointer) });
            }
            for bits in [name, empty, class] {
                dec_ref_bits(py, bits);
            }
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn parameter_collection_resource_denial_retires_partial_tuples_and_owners() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }

        // Exhaust the real remaining budget with ordinary live allocations.
        // Repeating this after collection refusal detects leaked reservations,
        // independently of parameter owners or the collector's implementation.
        fn available_capacity(py: &PyToken<'_>, label: &str) -> usize {
            let mut probes = Vec::new();
            let count = loop {
                let probe = crate::alloc_string(py, b"capacity probe");
                if probe.is_null() {
                    crate::test_support::assert_and_clear_emergency_memory_error(py);
                    break probes.len();
                }
                probes.push(MoltObject::from_ptr(probe).bits());
                assert!(
                    probes.len() < 1024,
                    "{label}: bounded capacity census must exhaust"
                );
            };
            for probe in probes {
                dec_ref_bits(py, probe);
            }
            count
        }

        fn refusal_case(
            py: &PyToken<'_>,
            label: &str,
            args: u64,
            parameters: &[u64],
            limits: ResourceLimits,
            large_outer_result: bool,
        ) {
            let name = string_bits(py, b"__parameters__");
            // A real positive witness checks the exact parameter identity/order
            // and warms immutable lookup authority before denying resources.
            let warm = crate::alloc_generic_alias(py, crate::builtin_classes(py).list, args);
            assert!(!warm.is_null());
            let warm = MoltObject::from_ptr(warm).bits();
            let result = super::molt_get_attr_name_attached_body(py, warm, name);
            assert!(!crate::exception_pending(py), "{label}: positive witness");
            let values = unsafe {
                crate::object::seq_access::pin_tuple(py, obj_from_bits(result).as_ptr().unwrap())
            }
            .unwrap();
            assert_eq!(
                &*values, parameters,
                "{label}: independent expected identities"
            );
            drop(values);
            dec_ref_bits(py, result);
            dec_ref_bits(py, warm);

            let owners: Vec<_> = parameters.iter().map(|&bits| refcount(bits)).collect();
            let alias = crate::alloc_generic_alias(py, crate::builtin_classes(py).list, args);
            assert!(!alias.is_null());
            let alias_bits = MoltObject::from_ptr(alias).bits();
            let reset = Reset;
            set_tracker(Box::new(LimitedTracker::new(&limits)));
            if large_outer_result {
                // The real memory limit admits the nested one-element result,
                // but refuses the larger final result. These direct allocator
                // controls prevent a normalization/early refusal passing as
                // coverage of the final result after nested tuple retirement.
                let small = crate::alloc_tuple(py, &parameters[..1]);
                assert!(!small.is_null(), "{label}: nested-size allocation admitted");
                dec_ref_bits(py, MoltObject::from_ptr(small).bits());
                let large = crate::alloc_tuple(py, parameters);
                assert!(large.is_null(), "{label}: outer-size allocation refused");
                crate::test_support::assert_and_clear_emergency_memory_error(py);
            }
            let capacity_before = available_capacity(py, label);
            let result = super::molt_get_attr_name_attached_body(py, alias_bits, name);
            assert!(obj_from_bits(result).is_none(), "{label}: must refuse");
            crate::test_support::assert_and_clear_emergency_memory_error(py);
            assert!(
                obj_from_bits(unsafe {
                    crate::object::layout::generic_alias_parameters_bits(alias)
                })
                .is_none(),
                "{label}: no partial publication"
            );
            let observed: Vec<_> = parameters.iter().map(|&bits| refcount(bits)).collect();
            assert_eq!(observed, owners, "{label}: temporary owners retired");
            assert_eq!(
                available_capacity(py, label),
                capacity_before,
                "{label}: all live capacity restored"
            );
            drop(reset);
            let retry = super::molt_get_attr_name_attached_body(py, alias_bits, name);
            assert!(
                !crate::exception_pending(py),
                "{label}: explicit read after resource restoration"
            );
            let values = unsafe {
                crate::object::seq_access::pin_tuple(py, obj_from_bits(retry).as_ptr().unwrap())
            }
            .unwrap();
            assert_eq!(
                &*values, parameters,
                "{label}: failed collection leaves a reusable receiver"
            );
            drop(values);
            dec_ref_bits(py, retry);
            dec_ref_bits(py, alias_bits);
            dec_ref_bits(py, name);
            let observed: Vec<_> = parameters.iter().map(|&bits| refcount(bits)).collect();
            assert_eq!(observed, owners, "{label}: failed receiver retirement");
            assert!(!crate::exception_pending(py));
        }

        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
            assert!(crate::cpython_abi_hooks::register_cpython_hooks());
            crate::with_gil_entry_nopanic!(py, {
                let release = string_bits(py, b"final");
                let result = crate::object::ops_sys::molt_sys_set_version_info_attached_body(
                    py,
                    MoltObject::from_int(3).bits(),
                    MoltObject::from_int(14).bits(),
                    MoltObject::from_int(0).bits(),
                    release,
                    MoltObject::from_int(0).bits(),
                );
                dec_ref_bits(py, result);
                dec_ref_bits(py, release);
                assert!(!crate::exception_pending(py));
                assert!(crate::object::ops_sys::runtime_target_at_least(py, 3, 14));
                let class = test_class_bits(
                    py,
                    b"BudgetParameter",
                    &[(b"__typing_subst__", MoltObject::none().bits())],
                );
                let parameters: Vec<_> = (0..128)
                    .map(|_| unsafe { test_instance_bits(py, class) })
                    .collect();
                let list = crate::alloc_list(py, &parameters[..1]);
                let nested = crate::alloc_tuple(py, &parameters[..1]);
                assert!(!list.is_null() && !nested.is_null());
                let list = MoltObject::from_ptr(list).bits();
                let nested = MoltObject::from_ptr(nested).bits();
                let list_args = crate::alloc_tuple(py, &[list]);
                let tuple_args = crate::alloc_tuple(py, &[nested]);
                assert!(!list_args.is_null() && !tuple_args.is_null());
                let list_args = MoltObject::from_ptr(list_args).bits();
                let tuple_args = MoltObject::from_ptr(tuple_args).bits();

                // LimitedTracker counts concurrently LIVE allocations, not
                // cumulative events. Separate inputs/limits isolate each stage
                // instead of assuming three sequential stages need three slots.
                for budget in [0, 1] {
                    refusal_case(
                        py,
                        if budget == 0 {
                            "normalization snapshot"
                        } else {
                            "normalization tuple"
                        },
                        list_args,
                        &parameters[..1],
                        ResourceLimits {
                            max_allocations: Some(budget),
                            ..ResourceLimits::default()
                        },
                        false,
                    );
                }
                refusal_case(
                    py,
                    "nested result tuple",
                    tuple_args,
                    &parameters[..1],
                    ResourceLimits {
                        max_allocations: Some(0),
                        ..ResourceLimits::default()
                    },
                    false,
                );

                let mut outer = vec![nested];
                outer.extend_from_slice(&parameters[1..]);
                let outer_args = crate::alloc_tuple(py, &outer);
                assert!(!outer_args.is_null());
                let outer_args = MoltObject::from_ptr(outer_args).bits();
                let memory_limit = crate::object::layout::TupleStorage::object_size(1).unwrap() * 8;
                assert!(
                    crate::object::layout::TupleStorage::object_size(parameters.len()).unwrap()
                        > memory_limit
                );
                refusal_case(
                    py,
                    "outer result after nested tuple",
                    outer_args,
                    &parameters,
                    ResourceLimits {
                        max_memory: Some(memory_limit),
                        ..ResourceLimits::default()
                    },
                    true,
                );

                for bits in [outer_args, tuple_args, list_args, nested, list] {
                    dec_ref_bits(py, bits);
                }
                for parameter in parameters {
                    dec_ref_bits(py, parameter);
                }
                dec_ref_bits(py, class);
                assert!(!crate::exception_pending(py));
            });
        });
    }
}
