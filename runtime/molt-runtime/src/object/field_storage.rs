//! One owner for ordinary instance attributes: inline until dictionary exposure,
//! dictionary-backed afterwards. Declared and intrinsic slots never move.
//! Function public dictionaries use this same physical owner, staged
//! publication, validation, and retirement protocol. Typed callable metadata
//! lives in function_metadata and never depends on dictionary contents.

pub(crate) use super::class_layout::ClassField as InstanceField;
use crate::*;
use std::mem::size_of;

#[path = "field_storage_debug.rs"]
pub(crate) mod debug;

/// The same layout traversal serves backing transitions, GC, and serialization.
/// Concrete typed rows retain every physical owner, including hidden inherited
/// slots. The pinned class record outlives callback-capable visits.
pub(crate) unsafe fn for_each_instance_field(
    py: &PyToken<'_>,
    object: *mut u8,
    class: *mut u8,
    visit: &mut dyn FnMut(InstanceField, *mut u64),
) {
    unsafe {
        if object_type_id(object) == TYPE_ID_DATACLASS {
            let desc = dataclass_desc_ptr(object);
            let fields = dataclass_fields_ptr(object);
            if desc.is_null() || fields.is_null() {
                return;
            }
            // Descriptor keys and slot classes are immutable after publication.
            // No mutable Vec borrow survives a callback.
            assert_eq!(
                (*fields).len(),
                (*desc).field_layout.len(),
                "dataclass projection must cover every owner"
            );
            let count = (*fields).len();
            for index in 0..count {
                visit(
                    InstanceField {
                        name: (&(*desc).field_layout)[index].name,
                        offset: index * size_of::<u64>(),
                        kind: (&(*desc).field_layout)[index].kind,
                    },
                    (*fields).as_mut_ptr().add(index),
                );
            }
            return;
        }
        if !super::object_has_class_shape(object) && !super::native_instance::has_fields(object) {
            return;
        }
        let extent =
            super::native_instance::field_payload_size(object).saturating_sub(size_of::<u64>());
        for_each_class_field(py, class, extent, &mut |field| {
            visit(
                field,
                super::native_instance::field_base(object)
                    .add(field.offset)
                    .cast(),
            );
        });
    }
}

/// Heap and stack construction share the same empty-field representation.
/// The missing singleton is immortal: initializing it neither retains an owner
/// nor sets HAS_PTRS. Inline readers must admit the loaded word as an immediate
/// before returning it; scalar writers can replace the empty word directly.
pub(crate) unsafe fn initialize_fields(
    py: &PyToken<'_>,
    object: *mut u8,
    class: *mut u8,
    payload_bytes: usize,
) -> Result<(), ()> {
    unsafe {
        let missing = missing_bits(py);
        if obj_from_bits(missing).as_ptr().is_none() {
            if !exception_pending(py) {
                raise_exception::<()>(py, "MemoryError", "empty field singleton allocation failed");
            }
            return Err(());
        }
        let extent = payload_bytes.saturating_sub(size_of::<u64>());
        for_each_class_field(py, class, extent, &mut |field| {
            *super::native_instance::field_base_for_class(object, class)
                .add(field.offset)
                .cast::<u64>() = missing;
        });
        if exception_pending(py) {
            Err(())
        } else {
            Ok(())
        }
    }
}

unsafe fn for_each_class_field(
    py: &PyToken<'_>,
    class: *mut u8,
    field_extent: usize,
    visit: &mut dyn FnMut(InstanceField),
) {
    unsafe {
        super::class_layout::for_each_field(py, class, &mut |field| {
            assert!(
                field.offset % size_of::<u64>() == 0
                    && field
                        .offset
                        .checked_add(size_of::<u64>())
                        .is_some_and(|end| end <= field_extent),
                "sealed physical field must fit the instance extent"
            );
            visit(field);
        });
    }
}

