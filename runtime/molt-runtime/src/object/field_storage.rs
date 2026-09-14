//! One owner for ordinary instance attributes: inline until dictionary exposure,
//! dictionary-backed afterwards. Declared slots never participate in that move.

use crate::builtins::attr::class_own_slot_field_offset;
use crate::*;
use std::mem::size_of;

#[derive(Clone, Copy)]
pub(crate) struct InstanceField {
    pub(crate) name: u64,
    pub(crate) offset: usize,
    pub(crate) declared_slot: bool,
}

/// The same layout traversal serves backing transitions, GC, and serialization.
/// A copied inherited offset retains its declaring slot's storage class. Fields
/// never alias the trailing managed dictionary word, even in malformed layouts.
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
            let count = (*fields).len().min((*desc).field_keys.len());
            for index in 0..count {
                visit(
                    InstanceField {
                        name: (&(*desc).field_keys)[index],
                        offset: index * size_of::<u64>(),
                        declared_slot: (&(*desc).declared_slots)[index],
                    },
                    (*fields).as_mut_ptr().add(index),
                );
            }
            return;
        }
        let extent = object_payload_size(object).saturating_sub(size_of::<u64>());
        for_each_class_field(py, class, extent, &mut |field| {
            visit(field, object.add(field.offset).cast());
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
            *object.add(field.offset).cast::<u64>() = missing;
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
        let mut fields: Vec<InstanceField> = Vec::new();
        for class_bits in class_mro_view(py, class).iter().copied() {
            let Some(current) = obj_from_bits(class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(current) != TYPE_ID_TYPE {
                continue;
            }
            // Physical fields never consult the mutable class namespace. The
            // sealed map owns exact string keys and integer offsets, so this
            // traversal cannot run equality callbacks during GC or transfer.
            let offsets_bits = super::layout::class_field_offsets_bits(current);
            assert_ne!(
                offsets_bits, 0,
                "physical field traversal requires a sealed class layout"
            );
            let Some(offsets) = obj_from_bits(offsets_bits).as_ptr() else {
                assert_eq!(
                    offsets_bits,
                    MoltObject::none().bits(),
                    "invalid sealed field map"
                );
                continue;
            };
            assert_eq!(
                object_type_id(offsets),
                TYPE_ID_DICT,
                "invalid sealed field map"
            );
            for pair in dict_order(offsets).chunks_exact(2) {
                let offset = obj_from_bits(pair[1])
                    .as_int()
                    .and_then(|offset| usize::try_from(offset).ok())
                    .expect("sealed physical field offset must be a nonnegative integer");
                assert!(
                    offset % size_of::<u64>() == 0
                        && offset
                            .checked_add(size_of::<u64>())
                            .is_some_and(|end| end <= field_extent),
                    "sealed physical field must fit the instance extent"
                );
                let declared_slot =
                    class_own_slot_field_offset(py, current, pair[0]) == Some(offset);
                if let Some(field) = fields.iter_mut().find(|field| field.offset == offset) {
                    field.declared_slot |= declared_slot;
                } else {
                    fields.push(InstanceField {
                        name: pair[0],
                        offset,
                        declared_slot,
                    });
                }
            }
        }
        for field in fields {
            visit(field);
        }
    }
}

pub(crate) unsafe fn field_at_offset(
    py: &PyToken<'_>,
    object: *mut u8,
    offset: usize,
) -> Option<InstanceField> {
    unsafe {
        if object_type_id(object) == TYPE_ID_DATACLASS {
            let desc = dataclass_desc_ptr(object);
            let index = offset / size_of::<u64>();
            if desc.is_null() || offset % size_of::<u64>() != 0 {
                return None;
            }
            return Some(InstanceField {
                name: *(&(*desc).field_keys).get(index)?,
                offset,
                declared_slot: *(&(*desc).declared_slots).get(index)?,
            });
        }
        let class = obj_from_bits(object_class_bits(object)).as_ptr()?;
        if object_type_id(class) != TYPE_ID_TYPE {
            return None;
        }
        let mut result = None;
        for_each_instance_field(py, object, class, &mut |field, _| {
            if field.offset == offset {
                result = Some(field);
            }
        });
        result
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
        if let Some(class) = obj_from_bits(object_class_bits(object)).as_ptr()
            && object_type_id(class) == TYPE_ID_TYPE
        {
            for_each_instance_field(py, object, class, &mut |field, slot| {
                if !field.declared_slot && !is_missing_bits(py, *slot) {
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
) {
    unsafe {
        let previous = instance_dict_bits(object);
        // Reset can retire initialized scalar fields without publishing a dict.
        // Their empty words must still pass through missing/class-fallback lookup.
        object_mark_has_ptrs(py, object);
        instance_set_dict_bits(py, object, dictionary);
        let missing = missing_bits(py);
        for (_, slot, _) in &fields {
            **slot = missing;
        }
        for (_, _, value) in fields {
            dec_ref_bits(py, value);
        }
        if previous != 0 {
            dec_ref_bits(py, previous);
        }
    }
}

/// `Some` replaces __dict__; `None` deletes it and restores lazy empty backing.
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
        if let Some(bits) = replacement
            && !obj_from_bits(bits)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT)
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
        publish(py, object, bits, fields);
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
        // Closure/task/raw allocation payloads have no class field layout and
        // do not reserve an instance-dictionary word. Their last word is data.
        if object_class_bits(object) == 0 {
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
        if field.declared_slot {
            Some(FieldStorage::Inline(slot))
        } else {
            Some(FieldStorage::Dictionary {
                dictionary,
                name: field.name,
            })
        }
    }
}

/// Return a borrowed dictionary, moving inferred inline owners exactly once.
/// Build before publication; clear every transferred word before any release.
/// Allocation failure leaves the instance and all existing ownership unchanged.
pub(crate) unsafe fn materialize(py: &PyToken<'_>, object: *mut u8) -> Option<u64> {
    unsafe {
        if !allows_dictionary(py, object) {
            raise_exception::<()>(py, "AttributeError", "object has no instance dictionary");
            return None;
        }
        if let Some(existing) = current_dictionary(py, object).ok()? {
            return Some(existing);
        }
        let fields = inferred_inline_owners(py, object);
        if exception_pending(py) {
            return None;
        }
        let pairs: Vec<u64> = fields
            .iter()
            .flat_map(|(name, _, value)| [*name, *value])
            .collect();
        let dict = alloc_dict_with_pairs(py, &pairs);
        if dict.is_null() {
            if !exception_pending(py) {
                raise_exception::<()>(py, "MemoryError", "instance dictionary allocation failed");
            }
            return None;
        }
        let bits = MoltObject::from_ptr(dict).bits();
        if exception_pending(py) {
            dec_ref_bits(py, bits);
            return None;
        }
        publish(py, object, bits, fields);
        Some(bits)
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
            && !obj_from_bits(object_class_bits(object))
                .as_ptr()
                .is_some_and(|class| {
                    crate::builtins::attr::class_slots_info(py, class)
                        .is_some_and(|info| !info.allows_dict)
                })
    }
}

/// Reset every physical owner and dictionary in one callback-free publication.
/// Pickle reconstruction must not consult mutable namespace offset maps or
/// release any old value while sibling slots still expose the previous state.
pub(crate) unsafe fn reset(py: &PyToken<'_>, object: *mut u8) {
    unsafe {
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
        // Traversal deduplicates physical offsets; dataclass indices are unique.
        // Reserve and collect the complete transition before destructive writes.
        for_each_instance_field(py, object, class, &mut |_, slot| {
            detached.push((slot, *slot));
        });
        let old_dict = instance_dict_bits(object);
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
            (*desc).field_keys.len()
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
            for (index, &name) in (*desc).field_keys.iter().enumerate() {
                if (&(*desc).declared_slots)[index]
                    && !names
                        .iter()
                        .any(|&seen| crate::builtins::attr::exact_string_bits_equal(seen, name))
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
        for index in 0..count {
            let flag = (&(*desc).field_flags).get(index).copied().unwrap_or(0x7);
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
