use super::*;

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
