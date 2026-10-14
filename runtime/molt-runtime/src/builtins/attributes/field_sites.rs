//! Per-site inline cache for instance fields.
//!
//! A compiled attribute site whose receivers share one class reads or writes
//! one inferred instance field. The site remembers that class and the field
//! offset, so a hit skips name interning, the MRO walks and the descriptor
//! probes of the generic transaction.
//!
//! An entry records only facts that make the generic transaction reach the
//! same field:
//! - the receiver is a plain class-shaped object (not a task),
//! - the class keeps the default `__getattribute__` without `__getattr__`
//!   (reads) or the default `__setattr__`/`__delattr__` pair (writes),
//! - no class in the MRO has an attribute of that name, so no descriptor
//!   can take priority over the instance,
//! - the class layout places the name in an inferred field.
//!
//! Entries hold no references. They are keyed by the global type version,
//! which advances on every class mutation and every type release, so a hit
//! never sees a stale offset or a reused class address. A hit still defers to
//! the generic path when the instance has a dictionary or the field is unset.

use super::*;
use crate::builtins::attr::{class_inferred_field_offset, class_namespace_lookup_mro_pure};
use crate::object::accessors::{object_field_get_ptr_raw, object_field_set_ptr_raw};
use molt_cpython_abi::hooks::AttributeMutation;
use std::cell::Cell;

#[derive(Clone, Copy)]
struct FieldSiteEntry {
    site_id: u64,
    class_bits: u64,
    type_version: u64,
    offset: usize,
}

const EMPTY_FIELD_SITE: FieldSiteEntry = FieldSiteEntry {
    site_id: 0,
    class_bits: 0,
    type_version: 0,
    offset: 0,
};

/// Direct-mapped by site id, like the call and method caches.
const FIELD_SITE_SLOTS: usize = 256;

type FieldSiteTable = [Cell<FieldSiteEntry>; FIELD_SITE_SLOTS];

// Reads and writes keep separate tables: their admission facts differ, so a
// colliding site id must never let a read entry bypass a custom __setattr__.
thread_local! {
    static READ_SITES: FieldSiteTable =
        const { [const { Cell::new(EMPTY_FIELD_SITE) }; FIELD_SITE_SLOTS] };
    static WRITE_SITES: FieldSiteTable =
        const { [const { Cell::new(EMPTY_FIELD_SITE) }; FIELD_SITE_SLOTS] };
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum FieldAccess {
    Read,
    Write,
}

impl FieldAccess {
    fn table(self) -> &'static std::thread::LocalKey<FieldSiteTable> {
        match self {
            Self::Read => &READ_SITES,
            Self::Write => &WRITE_SITES,
        }
    }
}

#[inline]
fn slot_index(site_id: u64) -> usize {
    (site_id as usize) & (FIELD_SITE_SLOTS - 1)
}

/// The receiver kinds whose default attribute transaction reaches inferred
/// fields directly. Their class edge is the semantic type.
#[inline]
unsafe fn plain_instance(obj_bits: u64) -> Option<*mut u8> {
    unsafe {
        let obj_ptr = maybe_ptr_from_bits(obj_bits)?;
        (object_type_id(obj_ptr) == TYPE_ID_OBJECT
            && !crate::object::object_shape_is_task(crate::object::object_shape_id(obj_ptr)))
        .then_some(obj_ptr)
    }
}

/// The field offset a warm site may use for `obj_ptr`, or `None` when the
/// generic transaction must decide.
#[inline]
unsafe fn cached_offset(
    access: FieldAccess,
    site_id: u64,
    obj_bits: u64,
) -> Option<(*mut u8, usize)> {
    unsafe {
        let entry = access
            .table()
            .with(|table| table[slot_index(site_id)].get());
        if entry.site_id != site_id || entry.class_bits == 0 {
            return None;
        }
        let obj_ptr = plain_instance(obj_bits)?;
        (object_class_bits(obj_ptr) == entry.class_bits
            && crate::object::global_type_version() == entry.type_version
            && instance_dict_bits(obj_ptr) == 0)
            .then_some((obj_ptr, entry.offset))
    }
}

