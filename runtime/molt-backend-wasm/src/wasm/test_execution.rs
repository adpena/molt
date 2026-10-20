//! Test-only execution of compiled WASM through real hosts and linkers.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Honor the explicit SDK linker without promoting its compiler directory to PATH.
pub(crate) fn wasm_ld_path() -> PathBuf {
    std::env::var_os("MOLT_WASM_LD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("wasm-ld"))
}

/// Custody of one directory that `allocate_wasm_test_temp_dir` created
/// exclusively. Only that allocator builds a guard, so a guard never removes
/// a directory another owner made.
pub(in crate::wasm) struct WasmTestTempGuard(PathBuf);

impl Drop for WasmTestTempGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "PRESERVE WASM test execution temp directory after panic: {}",
                self.0.display()
            );
            return;
        }
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!(
                "failed to remove WASM test execution temp directory {}: {error}",
                self.0.display()
            );
        }
    }
}

/// Allocate one test's private execution directory under the temp root.
///
/// A test binary runs its tests as threads of one pid, and the macOS clock
/// ticks in microseconds, so pid and clock alone once named one directory for
/// two tests. `create_dir_all` adopted it, and the first guard removed it
/// under the other test (HF-55). The process sequence makes each name unique
/// in this process, and exclusive creation never adopts a directory that
/// another process, PID namespace or host made under the same name.
pub(in crate::wasm) fn wasm_test_temp_dir() -> (PathBuf, WasmTestTempGuard) {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    allocate_wasm_test_temp_dir(&std::env::temp_dir(), stamp, &SEQUENCE)
}

fn allocate_wasm_test_temp_dir(
    root: &Path,
    stamp: u128,
    sequence: &AtomicU64,
) -> (PathBuf, WasmTestTempGuard) {
    fs::create_dir_all(root).expect("create WASM test execution temp root");
    // The same attempt bound as molt.temporary_artifacts.new_temporary_directory.
    for _attempt in 0..100 {
        let path = root.join(format!(
            "molt-wasm-test-execution-{}-{stamp}-{}",
            std::process::id(),
            sequence.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&path) {
            Ok(()) => return (path.clone(), WasmTestTempGuard(path)),
            // Another owner holds this name: never adopt or remove it.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!(
                "create WASM test execution temp dir {}: {error}",
                path.display()
            ),
        }
    }
    panic!(
        "cannot allocate a unique WASM test execution temp dir under {}",
        root.display()
    )
}

