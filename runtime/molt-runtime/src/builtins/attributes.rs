use crate::PyToken;
use crate::object::{HEADER_FLAG_COROUTINE, NEWLINE_KIND_CR, NEWLINE_KIND_CRLF, NEWLINE_KIND_LF};
use molt_obj_model::MoltObject;
use std::sync::atomic::Ordering;

use crate::async_rt::generators::generator_yieldfrom_bits;
use crate::builtins::annotations::pep649_enabled;
use crate::builtins::attr::{
    attr_lookup_ptr_allow_missing, class_slots_info, clear_attribute_error_if_pending,
    exception_is_attribute_error, object_attr_lookup_raw,
};
use crate::builtins::containers::tuple_method_bits;
use crate::builtins::exceptions::{
    ExceptionFieldSlot, exception_matches_builtin_name, exception_replace_field_bits,
    exception_replace_suppress_context, exception_typed_field_delete, exception_typed_field_get,
    exception_typed_field_replace, molt_exception_last_pending,
};
use crate::builtins::frames::suspended_frame_bits;
use crate::builtins::methods::{
    asyncgen_method_bits, complex_method_bits, coroutine_method_bits, generator_method_bits,
    object_method_bits, range_method_bits, type_method_bits,
};
use crate::*;

mod class_lookup;
mod mutation;
mod scalar_attrs;
mod state;
mod wrapper_attrs;

pub use wrapper_attrs::{
    molt_wrapper_member_delete, molt_wrapper_member_get, molt_wrapper_member_set,
};
pub(crate) use wrapper_attrs::{
    prepare_wrapper_members, property_name_value, wrapper_copy_metadata, wrapper_publish_members,
};

use class_lookup::classed_attr_lookup_without_dict;
pub(crate) use class_lookup::{type_attr_lookup_ptr, type_attr_lookup_ptr_default};
pub(crate) use mutation::{
    dataclass_delattr_raw_unchecked, dataclass_setattr_raw_unchecked, del_attr_ptr,
    object_delattr_raw, object_setattr_raw,
};
pub use mutation::{
    molt_del_attr_generic, molt_del_attr_name, molt_del_attr_object, molt_del_attr_ptr,
    molt_set_attr_generic, molt_set_attr_name, molt_set_attr_object, molt_set_attr_ptr,
};
pub(crate) use scalar_attrs::{is_numeric_scalar_attr_receiver, resolve_scalar_attr};
use state::{
    ATTR_LOOKUP_TRACE_LINES, AttrLookupTraceGuard, attr_site_name_cache, attributes_state,
    trace_attr_lookup_enabled,
};
pub(crate) use state::{
    AttributesRuntimeState, attributes_clear_runtime_state, debug_bound_method_enabled,
};

fn native_descriptor_metadata_field(
    name: &str,
) -> Option<crate::builtins::types::NativeDescriptorMetadata> {
    use crate::builtins::types::NativeDescriptorMetadata;
    match name {
        "__name__" => Some(NativeDescriptorMetadata::Name),
        "__qualname__" => Some(NativeDescriptorMetadata::Qualname),
        "__objclass__" => Some(NativeDescriptorMetadata::Owner),
        "__doc__" => Some(NativeDescriptorMetadata::Doc),
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
        let bits = attr_name_bits_from_bytes(_py, slice)?;
        inc_ref_bits(_py, bits);
        cache.insert(site_id, bits);
        Some(bits)
    }
}

fn is_typing_param(_py: &PyToken<'_>, bits: u64) -> bool {
    if obj_from_bits(bits).is_none() {
        return false;
    }
    let class_bits = type_of_bits(_py, bits);
    let name = class_name_for_error(class_bits);
    matches!(name.as_str(), "_TypeVar" | "_ParamSpec" | "_TypeVarTuple")
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_positions(code_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let code_obj = obj_from_bits(code_bits);
        let Some(code_ptr) = code_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code.co_positions() requires code");
        };
        unsafe {
            if object_type_id(code_ptr) != TYPE_ID_CODE {
                return raise_exception::<_>(_py, "TypeError", "code.co_positions() requires code");
            }
        }

        let mut owned_table = false;
        let mut table_bits = unsafe { code_linetable_bits(code_ptr) };
        let needs_fallback = if let Some(table_ptr) = obj_from_bits(table_bits).as_ptr() {
            unsafe {
                object_type_id(table_ptr) != TYPE_ID_TUPLE
                    || crate::object::seq_access::len(table_ptr) == 0
            }
        } else {
            true
        };

        if needs_fallback {
            let mut line = unsafe { code_firstlineno(code_ptr) };
            let mut start_col = 0i64;
            let mut end_col = 0i64;
            if let Some(filename) =
                string_obj_to_owned(obj_from_bits(unsafe { code_filename_bits(code_ptr) }))
                && let Ok(contents) = std::fs::read_to_string(&filename)
            {
                let lines: Vec<&str> = contents.lines().collect();
                let mut line_index = if line > 0 {
                    (line as usize).saturating_sub(1)
                } else {
                    0
                };
                if let Some(raw_line) = lines.get(line_index).copied() {
                    let mut trimmed = raw_line.trim_end_matches(['\r', '\n']);
                    let starts_def = {
                        let lead = trimmed.trim_start();
                        lead.starts_with("def ") || lead.starts_with("async def ")
                    };
                    if starts_def {
                        let next_index = line_index.saturating_add(1);
                        if let Some(next_line) = lines.get(next_index).copied() {
                            line_index = next_index;
                            line = (line_index + 1) as i64;
                            trimmed = next_line.trim_end_matches(['\r', '\n']);
                        }
                    }
                    end_col = trimmed.chars().count() as i64;
                    if let Some(pos) = trimmed.find("return ") {
                        start_col = (pos + "return ".len()) as i64;
                    } else if let Some(pos) = trimmed.chars().position(|ch| !ch.is_whitespace()) {
                        start_col = pos as i64;
                    }
                    if line <= 0 {
                        line = (line_index + 1) as i64;
                    }
                }
            }
            let line_bits = MoltObject::from_int(line).bits();
            let start_col_bits = MoltObject::from_int(start_col).bits();
            let end_col_bits = MoltObject::from_int(end_col).bits();
            let pos_ptr = alloc_tuple(_py, &[line_bits, line_bits, start_col_bits, end_col_bits]);
            if pos_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let pos_bits = MoltObject::from_ptr(pos_ptr).bits();
            let table_ptr = alloc_tuple(_py, &[pos_bits]);
            dec_ref_bits(_py, pos_bits);
            if table_ptr.is_null() {
                return MoltObject::none().bits();
            }
            table_bits = MoltObject::from_ptr(table_ptr).bits();
            owned_table = true;
        }

        let iter_bits = molt_iter(table_bits);
        if owned_table {
            dec_ref_bits(_py, table_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        iter_bits
    })
}

