// rustc also tracks this on the build-script executable itself, so restored
// mtimes cannot retain old protocol generation logic.
const _: Option<&str> = option_env!("MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR");

use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolSchemas {
    policy_schema: String,
    capability_schema: String,
    receipt_schema: String,
    event_log_schema: String,
    budgets: Budgets,
    export: ExportProtocol,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportProtocol {
    footer_magic: String,
    length_hex_digits: usize,
    event_max_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budgets {
    policy_input_bytes: usize,
    canonical_policy_bytes: usize,
    receipt_bytes: usize,
    event_record_bytes: usize,
    event_log_bytes: usize,
    event_records: usize,
    inventory_unique_images: usize,
    distinct_fixed_paths: usize,
    fixed_image_rows: usize,
    command_elements: usize,
    environment_entries: usize,
    derived_roots: usize,
    nonce_utf8_bytes: usize,
    role_utf8_bytes: usize,
    path_utf8_bytes: usize,
    one_image_roles_json_bytes: usize,
    stable_process_id_utf8_bytes: usize,
    file_id_utf8_bytes: usize,
    image_cache_entries: usize,
    lifetime_processes: usize,
    live_processes: usize,
    live_trace_tasks: usize,
    retained_observation_payload_bytes: usize,
    retained_derived_identity_bytes: usize,
    diagnostics_per_class: usize,
    combined_diagnostics_json_bytes: usize,
}

fn main() {
    // Cargo tracks content identity as an environment dependency, even when a
    // source sync restores mtimes (including protocol.json/build.rs changes).
    println!("cargo:rerun-if-env-changed=MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR");
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let authority_path = manifest_dir.join("protocol.json");
    println!("cargo:rerun-if-changed={}", authority_path.display());
    let bytes = fs::read(&authority_path).expect("read proof supervisor protocol authority");
    let schemas: ProtocolSchemas =
        serde_json::from_slice(&bytes).expect("parse proof supervisor protocol authority");
    let policy_max_bytes = schemas.budgets.policy_input_bytes;
    let rows = [
        ("POLICY_SCHEMA", schemas.policy_schema),
        ("CAPABILITY_SCHEMA", schemas.capability_schema),
        ("RECEIPT_SCHEMA", schemas.receipt_schema),
        ("EVENT_LOG_SCHEMA", schemas.event_log_schema),
    ];
    let mut generated = String::from("// Generated from protocol.json by build.rs.\n");
    generated.push_str(&format!(
        "pub const MAX_POLICY_BYTES: usize = {policy_max_bytes};\n"
    ));
    for (name, value) in rows {
        assert!(
            value.starts_with("molt.") && value.is_ascii(),
            "{name} must be one ASCII Molt schema identifier"
        );
        generated.push_str(&format!(
            "pub const {name}: &str = {};\n",
            serde_json::to_string(&value).expect("serialize Rust string literal")
        ));
    }
    let export = schemas.export;
    assert!(export.footer_magic.is_ascii() && !export.footer_magic.is_empty());
    assert_eq!(
        export.length_hex_digits, 16,
        "export lengths encode bounded u64 values"
    );
    assert!(schemas.budgets.receipt_bytes > 0 && export.event_max_bytes > 0);
    generated.push_str(&format!(
        "pub const EXPORT_FOOTER_MAGIC: &str = {};\n",
        serde_json::to_string(&export.footer_magic).expect("serialize export footer magic")
    ));
    generated.push_str(&format!(
        "pub const EXPORT_LENGTH_HEX_DIGITS: usize = {};\n\
         pub const EXPORT_RECEIPT_MAX_BYTES: usize = {};\n\
         pub const EXPORT_EVENT_MAX_BYTES: u64 = {};\n",
        export.length_hex_digits, schemas.budgets.receipt_bytes, export.event_max_bytes,
    ));
    let b = schemas.budgets;
    assert!(
        b.path_utf8_bytes
            .checked_mul(6)
            .and_then(|v| v.checked_add(b.one_image_roles_json_bytes + 4096))
            .is_some_and(|v| v < b.event_record_bytes),
        "image framing must fit one event"
    );
    assert!(b.combined_diagnostics_json_bytes + 8192 < b.receipt_bytes);
    assert!(b.retained_derived_identity_bytes <= b.retained_observation_payload_bytes);
    assert!(b.live_processes <= b.lifetime_processes && b.live_processes <= b.live_trace_tasks);
    for (name, value) in [
        ("BUDGET_POLICY_INPUT_BYTES", b.policy_input_bytes),
        ("BUDGET_CANONICAL_POLICY_BYTES", b.canonical_policy_bytes),
        ("BUDGET_RECEIPT_BYTES", b.receipt_bytes),
        ("BUDGET_EVENT_RECORD_BYTES", b.event_record_bytes),
        ("BUDGET_EVENT_LOG_BYTES", b.event_log_bytes),
        ("BUDGET_EVENT_RECORDS", b.event_records),
        ("BUDGET_INVENTORY_UNIQUE_IMAGES", b.inventory_unique_images),
        ("BUDGET_DISTINCT_FIXED_PATHS", b.distinct_fixed_paths),
        ("BUDGET_FIXED_IMAGE_ROWS", b.fixed_image_rows),
        ("BUDGET_COMMAND_ELEMENTS", b.command_elements),
        ("BUDGET_ENVIRONMENT_ENTRIES", b.environment_entries),
        ("BUDGET_DERIVED_ROOTS", b.derived_roots),
        ("BUDGET_NONCE_UTF8_BYTES", b.nonce_utf8_bytes),
        ("BUDGET_ROLE_UTF8_BYTES", b.role_utf8_bytes),
        ("BUDGET_PATH_UTF8_BYTES", b.path_utf8_bytes),
        (
            "BUDGET_ONE_IMAGE_ROLES_JSON_BYTES",
            b.one_image_roles_json_bytes,
        ),
        (
            "BUDGET_STABLE_PROCESS_ID_UTF8_BYTES",
            b.stable_process_id_utf8_bytes,
        ),
        ("BUDGET_FILE_ID_UTF8_BYTES", b.file_id_utf8_bytes),
        ("BUDGET_IMAGE_CACHE_ENTRIES", b.image_cache_entries),
        ("BUDGET_LIFETIME_PROCESSES", b.lifetime_processes),
        ("BUDGET_LIVE_PROCESSES", b.live_processes),
        ("BUDGET_LIVE_TRACE_TASKS", b.live_trace_tasks),
        (
            "BUDGET_RETAINED_OBSERVATION_PAYLOAD_BYTES",
            b.retained_observation_payload_bytes,
        ),
        (
            "BUDGET_RETAINED_DERIVED_IDENTITY_BYTES",
            b.retained_derived_identity_bytes,
        ),
        ("BUDGET_DIAGNOSTICS_PER_CLASS", b.diagnostics_per_class),
        (
            "BUDGET_COMBINED_DIAGNOSTICS_JSON_BYTES",
            b.combined_diagnostics_json_bytes,
        ),
    ] {
        assert!(
            value > 0 && value <= isize::MAX as usize,
            "positive portable budget required"
        );
        generated.push_str(&format!("pub const {name}: usize = {value};\n"));
    }
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"))
        .join("protocol.rs");
    fs::write(output, generated).expect("write generated proof supervisor schemas");
}
