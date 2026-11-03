//! Bounded raw type lookup observations. Namespace owners retain the values;
//! this cache owns only exact-string keys so address reuse cannot alias a key.
use crate::{
    MoltObject, PyToken, TYPE_ID_STRING, dec_ref_bits, inc_ref_bits, obj_from_bits,
    object_class_bits, object_type_id, string_len,
};
use std::sync::Mutex;

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
    entries: Mutex<Box<[Option<Entry>; SLOTS]>>,
}

impl ClassLookupCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(Box::new([None; SLOTS])),
        }
    }

    pub(super) fn lookup(&self, key: LookupKey) -> Option<LookupValue> {
        crate::gil_assert();
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key.slot())
            .copied()
            .flatten()
            .filter(|entry| entry.key == key)
            .map(|entry| entry.value)
    }

    /// Caller verifies the original key's versions after resolution, including
    /// callbacks and lazy declaration publication. No callback runs under lock.
    pub(super) fn publish(&self, py: &PyToken<'_>, key: LookupKey, value: LookupValue) {
        crate::gil_assert();
        inc_ref_bits(py, key.name);
        let displaced = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            entries[key.slot()].replace(Entry { key, value })
        };
        if let Some(entry) = displaced {
            dec_ref_bits(py, entry.key.name);
        }
    }

    pub(crate) fn clear(&self, py: &PyToken<'_>) -> bool {
        crate::gil_assert();
        let entries = std::mem::replace(
            &mut **self.entries.lock().unwrap_or_else(|e| e.into_inner()),
            [None; SLOTS],
        );
        let changed = entries.iter().any(Option::is_some);
        for entry in entries.into_iter().flatten() {
            dec_ref_bits(py, entry.key.name);
        }
        changed
    }
}
