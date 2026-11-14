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
//!
//! A cell-eligible module (call_anatomy.md 4.5a, `molt.cli.static_module_cells`)
//! has no slot table and no mirror: its namespace dictionary binds the
//! module's static cells instead ([`StaticCells`]), a fixed-address block in
//! program data that compiled code reads and stores in place. The dictionary
//! never holds a layout name, and the cells hold nothing else: every by-name
//! path of the namespace (module attribute access and mutation, global
//! lookup and deletion, `dir`, star import, shutdown retirement) consults the
//! cells first through the registry's sorted layout. The whole-program
//! analysis proves no code reaches the dictionary of such a module by
//! identity, so no other path can observe the split.

use crate::*;

/// The word of an unbound slot. Never a Python value.
pub(crate) const UNBOUND: u64 = molt_codegen_abi::NAMESPACE_SLOT_UNBOUND;
const NO_ROW: u32 = u32::MAX;
const NO_SLOT: u32 = u32::MAX;

/// The binding a namespace dictionary carries (`DictStorage::slots`): none, a
/// slot table that mirrors the dictionary, or a cell-eligible module's static
/// cells. One tagged word: both targets are boxed and at least 2-aligned, so
/// the low bit tells them apart.
#[repr(transparent)]
#[derive(Clone, Copy, Default)]
pub(crate) struct NamespaceBinding(usize);

const CELLS_TAG: usize = 1;

impl NamespaceBinding {
    #[inline(always)]
    fn is_none(self) -> bool {
        self.0 == 0
    }

    #[inline(always)]
    fn slots(self) -> Option<*mut NamespaceSlots> {
        (self.0 != 0 && self.0 & CELLS_TAG == 0).then_some(self.0 as *mut NamespaceSlots)
    }

    #[inline(always)]
    fn cells(self) -> Option<&'static StaticCells> {
        (self.0 & CELLS_TAG != 0)
            .then(|| unsafe { &*((self.0 & !CELLS_TAG) as *const StaticCells) })
    }
}

/// A cell-eligible module's static cells (call_anatomy.md 4.5a): the
/// registry's sorted layout names and the program's block, one word per name
/// in layout order. The block is program data; each bound word owns one
/// reference, and an unbound one holds [`UNBOUND`].
pub(crate) struct StaticCells {
    /// Slices of the installed registry blob, in strictly sorted order.
    names: Box<[&'static [u8]]>,
    words: *mut u64,
}

impl StaticCells {
    fn index_of(&self, name: &[u8]) -> Option<usize> {
        self.names.binary_search(&name).ok()
    }

    fn cell(&self, k: usize) -> &'static std::sync::atomic::AtomicU64 {
        debug_assert!(k < self.names.len());
        unsafe { std::sync::atomic::AtomicU64::from_ptr(self.words.add(k)) }
    }
}

/// One namespace's slot table. Owned by its dictionary
/// (`DictStorage::slots`) and freed with it; never resized or moved, so
/// `values` keeps one address for the dictionary's life.
pub(crate) struct NamespaceSlots {
    /// The registry ModuleId whose layout the table uses; compiled code
    /// trusts slot k only when its own module's id matches.
    layout: u32,
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

    #[cfg(test)]
    pub(crate) fn degraded(&self) -> bool {
        self.degraded
    }

    #[cfg(test)]
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
            if self.hashes[k] == hash
                && (self.names[k] == key || unsafe { str_bytes(self.names[k]) == str_bytes(key) })
            {
                return Some(k);
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
        let table = &mut *dict_storage(dict).slots.slots()?;
        (!table.degraded).then_some(table)
    }
}

