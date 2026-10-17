use super::model::DaemonJobRequest;
use std::sync::Arc;

fn job(module: &str, function: Option<&str>) -> DaemonJobRequest {
    DaemonJobRequest::from_json_value(
        &serde_json::json!({"id":"job", "is_wasm":cfg!(feature="wasm-backend"),
            "output":"unused", "cache_key":module, "function_cache_key":function}),
        "job",
    )
    .expect("valid job")
}

#[test]
fn key_take_moves_buffers_and_shares_context_without_opening_ir() {
    let mut job = job("module", Some("function"));
    job.config_namespace = Some("configuration".to_owned());
    job.ir_path = Some("absent-input-must-not-be-opened".to_owned());
    let module_buffer = job.cache_key.as_ptr();
    let function_buffer = job.function_cache_key.as_ref().unwrap().as_ptr();
    let namespace_buffer = job.config_namespace.as_ref().unwrap().as_ptr();
    let (module, function) = job.take_artifact_cache_keys().expect("cache identity");
    let module = module.unwrap();
    let function = function.unwrap();
    assert_eq!(module.source.as_ptr(), module_buffer);
    assert_eq!(function.source.as_ptr(), function_buffer);
    assert_eq!(
        module.context.config_namespace.as_ref().unwrap().as_ptr(),
        namespace_buffer
    );
    assert!(Arc::ptr_eq(&module.context, &function.context));
    assert!(!Arc::ptr_eq(&module, &function));
    assert!(job.cache_key.is_empty());
    assert!(job.function_cache_key.is_none());
    assert!(job.config_namespace.is_none());
    assert_eq!(
        job.ir_path.as_deref(),
        Some("absent-input-must-not-be-opened")
    );
    assert_eq!(
        job.take_artifact_cache_keys().expect("cache identity"),
        (None, None)
    );
}

#[test]
fn blank_tiers_disable_caching_and_equal_normalized_tiers_share_one_key() {
    for function in [None, Some("\t\u{2003}")] {
        assert_eq!(
            job(" \u{2003}\n", function)
                .take_artifact_cache_keys()
                .expect("cache identity"),
            (None, None)
        );
    }
    let (module, function) = job("\u{2003}same \n", Some(" same\t"))
        .take_artifact_cache_keys()
        .expect("cache identity");
    let (module, function) = (module.unwrap(), function.unwrap());
    assert_eq!(module.source, "same");
    assert!(Arc::ptr_eq(&module, &function));
    let (module, function) = job("", Some("function"))
        .take_artifact_cache_keys()
        .expect("cache identity");
    assert!(module.is_none());
    assert_eq!(function.unwrap().source, "function");
}

#[test]
fn namespace_is_structural_and_transport_fields_do_not_change_identity() {
    let mut first = job("b:c", None);
    first.config_namespace = Some("a".to_owned());
    let mut second = job("c", None);
    second.config_namespace = Some("a:b".to_owned());
    assert_ne!(
        first.take_artifact_cache_keys().expect("cache identity"),
        second.take_artifact_cache_keys().expect("cache identity")
    );
    let plain = job("same", None)
        .take_artifact_cache_keys()
        .expect("cache identity");
    let mut empty_namespace = job("same", None);
    empty_namespace.config_namespace = Some(String::new());
    assert_ne!(
        plain,
        empty_namespace
            .take_artifact_cache_keys()
            .expect("cache identity")
    );
    let mut transport = job("same", None);
    transport.id = "other-id".to_owned();
    transport.output = "other-output".to_owned();
    transport.probe_cache_only = true;
    transport.skip_module_output_if_synced = true;
    transport.skip_function_output_if_synced = true;
    assert_eq!(
        plain,
        transport
            .take_artifact_cache_keys()
            .expect("cache identity")
    );
}

#[cfg(feature = "native-backend")]
#[test]
fn native_target_and_output_kind_both_scope_identity() {
    let base =
        serde_json::json!({"id":"job", "is_wasm":false, "output":"unused", "cache_key":"same"});
    let expected = DaemonJobRequest::from_json_value(&base, "job")
        .unwrap()
        .take_artifact_cache_keys()
        .expect("cache identity");
    for (field, value) in [
        ("target_triple", "aarch64-unknown-linux-gnu"),
        ("native_output_kind", "archive"),
    ] {
        let mut changed = base.clone();
        changed[field] = value.into();
        assert_ne!(
            expected,
            DaemonJobRequest::from_json_value(&changed, "job")
                .unwrap()
                .take_artifact_cache_keys()
                .expect("cache identity"),
            "{field}"
        );
    }
}

#[cfg(feature = "wasm-backend")]
#[test]
fn every_wasm_layout_option_scopes_identity() {
    let base =
        serde_json::json!({"id":"job", "is_wasm":true, "output":"unused", "cache_key":"same"});
    let expected = DaemonJobRequest::from_json_value(&base, "job")
        .unwrap()
        .take_artifact_cache_keys()
        .expect("cache identity");
    for (field, value) in [
        ("wasm_link", serde_json::json!(true)),
        ("wasm_data_base", serde_json::json!(0)),
        ("wasm_table_base", serde_json::json!(0)),
        ("wasm_split_runtime_app_table_base", serde_json::json!(0)),
    ] {
        let mut changed = base.clone();
        changed[field] = value;
        assert_ne!(
            expected,
            DaemonJobRequest::from_json_value(&changed, "job")
                .unwrap()
                .take_artifact_cache_keys()
                .expect("cache identity"),
            "{field}"
        );
    }
}

#[test]
fn cache_context_binds_the_active_environment_not_the_asserted_namespace() {
    use molt_backend::backend_environment::{BackendEnvironment, enter};
    let request = |entry: &str| {
        Arc::new(
            BackendEnvironment::from_request(
                serde_json::json!({"MOLT_DISABLE_RC_COALESCE":entry})
                    .as_object()
                    .unwrap(),
            )
            .unwrap(),
        )
    };
    let one = request("");
    let first = {
        let _scope = enter(Arc::clone(&one));
        let mut job = job("same", None);
        job.config_namespace = Some("asserted-unchanged".to_owned());
        job.take_artifact_cache_keys().unwrap().0.unwrap()
    };
    let second = {
        let _scope = enter(request("1"));
        let mut job = job("same", None);
        job.config_namespace = Some("asserted-unchanged".to_owned());
        job.take_artifact_cache_keys().unwrap().0.unwrap()
    };
    assert_ne!(first, second);
    let _scope = enter(one);
    let mut job = job("another", None);
    let third = job.take_artifact_cache_keys().unwrap().0.unwrap();
    assert!(Arc::ptr_eq(
        &first.context.environment,
        &third.context.environment
    ));
}
