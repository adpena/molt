use super::*;

unsafe fn classed_attr_lookup_without_dict_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_bits: u64,
    attr_bits: u64,
    allow_custom_getattribute: bool,
) -> Option<u64> {
    unsafe {
        let class_ptr = obj_from_bits(class_bits).as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let getattribute_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattribute_name,
            b"__getattribute__",
        );
        let getattribute_raw = class_attr_lookup_raw_mro(_py, class_ptr, getattribute_bits);
        let default_getattribute_bits = object_method_bits(_py, "__getattribute__");
        let use_custom_getattribute = allow_custom_getattribute
            && match (getattribute_raw, default_getattribute_bits) {
                (Some(raw_bits), Some(default_bits)) => {
                    !obj_eq(_py, obj_from_bits(raw_bits), obj_from_bits(default_bits))
                }
                (Some(_), None) => true,
                (None, _) => false,
            };
        if use_custom_getattribute
            && !obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(getattribute_bits),
            )
            && let Some(call_bits) =
                class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), getattribute_bits)
        {
            let getattr_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.getattr_name,
                b"__getattr__",
            );
            let getattr_candidate =
                !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits))
                    && class_attr_lookup_raw_mro(_py, class_ptr, getattr_bits).is_some();
            if getattr_candidate {
                traceback_suppress_enter();
            }
            exception_stack_push();
            let res_bits = call_callable1(_py, call_bits, attr_bits);
            if getattr_candidate {
                traceback_suppress_exit();
            }
            if exception_pending(_py) {
                let exc_bits = molt_exception_last_pending();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                dec_ref_bits(_py, kind_bits);
                if kind.as_deref() == Some("AttributeError") && getattr_candidate {
                    molt_exception_clear();
                    dec_ref_bits(_py, exc_bits);
                    exception_stack_pop(_py);
                    if let Some(getattr_call_bits) =
                        class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), getattr_bits)
                    {
                        let getattr_res = call_callable1(_py, getattr_call_bits, attr_bits);
                        if exception_pending(_py) {
                            return None;
                        }
                        return Some(getattr_res);
                    }
                }
                dec_ref_bits(_py, exc_bits);
                exception_stack_pop(_py);
                return None;
            }
            exception_stack_pop(_py);
            return Some(res_bits);
        }
        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
            && descriptor_is_data(_py, val_bits)
        {
            if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                return Some(bound);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        let class_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(class_name_bits),
        ) {
            inc_ref_bits(_py, class_bits);
            return Some(class_bits);
        }
        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
            if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                return Some(bound);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        let getattr_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattr_name,
            b"__getattr__",
        );
        if !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits))
            && let Some(call_bits) =
                class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), getattr_bits)
        {
            let res_bits = call_callable1(_py, call_bits, attr_bits);
            if exception_pending(_py) {
                return None;
            }
            return Some(res_bits);
        }
        None
    }
}

pub(super) unsafe fn classed_attr_lookup_without_dict(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_bits: u64,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { classed_attr_lookup_without_dict_inner(_py, obj_ptr, class_bits, attr_bits, true) }
}

pub(crate) unsafe fn type_attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, true) }
}

pub(crate) unsafe fn type_attr_lookup_ptr_default(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, false) }
}

