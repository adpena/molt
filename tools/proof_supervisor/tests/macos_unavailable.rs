#![cfg(target_os = "macos")]

use molt_proof_supervisor::{
    CAPABILITY_SCHEMA, Capability, ClosureMode, FixedImage, POLICY_SCHEMA, Policy, RECEIPT_SCHEMA,
    Receipt, RootExitDisposition, sha256_file,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn missing_endpoint_security_authority_rejects_before_root_entry() {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = std::env::temp_dir().join(format!(
        "molt-proof-supervisor-macos-unavailable-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let marker = directory.join("root-entered");
    let policy_path = directory.join("policy.json");
    let receipt_path = directory.join("receipt.json");
    let policy = Policy {
        schema: POLICY_SCHEMA.to_owned(),
        nonce: "a".repeat(32),
        mode: ClosureMode::Leaf,
        cwd: directory.clone(),
        command: vec![
            binary.display().to_string(),
            "fixture-child".to_owned(),
            "write-pid-and-sleep-leaf".to_owned(),
            marker.display().to_string(),
        ],
        environment: BTreeMap::new(),
        root_role: "fixture".to_owned(),
        fixed_images: vec![FixedImage {
            role: "fixture".to_owned(),
            path: binary.clone(),
            sha256: sha256_file(&binary).unwrap(),
            root_exit_disposition: RootExitDisposition::RequireExit,
        }],
        derived_roots: Vec::new(),
    };
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();

    let capability_output = Command::new(&binary)
        .args(["capability", "leaf"])
        .output()
        .unwrap();
    assert!(capability_output.status.success());
    let capability: Capability = serde_json::from_slice(&capability_output.stdout).unwrap();
    assert_eq!(capability.schema, CAPABILITY_SCHEMA);
    assert_eq!(capability.backend, "macos-endpoint-security");
    assert!(!capability.available);
    assert!(!capability.pre_entry_exec_authority);
    assert!(!capability.pre_entry_process_create_authority);
    assert!(!capability.recursive_descendant_authority);

    let run = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    assert_eq!(run.code(), Some(78));
    assert!(
        !marker.exists(),
        "unavailable backend launched the root process"
    );
    let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(receipt.schema, RECEIPT_SCHEMA);
    assert_eq!(receipt.capability, capability);
    assert!(!receipt.complete);
    assert_eq!(receipt.accounting.process_creates, 0);

    let verify = Command::new(&binary)
        .args(["verify", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    assert!(verify.success());
    let _ = fs::remove_dir_all(directory);
}
