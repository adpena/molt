use super::*;

#[cfg(unix)]
#[test]
fn read_daemon_request_bytes_stops_at_protocol_newline() {
    let mut cursor = Cursor::new(b"{\"version\":1}\ntrailing".to_vec());
    let bytes = read_daemon_request_bytes(&mut cursor, 1024).expect("request bytes");
    assert_eq!(bytes, b"{\"version\":1}\n");
}

#[cfg(unix)]
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

#[test]
fn read_bounded_request_bytes_allows_exact_limit() {
    let cursor = Cursor::new(b"abcd".to_vec());
    let bytes =
        read_bounded_request_bytes(cursor, 4, "backend stdin request").expect("request bytes");
    assert_eq!(bytes, b"abcd");
}

#[test]
fn read_bounded_request_bytes_rejects_oversized_request() {
    let cursor = Cursor::new(b"abcde".to_vec());
    let err = read_bounded_request_bytes(cursor, 4, "backend stdin request")
        .expect_err("oversized request");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("backend stdin request exceeded 4 byte limit")
    );
}

#[test]
fn request_bounded_read_rejects_streaming_read_past_limit() {
    let cursor = Cursor::new(b"abcde".to_vec());
    let mut reader = RequestBoundedRead::new(cursor, 4, "streaming backend stdin request");
    let mut first_chunk = [0_u8; 4];
    assert_eq!(
        reader.read(&mut first_chunk).expect("first bounded read"),
        4
    );
    assert_eq!(&first_chunk, b"abcd");
    let mut probe = [0_u8; 1];
    let err = reader.read(&mut probe).expect_err("stream overflow");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("streaming backend stdin request exceeded 4 byte limit")
    );
}

#[test]
fn daemon_request_parse_applies_boolean_defaults() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    assert!(!job.wasm_link);
    assert_eq!(job.wasm_split_runtime_runtime_table_min, None);
    assert!(!job.skip_module_output_if_synced);
    assert!(!job.skip_function_output_if_synced);
    assert!(!job.probe_cache_only);
    assert!(job.ir.is_none());
    assert!(job.ir_path.is_none());
}

#[test]
fn daemon_request_parse_accepts_path_backed_ir_lease() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

#[test]
fn daemon_request_parse_rejects_duplicate_ir_authority() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

#[test]
fn daemon_request_parse_reads_split_runtime_table_min() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
                    "wasm_split_runtime_runtime_table_min": 8192,
                    "output": "/tmp/out.wasm",
                    "cache_key": "module"
                }
            ]
        }"#,
    )
    .expect("request parse");

    let job = request.jobs.expect("job list").pop().expect("job");
    assert!(job.wasm_link);
    assert_eq!(job.wasm_data_base, Some(1048576));
    assert_eq!(job.wasm_table_base, Some(4096));
    assert_eq!(job.wasm_split_runtime_runtime_table_min, Some(8192));
}

#[cfg(unix)]
#[test]
fn daemon_response_payload_omits_false_optional_fields() {
    let payload = daemon_response_payload(&DaemonResponse {
        ok: true,
        pong: false,
        jobs: vec![super::DaemonJobResponse {
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
