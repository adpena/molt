use super::*;

mod class_attrs;
mod core;
mod exceptions;
mod numeric_async;
mod pipeline;
mod repr_collections;
mod singleton_values;

#[derive(Debug)]
enum LuneStdout {
    Contains(&'static [&'static str]),
    TrimmedExact(&'static str),
    TrimmedPrefix(&'static str),
}

/// Executable proofs use the same PATH-selected image captured by the proof
/// plan. Compiler-only tests never discover or launch optional home tools.
fn execute_lune_oracle(
    label: &str,
    source: &str,
    expected_stdout: LuneStdout,
) -> std::process::Output {
    let path = std::env::temp_dir().join(format!("molt_luau_{label}_{}.luau", std::process::id()));
    std::fs::write(&path, source).expect("write executable Luau oracle");
    let output = std::process::Command::new("lune")
        .arg("run")
        .arg(&path)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "required proof-plan Lune runner failed: {error}; oracle: {}",
                path.display()
            )
        });
    let stdout_matches =
        std::str::from_utf8(&output.stdout).is_ok_and(|stdout| match &expected_stdout {
            LuneStdout::Contains(fragments) => {
                !fragments.is_empty()
                    && fragments
                        .iter()
                        .all(|fragment| !fragment.is_empty() && stdout.contains(*fragment))
            }
            LuneStdout::TrimmedExact(expected) => {
                !expected.is_empty() && stdout.trim() == *expected
            }
            LuneStdout::TrimmedPrefix(expected) => {
                !expected.is_empty() && stdout.trim().starts_with(*expected)
            }
        });
    // Keep the complete generated program when either execution or its actual
    // output oracle fails; deletion commits only after both have succeeded.
    assert!(
        output.status.success() && stdout_matches,
        "Lune oracle failed (status={} expected_stdout={expected_stdout:?}): stdout={} stderr={} source={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        path.display(),
    );
    std::fs::remove_file(&path).expect("remove successful Luau oracle");
    output
}