pub(crate) unsafe fn field_at_offset(
    _py: &PyToken<'_>,
    object: *mut u8,
    offset: usize,
) -> Option<InstanceField> {
    unsafe {
        if object_type_id(object) == TYPE_ID_DATACLASS {
            let desc = dataclass_desc_ptr(object);
            let index = offset / size_of::<u64>();
            if desc.is_null() || !offset.is_multiple_of(size_of::<u64>()) {
                return None;
            }
            let layout = &(*desc).field_layout;
            let field = layout.get(index)?;
            return Some(InstanceField {
                name: field.name,
                offset,
                kind: field.kind,
            });
        }
        if !super::object_has_class_shape(object) && !super::native_instance::has_fields(object) {
            return None;
        }
        let class = obj_from_bits(object_class_bits(object)).as_ptr()?;
        if object_type_id(class) != TYPE_ID_TYPE {
            return None;
        }
        let field = super::class_layout::field_at_offset(class, offset)?;
        let extent =
            super::native_instance::field_payload_size(object).saturating_sub(size_of::<u64>());
        assert!(
            field.offset % size_of::<u64>() == 0
                && field
                    .offset
                    .checked_add(size_of::<u64>())
                    .is_some_and(|end| end <= extent),
            "sealed physical field must fit the instance extent"
        );
        Some(field)
    }
}

/// Dataclass vectors preserve public field indices while projecting every
/// canonical declared-slot offset onto one distinct backing element.
pub(crate) unsafe fn dataclass_slot_storage_offset(
    object: *mut u8,
    slot_offset: usize,
) -> Option<usize> {
    unsafe {
        let desc = dataclass_desc_ptr(object);
        if desc.is_null() {
            return None;
        }
        (*desc)
            .field_layout
            .iter()
            .position(|field| field.slot_offset == Some(slot_offset))
            .and_then(|index| index.checked_mul(size_of::<u64>()))
    }
}

pub(crate) enum FieldStorage {
    Inline(*mut u64),
    Dictionary { dictionary: u64, name: u64 },
}

/// Validate backing without allocating or changing ownership. Corrupt backing
/// must not be silently reset, which would strand its existing owner.
pub(crate) unsafe fn current_dictionary(
    py: &PyToken<'_>,
    object: *mut u8,
) -> Result<Option<u64>, ()> {
    unsafe {
        let bits = instance_dict_bits(object);
        if bits == 0 {
            return Ok(None);
        }
        if obj_from_bits(bits)
            .as_ptr()
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT)
        {
            return Ok(Some(bits));
        }
        raise_exception::<()>(py, "SystemError", "invalid instance dictionary backing");
        Err(())
    }
}

unsafe fn inferred_inline_owners(py: &PyToken<'_>, object: *mut u8) -> Vec<(u64, *mut u64, u64)> {
    unsafe {
        let mut fields = Vec::new();
        // Native payloads can own dictionaries without hosting managed inline
        // fields. Their typed prefix/items must never be interpreted as slots.
        if !super::object_has_class_shape(object) && object_type_id(object) != TYPE_ID_DATACLASS {
            return fields;
        }
        if let Some(class) = obj_from_bits(object_class_bits(object)).as_ptr()
            && object_type_id(class) == TYPE_ID_TYPE
        {
            for_each_instance_field(py, object, class, &mut |field, slot| {
                if field.kind.is_inferred() && !is_missing_bits(py, *slot) {
                    fields.push((field.name, slot, *slot));
                }
            });
        }
        fields
    }
}

