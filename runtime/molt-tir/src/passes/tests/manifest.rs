use super::*;
use molt_ir::python_builtin_callables_generated::PYTHON_BUILTIN_CALLABLES;

#[test]
fn app_callable_manifest_publication_retains_the_supported_builtin_namespace() {
    let symbols: BTreeSet<String> = PYTHON_BUILTIN_CALLABLES
        .iter()
        .map(|spec| spec.runtime_name.to_owned())
        .collect();
    for target in [
        None,
        Some("molt_module_cache_set"),
        Some("module_cache_set"),
    ] {
        for name in [Some("builtins"), Some("other"), None] {
            let mut ops = Vec::new();
            if let Some(name) = name {
                ops.push(make_const_str("name", name));
            }
            ops.push(OpIR {
                kind: if target.is_some() {
                    "call"
                } else {
                    "module_cache_set"
                }
                .into(),
                s_value: target.map(str::to_owned),
                args: Some(vec!["name".into(), "module".into()]),
                ..Default::default()
            });
            let expected = if name == Some("other") {
                BTreeSet::new()
            } else {
                symbols.clone()
            };
            let functions = [manifest_func(ops)];
            let requirements = collect_app_callable_requirements(&functions);
            assert!(
                requirements.builtin_trampolines.is_empty(),
                "publication must not add mandatory capability roots"
            );
            assert_eq!(
                requirements
                    .builtin_namespace_trampolines
                    .into_keys()
                    .collect::<BTreeSet<_>>(),
                expected
            );
            assert_eq!(
                compute_app_callable_manifest(&functions, &symbols),
                expected
            );
        }
    }
}

#[test]
fn app_callable_manifest_canonicalizes_explicit_intrinsic_alias_candidates() {
    let function = manifest_func(vec![
        make_const_str("alias", "_molt_time_time"),
        make_const_str("primary", "molt_time_time"),
        make_const_str("not_an_alias", "__molt_getframe"),
    ]);
    let symbols = BTreeSet::from(["molt_time_time".into(), "molt_getframe".into()]);
    assert_eq!(
        compute_app_callable_manifest(&[function], &symbols),
        BTreeSet::from(["molt_time_time".into()])
    );
}

#[test]
fn app_callable_manifest_retains_every_named_builtin_without_specializing_lookup() {
    let symbols = PYTHON_BUILTIN_CALLABLES
        .iter()
        .map(|spec| spec.runtime_name.to_owned())
        .collect();
    for spec in PYTHON_BUILTIN_CALLABLES {
        for target in [
            None,
            Some("molt_module_get_global"),
            Some("module_get_global"),
        ] {
            let lookup = OpIR {
                kind: if target.is_some() {
                    "call"
                } else {
                    "module_get_global"
                }
                .into(),
                s_value: target.map(str::to_owned),
                args: Some(vec!["module".into(), "name".into()]),
                out: Some("callable".into()),
                ..Default::default()
            };
            let function = manifest_func(vec![make_const_str("name", spec.python_name), lookup]);
            let result = compute_app_callable_manifest(std::slice::from_ref(&function), &symbols);
            assert_eq!(
                result,
                BTreeSet::from([spec.runtime_name.to_owned()]),
                "{}",
                spec.python_name
            );
            assert!(function.ops[1].runtime_symbol.is_none());
        }
    }
}

#[test]
fn app_callable_manifest_computed_or_redefined_names_retain_only_generated_builtins() {
    let expected: BTreeSet<String> = PYTHON_BUILTIN_CALLABLES
        .iter()
        .map(|spec| spec.runtime_name.to_owned())
        .collect();
    let mut symbols = expected.clone();
    symbols.insert("molt_unreachable_intrinsic".into());
    for redefinition in [
        None,
        Some(make_store_var("name", "dynamic")),
        Some(make_const_str("name", "unrelated")),
    ] {
        let mut function = manifest_func(vec![make_const_str("name", "globals")]);
        if let Some(redefinition) = redefinition {
            function.ops.push(redefinition);
        } else {
            function.params.push("name".into());
        }
        function.ops.push(OpIR {
            kind: "module_get_global".into(),
            args: Some(vec!["module".into(), "name".into()]),
            ..Default::default()
        });
        assert_eq!(
            compute_app_callable_manifest(&[function], &symbols),
            expected
        );
    }
}

#[test]
fn app_callable_manifest_unrelated_names_and_shadowed_runtime_calls_add_no_builtin_roots() {
    let symbols = PYTHON_BUILTIN_CALLABLES
        .iter()
        .map(|spec| spec.runtime_name.to_owned())
        .collect();
    let function = manifest_func(vec![
        make_const_str("name", "application_global"),
        OpIR {
            kind: "module_get_global".into(),
            args: Some(vec!["module".into(), "name".into()]),
            ..Default::default()
        },
    ]);
    assert!(compute_app_callable_manifest(&[function], &symbols).is_empty());
    let mut shadow = manifest_func(vec![
        make_const_str("name", "globals"),
        OpIR {
            kind: "call".into(),
            s_value: Some("molt_module_get_global".into()),
            args: Some(vec!["module".into(), "name".into()]),
            ..Default::default()
        },
    ]);
    shadow.name = "molt_module_get_global".into();
    assert!(compute_app_callable_manifest(&[shadow], &symbols).is_empty());
}

