//! Builtin binding guards (HF-79).
//!
//! Inside a Python function, a read of a builtin name such as `len` is a live
//! lookup: the active globals may bind the name, and any code may rebind or
//! delete `builtins.len`. The compiler specializes such a call only behind
//! [`molt_guard_builtin_bindings`], as CPython 3.12 guards
//! `LOAD_GLOBAL_BUILTIN`. The guard holds when, for every name in its mask,
//! the active globals never bound the name and the builtins namespace still
//! holds the binding that the runtime published.
//!
//! Namespace dictionaries carry that fact in their inline aux state word. A
//! module namespace is born tracked (`NAMESPACE_TRACKED`). The builtins
//! namespace is also sealed (`BUILTINS_SEALED`) right after the runtime
//! publishes it. Below those flags, bit `i` belongs to guard name `i`
//! (`runtime/builtin_binding_guards.toml`). Every dictionary mutation that
//! can bind, rebind or delete a key reaches one of the hooks below through the
//! dictionary commit primitives, and sets the key's bit. Bits never clear:
//! a stale bit costs only the fast path, never correctness. A dictionary
//! without a tracked state word, such as a user dict passed as function
//! globals, never satisfies a guard.

use crate::object::{object_set_state, object_state};
use crate::*;

#[path = "binding_guards_generated.rs"]
mod table;

pub(crate) use table::{
    BUILTIN_BINDING_GUARD_ALL_MASK, BUILTIN_BINDING_GUARD_NAMES, builtin_binding_guard_bit,
};

/// The dictionary is a namespace that records guard bits.
const NAMESPACE_TRACKED: u64 = 1 << 63;
/// The namespace is the builtins dictionary, sealed after runtime publication.
const BUILTINS_SEALED: u64 = 1 << 62;

const _: () = assert!(BUILTIN_BINDING_GUARD_ALL_MASK & (NAMESPACE_TRACKED | BUILTINS_SEALED) == 0);

/// The state word of a tracked namespace dictionary, or `None`.
#[inline]
unsafe fn namespace_state(dict: *mut u8) -> Option<u64> {
    unsafe {
        if (*header_from_obj_ptr(dict)).aux_kind != crate::object::HEADER_AUX_KIND_STATE_INLINE
            || object_type_id(dict) != TYPE_ID_DICT
        {
            return None;
        }
    }
    let state = object_state(dict) as u64;
    (state & NAMESPACE_TRACKED != 0).then_some(state)
}

#[inline]
unsafe fn add_bits(dict: *mut u8, state: u64, bits: u64) {
    if state & bits != bits {
        object_set_state(dict, (state | bits) as i64);
    }
}

/// The guard bits a key can shadow. Only an exact `str` compares by its
/// bytes alone. Any other heap key may define `__eq__` and `__hash__` that
/// match a builtin name, so it conservatively covers every name. Inline
/// scalars (int, float, bool, None) never equal a `str`.
unsafe fn key_guard_bits(py: &PyToken<'_>, key_bits: u64) -> u64 {
    if obj_from_bits(key_bits).as_ptr().is_none() {
        return 0;
    }
    match unsafe { crate::object::ops::exact_string_bytes(py, key_bits) } {
        Some(bytes) => builtin_binding_guard_bit(bytes),
        None => BUILTIN_BINDING_GUARD_ALL_MASK,
    }
}

/// A key was inserted into `dict`. In module globals it now shadows the
/// builtin of the same name; in builtins it rebinds a deleted name.
#[inline]
pub(crate) unsafe fn note_namespace_insert(py: &PyToken<'_>, dict: *mut u8, key_bits: u64) {
    unsafe {
        if let Some(state) = namespace_state(dict) {
            add_bits(dict, state, key_guard_bits(py, key_bits));
        }
    }
}

/// A present key's value was replaced or the key was deleted. A module's bit
/// was set when the key was inserted, so only the builtins namespace records it.
#[inline]
pub(crate) unsafe fn note_namespace_rebind(py: &PyToken<'_>, dict: *mut u8, key_bits: u64) {
    unsafe {
        if let Some(state) = namespace_state(dict)
            && state & BUILTINS_SEALED != 0
        {
            add_bits(dict, state, key_guard_bits(py, key_bits));
        }
    }
}

