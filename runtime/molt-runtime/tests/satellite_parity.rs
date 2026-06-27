//! Fail-closed parity guard for runtime stdlib in-tree <-> satellite pairs.
//!
//! The tracked two-copy stdlib authority class is intentionally extinct:
//! `tools/check_satellite_parity.py` has an empty PAIRS table and
//! `tools/satellite_parity_baseline.json` has a zero residual ceiling. Reduced
//! builds now either compile leaf-owned satellite source by direct include or
//! have no fallback lane.
//!
//! This test keeps the zero-pair invariant executable. If a future change
//! reintroduces an in-tree <-> satellite pair, the Python guard normalizes away
//! by-design access-layer differences and fails on any residual drift beyond the
//! committed ratchet.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = runtime/molt-runtime; the repo root is two up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

#[test]
fn in_tree_and_satellite_stdlib_copies_have_not_drifted() {
    let root = repo_root();
    let guard = root.join("tools").join("check_satellite_parity.py");
    assert!(
        guard.exists(),
        "satellite parity guard missing at {guard:?}"
    );

    let python = std::env::var("PYTHON3").unwrap_or_else(|_| "python3".to_string());
    let output = Command::new(&python)
        .arg(&guard)
        .arg("--verbose")
        .current_dir(&root)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(err) => {
            // A missing python3 must not silently pass — it must fail loud so CI
            // surfaces the unmet prerequisite rather than skipping the guard.
            panic!(
                "failed to spawn `{python}` to run the satellite parity guard \
                 ({guard:?}): {err}. Set PYTHON3 to a working interpreter."
            );
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "satellite parity guard FAILED — the in-tree and satellite copies of a \
         stdlib module diverged beyond the committed baseline. Shipped behavior \
         now differs by build tier. Reconcile BOTH copies and regenerate the \
         baseline with `python3 tools/check_satellite_parity.py \
         --update-baseline`.\n\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}
