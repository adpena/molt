use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::wasm_abi_generated::{WasmRuntimeImport, wasm_runtime_import};

#[derive(Clone)]
pub(crate) struct TrackedImportIds {
    inner: BTreeMap<WasmRuntimeImport, u32>,
    by_index: BTreeMap<u32, WasmRuntimeImport>,
    used: Rc<RefCell<BTreeSet<WasmRuntimeImport>>>,
}

impl TrackedImportIds {
    pub(crate) fn new(inner: BTreeMap<WasmRuntimeImport, u32>) -> Self {
        let by_index = inner
            .iter()
            .filter_map(|(&import, &index)| (index != u32::MAX).then_some((index, import)))
            .collect();
        Self {
            inner,
            by_index,
            used: Rc::new(RefCell::new(BTreeSet::new())),
        }
    }

    pub(crate) fn insert(&mut self, key: WasmRuntimeImport, value: u32) {
        if let Some(previous) = self.inner.insert(key, value)
            && previous != u32::MAX
            && self.by_index.get(&previous) == Some(&key)
        {
            self.by_index.remove(&previous);
        }
        if value != u32::MAX {
            let displaced = self.by_index.insert(value, key);
            assert!(
                displaced.is_none() || displaced == Some(key),
                "duplicate WASM runtime import function index {value}"
            );
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return import names that were registered but never accessed.
    pub(crate) fn unused_names(&self) -> Vec<String> {
        let used = self.used.borrow();
        let mut names: Vec<String> = self
            .inner
            .keys()
            .filter(|key| !used.contains(key))
            .map(|key| key.name().to_string())
            .collect();
        names.sort();
        names
    }

    pub(crate) fn get(&self, key: WasmRuntimeImport) -> Option<&u32> {
        let val = self.inner.get(&key);
        if val.is_some() {
            self.used.borrow_mut().insert(key);
        }
        val
    }

    /// Check existence without marking the import as used.
    pub(crate) fn contains_key(&self, key: WasmRuntimeImport) -> bool {
        self.inner.contains_key(&key)
    }

    /// Check whether codegen actually referenced an import.
    pub(crate) fn is_used(&self, key: WasmRuntimeImport) -> bool {
        self.used.borrow().contains(&key)
    }

    pub(crate) fn is_used_name(&self, name: &str) -> bool {
        wasm_runtime_import(name).is_some_and(|key| self.is_used(key))
    }

    pub(crate) fn import_for_index(&self, index: u32) -> Option<WasmRuntimeImport> {
        self.by_index.get(&index).copied()
    }
}

impl std::ops::Index<WasmRuntimeImport> for TrackedImportIds {
    type Output = u32;
    fn index(&self, key: WasmRuntimeImport) -> &u32 {
        self.used.borrow_mut().insert(key);
        &self.inner[&key]
    }
}

pub(crate) fn selected_import_id(
    import_ids: &TrackedImportIds,
    import_key: WasmRuntimeImport,
    func_name: &str,
    op_kind: &str,
) -> u32 {
    let import_id = import_ids[import_key];
    assert_ne!(
        import_id,
        u32::MAX,
        "wasm auto import pruning removed required import '{}' for op '{op_kind}' in {func_name}",
        import_key.name()
    );
    import_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_index_identity_is_constant_time_and_tracks_replacement() {
        let first = WasmRuntimeImport::RuntimeInit;
        let second = WasmRuntimeImport::RuntimeShutdown;
        let mut imports = TrackedImportIds::new(BTreeMap::from([(first, 3), (second, u32::MAX)]));

        assert_eq!(imports.import_for_index(3), Some(first));
        assert_eq!(imports.import_for_index(u32::MAX), None);

        imports.insert(first, 7);
        imports.insert(second, 9);
        assert_eq!(imports.import_for_index(3), None);
        assert_eq!(imports.import_for_index(7), Some(first));
        assert_eq!(imports.import_for_index(9), Some(second));
    }
}
