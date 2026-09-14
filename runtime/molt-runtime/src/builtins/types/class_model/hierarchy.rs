use super::*;
use crate::TYPE_ID_OBJECT;
use crate::object::seq_access::snapshot;

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

/// Validate class-independent base layout facts before dynamic type allocation.
/// The static base setter consumes this same authority; duplicate-base/MRO
/// rejection remains a postallocation type-construction phase.
pub(crate) fn prepare_class_base_layout(
    _py: &PyToken<'_>,
    bases: &[u64],
    class_ptr: Option<*mut u8>,
) -> Option<(
    u32,
    crate::object::ObjectShapeId,
    molt_obj_model::ExceptionLayoutRoot,
)> {
    let mut inherited_instance_type_id = TYPE_ID_OBJECT;
    let mut inherited_instance_shape = crate::object::ObjectShapeId::Plain;
    let mut inherited_exception_layout_root = molt_obj_model::ExceptionLayoutRoot::Base;
    for base in bases.iter() {
        let base_obj = obj_from_bits(*base);
        let Some(base_ptr) = base_obj.as_ptr() else {
            raise_exception::<()>(_py, "TypeError", "base must be a type object");
            return None;
        };
        unsafe {
            if object_type_id(base_ptr) != TYPE_ID_TYPE {
                raise_exception::<()>(_py, "TypeError", "base must be a type object");
                return None;
            }
            if crate::object::class_is_not_base(_py, base_ptr) {
                let name = class_name_for_error(*base);
                raise_exception::<()>(
                    _py,
                    "TypeError",
                    &format!("type '{name}' is not an acceptable base type"),
                );
                return None;
            }
            if Some(base_ptr) == class_ptr {
                raise_exception::<()>(_py, "TypeError", "class cannot inherit from itself");
                return None;
            }
            let base_instance_type_id = crate::object::class_instance_type_id(base_ptr);
            if base_instance_type_id != TYPE_ID_OBJECT {
                if inherited_instance_type_id != TYPE_ID_OBJECT
                    && inherited_instance_type_id != base_instance_type_id
                {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "multiple bases define conflicting native instance kinds",
                    );
                    return None;
                }
                inherited_instance_type_id = base_instance_type_id;
            }
            let base_layout_root = crate::object::class_exception_layout_root(base_ptr);
            if base_layout_root != molt_obj_model::ExceptionLayoutRoot::Base {
                if inherited_exception_layout_root != molt_obj_model::ExceptionLayoutRoot::Base
                    && inherited_exception_layout_root != base_layout_root
                {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "multiple bases have instance lay-out conflict",
                    );
                    return None;
                }
                inherited_exception_layout_root = base_layout_root;
            }
            let base_instance_shape = crate::object::class_instance_shape_id(base_ptr);
            if base_instance_shape != crate::object::ObjectShapeId::Plain {
                if inherited_instance_shape != crate::object::ObjectShapeId::Plain
                    && inherited_instance_shape != base_instance_shape
                {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "multiple bases define conflicting native payload shapes",
                    );
                    return None;
                }
                inherited_instance_shape = base_instance_shape;
            }
        }
    }
    Some((
        inherited_instance_type_id,
        inherited_instance_shape,
        inherited_exception_layout_root,
    ))
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
            if crate::object::class_definition_is_finished(class_ptr) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "class bases are immutable after definition",
                );
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
                        let Some(bases) =
                            snapshot(_py, base_ptr, "class base tuple allocation failed")
                        else {
                            return MoltObject::none().bits();
                        };
                        bases_vec.extend_from_slice(&bases);
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
        let Some((
            inherited_instance_type_id,
            inherited_instance_shape,
            inherited_exception_layout_root,
        )) = prepare_class_base_layout(_py, &bases_vec, Some(class_ptr))
        else {
            return MoltObject::none().bits();
        };
        if !unsafe {
            crate::object::class_can_inherit_instance_shape_id(class_ptr, inherited_instance_shape)
        } {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "inherited native payload shape conflicts with class layout",
            );
        }
        if !unsafe {
            crate::object::class_can_inherit_instance_type_id(class_ptr, inherited_instance_type_id)
        } {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "inherited native instance kind conflicts with class layout",
            );
        }
        if !unsafe {
            crate::object::class_can_inherit_exception_layout_root(
                class_ptr,
                inherited_exception_layout_root,
            )
        } {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "inherited exception payload conflicts with class layout",
            );
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
            use crate::object::class_storage::ClassReferenceSlot;
            if !bases_owned {
                inc_ref_bits(_py, bases_bits);
            }
            // Adopt both new edges before releasing either old one. Keep the
            // displaced owners through namespace and layout publication too:
            // replacing the dictionary projections may otherwise trigger a
            // finalizer observing a half-published hierarchy.
            let old_bases = ClassReferenceSlot::Bases.exchange_owned(class_ptr, bases_bits);
            let old_mro = ClassReferenceSlot::Mro.exchange_owned(class_ptr, mro_bits);
            let bases_updated = old_bases != bases_bits;
            let mro_updated = old_mro != mro_bits;
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
            if bases_updated || mro_updated {
                let published = crate::object::class_inherit_instance_type_id(
                    class_ptr,
                    inherited_instance_type_id,
                );
                debug_assert!(
                    published,
                    "validated instance-kind publication must succeed"
                );
                let shape_published = crate::object::class_inherit_instance_shape_id(
                    class_ptr,
                    inherited_instance_shape,
                );
                debug_assert!(
                    shape_published,
                    "validated instance-shape publication must succeed"
                );
                let exception_layout_published = crate::object::class_inherit_exception_layout_root(
                    class_ptr,
                    inherited_exception_layout_root,
                );
                debug_assert!(
                    exception_layout_published,
                    "validated exception-layout publication must succeed"
                );
                class_bump_layout_version(class_ptr);
            }
            dec_ref_bits(_py, old_bases);
            dec_ref_bits(_py, old_mro);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_apply_set_name(class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            // The public finalization entrypoint includes slot layout. The
            // canonical type constructor calls the descriptor phase directly
            // because it has already established layout and published cells.
            if apply_class_slots_layout(_py, class_ptr) {
                class_apply_descriptor_names(_py, class_ptr);
            }
        }
        MoltObject::none().bits()
    })
}

