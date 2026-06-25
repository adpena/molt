use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

#[derive(Clone)]
pub(crate) struct TrackedImportIds {
    inner: BTreeMap<String, u32>,
    used: Rc<RefCell<BTreeSet<String>>>,
}

impl TrackedImportIds {
    pub(crate) fn new(inner: BTreeMap<String, u32>) -> Self {
        Self {
            inner,
            used: Rc::new(RefCell::new(BTreeSet::new())),
        }
    }

    pub(crate) fn insert(&mut self, key: String, value: u32) {
        self.inner.insert(key, value);
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
            .filter(|k| !used.contains(k.as_str()))
            .cloned()
            .collect();
        names.sort();
        names
    }

    pub(crate) fn get(&self, key: &str) -> Option<&u32> {
        let val = self.inner.get(key);
        if val.is_some() {
            self.used.borrow_mut().insert(key.to_string());
        }
        val
    }

    /// Check existence without marking the import as used.
    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }

    /// Check whether codegen actually referenced an import.
    pub(crate) fn is_used(&self, key: &str) -> bool {
        self.used.borrow().contains(key)
    }
}

impl std::ops::Index<&str> for TrackedImportIds {
    type Output = u32;
    fn index(&self, key: &str) -> &u32 {
        self.used.borrow_mut().insert(key.to_string());
        &self.inner[key]
    }
}

pub(crate) fn selected_import_id(
    import_ids: &TrackedImportIds,
    import_key: &str,
    func_name: &str,
    op_kind: &str,
) -> u32 {
    let import_id = import_ids[import_key];
    assert_ne!(
        import_id,
        u32::MAX,
        "wasm auto import pruning removed required import '{import_key}' for op '{op_kind}' in {func_name}"
    );
    import_id
}
