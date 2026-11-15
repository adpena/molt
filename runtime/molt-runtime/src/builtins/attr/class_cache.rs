//! Bounded raw type lookup observations. Namespace owners retain the values;
//! this cache owns only exact-string keys so address reuse cannot alias a key.
use crate::{
    MoltObject, PyToken, TYPE_ID_STRING, dec_ref_bits, inc_ref_bits, obj_from_bits,
    object_class_bits, object_type_id, string_len,
};

const SLOTS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LookupKey {
    class: u64,
    layout: u64,
    epoch: u64,
    name: u64,
}

impl LookupKey {
    pub(super) unsafe fn new(py: &PyToken<'_>, class: *mut u8, name: u64) -> Option<Self> {
        unsafe {
            let pointer = obj_from_bits(name).as_ptr()?;
            if object_type_id(pointer) != TYPE_ID_STRING {
                return None;
            }
            // Exact strings have no user finalizer. Bounded length also bounds
            // retained name memory independently of a caller's arbitrary input.
            let owner = object_class_bits(pointer);
            if string_len(pointer) > 100 || (owner != 0 && owner != crate::builtin_classes(py).str)
            {
                return None;
            }
            Some(Self {
                class: MoltObject::from_ptr(class).bits(),
                layout: crate::class_layout_version_bits(class),
                epoch: crate::object::global_type_version(),
                name,
            })
        }
    }
    fn slot(self) -> usize {
        (((self.class >> 4) ^ (self.name >> 4)) as usize) % SLOTS
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum LookupPurity {
    CallbackFree,
    CallbackDependent,
}

#[derive(Clone, Copy)]
pub(super) struct LookupValue {
    pub(super) value: Option<u64>,
    pub(super) name: Option<crate::NameId>,
    pub(super) purity: LookupPurity,
}

#[derive(Clone, Copy)]
struct Entry {
    key: LookupKey,
    value: LookupValue,
}

pub(crate) struct ClassLookupCache {
    // Keep this bounded cold table out of the runtime state's hot field layout.
    entries: Table,
}

type Slots = Box<[Option<Entry>; SLOTS]>;

/// The table. In a GIL build every access holds the GIL, which each method
/// proves with its `PyToken`, so the table is a plain cell: an attribute
/// lookup took a mutex lock and unlock it never contended (3.7% of
/// bench_struct's samples). Free-threaded execution keeps the mutex, as
/// list storage keeps its atomics (`object::backing`).
#[cfg(not(feature = "free-threaded"))]
struct Table(std::cell::UnsafeCell<Slots>);

// SAFETY: every access runs `Table::with`, which takes the `PyToken` that
// only the GIL holder has; no access re-enters the table.
#[cfg(not(feature = "free-threaded"))]
unsafe impl Sync for Table {}

// An access copies or replaces whole `Copy` entries, so a panic cannot leave
// a torn table; the mutex it replaces ignored poisoning for the same reason.
#[cfg(not(feature = "free-threaded"))]
impl std::panic::RefUnwindSafe for Table {}

#[cfg(not(feature = "free-threaded"))]
impl Table {
    fn new() -> Self {
        Self(std::cell::UnsafeCell::new(Box::new([None; SLOTS])))
    }

    fn with<R>(&self, _py: &PyToken<'_>, access: impl FnOnce(&mut Slots) -> R) -> R {
        crate::gil_assert();
        // SAFETY: the GIL serializes every access, and `access` never runs
        // Python or touches the table again.
        access(unsafe { &mut *self.0.get() })
    }
}

#[cfg(feature = "free-threaded")]
struct Table(std::sync::Mutex<Slots>);

#[cfg(feature = "free-threaded")]
impl Table {
    fn new() -> Self {
        Self(std::sync::Mutex::new(Box::new([None; SLOTS])))
    }

    fn with<R>(&self, _py: &PyToken<'_>, access: impl FnOnce(&mut Slots) -> R) -> R {
        access(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl ClassLookupCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Table::new(),
        }
    }

    pub(super) fn lookup(&self, py: &PyToken<'_>, key: LookupKey) -> Option<LookupValue> {
        self.entries.with(py, |entries| {
            entries[key.slot()]
                .filter(|entry| entry.key == key)
                .map(|entry| entry.value)
        })
    }

    /// Caller verifies the original key's versions after resolution, including
    /// callbacks and lazy declaration publication. No callback runs under lock.
    pub(super) fn publish(&self, py: &PyToken<'_>, key: LookupKey, value: LookupValue) {
        inc_ref_bits(py, key.name);
        let displaced = self.entries.with(py, |entries| {
            entries[key.slot()].replace(Entry { key, value })
        });
        if let Some(entry) = displaced {
            dec_ref_bits(py, entry.key.name);
        }
    }

    pub(crate) fn clear(&self, py: &PyToken<'_>) -> bool {
        let entries = self.entries.with(py, |entries| {
            std::mem::replace(&mut **entries, [None; SLOTS])
        });
        let changed = entries.iter().any(Option::is_some);
        for entry in entries.into_iter().flatten() {
            dec_ref_bits(py, entry.key.name);
        }
        changed
    }
}