/// Consumes the incoming dictionary owner. Publication and all physical-word
/// retirement precede releases so finalizers observe the complete new state.
unsafe fn publish(
    py: &PyToken<'_>,
    object: *mut u8,
    dictionary: u64,
    fields: Vec<(u64, *mut u64, u64)>,
) -> bool {
    unsafe {
        // The dictionary location and this publication row are the only
        // representation-specific steps. Exception projection must succeed
        // before the previous dictionary owner can be retired.
        if object_type_id(object) == TYPE_ID_EXCEPTION {
            debug::instance(py, "publish_previous", object);
            debug::dictionary(py, "publish_incoming", object, dictionary, None, None);
            debug_assert!(fields.is_empty());
            let result = crate::builtins::exceptions::exception_replace_field_bits(
                py,
                MoltObject::from_ptr(object).bits(),
                crate::builtins::exceptions::ExceptionFieldSlot::Dict,
                if dictionary == 0 {
                    MoltObject::none().bits()
                } else {
                    dictionary
                },
            );
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                if dictionary != 0 {
                    dec_ref_bits(py, dictionary);
                }
            });
            if let Err(message) = result {
                if !exception_pending(py) {
                    raise_exception::<()>(py, "SystemError", message);
                }
                return false;
            }
            return true;
        }
        let missing = if fields.is_empty() {
            0
        } else {
            missing_bits(py)
        };
        if exception_pending(py) {
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                if dictionary != 0 {
                    dec_ref_bits(py, dictionary);
                }
            });
            return false;
        }
        let previous = instance_dict_bits(object);
        debug::dictionary(py, "publish_previous", object, previous, None, None);
        debug::dictionary(py, "publish_incoming", object, dictionary, None, None);
        // Reset can retire initialized scalar fields without publishing a dict.
        // Their empty words must still pass through missing/class-fallback lookup.
        object_mark_has_ptrs(py, object);
        instance_set_dict_bits(py, object, dictionary);
        for (_, slot, _) in &fields {
            **slot = missing;
        }
        debug::fields(py, "inline_retired", object, dictionary, &fields);
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            for (_, _, value) in fields {
                dec_ref_bits(py, value);
            }
            if previous != 0 {
                debug::dictionary(py, "previous_release", object, previous, None, None);
                dec_ref_bits(py, previous);
            }
        });
        true
    }
}

/// `Some` replaces __dict__; `None` restores lazy empty instance backing.
/// Function dictionaries follow PyObject_GenericSetDict and cannot be deleted.
/// Declared slots and their same-name dictionary entries are independent.
pub(crate) unsafe fn replace_dictionary(
    py: &PyToken<'_>,
    object: *mut u8,
    replacement: Option<u64>,
) {
    unsafe {
        if !allows_dictionary(py, object) {
            raise_exception::<()>(py, "AttributeError", "object has no instance dictionary");
            return;
        }
        if replacement.is_none() && object_type_id(object) == TYPE_ID_FUNCTION {
            raise_exception::<()>(py, "TypeError", "cannot delete __dict__");
            return;
        }
        if let Some(bits) = replacement
            && obj_from_bits(bits)
                .as_ptr()
                .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_DICT)
        {
            let message = format!(
                "__dict__ must be set to a dictionary, not a '{}'",
                type_name(py, obj_from_bits(bits))
            );
            raise_exception::<()>(py, "TypeError", &message);
            return;
        }
        if current_dictionary(py, object).is_err() {
            return;
        }
        let fields = inferred_inline_owners(py, object);
        if exception_pending(py) {
            return;
        }
        let bits = replacement.unwrap_or(0);
        if bits != 0 {
            inc_ref_bits(py, bits);
        }
        let _ = publish(py, object, bits, fields);
    }
}

/// Resolve an already admitted physical field. Once a dictionary exists, an
/// inferred field may never read or reacquire ownership in its retired word.
pub(crate) unsafe fn resolve(
    py: &PyToken<'_>,
    object: *mut u8,
    offset: usize,
    slot: *mut u64,
) -> Option<FieldStorage> {
    unsafe {
        // The immutable task shape owns capture words even when the adapter
        // has a logical class. Raw/classless storage is equally direct.
        if object_class_bits(object) == 0
            || super::object_shape_is_task(super::object_shape_id(object))
        {
            return Some(FieldStorage::Inline(slot));
        }
        let Some(dictionary) = current_dictionary(py, object).ok()? else {
            return Some(FieldStorage::Inline(slot));
        };
        let Some(field) = field_at_offset(py, object, offset) else {
            raise_exception::<()>(
                py,
                "SystemError",
                "field offset is absent from instance layout",
            );
            return None;
        };
        if !field.kind.is_inferred() {
            Some(FieldStorage::Inline(slot))
        } else {
            Some(FieldStorage::Dictionary {
                dictionary,
                name: field.name,
            })
        }
    }
}

