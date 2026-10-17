use super::*;

fn env(value: serde_json::Value) -> BackendEnvironment {
    BackendEnvironment::from_request(value.as_object().unwrap()).expect("valid request environment")
}

#[test]
fn actual_semantics_are_target_specific_and_keep_absent_empty_distinct() {
    let base = BackendEnvironment::default();
    let native = BackendCodegenTarget::Native;
    let wasm = BackendCodegenTarget::Wasm;
    for value in ["", "0", "1"] {
        let changed = env(serde_json::json!({"MOLT_DISABLE_RC_COALESCE":value}));
        assert_ne!(
            base.semantic_identity(native),
            changed.semantic_identity(native)
        );
        assert_ne!(
            base.semantic_identity(wasm),
            changed.semantic_identity(wasm)
        );
    }
    let layout = env(serde_json::json!({"MOLT_WASM_MIN_PAGES":"256"}));
    assert_eq!(
        base.semantic_identity(native),
        layout.semantic_identity(native)
    );
    assert_ne!(base.semantic_identity(wasm), layout.semantic_identity(wasm));
    let entry = env(serde_json::json!({"MOLT_ENTRY_MODULE":"app"}));
    assert_ne!(
        base.semantic_identity(native),
        entry.semantic_identity(native)
    );
    assert_eq!(base.semantic_identity(wasm), entry.semantic_identity(wasm));
}

#[test]
fn request_reuse_values_and_partition_presence_are_not_output_path_identity() {
    let native = BackendCodegenTarget::Native;
    let base = BackendEnvironment::default().semantic_identity(native);
    let first = env(serde_json::json!({"MOLT_STDLIB_OBJ":"/first/stdlib.a"}));
    let relocated = env(serde_json::json!({"MOLT_STDLIB_OBJ":"/other/stdlib.a"}));
    let empty = env(serde_json::json!({"MOLT_STDLIB_OBJ":""}));
    assert_ne!(base, first.semantic_identity(native));
    assert_eq!(
        first.semantic_identity(native),
        relocated.semantic_identity(native)
    );
    assert_eq!(
        first.semantic_identity(native),
        empty.semantic_identity(native)
    );
    for value in [
        serde_json::json!({"MOLT_STDLIB_CACHE_KEY":"selected"}),
        serde_json::json!({"MOLT_STDLIB_CACHE_MANIFEST":"{}"}),
        serde_json::json!({"MOLT_STDLIB_MODULE_SYMBOLS":"[]"}),
        serde_json::json!({"MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256":"selected"}),
    ] {
        assert_ne!(base, env(value).semantic_identity(native));
    }
    let transport = env(serde_json::json!({
        "MOLT_STDLIB_CACHE_ENTRY":"/destination", "MOLT_RUNTIME_CALLABLE_SYMBOLS":"/input",
        "MOLT_DEBUG_ARTIFACT_DIR":"/scratch", "MOLT_BACKEND_TIMING":"1",
        "RAYON_NUM_THREADS":"7", "MOLT_BACKEND_MEMORY_AVAILABLE_GB":"16", "MOLT_DUMP_IR":""
    }));
    assert_eq!(base, transport.semantic_identity(native));
    let _scope = enter(Arc::new(transport));
    assert!(
        compilation_diagnostics_requested(),
        "excluded diagnostic still requires execution"
    );
}

#[test]
fn semantic_projection_is_lazy_shared_and_does_not_change_environment_equality() {
    let admitted = Arc::new(env(serde_json::json!({"MOLT_ENTRY_MODULE":"app"})));
    let same = (*admitted).clone();
    assert!(admitted.native_semantics.get().is_none());
    assert!(admitted.wasm_semantics.get().is_none());
    let _scope = enter(Arc::clone(&admitted));
    let first = current_semantic_environment(BackendCodegenTarget::Native).unwrap();
    let second = current_semantic_environment(BackendCodegenTarget::Native).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(*admitted, same);
    assert!(admitted.wasm_semantics.get().is_none());
    let carried = carry();
    let thread_identity = std::thread::spawn(move || {
        carried.enter(|| current_semantic_environment(BackendCodegenTarget::Native).unwrap())
    })
    .join()
    .unwrap();
    assert!(Arc::ptr_eq(&first, &thread_identity));
}

#[test]
fn worker_identity_keeps_its_closed_stage_and_rejects_old_serialization() {
    let baseline = NativeCodegenEnvironment::from_lookup(|_| Ok(None)).unwrap();
    let ignored_request_context = NativeCodegenEnvironment::from_lookup(|key| {
        Ok(matches!(
            key,
            BackendEnv::MOLT_STDLIB_CACHE_MANIFEST
                | BackendEnv::MOLT_ENTRY_MODULE
                | BackendEnv::MOLT_STDLIB_OBJ
                | BackendEnv::MOLT_BACKEND_TIMING
        )
        .then(|| "request-only".to_owned()))
    })
    .unwrap();
    assert_eq!(baseline, ignored_request_context);
    let empty = NativeCodegenEnvironment::from_lookup(|key| {
        Ok((key == BackendEnv::MOLT_DISABLE_RC_COALESCE).then(String::new))
    })
    .unwrap();
    assert_ne!(baseline, empty);
    let bytes = serde_json::to_vec(&empty).unwrap();
    assert_eq!(
        empty,
        serde_json::from_slice::<NativeCodegenEnvironment>(&bytes).unwrap()
    );
    let serialized = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    assert_eq!(serialized["values"]["MOLT_DISABLE_RC_COALESCE"], "");
    assert!(serialized["values"]["MOLT_BACKEND_INLINE_EXC_DISABLED"].is_null());
    let mut unknown = serialized.clone();
    unknown["values"]["MOLT_UNKNOWN_WORKER_INPUT"] = "unrecognized".into();
    assert!(serde_json::from_value::<NativeCodegenEnvironment>(unknown).is_err());
    let mut unknown_field = serialized;
    unknown_field["unrecognized"] = true.into();
    assert!(serde_json::from_value::<NativeCodegenEnvironment>(unknown_field).is_err());
    let mut old = serde_json::to_value(&baseline).unwrap();
    old["schema"] = 1.into();
    let old: NativeCodegenEnvironment = serde_json::from_value(old).unwrap();
    assert_ne!(
        baseline, old,
        "old worker identity cannot match current capture"
    );
}

#[test]
fn worker_replay_rejects_schema_and_membership_drift_against_actual_scope() {
    let _scope = enter(Arc::new(BackendEnvironment::default()));
    let current = NativeCodegenEnvironment::capture().expect("actual worker environment");
    current.validate_current().expect("unchanged replay");
    let value = serde_json::to_value(&current).unwrap();
    let mut old = value.clone();
    old["schema"] = 1.into();
    let old: NativeCodegenEnvironment = serde_json::from_value(old).unwrap();
    assert_eq!(
        old.validate_current().unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    let mut missing = value;
    missing["values"]
        .as_object_mut()
        .unwrap()
        .remove("MOLT_DISABLE_RC_COALESCE");
    let missing: NativeCodegenEnvironment = serde_json::from_value(missing).unwrap();
    assert!(missing.validate_current().is_err());
}