/// The whole mapping was cleared or its contents replaced. A module records
/// the keys it now binds; in sealed builtins every name may have changed.
pub(crate) unsafe fn note_namespace_reset(py: &PyToken<'_>, dict: *mut u8) {
    unsafe {
        let Some(state) = namespace_state(dict) else {
            return;
        };
        let bits = if state & BUILTINS_SEALED != 0 {
            BUILTIN_BINDING_GUARD_ALL_MASK
        } else {
            crate::builtins::containers::dict_live_entries(dict)
                .fold(0, |bits, row| bits | key_guard_bits(py, row.key))
        };
        add_bits(dict, state, bits);
    }
}

/// Allocate an empty module namespace dictionary that records guard bits.
pub(crate) fn alloc_namespace_dict(py: &PyToken<'_>) -> *mut u8 {
    crate::object::builders::alloc_dict_with_state(py, NAMESPACE_TRACKED as i64)
}

/// The guard bits a namespace has recorded; zero for an untracked dict.
pub(crate) fn namespace_guard_bits(dict: *mut u8) -> u64 {
    unsafe { namespace_state(dict) }.map_or(0, |state| state & BUILTIN_BINDING_GUARD_ALL_MASK)
}

/// Seal the builtins namespace the runtime has just published. Publication
/// copies the live namespace before it writes the runtime's objects, so a
/// name bound before publication (`prior`, from `namespace_guard_bits`) keeps
/// its bit, and so does a name the runtime did not publish. Every other guard
/// name now holds the runtime's own object. Later bindings, including the
/// Python facade's, set bits again.
pub(crate) fn seal_builtins_namespace(py: &PyToken<'_>, dict: *mut u8, prior: u64) {
    unsafe {
        if namespace_state(dict).is_none() {
            return;
        }
        let mut absent = prior & BUILTIN_BINDING_GUARD_ALL_MASK;
        for (bit, name) in BUILTIN_BINDING_GUARD_NAMES.iter().enumerate() {
            let hash = crate::object::ops_hash::hash_string_bytes(py, name.as_bytes()) as u64;
            if !matches!(
                crate::object::ops::dict_exact_string_lookup(py, dict, name.as_bytes(), hash),
                crate::object::ops::ExactStringLookup::Found(_)
            ) {
                absent |= 1 << bit;
            }
        }
        object_set_state(dict, (NAMESPACE_TRACKED | BUILTINS_SEALED | absent) as i64);
    }
}

/// Whether every name in `mask` still resolves to the runtime's builtin in
/// the active Python frame: its globals never bound the name and its builtins
/// are the sealed builtins namespace, unchanged for the name.
fn active_frame_admits(mask: u64) -> bool {
    let Some((globals_bits, builtins_bits)) =
        crate::builtins::frames::frame_stack_active_namespaces()
    else {
        return false;
    };
    let (Some(globals), Some(builtins)) = (
        obj_from_bits(globals_bits).as_ptr(),
        obj_from_bits(builtins_bits).as_ptr(),
    ) else {
        return false;
    };
    unsafe {
        let (Some(globals_state), Some(builtins_state)) =
            (namespace_state(globals), namespace_state(builtins))
        else {
            return false;
        };
        globals_state & mask == 0
            && builtins_state & BUILTINS_SEALED != 0
            && builtins_state & mask == 0
    }
}

/// The guard compiled code evaluates where Python loads a guarded builtin.
/// `mask_bits` is an inline int of guard bits. Returns a bool object. It
/// reads only the frame stack and namespace state words: it neither
/// allocates nor raises, and an unknown mask never holds.
#[unsafe(no_mangle)]
pub extern "C" fn molt_guard_builtin_bindings(mask_bits: u64) -> u64 {
    let admitted = match obj_from_bits(mask_bits).as_int() {
        Some(mask) if mask > 0 && (mask as u64) & !BUILTIN_BINDING_GUARD_ALL_MASK == 0 => {
            active_frame_admits(mask as u64)
        }
        _ => false,
    };
    MoltObject::from_bool(admitted).bits()
}

#[cfg(test)]
#[path = "binding_guards_tests.rs"]
mod tests;
