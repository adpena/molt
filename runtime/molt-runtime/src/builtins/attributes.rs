use crate::PyToken;
use molt_obj_model::MoltObject;
use std::borrow::Cow;

use crate::*;

#[no_mangle]
pub(crate) unsafe fn attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    profile_hit(_py, &ATTR_LOOKUP_COUNT);
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        return module_attr_lookup(_py, obj_ptr, attr_bits);
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
                    if let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() {
                        if object_type_id(func_ptr) == TYPE_ID_FUNCTION {
                            if let Some(bits) = function_attr_bits(_py, func_ptr, attr_bits) {
                                inc_ref_bits(_py, bits);
                                return Some(bits);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if type_id == TYPE_ID_EXCEPTION {
        let name = string_obj_to_owned(obj_from_bits(attr_bits));
        let attr_name = name.as_deref()?;
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
                let bits = exception_trace_bits(obj_ptr);
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            "__class__" => {
                let mut class_bits = exception_class_bits(obj_ptr);
                if obj_from_bits(class_bits).is_none() || class_bits == 0 {
                    let new_bits = exception_type_bits(_py, exception_kind_bits(obj_ptr));
                    let slot = obj_ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != new_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, new_bits);
                        *slot = new_bits;
                    }
                    class_bits = new_bits;
                }
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
                    let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != new_bits {
                        dec_ref_bits(_py, old_bits);
                        *slot = new_bits;
                    }
                    dict_bits = new_bits;
                }
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            "args" => {
                let mut args_bits = exception_args_bits(obj_ptr);
                if obj_from_bits(args_bits).is_none() || args_bits == 0 {
                    let ptr = alloc_tuple(_py, &[]);
                    if ptr.is_null() {
                        return None;
                    }
                    let new_bits = MoltObject::from_ptr(ptr).bits();
                    let slot = obj_ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != new_bits {
                        dec_ref_bits(_py, old_bits);
                        *slot = new_bits;
                    }
                    args_bits = new_bits;
                }
                inc_ref_bits(_py, args_bits);
                return Some(args_bits);
            }
            "value" => {
                let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)));
                if kind.as_deref() == Some("StopIteration") {
                    let bits = exception_value_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
            }
            _ => {}
        }
        let dict_bits = exception_dict_bits(obj_ptr);
        if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                }
            }
        }
        let mut class_bits = exception_class_bits(obj_ptr);
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = exception_type_bits(_py, exception_kind_bits(obj_ptr));
        }
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
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
    }
    if type_id == TYPE_ID_GENERATOR {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            match name.as_str() {
                "gi_running" => {
                    return Some(MoltObject::from_bool(generator_running(obj_ptr)).bits());
                }
                "gi_frame" => {
                    if generator_closed(obj_ptr) {
                        return Some(MoltObject::none().bits());
                    }
                    let lasti = if generator_started(obj_ptr) { 0 } else { -1 };
                    let frame_bits = molt_object_new();
                    let Some(frame_ptr) = maybe_ptr_from_bits(frame_bits) else {
                        return Some(MoltObject::none().bits());
                    };
                    let name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.f_lasti_name,
                        b"f_lasti",
                    );
                    let val_bits = MoltObject::from_int(lasti).bits();
                    let dict_ptr = alloc_dict_with_pairs(_py, &[name_bits, val_bits]);
                    if !dict_ptr.is_null() {
                        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        instance_set_dict_bits(_py, frame_ptr, dict_bits);
                        object_mark_has_ptrs(_py, frame_ptr);
                    }
                    return Some(frame_bits);
                }
                _ => {}
            }
        }
    }
    if type_id == TYPE_ID_ASYNC_GENERATOR {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
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
                    let frame_bits = molt_object_new();
                    let Some(frame_ptr) = maybe_ptr_from_bits(frame_bits) else {
                        return Some(MoltObject::none().bits());
                    };
                    let name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.f_lasti_name,
                        b"f_lasti",
                    );
                    let val_bits = MoltObject::from_int(lasti).bits();
                    let dict_ptr = alloc_dict_with_pairs(_py, &[name_bits, val_bits]);
                    if !dict_ptr.is_null() {
                        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        instance_set_dict_bits(_py, frame_ptr, dict_bits);
                        object_mark_has_ptrs(_py, frame_ptr);
                    }
                    return Some(frame_bits);
                }
                _ => {}
            }
            if let Some(func_bits) = asyncgen_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_MEMORYVIEW {
        let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
        match name.as_str() {
            "format" => {
                let bits = memoryview_format_bits(obj_ptr);
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            "itemsize" => {
                return Some(MoltObject::from_int(memoryview_itemsize(obj_ptr) as i64).bits());
            }
            "ndim" => {
                return Some(MoltObject::from_int(memoryview_ndim(obj_ptr) as i64).bits());
            }
            "shape" => {
                let shape = memoryview_shape(obj_ptr).unwrap_or(&[]);
                return Some(tuple_from_isize_slice(_py, shape));
            }
            "strides" => {
                let strides = memoryview_strides(obj_ptr).unwrap_or(&[]);
                return Some(tuple_from_isize_slice(_py, strides));
            }
            "readonly" => {
                return Some(MoltObject::from_bool(memoryview_readonly(obj_ptr)).bits());
            }
            "nbytes" => {
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
    if type_id == TYPE_ID_SLICE {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
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
    }
    if type_id == TYPE_ID_GENERIC_ALIAS {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
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
                    // TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial):
                    // derive __parameters__ from TypeVar/ParamSpec/TypeVarTuple when typing supports them.
                    let tuple_ptr = alloc_tuple(_py, &[]);
                    if tuple_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "__unpacked__" => {
                    return Some(MoltObject::from_bool(false).bits());
                }
                _ => {}
            }
        }
    }
    if type_id == TYPE_ID_FILE_HANDLE {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            let handle_ptr = file_handle_ptr(obj_ptr);
            if handle_ptr.is_null() {
                return None;
            }
            let handle = &*handle_ptr;
            match name.as_str() {
                "__class__" => {
                    let class_bits = if handle.class_bits != 0 {
                        handle.class_bits
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
                    if handle.detached {
                        return Some(MoltObject::none().bits());
                    }
                    if handle.buffer_bits != 0 {
                        inc_ref_bits(_py, handle.buffer_bits);
                        return Some(handle.buffer_bits);
                    }
                    return None;
                }
                _ => {}
            }
            if handle.text && name == "readinto" {
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
    }
    if type_id == TYPE_ID_DICT {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "fromkeys" {
                if let Some(func_bits) = dict_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let class_bits = type_of_bits(_py, self_bits);
                    let bound_bits = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound_bits);
                }
            }
            if let Some(func_bits) = dict_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_SET {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = set_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_FROZENSET {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = frozenset_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_LIST {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = list_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_STRING {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = string_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_BYTES {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = bytes_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_BYTEARRAY {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = bytearray_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_TYPE {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "__class__" {
                let builtins = builtin_classes(_py);
                let class_bits = object_class_bits(obj_ptr);
                let res_bits = if class_bits != 0 {
                    class_bits
                } else {
                    builtins.type_obj
                };
                inc_ref_bits(_py, res_bits);
                return Some(res_bits);
            }
            if name == "__dict__" {
                let dict_bits = class_dict_bits(obj_ptr);
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            if name == "__annotate__" {
                let mut annotate_bits = class_annotate_bits(obj_ptr);
                if annotate_bits == 0 {
                    let annotate_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotate_name,
                        b"__annotate__",
                    );
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            if let Some(val_bits) =
                                dict_get_in_place(_py, dict_ptr, annotate_name_bits)
                            {
                                annotate_bits = val_bits;
                                class_set_annotate_bits(_py, obj_ptr, annotate_bits);
                            }
                        }
                    }
                    if annotate_bits == 0 {
                        annotate_bits = MoltObject::none().bits();
                    }
                }
                inc_ref_bits(_py, annotate_bits);
                return Some(annotate_bits);
            }
            if name == "__annotations__" {
                let annotations_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotations_name,
                    b"__annotations__",
                );
                let dict_bits = class_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotations_bits) {
                            inc_ref_bits(_py, val_bits);
                            class_set_annotations_bits(_py, obj_ptr, val_bits);
                            return Some(val_bits);
                        }
                    }
                }
                let cached = class_annotations_bits(obj_ptr);
                if cached != 0 {
                    inc_ref_bits(_py, cached);
                    return Some(cached);
                }
                let annotate_bits = class_annotate_bits(obj_ptr);
                let res_bits = if annotate_bits != 0 && !obj_from_bits(annotate_bits).is_none() {
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
                class_set_annotations_bits(_py, obj_ptr, res_bits);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(_py, dict_ptr, annotations_bits, res_bits);
                    }
                }
                return Some(res_bits);
            }
            if name == "__name__" {
                let bits = class_name_bits(obj_ptr);
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            if name == "__base__" {
                let bases_bits = class_bases_bits(obj_ptr);
                let bases = class_bases_vec(bases_bits);
                if bases.is_empty() {
                    let none_bits = MoltObject::none().bits();
                    inc_ref_bits(_py, none_bits);
                    return Some(none_bits);
                }
                let base_bits = bases[0];
                inc_ref_bits(_py, base_bits);
                return Some(base_bits);
            }
            if name == "__bases__" {
                let bases_bits = class_bases_bits(obj_ptr);
                let bases_obj = obj_from_bits(bases_bits);
                if bases_obj.is_none() || bases_bits == 0 {
                    let tuple_ptr = alloc_tuple(_py, &[]);
                    if tuple_ptr.is_null() {
                        return None;
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                if let Some(bases_ptr) = bases_obj.as_ptr() {
                    let bases_type = object_type_id(bases_ptr);
                    if bases_type == TYPE_ID_TUPLE {
                        inc_ref_bits(_py, bases_bits);
                        return Some(bases_bits);
                    }
                    if bases_type == TYPE_ID_TYPE {
                        let tuple_ptr = alloc_tuple(_py, &[bases_bits]);
                        if tuple_ptr.is_null() {
                            return None;
                        }
                        return Some(MoltObject::from_ptr(tuple_ptr).bits());
                    }
                }
                return None;
            }
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if name == "fromkeys" {
                let builtins = builtin_classes(_py);
                if issubclass_bits(class_bits, builtins.dict) {
                    if let Some(func_bits) = dict_method_bits(_py, name.as_str()) {
                        let bound_bits = molt_bound_method_new(func_bits, class_bits);
                        return Some(bound_bits);
                    }
                }
            }
            if is_builtin_class_bits(_py, class_bits) {
                if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, name.as_str()) {
                    return descriptor_bind(_py, func_bits, obj_ptr, None);
                }
            }
        }
        return class_attr_lookup(_py, obj_ptr, obj_ptr, None, attr_bits);
    }
    if type_id == TYPE_ID_SUPER {
        let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
        let start_bits = super_type_bits(obj_ptr);
        let target_bits = super_obj_bits(obj_ptr);
        let target_ptr = maybe_ptr_from_bits(target_bits);
        let obj_type_bits = if let Some(raw_ptr) = target_ptr {
            if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                target_bits
            } else {
                type_of_bits(_py, target_bits)
            }
        } else {
            type_of_bits(_py, target_bits)
        };
        let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
        if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let mro_storage: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(obj_type_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(obj_type_bits))
        };
        let mut instance_ptr = None;
        let mut owner_ptr = obj_type_ptr;
        if let Some(raw_ptr) = target_ptr {
            if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                owner_ptr = raw_ptr;
                instance_ptr = Some(raw_ptr);
            } else {
                instance_ptr = Some(raw_ptr);
            }
        }
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
                return descriptor_bind(_py, val_bits, owner_ptr, instance_ptr);
            }
            if let Some(name) = attr_name.as_deref() {
                if is_builtin_class_bits(_py, *class_bits) {
                    if let Some(func_bits) = builtin_class_method_bits(_py, *class_bits, name) {
                        return descriptor_bind(_py, func_bits, owner_ptr, instance_ptr);
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_FUNCTION {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "__code__" {
                let code_bits = ensure_function_code_bits(_py, obj_ptr);
                if !obj_from_bits(code_bits).is_none() {
                    inc_ref_bits(_py, code_bits);
                    return Some(code_bits);
                }
                return None;
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
        ) {
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
            let res_bits = if annotate_bits != 0 && !obj_from_bits(annotate_bits).is_none() {
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
            let mut dict_bits = function_dict_bits(obj_ptr);
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    return None;
                }
                dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                function_set_dict_bits(obj_ptr, dict_bits);
            }
            inc_ref_bits(_py, dict_bits);
            return Some(dict_bits);
        }
        let dict_bits = function_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                        inc_ref_bits(_py, val);
                        return Some(val);
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_CODE {
        // TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial):
        // fill out code object fields (co_varnames, arg counts, co_linetable) for parity.
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
                let tuple_ptr = alloc_tuple(_py, &[]);
                if tuple_ptr.is_null() {
                    return Some(MoltObject::none().bits());
                }
                return Some(MoltObject::from_ptr(tuple_ptr).bits());
            }
            _ => {}
        }
        return None;
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let slots = (*desc_ptr).slots;
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let getattribute_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.getattribute_name,
                            b"__getattribute__",
                        );
                        if !obj_eq(
                            _py,
                            obj_from_bits(attr_bits),
                            obj_from_bits(getattribute_bits),
                        ) {
                            if let Some(call_bits) = class_attr_lookup(
                                _py,
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                getattribute_bits,
                            ) {
                                exception_stack_push();
                                let res_bits = call_callable1(_py, call_bits, attr_bits);
                                if exception_pending(_py) {
                                    let exc_bits = molt_exception_last();
                                    let kind_bits = molt_exception_kind(exc_bits);
                                    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                    dec_ref_bits(_py, kind_bits);
                                    if kind.as_deref() == Some("AttributeError") {
                                        let getattr_bits = intern_static_name(
                                            _py,
                                            &runtime_state(_py).interned.getattr_name,
                                            b"__getattr__",
                                        );
                                        if !obj_eq(
                                            _py,
                                            obj_from_bits(attr_bits),
                                            obj_from_bits(getattr_bits),
                                        ) && class_attr_lookup_raw_mro(
                                            _py,
                                            class_ptr,
                                            getattr_bits,
                                        )
                                        .is_some()
                                        {
                                            molt_exception_clear();
                                            dec_ref_bits(_py, exc_bits);
                                            exception_stack_pop(_py);
                                            if let Some(getattr_call_bits) = class_attr_lookup(
                                                _py,
                                                class_ptr,
                                                class_ptr,
                                                Some(obj_ptr),
                                                getattr_bits,
                                            ) {
                                                let getattr_res = call_callable1(
                                                    _py,
                                                    getattr_call_bits,
                                                    attr_bits,
                                                );
                                                if exception_pending(_py) {
                                                    return None;
                                                }
                                                return Some(getattr_res);
                                            }
                                        }
                                    }
                                    dec_ref_bits(_py, exc_bits);
                                    exception_stack_pop(_py);
                                    return None;
                                }
                                exception_stack_pop(_py);
                                return Some(res_bits);
                            }
                        }
                        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if descriptor_is_data(_py, val_bits) {
                                if let Some(bound) =
                                    descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                                {
                                    return Some(bound);
                                }
                                if exception_pending(_py) {
                                    return None;
                                }
                            }
                        }
                    }
                }
            }
            let class_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(class_name_bits),
            ) {
                if class_bits != 0 {
                    inc_ref_bits(_py, class_bits);
                    return Some(class_bits);
                }
                return None;
            }
            let dict_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
                if !slots {
                    let mut dict_bits = dataclass_dict_bits(obj_ptr);
                    if dict_bits == 0 {
                        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                        if !dict_ptr.is_null() {
                            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                            dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
                        }
                    }
                    if dict_bits != 0 {
                        inc_ref_bits(_py, dict_bits);
                        return Some(dict_bits);
                    }
                }
                return None;
            }
            if !slots {
                let dict_bits = dataclass_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                            inc_ref_bits(_py, val);
                            return Some(val);
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if let Some(bound) =
                                descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                            {
                                return Some(bound);
                            }
                            if exception_pending(_py) {
                                return None;
                            }
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let getattr_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.getattr_name,
                            b"__getattr__",
                        );
                        if !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                            if let Some(call_bits) = class_attr_lookup(
                                _py,
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                getattr_bits,
                            ) {
                                let res_bits = call_callable1(_py, call_bits, attr_bits);
                                return Some(res_bits);
                            }
                        }
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_OBJECT {
        let class_bits = object_class_bits(obj_ptr);
        let mut cached_attr_bits: Option<u64> = None;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let getattribute_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.getattribute_name,
                        b"__getattribute__",
                    );
                    if !obj_eq(
                        _py,
                        obj_from_bits(attr_bits),
                        obj_from_bits(getattribute_bits),
                    ) {
                        if let Some(call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            getattribute_bits,
                        ) {
                            let getattr_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.getattr_name,
                                b"__getattr__",
                            );
                            let getattr_candidate = !obj_eq(
                                _py,
                                obj_from_bits(attr_bits),
                                obj_from_bits(getattr_bits),
                            ) && class_attr_lookup_raw_mro(_py, class_ptr, getattr_bits).is_some();
                            if getattr_candidate {
                                traceback_suppress_enter();
                            }
                            exception_stack_push();
                            let res_bits = call_callable1(_py, call_bits, attr_bits);
                            if getattr_candidate {
                                traceback_suppress_exit();
                            }
                            if exception_pending(_py) {
                                let exc_bits = molt_exception_last();
                                let kind_bits = molt_exception_kind(exc_bits);
                                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                dec_ref_bits(_py, kind_bits);
                                if kind.as_deref() == Some("AttributeError") {
                                    if !obj_eq(
                                        _py,
                                        obj_from_bits(attr_bits),
                                        obj_from_bits(getattr_bits),
                                    ) && class_attr_lookup_raw_mro(_py, class_ptr, getattr_bits)
                                        .is_some()
                                    {
                                        molt_exception_clear();
                                        dec_ref_bits(_py, exc_bits);
                                        exception_stack_pop(_py);
                                        if let Some(getattr_call_bits) = class_attr_lookup(
                                            _py,
                                            class_ptr,
                                            class_ptr,
                                            Some(obj_ptr),
                                            getattr_bits,
                                        ) {
                                            let getattr_res =
                                                call_callable1(_py, getattr_call_bits, attr_bits);
                                            if exception_pending(_py) {
                                                return None;
                                            }
                                            return Some(getattr_res);
                                        }
                                    }
                                }
                                dec_ref_bits(_py, exc_bits);
                                exception_stack_pop(_py);
                                return None;
                            }
                            exception_stack_pop(_py);
                            return Some(res_bits);
                        }
                    }
                    let class_version = class_layout_version_bits(class_ptr);
                    if let Some(entry) =
                        descriptor_cache_lookup(class_bits, attr_bits, class_version)
                    {
                        if let Some(bits) = entry.data_desc_bits {
                            if let Some(bound) =
                                descriptor_bind(_py, bits, class_ptr, Some(obj_ptr))
                            {
                                return Some(bound);
                            }
                            if exception_pending(_py) {
                                return None;
                            }
                        }
                        cached_attr_bits = entry.class_attr_bits;
                    }
                    if cached_attr_bits.is_none() {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if descriptor_is_data(_py, val_bits) {
                                descriptor_cache_store(
                                    class_bits,
                                    attr_bits,
                                    class_version,
                                    Some(val_bits),
                                    None,
                                );
                                if let Some(bound) =
                                    descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                                {
                                    return Some(bound);
                                }
                                if exception_pending(_py) {
                                    return None;
                                }
                            }
                            cached_attr_bits = Some(val_bits);
                            descriptor_cache_store(
                                class_bits,
                                attr_bits,
                                class_version,
                                None,
                                Some(val_bits),
                            );
                        } else {
                            descriptor_cache_store(
                                class_bits,
                                attr_bits,
                                class_version,
                                None,
                                None,
                            );
                        }
                    }
                    if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                        let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                        return Some(bits);
                    }
                }
            }
        }
        let class_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(class_name_bits),
        ) {
            if class_bits != 0 {
                inc_ref_bits(_py, class_bits);
                return Some(class_bits);
            }
            return None;
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            let mut dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if !dict_ptr.is_null() {
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    instance_set_dict_bits(_py, obj_ptr, dict_bits);
                }
            }
            if dict_bits != 0 {
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            return None;
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                        inc_ref_bits(_py, val);
                        return Some(val);
                    }
                }
            }
        }
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if cached_attr_bits.is_none() {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            cached_attr_bits = Some(val_bits);
                            let class_version = class_layout_version_bits(class_ptr);
                            descriptor_cache_store(
                                class_bits,
                                attr_bits,
                                class_version,
                                None,
                                Some(val_bits),
                            );
                        }
                    }
                    if let Some(val_bits) = cached_attr_bits {
                        if let Some(bound) =
                            descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                        {
                            return Some(bound);
                        }
                        if exception_pending(_py) {
                            return None;
                        }
                    }
                }
            }
        }
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let getattr_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.getattr_name,
                        b"__getattr__",
                    );
                    if !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                        if let Some(call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            getattr_bits,
                        ) {
                            let res_bits = call_callable1(_py, call_bits, attr_bits);
                            return Some(res_bits);
                        }
                    }
                }
            }
        }
        return None;
    }
    None
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let attr_name_len = usize_from_bits(attr_name_len_bits);
        if obj_ptr.is_null() {
            return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
        }
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
        let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
            return MoltObject::none().bits() as i64;
        };
        let found = attr_lookup_ptr(_py, obj_ptr, attr_bits);
        dec_ref_bits(_py, attr_bits);
        if let Some(val) = found {
            return val as i64;
        }
        if exception_pending(_py) {
            let exc_bits = molt_exception_last();
            molt_exception_clear();
            let _ = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return MoltObject::none().bits() as i64;
        }
        let type_id = object_type_id(obj_ptr);
        if type_id == TYPE_ID_DATACLASS {
            let desc_ptr = dataclass_desc_ptr(obj_ptr);
            if !desc_ptr.is_null() && (*desc_ptr).slots {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error(_py, type_label, attr_name);
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
            let class_name =
                string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
            return raise_exception::<_>(_py, "AttributeError", &msg);
        }
        attr_error(
            _py,
            type_name(_py, MoltObject::from_ptr(obj_ptr)),
            attr_name,
        )
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        molt_get_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let attr_name_len = usize_from_bits(attr_name_len_bits);
        if obj_ptr.is_null() {
            return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
        }
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
        let type_id = object_type_id(obj_ptr);
        if type_id == TYPE_ID_MODULE {
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let module_bits = MoltObject::from_ptr(obj_ptr).bits();
            let res = molt_module_set_attr(module_bits, attr_bits, val_bits);
            dec_ref_bits(_py, attr_bits);
            return res as i64;
        }
        if type_id == TYPE_ID_TYPE {
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if is_builtin_class_bits(_py, class_bits) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot set attributes on builtin type",
                );
            }
            if attr_name == "__annotate__" {
                let val_obj = obj_from_bits(val_bits);
                if !val_obj.is_none() {
                    let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(val_bits)));
                    if !callable_ok {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotate__ must be callable or None",
                        );
                    }
                    class_set_annotations_bits(_py, obj_ptr, 0u64);
                }
                let dict_bits = class_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        dict_set_in_place(_py, dict_ptr, annotate_bits, val_bits);
                        if !val_obj.is_none() {
                            let annotations_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.annotations_name,
                                b"__annotations__",
                            );
                            dict_del_in_place(_py, dict_ptr, annotations_bits);
                        }
                    }
                }
                class_set_annotate_bits(_py, obj_ptr, val_bits);
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
            if attr_name == "__annotations__" {
                let dict_bits = class_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let annotations_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotations_name,
                            b"__annotations__",
                        );
                        dict_set_in_place(_py, dict_ptr, annotations_bits, val_bits);
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        let none_bits = MoltObject::none().bits();
                        dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                    }
                }
                class_set_annotations_bits(_py, obj_ptr, val_bits);
                class_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let dict_bits = class_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    class_bump_layout_version(obj_ptr);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
            }
            dec_ref_bits(_py, attr_bits);
            return attr_error(_py, "type", attr_name);
        }
        if type_id == TYPE_ID_EXCEPTION {
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let name = string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_default();
            if name == "__cause__" || name == "__context__" {
                let val_obj = obj_from_bits(val_bits);
                if !val_obj.is_none() {
                    let Some(val_ptr) = val_obj.as_ptr() else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            if name == "__cause__" {
                                "exception cause must be an exception or None"
                            } else {
                                "exception context must be an exception or None"
                            },
                        );
                    };
                    unsafe {
                        if object_type_id(val_ptr) != TYPE_ID_EXCEPTION {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                if name == "__cause__" {
                                    "exception cause must be an exception or None"
                                } else {
                                    "exception context must be an exception or None"
                                },
                            );
                        }
                    }
                }
                unsafe {
                    let slot = if name == "__cause__" {
                        obj_ptr.add(2 * std::mem::size_of::<u64>())
                    } else {
                        obj_ptr.add(3 * std::mem::size_of::<u64>())
                    } as *mut u64;
                    let old_bits = *slot;
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        *slot = val_bits;
                    }
                    if name == "__cause__" {
                        let suppress_bits = MoltObject::from_bool(true).bits();
                        let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *suppress_slot;
                        if old_bits != suppress_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, suppress_bits);
                            *suppress_slot = suppress_bits;
                        }
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return MoltObject::none().bits() as i64;
            }
            if name == "args" {
                let args_bits = exception_args_from_iterable(_py, val_bits);
                if obj_from_bits(args_bits).is_none() {
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                let msg_bits = exception_message_from_args(_py, args_bits);
                if obj_from_bits(msg_bits).is_none() {
                    dec_ref_bits(_py, args_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                unsafe {
                    exception_store_args_and_message(_py, obj_ptr, args_bits, msg_bits);
                    exception_set_stop_iteration_value(_py, obj_ptr, args_bits);
                }
                dec_ref_bits(_py, attr_bits);
                return MoltObject::none().bits() as i64;
            }
            if name == "__suppress_context__" {
                let suppress = is_truthy(_py, obj_from_bits(val_bits));
                let suppress_bits = MoltObject::from_bool(suppress).bits();
                unsafe {
                    let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, suppress_bits);
                        *slot = suppress_bits;
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return MoltObject::none().bits() as i64;
            }
            if name == "__dict__" {
                let val_obj = obj_from_bits(val_bits);
                let Some(val_ptr) = val_obj.as_ptr() else {
                    let msg = format!(
                        "__dict__ must be set to a dictionary, not a '{}'",
                        type_name(_py, val_obj)
                    );
                    dec_ref_bits(_py, attr_bits);
                    return raise_exception::<_>(_py, "TypeError", &msg);
                };
                if object_type_id(val_ptr) != TYPE_ID_DICT {
                    let msg = format!(
                        "__dict__ must be set to a dictionary, not a '{}'",
                        type_name(_py, val_obj)
                    );
                    dec_ref_bits(_py, attr_bits);
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                unsafe {
                    let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        *slot = val_bits;
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return MoltObject::none().bits() as i64;
            }
            if name == "value" {
                let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)))
                    .unwrap_or_default();
                if kind != "StopIteration" {
                    dec_ref_bits(_py, attr_bits);
                    return attr_error(_py, "exception", attr_name);
                }
                unsafe {
                    let slot = obj_ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        *slot = val_bits;
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return MoltObject::none().bits() as i64;
            }
            let mut dict_bits = exception_dict_bits(obj_ptr);
            if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if !dict_ptr.is_null() {
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != dict_bits {
                        dec_ref_bits(_py, old_bits);
                        *slot = dict_bits;
                    }
                }
            }
            if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
            dec_ref_bits(_py, attr_bits);
            return attr_error(_py, "exception", attr_name);
        }
        if type_id == TYPE_ID_FUNCTION {
            if attr_name == "__code__" {
                let val_obj = obj_from_bits(val_bits);
                let Some(val_ptr) = val_obj.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "function __code__ must be a code object",
                    );
                };
                unsafe {
                    if object_type_id(val_ptr) != TYPE_ID_CODE {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "function __code__ must be a code object",
                        );
                    }
                    function_set_code_bits(_py, obj_ptr, val_bits);
                }
                return MoltObject::none().bits() as i64;
            }
            if attr_name == "__annotate__" {
                let val_obj = obj_from_bits(val_bits);
                if !val_obj.is_none() {
                    let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(val_bits)));
                    if !callable_ok {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotate__ must be callable or None",
                        );
                    }
                    function_set_annotations_bits(_py, obj_ptr, 0);
                }
                function_set_annotate_bits(_py, obj_ptr, val_bits);
                return MoltObject::none().bits() as i64;
            }
            if attr_name == "__annotations__" {
                let val_obj = obj_from_bits(val_bits);
                let ann_bits = if val_obj.is_none() {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return MoltObject::none().bits() as i64;
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                } else {
                    let Some(val_ptr) = val_obj.as_ptr() else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotations__ must be set to a dict object",
                        );
                    };
                    if object_type_id(val_ptr) != TYPE_ID_DICT {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotations__ must be set to a dict object",
                        );
                    }
                    val_bits
                };
                function_set_annotations_bits(_py, obj_ptr, ann_bits);
                function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                return MoltObject::none().bits() as i64;
            }
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let mut dict_bits = function_dict_bits(obj_ptr);
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                function_set_dict_bits(obj_ptr, dict_bits);
            }
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
            }
            dec_ref_bits(_py, attr_bits);
            return attr_error(_py, "function", attr_name);
        }
        if type_id == TYPE_ID_CODE {
            return attr_error(_py, "code", attr_name);
        }
        if type_id == TYPE_ID_DATACLASS {
            let desc_ptr = dataclass_desc_ptr(obj_ptr);
            if !desc_ptr.is_null() && (*desc_ptr).frozen {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot assign to frozen dataclass field",
                );
            }
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            if !desc_ptr.is_null() {
                let class_bits = (*desc_ptr).class_bits;
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            let setattr_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.setattr_name,
                                b"__setattr__",
                            );
                            if let Some(call_bits) = class_attr_lookup(
                                _py,
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                setattr_bits,
                            ) {
                                let _ = call_callable2(_py, call_bits, attr_bits, val_bits);
                                dec_ref_bits(_py, attr_bits);
                                return MoltObject::none().bits() as i64;
                            }
                            if let Some(desc_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            {
                                if descriptor_is_data(_py, desc_bits) {
                                    let desc_obj = obj_from_bits(desc_bits);
                                    if let Some(desc_ptr) = desc_obj.as_ptr() {
                                        if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                            let set_bits = property_set_bits(desc_ptr);
                                            if obj_from_bits(set_bits).is_none() {
                                                dec_ref_bits(_py, attr_bits);
                                                return property_no_setter(
                                                    _py, attr_name, class_ptr,
                                                );
                                            }
                                            let inst_bits = instance_bits_for_call(obj_ptr);
                                            let _ = call_function_obj2(
                                                _py, set_bits, inst_bits, val_bits,
                                            );
                                            dec_ref_bits(_py, attr_bits);
                                            return MoltObject::none().bits() as i64;
                                        }
                                    }
                                    let set_bits = intern_static_name(
                                        _py,
                                        &runtime_state(_py).interned.set_name,
                                        b"__set__",
                                    );
                                    if let Some(method_bits) =
                                        descriptor_method_bits(_py, desc_bits, set_bits)
                                    {
                                        let self_bits = desc_bits;
                                        let inst_bits = instance_bits_for_call(obj_ptr);
                                        let method_obj = obj_from_bits(method_bits);
                                        if let Some(method_ptr) = method_obj.as_ptr() {
                                            if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                                let _ = call_function_obj3(
                                                    _py,
                                                    method_bits,
                                                    self_bits,
                                                    inst_bits,
                                                    val_bits,
                                                );
                                            } else {
                                                let _ = call_callable2(
                                                    _py,
                                                    method_bits,
                                                    inst_bits,
                                                    val_bits,
                                                );
                                            }
                                        } else {
                                            let _ = call_callable2(
                                                _py,
                                                method_bits,
                                                inst_bits,
                                                val_bits,
                                            );
                                        }
                                        dec_ref_bits(_py, attr_bits);
                                        return MoltObject::none().bits() as i64;
                                    }
                                    dec_ref_bits(_py, attr_bits);
                                    return descriptor_no_setter(_py, attr_name, class_ptr);
                                }
                            }
                        }
                    }
                }
                if (*desc_ptr).slots {
                    dec_ref_bits(_py, attr_bits);
                    let name = &(*desc_ptr).name;
                    let type_label = if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    };
                    return attr_error(_py, type_label, attr_name);
                }
            }
            let mut dict_bits = dataclass_dict_bits(obj_ptr);
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
            }
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
            }
            dec_ref_bits(_py, attr_bits);
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
        if type_id == TYPE_ID_OBJECT {
            let header = header_from_obj_ptr(obj_ptr);
            if (*header).poll_fn != 0 {
                return attr_error(_py, "object", attr_name);
            }
            let payload = object_payload_size(obj_ptr);
            if payload < std::mem::size_of::<u64>() {
                return attr_error(_py, "object", attr_name);
            }
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let class_bits = object_class_bits(obj_ptr);
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let setattr_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.setattr_name,
                            b"__setattr__",
                        );
                        if let Some(call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            setattr_bits,
                        ) {
                            let _ = call_callable2(_py, call_bits, attr_bits, val_bits);
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        if let Some(desc_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if descriptor_is_data(_py, desc_bits) {
                                let desc_obj = obj_from_bits(desc_bits);
                                if let Some(desc_ptr) = desc_obj.as_ptr() {
                                    if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                        let set_bits = property_set_bits(desc_ptr);
                                        if obj_from_bits(set_bits).is_none() {
                                            dec_ref_bits(_py, attr_bits);
                                            return property_no_setter(_py, attr_name, class_ptr);
                                        }
                                        let inst_bits = instance_bits_for_call(obj_ptr);
                                        let _ =
                                            call_function_obj2(_py, set_bits, inst_bits, val_bits);
                                        dec_ref_bits(_py, attr_bits);
                                        return MoltObject::none().bits() as i64;
                                    }
                                }
                                let set_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.set_name,
                                    b"__set__",
                                );
                                if let Some(method_bits) =
                                    descriptor_method_bits(_py, desc_bits, set_bits)
                                {
                                    let self_bits = desc_bits;
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let method_obj = obj_from_bits(method_bits);
                                    if let Some(method_ptr) = method_obj.as_ptr() {
                                        if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                            let _ = call_function_obj3(
                                                _py,
                                                method_bits,
                                                self_bits,
                                                inst_bits,
                                                val_bits,
                                            );
                                        } else {
                                            let _ = call_callable2(
                                                _py,
                                                method_bits,
                                                inst_bits,
                                                val_bits,
                                            );
                                        }
                                    } else {
                                        let _ =
                                            call_callable2(_py, method_bits, inst_bits, val_bits);
                                    }
                                    dec_ref_bits(_py, attr_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                                dec_ref_bits(_py, attr_bits);
                                return descriptor_no_setter(_py, attr_name, class_ptr);
                            }
                        }
                        if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                            dec_ref_bits(_py, attr_bits);
                            return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits) as i64;
                        }
                    }
                }
            }
            let mut dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                instance_set_dict_bits(_py, obj_ptr, dict_bits);
            }
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
            }
            dec_ref_bits(_py, attr_bits);
            return attr_error(_py, "object", attr_name);
        }
        attr_error(
            _py,
            type_name(_py, MoltObject::from_ptr(obj_ptr)),
            attr_name,
        )
    })
}

