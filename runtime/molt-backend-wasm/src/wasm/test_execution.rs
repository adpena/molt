use std::fs;
use std::path::PathBuf;
use std::process::Command;

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
    let available = Command::new(&tool)
        .arg("--version")
        .output()
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
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{purpose}: failed to start: {error}"));
    assert!(
        output.status.success(),
        "{purpose}: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
