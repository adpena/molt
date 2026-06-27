//! Fail-closed parity guard for the runtime stdlib in-tree <-> satellite pairs.
//! The PAIRS table contains only domains that still have two live source
//! authorities; completed leaf-only domains are deliberately absent.
//!
//! molt ships every feature-gated stdlib module in TWO physical copies: an
//! in-tree copy under `runtime/molt-runtime/src/builtins/<mod>.rs` (the SOLE
//! compiled source for the reduced build tiers — `--stdlib-profile micro`,
//! `stdlib_edge`, and the WASM feature set) and a satellite copy under
//! `runtime/molt-runtime-X/src/<mod>.rs` (compiled for the DEFAULT native
//! build). The two are the same behavior reached through two access models
//! (direct `crate::` calls vs an `extern "C"` FFI bridge). When a behavioral
//! fix lands in only one copy, shipped behavior DIFFERS BY BUILD TIER — the
//! silent-miscompile bug-class that docs/design/foundation/21 set out to kill.
//!
//! This test runs `tools/check_satellite_parity.py`, which normalizes away the
//! by-design access-layer differences and FAILS on any drift beyond the
//! committed `tools/satellite_parity_baseline.json` ratchet. It is the CI
//! contract that makes new drift a test failure. See the Python script's
//! docstring and `memory/recovery/baton_move_R_satellite_drift.md`.

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
