//! Exercise hermetic cache fixtures against an adversarial process environment.

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::{Duration, Instant};

#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

#[test]
fn cache_families_do_not_consume_unrelated_process_settings() {
    // Select complete owning modules, including future cases. The owner lives
    // outside those filters, so this child cannot recursively run this test.
    const FAMILIES: &[&str] = &[
        "backend_process::daemon::",
        #[cfg(feature = "native-backend")]
        "backend_process::shared_stdlib_cache::",
        "tests::daemon_cache::",
    ];
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff])
    };
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xd800])
    };
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("cache-environment-isolation")
        .expect("own child evidence");
    let stdout = artifacts.path().join("stdout.log");
    let stderr = artifacts.path().join("stderr.log");
    let mut command = artifacts
        .command(std::env::current_exe().expect("current test image"))
        .expect("child command");
    command
        .args(FAMILIES)
        .args(["--test-threads=1", "--color=never"]);
    // The child starts with stable hostile values. Parent test threads never
    // mutate process settings; ambient-admission controls retain their oracle.
    for key in molt_backend::backend_environment::daemon_request_env_keys() {
        command.env_remove(key.name());
    }
    command
        .env("MOLT_STDLIB_OBJ", &invalid)
        .env("MOLT_BACKEND_BATCH_SIZE", &invalid)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout).expect("retain stdout"))
        .stderr(std::fs::File::create(&stderr).expect("retain stderr"));
    let mut child = command.spawn().expect("start isolated cache families");
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll isolated cache families") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop timed out owned child");
            child.wait().expect("reap timed out owned child");
            panic!(
                "cache families timed out; evidence: {}",
                artifacts.path().display()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "isolated cache families failed ({status}); evidence: {}",
        artifacts.path().display()
    );
    let mut exercised = [false; FAMILIES.len()];
    for line in BufReader::new(std::fs::File::open(stdout).expect("read child results")).lines() {
        let line = line.expect("read child row");
        if let Some(name) = line
            .strip_prefix("test ")
            .and_then(|row| row.strip_suffix(" ... ok"))
        {
            for (index, family) in FAMILIES.iter().enumerate() {
                exercised[index] |= name.starts_with(*family);
            }
        }
    }
    assert!(
        exercised.into_iter().all(|value| value),
        "a cache family ran no tests"
    );
}