/// The live, non-degraded table of an exact dictionary.
#[cfg(test)]
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
    unsafe { dict_storage(dict).slots.slots().is_some() }
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
        let Some(slots) = dict_storage(dict).slots.slots() else {
            return;
        };
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
    layout: u32,
    names: impl ExactSizeIterator<Item = &'n [u8]>,
) {
    unsafe {
        if object_type_id(dict) != TYPE_ID_DICT || !dict_storage(dict).slots.is_none() {
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
            layout,
            degraded: false,
            names: name_bits.into_boxed_slice(),
            probe,
            hashes: hashes.into_boxed_slice(),
            row_slots: Vec::new(),
            rows: vec![NO_ROW; len].into_boxed_slice(),
            values: vec![UNBOUND; len].into_boxed_slice(),
        });
        dict_storage(dict).slots = NamespaceBinding(Box::into_raw(table) as usize);
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
    unsafe { bind(py, dict, id, names.into_iter()) };
}

/// Free a dictionary's binding. A table's names are immortal and its values
/// borrowed; static cells outlive the dictionary (program data), and their
/// values are released by shutdown retirement ([`retire_static_cells`]).
pub(crate) unsafe fn release(dict: *mut u8) {
    unsafe {
        let binding = std::mem::take(&mut dict_storage(dict).slots);
        if let Some(slots) = binding.slots() {
            drop(Box::from_raw(slots));
        } else if binding.cells().is_some() {
            drop(Box::from_raw((binding.0 & !CELLS_TAG) as *mut StaticCells));
        }
    }
}

/// The static cell of an exact `str` key in a cell-bound namespace, or None
/// for every other key and every other dictionary.
#[inline]
unsafe fn static_cell(
    py: &PyToken<'_>,
    dict: *mut u8,
    key: u64,
) -> Option<&'static std::sync::atomic::AtomicU64> {
    unsafe {
        let cells = dict_storage(dict).slots.cells()?;
        let name = crate::object::ops::exact_string_bytes(py, key)?;
        cells.index_of(name).map(|k| cells.cell(k))
    }
}

/// A by-name read of a namespace: the cell's value, retained, when `key`
/// names a bound static cell. `None` sends the caller to the dictionary,
/// which never holds a layout name, so an unbound cell misses there too.
pub(crate) fn static_cell_get(py: &PyToken<'_>, dict: *mut u8, key: u64) -> Option<u64> {
    let word = unsafe { static_cell(py, dict, key)? }.load(std::sync::atomic::Ordering::Acquire);
    if word == UNBOUND {
        return None;
    }
    inc_ref_bits(py, word);
    Some(word)
}

/// A by-name store into a namespace: when `key` names a static cell, the
/// cell takes `value` (retained) and releases its old word, as a compiled
/// store does; true. False for every other key.
pub(crate) fn static_cell_set(py: &PyToken<'_>, dict: *mut u8, key: u64, value: u64) -> bool {
    let Some(cell) = (unsafe { static_cell(py, dict, key) }) else {
        return false;
    };
    inc_ref_bits(py, value);
    let old = cell.swap(value, std::sync::atomic::Ordering::AcqRel);
    if old != UNBOUND {
        dec_ref_bits(py, old);
    }
    true
}

/// A by-name delete: `Some(true)` when `key` named a bound cell, now unbound;
/// `Some(false)` when it names an unbound one (the caller raises its
/// NameError or AttributeError); `None` for a key the cells do not hold.
pub(crate) fn static_cell_delete(py: &PyToken<'_>, dict: *mut u8, key: u64) -> Option<bool> {
    let cell = unsafe { static_cell(py, dict, key)? };
    let old = cell.swap(UNBOUND, std::sync::atomic::Ordering::AcqRel);
    if old == UNBOUND {
        return Some(false);
    }
    dec_ref_bits(py, old);
    Some(true)
}

/// The names and borrowed values of a cell-bound namespace's bound cells, in
/// layout order: `dir(module)` and `from module import *` list them with the
/// dictionary's keys. Empty for every other dictionary.
pub(crate) fn static_cell_items(dict: *mut u8) -> Vec<(&'static [u8], u64)> {
    let Some(cells) = (unsafe { dict_storage(dict).slots.cells() }) else {
        return Vec::new();
    };
    cells
        .names
        .iter()
        .enumerate()
        .filter_map(|(k, name)| {
            let word = cells.cell(k).load(std::sync::atomic::Ordering::Acquire);
            (word != UNBOUND).then_some((*name, word))
        })
        .collect()
}

