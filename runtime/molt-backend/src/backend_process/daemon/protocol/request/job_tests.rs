use super::*;

#[cfg(feature = "native-backend")]
#[test]
fn native_artifact_job_contract_defaults_and_rejects_unknown_kinds() {
    let _scope = molt_backend::backend_environment::enter(Arc::default());
    use crate::backend_process::{NativeArtifactKind, NativeTargetOptions};
    let mut value =
        serde_json::json!({"id":"job", "is_wasm":false, "output":"output", "cache_key":"same"});
    let mut job = DaemonJobRequest::from_json_value(&value, "job").expect("default object job");
    assert_eq!(
        job.target,
        DaemonJobTarget::Native(NativeTargetOptions {
            target_triple: None,
            output_kind: NativeArtifactKind::Object,
        })
    );
    let object_key = job.take_artifact_cache_keys().unwrap().module.unwrap();
    value["native_output_kind"] = "archive".into();
    let mut job = DaemonJobRequest::from_json_value(&value, "job").expect("archive job");
    assert_ne!(
        object_key,
        job.take_artifact_cache_keys().unwrap().module.unwrap()
    );
    value["is_wasm"] = true.into();
    assert!(DaemonJobRequest::from_json_value(&value, "job").is_err());
    value["is_wasm"] = false.into();
    for invalid in [
        serde_json::json!("unknown"),
        serde_json::json!(3),
        serde_json::json!(false),
    ] {
        value["native_output_kind"] = invalid;
        assert!(DaemonJobRequest::from_json_value(&value, "job").is_err());
    }
}

#[cfg(feature = "wasm-backend")]
#[test]
fn wasm_job_carries_its_complete_layout() {
    // src/molt/cli/backend_execution.py sends every layout field.
    let value = serde_json::json!({
        "id": "job", "is_wasm": true, "output": "app.wasm", "cache_key": "same",
        "target_triple": "wasm32-wasip1", "wasm_link": true, "wasm_data_base": 1048576,
        "wasm_table_base": 4096, "wasm_split_runtime_app_table_base": 8192,
    });
    let job = DaemonJobRequest::from_json_value(&value, "job").expect("wasm job");
    assert_eq!(
        job.target,
        DaemonJobTarget::Wasm(crate::backend_process::WasmTargetOptions {
            link_relocs: true,
            data_base: Some(1_048_576),
            table_base: Some(4096),
            split_runtime_app_table_base: Some(8192),
        })
    );
}

#[cfg(not(feature = "wasm-backend"))]
#[test]
fn daemon_without_wasm_backend_refuses_wasm_jobs_when_parsing() {
    let value = serde_json::json!({
        "id": "job", "is_wasm": true, "output": "app.wasm", "cache_key": "same",
    });
    let error = DaemonJobRequest::from_json_value(&value, "request.jobs[0]")
        .expect_err("a native-only daemon cannot build WASM");
    assert!(error.contains("built without wasm-backend"), "{error}");
}