unsafe fn type_attr_lookup_ptr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    allow_meta_custom_getattribute: bool,
) -> Option<u64> {
    unsafe {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();

        // CPython parity: type.__getattribute__ first checks the class's
        // own __dict__ for user-defined class attributes. This handles
        // attributes set via `cls.attr = value` in classmethods, metaclass
        // __init__, or module-level class attribute assignment.
        // Without this, dynamically-set attributes on user-defined classes
        // are invisible to getattr even though setattr succeeds.
        if !is_builtin_class_bits(_py, class_bits) {
            let dict_bits = class_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits)
            {
                // Invoke the descriptor protocol for classmethod/staticmethod/property
                // descriptors found in the class dict. CPython's type.__getattribute__
                // does this automatically; we must do it here too.
                if let Some(val_ptr) = obj_from_bits(val_bits).as_ptr() {
                    let val_type_id = object_type_id(val_ptr);
                    if val_type_id == TYPE_ID_CLASSMETHOD {
                        let func_bits = classmethod_func_bits(val_ptr);
                        return Some(molt_bound_method_new(func_bits, class_bits));
                    }
                    if val_type_id == TYPE_ID_STATICMETHOD {
                        let func_bits = staticmethod_func_bits(val_ptr);
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                    if val_type_id == TYPE_ID_PROPERTY {
                        // Property on a class (not instance) - return the descriptor itself.
                        inc_ref_bits(_py, val_bits);
                        return Some(val_bits);
                    }
                }
                inc_ref_bits(_py, val_bits);
                return Some(val_bits);
            }
        }

        if is_builtin_class_bits(_py, class_bits) {
            let getattribute_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.getattribute_name,
                b"__getattribute__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(getattribute_bits),
            ) && let Some(func_bits) =
                builtin_class_method_bits(_py, class_bits, "__getattribute__")
            {
                return descriptor_bind(_py, func_bits, obj_ptr, None);
            }
            let setattr_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.setattr_name,
                b"__setattr__",
            );
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(setattr_bits))
                && let Some(func_bits) = builtin_class_method_bits(_py, class_bits, "__setattr__")
            {
                return descriptor_bind(_py, func_bits, obj_ptr, None);
            }
            let delattr_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.delattr_name,
                b"__delattr__",
            );
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(delattr_bits))
                && let Some(func_bits) = builtin_class_method_bits(_py, class_bits, "__delattr__")
            {
                return descriptor_bind(_py, func_bits, obj_ptr, None);
            }
        }
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            let builtins = builtin_classes(_py);
            if class_bits == builtins.object
                && (name == "__getattribute__" || name == "__setattr__" || name == "__delattr__")
                && let Some(func_bits) = object_method_bits(_py, name.as_str())
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            if name == "__init_subclass__"
                && matches!(
                    std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                    Some("1")
                )
            {
                let builtins = builtin_classes(_py);
                eprintln!(
                    "molt init_subclass lookup class_bits=0x{:x} builtins.object=0x{:x} is_builtin={}",
                    class_bits,
                    builtins.object,
                    is_builtin_class_bits(_py, class_bits),
                );
            }
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

            // Builtin-type class surfaces that are implemented as Rust intrinsics rather than
            // being materialized in the class dict.
            //
            // CPython: str/bytes/bytearray expose `maketrans` as a staticmethod, bytes/bytearray
            // expose `fromhex` as a classmethod, and memoryview exposes `_from_flags` as a
            // staticmethod on the type object.
            let builtins = builtin_classes(_py);
            if (class_bits == builtins.str || issubclass_bits(class_bits, builtins.str))
                && name == "maketrans"
                && let Some(func_bits) = string_method_bits(_py, "maketrans")
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            if class_bits == builtins.bytes {
                if name == "fromhex" {
                    let func_bits = builtin_func_bits(
                        _py,
                        &attributes_state(_py).bytes_fromhex,
                        fn_addr!(molt_bytes_fromhex),
                        2,
                    );
                    let bound = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound);
                }
                if name == "maketrans"
                    && let Some(func_bits) = bytes_method_bits(_py, "maketrans")
                {
                    inc_ref_bits(_py, func_bits);
                    return Some(func_bits);
                }
            }
            if class_bits == builtins.bytearray {
                if name == "fromhex" {
                    let func_bits = builtin_func_bits(
                        _py,
                        &attributes_state(_py).bytearray_fromhex,
                        fn_addr!(molt_bytearray_fromhex),
                        2,
                    );
                    let bound = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound);
                }
                if name == "maketrans"
                    && let Some(func_bits) = bytearray_method_bits(_py, "maketrans")
                {
                    inc_ref_bits(_py, func_bits);
                    return Some(func_bits);
                }
            }
            if class_bits == builtins.memoryview && name == "_from_flags" {
                let func_bits = builtin_func_bits(
                    _py,
                    &attributes_state(_py).memoryview_from_flags,
                    fn_addr!(molt_memoryview_from_flags),
                    2,
                );
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            if class_bits == builtins.tuple
                && name == "__new__"
                && let Some(func_bits) = builtin_class_method_bits(_py, class_bits, "__new__")
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }

            if name == "__name__" {
                let name_bits = class_name_bits(obj_ptr);
                inc_ref_bits(_py, name_bits);
                return Some(name_bits);
            }
            if name == "__qualname__" {
                let qualname_bits = class_qualname_bits(obj_ptr);
                let bits = if qualname_bits == 0 {
                    class_name_bits(obj_ptr)
                } else {
                    qualname_bits
                };
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            if name == "__dict__" {
                let dict_bits = class_dict_bits(obj_ptr);
                let mappingproxy_bits = crate::builtins::types::mappingproxy_class_bits(_py);
                if !obj_from_bits(mappingproxy_bits).is_none() {
                    let res_bits = call_callable1(_py, mappingproxy_bits, dict_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    return Some(res_bits);
                }
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            if name == "__annotate__" && pep649_enabled(_py) {
                let mut annotate_bits = class_annotate_bits(obj_ptr);
                if annotate_bits == 0 {
                    let annotate_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotate_name,
                        b"__annotate__",
                    );
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                        && let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotate_name_bits)
                    {
                        annotate_bits = val_bits;
                        class_set_annotate_bits(_py, obj_ptr, annotate_bits);
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
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                    && let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotations_bits)
                {
                    inc_ref_bits(_py, val_bits);
                    class_set_annotations_bits(_py, obj_ptr, val_bits);
                    return Some(val_bits);
                }
                let cached = class_annotations_bits(obj_ptr);
                if cached != 0 {
                    inc_ref_bits(_py, cached);
                    return Some(cached);
                }
                let annotate_bits = class_annotate_bits(obj_ptr);
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
                class_set_annotations_bits(_py, obj_ptr, res_bits);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_set_in_place(_py, dict_ptr, annotations_bits, res_bits);
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
                if issubclass_bits(class_bits, builtins.dict)
                    && let Some(func_bits) = dict_method_bits(_py, name.as_str())
                {
                    let bound_bits = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound_bits);
                }
            }
            if is_builtin_class_bits(_py, class_bits) {
                if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, name.as_str()) {
                    if name == "__init_subclass__"
                        && matches!(
                            std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                            Some("1")
                        )
                    {
                        eprintln!("molt init_subclass builtin bits=0x{:x}", func_bits);
                    }
                    return descriptor_bind(_py, func_bits, obj_ptr, None);
                } else if name == "__init_subclass__"
                    && matches!(
                        std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                        Some("1")
                    )
                {
                    eprintln!("molt init_subclass builtin missing");
                }
            }
        }
        let meta_bits = object_class_bits(obj_ptr);
        let meta_ptr = if meta_bits != 0 {
            obj_from_bits(meta_bits).as_ptr()
        } else {
            obj_from_bits(builtin_classes(_py).type_obj).as_ptr()
        };
        let meta_ptr = match meta_ptr {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TYPE => Some(ptr),
            _ => None,
        };
        if let Some(meta_ptr) = meta_ptr {
            if allow_meta_custom_getattribute {
                let getattribute_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.getattribute_name,
                    b"__getattribute__",
                );
                let getattribute_raw = class_attr_lookup_raw_mro(_py, meta_ptr, getattribute_bits);
                // For class objects (`TYPE_ID_TYPE`), the default dispatch baseline is
                // `type.__getattribute__`, not `object.__getattribute__`.
                let default_getattribute_bits = type_method_bits(_py, "__getattribute__");
                let use_custom_getattribute = match (getattribute_raw, default_getattribute_bits) {
                    (Some(raw_bits), Some(default_bits)) => {
                        !obj_eq(_py, obj_from_bits(raw_bits), obj_from_bits(default_bits))
                    }
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                if use_custom_getattribute
                    && !obj_eq(
                        _py,
                        obj_from_bits(attr_bits),
                        obj_from_bits(getattribute_bits),
                    )
                    && let Some(call_bits) =
                        class_attr_lookup(_py, meta_ptr, meta_ptr, Some(obj_ptr), getattribute_bits)
                {
                    let getattr_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.getattr_name,
                        b"__getattr__",
                    );
                    let getattr_candidate =
                        !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits))
                            && class_attr_lookup_raw_mro(_py, meta_ptr, getattr_bits).is_some();
                    if getattr_candidate {
                        traceback_suppress_enter();
                    }
                    exception_stack_push();
                    let res_bits = call_callable1(_py, call_bits, attr_bits);
                    if getattr_candidate {
                        traceback_suppress_exit();
                    }
                    if exception_pending(_py) {
                        let exc_bits = molt_exception_last_pending();
                        let kind_bits = molt_exception_kind(exc_bits);
                        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                        dec_ref_bits(_py, kind_bits);
                        if kind.as_deref() == Some("AttributeError") && getattr_candidate {
                            molt_exception_clear();
                            dec_ref_bits(_py, exc_bits);
                            exception_stack_pop(_py);
                            if let Some(getattr_call_bits) = class_attr_lookup(
                                _py,
                                meta_ptr,
                                meta_ptr,
                                Some(obj_ptr),
                                getattr_bits,
                            ) {
                                let getattr_res = call_callable1(_py, getattr_call_bits, attr_bits);
                                if exception_pending(_py) {
                                    return None;
                                }
                                return Some(getattr_res);
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
            if let Some(meta_bits) = class_attr_lookup_raw_mro(_py, meta_ptr, attr_bits)
                && descriptor_is_data(_py, meta_bits)
            {
                return descriptor_bind(_py, meta_bits, meta_ptr, Some(obj_ptr));
            }
        }
        if let Some(class_bits) = class_attr_lookup(_py, obj_ptr, obj_ptr, None, attr_bits) {
            return Some(class_bits);
        }
        if let Some(meta_ptr) = meta_ptr
            && let Some(meta_bits) = class_attr_lookup_raw_mro(_py, meta_ptr, attr_bits)
        {
            return descriptor_bind(_py, meta_bits, meta_ptr, Some(obj_ptr));
        }
        None
    }
}