#[cfg(test)]
pub(super) fn cached_offset_for_test(
    access: FieldAccess,
    site_id: u64,
    obj_bits: u64,
) -> Option<usize> {
    unsafe { cached_offset(access, site_id, obj_bits).map(|(_, offset)| offset) }
}

/// A warm read site. `Some` carries the owned field value, or tagged None
/// with an exception pending; `None` sends the read to the generic path.
pub(super) unsafe fn read_cached_field(
    py: &PyToken<'_>,
    site_id: u64,
    obj_bits: u64,
) -> Option<u64> {
    unsafe {
        let (obj_ptr, offset) = cached_offset(FieldAccess::Read, site_id, obj_bits)?;
        let Ok(bits) = object_field_get_ptr_raw(py, obj_ptr, offset) else {
            return Some(MoltObject::none().bits());
        };
        if is_missing_bits(py, bits) {
            // An unset field resolves through class attributes or raises.
            dec_ref_bits(py, bits);
            return None;
        }
        Some(bits)
    }
}

/// A warm write site. `true` means the write completed or raised; `false`
/// sends it to the generic path.
pub(super) unsafe fn write_cached_field(
    py: &PyToken<'_>,
    site_id: u64,
    obj_bits: u64,
    val_bits: u64,
) -> bool {
    unsafe {
        let Some((obj_ptr, offset)) = cached_offset(FieldAccess::Write, site_id, obj_bits) else {
            return false;
        };
        let _completed = object_field_set_ptr_raw(py, obj_ptr, offset, val_bits);
        true
    }
}

/// Record `site_id` when the generic transaction for `name` on `obj_bits`
/// would reach an inferred field. Runs before that transaction on a miss.
/// Every fact is callback-free; an undecided fact records nothing.
pub(super) unsafe fn remember_field_site(
    py: &PyToken<'_>,
    access: FieldAccess,
    site_id: u64,
    obj_bits: u64,
    name_bits: u64,
    name: &[u8],
) -> Result<(), ()> {
    unsafe {
        let Some(obj_ptr) = plain_instance(obj_bits) else {
            return Ok(());
        };
        let class_bits = object_class_bits(obj_ptr);
        let Some(class) = obj_from_bits(class_bits).as_ptr() else {
            return Ok(());
        };
        if object_type_id(class) != TYPE_ID_TYPE || name == b"__weakref__" {
            return Ok(());
        }
        let type_version = crate::object::global_type_version();
        let slot = slot_index(site_id);
        let recorded = access.table().with(|table| table[slot].get());
        if recorded.site_id == site_id
            && recorded.class_bits == class_bits
            && recorded.type_version == type_version
        {
            // The class facts stand; this receiver missed for an instance
            // reason (a dictionary or an unset field).
            return Ok(());
        }
        let default_protocol = match access {
            FieldAccess::Read => class_lookup::plain_attribute_protocol(
                py,
                class,
                class_lookup::DefaultGetattribute::Object,
            )?,
            FieldAccess::Write => {
                mutation::pure_mutation_default_access(py, class)
                    == Some(AttributeMutation::Generic)
            }
        };
        if !default_protocol || class_namespace_lookup_mro_pure(py, class, name, None) != Some(None)
        {
            return Ok(());
        }
        let Some(offset) = class_inferred_field_offset(py, class, name_bits)? else {
            return Ok(());
        };
        // Finishing the class definition may publish layout; record only a
        // fact that still describes the current class graph.
        if crate::object::global_type_version() != type_version {
            return Ok(());
        }
        access.table().with(|table| {
            table[slot].set(FieldSiteEntry {
                site_id,
                class_bits,
                type_version,
                offset,
            });
        });
        Ok(())
    }
}