/// Apply descriptor names in snapshot order through special-method lookup.
/// The caller has established layout and published both compiler cells.
pub(crate) unsafe fn class_apply_descriptor_names(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let trace_set_name = matches!(
            std::env::var("MOLT_TRACE_SET_NAME").ok().as_deref(),
            Some("1")
        );
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return false;
        };
        let entries = dict_order(dict_ptr).clone();
        // Retain every pair before the first descriptor can delete a later one.
        let snapshot = alloc_tuple(_py, &entries);
        if snapshot.is_null() {
            return false;
        }
        let _snapshot_owner = crate::PtrDropGuard::new(snapshot);
        for pair in entries.chunks_exact(2) {
            let name_bits = pair[0];
            let value_bits = pair[1];
            let Some(set_name) =
                crate::builtins::attr::lookup_special_method(_py, value_bits, b"__set_name__")
            else {
                if exception_pending(_py) {
                    return false;
                }
                continue;
            };
            if trace_set_name {
                let class_name = class_name_for_error(class_bits);
                let key = string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<non-str>".to_string());
                let value_type_id = obj_from_bits(value_bits)
                    .as_ptr()
                    .map(|ptr| object_type_id(ptr))
                    .unwrap_or(0);
                let set_name_type_id = obj_from_bits(set_name)
                    .as_ptr()
                    .map(|ptr| object_type_id(ptr))
                    .unwrap_or(0);
                let set_name_type = type_name(_py, obj_from_bits(set_name));
                eprintln!(
                    "molt set_name: class={} key={} val_type_id={} set_name_type_id={} set_name_type={}",
                    class_name, key, value_type_id, set_name_type_id, set_name_type,
                );
            }
            let result = call_callable2(_py, set_name, class_bits, name_bits);
            crate::call::discard_owned_call_result(_py, result);
            dec_ref_bits(_py, set_name);
            if exception_pending(_py) {
                class_set_name_error_note(_py, class_bits, name_bits, value_bits);
                return false;
            }
        }
        true
    }
}

// All pinned reference interpreters (3.12.13/3.13.11/3.14.3) preserve the
// descriptor exception and append this note. A failure while constructing
// the note instead chains the original exception as its context.
unsafe fn class_set_name_error_note(
    _py: &PyToken<'_>,
    class_bits: u64,
    name_bits: u64,
    value_bits: u64,
) {
    use crate::builtins::exceptions::{ExceptionFieldSlot, exception_replace_field_bits};
    let Some(original) = crate::exception_last_bits_noinc(_py) else {
        return;
    };
    inc_ref_bits(_py, original);
    crate::molt_exception_clear();
    let name_repr_bits = crate::molt_repr_builtin(name_bits);
    if !exception_pending(_py) {
        let name_repr = string_obj_to_owned(obj_from_bits(name_repr_bits));
        if let Some(name_repr) = name_repr {
            let descriptor_name: String = type_name(_py, obj_from_bits(value_bits))
                .chars()
                .take(100)
                .collect();
            let class_name: String = class_name_for_error(class_bits).chars().take(100).collect();
            let note = format!(
                "Error calling __set_name__ on '{}' instance {} in '{}'",
                descriptor_name, name_repr, class_name,
            );
            let note_ptr = alloc_string(_py, note.as_bytes());
            if !note_ptr.is_null() {
                let note_bits = MoltObject::from_ptr(note_ptr).bits();
                let result =
                    crate::builtins::exceptions::molt_exception_add_note(original, note_bits);
                crate::call::discard_owned_call_result(_py, result);
                dec_ref_bits(_py, note_bits);
            }
        }
    }
    dec_ref_bits(_py, name_repr_bits);
    if let Some(note_error) = crate::exception_last_bits_noinc(_py) {
        if let Err(message) =
            exception_replace_field_bits(_py, note_error, ExceptionFieldSlot::Context, original)
        {
            let _ = raise_exception::<u64>(_py, "RuntimeError", message);
        }
    } else {
        crate::molt_exception_set_last(original);
    }
    dec_ref_bits(_py, original);
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

        let reserved_prefix = crate::object::class_reserved_layout_prefix(class_ptr);
        let reserved_tail = crate::object::class_reserved_layout_tail(_py, class_ptr);
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
            let required = max_slot_end_from_offsets_dict(merged_offsets_ptr)
                .max(reserved_prefix)
                .saturating_add(reserved_tail);
            layout_size = layout_size.max(required);
        }
        layout_size = layout_size.max(
            reserved_prefix
                .saturating_add(reserved_tail)
                .max(std::mem::size_of::<u64>()),
        );
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