#[test]
fn app_callable_manifest_retains_qualified_acquisitions_without_literal_symbol_ops() {
    let symbol = "molt_sys_gettrace";
    let function = manifest_func(vec![OpIR {
        kind: "module_get_attr".into(),
        runtime_symbol: Some(symbol.into()),
        ..Default::default()
    }]);
    assert_eq!(
        compute_app_callable_manifest(&[function], &BTreeSet::from([symbol.into()])),
        BTreeSet::from([symbol.into()])
    );
}

/// A const-string intrinsic name passed as a call argument (the wrapper case,
/// e.g. `_require_callable_intrinsic("molt_gc_collect")`) is captured even
/// though it is not the direct `call_func(require_intrinsic, const_str)` shape.
/// This is the regression the broadened scan fixes (`gc.collect()` etc.).
#[test]
fn app_callable_manifest_captures_wrapper_indirected_intrinsic_name() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("name", "molt_gc_collect"),
        // `_require_callable_intrinsic(name)` — a user wrapper call.
        make_call_func("res", "wrapper", &["name"]),
    ]);
    let manifest = compute_app_callable_manifest(&[func], &symbols);
    assert!(
        manifest.contains("molt_gc_collect"),
        "wrapper-indirected intrinsic name must be in the manifest"
    );
}

/// `compute_app_callable_manifest_checked` on a module with NO `molt_`-prefixed
/// const_str or builtin function (the CLI's empty post-build feature-probe
/// module) yields the empty manifest WITHOUT
/// requiring the staticlib symbol set. This is the regression for the
/// probe-panic loop: the probe runs the backend on `functions: []` with no
/// `MOLT_RUNTIME_CALLABLE_SYMBOLS` staged, and unconditionally requiring the
/// set wedged every backend rebuild behind "feature mismatch; cleaning and
/// rebuilding".
#[test]
fn app_callable_manifest_checked_empty_module_needs_no_symbol_set() {
    // The empty probe module.
    assert!(compute_app_callable_manifest_checked(&[]).is_empty());
    // A module with ops but no molt_-prefixed const_str.
    let func = manifest_func(vec![
        make_const_str("s", "hello world"),
        make_call_func("res", "print", &["s"]),
    ]);
    assert!(compute_app_callable_manifest_checked(&[func]).is_empty());
}

/// A const-string that names a symbol absent from the linked staticlib (e.g.
/// a crypto intrinsic on the micro profile) must NOT be captured — taking its
/// address would leave an unresolvable relocation.
#[test]
fn app_callable_manifest_excludes_intrinsic_absent_from_staticlib() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("name", "molt_pbkdf2_hmac"), // not in `symbols`
        make_call_func("res", "wrapper", &["name"]),
    ]);
    let manifest = compute_app_callable_manifest(&[func], &symbols);
    assert!(
        !manifest.contains("molt_pbkdf2_hmac"),
        "an intrinsic absent from the staticlib must never be address-taken"
    );
}

/// A const-string that merely begins with `molt_` but is free-text (a
/// diagnostic message, not a symbol) must not be captured.
#[test]
fn app_callable_manifest_excludes_non_symbol_molt_strings() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("msg", "molt_sys_platform intrinsic unavailable"),
        make_call_func("res", "panic", &["msg"]),
    ]);
    let manifest = compute_app_callable_manifest(&[func], &symbols);
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
fn app_callable_manifest_captures_stored_intrinsic_name() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("name", "molt_gc_collect"),
        // `name` is stored in an object field, not passed directly to a call —
        // it is still resolved later via `_require_intrinsic(self._name)`.
        make_store_var("slot", "name"),
    ]);
    let manifest = compute_app_callable_manifest(&[func], &symbols);
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
fn app_callable_manifest_excludes_well_formed_name_absent_from_symbol_set() {
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
    let manifest = compute_app_callable_manifest(&[func], &symbols);
    assert!(manifest.contains("molt_gc_collect"));
    assert!(
        !manifest.contains("molt_pbkdf2_hmac"),
        "a well-formed molt_ identifier absent from the staticlib must be excluded"
    );
}

#[test]
fn app_callable_manifest_captures_async_sleep_public_symbol_directly() {
    let symbols: BTreeSet<String> = ["molt_async_sleep".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("nm", "molt_async_sleep"),
        make_call_func("res", "require_intrinsic", &["nm"]),
    ]);
    let manifest = compute_app_callable_manifest(&[func], &symbols);
    assert!(manifest.contains("molt_async_sleep"));
    assert!(!manifest.contains("molt_async_sleep_new"));
}

/// A non-override intrinsic name is captured verbatim (the common case must be
/// untouched by the override remapping).
#[test]
fn app_callable_manifest_keeps_non_override_name_verbatim() {
    let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
    let func = manifest_func(vec![
        make_const_str("nm", "molt_gc_collect"),
        make_call_func("res", "require_intrinsic", &["nm"]),
    ]);
    let manifest = compute_app_callable_manifest(&[func], &symbols);
    assert!(manifest.contains("molt_gc_collect"));
}

#[test]
fn app_callable_manifest_captures_reachable_builtin_function_runtime_name() {
    let symbols: BTreeSet<String> = ["molt_len".to_string(), "molt_ord".to_string()]
        .into_iter()
        .collect();
    let func = manifest_func(vec![
        make_builtin_func("len_func", "molt_len", 1),
        make_builtin_func("ord_func", "molt_ord", 1),
    ]);
    let manifest = compute_app_callable_manifest(&[func], &symbols);

    assert_eq!(
        manifest,
        BTreeSet::from(["molt_len".to_string(), "molt_ord".to_string()])
    );
}
