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
//! - the class layout places the name in an inferred field, or an ordinary
//!   dictionary write is admitted and no inferred field exists.
//!
//! Entries hold no references. They are keyed by the global type version,
//! which advances on every class mutation and every type release, so a hit
//! never sees a stale offset or a reused class address. Inferred-field hits
//! defer when the instance has a dictionary or the field is unset. Dictionary
//! writes use the canonical storage transaction, including first insertion,
//! dictionary replacement, equality callbacks and displaced-value retirement.

use super::*;
use crate::builtins::attr::AttrNameCacheKey;
use crate::builtins::attr::{class_inferred_field_offset, class_namespace_lookup_mro_pure};
use crate::object::accessors::{object_field_get_ptr_raw, object_field_set_ptr_raw};
use molt_cpython_abi::hooks::AttributeMutation;
use std::cell::RefCell;

struct FieldSiteEntry {
    name: AttrNameCacheKey,
    site_id: u64,
    class_bits: u64,
    type_version: u64,
    target: FieldSiteTarget,
}

impl FieldSiteEntry {
    /// Inlined operations may share a source index in the emitted caller.
    /// The site identifies a cache slot; its bytes identify the attribute.
    fn matches(&self, site: u64, class: u64, epoch: u64, name: &[u8]) -> bool {
        self.site_id == site
            && self.class_bits == class
            && self.type_version == epoch
            && self.name.as_slice() == name
    }
}

#[derive(Clone, Copy)]
enum FieldSiteTarget {
    Inferred {
        offset: usize,
        // Number of later physical words, when all are consecutive inferred
        // rows. The receipt is valid only for this exact class and epoch.
        following: Option<usize>,
    },
    Dictionary,
}

/// Direct-mapped by site id, like the call and method caches.
const FIELD_SITE_SLOTS: usize = 256;

type FieldSiteTable = [RefCell<Option<FieldSiteEntry>>; FIELD_SITE_SLOTS];

// Reads and writes keep separate tables: their admission facts differ, so a
// colliding site id must never let a read entry bypass a custom __setattr__.
thread_local! {
    static READ_SITES: FieldSiteTable =
        const { [const { RefCell::new(None) }; FIELD_SITE_SLOTS] };
    static WRITE_SITES: FieldSiteTable =
        const { [const { RefCell::new(None) }; FIELD_SITE_SLOTS] };
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
unsafe fn cached_target(
    access: FieldAccess,
    site_id: u64,
    obj_bits: u64,
    name: &[u8],
) -> Option<(*mut u8, FieldSiteTarget)> {
    unsafe {
        access.table().with(|table| {
            let row = table[slot_index(site_id)].borrow();
            let row = row.as_ref()?;
            let object = plain_instance(obj_bits)?;
            row.matches(
                site_id,
                object_class_bits(object),
                crate::object::global_type_version(),
                name,
            )
            .then_some((object, row.target))
        })
    }
}

#[inline]
unsafe fn cached_offset(
    access: FieldAccess,
    site_id: u64,
    obj_bits: u64,
    name: &[u8],
) -> Option<(*mut u8, usize)> {
    unsafe {
        let (object, FieldSiteTarget::Inferred { offset, .. }) =
            cached_target(access, site_id, obj_bits, name)?
        else {
            return None;
        };
        (!crate::object::field_storage::dictionary_backed(object)).then_some((object, offset))
    }
}

#[cfg(test)]
pub(super) fn cached_offset_for_test(
    access: FieldAccess,
    site_id: u64,
    obj_bits: u64,
    name: &[u8],
) -> Option<usize> {
    unsafe { cached_offset(access, site_id, obj_bits, name).map(|(_, offset)| offset) }
}

/// A warm read site. `Some` carries the owned field value, or tagged None
/// with an exception pending; `None` sends the read to the generic path.
pub(super) unsafe fn read_cached_field(
    py: &PyToken<'_>,
    site_id: u64,
    obj_bits: u64,
    name: &[u8],
) -> Option<u64> {
    unsafe {
        let (obj_ptr, offset) = cached_offset(FieldAccess::Read, site_id, obj_bits, name)?;
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

/// A warm write site. Dictionary entries retain no name reference.
/// `true` means the write completed or raised; `false` takes the generic path.
pub(super) unsafe fn write_cached_field(
    py: &PyToken<'_>,
    site_id: u64,
    obj_bits: u64,
    val_bits: u64,
    name: super::mutation::WriteSiteName<'_>,
) -> bool {
    unsafe {
        let Some((object, target)) =
            cached_target(FieldAccess::Write, site_id, obj_bits, name.bytes)
        else {
            return false;
        };
        match target {
            FieldSiteTarget::Inferred { offset, following } => {
                if crate::object::field_storage::dictionary_backed(object) {
                    return false;
                }
                if let Some(following) = following
                    && crate::object::accessors::try_initialize_inferred_field(
                        py, object, offset, following, val_bits,
                    )
                {
                    return true;
                }
                let _ = object_field_set_ptr_raw(py, object, offset, val_bits);
            }
            FieldSiteTarget::Dictionary => {
                name.with_object(py, |name| {
                    let _ = crate::object::field_storage::set_item(py, object, name, val_bits);
                });
            }
        }
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
) -> Result<(), ()> {
    unsafe {
        let Some(obj_ptr) = plain_instance(obj_bits) else {
            return Ok(());
        };
        let class_bits = object_class_bits(obj_ptr);
        let Some(class) = obj_from_bits(class_bits).as_ptr() else {
            return Ok(());
        };
        if object_type_id(class) != TYPE_ID_TYPE {
            return Ok(());
        }
        let type_version = crate::object::global_type_version();
        let slot = slot_index(site_id);
        let Some(name) = crate::object::ops::exact_string_bytes(py, name_bits) else {
            return Ok(());
        };
        if access.table().with(|table| {
            table[slot]
                .borrow()
                .as_ref()
                .is_some_and(|row| row.matches(site_id, class_bits, type_version, name))
        }) {
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
        if !default_protocol
            || class_namespace_lookup_mro_pure(
                py,
                class,
                name_bits,
                None,
                crate::builtins::attr::NamespacePreparation::Unresolved,
            ) != Some(None)
        {
            return Ok(());
        }
        let target = match class_inferred_field_offset(py, class, name_bits)? {
            Some(offset) => FieldSiteTarget::Inferred {
                offset,
                following: if access == FieldAccess::Write && !cfg!(feature = "free-threaded") {
                    crate::object::class_layout::consecutive_inferred_suffix(py, class, offset)
                } else {
                    None
                },
            },
            None if access == FieldAccess::Write
                && crate::object::field_storage::allows_dictionary(py, obj_ptr) =>
            {
                FieldSiteTarget::Dictionary
            }
            None => return Ok(()),
        };
        // Finishing the class definition may publish layout; record only a
        // fact that still describes the current class graph.
        if crate::object::global_type_version() != type_version {
            return Ok(());
        }
        access.table().with(|table| {
            table[slot].replace(Some(FieldSiteEntry {
                name: AttrNameCacheKey::new(name),
                site_id,
                class_bits,
                type_version,
                target,
            }));
        });
        Ok(())
    }
}
