//! Module namespace slot tables (call_anatomy.md 4.5, invariant J3).
//!
//! Each module namespace dictionary may own one fixed table bound to its
//! module's compile-time global layout: the strictly sorted non-dunder names
//! the module's own code reads, writes or deletes as globals (registry schema
//! 6, `molt.cli.module_registry`). Slot k mirrors the dictionary's value for
//! the k-th name, or holds [`UNBOUND`].
//!
//! The dictionary stays the owner of every value and the authority for
//! iteration, order and the C-API. The table is a derived value index, like
//! the hash table: only the dictionary's commit primitives write it
//! (`object::ops::dict_set_tables`: insertion, value replacement, row
//! removal, row compaction and the reset commit), so every writer -
//! `STORE_GLOBAL`, `globals()[k] = v`, `setattr(module, ...)`,
//! `module.__dict__[k] = v`, `del`, import machinery and the C-API - reaches
//! it. Mirror rule S1: for a live table T of dictionary D and every k,
//! `T.values[k]` is `D[T.names[k]]`, or UNBOUND when the name is absent, and
//! `T.rows[k]` is that entry's row index, whenever Python or compiled code
//! can run.
//!
//! A namespace that admits a key other than an exact `str` is degraded for
//! good: every slot stays UNBOUND and readers take the by-name path, since a
//! key with a custom `__eq__` may then match a layout name.
//!
//! Slot values are borrowed: readers increment before use. Every commit runs
//! under the GIL. Free-threaded execution needs a try-increment protocol and
//! deferred reclamation of displaced values first; until then it does not
//! use the compiled slot reads.

use crate::*;

/// The word of an unbound slot. Never a Python value.
pub(crate) const UNBOUND: u64 = molt_codegen_abi::NAMESPACE_SLOT_UNBOUND;
const NO_ROW: u32 = u32::MAX;
const NO_SLOT: u32 = u32::MAX;

/// One namespace's slot table. Owned by its dictionary
/// (`DictStorage::slots`) and freed with it; never resized or moved, so
/// `values` keeps one address for the dictionary's life.
pub(crate) struct NamespaceSlots {
    degraded: bool,
    /// Interned immortal name objects, in layout order.
    names: Box<[u64]>,
    /// Open-addressed `str` hash index over `names`: `k + 1`, or 0.
    probe: Box<[u32]>,
    hashes: Box<[u64]>,
    /// The slot of each dictionary row, or `NO_SLOT`: the inverse of `rows`,
    /// so a replacement or removal, which knows its row, needs no probe.
    row_slots: Vec<u32>,
    /// The dictionary row of each bound slot, or `NO_ROW`.
    rows: Box<[u32]>,
    values: Box<[u64]>,
}

impl NamespaceSlots {
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn degraded(&self) -> bool {
        self.degraded
    }

    pub(crate) fn value(&self, k: usize) -> u64 {
        self.values[k]
    }

    pub(crate) fn row(&self, k: usize) -> Option<usize> {
        (self.rows[k] != NO_ROW).then_some(self.rows[k] as usize)
    }

    /// The slot of an exact `str` key with `str` hash `hash`, if the layout
    /// names it. The caller guarantees `key` is an exact `str`. The bound
    /// row's key or the layout's interned name decide by identity; otherwise
    /// the bytes do.
    #[inline]
    unsafe fn slot_of(&self, key: u64, hash: u64) -> Option<usize> {
        let mask = self.probe.len() - 1;
        let mut index = hash as usize & mask;
        loop {
            let entry = self.probe[index];
            if entry == 0 {
                return None;
            }
            let k = entry as usize - 1;
            if self.hashes[k] == hash {
                if self.names[k] == key || unsafe { str_bytes(self.names[k]) == str_bytes(key) } {
                    return Some(k);
                }
            }
            index = (index + 1) & mask;
        }
    }

    fn bind_slot(&mut self, k: usize, row: usize, value: u64) {
        self.values[k] = value;
        self.rows[k] = row as u32;
        if self.row_slots.len() <= row {
            self.row_slots.resize(row + 1, NO_SLOT);
        }
        self.row_slots[row] = k as u32;
    }

