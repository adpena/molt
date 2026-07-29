use molt_proof_supervisor::evidence::{durable_atomic_write, event_artifact_path};
use molt_proof_supervisor::{
    ClosureMode, DerivedRoot, FixedImage, POLICY_SCHEMA, Policy, Receipt, RootExitDisposition,
    sha256_bytes, sha256_file,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestRun {
    directory: PathBuf,
    binary: PathBuf,
    policy_path: PathBuf,
    receipt_path: PathBuf,
    policy: Policy,
}

impl Drop for TestRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn verify_binds_every_canonical_policy_dimension() {
    let run = run_fixture(ClosureMode::DeclaredTree);
    assert!(
        verify(&run.binary, &run.policy_path, &run.receipt_path)
            .status
            .success()
    );

    let alternate_cwd = run.directory.join("alternate-cwd");
    let derived_root = run.directory.join("derived");
    fs::create_dir_all(&alternate_cwd).unwrap();
    fs::create_dir_all(&derived_root).unwrap();
    let mut variants = Vec::new();

    let mut argv = run.policy.clone();
    argv.command.push("different-argument".to_owned());
    variants.push(argv);

    let mut cwd = run.policy.clone();
    cwd.cwd = alternate_cwd;
    variants.push(cwd);

    let mut environment = run.policy.clone();
    environment
        .environment
        .insert("MOLT_POLICY_BINDING".to_owned(), "different".to_owned());
    variants.push(environment);

    let mut role = run.policy.clone();
    role.root_role = "different-root-role".to_owned();
    role.fixed_images[0].role = role.root_role.clone();
    variants.push(role);

    let mut derived = run.policy.clone();
    derived.derived_roots.push(DerivedRoot {
        role: "generated-tool".to_owned(),
        path: derived_root,
    });
    variants.push(derived);

    for (index, policy) in variants.into_iter().enumerate() {
        let path = run.directory.join(format!("substitute-{index}.json"));
        fs::write(&path, serde_json::to_vec(&policy).unwrap()).unwrap();
        let output = verify(&run.binary, &path, &run.receipt_path);
        assert_eq!(output.status.code(), Some(79), "{}", text(&output));
        assert!(text(&output).contains("\"policy_digest_valid\":false"));
    }
}

#[test]
fn verify_rejects_unknown_policy_and_receipt_fields() {
    let run = run_fixture(ClosureMode::Leaf);
    let mut policy: Value = serde_json::from_slice(&fs::read(&run.policy_path).unwrap()).unwrap();
    policy["unknown_policy_authority"] = Value::Bool(true);
    let unknown_policy = run.directory.join("unknown-policy.json");
    fs::write(&unknown_policy, serde_json::to_vec(&policy).unwrap()).unwrap();
    let output = verify(&run.binary, &unknown_policy, &run.receipt_path);
    assert!(!output.status.success());
    assert!(text(&output).contains("unknown field"));

    let mut receipt: Value = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    receipt["unknown_receipt_authority"] = Value::Bool(true);
    durable_atomic_write(
        &run.receipt_path,
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
    assert!(!output.status.success());
    assert!(text(&output).contains("unknown field"));
}

#[test]
fn verify_replays_every_lifecycle_transition_even_after_reseal() {
    let run = run_fixture(ClosureMode::Leaf);
    let original: Receipt = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    let illegal = [
        vec![
            molt_proof_supervisor::SupervisorState::Created,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Complete,
        ],
        vec![
            molt_proof_supervisor::SupervisorState::Created,
            molt_proof_supervisor::SupervisorState::PolicySealed,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Draining,
            molt_proof_supervisor::SupervisorState::Complete,
        ],
        vec![
            molt_proof_supervisor::SupervisorState::Created,
            molt_proof_supervisor::SupervisorState::PolicySealed,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Complete,
            molt_proof_supervisor::SupervisorState::Draining,
        ],
    ];
    for lifecycle in illegal {
        let mut receipt = original.clone();
        receipt.lifecycle = lifecycle;
        receipt.seal();
        durable_atomic_write(
            &run.receipt_path,
            &serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
        assert_eq!(output.status.code(), Some(79), "{}", text(&output));
        assert!(text(&output).contains("\"lifecycle_valid\":false"));
    }
}

#[test]
fn verify_rejects_unknown_event_field_even_with_recomputed_artifact_and_receipt_digests() {
    let run = run_fixture(ClosureMode::Leaf);
    let mut receipt: Receipt =
        serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    let old_log = receipt.event_log.as_ref().unwrap();
    let old_path = run.directory.join(&old_log.file);
    let old_bytes = fs::read(&old_path).unwrap();
    let newline = old_bytes.iter().position(|byte| *byte == b'\n').unwrap();
    let mut first: Value = serde_json::from_slice(&old_bytes[..newline]).unwrap();
    first["unknown_event_authority"] = Value::Bool(true);
    let mut changed = serde_json::to_vec(&first).unwrap();
    changed.push(b'\n');
    changed.extend_from_slice(&old_bytes[newline + 1..]);
    let digest = sha256_bytes(&changed);
    let changed_path = event_artifact_path(&run.receipt_path, &digest).unwrap();
    durable_atomic_write(&changed_path, &changed).unwrap();

    let event_log = receipt.event_log.as_mut().unwrap();
    event_log.file = changed_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    event_log.bytes = changed.len() as u64;
    event_log.sha256 = digest;
    receipt.seal();
    durable_atomic_write(
        &run.receipt_path,
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();

    let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
    assert_eq!(output.status.code(), Some(79), "{}", text(&output));
    assert!(text(&output).contains("unknown field"));
}

fn run_fixture(mode: ClosureMode) -> TestRun {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let policy_path = directory.join("policy.json");
    let receipt_path = directory.join("receipt.json");
    let policy = Policy {
        schema: POLICY_SCHEMA.to_owned(),
        nonce: format!(
            "{:032x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
        mode,
        cwd: std::env::current_dir().unwrap(),
        command: vec![
            binary.display().to_string(),
            "fixture-child".to_owned(),
            "exit".to_owned(),
            "0".to_owned(),
        ],
        environment: BTreeMap::new(),
        root_role: "fixture".to_owned(),
        fixed_images: vec![FixedImage {
            role: "fixture".to_owned(),
            path: binary.clone(),
            sha256: sha256_file(&binary).unwrap(),
            root_exit_disposition: RootExitDisposition::RequireExit,
        }],
        derived_roots: vec![],
    };
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let output = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    TestRun {
        directory,
        binary,
        policy_path,
        receipt_path,
        policy,
    }
}

fn verify(binary: &Path, policy: &Path, receipt: &Path) -> Output {
    Command::new(binary)
        .args(["verify", "--policy"])
        .arg(policy)
        .arg("--receipt")
        .arg(receipt)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn unique_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "molt-proof-supervisor-verification-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