/// Stage a first dictionary, including its first insertion, before publishing
/// any owner. Public reads and private callable writers share this transition.
unsafe fn materialize_with_pairs(
    py: &PyToken<'_>,
    object: *mut u8,
    initial_pairs: &[u64],
) -> Option<u64> {
    unsafe {
        debug_assert!(current_dictionary(py, object).ok().flatten().is_none());
        let fields = inferred_inline_owners(py, object);
        if exception_pending(py) {
            return None;
        }
        let pairs: Vec<u64> = fields
            .iter()
            .flat_map(|(name, _, value)| [*name, *value])
            .chain(initial_pairs.iter().copied())
            .collect();
        debug::fields(py, "materialize_inline", object, 0, &fields);
        let dict = alloc_dict_with_pairs(py, &pairs);
        if dict.is_null() {
            if !exception_pending(py) {
                raise_exception::<()>(py, "MemoryError", "instance dictionary allocation failed");
            }
            return None;
        }
        let bits = MoltObject::from_ptr(dict).bits();
        if exception_pending(py) {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
            return None;
        }
        debug::dictionary(py, "materialize_staged", object, bits, None, None);
        publish(py, object, bits, fields).then_some(bits)
    }
}

/// Return a borrowed public dictionary, moving inferred inline owners exactly
/// once. Allocation failure leaves the object and all existing owners unchanged.
pub(crate) unsafe fn materialize(py: &PyToken<'_>, object: *mut u8) -> Option<u64> {
    unsafe {
        if exception_pending(py) {
            return None;
        }
        if !allows_dictionary(py, object) {
            raise_exception::<()>(py, "AttributeError", "object has no instance dictionary");
            return None;
        }
        if let Some(existing) = current_dictionary(py, object).ok()? {
            return Some(existing);
        }
        materialize_with_pairs(py, object, &[])
    }
}

/// Update an admitted physical dictionary owner, retaining displaced entries
/// until the caller commits dependent metadata. Public callers must first check
/// class capability; runtime callable metadata may use its private backing.
pub(crate) unsafe fn set_item_deferred<'a, 'py>(
    py: &'a PyToken<'py>,
    object: *mut u8,
    name: u64,
    value: u64,
) -> Result<Option<super::ops::DetachedDictReferences<'a, 'py>>, ()> {
    unsafe {
        if exception_pending(py) {
            return Err(());
        }
        if super::instance_dict_bits_ptr(object).is_null() {
            raise_exception::<()>(py, "AttributeError", "object has no instance dictionary");
            return Err(());
        }
        let Some(dictionary) = current_dictionary(py, object)? else {
            return materialize_with_pairs(py, object, &[name, value])
                .map(|_| None)
                .ok_or(());
        };
        // Key equality may replace the object's dictionary. Pin the original
        // mapping throughout the update and preserve errors when retiring it.
        inc_ref_bits(py, dictionary);
        debug::dictionary(
            py,
            "instance_set_before",
            object,
            dictionary,
            Some(name),
            Some(value),
        );
        let result = super::ops::dict_set_deferred(
            py,
            obj_from_bits(dictionary).as_ptr().unwrap(),
            name,
            value,
        );
        debug::dictionary(
            py,
            "instance_set_after",
            object,
            dictionary,
            Some(name),
            Some(value),
        );
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, dictionary));
        if result.is_err() && !exception_pending(py) {
            raise_exception::<()>(py, "MemoryError", "instance dictionary insertion failed");
        }
        result.map(Some)
    }
}

