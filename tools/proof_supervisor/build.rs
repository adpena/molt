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
    policy_max_bytes: usize,
    export: ExportProtocol,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportProtocol {
    footer_magic: String,
    length_hex_digits: usize,
    receipt_max_bytes: usize,
    event_max_bytes: u64,
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
    assert!(schemas.policy_max_bytes > 0);
    let policy_max_bytes = schemas.policy_max_bytes;
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
    assert!(export.receipt_max_bytes > 0 && export.event_max_bytes > 0);
    generated.push_str(&format!(
        "pub const EXPORT_FOOTER_MAGIC: &str = {};\n",
        serde_json::to_string(&export.footer_magic).expect("serialize export footer magic")
    ));
    generated.push_str(&format!(
        "pub const EXPORT_LENGTH_HEX_DIGITS: usize = {};\n\
         pub const EXPORT_RECEIPT_MAX_BYTES: usize = {};\n\
         pub const EXPORT_EVENT_MAX_BYTES: u64 = {};\n",
        export.length_hex_digits, export.receipt_max_bytes, export.event_max_bytes,
    ));
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"))
        .join("protocol.rs");
    fs::write(output, generated).expect("write generated proof supervisor schemas");
}
