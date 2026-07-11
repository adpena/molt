//! Free-threading (PEP 703) module GIL-declaration registry.
//!
//! CPython 3.13+ lets a C-extension module declare whether it is safe to run
//! without the GIL, via the multi-phase-init `Py_mod_gil` slot or, for legacy
//! single-phase init, `PyUnstable_Module_SetGIL`. The semantics on a
//! free-threaded (`Py_GIL_DISABLED`) interpreter are load-bearing:
//!
//! * A module that declares `{Py_mod_gil, Py_MOD_GIL_NOT_USED}` may run with
//!   the GIL disabled (numpy ≥ 2.1 declares exactly this on 3.13+ in
//!   `numpy/_core/src/multiarray/multiarraymodule.c`).
//! * A module that declares `Py_MOD_GIL_USED`, or declares **nothing** (the
//!   default when the slot is absent), forces the interpreter to RE-ENABLE the
//!   GIL at import time with a `RuntimeWarning` (overridable only by the user
//!   via `PYTHON_GIL=0` / `-X gil=0`).
//!
//! Primary sources: <https://docs.python.org/3.14/c-api/module.html> and
//! <https://docs.python.org/3.14/howto/free-threading-extensions.html>.
//!
//! Molt today serializes runtime entry with its own GIL on native targets
//! (`molt-runtime`'s `concurrency::gil`), so — exactly like a non-free-threaded
//! CPython — the declaration has no behavioral effect yet and used to be
//! silently DISCARDED at the slot-processing sites. Discarding it was
//! forward-debt: the declaration is the ONE honest signal molt needs when its
//! native runtime goes free-threaded (an undeclared extension must then keep
//! GIL-equivalent serialization; a declared-free one may run unserialized).
//! This registry records every declaration at module-definition time so the
//! future free-threaded runtime — and today's support-matrix tooling — can
//! query it. See `docs/agent/FREE_THREADING_READINESS.md`.
//!
//! Cost: one map insert per module *definition* (import-time, once per
//! extension module, O(1) amortized). Zero bytes and zero instructions on any
//! per-object or per-call hot path; refcount paths are untouched.

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A module's effective PEP 703 GIL declaration, mirroring CPython's
/// `md_gil` semantics (`Objects/moduleobject.c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleGilDeclaration {
    /// The module never declared anything. CPython treats this exactly like an
    /// explicit `Py_MOD_GIL_USED`: on a free-threaded interpreter the import
    /// re-enables the GIL (with a `RuntimeWarning`). Kept distinct from
    /// [`ModuleGilDeclaration::GilUsedExplicit`] so support-matrix tooling can
    /// distinguish "author opted out" from "author never audited".
    GilUsedDefault,
    /// Explicit `{Py_mod_gil, Py_MOD_GIL_USED}` slot (or
    /// `PyUnstable_Module_SetGIL(m, Py_MOD_GIL_USED)`).
    GilUsedExplicit,
    /// Explicit `{Py_mod_gil, Py_MOD_GIL_NOT_USED}` slot (or
    /// `PyUnstable_Module_SetGIL(m, Py_MOD_GIL_NOT_USED)`): the module declares
    /// it is safe to run without the GIL.
    GilNotUsed,
}

impl ModuleGilDeclaration {
    /// Would importing this module force a free-threaded interpreter to
    /// re-enable the GIL? (CPython semantics: everything except an explicit
    /// `Py_MOD_GIL_NOT_USED` does.)
    #[inline]
    pub fn requires_gil(self) -> bool {
        !matches!(self, ModuleGilDeclaration::GilNotUsed)
    }

    /// True when the module explicitly declared a value (either polarity),
    /// i.e. the author audited it for free-threading.
    #[inline]
    pub fn is_explicit(self) -> bool {
        !matches!(self, ModuleGilDeclaration::GilUsedDefault)
    }
}

