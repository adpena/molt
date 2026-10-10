#![cfg(target_os = "macos")]

use molt_proof_supervisor::{
    Admission, CAPABILITY_SCHEMA, Capability, ClosureMode, FixedImage, POLICY_SCHEMA, Policy,
    Receipt, RootExitDisposition, SupervisorState, platform, sha256_file,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_unqualified_macos_mode_refuses_before_root_entry() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    for (mode, argument) in [
        (ClosureMode::Leaf, "leaf"),
        (ClosureMode::DeclaredTree, "declared-tree"),
        (ClosureMode::InventoryTree, "inventory-tree"),
    ] {
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "molt-macos-refusal-{}-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        )));
        fs::create_dir_all(&fixture.0).unwrap();
        let marker = fixture.0.join("must-not-enter");
        let output = Command::new(&binary)
            .args(["capability", argument])
            .output()
            .unwrap();
        assert!(output.status.success());
        let capability: Capability = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(capability.schema, CAPABILITY_SCHEMA);
        assert_eq!(capability.mode, mode);
        let Admission::Ineligible { reason } = &capability.admission else {
            panic!("unqualified macOS authority must refuse: {capability:?}");
        };
        assert!(!reason.is_empty());
        assert!(!capability.pre_entry_exec_authority);
        assert!(!capability.pre_entry_process_create_authority);
        assert!(!capability.recursive_descendant_authority);
        if mode == ClosureMode::Leaf {
            assert!(reason.contains("SIGTRAP"), "{reason}");
        }
        let policy = Policy {
            schema: POLICY_SCHEMA.to_owned(),
            nonce: "a".repeat(32),
            mode,
            cwd: fixture.0.clone(),
            command: vec![
                binary.display().to_string(),
                "fixture-child".to_owned(),
                "write-marker".to_owned(),
                marker.display().to_string(),
            ],
            environment: platform::required_environment(),
            root_role: "fixture".to_owned(),
            fixed_images: vec![FixedImage {
                role: "fixture".to_owned(),
                path: binary.clone(),
                sha256: sha256_file(&binary).unwrap(),
                root_exit_disposition: RootExitDisposition::RequireExit,
            }],
            derived_roots: vec![],
        };
        let policy_path = fixture.0.join("policy.json");
        let receipt_path = fixture.0.join("receipt.json");
        fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
        let status = Command::new(&binary)
            .args([
                if mode == ClosureMode::InventoryTree {
                    "inventory"
                } else {
                    "run"
                },
                "--policy",
            ])
            .arg(&policy_path)
            .arg("--receipt")
            .arg(&receipt_path)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(78));
        assert!(!marker.exists());
        let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(receipt.capability, capability);
        assert_eq!(receipt.state, SupervisorState::Rejected);
        assert!(!receipt.complete);
        assert_eq!(receipt.accounting.process_creates, 0);
        assert_eq!(receipt.event_log.as_ref().unwrap().count, 0);
        let verified = Command::new(&binary)
            .args(["verify", "--policy"])
            .arg(&policy_path)
            .arg("--receipt")
            .arg(&receipt_path)
            .output()
            .unwrap();
        assert!(
            verified.status.success(),
            "{}",
            String::from_utf8_lossy(&verified.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
        assert_eq!(result["admission_replay_valid"], true);
        assert_eq!(result["complete"], false); // Integrity never means payload acceptance.
    }
}
