use super::*;

/// A const-string intrinsic name passed as a call argument (the wrapper case,
/// e.g. `_require_callable_intrinsic("molt_gc_collect")`) is captured even
/// though it is not the direct `call_func(require_intrinsic, const_str)` shape.
/// This is the regression the broadened scan fixes (`gc.collect()` etc.).
#[test]
fn manifest_captures_wrapper_indirected_intrinsic_name() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("name", "molt_gc_collect"),
        // `_require_callable_intrinsic(name)` — a user wrapper call.
        make_call_func("res", "wrapper", &["name"]),
    ]);
    let manifest = compute_intrinsic_manifest(&[func], &symbols);
    assert!(
        manifest.contains("molt_gc_collect"),
        "wrapper-indirected intrinsic name must be in the manifest"
    );
}

/// `compute_intrinsic_manifest_checked` on a module with NO `molt_`-prefixed
/// const_str (the CLI's empty post-build feature-probe module, or an app
/// reaching every intrinsic by direct call) yields the empty manifest WITHOUT
/// requiring the staticlib symbol set. This is the regression for the
/// probe-panic loop: the probe runs the backend on `functions: []` with no
/// `MOLT_RUNTIME_INTRINSIC_SYMBOLS` staged, and unconditionally requiring the
/// set wedged every backend rebuild behind "feature mismatch; cleaning and
/// rebuilding".
#[test]
fn manifest_checked_empty_module_needs_no_symbol_set() {
    // The empty probe module.
    assert!(compute_intrinsic_manifest_checked(&[]).is_empty());
    // A module with ops but no molt_-prefixed const_str.
    let func = manifest_func(vec![
        make_const_str("s", "hello world"),
        make_call_func("res", "print", &["s"]),
    ]);
    assert!(compute_intrinsic_manifest_checked(&[func]).is_empty());
}

/// A const-string that names a symbol absent from the linked staticlib (e.g.
/// a crypto intrinsic on the micro profile) must NOT be captured — taking its
/// address would leave an unresolvable relocation.
#[test]
fn manifest_excludes_intrinsic_absent_from_staticlib() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("name", "molt_pbkdf2_hmac"), // not in `symbols`
        make_call_func("res", "wrapper", &["name"]),
    ]);
    let manifest = compute_intrinsic_manifest(&[func], &symbols);
    assert!(
        !manifest.contains("molt_pbkdf2_hmac"),
        "an intrinsic absent from the staticlib must never be address-taken"
    );
}

/// A const-string that merely begins with `molt_` but is free-text (a
/// diagnostic message, not a symbol) must not be captured.
#[test]
fn manifest_excludes_non_symbol_molt_strings() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("msg", "molt_sys_platform intrinsic unavailable"),
        make_call_func("res", "panic", &["msg"]),
    ]);
    let manifest = compute_intrinsic_manifest(&[func], &symbols);
    assert!(
        manifest.is_empty(),
        "free-text molt_ strings must not enter the manifest"
    );
}

/// A const-string intrinsic name that is stored (not passed directly to a
/// call) MUST still be captured: the name can flow through an object field
/// and be resolved later (sys.py's `_LazyIntrinsic` stashes the name in
/// `self._name`). Missing it silently degrades to the wrapper's fallback
/// value, so the data-flow-complete scan keeps every intrinsic-named
/// const_str.
#[test]
fn manifest_captures_stored_intrinsic_name() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("name", "molt_gc_collect"),
        // `name` is stored in an object field, not passed directly to a call —
        // it is still resolved later via `_require_intrinsic(self._name)`.
        make_store_var("slot", "name"),
    ]);
    let manifest = compute_intrinsic_manifest(&[func], &symbols);
    assert!(
        manifest.contains("molt_gc_collect"),
        "an intrinsic name stored for later resolution must be captured"
    );
}

/// The filter is EXACT membership in the staticlib symbol set — there is no
/// structural heuristic fallback. A well-formed `molt_`-prefixed identifier that
/// is NOT in the set (e.g. an intrinsic feature-gated out of the active stdlib
/// profile) must be excluded, because address-taking an absent symbol leaves a
/// dangling relocation that corrupts the binary. This locks in the contract that
/// replaced the prior "degrade safely" heuristic (which itself enabled the
/// corruption class).
#[test]
fn manifest_excludes_well_formed_name_absent_from_symbol_set() {
    // Only `molt_gc_collect` is defined by the (simulated) staticlib.
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("present", "molt_gc_collect"),
        make_call_func("r1", "wrapper", &["present"]),
        // A structurally valid intrinsic identifier that is feature-gated out of
        // this profile's staticlib — must NOT be address-taken.
        make_const_str("absent", "molt_pbkdf2_hmac"),
        make_call_func("r2", "wrapper", &["absent"]),
    ]);
    let manifest = compute_intrinsic_manifest(&[func], &symbols);
    assert!(manifest.contains("molt_gc_collect"));
    assert!(
        !manifest.contains("molt_pbkdf2_hmac"),
        "a well-formed molt_ identifier absent from the staticlib must be excluded"
    );
}

#[test]
fn manifest_captures_async_sleep_public_symbol_directly() {
    let symbols: BTreeSet<String> = ["molt_async_sleep".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("nm", "molt_async_sleep"),
        make_call_func("res", "require_intrinsic", &["nm"]),
    ]);
    let manifest = compute_intrinsic_manifest(&[func], &symbols);
    assert!(manifest.contains("molt_async_sleep"));
    assert!(!manifest.contains("molt_async_sleep_new"));
}

/// A non-override intrinsic name is captured verbatim (the common case must be
/// untouched by the override remapping).
#[test]
fn manifest_keeps_non_override_name_verbatim() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("nm", "molt_gc_collect"),
        make_call_func("res", "require_intrinsic", &["nm"]),
    ]);
    let manifest = compute_intrinsic_manifest(&[func], &symbols);
    assert!(manifest.contains("molt_gc_collect"));
}
