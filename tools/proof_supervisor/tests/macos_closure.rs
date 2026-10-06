#![cfg(target_os = "macos")]

use molt_proof_supervisor::{
    CAPABILITY_SCHEMA, Capability, ClosureMode, FixedImage, ImageClass, POLICY_SCHEMA, Policy,
    ProcessEvent, ProcessEventKind, RECEIPT_SCHEMA, Receipt, RootExitDisposition, sha256_file,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture {
    binary: PathBuf,
    directory: PathBuf,
    policy_path: PathBuf,
    receipt_path: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn tree_modes_reject_before_root_entry_without_endpoint_security() {
    for mode in [ClosureMode::DeclaredTree, ClosureMode::InventoryTree] {
        let fixture = fixture("tree-refusal");
        let marker = fixture.directory.join("root-entered");
        let policy = policy(
            &fixture,
            mode,
            vec![
                "write-pid-and-sleep-leaf".to_owned(),
                marker.display().to_string(),
            ],
            vec![],
        );
        fs::write(&fixture.policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();

        let mode_name = match mode {
            ClosureMode::DeclaredTree => "declared-tree",
            ClosureMode::InventoryTree => "inventory-tree",
            ClosureMode::Leaf => unreachable!(),
        };
        let capability_output = Command::new(&fixture.binary)
            .args(["capability", mode_name])
            .output()
            .unwrap();
        assert!(capability_output.status.success());
        let capability: Capability = serde_json::from_slice(&capability_output.stdout).unwrap();
        assert_eq!(capability.schema, CAPABILITY_SCHEMA);
        assert_eq!(capability.platform, "macos");
        assert_eq!(capability.backend, "seatbelt+ptrace");
        assert!(!capability.available);
        assert!(!capability.pre_entry_exec_authority);
        assert!(!capability.pre_entry_process_create_authority);
        assert!(!capability.recursive_descendant_authority);
        let reason = capability.reason.clone().unwrap();
        assert!(reason.contains("Endpoint Security"), "{reason}");
        assert!(reason.contains("NOTE_TRACK"), "{reason}");

        let operation = if mode == ClosureMode::InventoryTree {
            "inventory"
        } else {
            "run"
        };
        let run = Command::new(&fixture.binary)
            .args([operation, "--policy"])
            .arg(&fixture.policy_path)
            .arg("--receipt")
            .arg(&fixture.receipt_path)
            .status()
            .unwrap();
        assert_eq!(run.code(), Some(78));
        assert!(
            !marker.exists(),
            "unavailable backend launched the root process"
        );
        let receipt = read_receipt(&fixture);
        assert_eq!(receipt.schema, RECEIPT_SCHEMA);
        assert_eq!(receipt.capability, capability);
        assert!(!receipt.complete);
        assert_eq!(receipt.accounting.process_creates, 0);
        assert!(verify(&fixture).success());
    }
}

#[test]
fn leaf_capability_is_kernel_complete() {
    let fixture = fixture("leaf-capability");
    let output = Command::new(&fixture.binary)
        .args(["capability", "leaf"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let capability: Capability = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(capability.backend, "seatbelt+ptrace");
    assert!(capability.available);
    assert!(capability.pre_entry_exec_authority);
    assert!(capability.pre_entry_process_create_authority);
    assert!(capability.recursive_descendant_authority);
    assert!(capability.required_environment.is_empty());
    assert_eq!(capability.reason, None);
}

#[test]
fn leaf_reexec_classifies_every_root_image_before_entry() {
    let fixture = fixture("leaf-reexec");
    let policy = policy(
        &fixture,
        ClosureMode::Leaf,
        vec![
            "exec-image".to_owned(),
            fixture.binary.display().to_string(),
            "exit".to_owned(),
            "3".to_owned(),
        ],
        vec![],
    );
    fs::write(&fixture.policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let status = run(&fixture);
    let receipt = read_receipt(&fixture);
    assert!(status.success(), "{receipt:#?}");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.root_exit_code, Some(3));
    assert_eq!(receipt.accounting.root_execs, 2);
    assert_eq!(receipt.accounting.execs, 2);
    assert_eq!(receipt.accounting.process_creates, 1);
    assert_eq!(receipt.accounting.process_exits, 1);
    let events = events(&fixture, &receipt);
    assert_eq!(events.len(), 4, "{events:#?}");
    let canonical = dunce::canonicalize(&fixture.binary).unwrap();
    for event in &events[1..3] {
        match &event.event {
            ProcessEventKind::Exec { image } => {
                assert_eq!(image.path, canonical);
                assert_eq!(image.class, ImageClass::Fixed);
            }
            other => panic!("expected pre-entry exec image, got {other:?}"),
        }
    }
    assert!(verify(&fixture).success());
}

#[test]
fn leaf_unadmitted_reexec_is_killed_by_the_kernel_before_entry() {
    let fixture = fixture("leaf-unadmitted-reexec");
    let other = fixture.directory.join("unadmitted-image");
    fs::copy(&fixture.binary, &other).unwrap();
    let marker = fixture.directory.join("unadmitted-entered");
    let policy = policy(
        &fixture,
        ClosureMode::Leaf,
        vec![
            "exec-image".to_owned(),
            other.display().to_string(),
            "write-pid-and-sleep-leaf".to_owned(),
            marker.display().to_string(),
        ],
        vec![],
    );
    fs::write(&fixture.policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let status = run(&fixture);
    let receipt = read_receipt(&fixture);
    assert_eq!(status.code(), Some(78), "{receipt:#?}");
    assert!(!receipt.complete, "{receipt:#?}");
    assert!(!marker.exists(), "unadmitted image reached user code");
    assert_eq!(receipt.root_exit_code, Some(137), "{receipt:#?}");
    assert_eq!(receipt.accounting.root_execs, 1);
    assert_eq!(receipt.violation_count, 1, "{receipt:#?}");
    assert!(
        receipt
            .violations
            .iter()
            .any(|value| value.contains("Seatbelt") && value.contains("unadmitted image")),
        "{receipt:#?}"
    );
    let canonical_other = dunce::canonicalize(&other).unwrap();
    let events = events(&fixture, &receipt);
    assert!(
        events.iter().all(|event| !matches!(
            &event.event,
            ProcessEventKind::Exec { image } if image.path == canonical_other
        )),
        "unadmitted image must never produce an exec event: {events:#?}"
    );
    assert!(matches!(
        events[events.len() - 2].event,
        ProcessEventKind::KernelPolicyTermination { .. }
    ));
    assert!(verify(&fixture).success());
}

#[test]
fn leaf_fork_attempt_is_attributed_to_the_sealed_policy() {
    let fixture = fixture("leaf-fork-attempt");
    let policy = policy(
        &fixture,
        ClosureMode::Leaf,
        vec!["spawn-self".to_owned()],
        vec![],
    );
    fs::write(&fixture.policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let status = run(&fixture);
    let receipt = read_receipt(&fixture);
    assert_eq!(status.code(), Some(78), "{receipt:#?}");
    assert!(!receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.root_exit_code, Some(137), "{receipt:#?}");
    assert_eq!(receipt.accounting.process_creates, 1);
    assert_eq!(receipt.accounting.process_exits, 1);
    assert_eq!(receipt.accounting.active_processes, 0);
    assert_eq!(receipt.violation_count, 1, "{receipt:#?}");
    assert!(
        receipt
            .violations
            .iter()
            .any(|value| value.contains("descendant process")),
        "{receipt:#?}"
    );
    assert!(verify(&fixture).success());
}

#[test]
fn supervisor_death_kills_the_traced_root_and_publishes_no_receipt() {
    let fixture = fixture("leaf-teardown");
    let marker = fixture.directory.join("root.pid");
    let policy = policy(
        &fixture,
        ClosureMode::Leaf,
        vec![
            "write-pid-and-sleep-leaf".to_owned(),
            marker.display().to_string(),
        ],
        vec![],
    );
    fs::write(&fixture.policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let mut supervisor = Command::new(&fixture.binary)
        .args(["run", "--policy"])
        .arg(&fixture.policy_path)
        .arg("--receipt")
        .arg(&fixture.receipt_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(marker.exists(), "root never reached user code");
    let root_pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    assert!(process_is_alive(root_pid));

    supervisor.kill().unwrap();
    supervisor.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_is_alive(root_pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(root_pid),
        "traced root {root_pid} survived its supervisor"
    );
    assert!(!fixture.receipt_path.exists());
}

fn process_is_alive(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        // A zombie still answers kill(0); it is dead once its state says so.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                std::mem::size_of::<libc::proc_bsdinfo>() as i32,
            )
        };
        return got as usize == std::mem::size_of::<libc::proc_bsdinfo>()
            && info.pbi_status != libc::SZOMB;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn fixture(label: &str) -> Fixture {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = std::env::temp_dir().join(format!(
        "molt-proof-supervisor-macos-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    Fixture {
        binary,
        policy_path: directory.join("policy.json"),
        receipt_path: directory.join("receipt.json"),
        directory,
    }
}

fn policy(
    fixture: &Fixture,
    mode: ClosureMode,
    fixture_args: Vec<String>,
    extra_images: Vec<FixedImage>,
) -> Policy {
    let mut fixed_images = vec![FixedImage {
        role: "fixture".to_owned(),
        path: fixture.binary.clone(),
        sha256: sha256_file(&fixture.binary).unwrap(),
        root_exit_disposition: RootExitDisposition::RequireExit,
    }];
    fixed_images.extend(extra_images);
    Policy {
        schema: POLICY_SCHEMA.to_owned(),
        nonce: format!(
            "{:032x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
        mode,
        cwd: fixture.directory.clone(),
        command: std::iter::once(fixture.binary.display().to_string())
            .chain(std::iter::once("fixture-child".to_owned()))
            .chain(fixture_args)
            .collect(),
        environment: BTreeMap::new(),
        root_role: "fixture".to_owned(),
        fixed_images,
        derived_roots: Vec::new(),
    }
}

fn run(fixture: &Fixture) -> std::process::ExitStatus {
    Command::new(&fixture.binary)
        .args(["run", "--policy"])
        .arg(&fixture.policy_path)
        .arg("--receipt")
        .arg(&fixture.receipt_path)
        .status()
        .unwrap()
}

fn verify(fixture: &Fixture) -> std::process::ExitStatus {
    Command::new(&fixture.binary)
        .args(["verify", "--policy"])
        .arg(&fixture.policy_path)
        .arg("--receipt")
        .arg(&fixture.receipt_path)
        .status()
        .unwrap()
}

fn read_receipt(fixture: &Fixture) -> Receipt {
    serde_json::from_slice(&fs::read(&fixture.receipt_path).unwrap()).unwrap()
}

fn events(fixture: &Fixture, receipt: &Receipt) -> Vec<ProcessEvent> {
    let file = Path::new(&receipt.event_log.as_ref().unwrap().file).to_path_buf();
    fs::read_to_string(fixture.receipt_path.with_file_name(file))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<ProcessEvent>(line).unwrap())
        .collect()
}
