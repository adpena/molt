use super::*;

mod class_attrs;
mod core;
mod exceptions;
mod numeric_async;
mod repr_collections;

/// Executable proofs use the same PATH-selected image captured by the proof
/// plan. Compiler-only tests never discover or launch optional home tools.
fn execute_lune_oracle(label: &str, source: &str) -> std::process::Output {
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
    assert!(
        output.status.success(),
        "Lune oracle failed: stdout={} stderr={} source={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        path.display(),
    );
    std::fs::remove_file(&path).expect("remove successful Luau oracle");
    output
}