pub(crate) unsafe fn del_attr_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        let dict_bits = module_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
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
                    if dict_del_in_place(_py, dict_ptr, annotations_bits) {
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        let none_bits = MoltObject::none().bits();
                        dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
                    return raise_exception::<_>(_py, "AttributeError", &msg);
                }
                let annotate_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotate_name,
                    b"__annotate__",
                );
                if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(annotate_bits)) {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot delete __annotate__ attribute",
                    );
                }
                if dict_del_in_place(_py, dict_ptr, attr_bits) {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        let module_name =
            string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
        return raise_exception::<_>(_py, "AttributeError", &msg);
    }
    if type_id == TYPE_ID_TYPE {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        if is_builtin_class_bits(_py, class_bits) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cannot delete attributes on builtin type",
            );
        }
        if attr_name == "__annotate__" {
            return raise_exception::<_>(_py, "TypeError", "cannot delete __annotate__ attribute");
        }
        if attr_name == "__annotations__" {
            let dict_bits = class_dict_bits(obj_ptr);
            let mut removed = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let annotations_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotations_name,
                        b"__annotations__",
                    );
                    if dict_del_in_place(_py, dict_ptr, annotations_bits) {
                        removed = true;
                    }
                    if removed {
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        let none_bits = MoltObject::none().bits();
                        dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                    }
                }
            }
            if !removed && class_annotations_bits(obj_ptr) != 0 {
                removed = true;
            }
            if removed {
                class_set_annotations_bits(_py, obj_ptr, 0u64);
                class_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
            let class_name =
                string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
            return raise_exception::<_>(_py, "AttributeError", &msg);
        }
        let dict_bits = class_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
        }
        let class_name =
            string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
        return raise_exception::<_>(_py, "AttributeError", &msg);
    }
    if type_id == TYPE_ID_EXCEPTION {
        if attr_name == "__cause__" || attr_name == "__context__" {
            unsafe {
                let slot = if attr_name == "__cause__" {
                    obj_ptr.add(2 * std::mem::size_of::<u64>())
                } else {
                    obj_ptr.add(3 * std::mem::size_of::<u64>())
                } as *mut u64;
                let old_bits = *slot;
                if !obj_from_bits(old_bits).is_none() {
                    dec_ref_bits(_py, old_bits);
                    let none_bits = MoltObject::none().bits();
                    inc_ref_bits(_py, none_bits);
                    *slot = none_bits;
                }
                if attr_name == "__cause__" {
                    let suppress_bits = MoltObject::from_bool(false).bits();
                    let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *suppress_slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, suppress_bits);
                        *suppress_slot = suppress_bits;
                    }
                }
            }
            return MoltObject::none().bits() as i64;
        }
        if attr_name == "__suppress_context__" {
            unsafe {
                let suppress_bits = MoltObject::from_bool(false).bits();
                let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != suppress_bits {
                    dec_ref_bits(_py, old_bits);
                    inc_ref_bits(_py, suppress_bits);
                    *slot = suppress_bits;
                }
            }
            return MoltObject::none().bits() as i64;
        }
        let dict_bits = exception_dict_bits(obj_ptr);
        if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(_py, dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error(_py, "exception", attr_name);
    }
    if type_id == TYPE_ID_FUNCTION {
        if attr_name == "__annotate__" {
            return raise_exception::<_>(_py, "TypeError", "cannot delete __annotate__ attribute");
        }
        if attr_name == "__annotations__" {
            function_set_annotations_bits(_py, obj_ptr, 0);
            function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
            return MoltObject::none().bits() as i64;
        }
        let dict_bits = function_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(_py, dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error(_py, "function", attr_name);
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let delattr_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.delattr_name,
                            b"__delattr__",
                        );
                        if let Some(call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            delattr_bits,
                        ) {
                            let _ = call_callable1(_py, call_bits, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        if let Some(desc_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let del_bits = property_del_bits(desc_ptr);
                                    if obj_from_bits(del_bits).is_none() {
                                        return property_no_deleter(_py, attr_name, class_ptr);
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj1(_py, del_bits, inst_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let del_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.delete_name,
                                b"__delete__",
                            );
                            if let Some(method_bits) =
                                descriptor_method_bits(_py, desc_bits, del_bits)
                            {
                                let self_bits = desc_bits;
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj2(
                                            _py,
                                            method_bits,
                                            self_bits,
                                            inst_bits,
                                        );
                                    } else {
                                        let _ = call_callable1(_py, method_bits, inst_bits);
                                    }
                                } else {
                                    let _ = call_callable1(_py, method_bits, inst_bits);
                                }
                                return MoltObject::none().bits() as i64;
                            }
                            let set_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.set_name,
                                b"__set__",
                            );
                            if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                                return descriptor_no_deleter(_py, attr_name, class_ptr);
                            }
                        }
                    }
                }
            }
            if (*desc_ptr).frozen {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete frozen dataclass field",
                );
            }
            if (*desc_ptr).slots {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error(_py, type_label, attr_name);
            }
        }
        let dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(_py, dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
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
    if type_id == TYPE_ID_OBJECT {
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return attr_error(_py, "object", attr_name);
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error(_py, "object", attr_name);
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let delattr_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.delattr_name,
                        b"__delattr__",
                    );
                    if let Some(call_bits) =
                        class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), delattr_bits)
                    {
                        let _ = call_callable1(_py, call_bits, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                        if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let del_bits = property_del_bits(desc_ptr);
                                if obj_from_bits(del_bits).is_none() {
                                    return property_no_deleter(_py, attr_name, class_ptr);
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj1(_py, del_bits, inst_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let del_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.delete_name,
                            b"__delete__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits)
                        {
                            let self_bits = desc_bits;
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ =
                                        call_function_obj2(_py, method_bits, self_bits, inst_bits);
                                } else {
                                    let _ = call_callable1(_py, method_bits, inst_bits);
                                }
                            } else {
                                let _ = call_callable1(_py, method_bits, inst_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.set_name,
                            b"__set__",
                        );
                        if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                            return descriptor_no_deleter(_py, attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(_py, dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error(_py, "object", attr_name);
    }
    attr_error(
        _py,
        type_name(_py, MoltObject::from_ptr(obj_ptr)),
        attr_name,
    )
}

pub(crate) unsafe fn object_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).poll_fn != 0 {
        return attr_error(_py, "object", attr_name);
    }
    let payload = object_payload_size(obj_ptr);
    if payload < std::mem::size_of::<u64>() {
        return attr_error(_py, "object", attr_name);
    }
    let class_bits = object_class_bits(obj_ptr);
    if class_bits != 0 {
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                    if descriptor_is_data(_py, desc_bits) {
                        let desc_obj = obj_from_bits(desc_bits);
                        if let Some(desc_ptr) = desc_obj.as_ptr() {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let set_bits = property_set_bits(desc_ptr);
                                if obj_from_bits(set_bits).is_none() {
                                    return property_no_setter(_py, attr_name, class_ptr);
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj2(_py, set_bits, inst_bits, val_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let set_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.set_name,
                            b"__set__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, set_bits)
                        {
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ = call_function_obj3(
                                        _py,
                                        method_bits,
                                        desc_bits,
                                        inst_bits,
                                        val_bits,
                                    );
                                } else {
                                    let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                                }
                            } else {
                                let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        return descriptor_no_setter(_py, attr_name, class_ptr);
                    }
                }
                if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                    return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits) as i64;
                }
            }
        }
    }
    let mut dict_bits = instance_dict_bits(obj_ptr);
    if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        instance_set_dict_bits(_py, obj_ptr, dict_bits);
    }
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        if object_type_id(dict_ptr) == TYPE_ID_DICT {
            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
            return MoltObject::none().bits() as i64;
        }
    }
    attr_error(_py, "object", attr_name)
}