/// Ordinary instance and dataclass writers use the same staged first insertion
/// as private callable metadata. Public capability remains a sealed class fact.
pub(crate) unsafe fn set_item(py: &PyToken<'_>, object: *mut u8, name: u64, value: u64) {
    unsafe {
        if !allows_dictionary(py, object) {
            raise_exception::<()>(py, "AttributeError", "object has no instance dictionary");
            return;
        }
        let result = set_item_deferred(py, object, name, value);
        molt_cpython_abi::api::errors::with_preserved_error(|| drop(result));
    }
}

/// Whether the sealed representation provides ordinary dictionary attributes.
pub(crate) unsafe fn allows_dictionary(py: &PyToken<'_>, object: *mut u8) -> bool {
    unsafe {
        if object_type_id(object) == TYPE_ID_DATACLASS {
            let desc = dataclass_desc_ptr(object);
            return !desc.is_null() && (*desc).allows_dict;
        }
        !crate::object::instance_dict_bits_ptr(object).is_null()
            && class_allows_dictionary(py, object)
    }
}

/// Public dictionary capability is sealed on the real Python type. Native
/// functions use this policy independently of their private metadata backing.
pub(crate) unsafe fn class_allows_dictionary(py: &PyToken<'_>, object: *mut u8) -> bool {
    unsafe {
        obj_from_bits(crate::type_of_bits(py, MoltObject::from_ptr(object).bits()))
            .as_ptr()
            .is_some_and(|class| {
                crate::builtins::attr::class_slots_info(py, class)
                    .is_some_and(|info| info.allows_dict)
            })
    }
}

/// Reset every physical owner and dictionary in one callback-free publication.
/// Pickle reconstruction must not consult mutable namespace offset maps or
/// release any old value while sibling slots still expose the previous state.
pub(crate) unsafe fn reset(py: &PyToken<'_>, object: *mut u8) {
    unsafe {
        if super::instance_dict_bits_ptr(object).is_null() {
            return;
        }
        let Some(class) = obj_from_bits(object_class_bits(object)).as_ptr() else {
            return;
        };
        let missing = missing_bits(py);
        if exception_pending(py) {
            return;
        }
        let mut count = 0usize;
        for_each_instance_field(py, object, class, &mut |_, _| {
            count += 1;
        });
        let Some(detached) =
            super::backing::tracked_vec_box_with_capacity::<(*mut u64, u64)>(count)
        else {
            raise_exception::<()>(py, "MemoryError", "instance field reset allocation failed");
            return;
        };
        let mut detached = super::backing::tracked_vec_box_from_raw(detached);
        // Sealed physical offsets and projected dataclass indices are unique.
        // Reserve and collect the complete transition before destructive writes.
        for_each_instance_field(py, object, class, &mut |_, slot| {
            detached.push((slot, *slot));
        });
        let old_dict = instance_dict_bits(object);
        debug::dictionary(py, "instance_reset", object, old_dict, None, None);
        for &(slot, _) in detached.iter() {
            *slot = missing;
        }
        instance_set_dict_bits(py, object, 0);
        object_mark_has_ptrs(py, object);
        for &(_, bits) in detached.iter() {
            dec_ref_bits(py, bits);
        }
        if old_dict != 0 {
            dec_ref_bits(py, old_dict);
        }
    }
}

