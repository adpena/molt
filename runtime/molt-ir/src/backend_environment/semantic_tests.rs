use super::*;

fn env(value: serde_json::Value) -> BackendEnvironment {
    BackendEnvironment::from_request(value.as_object().unwrap()).expect("valid request environment")
}

#[test]
fn actual_semantics_keep_absent_empty_distinct() {
    let base = BackendEnvironment::default();
    for value in ["", "0", "1"] {
        let changed = env(serde_json::json!({"MOLT_DISABLE_SPLIT_FIELD_DEFOREST":value}));
        assert_ne!(base.semantic_identity(), changed.semantic_identity());
    }
    let entry = env(serde_json::json!({"MOLT_ENTRY_MODULE":"app"}));
    assert_ne!(base.semantic_identity(), entry.semantic_identity());
}

#[test]
fn request_reuse_values_and_partition_presence_are_not_output_path_identity() {
    let base = BackendEnvironment::default().semantic_identity();
    let first = env(serde_json::json!({"MOLT_STDLIB_OBJ":"/first/stdlib.a"}));
    let relocated = env(serde_json::json!({"MOLT_STDLIB_OBJ":"/other/stdlib.a"}));
    let empty = env(serde_json::json!({"MOLT_STDLIB_OBJ":""}));
    assert_ne!(base, first.semantic_identity());
    assert_eq!(first.semantic_identity(), relocated.semantic_identity());
    assert_eq!(first.semantic_identity(), empty.semantic_identity());
    for value in [
        serde_json::json!({"MOLT_STDLIB_CACHE_KEY":"selected"}),
        serde_json::json!({"MOLT_STDLIB_CACHE_MANIFEST":"{}"}),
        serde_json::json!({"MOLT_STDLIB_MODULES":"[]"}),
        serde_json::json!({"MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256":"selected"}),
    ] {
        assert_ne!(base, env(value).semantic_identity());
    }
    let transport = env(serde_json::json!({
        "MOLT_RUNTIME_CALLABLE_SYMBOLS":"/input",
        "MOLT_DEBUG_ARTIFACT_DIR":"/scratch", "MOLT_BACKEND_TIMING":"1",
        "RAYON_NUM_THREADS":"7", "MOLT_BACKEND_MEMORY_AVAILABLE_GB":"16", "MOLT_DUMP_IR":""
    }));
    assert_eq!(base, transport.semantic_identity());
    let _scope = enter(Arc::new(transport));
    assert!(
        compilation_diagnostics_requested(),
        "excluded diagnostic still requires execution"
    );
}

#[test]
fn semantic_identity_is_lazy_cached_and_does_not_change_environment_equality() {
    let admitted = Arc::new(env(serde_json::json!({"MOLT_ENTRY_MODULE":"app"})));
    let same = env(serde_json::json!({"MOLT_ENTRY_MODULE":"app"}));
    assert!(admitted.semantics.get().is_none());
    let _scope = enter(Arc::clone(&admitted));
    let first = current_semantic_environment().unwrap();
    let second = current_semantic_environment().unwrap();
    assert_eq!(first, second);
    assert_eq!(admitted.semantics.get(), Some(&first));
    assert_eq!(*admitted, same);
    let carried = carry();
    let thread_identity =
        std::thread::spawn(move || carried.enter(|| current_semantic_environment().unwrap()))
            .join()
            .unwrap();
    assert_eq!(first, thread_identity);
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
        Ok((key == BackendEnv::MOLT_DISABLE_SPLIT_FIELD_DEFOREST).then(String::new))
    })
    .unwrap();
    assert_ne!(baseline, empty);
    let bytes = serde_json::to_vec(&empty).unwrap();
    assert_eq!(
        empty,
        serde_json::from_slice::<NativeCodegenEnvironment>(&bytes).unwrap()
    );
    let serialized = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    assert_eq!(
        serialized["values"]["MOLT_DISABLE_SPLIT_FIELD_DEFOREST"],
        ""
    );
    assert!(serialized["values"]["MOLT_BACKEND_ENABLE_VERIFIER"].is_null());
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
        .remove("MOLT_DISABLE_SPLIT_FIELD_DEFOREST");
    let missing: NativeCodegenEnvironment = serde_json::from_value(missing).unwrap();
    assert!(missing.validate_current().is_err());
}

#[test]
fn semantic_identity_has_independent_domain_and_projection_vectors() {
    use crate::content_digest::bytes_to_lower_hex;
    let cases = [
        (
            serde_json::json!({}),
            "b3c40304f7c8e0ef91d46057f02c5ff81b514f7c15dfaa18680911dc117820a0",
        ),
        (
            serde_json::json!({"MOLT_STDLIB_OBJ":""}),
            "accdb136a2d54495b2431d23a1aa762d5a0da6dbd9efbe34d71e2e3bb7ced814",
        ),
        (
            serde_json::json!({"MOLT_MAX_FUNCTION_OPS":""}),
            "a4268b8ae159ad92c82c01bcbd502498643a1e922a2edf4a27ff5d6a089af5b8",
        ),
    ];
    for (values, expected) in cases {
        // Independent hashlib vectors pin domain, target, key and value tags.
        assert_eq!(
            bytes_to_lower_hex(&env(values).semantic_identity().0),
            expected
        );
    }
    let first = env(serde_json::from_str(
        r#"{"MOLT_ENTRY_MODULE":"app","MOLT_MAX_FUNCTION_OPS":"π"}"#,
    )
    .unwrap());
    let reordered = env(serde_json::from_str(
        r#"{"MOLT_MAX_FUNCTION_OPS":"π","MOLT_ENTRY_MODULE":"app"}"#,
    )
    .unwrap());
    assert_eq!(first.semantic_identity(), reordered.semantic_identity());
    assert_ne!(
        env(serde_json::json!({"MOLT_MAX_FUNCTION_OPS":"a\0bc"})).semantic_identity(),
        env(serde_json::json!({"MOLT_MAX_FUNCTION_OPS":"ab\0c"})).semantic_identity(),
    );
    assert_eq!(std::mem::size_of::<SemanticBackendIdentity>(), 32);
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "private self-executing test probe marker, not a backend setting"
)]
fn semantic_identity_process_probe() {
    const MARKER: &str = "MOLT_TEST_SEMANTIC_IDENTITY_PROCESS";
    const SUCCESS: &str = "molt-semantic-process-and-scope-identity-verified";
    if std::env::var_os(MARKER).is_none() {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args([
            "--exact",
            "backend_environment::semantic_tests::semantic_identity_process_probe",
            "--nocapture",
        ]);
        for key in daemon_request_env_keys() {
            child.env_remove(key.name());
        }
        let result = child
            .env(MARKER, "1")
            .env("MOLT_MAX_FUNCTION_OPS", "π")
            .env("MOLT_ENTRY_MODULE", "app")
            .env("MOLT_STDLIB_OBJ", "")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            result.status.success() && stdout.contains(SUCCESS),
            "stdout: {stdout}\nstderr: {stderr}"
        );
        return;
    }
    let process = current_semantic_environment().unwrap();
    assert_eq!(
        crate::content_digest::bytes_to_lower_hex(&process.0),
        "5e846a7edf5f43542310ee005189c602882785566af35c82f1cbcc96ab6d6c6e"
    );
    let request = env(
        serde_json::json!({"MOLT_MAX_FUNCTION_OPS":"π", "MOLT_ENTRY_MODULE":"app", "MOLT_STDLIB_OBJ":""}),
    );
    let _scope = enter(Arc::new(request));
    assert_eq!(process, current_semantic_environment().unwrap());
    println!("{SUCCESS}");
}