pub(crate) unsafe fn dataclass_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    let desc_ptr = dataclass_desc_ptr(obj_ptr);
    if !desc_ptr.is_null() && (*desc_ptr).frozen {
        return raise_exception::<_>(_py, "TypeError", "cannot assign to frozen dataclass field");
    }
    if !desc_ptr.is_null() {
        let class_bits = (*desc_ptr).class_bits;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                        if descriptor_is_data(_py, desc_bits) {
                            let desc_obj = obj_from_bits(desc_bits);
                            if let Some(desc_ptr) = desc_obj.as_ptr() {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let set_bits = property_set_bits(desc_ptr);
                                    if obj_from_bits(set_bits).is_none() {
                                        return property_no_setter(_py, attr_name, class_ptr);
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj2(_py, set_bits, inst_bits, val_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let set_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.set_name,
                                b"__set__",
                            );
                            if let Some(method_bits) =
                                descriptor_method_bits(_py, desc_bits, set_bits)
                            {
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj3(
                                            _py,
                                            method_bits,
                                            desc_bits,
                                            inst_bits,
                                            val_bits,
                                        );
                                    } else {
                                        let _ =
                                            call_callable2(_py, method_bits, inst_bits, val_bits);
                                    }
                                } else {
                                    let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                                }
                                return MoltObject::none().bits() as i64;
                            }
                            return descriptor_no_setter(_py, attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        if (*desc_ptr).slots {
            let name = &(*desc_ptr).name;
            let type_label = if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            };
            return attr_error(_py, type_label, attr_name);
        }
    }
    let mut dict_bits = dataclass_dict_bits(obj_ptr);
    if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
    }
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        if object_type_id(dict_ptr) == TYPE_ID_DICT {
            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
            return MoltObject::none().bits() as i64;
        }
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
    attr_error(_py, type_label, attr_name)
}