#[unsafe(no_mangle)]
/// Attribute lookup for a `TYPE_ID_FOREIGN` wrapper: route through the wrapped
/// C object's own type slots via the ABI bridge. Type name-dunders
/// (`__name__`/`__qualname__`) resolve hooks-free directly from the C `tp_name`
/// (runtime attribute name + runtime string allocator), robust across
/// split-runtime modules where the getattr's ABI hook table may be a stub.
/// Returns `Some(bits)` on success, or `None` (caller raises `AttributeError`,
/// or propagates a pending exception the C slot left set).
///
/// # Safety
/// `obj_ptr` must be a live `TYPE_ID_FOREIGN` object.
pub(crate) unsafe fn foreign_attr_lookup(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    let c_ptr = unsafe { crate::object::foreign::foreign_ptr_from_obj(obj_ptr) };
    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))?;
    if attr_name == "__name__" || attr_name == "__qualname__" {
        let mut buf = [0u8; 256];
        let n = unsafe {
            molt_cpython_abi::bridge::molt_foreign_type_dunder_name(
                c_ptr,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        if n >= 0 && (n as usize) <= buf.len() {
            let name_ptr = crate::object::builders::alloc_string(_py, &buf[..n as usize]);
            if !name_ptr.is_null() {
                return Some(MoltObject::from_ptr(name_ptr).bits());
            }
        }
    }
    let val = unsafe { molt_cpython_abi::bridge::molt_foreign_getattr(c_ptr, attr_bits) };
    match val.decode() {
        molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => Some(bits),
        molt_cpython_abi::hooks::DecodedHandleResult::Missing
        | molt_cpython_abi::hooks::DecodedHandleResult::Error => None,
    }
}

pub(crate) unsafe fn attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        let type_id = object_type_id(obj_ptr);
        if type_id == TYPE_ID_TYPE {
            return type_attr_lookup_ptr(_py, obj_ptr, attr_bits);
        }
        if type_id == TYPE_ID_MODULE {
            return module_attr_lookup(_py, obj_ptr, attr_bits);
        }
        if !matches!(
            type_id,
            TYPE_ID_FOREIGN | TYPE_ID_EXCEPTION | TYPE_ID_FUNCTION | TYPE_ID_MODULE
        ) && let Some(class) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
            && object_type_id(class) == TYPE_ID_TYPE
        {
            return class_lookup::attribute_lookup_transaction(
                _py,
                obj_ptr,
                class,
                attr_bits,
                object_method_bits(_py, "__getattribute__"),
                || attr_lookup_ptr_default(_py, obj_ptr, attr_bits),
            );
        }
        attr_lookup_ptr_default(_py, obj_ptr, attr_bits)
    }
}

