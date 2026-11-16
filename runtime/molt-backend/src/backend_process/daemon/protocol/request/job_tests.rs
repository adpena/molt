use super::*;

#[cfg(feature = "native-backend")]
#[test]
fn native_artifact_job_contract_defaults_and_rejects_unknown_kinds() {
    let _scope = molt_backend::backend_environment::enter(Arc::default());
    use crate::backend_process::{NativeArtifactKind, NativeTargetOptions};
    let mut value = serde_json::json!({"id":"job", "output":"output", "cache_key":"same", "probe_cache_only":true, "function_digests": ""});
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
    for invalid in [
        serde_json::json!("unknown"),
        serde_json::json!(3),
        serde_json::json!(false),
    ] {
        value["native_output_kind"] = invalid;
        assert!(DaemonJobRequest::from_json_value(&value, "job").is_err());
    }
}

#[test]
fn daemon_refuses_the_retired_wasm_job_selector() {
    for selector in [true, false] {
        let value = serde_json::json!({
            "id": "job", "is_wasm": selector, "output": "app.wasm", "cache_key": "same",
            "probe_cache_only": true, "function_digests": "",
        });
        let error = DaemonJobRequest::from_json_value(&value, "request.jobs[0]")
            .expect_err("every job is native");
        assert!(error.contains("is_wasm is retired"), "{error}");
    }
}

#[test]
fn daemon_ir_source_rejects_inline_documents_and_ambiguous_probe_custody() {
    let base = serde_json::json!({
        "id": "job", "output": "unused", "cache_key": "same", "ir_path": "fixture.msgpack",
    });
    for inline in [
        serde_json::Value::Null,
        serde_json::json!({"functions": []}),
    ] {
        for probe in [false, true] {
            let mut value = base.clone();
            if probe {
                value.as_object_mut().unwrap().remove("ir_path");
            }
            value["ir"] = inline.clone();
            value["probe_cache_only"] = probe.into();
            let error = DaemonJobRequest::from_json_value(&value, "job").unwrap_err();
            assert!(error.contains(".ir is retired"), "{error}");
        }
    }
    for path in [
        serde_json::Value::Null,
        serde_json::json!("fixture.msgpack"),
    ] {
        let mut value = base.clone();
        value["ir_path"] = path;
        value["probe_cache_only"] = true.into();
        value["function_digests"] = "".into();
        let error = DaemonJobRequest::from_json_value(&value, "job").unwrap_err();
        assert!(error.contains("ir_path"), "{error}");
    }
}

#[test]
fn full_daemon_job_requires_a_nonempty_messagepack_lease_path() {
    let base = serde_json::json!({
        "id": "job", "output": "unused", "cache_key": "same",
    });
    for path in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!("")),
        Some(serde_json::json!(3)),
    ] {
        for probe in [None, Some(false)] {
            let mut value = base.clone();
            if let Some(path) = path.clone() {
                value["ir_path"] = path;
            }
            if let Some(probe) = probe {
                value["probe_cache_only"] = probe.into();
            }
            let error = DaemonJobRequest::from_json_value(&value, "job").unwrap_err();
            assert!(error.contains("ir_path"), "{error}");
        }
    }
    let mut valid = base;
    valid["ir_path"] = "fixture.msgpack".into();
    let job = DaemonJobRequest::from_json_value(&valid, "job").expect("lease source");
    assert_eq!(
        job.ir_source,
        DaemonIrSource::Lease("fixture.msgpack".to_owned())
    );
}

#[test]
fn a_probe_names_its_program_and_a_lease_names_none() {
    use molt_ir::addressed_program::FunctionDigest;
    let probe = serde_json::json!({
        "id": "job", "output": "unused", "cache_key": "same", "probe_cache_only": true,
    });
    let error = DaemonJobRequest::from_json_value(&probe, "job").unwrap_err();
    assert!(error.contains("requires function_digests"), "{error}");
    let digests = [FunctionDigest::of_body(b"a"), FunctionDigest::of_body(b"b")];
    let mut named = probe.clone();
    named["function_digests"] = digests
        .iter()
        .map(ToString::to_string)
        .collect::<String>()
        .into();
    assert_eq!(
        DaemonJobRequest::from_json_value(&named, "job")
            .unwrap()
            .ir_source,
        DaemonIrSource::Probe(digests.to_vec())
    );
    for malformed in ["0", "zz", &"A".repeat(64)] {
        named["function_digests"] = malformed.into();
        let error = DaemonJobRequest::from_json_value(&named, "job").unwrap_err();
        assert!(error.contains("function_digests"), "{error}");
    }
    let mut lease = probe;
    lease["probe_cache_only"] = false.into();
    lease["ir_path"] = "fixture.msgpack".into();
    lease["function_digests"] = "".into();
    let error = DaemonJobRequest::from_json_value(&lease, "job").unwrap_err();
    assert!(
        error.contains("names its functions in the ir_path lease"),
        "{error}"
    );
}
