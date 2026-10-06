//! Ordered retirement of the actual Python shutdown namespaces.
//!
//! Ordinary owners, then sys, then builtins reach a callback fixed point before
//! the next namespace is cleared. Pins preserve identities, never copies or
//! replacement builtin bindings. Cache aliases follow the same object lifetime.

use super::*;

pub(super) struct ModuleRetirement<'a, 'py> {
    py: &'a PyToken<'py>,
    phase: u8,
    // Each entry owns one reference. Reentrant replacement can add a new
    // namespace while the old captured namespace remains live for its callers.
    namespaces: Vec<(u64, u8)>,
}

impl<'a, 'py> ModuleRetirement<'a, 'py> {
    pub(super) fn new(py: &'a PyToken<'py>) -> Self {
        Self {
            py,
            phase: 0,
            namespaces: Vec::new(),
        }
    }

    fn dictionary(bits: u64) -> Option<u64> {
        let ptr = obj_from_bits(bits).as_ptr()?;
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_MODULE => Some(module_dict_bits(ptr)),
                TYPE_ID_DICT => Some(bits),
                _ => None,
            }
        }
    }

    fn keeps(&self, name: &str, bits: u64) -> bool {
        if (name == "sys" && self.phase < 1) || (name == "builtins" && self.phase < 2) {
            return true;
        }
        let dictionary = Self::dictionary(bits);
        self.namespaces.iter().any(|&(owned, phase)| {
            phase > self.phase
                && (owned == bits
                    || (dictionary.is_some() && dictionary == Self::dictionary(owned)))
        })
    }

    fn capture_namespaces(&mut self, state: &RuntimeState) {
        if self.phase < 1
            && let Some(bits) = state.interpreter_sys.module(self.py)
            && !self
                .namespaces
                .iter()
                .any(|&(old, rank)| old == bits && rank == 1)
        {
            inc_ref_bits(self.py, bits);
            self.namespaces.push((bits, 1));
        }
        for (name, phase) in [("sys", 1), ("builtins", 2)] {
            if phase > self.phase
                && let Some(bits) =
                    crate::builtins::module_table::retain_shutdown_namespace(self.py, state, name)
            {
                if self
                    .namespaces
                    .iter()
                    .any(|&(old, rank)| old == bits && rank == phase)
                {
                    dec_ref_bits(self.py, bits);
                } else {
                    self.namespaces.push((bits, phase));
                }
            }
        }
        {
            let cache = state.module_cache.lock().unwrap();
            for (name, phase) in [("sys", 1), ("builtins", 2)] {
                if phase <= self.phase {
                    continue;
                }
                if let Some(&bits) = cache.get(name)
                    && !self
                        .namespaces
                        .iter()
                        .any(|&(old, rank)| old == bits && rank == phase)
                {
                    self.namespaces.reserve(1);
                    inc_ref_bits(self.py, bits);
                    self.namespaces.push((bits, phase));
                }
            }
        }
    }

    pub(super) fn drain(&mut self, state: &RuntimeState) -> bool {
        crate::gil_assert();
        self.capture_namespaces(state);
        let mut modules = {
            let mut cache = state.module_cache.lock().unwrap();
            let mut detached = Vec::with_capacity(cache.len());
            cache.retain(|name, bits| {
                if self.keeps(name, *bits) {
                    true
                } else {
                    detached.push(*bits);
                    false
                }
            });
            detached
        };
        // The interpreter role retires with sys, independently of public
        // replacement/deletion. Its terminal sentinel forbids resurrection.
        if self.phase >= 1
            && let Some(bits) = state.interpreter_sys.take_for_shutdown(self.py)
        {
            modules.push(bits);
        }
        // Retire the registry projection before the first callback. Leaving
        // READY slots behind retains modules and permits torn namespaces to be
        // returned or reconstructed after the owning cache has been detached.
        modules.extend(
            crate::builtins::module_table::take_module_roots_for_shutdown(
                self.py,
                state,
                |name, bits| self.keeps(name, bits),
            ),
        );
        self.namespaces.retain(|&(bits, phase)| {
            if phase <= self.phase {
                modules.push(bits);
                false
            } else {
                true
            }
        });
        let changed = !modules.is_empty();
        let mut cleared = Vec::new();
        // Different cache keys and the registry each own an edge, but one
        // namespace is cleared only once per pass. All owners remain pinned
        // until every namespace in this detached cohort has been processed.
        for &bits in &modules {
            // Earlier finalizers may replace a core role with a module that
            // was ordinary when this cohort was detached. Capture the exact
            // newly published object before deciding its namespace lifetime.
            self.capture_namespaces(state);
            let Some(dictionary) = Self::dictionary(bits) else {
                continue;
            };
            if self.keeps("", bits) || cleared.contains(&dictionary) {
                continue;
            }
            let Some(ptr) = obj_from_bits(dictionary).as_ptr() else {
                continue;
            };
            if unsafe { object_type_id(ptr) } == TYPE_ID_DICT {
                cleared.push(dictionary);
                unsafe { dict_clear_in_place_shutdown(self.py, ptr) };
            }
        }
        for bits in modules {
            dec_ref_bits(self.py, bits);
        }
        changed
    }

    // The caller invokes this only when every callback owner and thread-state
    // domain has stopped changing. A new phase itself requires another pass.
    pub(super) fn advance(&mut self) -> bool {
        if self.phase == 2 {
            return false;
        }
        self.phase += 1;
        true
    }
}

