use std::sync::OnceLock;

use super::*;

/// Cached `MOLT_TRACE_BUILTIN_TYPE` flag. `molt_builtin_type` resolves builtin
/// type objects (`int`, `str`, ...) and is on a very hot dispatch path; read
/// the env var once rather than per call (per-call `std::env::var` takes the
/// libc environ lock and heap-allocates).
#[inline]
fn trace_builtin_type_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_BUILTIN_TYPE").as_deref() == Ok("1"))
}

/// Cached `MOLT_TRACE_ISINSTANCE` flag. `molt_isinstance` runs on every
/// `isinstance()` call; read the env var once rather than per call.
#[inline]
fn trace_isinstance_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_ISINSTANCE").as_deref() == Ok("1"))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_string_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val_bits);
        let is_string = obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING });
        MoltObject::from_bool(is_string).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_new(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "class name must be str");
            }
        }
        let ptr = alloc_class_obj(_py, name_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        // `class` statements lowered via `molt_class_new` are only used on the
        // static fast-path where the metaclass is known to be `type`. Ensure the
        // new class object is an instance of `type` (CPython parity).
        unsafe {
            let builtins = builtin_classes(_py);
            let old_bits = object_class_bits(ptr);
            if old_bits != builtins.type_obj {
                if old_bits != 0 {
                    dec_ref_bits(_py, old_bits);
                }
                object_set_class_bits(_py, ptr, builtins.type_obj);
                inc_ref_bits(_py, builtins.type_obj);
            }
        }
        // Set __doc__ = None on the class dict (CPython parity).
        // Every class has a __doc__ attribute; without this, `cls.__doc__`
        // raises AttributeError which breaks libraries like six.
        unsafe {
            let dict_bits = class_dict_bits(ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                let doc_key = alloc_string(_py, b"__doc__");
                if !doc_key.is_null() {
                    dict_set_in_place(
                        _py,
                        dict_ptr,
                        MoltObject::from_ptr(doc_key).bits(),
                        MoltObject::none().bits(),
                    );
                    dec_ref_bits(_py, MoltObject::from_ptr(doc_key).bits());
                }
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_builtin_type(tag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tag = match to_i64(obj_from_bits(tag_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "builtin type tag must be int"),
        };
        let Some(bits) = builtin_type_bits(_py, tag) else {
            return raise_exception::<_>(_py, "TypeError", "unknown builtin type tag");
        };
        if trace_builtin_type_enabled() {
            eprintln!("molt builtin_type tag={} bits=0x{:x}", tag, bits);
        }
        inc_ref_bits(_py, bits);
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_of(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits = type_of_bits(_py, val_bits);
        inc_ref_bits(_py, bits);
        bits
    })
}

/// Returns the type of an object WITHOUT incrementing the refcount.
/// The type is guaranteed alive because the object holds a strong reference
/// to its type internally. This is the borrowed-reference equivalent of
/// `molt_type_of` and mirrors CPython's `Py_TYPE()` semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_type_of_borrowed(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { type_of_bits(_py, val_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_new(
    cls_bits: u64,
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "type.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "type.__new__ expects type");
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "class name must be str");
            }
        }

        let mut bases_vec: Vec<u64> = Vec::new();
        let mut bases_tuple_bits = bases_bits;
        let mut bases_owned = false;
        if obj_from_bits(bases_bits).is_none() || bases_bits == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
        } else if let Some(bases_ptr) = obj_from_bits(bases_bits).as_ptr() {
            unsafe {
                match object_type_id(bases_ptr) {
                    TYPE_ID_TUPLE => {
                        bases_vec = seq_vec_ref(bases_ptr).clone();
                    }
                    TYPE_ID_TYPE => {
                        let tuple_ptr = alloc_tuple(_py, &[bases_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                        bases_owned = true;
                        bases_vec.push(bases_bits);
                    }
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "bases must be a tuple of types",
                        );
                    }
                }
            }
        }

        if bases_vec.is_empty() {
            let builtins = builtin_classes(_py);
            let tuple_ptr = alloc_tuple(_py, &[builtins.object]);
            if tuple_ptr.is_null() {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
            bases_vec.push(builtins.object);
        }

        let class_ptr = alloc_class_obj(_py, name_bits);
        if class_ptr.is_null() {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        unsafe {
            object_set_class_bits(_py, class_ptr, cls_bits);
            inc_ref_bits(_py, cls_bits);
        }

        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        unsafe {
            let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, namespace_bits);
        }
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let mut qualname_bits = 0u64;
        let mut qualname_owned = false;
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        {
            let qualname_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.qualname_name,
                b"__qualname__",
            );
            if let Some(val_bits) = unsafe { dict_get_in_place(_py, dict_ptr, qualname_name_bits) }
            {
                qualname_bits = val_bits;
                // We're about to delete __qualname__ from the class dict; hold a strong
                // reference so we can safely move it into the class qualname slot.
                inc_ref_bits(_py, qualname_bits);
                qualname_owned = true;
                unsafe {
                    dict_del_in_place(_py, dict_ptr, qualname_name_bits);
                }
                if exception_pending(_py) {
                    if qualname_owned {
                        dec_ref_bits(_py, qualname_bits);
                    }
                    if bases_owned {
                        dec_ref_bits(_py, bases_tuple_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
            if let Some(classdictcell_bits) = attr_name_bits_from_bytes(_py, b"__classdictcell__") {
                unsafe {
                    dict_del_in_place(_py, dict_ptr, classdictcell_bits);
                }
                dec_ref_bits(_py, classdictcell_bits);
                if exception_pending(_py) {
                    if qualname_owned {
                        dec_ref_bits(_py, qualname_bits);
                    }
                    if bases_owned {
                        dec_ref_bits(_py, bases_tuple_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        if qualname_bits == 0 {
            qualname_bits = name_bits;
        }
        let qualname_obj = obj_from_bits(qualname_bits);
        let qualname_is_str = if let Some(ptr) = qualname_obj.as_ptr() {
            unsafe { object_type_id(ptr) == TYPE_ID_STRING }
        } else {
            false
        };
        if !qualname_is_str {
            let type_label = type_name(_py, qualname_obj);
            let msg = format!("type __qualname__ must be a str, not {}", type_label);
            if qualname_owned {
                dec_ref_bits(_py, qualname_bits);
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        unsafe {
            class_set_qualname_bits(_py, class_ptr, qualname_bits);
        }
        if qualname_owned {
            // Balance the strong ref we took before deleting __qualname__ from the dict.
            dec_ref_bits(_py, qualname_bits);
        }

        let _ = molt_class_set_base(class_bits, bases_tuple_bits);
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        if unsafe { !apply_class_slots_layout(_py, class_ptr) } {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        unsafe {
            crate::object::class_finish_definition(_py, class_ptr);
        }

        let mut kw_pairs: Vec<(u64, u64)> = Vec::new();
        let kwargs_obj = obj_from_bits(kwargs_bits);
        if !kwargs_obj.is_none()
            && let Some(kwargs_ptr) = kwargs_obj.as_ptr()
        {
            unsafe {
                if object_type_id(kwargs_ptr) == TYPE_ID_DICT {
                    let entries = dict_order(kwargs_ptr).clone();
                    for pair in entries.chunks(2) {
                        if pair.len() == 2 {
                            kw_pairs.push((pair[0], pair[1]));
                        }
                    }
                }
            }
        }

        let init_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.init_subclass_name,
            b"__init_subclass__",
        );
        for base_bits in bases_vec.iter().copied() {
            let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
                continue;
            };
            let Some(init_bits) =
                (unsafe { attr_lookup_ptr_allow_missing(_py, base_ptr, init_name_bits) })
            else {
                continue;
            };
            let builder_bits =
                molt_callargs_new((1 + kw_pairs.len()) as u64, kw_pairs.len() as u64);
            if builder_bits == 0 {
                dec_ref_bits(_py, init_bits);
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            unsafe {
                let _ = molt_callargs_push_pos(builder_bits, class_bits);
            }
            for (name_bits, val_bits) in kw_pairs.iter().copied() {
                unsafe {
                    let _ = molt_callargs_push_kw(builder_bits, name_bits, val_bits);
                }
            }
            let _ = molt_call_bind(init_bits, builder_bits);
            dec_ref_bits(_py, init_bits);
            if exception_pending(_py) {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
        }

        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        class_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_init(
    _cls_bits: u64,
    _name_bits: u64,
    _bases_bits: u64,
    _namespace_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = kwargs_bits;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_prepare(_cls_bits: u64, _name_bits: u64, _bases_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_mro(cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "mro expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "mro expects type");
            }
        }
        let mro = class_mro_vec(cls_bits);
        let list_ptr = alloc_list(_py, &mro);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_instancecheck(cls_bits: u64, inst_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let inst_type = type_of_bits(_py, inst_bits);
        MoltObject::from_bool(issubclass_bits(inst_type, cls_bits)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_subclasscheck(cls_bits: u64, sub_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(issubclass_bits(sub_bits, cls_bits)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isinstance(val_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let result = isinstance_runtime(_py, val_bits, class_bits);
        if trace_isinstance_enabled() {
            eprintln!(
                "molt isinstance val_type={} class_type={} result={}",
                crate::type_name(_py, obj_from_bits(val_bits)),
                crate::type_name(_py, obj_from_bits(class_bits)),
                result
            );
        }
        MoltObject::from_bool(result).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_issubclass(sub_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(sub_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "issubclass() arg 1 must be a class");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "issubclass() arg 1 must be a class",
                );
            }
        }
        let mut classes = Vec::new();
        collect_runtime_classinfo(_py, class_bits, ClassInfoProtocol::Subclass, &mut classes);
        for class_info in classes {
            match class_info {
                RuntimeClassInfo::Type(class_bits) => {
                    if issubclass_runtime(_py, sub_bits, class_bits) {
                        return MoltObject::from_bool(true).bits();
                    }
                }
                RuntimeClassInfo::Protocol(class_bits) => {
                    match runtime_classinfo_protocol_match(
                        _py,
                        class_bits,
                        sub_bits,
                        ClassInfoProtocol::Subclass,
                    ) {
                        Some(true) => return MoltObject::from_bool(true).bits(),
                        Some(false) => {}
                        None => break,
                    }
                }
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_alloc(std::mem::size_of::<u64>() as u64);
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let class_bits = builtin_classes(_py).object;
        unsafe {
            let _ = molt_object_set_class(obj_ptr as usize as u64, class_bits);
        }
        obj_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_new_bound(cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        if is_builtin_class_bits(_py, cls_bits) && cls_bits != builtins.object {
            let class_name = class_name_for_error(cls_bits);
            let msg =
                format!("object.__new__({class_name}) is not safe, use {class_name}.__new__()");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        unsafe { alloc_instance_for_class(_py, cls_ptr) }
    })
}

/// Sized variant of [`molt_object_new_bound`] â€” the codegen passes
/// the static instance payload size (in bytes, header-exclusive)
/// when the frontend carries it on the `OBJECT_NEW_BOUND` op's
/// `value` field (set from `class_info["size"]`).  The runtime
/// then skips the `class_layout_size` MRO walk + dict probe + name
/// interning entirely.
///
/// All other guards (cls_bits validity, type_id check, builtin
/// safety check) match the unsized entry point exactly.
#[unsafe(no_mangle)]
pub extern "C" fn molt_object_new_bound_sized(cls_bits: u64, payload_size_bytes: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        if is_builtin_class_bits(_py, cls_bits) && cls_bits != builtins.object {
            let class_name = class_name_for_error(cls_bits);
            let msg =
                format!("object.__new__({class_name}) is not safe, use {class_name}.__new__()");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        unsafe {
            crate::call::class_init::alloc_instance_for_class_sized(
                _py,
                cls_ptr,
                payload_size_bytes as usize,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_new_bound(cls_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "tuple.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "tuple.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        let tuple_bits = if iterable_bits == missing_bits(_py) {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        } else {
            let bits = unsafe { tuple_from_iter_bits(_py, iterable_bits) };
            let Some(bits) = bits else {
                return MoltObject::none().bits();
            };
            bits
        };
        if cls_bits == builtins.tuple {
            return tuple_bits;
        }
        let iter_is_tuple = maybe_ptr_from_bits(iterable_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE });
        if iter_is_tuple && tuple_bits == iterable_bits {
            let Some(tuple_ptr) = obj_from_bits(tuple_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let elems = unsafe { seq_vec_ref(tuple_ptr) }.clone();
            let new_ptr = alloc_tuple(_py, &elems);
            dec_ref_bits(_py, tuple_bits);
            if new_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            unsafe {
                object_set_class_bits(_py, new_ptr, cls_bits);
            }
            inc_ref_bits(_py, cls_bits);
            return new_bits;
        }
        if let Some(tuple_ptr) = obj_from_bits(tuple_bits).as_ptr() {
            unsafe {
                object_set_class_bits(_py, tuple_ptr, cls_bits);
            }
            inc_ref_bits(_py, cls_bits);
        }
        tuple_bits
    })
}

fn c3_merge(seqs: Vec<Vec<u64>>) -> Option<Vec<u64>> {
    let mut result = Vec::new();
    let mut heads = vec![0usize; seqs.len()];
    let mut tail_counts: HashMap<u64, usize> = HashMap::new();
    for seq in &seqs {
        for &value in seq.iter().skip(1) {
            *tail_counts.entry(value).or_insert(0) += 1;
        }
    }
    loop {
        let mut remaining = 0usize;
        for (idx, seq) in seqs.iter().enumerate() {
            if heads[idx] < seq.len() {
                remaining += 1;
            }
        }
        if remaining == 0 {
            return Some(result);
        }
        let mut candidate = None;
        'outer: for (seq_idx, seq) in seqs.iter().enumerate() {
            let head_idx = heads[seq_idx];
            if head_idx >= seq.len() {
                continue;
            }
            let head = seq[head_idx];
            if tail_counts.get(&head).copied().unwrap_or(0) == 0 {
                candidate = Some(head);
                break 'outer;
            }
        }
        let cand = candidate?;
        result.push(cand);
        for (idx, seq) in seqs.iter().enumerate() {
            let head_idx = heads[idx];
            if head_idx < seq.len() && seq[head_idx] == cand {
                heads[idx] += 1;
                let next_head_idx = heads[idx];
                if next_head_idx < seq.len() {
                    let next_head = seq[next_head_idx];
                    if let Some(count) = tail_counts.get_mut(&next_head) {
                        if *count <= 1 {
                            tail_counts.remove(&next_head);
                        } else {
                            *count -= 1;
                        }
                    }
                }
            }
        }
    }
}

fn compute_mro(class_bits: u64, bases: &[u64]) -> Option<Vec<u64>> {
    let mut seqs = Vec::with_capacity(bases.len() + 1);
    for base in bases {
        seqs.push(class_mro_vec(*base));
    }
    seqs.push(bases.to_vec());
    let mut out = vec![class_bits];
    let merged = c3_merge(seqs)?;
    out.extend(merged);
    Some(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_set_base(class_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
        }
        let mut bases_vec = Vec::new();
        let bases_owned;
        let bases_bits = if obj_from_bits(base_bits).is_none() || base_bits == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_owned = true;
            MoltObject::from_ptr(tuple_ptr).bits()
        } else {
            let base_obj = obj_from_bits(base_bits);
            let Some(base_ptr) = base_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "base must be a type object or tuple of types",
                );
            };
            unsafe {
                match object_type_id(base_ptr) {
                    TYPE_ID_TYPE => {
                        bases_vec.push(base_bits);
                        let tuple_ptr = alloc_tuple(_py, &[base_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        bases_owned = true;
                        MoltObject::from_ptr(tuple_ptr).bits()
                    }
                    TYPE_ID_TUPLE => {
                        for item in seq_vec_ref(base_ptr).iter() {
                            bases_vec.push(*item);
                        }
                        let tuple_ptr = alloc_tuple(_py, &bases_vec);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        bases_owned = true;
                        MoltObject::from_ptr(tuple_ptr).bits()
                    }
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "base must be a type object or tuple of types",
                        );
                    }
                }
            }
        };

        if bases_vec.is_empty() {
            bases_vec = class_bases_vec(bases_bits);
        }
        let mut seen = HashSet::new();
        for base in &bases_vec {
            if !seen.insert(*base) {
                let name = class_name_for_error(*base);
                let msg = format!("duplicate base class {name}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        for base in bases_vec.iter() {
            let base_obj = obj_from_bits(*base);
            let Some(base_ptr) = base_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "base must be a type object");
            };
            unsafe {
                if object_type_id(base_ptr) != TYPE_ID_TYPE {
                    return raise_exception::<_>(_py, "TypeError", "base must be a type object");
                }
                if base_ptr == class_ptr {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "class cannot inherit from itself",
                    );
                }
            }
        }

        let mro = match compute_mro(class_bits, &bases_vec) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "Cannot create a consistent method resolution order (MRO) for bases",
                );
            }
        };
        let mro_ptr = alloc_tuple(_py, &mro);
        if mro_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mro_bits = MoltObject::from_ptr(mro_ptr).bits();

        unsafe {
            let old_bases = class_bases_bits(class_ptr);
            let old_mro = class_mro_bits(class_ptr);
            let mut bases_updated = false;
            let mut mro_updated = false;
            if old_bases != bases_bits {
                dec_ref_bits(_py, old_bases);
                if !bases_owned {
                    inc_ref_bits(_py, bases_bits);
                }
                class_set_bases_bits(class_ptr, bases_bits);
                bases_updated = true;
            }
            if old_mro != mro_bits {
                dec_ref_bits(_py, old_mro);
                class_set_mro_bits(class_ptr, mro_bits);
                mro_updated = true;
            }
            let dict_bits = class_dict_bits(class_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                let bases_name =
                    intern_static_name(_py, &runtime_state(_py).interned.bases_name, b"__bases__");
                let mro_name =
                    intern_static_name(_py, &runtime_state(_py).interned.mro_name, b"__mro__");
                dict_set_in_place(_py, dict_ptr, bases_name, bases_bits);
                dict_set_in_place(_py, dict_ptr, mro_name, mro_bits);
            }
            if bases_owned && !bases_updated {
                dec_ref_bits(_py, bases_bits);
            }
            if !mro_updated {
                dec_ref_bits(_py, mro_bits);
            }
            if bases_updated || mro_updated {
                crate::object::class_refresh_finalizer_flag(_py, class_ptr);
                class_bump_layout_version(class_ptr);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_apply_set_name(class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_set_name = matches!(
            std::env::var("MOLT_TRACE_SET_NAME").ok().as_deref(),
            Some("1")
        );
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            if !apply_class_slots_layout(_py, class_ptr) {
                return MoltObject::none().bits();
            }
            let dict_bits = class_dict_bits(class_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return MoltObject::none().bits();
            }
            let entries = dict_order(dict_ptr).clone();
            let set_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.set_name_method,
                b"__set_name__",
            );
            for pair in entries.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                let name_bits = pair[0];
                let val_bits = pair[1];
                // `entries` is a borrowed snapshot of the class dict.  A user
                // `__set_name__` hook can mutate that dict, including deleting
                // the descriptor currently being initialized, so the apply loop
                // must own the key/value pair across arbitrary hook execution.
                inc_ref_bits(_py, name_bits);
                inc_ref_bits(_py, val_bits);
                let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
                    dec_ref_bits(_py, val_bits);
                    dec_ref_bits(_py, name_bits);
                    continue;
                };
                if let Some(set_name) = attr_lookup_ptr_allow_missing(_py, val_ptr, set_name_bits) {
                    if trace_set_name {
                        let class_name = class_name_for_error(class_bits);
                        let key = string_obj_to_owned(obj_from_bits(name_bits))
                            .unwrap_or_else(|| "<non-str>".to_string());
                        let val_type_id = object_type_id(val_ptr);
                        let (set_name_type_id, set_name_type) =
                            if let Some(ptr) = obj_from_bits(set_name).as_ptr() {
                                (object_type_id(ptr), type_name(_py, obj_from_bits(set_name)))
                            } else {
                                (0, type_name(_py, obj_from_bits(set_name)))
                            };
                        eprintln!(
                            "molt set_name: class={} key={} val_type_id={} set_name_type_id={} set_name_type={}",
                            class_name, key, val_type_id, set_name_type_id, set_name_type,
                        );
                    }
                    let _ = call_callable2(_py, set_name, class_bits, name_bits);
                    dec_ref_bits(_py, set_name);
                }
                dec_ref_bits(_py, val_bits);
                dec_ref_bits(_py, name_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_layout_version(class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            MoltObject::from_int(class_layout_version_bits(class_ptr) as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_set_layout_version(class_bits: u64, version_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            let version = match to_i64(obj_from_bits(version_bits)) {
                Some(val) if val >= 0 => val as u64,
                _ => return raise_exception::<_>(_py, "TypeError", "layout version must be int"),
            };
            class_set_layout_version_bits(class_ptr, version);
            crate::bump_type_version();
        }
        MoltObject::none().bits()
    })
}

unsafe fn max_slot_end_from_offsets_dict(offsets_ptr: *mut u8) -> usize {
    unsafe {
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return 0;
        }
        let mut max_end = 0usize;
        let entries = dict_order(offsets_ptr).clone();
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            if let Some(offset) = obj_from_bits(pair[1]).as_int()
                && offset >= 0
            {
                let end = (offset as usize).saturating_add(std::mem::size_of::<u64>());
                if end > max_end {
                    max_end = end;
                }
            }
        }
        max_end
    }
}

unsafe fn merge_class_layout_metadata(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    offsets_bits: u64,
    size_bits: u64,
) -> Result<(), u64> {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return Ok(());
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return Ok(());
        }

        let offsets_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let layout_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );

        let mut merged_offsets_ptr: *mut u8 = std::ptr::null_mut();
        if !obj_from_bits(offsets_bits).is_none() {
            let Some(source_offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict or None",
                ));
            };
            if object_type_id(source_offsets_ptr) != TYPE_ID_DICT {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict or None",
                ));
            }
            let mut target_offsets_bits =
                dict_get_in_place(_py, dict_ptr, offsets_name_bits).unwrap_or(0);
            if obj_from_bits(target_offsets_bits).is_none() || target_offsets_bits == 0 {
                let new_ptr = alloc_dict_with_pairs(_py, &[]);
                if new_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                target_offsets_bits = MoltObject::from_ptr(new_ptr).bits();
                dict_set_in_place(_py, dict_ptr, offsets_name_bits, target_offsets_bits);
            }
            let Some(target_offsets_ptr) = obj_from_bits(target_offsets_bits).as_ptr() else {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict",
                ));
            };
            if object_type_id(target_offsets_ptr) != TYPE_ID_DICT {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict",
                ));
            }
            let entries = dict_order(source_offsets_ptr).clone();
            for pair in entries.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                if dict_get_in_place(_py, target_offsets_ptr, pair[0]).is_some() {
                    continue;
                }
                dict_set_in_place(_py, target_offsets_ptr, pair[0], pair[1]);
            }
            merged_offsets_ptr = target_offsets_ptr;
        } else if let Some(existing_offsets_bits) =
            dict_get_in_place(_py, dict_ptr, offsets_name_bits)
            && let Some(existing_offsets_ptr) = obj_from_bits(existing_offsets_bits).as_ptr()
            && object_type_id(existing_offsets_ptr) == TYPE_ID_DICT
        {
            merged_offsets_ptr = existing_offsets_ptr;
        }

        let builtins = builtin_classes(_py);
        let reserved_tail = if issubclass_bits(class_bits, builtins.dict) {
            2 * std::mem::size_of::<u64>()
        } else {
            std::mem::size_of::<u64>()
        };
        let mut layout_size = 0usize;
        if let Some(existing_size_bits) = dict_get_in_place(_py, dict_ptr, layout_name_bits)
            && let Some(existing_size) = obj_from_bits(existing_size_bits).as_int()
            && existing_size > 0
        {
            layout_size = existing_size as usize;
        }
        let hinted_size = match to_i64(obj_from_bits(size_bits)) {
            Some(value) if value >= 0 => value as usize,
            _ => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_layout_size__ must be int",
                ));
            }
        };
        layout_size = layout_size.max(hinted_size);
        if !merged_offsets_ptr.is_null() {
            let required =
                max_slot_end_from_offsets_dict(merged_offsets_ptr).saturating_add(reserved_tail);
            layout_size = layout_size.max(required);
        }
        if layout_size == 0 {
            layout_size = reserved_tail.max(std::mem::size_of::<u64>());
        }
        let layout_bits = MoltObject::from_int(layout_size as i64).bits();
        dict_set_in_place(_py, dict_ptr, layout_name_bits, layout_bits);
        if !apply_class_slots_layout(_py, class_ptr) {
            return Err(MoltObject::none().bits());
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_merge_layout(
    class_bits: u64,
    offsets_bits: u64,
    size_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class layout merge expects type");
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class layout merge expects type");
            }
            match merge_class_layout_metadata(_py, class_ptr, offsets_bits, size_bits) {
                Ok(()) => MoltObject::none().bits(),
                Err(bits) => bits,
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr_bits: u64, class_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = obj_ptr_bits as usize as *mut u8;
            if obj_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let header = header_from_obj_ptr(obj_ptr);
            if crate::object::object_poll_fn(obj_ptr) != 0 {
                return raise_exception::<_>(_py, "TypeError", "cannot set class on async object");
            }
            if object_type_id(obj_ptr) == TYPE_ID_DATACLASS {
                return dataclass_set_class_raw(_py, obj_ptr, class_bits);
            }
            if class_bits != 0 {
                let class_obj = obj_from_bits(class_bits);
                let Some(class_ptr) = class_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    return MoltObject::none().bits();
                }
            }
            let skip_class_ref = ((*header).flags & HEADER_FLAG_SKIP_CLASS_DECREF) != 0;
            let old_bits = object_class_bits(obj_ptr);
            if old_bits != 0 && !skip_class_ref {
                dec_ref_bits(_py, old_bits);
            }
            object_set_class_bits(_py, obj_ptr, class_bits);
            if class_bits != 0 && !skip_class_ref {
                inc_ref_bits(_py, class_bits);
            }
            MoltObject::none().bits()
        })
    }
}