/// Snapshot declared attribute names, not physical values, in MRO order.
/// Duplicate declarations remain observable repeated attribute reads. Dataclass
/// descriptor-only slots follow in descriptor order; GC/reset retain their
/// separate complete physical traversal.
pub(crate) unsafe fn slot_state_names<'a, 'py>(
    py: &'a PyToken<'py>,
    object: *mut u8,
) -> Option<super::seq_access::PinnedSequenceSnapshot<'a, 'py>> {
    unsafe {
        if super::object_shape_is_task(super::object_shape_id(object)) {
            return None;
        }
        let class = obj_from_bits(object_class_bits(object)).as_ptr()?;
        if object_type_id(class) != TYPE_ID_TYPE {
            return None;
        }
        let mro = class_mro_view(py, class);
        let desc = if object_type_id(object) == TYPE_ID_DATACLASS {
            dataclass_desc_ptr(object)
        } else {
            std::ptr::null_mut()
        };
        let visit_declarations = |visit: &mut dyn FnMut(u64)| {
            for &class in mro.iter() {
                let Some(class) = obj_from_bits(class).as_ptr() else {
                    continue;
                };
                if let super::layout::ClassSlotDeclaration::Names(names) =
                    super::layout::class_slot_declaration(class)
                    && let Some(names) = obj_from_bits(names).as_ptr()
                {
                    super::seq_access::with_immutable_tuple_slice(names, |names| {
                        for &name in names {
                            let key = obj_from_bits(name).as_ptr().expect("sealed slot name");
                            let bytes =
                                std::slice::from_raw_parts(string_bytes(key), string_len(key));
                            if bytes != b"__dict__" && bytes != b"__weakref__" {
                                visit(name);
                            }
                        }
                    });
                }
            }
        };
        let mut count = if desc.is_null() {
            0
        } else {
            (*desc).field_layout.len()
        };
        visit_declarations(&mut |_| {
            count = count.saturating_add(1);
        });
        if count == 0 {
            return None;
        }
        let Some(names) = super::backing::tracked_vec_box_with_capacity::<u64>(count) else {
            raise_exception::<()>(py, "MemoryError", "instance slot names allocation failed");
            return None;
        };
        let mut names = super::backing::tracked_vec_box_from_raw(names);
        visit_declarations(&mut |name| {
            inc_ref_bits(py, name);
            names.push(name);
        });
        if !desc.is_null() {
            // Hidden physical rows are already represented by their captured
            // class declarations. Only logical descriptor-only names can add
            // observable state reads here.
            for field in (*desc).field_layout.iter().take((*desc).field_names.len()) {
                let name = field.name;
                if field.kind.is_declared_slot()
                    && !names
                        .iter()
                        .any(|&seen| crate::object::ops_compare::string_storage_equal(seen, name))
                {
                    inc_ref_bits(py, name);
                    names.push(name);
                }
            }
        }
        Some(super::seq_access::PinnedSequenceSnapshot::from_owned_values(py, names))
    }
}

/// Pin authoritative dataclass values before equality/hash/repr callbacks.
/// No raw backing borrow escapes a field read; callback replacement cannot
/// invalidate a later element or make these operations read retired words.
pub(crate) unsafe fn dataclass_snapshot<'a, 'py>(
    py: &'a PyToken<'py>,
    object: *mut u8,
    flag_mask: u8,
) -> Option<super::seq_access::PinnedSequenceSnapshot<'a, 'py>> {
    unsafe {
        let desc = dataclass_desc_ptr(object);
        if desc.is_null() {
            return None;
        }
        let count = (*desc).field_names.len();
        let Some(values) = super::backing::tracked_vec_box_with_capacity::<u64>(count) else {
            raise_exception::<()>(
                py,
                "MemoryError",
                "dataclass field snapshot allocation failed",
            );
            return None;
        };
        let flags = &(*desc).field_flags;
        for index in 0..count {
            let flag = flags.get(index).copied().unwrap_or(0x7);
            let bits = if flag & flag_mask == 0 {
                MoltObject::none().bits()
            } else {
                super::accessors::object_field_get_ptr_raw(py, object, index * size_of::<u64>())
            };
            (*values).push(bits);
            if is_missing_bits(py, bits) && !exception_pending(py) {
                let name = (&(*desc).field_names)[index].as_str();
                let _ = attr_error(py, &(*desc).name, name);
            }
            if exception_pending(py) {
                drop(
                    super::seq_access::PinnedSequenceSnapshot::from_owned_values(
                        py,
                        super::backing::tracked_vec_box_from_raw(values),
                    ),
                );
                return None;
            }
        }
        Some(
            super::seq_access::PinnedSequenceSnapshot::from_owned_values(
                py,
                super::backing::tracked_vec_box_from_raw(values),
            ),
        )
    }
}
