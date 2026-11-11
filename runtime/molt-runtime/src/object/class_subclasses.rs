//! Direct-subclass edges, so a class mutation reaches every subclass.
//!
//! A layout guard checks the receiver's own class at its layout version
//! (HF-220), but a class's field facts also depend on every class in its MRO:
//! a data descriptor added to a base shadows the subclass's inferred field.
//! CPython's `PyType_Modified` invalidates a type and all of its subclasses;
//! `class_bump_layout_version` uses these edges to do the same (HF-812).
//!
//! Only heap classes are bases here: static builtin types are immutable, so
//! no mutation of theirs can reach a subclass. Edges hold no references. A
//! subclass owns its bases tuple, so every base outlives the subclasses
//! registered under it, and terminal class release removes a class's edges
//! before its storage is freed. Bases change only through the two base
//! writers in `layout` and `class_model::hierarchy`; assigning `__bases__` to
//! a finished class raises `TypeError` (spec 0216).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::gc::RegistryAddressHash;
use crate::{TYPE_ID_TUPLE, obj_from_bits, object_type_id};

/// Heap base class address -> the addresses of its direct subclasses. Keys are
/// allocator-chosen addresses, so the GC registry's unkeyed address hash fits.
type Edges = HashMap<usize, HashSet<usize, RegistryAddressHash>, RegistryAddressHash>;

static EDGES: Mutex<Edges> = Mutex::new(HashMap::with_hasher(RegistryAddressHash::new()));

fn edges() -> MutexGuard<'static, Edges> {
    EDGES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The heap classes in a bases tuple; anything else contributes no edge.
unsafe fn heap_bases(bases_bits: u64) -> Vec<usize> {
    let Some(tuple) = obj_from_bits(bases_bits).as_ptr() else {
        return Vec::new();
    };
    unsafe {
        if object_type_id(tuple) != TYPE_ID_TUPLE {
            return Vec::new();
        }
        super::seq_access::with_borrowed(tuple, |bases| {
            bases
                .iter()
                .filter_map(|&base| obj_from_bits(base).as_ptr())
                .filter(|&base| super::class_storage::class_is_heap_type(base))
                .map(|base| base as usize)
                .collect()
        })
    }
}

/// `class` replaced the bases tuple `old_bases_bits` with `new_bases_bits`.
///
/// # Safety
/// Both values are bases slot contents (a tuple, None or 0) that the caller
/// keeps alive for the call.
pub(crate) unsafe fn rebase(class: *mut u8, old_bases_bits: u64, new_bases_bits: u64) {
    let (old, new) = unsafe { (heap_bases(old_bases_bits), heap_bases(new_bases_bits)) };
    if old == new {
        return;
    }
    let class = class as usize;
    let mut edges = edges();
    for base in old {
        if let Some(subclasses) = edges.get_mut(&base) {
            subclasses.remove(&class);
            if subclasses.is_empty() {
                edges.remove(&base);
            }
        }
    }
    for base in new {
        // Set before the edge exists, so a mutation that sees the edge also
        // sees the bit.
        unsafe { super::class_storage::class_mark_has_subclass(base as *mut u8) };
        edges.entry(base).or_default().insert(class);
    }
}

/// Terminal release of `class`, whose bases slot held `bases_bits`.
///
/// # Safety
/// `bases_bits` is the detached bases slot, still owned by the caller.
pub(crate) unsafe fn forget(class: *mut u8, bases_bits: u64) {
    unsafe {
        rebase(class, bases_bits, 0);
        if super::class_storage::class_has_subclass(class) {
            edges().remove(&(class as usize));
        }
    }
}