/// Shutdown retirement clears a cell-bound namespace as it clears the
/// dictionary: each bound cell becomes unbound and drops its reference, in
/// layout order. A finalizer that stores a global again is cleared on the
/// retirement's next pass.
pub(crate) fn retire_static_cells(py: &PyToken<'_>, dict: *mut u8) -> bool {
    let Some(cells) = (unsafe { dict_storage(dict).slots.cells() }) else {
        return false;
    };
    let mut cleared = false;
    for k in 0..cells.names.len() {
        let old = cells
            .cell(k)
            .swap(UNBOUND, std::sync::atomic::Ordering::AcqRel);
        if old != UNBOUND {
            dec_ref_bits(py, old);
            cleared = true;
        }
    }
    cleared
}

/// `molt_static_cells_bind`: a cell-eligible module's init binds its static
/// cell block to the module object it just created, before any store. `base`
/// is the block as a slot base (an inline int of its address shifted right by
/// `GLOBAL_SLOT_BASE_SHIFT`). `guard_mask` holds the builtin binding guard
/// bits (HF-79) of the layout names some code may store: a cell store never
/// reaches the dictionary's guard state, so they are recorded now, before the
/// first store. The registry row named like the module supplies
/// the layout; the slot table that module creation attached is released (it
/// mirrors nothing yet). Never raises: a module without a matching row keeps
/// its dictionary namespace, which only costs speed, and the compiler emits
/// the call only where the row exists.
pub(crate) fn molt_static_cells_bind_attached_body(
    py: &crate::PyToken<'_>,
    module_bits: u64,
    base_bits: u64,
    guard_mask_bits: u64,
) -> u64 {
    let none = MoltObject::none().bits();
    let Some(module) = obj_from_bits(module_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_MODULE)
    else {
        return none;
    };
    let Some(dict) = obj_from_bits(unsafe { module_dict_bits(module) })
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_DICT)
    else {
        return none;
    };
    let Some(shifted) = obj_from_bits(base_bits).as_int() else {
        return none;
    };
    let Some(registry) = crate::builtins::module_table::module_registry() else {
        return none;
    };
    let name_bits = unsafe { module_name_bits(module) };
    let Some(id) = (unsafe { crate::object::ops::exact_string_bytes(py, name_bits) })
        .and_then(|name| std::str::from_utf8(name).ok())
        .and_then(|name| registry.id_of(name))
    else {
        return none;
    };
    let names: Box<[&'static [u8]]> = registry.globals_of(id).iter().collect();
    debug_assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    let words = ((shifted as u64) << molt_codegen_abi::global_slot::GLOBAL_SLOT_BASE_SHIFT) as usize
        as *mut u64;
    let guard_mask = obj_from_bits(guard_mask_bits).as_int().unwrap_or(-1) as u64;
    unsafe {
        crate::builtins::binding_guards::note_namespace_cell_bindings(dict, guard_mask);
        release(dict);
        let cells = Box::new(StaticCells { names, words });
        dict_storage(dict).slots = NamespaceBinding(Box::into_raw(cells) as usize | CELLS_TAG);
    }
    none
}

/// The all-unbound slot words a foreign-globals activation reads: every
/// compiled read of it takes the by-name path. Sized to the longest layout
/// of the installed registry, so any slot index of any module is in bounds.
fn unbound_slots() -> &'static [u64] {
    static UNBOUND_SLOTS: std::sync::OnceLock<Box<[u64]>> = std::sync::OnceLock::new();
    UNBOUND_SLOTS.get_or_init(|| {
        let longest = crate::builtins::module_table::module_registry().map_or(0, |registry| {
            (0..registry.count())
                .map(|id| registry.globals_of(id).iter().count())
                .max()
                .unwrap_or(0)
        });
        vec![UNBOUND; longest.max(1)].into_boxed_slice()
    })
}

