use super::*;
use crate::TYPE_ID_OBJECT;
use crate::object::seq_access::snapshot;

fn compute_mro(class_bits: u64, bases: &[u64]) -> Option<Vec<u64>> {
    let mut seqs = Vec::with_capacity(bases.len() + 1);
    for base in bases {
        seqs.push(class_mro_vec(*base));
    }
    seqs.push(bases.to_vec());
    let mut out = vec![class_bits];
    let merged = molt_obj_model::hierarchy::c3_merge(&seqs)?;
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
    for &base in bases {
        let Some(base_ptr) = obj_from_bits(base).as_ptr() else {
            raise_exception::<()>(_py, "TypeError", "base must be a type object");
            return None;
        };
        unsafe {
            if object_type_id(base_ptr) != TYPE_ID_TYPE {
                raise_exception::<()>(_py, "TypeError", "base must be a type object");
                return None;
            }
            if crate::object::class_is_not_base(_py, base_ptr) {
                let name = class_name_for_error(base);
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
        }
    }
    match unsafe { crate::object::class_layout::select_best_base(_py, bases) } {
        Ok(Some(best)) => Some((
            best.native.type_id,
            best.native.shape,
            best.native.exception,
        )),
        Ok(None) => Some((
            TYPE_ID_OBJECT,
            crate::object::ObjectShapeId::Plain,
            molt_obj_model::ExceptionLayoutRoot::Base,
        )),
        Err(()) => None,
    }
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
            if crate::object::class_definition_is_finished(class_ptr)
                || crate::object::layout::class_slot_declaration_bits(class_ptr) != 0
            {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "class bases are immutable after slot admission",
                );
            }
        }
        inc_ref_bits(_py, class_bits);
        let _class_owner = crate::PtrDropGuard::new(class_ptr);
        let original_bases = unsafe { class_bases_bits(class_ptr) };
        if original_bases != 0 {
            inc_ref_bits(_py, original_bases);
        }
        let _original_bases_owner = obj_from_bits(original_bases)
            .as_ptr()
            .map(crate::PtrDropGuard::new);
        let mut bases_vec = Vec::new();
        let bases_bits = if obj_from_bits(base_bits).is_none() || base_bits == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
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

        let mut bases_owner = crate::PtrDropGuard::new(
            obj_from_bits(bases_bits)
                .as_ptr()
                .expect("owned base tuple"),
        );
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

        if unsafe {
            class_bases_bits(class_ptr) != original_bases
                || crate::object::class_definition_is_finished(class_ptr)
                || crate::object::layout::class_slot_declaration_bits(class_ptr) != 0
        } {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "class bases changed during hierarchy admission",
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
            let bases_name =
                intern_static_name(_py, &runtime_state(_py).interned.bases_name, b"__bases__");
            let mro_name =
                intern_static_name(_py, &runtime_state(_py).interned.mro_name, b"__mro__");
            let Some(mut publication) =
                crate::builtins::attributes::TypeMutation::prepare(_py, class_ptr, bases_name)
            else {
                dec_ref_bits(_py, mro_bits);
                return MoltObject::none().bits();
            };
            let mut retired = Vec::with_capacity(2);
            // Adopt both new edges before releasing either old one. Keep the
            // displaced owners through namespace and layout publication too:
            // replacing the dictionary projections may otherwise trigger a
            // finalizer observing a half-published hierarchy.
            let old_bases = ClassReferenceSlot::Bases.exchange_owned(class_ptr, bases_bits);
            bases_owner.release();
            let old_mro = ClassReferenceSlot::Mro.exchange_owned(class_ptr, mro_bits);
            let bases_updated = old_bases != bases_bits;
            let mro_updated = old_mro != mro_bits;
            let dict_bits = class_dict_bits(class_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                for (name, value) in [(bases_name, bases_bits), (mro_name, mro_bits)] {
                    match crate::object::ops::dict_set_deferred(_py, dict_ptr, name, value) {
                        Ok(previous) => retired.push(previous),
                        Err(()) => break,
                    }
                }
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
            }
            // Even a failed later projection write must invalidate a committed
            // hierarchy before the first displaced owner can reenter Python.
            if exception_pending(_py) {
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    publication.publish();
                });
            } else {
                publication.publish();
            }
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                drop(retired);
                dec_ref_bits(_py, old_bases);
                dec_ref_bits(_py, old_mro);
            });
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
            if crate::object::class_finish_definition(_py, class_ptr).is_ok() {
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
        for pair in entries.as_chunks::<2>().0 {
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

/// Merge construction inputs only. A finished class's physical record is
/// immutable; explicit callers must agree with that record instead of resizing
/// it or reconstructing a competing extent from namespace dictionaries.
unsafe fn merge_class_layout_metadata(
    py: &PyToken<'_>,
    class: *mut u8,
    offsets_bits: u64,
    size_bits: u64,
) -> Result<(), u64> {
    unsafe {
        let hinted_size = to_i64(obj_from_bits(size_bits))
            .filter(|&size| size >= 0)
            .and_then(|size| usize::try_from(size).ok())
            .ok_or_else(|| {
                raise_exception::<u64>(py, "TypeError", "__molt_layout_size__ must be int")
            })?;
        let source = if obj_from_bits(offsets_bits).is_none() {
            None
        } else {
            Some(
                obj_from_bits(offsets_bits)
                    .as_ptr()
                    .filter(|&ptr| object_type_id(ptr) == TYPE_ID_DICT)
                    .ok_or_else(|| {
                        raise_exception::<u64>(
                            py,
                            "TypeError",
                            "__molt_field_offsets__ must be dict or None",
                        )
                    })?,
            )
        };
        if let Some(source) = source {
            inc_ref_bits(py, offsets_bits);
            let _source_owner = crate::PtrDropGuard::new(source);
            if crate::object::class_definition_is_finished(class) {
                let size = crate::object::layout::class_cached_layout_size(class).unwrap();
                let tail = crate::object::class_reserved_layout_tail(py, class);
                crate::object::validate_class_field_offsets(
                    py,
                    source,
                    0,
                    size.saturating_sub(tail),
                )
                .map_err(|()| MoltObject::none().bits())?;
                if hinted_size > size
                    || dict_order(source).as_chunks::<2>().0.iter().any(|pair| {
                        crate::builtins::attr::class_field_offset(py, class, pair[0])
                            != obj_from_bits(pair[1])
                                .as_int()
                                .and_then(|offset| usize::try_from(offset).ok())
                    })
                {
                    return Err(raise_exception::<u64>(
                        py,
                        "ValueError",
                        "class layout hint disagrees with sealed physical storage",
                    ));
                }
                return Ok(());
            }
        }
        // A size-only dataclass/compiler hint describes its logical vector,
        // not an additional physical storage claim on an already sealed class.
        if crate::object::class_definition_is_finished(class) {
            return Ok(());
        }
        let Some(namespace) = obj_from_bits(class_dict_bits(class))
            .as_ptr()
            .filter(|&ptr| object_type_id(ptr) == TYPE_ID_DICT)
        else {
            return Ok(());
        };
        inc_ref_bits(py, MoltObject::from_ptr(namespace).bits());
        let _namespace_owner = crate::PtrDropGuard::new(namespace);
        let offsets_name = intern_static_name(
            py,
            &runtime_state(py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let size_name = intern_static_name(
            py,
            &runtime_state(py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if let Some(source) = source {
            inc_ref_bits(py, offsets_bits);
            let _source_owner = crate::PtrDropGuard::new(source);
            crate::object::validate_class_field_offsets(py, source, 0, usize::MAX)
                .map_err(|()| MoltObject::none().bits())?;
            let target_bits = dict_get_in_place(py, namespace, offsets_name)
                .filter(|&bits| !obj_from_bits(bits).is_none());
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            let target = if let Some(bits) = target_bits {
                let target = obj_from_bits(bits)
                    .as_ptr()
                    .filter(|&ptr| object_type_id(ptr) == TYPE_ID_DICT)
                    .ok_or_else(|| {
                        raise_exception::<u64>(
                            py,
                            "TypeError",
                            "__molt_field_offsets__ must be dict",
                        )
                    })?;
                inc_ref_bits(py, bits);
                target
            } else {
                let target = alloc_dict_with_pairs(py, &[]);
                if target.is_null() {
                    return Err(MoltObject::none().bits());
                }
                dict_set_in_place(
                    py,
                    namespace,
                    offsets_name,
                    MoltObject::from_ptr(target).bits(),
                );
                target
            };
            let _target_owner = crate::PtrDropGuard::new(target);
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            crate::object::validate_class_field_offsets(py, target, 0, usize::MAX)
                .map_err(|()| MoltObject::none().bits())?;
            let entries = dict_order(source).clone();
            for pair in entries.as_chunks::<2>().0 {
                if let Some(prior) = dict_get_in_place(py, target, pair[0]) {
                    if prior != pair[1] {
                        return Err(raise_exception::<u64>(
                            py,
                            "ValueError",
                            "conflicting explicit class field offsets",
                        ));
                    }
                } else {
                    dict_set_in_place(py, target, pair[0], pair[1]);
                }
                if exception_pending(py) {
                    return Err(MoltObject::none().bits());
                }
            }
        }
        let previous = dict_get_in_place(py, namespace, size_name)
            .and_then(|bits| obj_from_bits(bits).as_int())
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(0);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let size = i64::try_from(previous.max(hinted_size)).map_err(|_| {
            raise_exception::<u64>(py, "OverflowError", "class instance layout is too large")
        })?;
        dict_set_in_place(py, namespace, size_name, MoltObject::from_int(size).bits());
        if exception_pending(py) {
            Err(MoltObject::none().bits())
        } else {
            Ok(())
        }
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
