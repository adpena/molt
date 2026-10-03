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
}

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let authority_path = manifest_dir.join("protocol.json");
    println!("cargo:rerun-if-changed={}", authority_path.display());
    let bytes = fs::read(&authority_path).expect("read proof supervisor protocol authority");
    let schemas: ProtocolSchemas =
        serde_json::from_slice(&bytes).expect("parse proof supervisor protocol authority");
    let rows = [
        ("POLICY_SCHEMA", schemas.policy_schema),
        ("CAPABILITY_SCHEMA", schemas.capability_schema),
        ("RECEIPT_SCHEMA", schemas.receipt_schema),
        ("EVENT_LOG_SCHEMA", schemas.event_log_schema),
    ];
    let mut generated = String::from("// Generated from protocol.json by build.rs.\n");
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
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"))
        .join("protocol.rs");
    fs::write(output, generated).expect("write generated proof supervisor schemas");
}