/// The slot table of the executing frame's globals when it uses `layout`.
fn active_table(py: &PyToken<'_>, layout: u32) -> Option<(*mut u8, &'static mut NamespaceSlots)> {
    let globals = crate::builtins::frames::frame_stack_active_globals_bits();
    let dict = crate::builtins::frames::globals_namespace_storage_ptr(py, globals)
        .ok()
        .flatten()?;
    let table = unsafe { table_of(dict)? };
    (table.layout == layout).then_some((dict, table))
}

fn module_id(bits: u64) -> Option<u32> {
    obj_from_bits(bits)
        .as_int()
        .and_then(|id| u32::try_from(id).ok())
}

/// `GlobalSlotBase`: the slot words compiled reads of `module_id`'s globals
/// use in this activation, as an inline int of the address shifted right by
/// `GLOBAL_SLOT_BASE_SHIFT`. The executing frame's globals table when it uses
/// that module's layout; else the all-unbound words. Neither raises nor
/// allocates. Pure within one activation: the frame's globals never change,
/// and a table never moves while its dictionary lives.
pub(crate) fn molt_global_slot_base_attached_body(
    _py: &crate::PyToken<'_>,
    module_id_bits: u64,
) -> u64 {
    // Free-threaded execution has no safe unlocked slot read yet (a reader
    // could retain a value a concurrent store just released): every site
    // takes its by-name path there.
    let words = module_id(module_id_bits)
        .filter(|_| !molt_codegen_abi::MOLT_REFCOUNT_ATOMIC)
        .and_then(|layout| active_table(_py, layout))
        .map_or(unbound_slots().as_ptr(), |(_, table)| table.values.as_ptr());
    let shifted = (words as usize as u64) >> molt_codegen_abi::global_slot::GLOBAL_SLOT_BASE_SHIFT;
    MoltObject::from_int(shifted as i64).bits()
}

/// `GlobalSlotSet`: store `value` into slot `index` of `module_id`'s globals.
/// A bound slot replaces its dictionary row through the dictionary's own
/// replacement commit: no name is hashed. Otherwise the store takes the
/// by-name path the site had: the module object's namespace when `module`
/// is a module (module-level code), else the frame's globals. Returns None.
pub(crate) fn molt_global_slot_set_attached_body(
    _py: &crate::PyToken<'_>,
    module_id_bits: u64,
    index_bits: u64,
    module_bits: u64,
    name_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let none = MoltObject::none().bits();
        if let (Some(layout), Some(index)) = (
            module_id(module_id_bits),
            obj_from_bits(index_bits)
                .as_int()
                .and_then(|index| usize::try_from(index).ok()),
        ) && let Some((dict, table)) = active_table(py, layout)
            && index < table.len()
            && let Some(row) = table.row(index)
            && unsafe { crate::object::ops::dict_require_writable(py, dict) }
        {
            drop(unsafe { crate::object::ops::dict_replace_row(py, dict, row, value_bits) });
            return none;
        }
        if exception_pending(py) {
            return none;
        }
        if obj_from_bits(module_bits).is_none() {
            // The site's own op: DICT_SET(globals(), name, value).
            let globals = crate::builtins::frames::molt_globals_builtin_attached_body(py);
            if !exception_pending(py) {
                let _ = crate::object::ops_dict::molt_dict_set_attached_body(
                    py, globals, name_bits, value_bits,
                );
            }
            dec_ref_bits(py, globals);
            return none;
        }
        let _ = crate::builtins::modules::module_set_attr(py, module_bits, name_bits, value_bits);
        none
    })
}

#[cfg(test)]
#[path = "namespace_slots_tests.rs"]
mod tests;

include!("../attached_exports_generated/builtins/namespace_slots.rs");