pub(crate) unsafe fn object_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).poll_fn != 0 {
        return attr_error(_py, "object", attr_name);
    }
    let payload = object_payload_size(obj_ptr);
    if payload < std::mem::size_of::<u64>() {
        return attr_error(_py, "object", attr_name);
    }
    let class_bits = object_class_bits(obj_ptr);
    if class_bits != 0 {
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                    if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                        if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                            let del_bits = property_del_bits(desc_ptr);
                            if obj_from_bits(del_bits).is_none() {
                                return property_no_deleter(_py, attr_name, class_ptr);
                            }
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let _ = call_function_obj1(_py, del_bits, inst_bits);
                            return MoltObject::none().bits() as i64;
                        }
                    }
                    let del_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.delete_name,
                        b"__delete__",
                    );
                    if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits) {
                        let inst_bits = instance_bits_for_call(obj_ptr);
                        let method_obj = obj_from_bits(method_bits);
                        if let Some(method_ptr) = method_obj.as_ptr() {
                            if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                let _ = call_function_obj2(_py, method_bits, desc_bits, inst_bits);
                            } else {
                                let _ = call_callable1(_py, method_bits, inst_bits);
                            }
                        } else {
                            let _ = call_callable1(_py, method_bits, inst_bits);
                        }
                        return MoltObject::none().bits() as i64;
                    }
                    let set_bits =
                        intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
                    if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                        return descriptor_no_deleter(_py, attr_name, class_ptr);
                    }
                }
            }
        }
    }
    let dict_bits = instance_dict_bits(obj_ptr);
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                return MoltObject::none().bits() as i64;
            }
        }
    }
    attr_error(_py, "object", attr_name)
}