/// Complete builtin/default lookup without user __getattribute__ or
/// __getattr__. Normal lookup wraps this entire operation in one transaction,
/// including the trailing builtin members of dictless receiver kinds.
pub(crate) unsafe fn attr_lookup_ptr_default(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
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
            return foreign_attr_lookup(_py, obj_ptr, attr_bits);
        }
        if type_id == crate::TYPE_ID_NATIVE_DESCRIPTOR
            && let Some(field) = string_obj_to_owned(obj_from_bits(attr_bits))
                .and_then(|name| native_descriptor_metadata_field(&name))
        {
            let result = crate::builtins::types::native_descriptor_metadata(_py, obj_bits, field);
            if exception_pending(_py) {
                dec_ref_bits(_py, result);
                return None;
            }
            return Some(result);
        }
        if matches!(type_id, TYPE_ID_BIGINT | TYPE_ID_FLOAT) {
            let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            if let Some(bits) = resolve_scalar_attr(_py, self_bits, name.as_str()) {
                return Some(bits);
            }
        }
        if !crate::object::heap_kind_has_class_shape(type_id)
            && !matches!(
                type_id,
                TYPE_ID_DATACLASS | TYPE_ID_TYPE | TYPE_ID_EXCEPTION
            )
        {
            let class_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(class_name_bits),
            ) {
                let res_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
                inc_ref_bits(_py, res_bits);
                return Some(res_bits);
            }
        }
        if type_id == TYPE_ID_MODULE {
            return crate::builtins::attr::module_attr_lookup_default(_py, obj_ptr, attr_bits);
        }
        if type_id == TYPE_ID_BOUND_METHOD {
            let name = string_obj_to_owned(obj_from_bits(attr_bits));
            if let Some(name) = name.as_deref() {
                match name {
                    "__func__" => {
                        let func_bits = bound_method_func_bits(obj_ptr);
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                    "__self__" => {
                        let self_bits = bound_method_self_bits(obj_ptr);
                        inc_ref_bits(_py, self_bits);
                        return Some(self_bits);
                    }
                    "__name__" | "__qualname__" | "__doc__" => {
                        let func_bits = bound_method_func_bits(obj_ptr);
                        if let Some(func_ptr) = obj_from_bits(func_bits).as_ptr()
                            && object_type_id(func_ptr) == TYPE_ID_FUNCTION
                            && let Some(bits) = function_attr_bits(_py, func_ptr, attr_bits)
                        {
                            inc_ref_bits(_py, bits);
                            return Some(bits);
                        }
                    }
                    _ => {}
                }
            }
        }
        if type_id == TYPE_ID_EXCEPTION {
            let name = string_obj_to_owned(obj_from_bits(attr_bits));
            let attr_name = name.as_deref()?;
            if let Some(result) = exception_typed_field_get(_py, obj_ptr, attr_name) {
                match result {
                    Ok(bits) => return Some(bits),
                    Err(message) => {
                        let _ = raise_exception::<u64>(_py, "AttributeError", message);
                        return None;
                    }
                }
            }
            match attr_name {
                "__cause__" => {
                    let bits = exception_cause_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__context__" => {
                    let bits = exception_context_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__suppress_context__" => {
                    let bits = exception_suppress_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__traceback__" => {
                    let bits = exception_materialize_traceback_bits(_py, obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__notes__" => {
                    let bits = exception_notes_bits(obj_ptr);
                    if !obj_from_bits(bits).is_none() {
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                }
                "__class__" => {
                    let class_bits = object_class_bits(obj_ptr);
                    inc_ref_bits(_py, class_bits);
                    return Some(class_bits);
                }
                "__dict__" => {
                    let mut dict_bits = exception_dict_bits(obj_ptr);
                    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                        if dict_ptr.is_null() {
                            return None;
                        }
                        let new_bits = MoltObject::from_ptr(dict_ptr).bits();
                        if exception_replace_field_bits(
                            _py,
                            MoltObject::from_ptr(obj_ptr).bits(),
                            ExceptionFieldSlot::Dict,
                            new_bits,
                        )
                        .is_err()
                        {
                            dec_ref_bits(_py, new_bits);
                            return None;
                        }
                        dec_ref_bits(_py, new_bits);
                        dict_bits = new_bits;
                    }
                    inc_ref_bits(_py, dict_bits);
                    return Some(dict_bits);
                }
                "args" => {
                    let args_bits = exception_materialized_args_bits(_py, obj_ptr);
                    if obj_from_bits(args_bits).is_none() {
                        return None;
                    }
                    inc_ref_bits(_py, args_bits);
                    return Some(args_bits);
                }
                _ => {}
            }
            let dict_bits = exception_dict_bits(obj_ptr);
            if !obj_from_bits(dict_bits).is_none()
                && dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && let Some(bits) = dict_get_in_place(_py, dict_ptr, attr_bits)
            {
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            let class_bits = object_class_bits(obj_ptr);
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
            {
                if let Some(val_bits) =
                    class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), attr_bits)
                {
                    return Some(val_bits);
                }
                if exception_pending(_py) {
                    return None;
                }
            }
        }
        if type_id == TYPE_ID_GENERATOR
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            match name.as_str() {
                "gi_running" => {
                    return Some(MoltObject::from_bool(generator_running(obj_ptr)).bits());
                }
                "gi_code" => {
                    let code_bits = crate::object::aux_header::object_frame_code_bits(obj_ptr);
                    if code_bits != 0 {
                        inc_ref_bits(_py, code_bits);
                        return Some(code_bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                "gi_frame" => {
                    if generator_closed(obj_ptr) {
                        return Some(MoltObject::none().bits());
                    }
                    let lasti = if generator_started(obj_ptr) { 0 } else { -1 };
                    return Some(suspended_frame_bits(_py, obj_ptr, lasti));
                }
                "gi_yieldfrom" => {
                    if generator_closed(obj_ptr) {
                        return Some(MoltObject::none().bits());
                    }
                    let bits = generator_yieldfrom_bits(obj_ptr);
                    if !obj_from_bits(bits).is_none() {
                        inc_ref_bits(_py, bits);
                    }
                    return Some(bits);
                }
                _ => {}
            }
            if let Some(func_bits) = generator_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0
            && !crate::object::heap_kind_has_class_shape(type_id)
            && type_id != TYPE_ID_DATACLASS
            && type_id != TYPE_ID_EXCEPTION
            && type_id != TYPE_ID_FUNCTION
            && type_id != TYPE_ID_TYPE
        {
            let result = classed_attr_lookup_without_dict(_py, obj_ptr, class_bits, attr_bits);
            if result.is_some() || exception_pending(_py) {
                return result;
            }
        }
        if type_id == TYPE_ID_ASYNC_GENERATOR
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            match name.as_str() {
                "ag_running" => {
                    let gen_bits = asyncgen_gen_bits(obj_ptr);
                    let gen_running = if let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) {
                        object_type_id(gen_ptr) == TYPE_ID_GENERATOR && generator_running(gen_ptr)
                    } else {
                        false
                    };
                    let running = asyncgen_running(obj_ptr) || gen_running;
                    return Some(MoltObject::from_bool(running).bits());
                }
                "ag_await" => {
                    let await_bits = asyncgen_await_bits(_py, obj_ptr);
                    return Some(await_bits);
                }
                "ag_code" => {
                    let code_bits = asyncgen_code_bits(_py, obj_ptr);
                    return Some(code_bits);
                }
                "ag_frame" => {
                    let gen_bits = asyncgen_gen_bits(obj_ptr);
                    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
                        return Some(MoltObject::none().bits());
                    };
                    if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                        return Some(MoltObject::none().bits());
                    }
                    if generator_closed(gen_ptr) {
                        return Some(MoltObject::none().bits());
                    }
                    let lasti = if generator_started(gen_ptr) { 0 } else { -1 };
                    return Some(suspended_frame_bits(_py, gen_ptr, lasti));
                }
                _ => {}
            }
            if let Some(func_bits) = asyncgen_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_MEMORYVIEW {
            let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
            match name.as_str() {
                "_from_flags" => {
                    let func_bits = builtin_func_bits(
                        _py,
                        &attributes_state(_py).memoryview_from_flags,
                        fn_addr!(molt_memoryview_from_flags),
                        2,
                    );
                    inc_ref_bits(_py, func_bits);
                    return Some(func_bits);
                }
                "format" => {
                    if memoryview_released(obj_ptr) {
                        return Some(raise_released_memoryview(_py));
                    }
                    let bits = memoryview_format_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "itemsize" => {
                    if memoryview_released(obj_ptr) {
                        return Some(raise_released_memoryview(_py));
                    }
                    return Some(MoltObject::from_int(memoryview_itemsize(obj_ptr) as i64).bits());
                }
                "ndim" => {
                    if memoryview_released(obj_ptr) {
                        return Some(raise_released_memoryview(_py));
                    }
                    return Some(MoltObject::from_int(memoryview_ndim(obj_ptr) as i64).bits());
                }
                "shape" => {
                    if memoryview_released(obj_ptr) {
                        return Some(raise_released_memoryview(_py));
                    }
                    let shape = memoryview_shape(obj_ptr).unwrap_or(&[]);
                    return Some(tuple_from_isize_slice(_py, shape));
                }
                "strides" => {
                    if memoryview_released(obj_ptr) {
                        return Some(raise_released_memoryview(_py));
                    }
                    let strides = memoryview_strides(obj_ptr).unwrap_or(&[]);
                    return Some(tuple_from_isize_slice(_py, strides));
                }
                "readonly" => {
                    if memoryview_released(obj_ptr) {
                        return Some(raise_released_memoryview(_py));
                    }
                    return Some(MoltObject::from_bool(memoryview_readonly(obj_ptr)).bits());
                }
                "nbytes" => {
                    if memoryview_released(obj_ptr) {
                        return Some(raise_released_memoryview(_py));
                    }
                    return Some(MoltObject::from_int(memoryview_nbytes(obj_ptr) as i64).bits());
                }
                _ => {}
            }
            if let Some(func_bits) = memoryview_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_RANGE
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            match name.as_str() {
                "start" => {
                    let bits = range_start_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "stop" => {
                    let bits = range_stop_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "step" => {
                    let bits = range_step_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                _ => {}
            }
            if let Some(func_bits) = range_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_SLICE
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            match name.as_str() {
                "start" => {
                    let bits = slice_start_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "stop" => {
                    let bits = slice_stop_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "step" => {
                    let bits = slice_step_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                _ => {}
            }
            if let Some(func_bits) = slice_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_GENERIC_ALIAS
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            match name.as_str() {
                "__origin__" => {
                    let bits = generic_alias_origin_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__args__" => {
                    let bits = generic_alias_args_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__parameters__" => {
                    let args_bits = generic_alias_args_bits(obj_ptr);
                    let mut params: Vec<u64> = Vec::new();
                    if let Some(args_ptr) = obj_from_bits(args_bits).as_ptr()
                        && object_type_id(args_ptr) == TYPE_ID_TUPLE
                    {
                        let Some(args) = crate::object::seq_access::snapshot(
                            _py,
                            args_ptr,
                            "sequence snapshot allocation failed",
                        ) else {
                            return Some(MoltObject::none().bits());
                        };
                        for &arg_bits in args.iter() {
                            if !is_typing_param(_py, arg_bits) {
                                continue;
                            }
                            if params.contains(&arg_bits) {
                                continue;
                            }
                            params.push(arg_bits);
                        }
                    }
                    let tuple_ptr = alloc_tuple(_py, &params);
                    if tuple_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "__unpacked__" => {
                    return Some(MoltObject::from_bool(false).bits());
                }
                "__mro_entries__" => {
                    let func_bits = builtin_func_bits(
                        _py,
                        &attributes_state(_py).generic_alias_mro_entries,
                        fn_addr!(molt_generic_alias_mro_entries),
                        2,
                    );
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    return Some(molt_bound_method_new(func_bits, self_bits));
                }
                _ => {}
            }
        }
        if type_id == TYPE_ID_UNION
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            match name.as_str() {
                "__origin__" => {
                    let bits = builtin_classes(_py).union_type;
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__args__" => {
                    let bits = union_type_args_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                _ => {}
            }
        }
        if type_id == TYPE_ID_FILE_HANDLE
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            let handle_ptr = file_handle_ptr(obj_ptr);
            if handle_ptr.is_null() {
                return None;
            }
            let handle = &*handle_ptr;
            match name.as_str() {
                "__class__" => {
                    let class_bits = object_class_bits(obj_ptr);
                    let class_bits = if class_bits != 0 {
                        class_bits
                    } else {
                        builtin_classes(_py).file
                    };
                    inc_ref_bits(_py, class_bits);
                    return Some(class_bits);
                }
                "closed" => {
                    if handle.detached {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            file_handle_detached_message(handle),
                        );
                    }
                    return Some(MoltObject::from_bool(file_handle_is_closed(handle)).bits());
                }
                "name" => {
                    if handle.detached {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            file_handle_detached_message(handle),
                        );
                    }
                    if handle.name_bits != 0 {
                        inc_ref_bits(_py, handle.name_bits);
                        return Some(handle.name_bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                "mode" => {
                    if handle.detached && !handle.text {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            file_handle_detached_message(handle),
                        );
                    }
                    let ptr = alloc_string(_py, handle.mode.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "encoding" => {
                    if !handle.text {
                        return None;
                    }
                    let Some(encoding) = handle.encoding.as_deref() else {
                        return Some(MoltObject::none().bits());
                    };
                    let ptr = alloc_string(_py, encoding.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "errors" => {
                    if !handle.text {
                        return None;
                    }
                    let Some(errors) = handle.errors.as_deref() else {
                        return Some(MoltObject::none().bits());
                    };
                    let ptr = alloc_string(_py, errors.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "newline" => {
                    if !handle.text {
                        return None;
                    }
                    let Some(newline) = handle.newline.as_deref() else {
                        return Some(MoltObject::none().bits());
                    };
                    let ptr = alloc_string(_py, newline.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "newlines" => {
                    if !handle.text {
                        return None;
                    }
                    if handle.newlines_len == 0 {
                        return Some(MoltObject::none().bits());
                    }
                    let mut out_bits: Vec<u64> = Vec::new();
                    for idx in 0..handle.newlines_len {
                        let kind = handle.newlines_seen[idx as usize];
                        let text = match kind {
                            NEWLINE_KIND_LF => "\n",
                            NEWLINE_KIND_CR => "\r",
                            NEWLINE_KIND_CRLF => "\r\n",
                            _ => "\n",
                        };
                        let ptr = alloc_string(_py, text.as_bytes());
                        if ptr.is_null() {
                            for bits in out_bits {
                                dec_ref_bits(_py, bits);
                            }
                            return Some(MoltObject::none().bits());
                        }
                        out_bits.push(MoltObject::from_ptr(ptr).bits());
                    }
                    if out_bits.len() == 1 {
                        return Some(out_bits[0]);
                    }
                    let tuple_ptr = alloc_tuple(_py, out_bits.as_slice());
                    if tuple_ptr.is_null() {
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return Some(MoltObject::none().bits());
                    }
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "line_buffering" => {
                    return Some(MoltObject::from_bool(handle.line_buffering).bits());
                }
                "write_through" => {
                    if !handle.text {
                        return None;
                    }
                    return Some(MoltObject::from_bool(handle.write_through).bits());
                }
                "buffer" => {
                    if !handle.text {
                        return None;
                    }
                    if handle.detached {
                        return Some(MoltObject::none().bits());
                    }
                    let buffer_bits = handle.buffer_bits;
                    if buffer_bits == 0 || buffer_bits == MoltObject::none().bits() {
                        return Some(MoltObject::none().bits());
                    }
                    inc_ref_bits(_py, buffer_bits);
                    return Some(buffer_bits);
                }
                "closefd" => {
                    let builtins = builtin_classes(_py);
                    if object_class_bits(obj_ptr) != builtins.file_io {
                        return None;
                    }
                    if handle.detached {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            file_handle_detached_message(handle),
                        );
                    }
                    return Some(MoltObject::from_bool(handle.closefd).bits());
                }
                _ => {}
            }
            if handle.text && (name == "readinto" || name == "readinto1") {
                return None;
            }
            if !handle.text && name == "reconfigure" {
                return None;
            }
            if let Some(func_bits) = file_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_DICT {
            let class_bits = object_class_bits(obj_ptr);
            let builtins = builtin_classes(_py);
            if class_bits != 0 && class_bits != builtins.dict {
                return object_attr_lookup_raw(_py, obj_ptr, attr_bits);
            }
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if name == "fromkeys"
                    && let Some(func_bits) = dict_method_bits(_py, name.as_str())
                {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let class_bits = type_of_bits(_py, self_bits);
                    let bound_bits = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound_bits);
                }
                if let Some(func_bits) = dict_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_SET
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
            && let Some(func_bits) = set_method_bits(_py, name.as_str())
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            let bound_bits = molt_bound_method_new(func_bits, self_bits);
            return Some(bound_bits);
        }
        if type_id == TYPE_ID_FROZENSET
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
            && let Some(func_bits) = frozenset_method_bits(_py, name.as_str())
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            let bound_bits = molt_bound_method_new(func_bits, self_bits);
            return Some(bound_bits);
        }
        if type_id == TYPE_ID_LIST
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
            && let Some(func_bits) = list_method_bits(_py, name.as_str())
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            let bound_bits = molt_bound_method_new(func_bits, self_bits);
            return Some(bound_bits);
        }
        if type_id == TYPE_ID_TUPLE
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
            && let Some(func_bits) = tuple_method_bits(_py, name.as_str())
        {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            let bound_bits = molt_bound_method_new(func_bits, self_bits);
            return Some(bound_bits);
        }
        if type_id == TYPE_ID_STRING
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            if name == "maketrans"
                && let Some(func_bits) = string_method_bits(_py, name.as_str())
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            if let Some(func_bits) = string_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_BYTES
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            if name == "fromhex" {
                let builtins = builtin_classes(_py);
                let func_bits = builtin_func_bits(
                    _py,
                    &attributes_state(_py).bytes_fromhex,
                    fn_addr!(molt_bytes_fromhex),
                    2,
                );
                let bound = molt_bound_method_new(func_bits, builtins.bytes);
                return Some(bound);
            }
            if name == "maketrans"
                && let Some(func_bits) = bytes_method_bits(_py, name.as_str())
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            if let Some(func_bits) = bytes_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_BYTEARRAY
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            if name == "fromhex" {
                let builtins = builtin_classes(_py);
                let func_bits = builtin_func_bits(
                    _py,
                    &attributes_state(_py).bytearray_fromhex,
                    fn_addr!(molt_bytearray_fromhex),
                    2,
                );
                let bound = molt_bound_method_new(func_bits, builtins.bytearray);
                return Some(bound);
            }
            if name == "maketrans"
                && let Some(func_bits) = bytearray_method_bits(_py, name.as_str())
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            if let Some(func_bits) = bytearray_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_COMPLEX
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            if name == "real" {
                let value = *complex_ref(obj_ptr);
                return Some(MoltObject::from_float(value.re).bits());
            }
            if name == "imag" {
                let value = *complex_ref(obj_ptr);
                return Some(MoltObject::from_float(value.im).bits());
            }
            if let Some(func_bits) = complex_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_TYPE {
            return type_attr_lookup_ptr_default(_py, obj_ptr, attr_bits);
        }
        if type_id == TYPE_ID_SUPER {
            let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
            let start_bits = super_type_bits(obj_ptr);
            let target_bits = super_obj_bits(obj_ptr);
            let obj_type_bits = crate::object::layout::super_receiver_class_bits(obj_ptr);
            let member = match attr_name.as_deref() {
                Some("__thisclass__") => Some(start_bits),
                Some("__self__") => Some(target_bits),
                Some("__self_class__") => Some(obj_type_bits),
                Some("__class__") => Some(builtin_classes(_py).super_type),
                _ => None,
            };
            if let Some(bits) = member {
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            if obj_from_bits(obj_type_bits).is_none() {
                return None;
            }
            let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
            if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
                return None;
            }
            let mro_storage = class_mro_view(_py, obj_type_ptr);
            // A class receiver is unbound for descriptors; a metaclass receiver
            // remains an instance of its resolved metaclass.
            let instance_bits = if target_bits == obj_type_bits {
                None
            } else {
                Some(target_bits)
            };
            let owner_ptr = obj_type_ptr;
            let mut found_start = false;
            for class_bits in mro_storage.iter() {
                if !found_start {
                    if *class_bits == start_bits {
                        found_start = true;
                    }
                    continue;
                }
                let class_obj = obj_from_bits(*class_bits);
                let Some(class_ptr) = class_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    continue;
                }
                let dict_bits = class_dict_bits(class_ptr);
                let dict_obj = obj_from_bits(dict_bits);
                let Some(dict_ptr) = dict_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    continue;
                }
                if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                    if attr_name.as_deref() == Some("__new__")
                        && let Some(val_ptr) = obj_from_bits(val_bits).as_ptr()
                        && object_type_id(val_ptr) == TYPE_ID_FUNCTION
                    {
                        inc_ref_bits(_py, val_bits);
                        return Some(val_bits);
                    }
                    return descriptor_bind(
                        _py,
                        val_bits,
                        Some(MoltObject::from_ptr(owner_ptr).bits()),
                        instance_bits,
                    );
                }
                if let Some(name) = attr_name.as_deref()
                    && is_builtin_class_bits(_py, *class_bits)
                    && let Some(func_bits) = builtin_class_method_bits(_py, *class_bits, name)
                {
                    if name == "__new__" {
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                    return descriptor_bind(
                        _py,
                        func_bits,
                        Some(MoltObject::from_ptr(owner_ptr).bits()),
                        instance_bits,
                    );
                }
            }
            return None;
        }
        if type_id == TYPE_ID_FUNCTION {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if name == "__get__" {
                    // CPython parity: all function objects (including
                    // builtin_function_or_method) expose __get__ for the
                    // descriptor protocol.  f.__get__(instance, owner) returns
                    // a bound method binding f to instance, or f itself when
                    // instance is None.
                    let none = MoltObject::none().bits();
                    let func_bits = crate::builtins::methods::builtin_func_bits_with_defaults_tuple(
                        _py,
                        &runtime_state(_py).method_cache.function_descriptor_get,
                        fn_addr!(molt_function_descriptor_get),
                        3, // (self, instance, owner)
                        &[none],
                    );
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    return Some(molt_bound_method_new(func_bits, self_bits));
                }
                if name == "__code__" {
                    // CPython parity: builtin_function_or_method objects do not expose __code__.
                    if builtin_classes(_py).is_builtin_callable_class(object_class_bits(obj_ptr)) {
                        return None;
                    }
                    let code_bits = ensure_function_code_bits(_py, obj_ptr);
                    if !obj_from_bits(code_bits).is_none() {
                        inc_ref_bits(_py, code_bits);
                        return Some(code_bits);
                    }
                    return None;
                }
                if name == "__text_signature__" {
                    // CPython parity: builtin_function_or_method objects expose a read-only
                    // `__text_signature__` string used by `inspect.signature`.
                    if builtin_classes(_py).is_builtin_callable_class(object_class_bits(obj_ptr)) {
                        let fn_ptr = function_fn_ptr(obj_ptr);
                        let text_sig = match fn_ptr {
                            v if v == fn_addr!(molt_abs_builtin) => Some("(x, /)"),
                            v if v == fn_addr!(molt_aiter) => Some("(async_iterable, /)"),
                            v if v == fn_addr!(molt_all_builtin) => Some("(iterable, /)"),
                            v if v == fn_addr!(molt_any_builtin) => Some("(iterable, /)"),
                            v if v == fn_addr!(molt_ascii_from_obj) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_bin_builtin) => Some("(number, /)"),
                            v if v == fn_addr!(molt_callable_builtin) => Some("(obj, /)"),
                            v if v == fn_addr!(crate::object::ops::molt_chr) => Some("(i, /)"),
                            v if v == fn_addr!(molt_del_attr_name) => Some("(obj, name, /)"),
                            v if v == fn_addr!(molt_divmod_builtin) => Some("(x, y, /)"),
                            v if v == fn_addr!(molt_format_builtin) => {
                                Some("(value, format_spec='', /)")
                            }
                            v if v == fn_addr!(molt_has_attr_name) => Some("(obj, name, /)"),
                            v if v == fn_addr!(molt_hash_builtin) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_hex_builtin) => Some("(number, /)"),
                            v if v == fn_addr!(molt_id) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_isinstance) => Some("(obj, class_or_tuple, /)"),
                            v if v == fn_addr!(molt_issubclass) => Some("(cls, class_or_tuple, /)"),
                            v if v == fn_addr!(molt_len) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_open_builtin) => Some(
                                "(file, mode='r', buffering=-1, encoding=None, errors=None, newline=None, closefd=True, opener=None)",
                            ),
                            v if v == fn_addr!(molt_oct_builtin) => Some("(number, /)"),
                            v if v == fn_addr!(crate::object::ops_sys::molt_ord) => Some("(c, /)"),
                            v if v == fn_addr!(molt_pow) => Some("(base, exp, mod=None)"),
                            v if v == fn_addr!(molt_print_builtin) => {
                                Some("(*args, sep=' ', end='\\n', file=None, flush=False)")
                            }
                            v if v == fn_addr!(molt_repr_builtin) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_round_builtin) => {
                                Some("(number, ndigits=None)")
                            }
                            v if v == fn_addr!(molt_set_attr_name) => Some("(obj, name, value, /)"),
                            v if v == fn_addr!(molt_sorted_builtin) => {
                                Some("(iterable, /, *, key=None, reverse=False)")
                            }
                            v if v == fn_addr!(molt_sum_builtin) => Some("(iterable, /, start=0)"),
                            _ => None,
                        };
                        if let Some(text_sig) = text_sig {
                            let ptr = alloc_string(_py, text_sig.as_bytes());
                            if ptr.is_null() {
                                return None;
                            }
                            return Some(MoltObject::from_ptr(ptr).bits());
                        }
                    }
                }
                if name == "__closure__" {
                    if builtin_classes(_py).is_builtin_callable_class(object_class_bits(obj_ptr)) {
                        return None;
                    }
                    if function_call_abi(obj_ptr) == FunctionCallAbi::OpaqueContextFirst {
                        return Some(MoltObject::none().bits());
                    }
                    let closure_bits = function_closure_bits(obj_ptr);
                    if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
                        inc_ref_bits(_py, closure_bits);
                        return Some(closure_bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                if name == "__module__" {
                    if let Some(result) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .cfunction_module(MoltObject::from_ptr(obj_ptr).bits())
                    {
                        return match result {
                            Ok(bits) => Some(bits),
                            Err(()) => {
                                crate::cpython_abi_hooks::transfer_pending_cpython_exception();
                                None
                            }
                        };
                    }
                    // `__module__` is writable on CPython builtin_function_or_method objects.
                    // Ensure attribute reads consult the per-function dict rather than falling
                    // back to the type's own `__module__` (which is always "builtins").
                    let dict_bits = function_dict_bits(obj_ptr);
                    if dict_bits != 0
                        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                        && let Some(module_key_bits) = attr_name_bits_from_bytes(_py, b"__module__")
                    {
                        let value = dict_get_in_place(_py, dict_ptr, module_key_bits);
                        dec_ref_bits(_py, module_key_bits);
                        if let Some(bits) = value {
                            inc_ref_bits(_py, bits);
                            return Some(bits);
                        }
                    }
                }
            }
            let annotate_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotate_name,
                b"__annotate__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(annotate_name_bits),
            ) && pep649_enabled(_py)
            {
                let mut annotate_bits = function_annotate_bits(obj_ptr);
                if annotate_bits == 0 {
                    annotate_bits = MoltObject::none().bits();
                }
                inc_ref_bits(_py, annotate_bits);
                return Some(annotate_bits);
            }
            let annotations_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotations_name,
                b"__annotations__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(annotations_bits),
            ) {
                let cached = function_annotations_bits(obj_ptr);
                if cached != 0 {
                    inc_ref_bits(_py, cached);
                    return Some(cached);
                }
                let annotate_bits = function_annotate_bits(obj_ptr);
                let res_bits = if pep649_enabled(_py)
                    && annotate_bits != 0
                    && !obj_from_bits(annotate_bits).is_none()
                {
                    let format_bits = MoltObject::from_int(1).bits();
                    let res_bits = call_callable1(_py, annotate_bits, format_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    let Some(res_ptr) = res_obj.as_ptr() else {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    };
                    if object_type_id(res_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    res_bits
                } else {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                };
                function_set_annotations_bits(_py, obj_ptr, res_bits);
                return Some(res_bits);
            }
            let dict_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
                let dict_ptr = crate::call::class_init::function_ensure_dict(_py, obj_ptr)?;
                let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            let dict_bits = function_dict_bits(obj_ptr);
            if dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits)
            {
                inc_ref_bits(_py, val);
                return Some(val);
            }
            // Fall through to the function type for descriptor-backed attributes
            // such as function.__get__ and function.__repr__.
            let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
            {
                if let Some(bound) = descriptor_bind(
                    _py,
                    val_bits,
                    Some(MoltObject::from_ptr(class_ptr).bits()),
                    Some(obj_bits),
                ) {
                    return Some(bound);
                }
                if exception_pending(_py) {
                    return None;
                }
            }
            return None;
        }
        if type_id == crate::TYPE_ID_CELL {
            return classed_attr_lookup_without_dict(
                _py,
                obj_ptr,
                crate::builtins::types::cell_class(_py),
                attr_bits,
            );
        }
        if type_id == TYPE_ID_CODE {
            // Keep basic CPython-compatible code metadata available for inspect/types consumers.
            let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
            match name.as_str() {
                "co_filename" => {
                    let bits = code_filename_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "co_name" => {
                    let bits = code_name_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "co_firstlineno" => {
                    return Some(MoltObject::from_int(code_firstlineno(obj_ptr)).bits());
                }
                "co_linetable" => {
                    let bits = code_linetable_bits(obj_ptr);
                    if bits != 0 {
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                "co_varnames" => {
                    let bits = code_varnames_bits(obj_ptr);
                    if bits != 0 {
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    let tuple_ptr = alloc_tuple(_py, &[]);
                    if tuple_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "co_names" => {
                    let bits = code_names_bits(obj_ptr);
                    if bits != 0 {
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    let tuple_ptr = alloc_tuple(_py, &[]);
                    if tuple_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "co_freevars" => {
                    let bits = code_freevars_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "co_cellvars" => {
                    let bits = code_cellvars_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "co_argcount" => {
                    return Some(MoltObject::from_int(code_argcount(obj_ptr) as i64).bits());
                }
                "co_posonlyargcount" => {
                    return Some(MoltObject::from_int(code_posonlyargcount(obj_ptr) as i64).bits());
                }
                "co_kwonlyargcount" => {
                    return Some(MoltObject::from_int(code_kwonlyargcount(obj_ptr) as i64).bits());
                }
                "co_nlocals" => {
                    let bits = code_varnames_bits(obj_ptr);
                    if let Some(ptr) = obj_from_bits(bits).as_ptr()
                        && object_type_id(ptr) == TYPE_ID_TUPLE
                    {
                        return Some(MoltObject::from_int(tuple_len(ptr) as i64).bits());
                    }
                    return Some(MoltObject::from_int(0).bits());
                }
                "co_flags" => {
                    return Some(
                        MoltObject::from_int(crate::object::layout::code_flags(obj_ptr) as i64)
                            .bits(),
                    );
                }
                "co_consts" => {
                    let name_bits = code_name_bits(obj_ptr);
                    let is_module = string_obj_to_owned(obj_from_bits(name_bits))
                        .is_some_and(|value| value == "<module>");
                    let elems: [u64; 2] =
                        [MoltObject::none().bits(), MoltObject::from_int(0).bits()];
                    let ptr = if is_module {
                        alloc_tuple(_py, &elems)
                    } else {
                        alloc_tuple(_py, &[])
                    };
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "co_positions" => {
                    let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                        _py,
                        crate::molt_code_positions as *const () as usize as u64,
                        1,
                    );
                    if func_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    let func_bits = MoltObject::from_ptr(func_ptr).bits();
                    let _ = crate::molt_function_set_builtin(func_bits);
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
                    dec_ref_bits(_py, func_bits);
                    if bound_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(bound_ptr).bits());
                }
                _ => {}
            }
            return None;
        }
        if type_id == TYPE_ID_DATACLASS {
            return crate::builtins::attr::dataclass_attr_lookup_raw(_py, obj_ptr, attr_bits);
        }
        if crate::object::heap_kind_has_class_shape(type_id) {
            return classed_default_attr_lookup(_py, obj_ptr, attr_bits);
        }
        None
    }
}

/// Synthetic coroutine members precede ordinary object storage on the default
/// path. All descriptor/slot/dictionary precedence is owned by the same raw
/// object lookup that implements explicit object.__getattribute__.
unsafe fn classed_default_attr_lookup(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).load_metadata_flags() & HEADER_FLAG_COROUTINE != 0
            && let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits))
        {
            match name.as_str() {
                "cr_running" => {
                    let running =
                        ((*header).load_synchronized_flags() & HEADER_FLAG_TASK_RUNNING) != 0;
                    return Some(MoltObject::from_bool(running).bits());
                }
                "cr_frame" => {
                    if crate::object::object_poll_fn(obj_ptr) == 0
                        || ((*header).load_synchronized_flags() & HEADER_FLAG_TASK_DONE) != 0
                    {
                        return Some(MoltObject::none().bits());
                    }
                    let lasti = if crate::object::object_state(obj_ptr) == 0 {
                        -1
                    } else {
                        0
                    };
                    return Some(suspended_frame_bits(_py, obj_ptr, lasti));
                }
                "cr_code" => {
                    let code_bits = crate::object::aux_header::object_frame_code_bits(obj_ptr);
                    if code_bits != 0 {
                        inc_ref_bits(_py, code_bits);
                        return Some(code_bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                "cr_await" => {
                    let awaited = {
                        let guard = task_waiting_on(_py).lock().unwrap();
                        guard.get(&PtrSlot(obj_ptr)).copied()
                    };
                    if let Some(waiting_on) = awaited {
                        let bits = MoltObject::from_ptr(waiting_on.0).bits();
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                _ => {}
            }
            if let Some(func_bits) = coroutine_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }

        object_attr_lookup_raw(_py, obj_ptr, attr_bits)
    }
}

/// Consume the owned result of a successful attribute lookup when the caller
/// needs only existence.  Both pointer and scalar attribute resolvers return a
/// new owned reference on success (including freshly bound methods and values
/// produced by descriptors); `hasattr` must discard that value immediately.
#[inline]
fn discard_owned_attr_result(_py: &PyToken<'_>, result: Option<u64>) -> bool {
    let Some(attr_bits) = result else {
        return false;
    };
    dec_ref_bits(_py, attr_bits);
    true
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
            };
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let type_id = object_type_id(obj_ptr);
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits();
            };
            let found = attr_lookup_ptr(_py, obj_ptr, attr_bits);
            dec_ref_bits(_py, attr_bits);
            if let Some(val) = found {
                return val;
            }
            if exception_pending(_py) {
                let exc_bits = molt_exception_last_pending();
                molt_exception_clear();
                let _ = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(obj_ptr);
                if !desc_ptr.is_null() && (*desc_ptr).slots && !(*desc_ptr).allows_dict {
                    let name = &(*desc_ptr).name;
                    let type_label = if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    };
                    return attr_error_with_obj(
                        _py,
                        type_label,
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                let type_label = if !desc_ptr.is_null() {
                    let name = &(*desc_ptr).name;
                    if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    }
                } else {
                    "dataclass"
                };
                return attr_error(_py, type_label, attr_name);
            }
            if type_id == TYPE_ID_TYPE {
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                let res = attr_error_with_message(_py, &msg);
                let exc_bits = molt_exception_last_pending();
                if !obj_from_bits(exc_bits).is_none() {
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                }
                return res;
            }
            attr_error_with_obj(
                _py,
                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            )
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            molt_get_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
            };
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
                if object_type_id(ptr) == TYPE_ID_TYPE {
                    let class_bits = MoltObject::from_ptr(ptr).bits();
                    if is_builtin_class_bits(_py, class_bits)
                        && matches!(
                            attr_name,
                            "__getattribute__" | "__setattr__" | "__delattr__"
                        )
                        && let Some(func_bits) =
                            builtin_class_method_bits(_py, class_bits, attr_name)
                        && let Some(bits) = descriptor_bind(
                            _py,
                            func_bits,
                            Some(MoltObject::from_ptr(ptr).bits()),
                            None,
                        )
                    {
                        return bits;
                    }
                }
                return molt_get_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
            }
            if let Some(val) = resolve_scalar_attr(_py, obj_bits, attr_name) {
                return val;
            }
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_object_ic(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    site_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(site_id) = ic_site_from_bits(site_bits) else {
                return molt_get_attr_object(obj_bits, attr_name_ptr, attr_name_len_bits);
            };
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
            };
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);

            let Some(name_bits) = attr_name_bits_for_site(_py, site_id, slice) else {
                return MoltObject::none().bits();
            };
            let out = molt_get_attr_name(obj_bits, name_bits);
            dec_ref_bits(_py, name_bits);
            out
        })
    }
}

/// # Safety
/// Dereferences `attr_name_ptr`. Caller must ensure it points to valid UTF-8
/// of length `attr_name_len_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_special(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
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
                } else {
                    None
                }
            } else {
                None
            };
            dec_ref_bits(_py, name_bits);
            if let Some(bits) = res {
                return bits;
            }
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

/// Implements the descriptor protocol `__get__` for function objects.
///
/// CPython semantics: `func.__get__(instance, owner=None)` returns a bound
/// method when `instance` is not `None`, or the function itself otherwise.
/// The `owner` argument is accepted but unused (CPython ignores it for
/// regular functions and builtins).
#[unsafe(no_mangle)]
pub extern "C" fn molt_function_descriptor_get(
    self_bits: u64,
    instance_bits: u64,
    _owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let instance_obj = obj_from_bits(instance_bits);
        if instance_obj.is_none() {
            // Unbound access: return the function itself.
            inc_ref_bits(_py, self_bits);
            return self_bits;
        }
        // Bound access: return a bound method wrapping self bound to instance.
        molt_bound_method_new(self_bits, instance_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        if exception_pending(_py) {
            // Preserve any pre-existing exception; callers should unwind.
            return MoltObject::none().bits();
        }
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                // Foreign (C-extension) objects are handled inside the shared
                // `attr_lookup_ptr` (below) so every getattr entry point routes
                // them uniformly — do NOT special-case here.
                if let Some(val) = attr_lookup_ptr(_py, obj_ptr, name_bits) {
                    return val;
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_DATACLASS {
                    let desc_ptr = dataclass_desc_ptr(obj_ptr);
                    if !desc_ptr.is_null() && (*desc_ptr).slots && !(*desc_ptr).allows_dict {
                        let name = &(*desc_ptr).name;
                        let type_label = if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        };
                        return attr_error_with_obj(_py, type_label, &attr_name, obj_bits);
                    }
                    let type_label = if !desc_ptr.is_null() {
                        let name = &(*desc_ptr).name;
                        if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        }
                    } else {
                        "dataclass"
                    };
                    return attr_error_with_obj(_py, type_label, &attr_name, obj_bits);
                }
                if type_id == TYPE_ID_TYPE {
                    let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                    return attr_error_with_obj_message(_py, &msg, &attr_name, obj_bits);
                }
                return attr_error_with_obj(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    &attr_name,
                    obj_bits,
                );
            }
            if let Some(val) = resolve_scalar_attr(_py, obj_bits, &attr_name) {
                return val;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_get_attr_name_default(
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if is_missing_bits(_py, default_bits) {
            return molt_get_attr_name(obj_bits, name_bits);
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if let Some(val) = attr_lookup_ptr_allow_missing(_py, obj_ptr, name_bits) {
                    if matches!(
                        std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                        Some("1")
                    ) && string_obj_to_owned(obj_from_bits(name_bits)).as_deref()
                        == Some("__init_subclass__")
                    {
                        let val_obj = obj_from_bits(val);
                        eprintln!(
                            "molt init_subclass found val_bits=0x{:x} none={} ptr={}",
                            val,
                            val_obj.is_none(),
                            val_obj.as_ptr().is_some(),
                        );
                    }
                    return val;
                }
                if exception_pending(_py) {
                    if clear_attribute_error_if_pending(_py) {
                        inc_ref_bits(_py, default_bits);
                        return default_bits;
                    }
                    return MoltObject::none().bits();
                }
                if matches!(
                    std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                    Some("1")
                ) && string_obj_to_owned(obj_from_bits(name_bits)).as_deref()
                    == Some("__init_subclass__")
                {
                    let type_id = object_type_id(obj_ptr);
                    let class_bits = if type_id == TYPE_ID_TYPE {
                        MoltObject::from_ptr(obj_ptr).bits()
                    } else {
                        object_class_bits(obj_ptr)
                    };
                    eprintln!(
                        "molt init_subclass default obj_bits=0x{:x} type_id={} class_bits=0x{:x} default_bits=0x{:x} default_is_none={}",
                        MoltObject::from_ptr(obj_ptr).bits(),
                        type_id,
                        class_bits,
                        default_bits,
                        obj_from_bits(default_bits).is_none(),
                    );
                }
                inc_ref_bits(_py, default_bits);
                return default_bits;
            }
            if let Some(val) = resolve_scalar_attr(_py, obj_bits, &attr_name) {
                return val;
            }
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_has_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        if exception_pending(_py) {
            let exc_bits = molt_exception_last_pending();
            if exception_is_attribute_error(_py, exc_bits) {
                clear_exception(_py);
                dec_ref_bits(_py, exc_bits);
                return MoltObject::from_bool(false).bits();
            }
            clear_exception(_py);
            let _ = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return MoltObject::from_bool(false).bits();
        }
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if discard_owned_attr_result(_py, attr_lookup_ptr(_py, obj_ptr, name_bits)) {
                    return MoltObject::from_bool(true).bits();
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last_pending();
                    if exception_is_attribute_error(_py, exc_bits) {
                        clear_exception(_py);
                        dec_ref_bits(_py, exc_bits);
                        return MoltObject::from_bool(false).bits();
                    }
                    clear_exception(_py);
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::from_bool(false).bits();
                }
                return MoltObject::from_bool(false).bits();
            }
            // Scalar receiver. Route through the same resolver `getattr` uses
            // so `hasattr` can never disagree with it; the freshly materialized
            // bound method is released immediately, mirroring CPython's
            // `hasattr` discarding the value `getattr` returns.
            if discard_owned_attr_result(_py, resolve_scalar_attr(_py, obj_bits, &attr_name)) {
                return MoltObject::from_bool(true).bits();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

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

    extern "C" fn visibility_probe() -> u64 {
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
        let namespace_bits = crate::molt_dict_new(attrs.len() as u64);
        assert!(!obj_from_bits(namespace_bits).is_none());
        for &(attr_name, value_bits) in attrs {
            let attr_bits = string_bits(_py, attr_name);
            assert_eq!(
                crate::c_api::molt_mapping_setitem(namespace_bits, attr_bits, value_bits),
                0
            );
            dec_ref_bits(_py, attr_bits);
        }
        let class_bits = crate::builtins::types::molt_type_new(
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

    extern "C" fn changing_getattribute(_self_bits: u64, _name_bits: u64) -> u64 {
        let call = CUSTOM_GETATTRIBUTE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        MoltObject::from_int(call as i64).bits()
    }

    #[test]
    fn class_attribute_errors_return_boxed_none_across_entrypoints() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let declared_name = string_bits(_py, b"BoxedAttributeErrors");
            let class_bits = crate::molt_class_new(declared_name);
            let class_ptr = MoltObject::from_bits(class_bits).as_ptr().unwrap();
            let invalid_name = MoltObject::from_int(17).bits();
            for (operation, name, expected_error, routes) in [
                ("get", "__missing_boxed_attribute__", "AttributeError", 8),
                ("set", "__name__", "TypeError", 5),
                ("del", "__missing_boxed_attribute__", "AttributeError", 4),
            ] {
                let name_bits = string_bits(_py, name.as_bytes());
                for route in 0..routes {
                    assert!(!crate::exception_pending(_py));
                    let result: u64 = unsafe {
                        match (operation, route) {
                            ("get", 0) => super::molt_get_attr_name(class_bits, name_bits),
                            ("get", 1) => super::molt_get_attr_generic(
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 2) => super::molt_get_attr_ptr(
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 3) => super::molt_get_attr_object(
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 4) => super::molt_get_attr_object_ic(
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                                MoltObject::from_int(91).bits(),
                            ),
                            ("get", 5) => super::molt_get_attr_special(
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("get", 6) => crate::molt_object_getattribute(class_bits, name_bits),
                            ("get", 7) => crate::molt_type_getattribute(class_bits, name_bits),
                            ("set", 0) => {
                                super::molt_set_attr_name(class_bits, name_bits, invalid_name)
                            }
                            ("set", 1) => super::molt_set_attr_generic(
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                                invalid_name,
                            ),
                            ("set", 2) => super::molt_set_attr_ptr(
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                                invalid_name,
                            ),
                            ("set", 3) => super::molt_set_attr_object(
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                                invalid_name,
                            ),
                            ("set", 4) => {
                                crate::molt_object_setattr(class_bits, name_bits, invalid_name)
                            }
                            ("del", 0) => super::molt_del_attr_name(class_bits, name_bits),
                            ("del", 1) => super::molt_del_attr_generic(
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("del", 2) => super::molt_del_attr_ptr(
                                class_ptr,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
                            ("del", 3) => super::molt_del_attr_object(
                                class_bits,
                                name.as_ptr(),
                                name.len() as u64,
                            ),
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
                        crate::builtins::exceptions::exception_matches_builtin_name(
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
            let actual_name = super::molt_get_attr_name(class_bits, name_key);
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
    fn gpu_attribute_bridge_errors_return_boxed_none() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name_bits = string_bits(_py, b"attribute");
            let receiver = alloc_string(_py, b"receiver");
            assert!(!receiver.is_null());
            for invalid_receiver in [true, false] {
                let result: u64 = crate::gpu_bridge::__molt_gpu_object_setattr_raw(
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
                assert_eq!(result, MoltObject::none().bits());
                assert!(crate::exception_pending(_py));
                let exception = crate::exception_last_bits_noinc(_py).unwrap();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    _py,
                    exception,
                    "TypeError",
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

            let by_name = super::molt_get_attr_name(builtin, code_name);
            assert!(MoltObject::from_bits(by_name).is_none());
            assert!(crate::exception_pending(_py));
            crate::clear_exception(_py);

            let by_generic =
                unsafe { super::molt_get_attr_generic(builtin_ptr, b"__code__".as_ptr(), 8) };
            assert!(MoltObject::from_bits(by_generic).is_none());
            assert!(crate::exception_pending(_py));
            crate::clear_exception(_py);

            let closure_name = string_bits(_py, b"__closure__");
            let closure_by_name = super::molt_get_attr_name(builtin, closure_name);
            assert!(MoltObject::from_bits(closure_by_name).is_none());
            assert!(crate::exception_pending(_py));
            crate::clear_exception(_py);

            let closure_by_generic =
                unsafe { super::molt_get_attr_generic(builtin_ptr, b"__closure__".as_ptr(), 11) };
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
            let opaque_by_name = super::molt_get_attr_name(opaque, closure_name);
            assert!(MoltObject::from_bits(opaque_by_name).is_none());
            assert!(!crate::exception_pending(_py));
            let opaque_by_generic =
                unsafe { super::molt_get_attr_generic(opaque_ptr, b"__closure__".as_ptr(), 11) };
            assert!(MoltObject::from_bits(opaque_by_generic).is_none());
            assert!(!crate::exception_pending(_py));

            let ordinary_ptr =
                crate::alloc_function_obj(_py, visibility_probe as *const () as usize as u64, 0);
            assert!(!ordinary_ptr.is_null());
            let ordinary = MoltObject::from_ptr(ordinary_ptr).bits();
            let ordinary_code = super::molt_get_attr_name(ordinary, code_name);
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

    fn assert_callable_attr(_py: &PyToken<'_>, bits: u64) {
        assert!(!MoltObject::from_bits(bits).is_none());
        assert!(crate::builtins::callable::is_callable_impl(_py, bits));
        dec_ref_bits(_py, bits);
    }

    fn assert_numeric_scalar_attr_entrypoints(_py: &PyToken<'_>, receiver_bits: u64, name: &[u8]) {
        let name_bits = string_bits(_py, name);
        let default_bits = string_bits(_py, b"default-sentinel");

        let name_str = std::str::from_utf8(name).unwrap();
        let resolved = super::resolve_scalar_attr(_py, receiver_bits, name_str)
            .expect("numeric scalar resolver should find attribute");
        assert_callable_attr(_py, resolved);

        let getattr_bits = super::molt_get_attr_name(receiver_bits, name_bits);
        assert_eq!(crate::molt_exception_pending(), 0);
        assert_callable_attr(_py, getattr_bits);

        let default_getattr_bits =
            super::molt_get_attr_name_default(receiver_bits, name_bits, default_bits);
        assert_eq!(crate::molt_exception_pending(), 0);
        assert_ne!(default_getattr_bits, default_bits);
        assert_callable_attr(_py, default_getattr_bits);

        let direct_bits = crate::molt_object_getattribute(receiver_bits, name_bits);
        assert_eq!(crate::molt_exception_pending(), 0);
        assert_callable_attr(_py, direct_bits);

        let has_bits = super::molt_has_attr_name(receiver_bits, name_bits);
        assert_eq!(MoltObject::from_bits(has_bits).as_bool(), Some(true));

        let missing_name_bits = string_bits(_py, b"definitely_missing_scalar_attr");
        let missing_has_bits = super::molt_has_attr_name(receiver_bits, missing_name_bits);
        assert_eq!(
            MoltObject::from_bits(missing_has_bits).as_bool(),
            Some(false)
        );

        let missing_default_bits =
            super::molt_get_attr_name_default(receiver_bits, missing_name_bits, default_bits);
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

            let has_bits = super::molt_has_attr_name(dict_bits, name_bits);

            assert_eq!(MoltObject::from_bits(has_bits).as_bool(), Some(true));
            assert_eq!(crate::molt_exception_pending(), 0);
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
            let class_bits = crate::molt_class_new(declared_name);
            let class_ptr = MoltObject::from_bits(class_bits)
                .as_ptr()
                .expect("class allocation");
            let name_key = string_bits(_py, b"__name__");
            let shadow_name = string_bits(_py, b"ShadowName");
            let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
            let dict_ptr = MoltObject::from_bits(dict_bits)
                .as_ptr()
                .expect("class namespace");
            unsafe { crate::dict_set_in_place(_py, dict_ptr, name_key, shadow_name) };

            let resolved = super::molt_get_attr_name(class_bits, name_key);
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
                super::molt_set_attr_name(
                    instance_bits,
                    first_name,
                    MoltObject::from_int(11).bits()
                ),
                MoltObject::none().bits()
            );
            assert_eq!(
                super::molt_set_attr_name(
                    instance_bits,
                    second_name,
                    MoltObject::from_int(22).bits(),
                ),
                MoltObject::none().bits()
            );
            let site = MoltObject::from_int(700).bits();
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic(instance_bits, b"first".as_ptr(), 5, site)
                },
                MoltObject::from_int(11).bits()
            );
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic(instance_bits, b"second".as_ptr(), 6, site)
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
                super::molt_set_attr_name(first, name_bits, MoltObject::from_int(31).bits()),
                MoltObject::none().bits()
            );
            assert_eq!(
                super::molt_set_attr_name(second, name_bits, MoltObject::from_int(32).bits()),
                MoltObject::none().bits()
            );
            let site = MoltObject::from_int(701).bits();
            assert_eq!(
                unsafe { super::molt_get_attr_object_ic(first, b"value".as_ptr(), 5, site) },
                MoltObject::from_int(31).bits()
            );
            assert_eq!(
                unsafe { super::molt_get_attr_object_ic(second, b"value".as_ptr(), 5, site) },
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
                    super::molt_get_attr_object_ic(instance_bits, b"probe".as_ptr(), 5, site)
                },
                MoltObject::from_int(1).bits()
            );
            assert_eq!(
                unsafe {
                    super::molt_get_attr_object_ic(instance_bits, b"probe".as_ptr(), 5, site)
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
}
