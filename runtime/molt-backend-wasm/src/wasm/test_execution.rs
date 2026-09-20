use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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

pub(in crate::wasm) fn wasm_test_temp_dir() -> (PathBuf, WasmTestTempGuard) {
    let path = std::env::temp_dir().join(format!(
        "molt-wasm-test-execution-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&path).expect("create WASM test execution temp dir");
    (path.clone(), WasmTestTempGuard(path))
}

pub(in crate::wasm) fn real_execution_tool(
    tool: PathBuf,
    required_env: &str,
    purpose: &str,
) -> Option<PathBuf> {
    let available = execution_output(
        Command::new(&tool).arg("--version"),
        &format!("probe {purpose}: {}", tool.display()),
        Duration::from_secs(5),
    )
    .is_ok_and(|output| output.status.success());
    if available {
        return Some(tool);
    }
    if std::env::var_os("CI").is_some()
        || std::env::var_os(required_env).is_some()
        || std::env::var_os("MOLT_REQUIRE_REAL_NATIVE_CALLABLE_EXECUTION_TESTS").is_some()
    {
        panic!(
            "real {purpose} is required but `{}` is unavailable",
            tool.display()
        );
    }
    eprintln!(
        "SKIP real {purpose}: `{}` is unavailable; set {required_env}=1 to make this a hard failure",
        tool.display()
    );
    None
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
fn node_guest_deadline_interrupts_emitted_wasm_loop() {
    let Some(node) = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "WASM execution deadline",
    ) else {
        return;
    };
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