pub(crate) unsafe fn dataclass_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    let desc_ptr = dataclass_desc_ptr(obj_ptr);
    if !desc_ptr.is_null() {
        let class_bits = (*desc_ptr).class_bits;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                        if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let del_bits = property_del_bits(desc_ptr);
                                if obj_from_bits(del_bits).is_none() {
                                    return property_no_deleter(_py, attr_name, class_ptr);
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj1(_py, del_bits, inst_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let del_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.delete_name,
                            b"__delete__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits)
                        {
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ =
                                        call_function_obj2(_py, method_bits, desc_bits, inst_bits);
                                } else {
                                    let _ = call_callable1(_py, method_bits, inst_bits);
                                }
                            } else {
                                let _ = call_callable1(_py, method_bits, inst_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.set_name,
                            b"__set__",
                        );
                        if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                            return descriptor_no_deleter(_py, attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        if (*desc_ptr).frozen {
            return raise_exception::<_>(_py, "TypeError", "cannot delete frozen dataclass field");
        }
        if (*desc_ptr).slots {
            let name = &(*desc_ptr).name;
            let type_label = if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            };
            return attr_error(_py, type_label, attr_name);
        }
    }
    let dict_bits = dataclass_dict_bits(obj_ptr);
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                return MoltObject::none().bits() as i64;
            }
        }
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
    attr_error(_py, type_label, attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        molt_set_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits)
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let attr_name_len = usize_from_bits(attr_name_len_bits);
        if obj_ptr.is_null() {
            return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
        }
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
        let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
            return MoltObject::none().bits() as i64;
        };
        let res = del_attr_ptr(_py, obj_ptr, attr_bits, attr_name);
        dec_ref_bits(_py, attr_bits);
        res
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        molt_del_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let attr_name_len = usize_from_bits(attr_name_len_bits);
        if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
            return molt_get_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
        }
        let obj = obj_from_bits(obj_bits);
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
        attr_error(_py, type_name(_py, obj), attr_name)
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_special(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let attr_name_len = usize_from_bits(attr_name_len_bits);
        let obj = obj_from_bits(obj_bits);
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
        let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
            return attr_error(_py, type_name(_py, obj), attr_name);
        };
        let name_ptr = alloc_string(_py, slice);
        if name_ptr.is_null() {
            return MoltObject::none().bits() as i64;
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
            return bits as i64;
        }
        attr_error(_py, type_name(_py, obj), attr_name)
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let attr_name_len = usize_from_bits(attr_name_len_bits);
        if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
            return molt_set_attr_generic(ptr, attr_name_ptr, attr_name_len_bits, val_bits);
        }
        let obj = obj_from_bits(obj_bits);
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
        attr_error(_py, type_name(_py, obj), attr_name)
    })
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let attr_name_len = usize_from_bits(attr_name_len_bits);
        if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
            return molt_del_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
        }
        let obj = obj_from_bits(obj_bits);
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
        attr_error(_py, type_name(_py, obj), attr_name)
    })
}

