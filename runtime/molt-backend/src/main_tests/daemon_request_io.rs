use super::*;

#[test]
fn read_daemon_request_bytes_stops_at_protocol_newline() {
    let mut cursor = Cursor::new(b"{\"version\":1}\ntrailing".to_vec());
    let bytes = read_daemon_request_bytes(&mut cursor, 1024).expect("request bytes");
    assert_eq!(bytes, b"{\"version\":1}\n");
}

#[test]
fn read_daemon_request_bytes_rejects_oversized_request() {
    let mut cursor = Cursor::new(b"{\"version\":1}\n".to_vec());
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
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    let request = DaemonRequest::from_json_bytes(
        br#"{
            "version": 1,
            "jobs": [
                {
                    "id": "job0",
                    "is_wasm": false,
                    "output": "/tmp/out.o",
                    "cache_key": "module"
                }
            ]
        }"#,
    )
    .expect("request parse");

    let job = request.jobs.expect("job list").pop().expect("job");
    assert_eq!(
        job.target,
        DaemonJobTarget::Native(NativeTargetOptions::default())
    );
    assert!(!job.skip_module_output_if_synced);
    assert!(!job.skip_function_output_if_synced);
    assert!(!job.probe_cache_only);
    assert!(job.ir.is_none());
    assert!(job.ir_path.is_none());
}

#[cfg(feature = "native-backend")]
#[test]
fn daemon_request_parse_accepts_path_backed_ir_lease() {
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    let request = DaemonRequest::from_json_bytes(
        br#"{
            "version": 1,
            "jobs": [
                {
                    "id": "job0",
                    "is_wasm": false,
                    "output": "/tmp/out.o",
                    "cache_key": "module",
                    "ir_path": "/tmp/molt-ir.json"
                }
            ]
        }"#,
    )
    .expect("request parse");

    let job = request.jobs.expect("job list").pop().expect("job");
    assert!(job.ir.is_none());
    assert_eq!(job.ir_path.as_deref(), Some("/tmp/molt-ir.json"));
}

#[cfg(feature = "native-backend")]
#[test]
fn daemon_request_parse_rejects_duplicate_ir_authority() {
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    let err = DaemonRequest::from_json_bytes(
        br#"{
            "version": 1,
            "jobs": [
                {
                    "id": "job0",
                    "is_wasm": false,
                    "output": "/tmp/out.o",
                    "cache_key": "module",
                    "ir_path": "/tmp/molt-ir.json",
                    "ir": {"functions": []}
                }
            ]
        }"#,
    )
    .expect_err("duplicate IR sources");

    assert!(err.contains("request.jobs[0] must use exactly one IR custody field: ir or ir_path"));
}

#[cfg(feature = "wasm-backend")]
#[test]
fn daemon_request_parse_reads_split_runtime_app_table_base() {
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    let request = DaemonRequest::from_json_bytes(
        br#"{
            "version": 1,
            "jobs": [
                {
                    "id": "job0",
                    "is_wasm": true,
                    "wasm_link": true,
                    "wasm_data_base": 1048576,
                    "wasm_table_base": 4096,
                    "wasm_split_runtime_app_table_base": 8192,
                    "output": "/tmp/out.wasm",
                    "cache_key": "module"
                }
            ]
        }"#,
    )
    .expect("request parse");

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