impl Drop for ModuleRetirement<'_, '_> {
    fn drop(&mut self) {
        for (bits, _) in self.namespaces.drain(..) {
            dec_ref_bits(self.py, bits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static BUILTINS: AtomicU64 = AtomicU64::new(0);
    static SYS: AtomicU64 = AtomicU64::new(0);
    static FINALIZED: AtomicUsize = AtomicUsize::new(0);

    fn publish(py: &PyToken<'_>, name: &str, bits: u64) {
        inc_ref_bits(py, bits);
        let old = runtime_state(py)
            .module_cache
            .lock()
            .unwrap()
            .insert(name.into(), bits);
        if let Some(old) = old {
            dec_ref_bits(py, old);
        }
    }

    fn module(py: &PyToken<'_>, name: &[u8]) -> u64 {
        let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
        let bits = MoltObject::from_ptr(crate::alloc_module_obj(py, key)).bits();
        dec_ref_bits(py, key);
        bits
    }

    fn marker(py: &PyToken<'_>, module: u64, expected: i64) {
        let key = crate::attr_name_bits_from_bytes(py, b"sentinel").unwrap();
        let result = crate::builtins::modules::molt_module_get_attr(module, key);
        assert!(!exception_pending(py));
        assert_eq!(obj_from_bits(result).as_int(), Some(expected));
        dec_ref_bits(py, key);
        dec_ref_bits(py, result);
    }

    extern "C" fn finalizer(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            marker(py, BUILTINS.load(Ordering::SeqCst), 42);
            marker(py, SYS.load(Ordering::SeqCst), 43);
            // Reentrant aliases must inherit the same protected dictionary,
            // even when this callback runs after the cache drain began.
            publish(py, "late_builtin_alias", BUILTINS.load(Ordering::SeqCst));
            FINALIZED.fetch_add(1, Ordering::SeqCst);
            MoltObject::none().bits()
        })
    }

    #[test]
    fn namespaces_survive_ordinary_aliases_and_late_callback_roots() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let builtin = module(py, b"builtins");
                let sys = module(py, b"sys");
                crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
                BUILTINS.store(builtin, Ordering::SeqCst);
                SYS.store(sys, Ordering::SeqCst);
                FINALIZED.store(0, Ordering::SeqCst);
                let key = crate::attr_name_bits_from_bytes(py, b"sentinel").unwrap();
                crate::builtins::modules::molt_module_set_attr(
                    builtin,
                    key,
                    MoltObject::from_int(42).bits(),
                );
                crate::builtins::modules::molt_module_set_attr(
                    sys,
                    key,
                    MoltObject::from_int(43).bits(),
                );
                dec_ref_bits(py, key);
                for (name, bits) in [
                    ("builtins", builtin),
                    ("builtin_alias", builtin),
                    ("sys", sys),
                    ("sys_alias", sys),
                ] {
                    publish(py, name, bits);
                }
                let name = crate::attr_name_bits_from_bytes(py, b"NamespaceFinalizer").unwrap();
                let class = crate::molt_class_new(name);
                dec_ref_bits(py, name);
                crate::molt_class_set_base(class, crate::builtin_classes(py).object);
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
                let method = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::provenance::abi::expose_function_address(finalizer as *const ()),
                    1,
                );
                let method = MoltObject::from_ptr(method).bits();
                let key = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
                crate::molt_set_attr_name(class, key, method);
                dec_ref_bits(py, key);
                dec_ref_bits(py, method);
                let mut retirement = ModuleRetirement::new(py);
                let state = runtime_state(py);
                for index in 0..2 {
                    let owner = module(py, b"owner");
                    let value = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
                    let key = crate::attr_name_bits_from_bytes(py, b"value").unwrap();
                    crate::builtins::modules::molt_module_set_attr(owner, key, value);
                    dec_ref_bits(py, key);
                    dec_ref_bits(py, value);
                    publish(py, "owner", owner);
                    dec_ref_bits(py, owner);
                    assert!(retirement.drain(state));
                    assert_eq!(FINALIZED.load(Ordering::SeqCst), index + 1);
                    marker(py, builtin, 42);
                    marker(py, sys, 43);
                    assert!(
                        !retirement.drain(state),
                        "core roots must not keep the fixed point busy"
                    );
                }
                assert_eq!(state.interpreter_sys.module(py), Some(sys));
                assert!(retirement.advance());
                assert!(retirement.drain(state));
                assert!(state.interpreter_sys.module(py).is_none());
                assert!(
                    state.interpreter_sys.take_for_shutdown(py).is_none(),
                    "sys owner transferred exactly once"
                );
                marker(py, builtin, 42);
                assert!(unsafe {
                    crate::dict_order(
                        obj_from_bits(module_dict_bits(obj_from_bits(sys).as_ptr().unwrap()))
                            .as_ptr()
                            .unwrap(),
                    )
                    .is_empty()
                });
                assert!(retirement.advance());
                assert!(retirement.drain(state));
                assert!(!retirement.advance());
                assert!(!retirement.drain(state));
                assert!(state.module_cache.lock().unwrap().is_empty());
                for bits in [class, sys, builtin] {
                    dec_ref_bits(py, bits);
                }
                BUILTINS.store(0, Ordering::SeqCst);
                SYS.store(0, Ordering::SeqCst);
            });
        });
    }
}