/// Registry: module name → declaration. Keyed by the module's `m_name` /
/// `__name__` (module identity in `sys.modules` is by name, and both the
/// slot path and the `PyUnstable_Module_SetGIL` path can produce it).
static MODULE_GIL_DECLARATIONS: Lazy<Mutex<HashMap<String, ModuleGilDeclaration>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Declarations that could not be keyed by name (NULL / non-UTF-8 `m_name`,
/// or a `PyUnstable_Module_SetGIL` call whose module name was unresolvable).
/// Counted rather than dropped so the aggregate picture stays honest: a
/// non-zero count means "something declared, but the registry cannot say who".
static UNRESOLVED_GIL_DECLARATIONS: AtomicUsize = AtomicUsize::new(0);

/// Record `decl` for module `name`.
///
/// Merge rule: an explicit declaration always wins; the implicit
/// [`ModuleGilDeclaration::GilUsedDefault`] never downgrades a previously
/// recorded explicit value (module creation paths overlap — e.g.
/// `PyModule_FromDefAndSpec` followed by `PyModule_ExecDef` records twice —
/// and a re-import must not erase the slot's signal).
pub fn record_module_gil_declaration(name: &str, decl: ModuleGilDeclaration) {
    let mut map = MODULE_GIL_DECLARATIONS.lock();
    match map.get(name) {
        Some(existing) if existing.is_explicit() && !decl.is_explicit() => {}
        _ => {
            map.insert(name.to_owned(), decl);
        }
    }
}

/// Record a declaration that could not be attributed to a named module.
pub fn record_unresolved_gil_declaration() {
    UNRESOLVED_GIL_DECLARATIONS.fetch_add(1, Ordering::Relaxed);
}

/// The declaration recorded for `name`, if any module definition with that
/// name has been processed.
pub fn module_gil_declaration(name: &str) -> Option<ModuleGilDeclaration> {
    MODULE_GIL_DECLARATIONS.lock().get(name).copied()
}

/// Names of all recorded modules whose import would force a free-threaded
/// interpreter to re-enable the GIL (undeclared or explicitly GIL-using).
/// Sorted for deterministic output. This is the honest input to any future
/// "can this program run free-threaded?" gate or support-matrix claim.
pub fn modules_requiring_gil() -> Vec<String> {
    let mut names: Vec<String> = MODULE_GIL_DECLARATIONS
        .lock()
        .iter()
        .filter(|(_, decl)| decl.requires_gil())
        .map(|(name, _)| name.clone())
        .collect();
    names.sort_unstable();
    names
}

/// Count of declarations that could not be keyed by module name.
pub fn unresolved_gil_declaration_count() -> usize {
    UNRESOLVED_GIL_DECLARATIONS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_declaration_is_recorded_and_queryable() {
        record_module_gil_declaration("gil_decl_test_notused", ModuleGilDeclaration::GilNotUsed);
        assert_eq!(
            module_gil_declaration("gil_decl_test_notused"),
            Some(ModuleGilDeclaration::GilNotUsed)
        );
        assert!(!ModuleGilDeclaration::GilNotUsed.requires_gil());
    }

    #[test]
    fn default_never_downgrades_an_explicit_declaration() {
        // The two-step init path (FromDefAndSpec + ExecDef) records more than
        // once; the second (default) pass must not erase the slot's signal.
        record_module_gil_declaration("gil_decl_test_merge", ModuleGilDeclaration::GilNotUsed);
        record_module_gil_declaration("gil_decl_test_merge", ModuleGilDeclaration::GilUsedDefault);
        assert_eq!(
            module_gil_declaration("gil_decl_test_merge"),
            Some(ModuleGilDeclaration::GilNotUsed)
        );
        // But an explicit flip IS honored (last explicit wins, matching
        // CPython where md_gil is simply assigned).
        record_module_gil_declaration("gil_decl_test_merge", ModuleGilDeclaration::GilUsedExplicit);
        assert_eq!(
            module_gil_declaration("gil_decl_test_merge"),
            Some(ModuleGilDeclaration::GilUsedExplicit)
        );
    }

    #[test]
    fn undeclared_and_explicit_used_require_gil_notused_does_not() {
        assert!(ModuleGilDeclaration::GilUsedDefault.requires_gil());
        assert!(ModuleGilDeclaration::GilUsedExplicit.requires_gil());
        assert!(!ModuleGilDeclaration::GilNotUsed.requires_gil());
        assert!(!ModuleGilDeclaration::GilUsedDefault.is_explicit());
        assert!(ModuleGilDeclaration::GilUsedExplicit.is_explicit());
        assert!(ModuleGilDeclaration::GilNotUsed.is_explicit());
    }

    #[test]
    fn modules_requiring_gil_lists_only_gil_requiring_entries() {
        record_module_gil_declaration("gil_decl_test_req_a", ModuleGilDeclaration::GilUsedDefault);
        record_module_gil_declaration("gil_decl_test_req_b", ModuleGilDeclaration::GilNotUsed);
        let requiring = modules_requiring_gil();
        assert!(requiring.iter().any(|n| n == "gil_decl_test_req_a"));
        assert!(!requiring.iter().any(|n| n == "gil_decl_test_req_b"));
    }

    #[test]
    fn unresolved_declarations_are_counted() {
        let before = unresolved_gil_declaration_count();
        record_unresolved_gil_declaration();
        assert_eq!(unresolved_gil_declaration_count(), before + 1);
    }
}