pub(in crate::wasm) fn run_execution_command(command: &mut Command, purpose: &str) {
    let output = execution_output(command, purpose, Duration::from_secs(30))
        .unwrap_or_else(|error| panic!("{purpose}: failed to start: {error}"));
    assert!(
        output.status.success(),
        "{purpose}: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Guest deadlines catch nonterminating emitted instructions; the process
/// deadline also bounds startup and host callbacks outside the guest VM.
pub(in crate::wasm) fn run_node_test_script(
    node: &Path,
    script: &str,
    args: &[&Path],
    purpose: &str,
) {
    run_execution_command(
        &mut node_test_command(node, script, args, purpose, 10_000),
        purpose,
    );
}

fn node_test_command(
    node: &Path,
    script: &str,
    args: &[&Path],
    purpose: &str,
    timeout_ms: u64,
) -> Command {
    const DRIVER: &str = r#"
const vm = require('node:vm');
const [source, purpose, timeout] = process.argv.splice(1, 3);
console.error('WASM execution: ' + purpose);
vm.runInNewContext(source, {require, process, console, Buffer, WebAssembly}, {
  filename: purpose, timeout: Number(timeout), microtaskMode: 'afterEvaluate'
});
"#;
    let mut command = Command::new(node);
    command
        .arg("-e")
        .arg(DRIVER)
        .arg(script)
        .arg(purpose)
        .arg(timeout_ms.to_string())
        .args(args);
    command
}

fn execution_output(
    command: &mut Command,
    purpose: &str,
    timeout: Duration,
) -> std::io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("piped test stdout");
    let stderr = child.stderr.take().expect("piped test stderr");
    fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).map(|_| bytes)
    }
    let stdout = std::thread::spawn(move || read_pipe(stdout));
    let stderr = std::thread::spawn(move || read_pipe(stderr));
    let started = Instant::now();
    let mut failure = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            observation => {
                // This retained Child handle owns only this test subprocess;
                // never discover or signal parents or name-matched processes.
                failure = Some(match observation {
                    Err(error) => format!("child observation failed: {error}"),
                    _ => format!("owned subprocess exceeded {timeout:?}"),
                });
                let status = match child.kill() {
                    Ok(()) => child.wait().expect("reap owned test child"),
                    Err(kill_error) => match child.try_wait() {
                        // Windows termination can race a natural exit. Collect
                        // that exit and both streams before reporting failure.
                        Ok(Some(status)) => status,
                        state => panic!(
                            "{purpose}: owned child {} cleanup indeterminate: kill={kill_error}; wait={state:?}; original={failure:?}",
                            child.id()
                        ),
                    },
                };
                break status;
            }
        }
    };
    let output = Output {
        status,
        stdout: stdout.join().expect("stdout reader").expect("read stdout"),
        stderr: stderr.join().expect("stderr reader").expect("read stderr"),
    };
    assert!(
        failure.is_none(),
        "{purpose}: {}; status={}\nstdout:\n{}\nstderr:\n{}",
        failure.unwrap_or_default(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

#[test]
#[should_panic(expected = "missing execution tool: failed to start:")]
fn missing_execution_tool_fails_instead_of_skipping() {
    let tool = std::env::current_exe()
        .expect("current test executable")
        .with_file_name(format!(
            "molt-missing-execution-tool-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
    assert!(
        !tool.exists(),
        "missing-tool fixture must name an absent executable"
    );
    run_execution_command(&mut Command::new(tool), "missing execution tool");
}

#[test]
fn node_guest_deadline_interrupts_emitted_wasm_loop() {
    let node = PathBuf::from("node");
    // (module (func (export "run") (loop br 0)))
    let script = r#"
const bytes = [0,97,115,109,1,0,0,0,1,4,1,96,0,0,3,2,1,0,
               7,7,1,3,114,117,110,0,0,10,9,1,7,0,3,64,12,0,11,11];
new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.from(bytes))).exports.run();
"#;
    let output = execution_output(
        &mut node_test_command(&node, script, &[], "deadline negative control", 100),
        "deadline negative control",
        Duration::from_secs(5),
    )
    .expect("launch guest deadline negative control");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("ERR_SCRIPT_EXECUTION_TIMEOUT"),
        "guest execution must time out, not fail validation: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn process_deadline_reaps_owned_host_loop_outside_guest_vm() {
    const CHILD: &str = "MOLT_WASM_TEST_PROCESS_DEADLINE_CHILD";
    const READY: &str = "owned native deadline child ready";
    if std::env::var_os(CHILD).is_some() {
        use std::io::Write;

        println!("{READY}");
        std::io::stdout().flush().expect("flush child readiness");
        loop {
            std::thread::park();
        }
    }
    // The process deadline is a native Child-handle contract, independent of
    // Node and its graceful runtime-hook shutdown. Re-exec the admitted test
    // binary so the negative control proves forced termination/reaping without
    // requiring a killed language runtime to send a terminal handshake.
    let failure = std::panic::catch_unwind(|| {
        execution_output(
            Command::new(std::env::current_exe().expect("current test executable"))
                .args([
                    "--exact",
                    "wasm::test_execution::process_deadline_reaps_owned_host_loop_outside_guest_vm",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, "1"),
            "host loop negative control",
            Duration::from_secs(2),
        )
        .expect("launch host deadline negative control")
    })
    .expect_err("host loop must not outlive the process deadline");
    let message = failure
        .downcast_ref::<String>()
        .expect("deadline diagnostic must retain the child outcome");
    assert!(message.contains("owned subprocess exceeded"), "{message}");
    assert!(
        message.contains(READY),
        "deadline must exercise the running child, not just startup: {message}"
    );
}

#[test]
fn equal_clock_readings_allocate_independent_owners() {
    // HF-55: two tests in one pid read one macOS clock tick. Each must own
    // its directory; a finished owner's guard must leave the live one intact.
    let (root, _root) = wasm_test_temp_dir();
    let sequence = AtomicU64::new(0);
    let (finished, finished_guard) = allocate_wasm_test_temp_dir(&root, 7, &sequence);
    let (live, live_guard) = allocate_wasm_test_temp_dir(&root, 7, &sequence);
    assert_ne!(
        finished, live,
        "one clock reading named one directory twice"
    );
    let live_file = live.join("cases.json");
    fs::write(&live_file, b"live owner").expect("write the live owner's file");
    drop(finished_guard);
    assert!(!finished.exists(), "a guard removes its own directory");
    assert_eq!(
        fs::read(&live_file).expect("the live owner's directory must survive"),
        b"live owner"
    );
    drop(live_guard);
    assert!(
        !live.exists(),
        "the owner reclaims its directory when it ends"
    );
}

#[test]
fn allocation_never_adopts_an_existing_directory_with_its_name() {
    // A process in another PID namespace or on another host can carry this
    // pid. Its directory is foreign custody even when the name matches.
    let (root, _root) = wasm_test_temp_dir();
    let (foreign, probe) = allocate_wasm_test_temp_dir(&root, 7, &AtomicU64::new(0));
    drop(probe);
    fs::create_dir(&foreign).expect("create the foreign owner's directory");
    let foreign_file = foreign.join("owner");
    fs::write(&foreign_file, b"foreign owner").expect("write the foreign owner's file");
    // The same inputs name the foreign directory first.
    let (own, guard) = allocate_wasm_test_temp_dir(&root, 7, &AtomicU64::new(0));
    assert_ne!(own, foreign, "allocation adopted an existing directory");
    drop(guard);
    assert!(!own.exists(), "a guard removes its own directory");
    assert_eq!(
        fs::read(&foreign_file).expect("the foreign directory must survive"),
        b"foreign owner"
    );
}