    /// The slot bound to dictionary row `row`, if any.
    #[inline]
    fn slot_of_row(&self, row: usize) -> Option<usize> {
        match self.row_slots.get(row) {
            Some(&k) if k != NO_SLOT => Some(k as usize),
            _ => None,
        }
    }

    fn unbind_all(&mut self) {
        self.values.fill(UNBOUND);
        self.rows.fill(NO_ROW);
        self.row_slots.clear();
    }

    fn degrade(&mut self) {
        self.degraded = true;
        self.unbind_all();
    }
}

#[inline]
unsafe fn table_of(dict: *mut u8) -> Option<&'static mut NamespaceSlots> {
    unsafe {
        let slots = dict_storage(dict).slots;
        if slots.is_null() {
            return None;
        }
        let table = &mut *slots;
        (!table.degraded).then_some(table)
    }
}

/// The live, non-degraded table of an exact dictionary.
pub(crate) unsafe fn namespace_slots(dict: *mut u8) -> Option<&'static NamespaceSlots> {
    unsafe { table_of(dict).map(|table| &*table) }
}

/// The bytes of a `str` object (exact or not; the layout compares bytes).
#[inline]
unsafe fn str_bytes<'s>(bits: u64) -> &'s [u8] {
    unsafe {
        let ptr = obj_from_bits(bits).as_ptr().unwrap();
        std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr))
    }
}

/// Whether `key` is an exact `str` (the only key kind a slot can mirror).
unsafe fn exact_str_key(py: &PyToken<'_>, key: u64) -> bool {
    unsafe { crate::object::ops::exact_string_bytes(py, key).is_some() }
}

/// Whether `dict` owns a slot table: the only test an unslotted dictionary's
/// commit pays. The slotted work stays out of line so the dictionary's hot
/// primitives keep their inlining (measured: an inlined hook outlined
/// `dict_find_entry` and cost 1.7% on a module-global loop).
#[inline(always)]
unsafe fn has_table(dict: *mut u8) -> bool {
    unsafe { !dict_storage(dict).slots.is_null() }
}

/// A key entered row `row` of `dict` with value `value`.
#[inline(always)]
pub(crate) unsafe fn note_insert(py: &PyToken<'_>, dict: *mut u8, row: usize, key: u64, hash: u64) {
    unsafe {
        if has_table(dict) {
            insert_slotted(py, dict, row, key, hash);
        }
    }
}

#[cold]
#[inline(never)]
unsafe fn insert_slotted(py: &PyToken<'_>, dict: *mut u8, row: usize, key: u64, hash: u64) {
    unsafe {
        let Some(table) = table_of(dict) else {
            return;
        };
        if !exact_str_key(py, key) {
            table.degrade();
            return;
        }
        if let Some(k) = table.slot_of(key, hash) {
            table.bind_slot(k, row, dict_entries(dict)[row].value);
        }
    }
}

/// Row `row` of `dict` now holds `value`. Its key is unchanged. This is
/// every global store's commit, so it is a few inline loads and one store: a
/// degraded or unbound row has no entry in `row_slots`.
#[inline(always)]
pub(crate) unsafe fn note_replace(dict: *mut u8, row: usize, value: u64) {
    unsafe {
        let slots = dict_storage(dict).slots;
        if slots.is_null() {
            return;
        }
        let table = &mut *slots;
        if let Some(&k) = table.row_slots.get(row)
            && k != NO_SLOT
        {
            debug_assert_eq!(table.rows[k as usize], row as u32);
            table.values[k as usize] = value;
        }
    }
}

/// Row `row` of `dict` was removed.
#[inline(always)]
pub(crate) unsafe fn note_remove(dict: *mut u8, row: usize) {
    unsafe {
        if has_table(dict) {
            remove_slotted(dict, row);
        }
    }
}

#[cold]
#[inline(never)]
unsafe fn remove_slotted(dict: *mut u8, row: usize) {
    unsafe {
        let Some(table) = table_of(dict) else {
            return;
        };
        if let Some(k) = table.slot_of_row(row) {
            table.values[k] = UNBOUND;
            table.rows[k] = NO_ROW;
            table.row_slots[row] = NO_SLOT;
        }
    }
}

