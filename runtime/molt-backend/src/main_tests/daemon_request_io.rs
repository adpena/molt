use super::*;

#[test]
fn read_daemon_request_bytes_stops_at_protocol_newline() {
    let frame = format!("{{\"version\":{BACKEND_DAEMON_PROTOCOL_VERSION}}}\n");
    let mut cursor = Cursor::new(format!("{frame}trailing").into_bytes());
    let bytes = read_daemon_request_bytes(&mut cursor, 1024).expect("request bytes");
    assert_eq!(bytes, frame.as_bytes());
}

#[test]
fn read_daemon_request_bytes_rejects_oversized_request() {
    let mut cursor =
        Cursor::new(format!("{{\"version\":{BACKEND_DAEMON_PROTOCOL_VERSION}}}\n").into_bytes());
    let err = read_daemon_request_bytes(&mut cursor, 4).expect_err("oversized request");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("daemon request exceeded 4 byte limit")
    );
}

#[cfg(feature = "native-backend")]
#[test]
fn daemon_request_parse_applies_boolean_defaults() {
    let frame = serde_json::to_vec(&serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "jobs": [{
            "id": "job0", "is_wasm": false, "output": "/tmp/out.o",
            "cache_key": "module", "ir_path": "/tmp/molt-ir.msgpack",
        }],
    }))
    .unwrap();
    let request = DaemonRequest::from_json_bytes(&frame).expect("request parse");
    let job = request.jobs.expect("job list").pop().expect("job");
    assert_eq!(
        job.target,
        DaemonJobTarget::Native(NativeTargetOptions::default())
    );
    assert!(!job.skip_module_output_if_synced);
    assert!(!job.skip_function_output_if_synced);
    assert_eq!(
        job.ir_source,
        DaemonIrSource::Lease("/tmp/molt-ir.msgpack".to_owned())
    );
}

#[test]
fn daemon_request_parse_accepts_a_probe_without_ir_custody() {
    let frame = serde_json::to_vec(&serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "jobs": [{
            "id": "probe", "is_wasm": cfg!(feature = "wasm-backend"),
            "output": "unused", "cache_key": "module", "probe_cache_only": true,
        }],
    }))
    .unwrap();
    let request = DaemonRequest::from_json_bytes(&frame).expect("probe request");
    assert_eq!(request.jobs.unwrap()[0].ir_source, DaemonIrSource::Probe);
}

#[cfg(feature = "wasm-backend")]
#[test]
fn daemon_request_parse_reads_split_runtime_app_table_base() {
    let frame = serde_json::to_vec(&serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "jobs": [{
            "id": "job0", "is_wasm": true, "wasm_link": true,
            "wasm_data_base": 1048576, "wasm_table_base": 4096,
            "wasm_split_runtime_app_table_base": 8192, "output": "/tmp/out.wasm",
            "cache_key": "module", "probe_cache_only": true,
        }],
    }))
    .unwrap();
    let request = DaemonRequest::from_json_bytes(&frame).expect("request parse");
    let job = request.jobs.expect("job list").pop().expect("job");
    assert_eq!(
        job.target,
        DaemonJobTarget::Wasm(WasmTargetOptions {
            link_relocs: true,
            data_base: Some(1048576),
            table_base: Some(4096),
            split_runtime_app_table_base: Some(8192),
        })
    );
}

#[test]
fn daemon_response_payload_omits_false_optional_fields() {
    let payload = daemon_response_payload(&DaemonResponse {
        ok: true,
        pong: false,
        jobs: vec![DaemonJobResponse {
            id: "job0".to_string(),
            ok: true,
            cached: false,
            cache_tier: None,
            output_written: true,
            needs_ir: false,
            message: None,
            warnings: Vec::new(),
        }],
        error: None,
        health: None,
    })
    .expect("response payload");

    let text = String::from_utf8(payload).expect("utf8 json");
    assert!(!text.contains("\"needs_ir\""));
    assert!(!text.contains("\"health\""));
    assert!(!text.contains("\"error\""));
}

#[test]
fn daemon_control_requires_a_complete_request_frame() {
    let frame = serde_json::to_vec(&serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "jobs": [{
            "id": "frame", "is_wasm": cfg!(feature = "wasm-backend"),
            "output": "unused", "cache_key": "frame", "ir_path": "unopened.msgpack",
        }],
    }))
    .unwrap();
    let mut whitespace = frame.clone();
    whitespace.extend_from_slice(b" \t\r\n");
    let request = DaemonRequest::from_json_bytes(&whitespace).expect("complete control request");
    assert_eq!(
        request.jobs.unwrap()[0].ir_source,
        DaemonIrSource::Lease("unopened.msgpack".to_owned())
    );
    for suffix in [b"{}".as_slice(), b"!".as_slice(), frame.as_slice()] {
        let mut bytes = frame.clone();
        bytes.extend_from_slice(suffix);
        let error = DaemonRequest::from_json_bytes(&bytes).expect_err("request suffix");
        assert!(error.contains("trailing"), "{error}");
    }
}