/// Apply `visit` to every class that inherits from `class`, transitively,
/// each once. `visit` runs under the edge lock: a class leaves the edges
/// before its storage is freed, so no visited class can be freed meanwhile.
/// `visit` must not reenter this module or run Python.
pub(crate) fn for_each_descendant(class: *mut u8, mut visit: impl FnMut(*mut u8)) {
    // Most mutations (every one during a class's own creation) find a class
    // that never had a subclass; they skip the lock.
    if !unsafe { super::class_storage::class_has_subclass(class) } {
        return;
    }
    let edges = edges();
    let Some(direct) = edges.get(&(class as usize)) else {
        return;
    };
    let mut seen: HashSet<usize, RegistryAddressHash> = direct.iter().copied().collect();
    let mut pending: Vec<usize> = direct.iter().copied().collect();
    while let Some(base) = pending.pop() {
        visit(base as *mut u8);
        if let Some(subclasses) = edges.get(&base) {
            for &sub in subclasses {
                if seen.insert(sub) {
                    pending.push(sub);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::for_each_descendant;
    use crate::object::layout::{class_bases_bits, class_layout_version_bits};
    use crate::{
        MoltObject, PyToken, builtin_classes, dec_ref_bits, exception_pending, obj_from_bits,
    };
    use std::collections::HashSet;

    fn class_named(py: &PyToken<'_>, name: &[u8]) -> u64 {
        let name = crate::object::builders::alloc_string(py, name);
        assert!(!name.is_null());
        let name = MoltObject::from_ptr(name).bits();
        let class = crate::molt_class_new_attached_body(py, name);
        dec_ref_bits(py, name);
        assert!(obj_from_bits(class).as_ptr().is_some());
        class
    }

    fn version(class: u64) -> u64 {
        unsafe { class_layout_version_bits(obj_from_bits(class).as_ptr().unwrap()) }
    }

    fn descendants(class: u64) -> HashSet<usize> {
        let mut found = HashSet::new();
        for_each_descendant(obj_from_bits(class).as_ptr().unwrap(), |sub| {
            found.insert(sub as usize);
        });
        found
    }

    fn address(class: u64) -> usize {
        obj_from_bits(class).as_ptr().unwrap() as usize
    }

    /// The expected relation is the declared graph: Child(Base),
    /// Grandchild(Child), Diamond(Child, Sibling) and an unrelated Sibling.
    #[test]
    fn a_class_mutation_leaves_every_subclass_version_once() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let object = builtin_classes(py).object;
            let base = class_named(py, b"Base");
            let child = class_named(py, b"Child");
            let grandchild = class_named(py, b"Grandchild");
            let sibling = class_named(py, b"Sibling");
            let diamond = class_named(py, b"Diamond");
            crate::molt_class_set_base_attached_body(py, base, object);
            crate::molt_class_set_base_attached_body(py, child, base);
            crate::molt_class_set_base_attached_body(py, grandchild, child);
            crate::molt_class_set_base_attached_body(py, sibling, object);
            let pair = crate::object::builders::alloc_tuple(py, &[child, sibling]);
            assert!(!pair.is_null());
            let pair = MoltObject::from_ptr(pair).bits();
            crate::molt_class_set_base_attached_body(py, diamond, pair);
            assert!(!exception_pending(py));
            assert_eq!(
                descendants(base),
                HashSet::from([address(child), address(grandchild), address(diamond)])
            );
            assert_eq!(descendants(sibling), HashSet::from([address(diamond)]));
            assert!(descendants(grandchild).is_empty());

            let classes = [base, child, grandchild, sibling, diamond];
            let before = classes.map(version);
            let name = crate::object::builders::alloc_string(py, b"x");
            let name = MoltObject::from_ptr(name).bits();
            crate::molt_set_attr_name_attached_body(py, base, name, MoltObject::from_int(1).bits());
            assert!(!exception_pending(py));
            let after = classes.map(version);
            let advanced = |index: usize| after[index] - before[index];
            assert!(advanced(0) > 0);
            // Each descendant advances exactly as often as the mutated class;
            // the diamond is reachable twice but advances once per mutation.
            assert_eq!(advanced(1), advanced(0));
            assert_eq!(advanced(2), advanced(0));
            assert_eq!(advanced(4), advanced(0));
            assert_eq!(advanced(3), 0);

            // Terminal release removes a class's edges, so a later mutation
            // never visits its storage.
            unsafe {
                let grandchild_ptr = obj_from_bits(grandchild).as_ptr().unwrap();
                super::forget(grandchild_ptr, class_bases_bits(grandchild_ptr));
            }
            assert_eq!(
                descendants(base),
                HashSet::from([address(child), address(diamond)])
            );
            for bits in [name, pair, diamond, sibling, grandchild, child, base] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }
}