#[no_mangle]
pub extern "C" fn molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if let Some(val) = attr_lookup_ptr(_py, obj_ptr, name_bits) {
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_DATACLASS {
                    let desc_ptr = dataclass_desc_ptr(obj_ptr);
                    if !desc_ptr.is_null() && (*desc_ptr).slots {
                        let name = &(*desc_ptr).name;
                        let type_label = if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        };
                        return attr_error(_py, type_label, &attr_name) as u64;
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
                    return attr_error(_py, type_label, &attr_name) as u64;
                }
                if type_id == TYPE_ID_TYPE {
                    let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                    return raise_exception::<_>(_py, "AttributeError", &msg);
                }
                return attr_error(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    &attr_name,
                ) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error(_py, type_name(_py, obj), &attr_name) as u64
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_get_attr_name_default(
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if let Some(val) = attr_lookup_ptr(_py, obj_ptr, name_bits) {
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    let kind_bits = molt_exception_kind(exc_bits);
                    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                    dec_ref_bits(_py, kind_bits);
                    if kind.as_deref() == Some("AttributeError") {
                        molt_exception_clear();
                        dec_ref_bits(_py, exc_bits);
                        return default_bits;
                    }
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                return default_bits;
            }
        }
        default_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_has_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if attr_lookup_ptr(_py, obj_ptr, name_bits).is_some() {
                    return MoltObject::from_bool(true).bits();
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    let kind_bits = molt_exception_kind(exc_bits);
                    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                    dec_ref_bits(_py, kind_bits);
                    if kind.as_deref() == Some("AttributeError") {
                        molt_exception_clear();
                        dec_ref_bits(_py, exc_bits);
                        return MoltObject::from_bool(false).bits();
                    }
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::from_bool(false).bits();
                }
                return MoltObject::from_bool(false).bits();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let bytes = string_bytes(name_ptr);
                let len = string_len(name_ptr);
                return molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits) as u64;
            }
        }
        let obj = obj_from_bits(obj_bits);
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
        attr_error(_py, type_name(_py, obj), &name) as u64
    })
}

#[no_mangle]
pub extern "C" fn molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                return del_attr_ptr(_py, obj_ptr, name_bits, &attr_name) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error(_py, type_name(_py, obj), &attr_name) as u64
        }
    })
}