/// Rebuild every slot from the live rows: the reset commit (clear, staged
/// publication, clone, lifecycle clear) and row compaction.
#[inline(always)]
pub(crate) unsafe fn note_rebuild(py: &PyToken<'_>, dict: *mut u8) {
    unsafe {
        if has_table(dict) {
            rebuild_slotted(py, dict);
        }
    }
}

#[cold]
#[inline(never)]
unsafe fn rebuild_slotted(py: &PyToken<'_>, dict: *mut u8) {
    unsafe {
        let Some(table) = table_of(dict) else {
            return;
        };
        table.unbind_all();
        if dict_entries_ptr(dict).is_null() {
            return;
        }
        for (row, entry) in dict_entries(dict).iter().enumerate() {
            let Some(hash) = entry.hash else {
                continue;
            };
            if !exact_str_key(py, entry.key) {
                table.degrade();
                return;
            }
            if let Some(k) = table.slot_of(entry.key, hash.get()) {
                table.bind_slot(k, row, entry.value);
            }
        }
    }
}

/// Build and attach the table for `layout` to `dict`, mirroring its rows.
/// `names` are the layout's strictly sorted names. A dictionary that already
/// has a table keeps it. Errors leave the dictionary without a table, which
/// costs only speed.
pub(crate) unsafe fn bind<'n>(
    py: &PyToken<'_>,
    dict: *mut u8,
    names: impl ExactSizeIterator<Item = &'n [u8]>,
) {
    unsafe {
        if object_type_id(dict) != TYPE_ID_DICT || !dict_storage(dict).slots.is_null() {
            return;
        }
        let len = names.len();
        if len == 0 || len >= u32::MAX as usize {
            return;
        }
        let mut name_bits = Vec::with_capacity(len);
        let mut hashes = Vec::with_capacity(len);
        for name in names {
            let ptr = crate::object::builders::alloc_interned_string(py, name);
            if ptr.is_null() {
                clear_exception(py);
                return;
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            let Some(hash) = crate::object::ops::dict_key_hash(py, bits) else {
                clear_exception(py);
                return;
            };
            name_bits.push(bits);
            hashes.push(hash);
        }
        let capacity = (len * 2).next_power_of_two();
        let mut probe = vec![0u32; capacity].into_boxed_slice();
        for (k, &hash) in hashes.iter().enumerate() {
            let mut index = hash as usize & (capacity - 1);
            while probe[index] != 0 {
                index = (index + 1) & (capacity - 1);
            }
            probe[index] = k as u32 + 1;
        }
        let table = Box::new(NamespaceSlots {
            degraded: false,
            names: name_bits.into_boxed_slice(),
            probe,
            hashes: hashes.into_boxed_slice(),
            row_slots: Vec::new(),
            rows: vec![NO_ROW; len].into_boxed_slice(),
            values: vec![UNBOUND; len].into_boxed_slice(),
        });
        dict_storage(dict).slots = Box::into_raw(table);
        note_rebuild(py, dict);
    }
}

/// Attach the layout of the registry row named like the module, if any.
/// Binding the layout of a same-named row to a different module object is
/// harmless: the table only mirrors this dictionary.
pub(crate) fn bind_module_namespace(py: &PyToken<'_>, dict: *mut u8, module_name: u64) {
    let Some(registry) = crate::builtins::module_table::module_registry() else {
        return;
    };
    let Some(name) = (unsafe { crate::object::ops::exact_string_bytes(py, module_name) }) else {
        return;
    };
    let Ok(name) = std::str::from_utf8(name) else {
        return;
    };
    let Some(id) = registry.id_of(name) else {
        return;
    };
    let names: Vec<&[u8]> = registry.globals_of(id).iter().collect();
    unsafe { bind(py, dict, names.into_iter()) };
}

/// Free a dictionary's table. Its names are immortal and its values borrowed.
pub(crate) unsafe fn release(dict: *mut u8) {
    unsafe {
        let slots = std::mem::replace(&mut dict_storage(dict).slots, std::ptr::null_mut());
        if !slots.is_null() {
            drop(Box::from_raw(slots));
        }
    }
}

#[cfg(test)]
#[path = "namespace_slots_tests.rs"]
mod tests;
